pub mod client;

use crate::renderer::WgpuState;
use smithay::reexports::calloop::LoopSignal;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::compositor::CompositorState;

pub struct SpatilState {
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub wgpu_state: WgpuState,
    pub compositor_state: CompositorState,
}

impl SpatilState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_signal: LoopSignal,
        wgpu_state: WgpuState,
    ) -> Self {
        let compositor_state = CompositorState::new_v6::<Self>(&display_handle);
        Self {
            display_handle,
            loop_signal,
            wgpu_state,
            compositor_state,
        }
    }
}
