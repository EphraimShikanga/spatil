mod config;
mod handlers;
mod input;
mod ipc;
mod renderer;
mod scene;
mod state;

use smithay::reexports::calloop::EventLoop as CalloopEventLoop;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::wayland::socket::ListeningSocketSource;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use config::SpatilConfig;
use renderer::WgpuState;
use state::SpatilState;
use state::client::ClientState;

struct SpatilApp {
    state: Option<SpatilState>,
    window: Option<Arc<Window>>,
    display_handle: DisplayHandle,
    calloop: CalloopEventLoop<'static, SpatilState>,
    config: SpatilConfig,
    /// Kept alive to hold the inotify watch open; dropping it stops hot-reload.
    _config_watcher: Option<notify::RecommendedWatcher>,
}

impl ApplicationHandler for SpatilApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let window_attributes = Window::default_attributes().with_title("Spatil Compositor");
            let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
            self.window = Some(window.clone());

            let wgpu_state = pollster::block_on(WgpuState::new(window, &self.config));

            let state = SpatilState::new(
                self.display_handle.clone(),
                self.calloop.handle(),
                self.calloop.get_signal(),
                wgpu_state,
                self.config.clone(),
            );

            state.run_autostart();
            self.state = Some(state);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let state = if let Some(state) = &mut self.state {
            state
        } else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                state.wgpu_state.resize(size);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                state.handle_keyboard(event);
                // winit owns the thread, so a `quit` action exits its loop here.
                if state.should_quit {
                    event_loop.exit();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.handle_pointer_motion(position);
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                state.handle_pointer_button(button, btn_state);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                state.handle_pointer_axis(delta);
            }
            WindowEvent::RedrawRequested => {
                state.reveal_pending();
                state.tick_animations();

                match state.wgpu_state.render(&mut state.world) {
                    renderer::RenderOutcome::Rendered => {
                        // Unblock every client waiting on wl_surface.frame(): a
                        // frame was just presented, so tell them it's safe to
                        // draw the next one. Snapshot output/time into locals
                        // first so the closure doesn't need to borrow `state`
                        // while `&state.toplevels` is already borrowed.
                        let output = state.output.clone();
                        let elapsed = state.start_time.elapsed();
                        for toplevel in &state.toplevels {
                            smithay::desktop::utils::send_frames_surface_tree(
                                toplevel.wl_surface(),
                                &output,
                                elapsed,
                                Some(std::time::Duration::ZERO),
                                |_, _| Some(output.clone()),
                            );
                        }
                    }
                    renderer::RenderOutcome::Skipped => {}
                    renderer::RenderOutcome::Lost => {
                        state.wgpu_state.resize(winit::dpi::PhysicalSize::new(
                            state.wgpu_state.config.width.max(1),
                            state.wgpu_state.config.height.max(1),
                        ));
                    }
                }

                // Dispatch wayland clients and flush any queued outgoing events
                self.calloop
                    .dispatch(Some(std::time::Duration::from_millis(1)), state)
                    .unwrap();
                state.display_handle.flush_clients().unwrap();

                // Request next frame
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => (),
        }
    }
}

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Spatil Compositor");

    // Ignoring SIGCHLD makes the kernel auto-reap children, so apps launched by
    // `exec` keybindings don't pile up as zombies.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }

    config::bootstrap_config_dir();
    let config = SpatilConfig::load();

    // Autostart entries and keybinding `exec`s inherit this so hooks find the
    // socket without hardcoding its name.
    unsafe {
        std::env::set_var("SPATIL_SOCKET", &config.general.socket_name);
    }

    let event_loop = EventLoop::new().unwrap();
    let calloop = CalloopEventLoop::<'static, SpatilState>::try_new().unwrap();

    let (config_tx, config_channel) =
        smithay::reexports::calloop::channel::channel::<SpatilConfig>();
    calloop
        .handle()
        .insert_source(config_channel, |event, _, state: &mut SpatilState| {
            if let smithay::reexports::calloop::channel::Event::Msg(new_config) = event {
                state.apply_config(new_config);
            }
        })
        .expect("failed to insert the config-reload event source");
    let config_watcher = config::spawn_config_watcher(config_tx);

    if let Err(err) = ipc::init_ipc(&calloop.handle(), &config) {
        tracing::warn!("failed to start IPC control socket: {err}; scripting disabled");
    }

    let wayland_display = Display::<SpatilState>::new().unwrap();
    let display_handle = wayland_display.handle();

    // Dispatch already-connected clients every calloop iteration, not just on redraw.
    let display_source = Generic::new(wayland_display, Interest::READ, Mode::Level);
    calloop
        .handle()
        .insert_source(display_source, |_, display, state: &mut SpatilState| {
            // SAFETY: this Generic source owns the Display for its entire life.
            unsafe { display.get_mut() }
                .dispatch_clients(state)
                .unwrap();
            Ok(PostAction::Continue)
        })
        .expect("failed to init the wayland display event source");

    let listening_socket =
        ListeningSocketSource::new_auto().expect("failed to init wayland socket");
    let socket_name = listening_socket
        .socket_name()
        .to_string_lossy()
        .into_owned();
    tracing::info!("Listening on WAYLAND_DISPLAY={socket_name}");
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket_name);
    }

    calloop
        .handle()
        .insert_source(listening_socket, |stream, _, state: &mut SpatilState| {
            if let Err(err) = state
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                tracing::warn!("Error adding wayland client: {err}");
            }
        })
        .expect("failed to init the wayland socket source");

    let mut app = SpatilApp {
        state: None,
        window: None,
        display_handle,
        calloop,
        config,
        _config_watcher: config_watcher,
    };

    event_loop.run_app(&mut app).unwrap();
}
