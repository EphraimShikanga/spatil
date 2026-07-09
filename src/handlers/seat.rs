use smithay::delegate_seat;
use smithay::input::{SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::SpatilState;

impl SeatHandler for SpatilState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<SpatilState> {
        &mut self.seat_state
    }
}

delegate_seat!(SpatilState);
