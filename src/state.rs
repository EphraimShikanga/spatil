pub mod client;

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::time::Instant;

use crate::config::{Action, KeyCombo, SpatilConfig};
use crate::renderer::WgpuState;
use crate::scene::components::WaylandSurface;
use smithay::backend::allocator::{Format, Fourcc};
use smithay::input::keyboard::XkbConfig;
use smithay::input::{Seat, SeatState};
use smithay::utils::{Logical, Point};
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState};
use smithay::wayland::output::OutputManagerState;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel, Scale};
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState};
use smithay::wayland::shm::ShmState;
use smithay::wayland::xdg_activation::XdgActivationState;

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
    pub xdg_activation_state: XdgActivationState,
    pub output_manager_state: OutputManagerState,
    pub dmabuf_state: DmabufState,
    /// Handle to the `zwp_linux_dmabuf_v1` global, if the wgpu device supports
    /// Vulkan dmabuf import. `None` means SHM-only mode: the global is never
    /// advertised, so clients won't try hardware buffers we can't accept.
    pub dmabuf_global: Option<DmabufGlobal>,
    /// Registry of live toplevel windows, for lookup by `WlSurface`.
    ///
    /// This is deliberately NOT `smithay::desktop::Space`/`Window`: it carries no
    /// 2D stacking/layout semantics. It's a flat "which toplevels exist" list that
    /// the 3D scene graph will eventually absorb once scene nodes exist.
    pub toplevels: Vec<ToplevelSurface>,
    /// Seat with a keyboard and pointer added, so real clients receive input.
    /// Winit window events are translated into smithay input calls in
    /// `src/input.rs`. Touch is intentionally left out for now.
    pub seat_state: SeatState<SpatilState>,
    pub seat: Seat<SpatilState>,
    /// Current pointer position, in the compositor's logical space. Tracked so
    /// button/axis events can reuse the last motion location.
    pub pointer_location: Point<f64, Logical>,
    /// The single virtual output. Stored so per-frame code (frame callbacks,
    /// future scanout tracking) can reference it;
    pub output: Output,
    /// Monotonic clock, started at compositor boot. Used to compute the
    /// millisecond timestamps `wl_callback::done` events require.
    pub start_time: std::time::Instant,
    pub world: hecs::World,
    pub config: SpatilConfig,
    /// `config.keybindings` compiled into matchable combos; rebuilt on reload.
    pub keybindings: HashMap<KeyCombo, Action>,
    /// Write-side clones of connected scripts' sockets, for event broadcasts;
    /// read sides live in calloop sources. Pruned lazily on failed writes.
    pub ipc_clients: Vec<UnixStream>,
    /// Set by the `quit` action; polled in `main.rs` to exit the winit loop.
    pub should_quit: bool,
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
                            "advertising zwp_linux_dmabuf_v1 v4 with feedback \
                             (main_device set; Argb8888/Xrgb8888, Linear modifier)"
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

        Self {
            display_handle,
            loop_handle,
            loop_signal,
            wgpu_state,
            compositor_state,
            shm_state,
            xdg_shell_state,
            xdg_activation_state,
            output_manager_state,
            dmabuf_state,
            dmabuf_global,
            toplevels: Vec::new(),
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
            window_id_counter: 1,
        }
    }

    pub fn next_window_id(&mut self) -> u32 {
        let id = self.window_id_counter;
        self.window_id_counter = self.window_id_counter.wrapping_add(1);
        id
    }

    /// Launches every `[autostart] exec` entry once via `sh -c`, cwd'd to the
    /// config dir so relative paths like `hooks/foo.py` resolve. Fire-and-forget
    /// (children auto-reaped via `SIGCHLD`); they inherit `WAYLAND_DISPLAY` and
    /// `SPATIL_SOCKET` from our env.
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

    /// Starts the map-postponement clock on the first buffer commit for
    /// `surface_id`, returning the window id once so the caller broadcasts
    /// `window_committed`. `None` on later commits or untracked surfaces.
    pub fn mark_committed(&mut self, surface_id: &ObjectId) -> Option<u32> {
        for (_entity, surface) in self.world.query_mut::<&mut WaylandSurface>() {
            if &surface.id == surface_id && surface.committed_at.is_none() {
                surface.committed_at = Some(Instant::now());
                return Some(surface.window_id);
            }
        }
        None
    }

    /// Hot-reload: re-applies what can change at runtime (keybindings, keyboard
    /// repeat + layout, output mode). Socket path and default window size only
    /// take effect on restart.
    pub fn apply_config(&mut self, config: SpatilConfig) {
        self.keybindings = config.compile_keybindings();

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

        self.config = config;
        tracing::info!("configuration reloaded");
    }
}
