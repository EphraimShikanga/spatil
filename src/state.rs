pub mod client;
pub mod layers;
pub mod popups;
pub mod presentation;

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::time::Instant;

use std::time::Duration;

use cgmath::Vector3;

use crate::config::{Action, KeyCombo, SessionState, SpatilConfig};
use crate::input::drag::{Drag, Navigate};
use crate::renderer::WgpuState;
use crate::shell::orb::OrbRuntime;
use crate::shell::module::{ModuleId, ModuleState};
use crate::shell::network::init_network;
use crate::shell::tray::{Tray, init_tray};
use crate::scene::WORLD_SCALE;
use crate::scene::animation::{self, AnimSnapshot, Easing, OnComplete, Physics, Tween};
use crate::scene::components::{Billboard, Placement, Popup, Transform, WaylandSurface};
use crate::scene::mode::{SceneMode, default_transform};
use crate::scene::systems::update_map_visibility;
use smithay::backend::allocator::{Format, Fourcc};
use smithay::input::keyboard::XkbConfig;
use smithay::input::{Seat, SeatState};
use smithay::utils::{Logical, Point};
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::desktop::PopupManager;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState};
use smithay::wayland::output::OutputManagerState;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel, Scale};
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState};
use smithay::wayland::shm::ShmState;
use smithay::wayland::xdg_activation::XdgActivationState;

fn component_mul(a: Vector3<f32>, b: Vector3<f32>) -> Vector3<f32> {
    Vector3::new(a.x * b.x, a.y * b.y, a.z * b.z)
}

/// Guards a zero denominator, which a never-committed window's tween can hold.
fn component_ratio(target: Vector3<f32>, current: Vector3<f32>) -> Vector3<f32> {
    let ratio = |t: f32, c: f32| if c.abs() < 1e-6 { 1.0 } else { t / c };
    Vector3::new(
        ratio(target.x, current.x),
        ratio(target.y, current.y),
        1.0,
    )
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|v| {
        matches!(
            v.as_str(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        )
    })
}

pub struct SpatilState {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, SpatilState>,
    pub loop_signal: LoopSignal,
    pub wgpu_state: WgpuState,
    pub compositor_state: CompositorState,
    pub shm_state: ShmState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub xdg_activation_state: XdgActivationState,
    pub output_manager_state: OutputManagerState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub toplevels: Vec<ToplevelSurface>,
    pub popups: PopupManager,
    pub seat_state: SeatState<SpatilState>,
    pub seat: Seat<SpatilState>,
    pub pointer_location: Point<f64, Logical>,
    pub output: Output,
    pub start_time: std::time::Instant,
    pub world: hecs::World,
    pub config: SpatilConfig,
    pub keybindings: HashMap<KeyCombo, Action>,
    pub ipc_clients: Vec<UnixStream>,
    pub should_quit: bool,
    pub last_frame: std::time::Instant,
    pub drm: Option<crate::backend::drm::DrmOutput>,
    pub drag: Option<Drag>,
    pub navigate: Option<Navigate>,
    pub pointer_moved: bool,
    pub last_input: Instant,
    pub orb: OrbRuntime,
    pub tray: Tray,
    pub module_slots: Vec<(ModuleId, [f32; 4])>,
    pub network: Option<ModuleState>,
    pub cursor_hidden: bool,
    window_id_counter: u32,
}

impl SpatilState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_handle: LoopHandle<'static, SpatilState>,
        loop_signal: LoopSignal,
        wgpu_state: WgpuState,
        config: SpatilConfig,
    ) -> Self {
        let start_time = std::time::Instant::now();
        let compositor_state = CompositorState::new_v6::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "seat0");
        let xkb_config = XkbConfig {
            layout: &config.keyboard.xkb_layout,
            model: &config.keyboard.xkb_model,
            ..Default::default()
        };
        seat.add_keyboard(
            xkb_config,
            config.keyboard.repeat_delay,
            config.keyboard.repeat_rate,
        )
        .expect("failed to initialize the keyboard");
        seat.add_pointer();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let xdg_activation_state = XdgActivationState::new::<Self>(&display_handle);

        let output = Output::new(
            "spatil-output".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Spatil".to_string(),
                model: "Virtual".to_string(),
            },
        );
        let mode = Mode {
            size: (config.display.width as i32, config.display.height as i32).into(),
            refresh: config.display.refresh_rate as i32,
        };
        output.change_current_state(
            Some(mode),
            None,
            Some(Scale::Integer(1)),
            None,
        );
        output.set_preferred(mode);
        output.create_global::<Self>(&display_handle);


        // Only advertise zwp_linux_dmabuf_v1 if the device can actually import
        // dmabufs; advertising formats we can't back would break clients.
        // Linear-only: we have no way (yet) to query which vendor tiled
        // modifiers the GPU supports through wgpu-hal, so we don't claim them.
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_disabled = env_flag("SPATIL_DISABLE_DMABUF");
        let dmabuf_global = if dmabuf_disabled {
            tracing::warn!(
                "SPATIL_DISABLE_DMABUF is set; not advertising zwp_linux_dmabuf_v1 (forcing SHM-only clients)"
            );
            None
        } else if wgpu_state
            .device
            .features()
            .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
        {
            let modifiers = &wgpu_state.dmabuf_modifiers;
            let mut formats = Vec::new();
            for &modifier in modifiers {
                formats.push(Format {
                    code: Fourcc::Argb8888,
                    modifier,
                });
                formats.push(Format {
                    code: Fourcc::Xrgb8888,
                    modifier,
                });
            }
            // Prefer the v4+ feedback global: Mesa's EGL platform learns which
            // GPU to allocate on from feedback's `main_device` (there is no
            // legacy wl_drm in Smithay). With a bare v3 global, Mesa can't
            // identify the device ("failed to get driver name for fd -1") and
            // hardware EGL contexts fail entirely -- verified empirically.
            let global = match wgpu_state.main_device_id {
                Some(main_device) => match DmabufFeedbackBuilder::new(main_device, formats.clone()).build()
                {
                    Ok(feedback) => {
                        tracing::info!(
                            modifiers = ?wgpu_state.dmabuf_modifiers,
                            "advertising zwp_linux_dmabuf_v1 v4 with feedback \
                             (main_device set; Argb8888/Xrgb8888)"
                        );
                        dmabuf_state
                            .create_global_with_default_feedback::<Self>(&display_handle, &feedback)
                    }
                    Err(err) => {
                        tracing::warn!(
                            "failed to build dmabuf feedback ({err}); falling back to v3 global"
                        );
                        dmabuf_state.create_global::<Self>(&display_handle, formats)
                    }
                },
                None => {
                    tracing::info!(
                        "advertising zwp_linux_dmabuf_v1 v3 (no main_device available; \
                         hardware clients may fall back to software rendering)"
                    );
                    dmabuf_state.create_global::<Self>(&display_handle, formats)
                }
            };
            Some(global)
        } else {
            tracing::warn!("wgpu device lacks VULKAN_EXTERNAL_MEMORY_DMA_BUF; running SHM-only");
            None
        };

        let keybindings = config.compile_keybindings();

        let mut state = Self {
            display_handle,
            loop_handle,
            loop_signal,
            wgpu_state,
            compositor_state,
            shm_state,
            xdg_shell_state,
            layer_shell_state,
            xdg_activation_state,
            output_manager_state,
            dmabuf_state,
            dmabuf_global,
            toplevels: Vec::new(),
            popups: PopupManager::default(),
            seat_state,
            seat,
            pointer_location: (0.0, 0.0).into(),
            output,
            start_time,
            world: hecs::World::new(),
            config,
            keybindings,
            ipc_clients: Vec::new(),
            should_quit: false,
            last_frame: std::time::Instant::now(),
            drm: None,
            drag: None,
            navigate: None,
            pointer_moved: false,
            last_input: Instant::now(),
            orb: OrbRuntime::default(),
            tray: Tray::default(),
            module_slots: Vec::new(),
            network: None,
            cursor_hidden: false,
            window_id_counter: 1,
        };
        state.restore_orb_corner(&SessionState::load());
        let tray_requests = init_tray(&state.loop_handle);
        state.tray.attach(tray_requests);
        init_network(&state.loop_handle);
        state
    }

    /// Resolves every toplevel's pending `wl_surface.frame()` callbacks so clients
    /// draw their next frame. Called by both backends after presenting a frame; without
    /// it real clients freeze after their first.
    pub fn send_frame_callbacks(&self) {
        let elapsed = self.start_time.elapsed();
        let send = |surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface| {
            smithay::desktop::utils::send_frames_surface_tree(
                surface,
                &self.output,
                elapsed,
                Some(Duration::ZERO),
                |_, _| Some(self.output.clone()),
            );
        };
        for toplevel in &self.toplevels {
            send(toplevel.wl_surface());
            for (popup, _) in PopupManager::popups_for_surface(toplevel.wl_surface()) {
                send(popup.wl_surface());
            }
        }
        for surface in self.layer_surfaces() {
            send(&surface);
        }
    }

    pub fn reveal_pending(&mut self) {
        let timeout = Duration::from_millis(self.config.windows.map_timeout_ms);
        let has_ipc = !self.ipc_clients.is_empty();
        let revealed = update_map_visibility(&mut self.world, has_ipc, timeout);
        if revealed.is_empty() || !self.config.animation.enabled {
            return;
        }
        let mode = self.config.scene_graph.scene_mode();
        let open_ms = self.config.animation.open_ms;
        for entity in revealed {
            let Some(resting) = self
                .world
                .get::<&Transform>(entity)
                .ok()
                .map(|t| AnimSnapshot::of(&t))
            else {
                continue;
            };
            let start = animation::open_start(mode, &resting);
            if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(entity) {
                start.apply_to(transform);
            }
            animation::tween_entity(
                &mut self.world,
                entity,
                start,
                resting,
                open_ms,
                Easing::EaseOutCubic,
                OnComplete::Remove,
            );
        }
    }

    pub fn tick_animations(&mut self, dt_ms: f32) {
        self.popups.cleanup();
        self.update_hud_layout();
        let dt_ms = dt_ms.min(50.0);
        for id in animation::update(&mut self.world, dt_ms) {
            self.wgpu_state.remove_texture(&id);
        }
        let physics = self.physics();
        animation::integrate(&mut self.world, dt_ms, physics);
        self.wgpu_state.camera_controller.tick(dt_ms, physics);
        self.wgpu_state.scene_transition.tick(dt_ms);
        self.tick_backdrop(dt_ms);
        self.tick_orb(dt_ms);
        self.tick_modules();
        // Derived, not latched: a Presented window that is *closed* rather than
        // dismissed would otherwise leave the parallax frozen for good.
        self.wgpu_state.parallax_frozen =
            self.drag.is_some() || self.navigate.is_some() || self.world_is_modal();
    }

    fn physics(&self) -> Physics {
        Physics {
            damping: self.config.animation.damping,
            spring_stiffness: self.config.animation.spring_stiffness,
            spring_damping: self.config.animation.spring_damping,
        }
    }

    /// CPU-clock frame delta (winit backend). DRM uses the vblank timestamp instead.
    pub fn cpu_dt_ms(&mut self) -> f32 {
        let now = std::time::Instant::now();
        let dt = (now - self.last_frame).as_secs_f32() * 1000.0;
        self.last_frame = now;
        dt
    }

    pub fn next_window_id(&mut self) -> u32 {
        let id = self.window_id_counter;
        self.window_id_counter = self.window_id_counter.wrapping_add(1);
        id
    }

    pub fn run_autostart(&self) {
        let config_dir = SpatilConfig::config_dir();
        for entry in &self.config.autostart.exec {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c").arg(entry);
            if let Some(dir) = &config_dir {
                cmd.current_dir(dir);
            }
            match cmd.spawn() {
                Ok(_) => tracing::info!("autostart: launched {entry:?}"),
                Err(err) => tracing::warn!("autostart: failed to launch {entry:?}: {err}"),
            }
        }
    }

    pub fn entity_for_surface(&self, surface_id: &ObjectId) -> Option<hecs::Entity> {
        self.world
            .query::<&WaylandSurface>()
            .iter()
            .find(|(_, s)| &s.id == surface_id && !s.closing)
            .map(|(entity, _)| entity)
    }

    pub fn mark_committed(&mut self, surface_id: &ObjectId) -> Option<u32> {
        for (_entity, surface) in self.world.query_mut::<&mut WaylandSurface>() {
            if &surface.id == surface_id && surface.committed_at.is_none() {
                surface.committed_at = Some(Instant::now());
                return Some(surface.window_id);
            }
        }
        None
    }

    /// Scale follows the committed buffer, never the config; an in-flight tween has
    /// its endpoints rescaled rather than clobbering it
    pub fn set_window_size(&mut self, surface_id: &ObjectId, width: u32, height: u32) {
        let scale = Vector3::new(
            width as f32 / WORLD_SCALE,
            height as f32 / WORLD_SCALE,
            1.0,
        );
        for (_entity, (transform, surface, popup, tween)) in self.world.query_mut::<(
            &mut Transform,
            &mut WaylandSurface,
            Option<&Popup>,
            Option<&mut Tween>,
        )>() {
            if &surface.id != surface_id {
                continue;
            }
            surface.size = (width, height);
            if popup.is_some() {
                return;
            }
            if let Some(tween) = tween {
                let ratio = component_ratio(scale, tween.end.scale);
                tween.start.scale = component_mul(tween.start.scale, ratio);
                tween.end.scale = component_mul(tween.end.scale, ratio);
            }
            transform.scale.x = scale.x;
            transform.scale.y = scale.y;
            return;
        }
    }

    pub fn toggle_scene(&mut self) {
        let next = match self.config.scene_graph.scene_mode() {
            SceneMode::Strata => SceneMode::Aether,
            SceneMode::Aether => SceneMode::Strata,
        };
        self.set_scene(next);
    }

    pub fn set_scene(&mut self, mode: SceneMode) {
        if self.config.scene_graph.scene_mode() == mode {
            return;
        }
        let mut config = self.config.clone();
        config.scene_graph.mode = mode.as_str().to_string();
        tracing::info!("switching scene to {}", mode.as_str());
        self.apply_config(config);
    }

    /// Routed through `apply_config`, i.e. the same path a same-mode reload takes.
    pub fn tilt_windows(&mut self, degrees: f32) {
        if self.config.scene_graph.scene_mode() != SceneMode::Strata {
            tracing::info!("tilt only applies in strata; aether panes billboard");
            return;
        }
        let mut config = self.config.clone();
        let pitch = (config.scene_graph.strata.window_pitch + degrees).clamp(0.0, 89.0);
        config.scene_graph.strata.window_pitch = pitch;
        self.apply_config(config);
    }

    pub fn apply_config(&mut self, config: SpatilConfig) {
        self.keybindings = config.compile_keybindings();
        if config.shell.familiar_corner != self.config.shell.familiar_corner {
            self.forget_orb_corner();
        }

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.change_repeat_info(config.keyboard.repeat_rate, config.keyboard.repeat_delay);
            let xkb_config = XkbConfig {
                layout: &config.keyboard.xkb_layout,
                model: &config.keyboard.xkb_model,
                ..Default::default()
            };
            if let Err(err) = keyboard.set_xkb_config(self, xkb_config) {
                tracing::warn!("failed to apply reloaded xkb config: {err:?}");
            }
        }

        let mode = Mode {
            size: (config.display.width as i32, config.display.height as i32).into(),
            refresh: config.display.refresh_rate as i32,
        };
        self.output.change_current_state(Some(mode), None, None, None);
        self.output.set_preferred(mode);
        self.rearrange_layers();

        let old_mode = self.wgpu_state.scene_mode;
        self.wgpu_state
            .apply_scene_config(
                &config.scene_graph,
                &config.display,
                &config.input,
                &config.shell,
            );
        let new_mode = config.scene_graph.scene_mode();
        let mode_changed = old_mode != new_mode;

        if mode_changed {
            for entity in self.placeable_entities() {
                self.remember_placement_in(entity, old_mode);
            }
        }

        let facing = self.wgpu_state.facing_rotation(new_mode);

        // A window hand-placed in the target mode goes back exactly where the user
        // left it; only one that has never been there gets laid out.
        let mut plans = Vec::new();
        for (entity, (transform, surface, placement, popup)) in self
            .world
            .query::<(&Transform, &WaylandSurface, Option<&Placement>, Option<&Popup>)>()
            .iter()
        {
            if surface.closing || popup.is_some() {
                continue;
            }
            let start = AnimSnapshot::of(transform);
            let mut end = start;
            match placement.and_then(|p| p.get(new_mode)) {
                Some(remembered) => {
                    end.translation = remembered.translation;
                    end.rotation = remembered.rotation;
                }
                None => {
                    let target =
                        default_transform(new_mode, surface.window_id, &config.scene_graph, facing);
                    end.rotation = target.rotation;
                    // A same-mode reload keeps translation so IPC-set and
                    // hand-placed positions survive a live pitch or colour edit.
                    if mode_changed {
                        end.translation = target.translation;
                    }
                }
            }
            plans.push((entity, start, end));
        }

        if mode_changed && config.animation.enabled {
            self.wgpu_state
                .scene_transition
                .start(old_mode, new_mode, config.animation.switch_ms);
            let switch_ms = config.animation.switch_ms;
            for (entity, start, end) in plans {
                animation::tween_entity(
                    &mut self.world,
                    entity,
                    start,
                    end,
                    switch_ms,
                    Easing::EaseOutExpo,
                    OnComplete::Remove,
                );
            }
        } else {
            for (entity, _, end) in plans {
                if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(entity) {
                    // Scale is the committed buffer's, never the config's.
                    transform.translation = end.translation;
                    transform.rotation = end.rotation;
                }
            }
        }

        if mode_changed {
            self.apply_billboards(new_mode);
        }

        self.config = config;
        tracing::info!("configuration reloaded");
    }

    pub fn apply_billboards(&mut self, mode: SceneMode) {
        let raised = self.modal_window().map(|(entity, _)| entity);
        let entities: Vec<_> = self
            .world
            .query::<(&WaylandSurface, Option<&Popup>)>()
            .iter()
            .filter(|(_, (_, popup))| popup.is_none())
            .map(|(entity, _)| entity)
            .collect();
        for entity in entities {
            if mode == SceneMode::Aether && Some(entity) != raised {
                let _ = self.world.insert_one(entity, Billboard);
            } else {
                let _ = self.world.remove_one::<Billboard>(entity);
            }
        }
    }

    fn placeable_entities(&self) -> Vec<hecs::Entity> {
        self.world
            .query::<(&WaylandSurface, Option<&Popup>)>()
            .iter()
            .filter(|(_, (s, popup))| !s.closing && popup.is_none())
            .map(|(entity, _)| entity)
            .collect()
    }

    pub fn remember_placement_in(&mut self, entity: hecs::Entity, mode: SceneMode) {
        let Ok(snapshot) = self.world.get::<&Transform>(entity).map(|t| AnimSnapshot::of(&t)) else {
            return;
        };
        if let Ok(mut placement) = self.world.get::<&mut Placement>(entity) {
            placement.set(mode, snapshot);
            return;
        }
        let mut placement = Placement::default();
        placement.set(mode, snapshot);
        let _ = self.world.insert_one(entity, placement);
    }
}
