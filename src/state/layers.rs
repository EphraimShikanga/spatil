use smithay::desktop::{PopupManager, WindowSurfaceType, layer_map_for_output};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{
    KeyboardInteractivity, Layer as WlrLayer, LayerSurfaceData,
};

use crate::renderer::hud::HudQuad;
use crate::state::SpatilState;

const HIT_ORDER: [WlrLayer; 2] = [WlrLayer::Overlay, WlrLayer::Top];
const DRAW_ORDER: [WlrLayer; 2] = [WlrLayer::Top, WlrLayer::Overlay];

impl SpatilState {
    /// The texture key a commit belongs to, if `surface` is a layer surface or one of
    /// its subsurfaces.
    pub fn layer_commit_target(&mut self, surface: &WlSurface) -> Option<ObjectId> {
        let layer_surface = {
            let mut map = layer_map_for_output(&self.output);
            map.arrange();
            map.layer_for_surface(
                surface,
                WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
            )?
            .layer_surface()
            .clone()
        };

        let root = layer_surface.wl_surface().clone();
        let configured = with_states(&root, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .is_some_and(|data| data.lock().unwrap().initial_configure_sent)
        });
        // `arrange` deliberately never sends the initial configure: the protocol
        // requires it in response to the first commit.
        if !configured {
            layer_surface.send_configure();
            self.refresh_layer_focus();
        }

        Some(root.id())
    }

    /// Resolves mapped layer surfaces and their popups into screen-pixel HUD quads,
    /// back-to-front. Run once per frame before rendering.
    pub fn update_hud_layout(&mut self) {
        let mut layout: Vec<HudQuad> = Vec::new();
        {
            let mut map = layer_map_for_output(&self.output);
            map.cleanup();
            for layer in DRAW_ORDER {
                for mapped in map.layers_on(layer) {
                    let Some(geo) = map.layer_geometry(mapped) else {
                        continue;
                    };
                    let surface = mapped.wl_surface();
                    let origin = (geo.loc.x as f32, geo.loc.y as f32);
                    if let Some(rect) = self.hud_rect(&surface.id(), origin) {
                        layout.push(HudQuad { id: surface.id(), rect });
                    }
                    for (popup, location) in PopupManager::popups_for_surface(surface) {
                        let offset = location - popup.geometry().loc;
                        let id = popup.wl_surface().id();
                        let origin = (
                            (geo.loc.x + offset.x) as f32,
                            (geo.loc.y + offset.y) as f32,
                        );
                        if let Some(rect) = self.hud_rect(&id, origin) {
                            layout.push(HudQuad { id, rect });
                        }
                    }
                }
            }
        }
        self.wgpu_state.hud_layout = layout;
    }

    fn hud_rect(&self, id: &ObjectId, origin: (f32, f32)) -> Option<[f32; 4]> {
        let (_, quad) = self.wgpu_state.quads.iter().find(|(qid, _)| qid == id)?;
        Some([origin.0, origin.1, quad.width as f32, quad.height as f32])
    }

    /// The HUD surface under the cursor, and the point in its own logical pixels.
    pub fn layer_surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let map = layer_map_for_output(&self.output);
        for layer in HIT_ORDER {
            let Some(mapped) = map.layer_under(layer, point) else {
                continue;
            };
            let Some(geo) = map.layer_geometry(mapped) else {
                continue;
            };
            let local = point - geo.loc.to_f64();
            if let Some((surface, offset)) = mapped.surface_under(local, WindowSurfaceType::ALL) {
                return Some((surface, local - offset.to_f64()));
            }
        }
        None
    }

    pub fn layer_accepts_focus(&self, surface: &WlSurface) -> bool {
        let map = layer_map_for_output(&self.output);
        map.layer_for_surface(surface, WindowSurfaceType::ALL)
            .is_some_and(|layer| layer.can_receive_keyboard_focus())
    }

    /// The topmost surface demanding `Exclusive` keyboard interactivity, if any.
    pub fn exclusive_layer(&self) -> Option<WlSurface> {
        let map = layer_map_for_output(&self.output);
        for layer in HIT_ORDER {
            let found = map.layers_on(layer).rev().find(|mapped| {
                mapped.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
            });
            if let Some(mapped) = found {
                return Some(mapped.wl_surface().clone());
            }
        }
        None
    }

    /// Hands the keyboard to an exclusive layer surface, or recovers focus when the
    /// surface holding it has died.
    pub fn refresh_layer_focus(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let current = keyboard.current_focus();
        let serial = SERIAL_COUNTER.next_serial();

        if let Some(exclusive) = self.exclusive_layer() {
            if current.as_ref() != Some(&exclusive) {
                keyboard.set_focus(self, Some(exclusive), serial);
            }
            return;
        }

        if current.is_some_and(|surface| surface.alive()) {
            return;
        }
        let fallback = self.toplevels.last().map(|t| t.wl_surface().clone());
        keyboard.set_focus(self, fallback, serial);
    }

    /// Must run before the root is unmapped, while its popup tree is still reachable.
    pub fn despawn_layer_popups(&mut self, root: &WlSurface) {
        let doomed: Vec<ObjectId> = PopupManager::popups_for_surface(root)
            .map(|(popup, _)| popup.wl_surface().id())
            .collect();
        for id in doomed {
            self.wgpu_state.remove_texture(&id);
        }
    }

    /// Every mapped layer surface and layer popup, for the frame-callback heartbeat.
    pub fn layer_surfaces(&self) -> Vec<WlSurface> {
        let map = layer_map_for_output(&self.output);
        let mut surfaces = Vec::new();
        for mapped in map.layers() {
            let surface = mapped.wl_surface();
            surfaces.push(surface.clone());
            for (popup, _) in PopupManager::popups_for_surface(surface) {
                surfaces.push(popup.wl_surface().clone());
            }
        }
        surfaces
    }

    pub fn rearrange_layers(&mut self) {
        layer_map_for_output(&self.output).arrange();
    }
}
