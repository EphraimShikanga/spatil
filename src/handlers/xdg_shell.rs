use smithay::delegate_xdg_shell;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::utils::{Serial, SERIAL_COUNTER};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::state::SpatilState;
use crate::scene::animation::{self, AnimSnapshot, Easing, OnComplete};
use crate::scene::components::{Billboard, Transform, WaylandSurface, WindowState};
use crate::scene::mode::{SceneMode, default_transform};

impl XdgShellHandler for SpatilState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // The client must ack this before it may
        // attach a buffer that rule is enforced in `CompositorHandler::commit`
        let default_size = (
            self.config.windows.default_width,
            self.config.windows.default_height,
        );
        surface.with_pending_state(|state| {
            state.size = Some(default_size.into());
            state.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();

        // Register the toplevel so `commit` can find it. This is a flat object
        // registry, not a 2D stacking structure the 3D scene graph will absorb it.
        self.toplevels.push(surface.clone());

        let window_id = self.next_window_id();
        let mode = self.config.scene_graph.scene_mode();
        let facing = self.wgpu_state.facing_rotation(mode);
        let mut transform = default_transform(mode, window_id, &self.config.scene_graph, facing);
        if let Some(spawn) = self.cursor_spawn_point() {
            transform.translation = spawn;
        }
        let entity = self.world.spawn((
            transform,
            WaylandSurface::new(surface.wl_surface().id(), window_id),
        ));
        if mode == SceneMode::Aether {
            let _ = self.world.insert_one(entity, Billboard);
        }

        // Give the new window keyboard focus so the user can type into it right
        // away. Pointer focus follows the cursor via motion events.
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(surface.wl_surface().clone()), serial);
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let id_to_remove = surface.wl_surface().id();
        self.toplevels
            .retain(|t| t.wl_surface().id() != id_to_remove);
            
        let mut entity_to_close = None;
        let mut closed_window_id = None;
        for (entity, wayland_surface) in self.world.query::<&WaylandSurface>().iter() {
            if wayland_surface.id == id_to_remove {
                entity_to_close = Some(entity);
                closed_window_id = Some(wayland_surface.window_id);
                break;
            }
        }

        if let Some(entity) = entity_to_close {
            if self.config.animation.enabled {
                // The entity outlives its destroyed Wayland surface to play the
                // exit animation so the despawn + texture release happen when it ends.
                let mode = self.config.scene_graph.scene_mode();
                let close_ms = self.config.animation.close_ms;
                if let Ok(surface) = self.world.query_one_mut::<&mut WaylandSurface>(entity) {
                    surface.closing = true;
                }
                if let Some(resting) = self
                    .world
                    .get::<&Transform>(entity)
                    .ok()
                    .map(|t| AnimSnapshot::of(&t))
                {
                    let end = animation::close_end(mode, &resting);
                    animation::tween_entity(
                        &mut self.world,
                        entity,
                        resting,
                        end,
                        close_ms,
                        Easing::EaseInOutCubic,
                        OnComplete::Despawn,
                    );
                }
            } else {
                let _ = self.world.despawn(entity);
                self.wgpu_state.remove_texture(&id_to_remove);
            }
        }

        // Let scripts react to the window going away.
        if let Some(window_id) = closed_window_id {
            self.broadcast_event(&crate::ipc::SpatilEvent::new("window_closed", window_id));
        }
    }

    // Clients fullscreen themselves, so this path must work without our keybinding.
    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        match self.entity_for_surface(&surface.wl_surface().id()) {
            Some(entity) => self.raise_window(entity, WindowState::Fullscreen),
            // No scene entity yet: the client is still owed a configure or it hangs.
            None => {
                surface.send_configure();
            }
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        match self.entity_for_surface(&surface.wl_surface().id()) {
            Some(entity) => self.return_to_ambient(entity),
            None => {
                surface.send_configure();
            }
        }
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
