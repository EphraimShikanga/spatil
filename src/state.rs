use crate::renderer::WgpuState;
use smithay::reexports::calloop::LoopSignal;
use wayland_server::DisplayHandle;

pub struct SpatilState {
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub wgpu_state: WgpuState,
}

impl SpatilState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_signal: LoopSignal,
        wgpu_state: WgpuState,
    ) -> Self {
        Self {
            display_handle,
            loop_signal,
            wgpu_state,
        }
    }
}
