use smithay::delegate_seat;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use smithay::wayland::selection::data_device::set_data_device_focus;

use crate::state::SpatilState;

impl SeatHandler for SpatilState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<SpatilState> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let focused = focused.map(|surface| self.root_surface(surface));
        self.wgpu_state.focused_surface = focused.as_ref().map(|s| s.id());

        let client = focused.as_ref().and_then(|surface| {
            self.display_handle.get_client(surface.id()).ok()
        });
        set_data_device_focus(&self.display_handle.clone(), seat, client);

        for toplevel in &self.toplevels {
            let is_focused = focused.as_ref() == Some(toplevel.wl_surface());
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
