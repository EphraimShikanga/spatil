use cgmath::Vector3;
use hecs::Entity;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::scene::WORLD_SCALE;
use crate::scene::animation::{self, AnimSnapshot, Easing, OnComplete};
use crate::scene::components::{Billboard, Presentation, Transform, WaylandSurface, WindowState};
use crate::state::SpatilState;

impl SpatilState {
    pub fn focused_entity(&self) -> Option<Entity> {
        let focused = self.seat.get_keyboard()?.current_focus()?;
        let root = self.root_surface(&focused);
        self.world
            .query::<&WaylandSurface>()
            .iter()
            .find(|(_, s)| s.id == root.id() && !s.closing)
            .map(|(entity, _)| entity)
    }

    pub fn focus_entity(&mut self, entity: Entity) {
        let Some(surface) = self
            .world
            .get::<&WaylandSurface>(entity)
            .ok()
            .map(|s| s.surface.clone())
        else {
            return;
        };
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(surface), serial);
        }
    }

    pub fn modal_window(&self) -> Option<(Entity, WindowState)> {
        self.world
            .query::<(&Presentation, &WaylandSurface)>()
            .iter()
            .find(|(_, (p, s))| {
                matches!(p.state, WindowState::Presented | WindowState::Fullscreen) && !s.closing
            })
            .map(|(entity, (p, _))| (entity, p.state))
    }

    pub fn spotlit_windows(&self) -> Vec<Entity> {
        self.world
            .query::<(&Presentation, &WaylandSurface)>()
            .iter()
            .filter(|(_, (p, s))| p.state == WindowState::Spotlit && !s.closing)
            .map(|(entity, _)| entity)
            .collect()
    }

    pub fn anything_raised(&self) -> bool {
        self.modal_window().is_some() || !self.spotlit_windows().is_empty()
    }

    pub fn toggle_present(&mut self) {
        match self.modal_window() {
            Some((entity, WindowState::Presented)) => self.return_to_ambient(entity),
            Some((entity, _)) => {
                self.return_to_ambient(entity);
                if let Some(focused) = self.focused_entity() {
                    self.raise_window(focused, WindowState::Presented);
                }
            }
            None => {
                if let Some(entity) = self.focused_entity() {
                    self.raise_window(entity, WindowState::Presented);
                }
            }
        }
    }

    pub fn toggle_fullscreen(&mut self) {
        match self.modal_window() {
            Some((entity, WindowState::Fullscreen)) => self.return_to_ambient(entity),
            Some((entity, _)) => {
                // Same window: restoring to Ambient in between would race the
                // size configure.
                self.raise_window(entity, WindowState::Fullscreen);
            }
            None => {
                if let Some(entity) = self.focused_entity() {
                    self.raise_window(entity, WindowState::Fullscreen);
                }
            }
        }
    }

    pub fn dismiss_modal(&mut self) -> bool {
        match self.modal_window() {
            Some((entity, _)) => {
                self.return_to_ambient(entity);
                true
            }
            None => false,
        }
    }

    pub fn raise_window(&mut self, entity: Entity, target: WindowState) {
        let Some(surface) = self.toplevel_for(entity) else {
            return;
        };
        let spotlight = target == WindowState::Spotlit;
        if !spotlight
            && let Some((other, _)) = self.modal_window()
            && other != entity
        {
            self.return_to_ambient(other);
        }

        let saved = self.world.get::<&Presentation>(entity).ok().map(|p| (p.ambient, p.ambient_size));
        let (ambient, ambient_size) = match saved {
            Some(saved) => saved,
            None => {
                let Ok(transform) = self.world.get::<&Transform>(entity) else {
                    return;
                };
                (AnimSnapshot::of(&transform), self.configured_size(&surface))
            }
        };

        let height = if spotlight {
            self.natural_height(entity)
        } else {
            let (width, height) = self.raised_size(target);
            let fullscreen = target == WindowState::Fullscreen;
            surface.with_pending_state(|state| {
                state.size = Some((width, height).into());
                if fullscreen {
                    state.states.set(xdg_toplevel::State::Fullscreen);
                } else {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                }
            });
            surface.send_configure();
            height
        };

        let _ = self.world.insert_one(
            entity,
            Presentation { state: target, ambient, ambient_size },
        );
        if !spotlight {
            // The world behind is modal; further drags are refused by `world_is_modal`.
            self.drag = None;
            self.navigate = None;
        }
        self.focus_entity(entity);

        let end = self.raised_snapshot(entity, target, height);
        self.tween_to(entity, end);
    }

    pub fn return_to_ambient(&mut self, entity: Entity) {
        let Ok(presentation) = self.world.get::<&Presentation>(entity).map(|p| {
            (p.state, p.ambient, p.ambient_size)
        }) else {
            return;
        };
        let (state, ambient, ambient_size) = presentation;
        if state == WindowState::Ambient {
            return;
        }

        if state != WindowState::Spotlit
            && let Some(surface) = self.toplevel_for(entity)
        {
            surface.with_pending_state(|state| {
                state.size = Some(ambient_size.into());
                state.states.unset(xdg_toplevel::State::Fullscreen);
            });
            surface.send_configure();
        }

        let _ = self.world.remove_one::<Presentation>(entity);
        self.tween_to(entity, ambient);
    }

    pub fn dismiss_on_backdrop(&mut self, hit: Option<Entity>) -> bool {
        let window = hit.map(|entity| self.window_entity(entity));
        for entity in self.spotlit_windows() {
            if window != Some(entity) {
                self.return_to_ambient(entity);
            }
        }
        let Some((raised, _)) = self.modal_window() else {
            return false;
        };
        if window == Some(raised) {
            return false;
        }
        self.return_to_ambient(raised);
        true
    }

    pub fn scene_report(&self) -> serde_json::Value {
        let pointer = self.pointer_location;
        let hit = crate::input::raycast::raycast_entity(self, pointer);
        let focus = self
            .seat
            .get_pointer()
            .and_then(|p| p.current_focus())
            .map(|s| format!("{:?}", s.id()));

        let windows: Vec<serde_json::Value> = self
            .world
            .query::<(&Transform, &WaylandSurface)>()
            .iter()
            .map(|(entity, (transform, surface))| {
                let state = self
                    .world
                    .get::<&Presentation>(entity)
                    .map(|p| format!("{:?}", p.state))
                    .unwrap_or_else(|_| "Ambient".to_string());
                serde_json::json!({
                    "entity": format!("{entity:?}"),
                    "state": state,
                    "visible": surface.visible,
                    "closing": surface.closing,
                    "size": [surface.size.0, surface.size.1],
                    "at": [transform.translation.x, transform.translation.y, transform.translation.z],
                })
            })
            .collect();

        serde_json::json!({
            "pointer": [pointer.x, pointer.y],
            "pointer_focus": focus,
            "under_cursor": hit.as_ref().map(|h| format!("{:?}", h.entity)),
            "under_cursor_local": hit.as_ref().map(|h| [h.local_point.x, h.local_point.y]),
            "spotlit": self.spotlit_windows().iter().map(|e| format!("{e:?}")).collect::<Vec<_>>(),
            "modal": self.modal_window().map(|(e, s)| format!("{e:?} {s:?}")),
            "windows": windows,
        })
    }

    fn natural_height(&self, entity: Entity) -> i32 {
        self.world
            .get::<&WaylandSurface>(entity)
            .map(|s| s.size.1 as i32)
            .unwrap_or(0)
            .max(1)
    }

    fn raised_size(&self, target: WindowState) -> (i32, i32) {
        let (w, h) = (self.wgpu_state.width as f32, self.wgpu_state.height as f32);
        let fill = match target {
            WindowState::Fullscreen => 1.0,
            _ => self.config.input.present_fill.clamp(0.1, 1.0),
        };
        (((w * fill) as i32).max(1), ((h * fill) as i32).max(1))
    }

    fn raised_snapshot(&self, entity: Entity, target: WindowState, height_px: i32) -> AnimSnapshot {
        let mode = self.config.scene_graph.scene_mode();
        let fill = match target {
            WindowState::Fullscreen => 1.0,
            WindowState::Spotlit => self.config.shell.spotlight_fill.clamp(0.1, 1.0),
            _ => self.config.input.present_fill.clamp(0.1, 1.0),
        };
        let height = height_px as f32 / WORLD_SCALE;
        let translation = self.wgpu_state.fit_placement(mode, height, fill);
        let rotation = self.wgpu_state.facing_rotation(mode);

        let mut snapshot = self
            .world
            .get::<&Transform>(entity)
            .map(|t| AnimSnapshot::of(&t))
            .unwrap_or(AnimSnapshot {
                translation: Vector3::new(0.0, 0.0, 0.0),
                rotation,
                scale: Vector3::new(1.0, 1.0, 1.0),
                opacity: 1.0,
            });
        snapshot.translation = translation;
        snapshot.rotation = rotation;
        snapshot.opacity = 1.0;
        snapshot
    }

    fn tween_to(&mut self, entity: Entity, end: AnimSnapshot) {
        // A raised window is posed by us, not by the billboard system.
        let _ = self.world.remove_one::<Billboard>(entity);
        let Ok(start) = self.world.get::<&Transform>(entity).map(|t| AnimSnapshot::of(&t)) else {
            return;
        };
        if !self.config.animation.enabled {
            if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(entity) {
                end.apply_to(transform);
            }
            self.restore_billboard(entity);
            return;
        }
        let duration = self.config.animation.present_ms;
        animation::tween_entity(
            &mut self.world,
            entity,
            start,
            end,
            duration,
            Easing::EaseOutCubic,
            OnComplete::Remove,
        );
        self.restore_billboard(entity);
    }

    fn restore_billboard(&mut self, entity: Entity) {
        let ambient = self
            .world
            .get::<&Presentation>(entity)
            .map(|p| p.state == WindowState::Ambient)
            .unwrap_or(true);
        if ambient && self.config.scene_graph.scene_mode() == crate::scene::mode::SceneMode::Aether {
            let _ = self.world.insert_one(entity, Billboard);
        }
    }

    fn toplevel_for(&self, entity: Entity) -> Option<ToplevelSurface> {
        let surface_id = self.world.get::<&WaylandSurface>(entity).ok()?.id.clone();
        self.toplevels
            .iter()
            .find(|t| t.wl_surface().id() == surface_id)
            .cloned()
    }

    fn configured_size(&self, surface: &ToplevelSurface) -> (i32, i32) {
        surface
            .current_state()
            .size
            .map(|s| (s.w, s.h))
            .unwrap_or((
                self.config.windows.default_width,
                self.config.windows.default_height,
            ))
    }

    pub fn tick_backdrop(&mut self, dt_ms: f32) {
        let presented = matches!(self.modal_window(), Some((_, WindowState::Presented)));
        let target = if presented || !self.spotlit_windows().is_empty() { 1.0 } else { 0.0 };
        if !self.config.animation.enabled {
            self.wgpu_state.backdrop_dim = target;
            return;
        }
        let step = dt_ms / self.config.animation.present_ms.max(1.0);
        let current = self.wgpu_state.backdrop_dim;
        self.wgpu_state.backdrop_dim = if current < target {
            (current + step).min(target)
        } else {
            (current - step).max(target)
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::scene::components::WindowState;

    #[test]
    fn only_present_and_fullscreen_occupy_the_exclusive_slot() {
        let exclusive = |state| matches!(state, WindowState::Presented | WindowState::Fullscreen);
        assert!(exclusive(WindowState::Presented));
        assert!(exclusive(WindowState::Fullscreen));
        assert!(
            !exclusive(WindowState::Spotlit),
            "a spotlight must not evict a presented window",
        );
        assert!(!exclusive(WindowState::Ambient));
    }
}
