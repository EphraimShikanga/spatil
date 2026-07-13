use cgmath::Vector3;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;

use super::protocol::SpatilCommand;
use crate::scene::components::{Transform, WaylandSurface};
use crate::state::SpatilState;

impl SpatilState {
    pub(super) fn apply_ipc_command(&mut self, cmd: SpatilCommand) {
        match cmd.command.as_str() {
            "move" | "move_cluster" => {
                let Some(window_id) = cmd.window_id else {
                    tracing::warn!("move_cluster needs a window_id; got {cmd:?}");
                    return;
                };
                let [x, y, z] = cmd.args[..] else {
                    tracing::warn!("move_cluster needs 3 args [x,y,z]; got {cmd:?}");
                    return;
                };
                if !self.set_window_transform(window_id, Vector3::new(x, y, z)) {
                    tracing::warn!(window_id, "move_cluster for unknown window_id");
                }
            }
            "show" | "map" => match cmd.window_id {
                Some(window_id) if self.reveal_window(window_id) => {}
                Some(window_id) => tracing::warn!(window_id, "show for unknown window_id"),
                None => tracing::warn!("show command needs a window_id"),
            },
            "close" => match cmd.window_id {
                Some(window_id) if self.close_window_by_id(window_id) => {}
                Some(window_id) => tracing::warn!(window_id, "close for unknown window_id"),
                None => tracing::warn!("close command needs a window_id"),
            },
            other => tracing::warn!("ignoring unknown IPC command: {other:?}"),
        }
    }

    /// Positions a window and reveals it (ends map-postponement).
    fn set_window_transform(&mut self, window_id: u32, translation: Vector3<f32>) -> bool {
        for (_entity, (transform, surface)) in
            self.world.query_mut::<(&mut Transform, &mut WaylandSurface)>()
        {
            if surface.window_id == window_id {
                transform.translation = translation;
                surface.visible = true;
                return true;
            }
        }
        false
    }

    fn reveal_window(&mut self, window_id: u32) -> bool {
        for (_entity, surface) in self.world.query_mut::<&mut WaylandSurface>() {
            if surface.window_id == window_id {
                surface.visible = true;
                return true;
            }
        }
        false
    }

    fn close_window_by_id(&mut self, window_id: u32) -> bool {
        let Some(surface_id) = self.surface_id_for_window(window_id) else {
            return false;
        };
        match self.toplevels.iter().find(|t| t.wl_surface().id() == surface_id) {
            Some(toplevel) => {
                toplevel.send_close();
                true
            }
            None => false,
        }
    }

    fn surface_id_for_window(&self, window_id: u32) -> Option<ObjectId> {
        self.world
            .query::<&WaylandSurface>()
            .iter()
            .find(|(_, s)| s.window_id == window_id)
            .map(|(_, s)| s.id.clone())
    }
}
