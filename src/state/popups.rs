use hecs::Entity;
use smithay::desktop::{PopupKind, PopupManager, find_popup_root_surface};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{get_role, with_states};
use smithay::wayland::shell::xdg::{SurfaceCachedState, XDG_POPUP_ROLE, XdgPopupSurfaceData};

use crate::scene::components::{POPUP_WINDOW_ID, Popup, Transform, WaylandSurface};
use crate::state::SpatilState;

fn geometry_offset(surface: &WlSurface) -> (i32, i32) {
    let loc = with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .unwrap_or_default()
            .loc
    });
    (loc.x, loc.y)
}

fn popup_depth(popup: &PopupKind) -> u32 {
    let PopupKind::Xdg(surface) = popup else {
        return 0;
    };
    let mut depth = 0;
    let mut parent = surface.get_parent_surface();
    while let Some(current) = parent {
        if get_role(&current) != Some(XDG_POPUP_ROLE) {
            break;
        }
        depth += 1;
        parent = with_states(&current, |states| {
            states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .and_then(|data| data.lock().ok().and_then(|d| d.parent.clone()))
        });
    }
    depth
}

impl SpatilState {
    pub fn spawn_popup(&mut self, popup: &PopupKind) {
        let Ok(root_surface) = find_popup_root_surface(popup) else {
            return;
        };
        let Some(root) = self.entity_for_surface(&root_surface.id()) else {
            tracing::warn!("popup mapped on a surface with no scene entity; ignoring");
            return;
        };
        let surface = popup.wl_surface().clone();
        let entity = self.world.spawn((
            Transform::default(),
            WaylandSurface {
                visible: true,
                ..WaylandSurface::new(surface, POPUP_WINDOW_ID)
            },
            Popup {
                root,
                depth: popup_depth(popup),
                offset: (0, 0),
            },
        ));
        tracing::debug!(?entity, "spawned popup entity");
        self.refresh_popup_offsets(&root_surface);
    }

    pub fn refresh_popup_offsets(&mut self, root: &WlSurface) {
        let root_offset = geometry_offset(root);
        let offsets: Vec<(ObjectId, (i32, i32))> = PopupManager::popups_for_surface(root)
            .map(|(popup, location)| {
                let geometry = popup.geometry().loc;
                (
                    popup.wl_surface().id(),
                    (
                        root_offset.0 + location.x - geometry.x,
                        root_offset.1 + location.y - geometry.y,
                    ),
                )
            })
            .collect();
        if offsets.is_empty() {
            return;
        }
        for (_entity, (popup, surface)) in self.world.query_mut::<(&mut Popup, &WaylandSurface)>() {
            if let Some((_, offset)) = offsets.iter().find(|(id, _)| *id == surface.id) {
                popup.offset = *offset;
            }
        }
    }

    pub fn despawn_popup(&mut self, surface_id: &ObjectId) {
        if let Some(entity) = self.entity_for_surface(surface_id) {
            let _ = self.world.despawn(entity);
        }
        self.wgpu_state.remove_texture(surface_id);
    }

    pub fn despawn_popups_of(&mut self, root: Entity) {
        let doomed: Vec<(Entity, ObjectId)> = self
            .world
            .query::<(&Popup, &WaylandSurface)>()
            .iter()
            .filter(|(_, (popup, _))| popup.root == root)
            .map(|(entity, (_, surface))| (entity, surface.id.clone()))
            .collect();
        for (entity, id) in doomed {
            let _ = self.world.despawn(entity);
            self.wgpu_state.remove_texture(&id);
        }
    }

    pub fn window_entity(&self, entity: Entity) -> Entity {
        self.world
            .get::<&Popup>(entity)
            .map(|popup| popup.root)
            .unwrap_or(entity)
    }

    pub fn root_surface(&self, surface: &WlSurface) -> WlSurface {
        self.popups
            .find_popup(surface)
            .and_then(|popup| find_popup_root_surface(&popup).ok())
            .unwrap_or_else(|| surface.clone())
    }

    pub fn popup_render_root(&self, surface: &WlSurface) -> Option<WlSurface> {
        if self.popups.find_popup(surface).is_some() {
            return Some(surface.clone());
        }
        let candidates: Vec<WlSurface> = self
            .world
            .query::<(&Popup, &WaylandSurface)>()
            .iter()
            .map(|(_, (_, s))| s.surface.clone())
            .collect();
        candidates
            .into_iter()
            .find(|popup| crate::handlers::compositor::tree_contains(popup, surface))
    }
}
