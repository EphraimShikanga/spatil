use std::sync::Arc;

use smithay::reexports::calloop::EventLoop as CalloopEventLoop;
use smithay::reexports::wayland_server::DisplayHandle;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::backend;
use crate::config::SpatilConfig;
use crate::renderer::WgpuState;
use crate::state::SpatilState;

struct SpatilApp {
    state: Option<SpatilState>,
    window: Option<Arc<Window>>,
    // Backend-owned (not WgpuState) because acquire/present is winit-specific.
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    needs_reconfigure: bool,
    display_handle: DisplayHandle,
    calloop: CalloopEventLoop<'static, SpatilState>,
    config: SpatilConfig,
    _config_watcher: Option<notify::RecommendedWatcher>,
}

impl ApplicationHandler for SpatilApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let window_attributes = Window::default_attributes().with_title("Spatil Compositor");
            let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
            self.window = Some(window.clone());

            let (wgpu_state, surface, surface_config) =
                pollster::block_on(WgpuState::new_winit(window, &self.config));
            self.surface = Some(surface);
            self.surface_config = Some(surface_config);

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
                if let (Some(surface), Some(surface_config)) =
                    (&self.surface, &mut self.surface_config)
                {
                    surface_config.width = size.width.max(1);
                    surface_config.height = size.height.max(1);
                    surface.configure(&state.wgpu_state.device, surface_config);
                    state.wgpu_state.resize(size.width, size.height);
                }
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

                let surface = self.surface.as_ref().unwrap();
                let surface_config = self.surface_config.as_mut().unwrap();

                let acquired = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t) => Some(t),
                    wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                        // Drawable this frame; reconfigure after present.
                        self.needs_reconfigure = true;
                        Some(t)
                    }
                    wgpu::CurrentSurfaceTexture::Outdated => {
                        surface.configure(&state.wgpu_state.device, surface_config);
                        None
                    }
                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded
                    | wgpu::CurrentSurfaceTexture::Validation => None,
                    wgpu::CurrentSurfaceTexture::Lost => {
                        surface.configure(&state.wgpu_state.device, surface_config);
                        state
                            .wgpu_state
                            .resize(surface_config.width, surface_config.height);
                        None
                    }
                };

                if let Some(surface_texture) = acquired {
                    let view = surface_texture
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    state.wgpu_state.render(&mut state.world, &view);
                    state.wgpu_state.queue.present(surface_texture);
                    state.send_frame_callbacks();

                    if self.needs_reconfigure {
                        self.needs_reconfigure = false;
                        surface.configure(&state.wgpu_state.device, surface_config);
                    }
                }

                self.calloop
                    .dispatch(Some(std::time::Duration::from_millis(1)), state)
                    .unwrap();
                state.display_handle.flush_clients().unwrap();

                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => (),
        }
    }
}

pub fn run(config: SpatilConfig) {
    let event_loop = EventLoop::new().unwrap();
    let backend::Harness {
        calloop,
        display_handle,
        config_watcher,
    } = backend::harness(&config);

    let mut app = SpatilApp {
        state: None,
        window: None,
        surface: None,
        surface_config: None,
        needs_reconfigure: false,
        display_handle,
        calloop,
        config,
        _config_watcher: config_watcher,
    };

    event_loop.run_app(&mut app).unwrap();
}
