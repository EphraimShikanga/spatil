pub mod client;

use crate::renderer::WgpuState;
use smithay::input::{Seat, SeatState};
use smithay::reexports::calloop::LoopSignal;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState};
use smithay::wayland::shm::ShmState;

pub struct SpatilState {
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub wgpu_state: WgpuState,
    pub compositor_state: CompositorState,
    pub shm_state: ShmState,
    pub xdg_shell_state: XdgShellState,
    /// Registry of live toplevel windows, for lookup by `WlSurface`.
    ///
    /// This is deliberately NOT `smithay::desktop::Space`/`Window`: it carries no
    /// 2D stacking/layout semantics. It's a flat "which toplevels exist" list that
    /// the 3D scene graph will eventually absorb once scene nodes exist.
    pub toplevels: Vec<ToplevelSurface>,
    /// Minimal seat foundation. xdg-shell's popup-grab dispatch requires
    /// `SeatHandler`, and real clients need a `wl_seat` to receive input, so a
    /// seat is created here. Input routing itself is not yet wired (no keyboard/
    /// pointer added); that is future work.
    pub seat_state: SeatState<SpatilState>,
    pub seat: Seat<SpatilState>,
}

impl SpatilState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_signal: LoopSignal,
        wgpu_state: WgpuState,
    ) -> Self {
        let compositor_state = CompositorState::new_v6::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(&display_handle, "seat0");
        Self {
            display_handle,
            loop_signal,
            wgpu_state,
            compositor_state,
            shm_state,
            xdg_shell_state,
            toplevels: Vec::new(),
            seat_state,
            seat,
        }
    }
}
