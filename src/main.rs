mod renderer;
mod state;

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};
use wayland_server::Display;
use smithay::reexports::calloop::EventLoop as CalloopEventLoop;

use renderer::WgpuState;
use state::SpatilState;

struct SpatilApp {
    state: Option<SpatilState>,
    window: Option<Arc<Window>>,
    wayland_display: Display<SpatilState>,
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
                self.wayland_display.handle(),
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
                
                // Dispatch wayland clients
                self.calloop.dispatch(Some(std::time::Duration::from_millis(1)), state).unwrap();
                self.wayland_display.dispatch_clients(state).unwrap();
                
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

    let mut app = SpatilApp {
        state: None,
        window: None,
        wayland_display,
        calloop,
    };

    event_loop.run_app(&mut app).unwrap();
}
