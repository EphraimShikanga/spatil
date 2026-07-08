mod client;
mod compositor;
mod renderer;
mod state;

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};
use smithay::reexports::calloop::EventLoop as CalloopEventLoop;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::wayland::socket::ListeningSocketSource;

use client::ClientState;
use renderer::WgpuState;
use state::SpatilState;

struct SpatilApp {
    state: Option<SpatilState>,
    window: Option<Arc<Window>>,
    display_handle: DisplayHandle,
    calloop: CalloopEventLoop<'static, SpatilState>,
}

impl ApplicationHandler for SpatilApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let window_attributes = Window::default_attributes().with_title("Spatil Compositor");
            let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
            self.window = Some(window.clone());

            let wgpu_state = pollster::block_on(WgpuState::new(window));

            let state = SpatilState::new(
                self.display_handle.clone(),
                self.calloop.get_signal(),
                wgpu_state,
            );

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
            WindowEvent::RedrawRequested => {
                match state.wgpu_state.render() {
                    Ok(_) => {}
                    Err(wgpu::SurfaceError::Lost) => state.wgpu_state.resize(winit::dpi::PhysicalSize::new(state.wgpu_state.config.width.max(1), state.wgpu_state.config.height.max(1))),
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    Err(e) => eprintln!("{:?}", e),
                }
                
                // Dispatch wayland clients and flush any queued outgoing events
                self.calloop.dispatch(Some(std::time::Duration::from_millis(1)), state).unwrap();
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

    let event_loop = EventLoop::new().unwrap();
    let calloop = CalloopEventLoop::<'static, SpatilState>::try_new().unwrap();

    let wayland_display = Display::<SpatilState>::new().unwrap();
    let display_handle = wayland_display.handle();

    // Dispatch already-connected clients every calloop iteration, not just on redraw.
    let display_source = Generic::new(wayland_display, Interest::READ, Mode::Level);
    calloop
        .handle()
        .insert_source(display_source, |_, display, state: &mut SpatilState| {
            // SAFETY: this Generic source owns the Display for its entire life.
            unsafe { display.get_mut() }.dispatch_clients(state).unwrap();
            Ok(PostAction::Continue)
        })
        .expect("failed to init the wayland display event source");

    let listening_socket = ListeningSocketSource::new_auto().expect("failed to init wayland socket");
    let socket_name = listening_socket.socket_name().to_string_lossy().into_owned();
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
    };

    event_loop.run_app(&mut app).unwrap();
}
