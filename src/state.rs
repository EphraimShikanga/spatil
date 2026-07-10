pub mod client;

use crate::renderer::WgpuState;
use smithay::backend::allocator::{Format, Fourcc, Modifier};
use smithay::input::{Seat, SeatState};
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState};
use smithay::wayland::output::OutputManagerState;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel, Scale};
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState};
use smithay::wayland::shm::ShmState;

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
    /// Minimal seat foundation. xdg-shell's popup-grab dispatch requires
    /// `SeatHandler`, and real clients need a `wl_seat` to receive input, so a
    /// seat is created here. Input routing itself is not yet wired (no keyboard/
    /// pointer added); that is future work.
    pub seat_state: SeatState<SpatilState>,
    pub seat: Seat<SpatilState>,
}

impl SpatilState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_handle: LoopHandle<'static, SpatilState>,
        loop_signal: LoopSignal,
        wgpu_state: WgpuState,
    ) -> Self {
        let compositor_state = CompositorState::new_v6::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(&display_handle, "seat0");
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);

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
            size: (1920, 1080).into(),
            refresh: 60_000,
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

        Self {
            display_handle,
            loop_handle,
            loop_signal,
            wgpu_state,
            compositor_state,
            shm_state,
            xdg_shell_state,
            output_manager_state,
            dmabuf_state,
            dmabuf_global,
            toplevels: Vec::new(),
            seat_state,
            seat,
        }
    }
}
