use smithay::delegate_seat;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::SpatilState;

impl SeatHandler for SpatilState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<SpatilState> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&WlSurface>) {
        // The focus outline is drawn from this; the seat is the one place that
        // tracks focus.
        self.wgpu_state.focused_surface = focused.map(|s| s.id());

        // Keep each toplevel's `Activated` flag in sync with keyboard focus, so
        // clients render their focused state (e.g. a solid vs hollow cursor).
        for toplevel in &self.toplevels {
            let is_focused = focused == Some(toplevel.wl_surface());
            toplevel.with_pending_state(|state| {
                if is_focused {
                    state.states.set(xdg_toplevel::State::Activated);
                } else {
                    state.states.unset(xdg_toplevel::State::Activated);
                }
            });
            toplevel.send_pending_configure();
        }
    }
}

delegate_seat!(SpatilState);
