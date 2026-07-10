use smithay::delegate_xdg_shell;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::utils::{Serial, SERIAL_COUNTER};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::state::SpatilState;

impl XdgShellHandler for SpatilState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Suggest an initial size and mark the window active, then send the first
        // configure. The client must ack this before it may attach a buffer that
        // rule is enforced in `CompositorHandler::commit`
        surface.with_pending_state(|state| {
            state.size = Some((800, 600).into());
            state.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();

        // Register the toplevel so `commit` can find it. This is a flat object
        // registry, not a 2D stacking structure the 3D scene graph will absorb it.
        self.toplevels.push(surface.clone());

        // Give the new window keyboard focus so the user can type into it right
        // away. Pointer focus follows the cursor via motion events.
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(surface.wl_surface().clone()), serial);
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.toplevels
            .retain(|t| t.wl_surface() != surface.wl_surface());
        self.wgpu_state.remove_texture(&surface.wl_surface().id());
    }

    // Popups: not yet supported. These are required by the trait, so they're
    // stubbed until popup positioning lands.

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        tracing::warn!("xdg_popup requested but popups are not yet supported");
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        tracing::warn!("xdg_popup grab requested but popups are not yet supported");
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        tracing::warn!("xdg_popup reposition requested but popups are not yet supported");
    }
}

delegate_xdg_shell!(SpatilState);
