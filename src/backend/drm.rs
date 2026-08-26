use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Buffer as _, Fourcc, Modifier};
use std::time::Duration;

use smithay::backend::drm::gbm::{GbmFramebuffer, framebuffer_from_bo};
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, DrmSurface, NodeType,
    PlaneConfig, PlaneState,
};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::reexports::drm::buffer::DrmFourcc;
use smithay::reexports::drm::control::dumbbuffer::DumbBuffer;
use smithay::reexports::drm::control::{Device as ControlDevice, Mode, ModeTypeFlags, connector, crtc};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Rectangle, Size, Transform};

use crate::backend;
use crate::config::SpatilConfig;
use crate::renderer::{self, WgpuState};
use crate::state::SpatilState;

/// Round-robin depth; a buffer's flip always completes before we return to it.
const RING_LEN: usize = 3;

/// Upper bound on waiting for a frame's GPU work. Generous for a ~16 ms frame; if it
/// trips, the GPU is wedged and we must not block the event loop (see the wait below).
const GPU_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

struct ScanoutBuffer {
    _bo: GbmBuffer, // owns the GBM allocation the view + fb alias
    view: wgpu::TextureView,
    framebuffer: GbmFramebuffer,
}

struct HwCursor {
    buffer: DumbBuffer,
    hotspot: (i32, i32),
}

pub struct DrmOutput {
    session: LibSeatSession,
    device: DrmDevice,
    _gbm: GbmDevice<DrmDeviceFd>, // keeps the buffers/framebuffers valid
    surface: DrmSurface,
    connector: connector::Handle,
    crtc: crtc::Handle,
    ring: Vec<ScanoutBuffer>,
    front: usize,
    first_flip_done: bool,
    mode_size: (i32, i32),
    active: bool,
    last_vblank: Option<Duration>,
    cursor: Option<HwCursor>,
    cursor_hidden: bool,
}

pub fn run(config: SpatilConfig) {
    if let Err(err) = try_run(config) {
        tracing::error!("DRM backend failed: {err}");
    }
}

fn try_run(config: SpatilConfig) -> Result<(), String> {
    let (mut session, session_notifier) = LibSeatSession::new()
        .map_err(|e| format!("failed to create libseat session (run from a TTY?): {e}"))?;
    let seat_name = session.seat();
    tracing::info!("session on seat {seat_name}");

    let path = udev::primary_gpu(&seat_name)
        .map_err(|e| format!("primary_gpu query failed: {e}"))?
        .or_else(|| {
            udev::all_gpus(&seat_name)
                .ok()
                .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0)))
        })
        .ok_or_else(|| "no GPU found".to_string())?;
    tracing::info!("primary GPU: {}", path.display());

    let kms_node = DrmNode::from_path(&path).map_err(|e| format!("not a DRM node: {e}"))?;
    let render_dev = kms_node
        .node_with_type(NodeType::Render)
        .and_then(|r| r.ok())
        .map(|n| n.dev_id());

    let fd = session
        .open(
            &path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NONBLOCK,
        )
        .map_err(|e| format!("session failed to open {}: {e}", path.display()))?;
    let drm_fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (mut drm, drm_notifier) =
        DrmDevice::new(drm_fd.clone(), true).map_err(|e| format!("DrmDevice::new: {e}"))?;
    let gbm = GbmDevice::new(drm_fd.clone()).map_err(|e| format!("GbmDevice::new: {e}"))?;

    let (connector, mode, crtc) = pick_output(&drm, &config.output)?;
    let (mw, mh) = mode.size();
    tracing::info!(
        "output: connector {connector:?} mode {mw}x{mh}@{} crtc {crtc:?}",
        mode.vrefresh()
    );

    let instance = renderer::new_vulkan_instance();
    let adapter = pick_adapter(&instance, render_dev)?;
    tracing::info!("wgpu adapter: {}", adapter.get_info().name);
    if !adapter
        .features()
        .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
    {
        return Err("adapter lacks VULKAN_EXTERNAL_MEMORY_DMA_BUF -- cannot scan out".into());
    }
    let (device, queue) = pollster::block_on(renderer::request_device(&adapter));

    // Only modifiers the driver says it can rasterize sRGB into. Never widen this set:
    // rendering into an unadvertised modifier (e.g. Linear on NVIDIA) hangs the GPU
    // rather than erroring, which strands the display
    let render_mods = renderer::query_render_modifiers(&adapter);
    if render_mods.is_empty() {
        return Err(
            "adapter advertises no scanout-renderable modifier for Bgra8UnormSrgb -- \
             cannot render to this display"
                .into(),
        );
    }
    tracing::info!(
        "renderable scanout modifiers: {render_mods:?} (Linear supported: {})",
        render_mods.contains(&Modifier::Linear)
    );

    // XRGB8888 scanout -> Bgra8UnormSrgb, matching the client dmabuf-import format.
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let wgpu_state = WgpuState::from_device(
        device,
        queue,
        &adapter,
        format,
        mw as u32,
        mh as u32,
        &config,
    );

    let mut allocator =
        GbmAllocator::new(gbm.clone(), GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING);
    let surface = drm
        .create_surface(crtc, mode, &[connector])
        .map_err(|e| format!("create_surface: {e}"))?;
    let ring = build_ring(
        &wgpu_state.device,
        &drm_fd,
        &mut allocator,
        &render_mods,
        mw,
        mh,
    )?;
    tracing::info!("allocated {} scanout buffers", ring.len());

    let cursor = init_cursor(&drm, crtc);

    let backend::Harness {
        mut calloop,
        display_handle,
        config_watcher,
    } = backend::harness(&config);
    let _config_watcher = config_watcher;

    let mut state = SpatilState::new(
        display_handle,
        calloop.handle(),
        calloop.get_signal(),
        wgpu_state,
        config,
    );
    state.run_autostart();
    state.drm = Some(DrmOutput {
        session: session.clone(),
        device: drm,
        _gbm: gbm,
        surface,
        connector,
        crtc,
        ring,
        front: 0,
        first_flip_done: false,
        mode_size: (mw as i32, mh as i32),
        active: true,
        last_vblank: None,
        cursor,
        cursor_hidden: false,
    });

    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput
        .udev_assign_seat(&seat_name)
        .map_err(|()| "failed to assign libinput seat".to_string())?;
    // The context is refcounted: the backend gets a clone, the original moves into
    // the session closure to suspend/resume the same context on VT switch.
    let libinput_backend = LibinputInputBackend::new(libinput.clone());
    calloop
        .handle()
        .insert_source(libinput_backend, |event, _, state: &mut SpatilState| {
            if let InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } = event {
                return;
            }
            state.process_input_event(event);
        })
        .map_err(|e| format!("insert libinput source: {e}"))?;

    calloop
        .handle()
        .insert_source(session_notifier, move |event, _, state: &mut SpatilState| {
            let Some(drm) = state.drm.as_mut() else { return };
            match event {
                SessionEvent::PauseSession => {
                    tracing::info!("session paused (VT switch away)");
                    drm.active = false;
                    drm.device.pause();
                    libinput.suspend();
                }
                SessionEvent::ActivateSession => {
                    tracing::info!("session activated (VT switch back)");
                    if libinput.resume().is_err() {
                        tracing::error!("failed to resume libinput after VT switch");
                    }
                    if let Err(err) = drm.device.activate(true) {
                        tracing::error!("failed to re-activate DRM device: {err}");
                    }
                    drm.active = true;
                    drm.first_flip_done = false;
                    drm.last_vblank = None;
                    drm.rearm_cursor();
                    state.render_drm_frame(0.0, true);
                }
            }
        })
        .map_err(|e| format!("insert session source: {e}"))?;

    calloop
        .handle()
        .insert_source(
            drm_notifier,
            |event, meta: &mut Option<DrmEventMetadata>, state: &mut SpatilState| {
                if let DrmEvent::VBlank(_crtc) = event {
                    let ts = meta.as_ref().and_then(|m| match m.time {
                        DrmEventTime::Monotonic(d) => Some(d),
                        DrmEventTime::Realtime(_) => None,
                    });
                    let dt_ms = state.vblank_dt_ms(ts);
                    state.render_drm_frame(dt_ms, false);
                }
            },
        )
        .map_err(|e| format!("insert drm source: {e}"))?;

    // a udev change on our card signals a connector plug/unplug.
    let our_device = state.drm.as_ref().map(|d| d.device.device_id());
    let udev = UdevBackend::new(&seat_name).map_err(|e| format!("UdevBackend::new: {e}"))?;
    calloop
        .handle()
        .insert_source(udev, move |event, _, state: &mut SpatilState| {
            if let UdevEvent::Changed { device_id } = event
                && Some(device_id) == our_device
            {
                state.handle_connector_hotplug();
            }
        })
        .map_err(|e| format!("insert udev source: {e}"))?;

    // First frame does the modeset via commit(); vblanks drive the rest.
    state.render_drm_frame(0.0, true);

    tracing::info!("DRM backend up; entering event loop");
    calloop
        .run(None, &mut state, |state| {
            if state.should_quit {
                state.loop_signal.stop();
            }
        })
        .map_err(|e| format!("event loop: {e}"))?;
    Ok(())
}

impl SpatilState {
    pub fn render_drm_frame(&mut self, dt_ms: f32, initial: bool) {
        {
            let Some(drm) = self.drm.as_ref() else { return };
            if !drm.active {
                return;
            }
        }

        self.reveal_pending();
        self.tick_animations(dt_ms);
        self.sync_hw_cursor_visibility();

        // Clone the view (Arc) + copy the fb handle so no borrow of self.drm is held
        // across the mutable render below.
        let (idx, view, fb) = {
            let drm = self.drm.as_ref().unwrap();
            let idx = if initial {
                0
            } else {
                (drm.front + 1) % drm.ring.len()
            };
            (
                idx,
                drm.ring[idx].view.clone(),
                *drm.ring[idx].framebuffer.as_ref(),
            )
        };

        self.wgpu_state.render(&mut self.world, &view);

        // Block until the GPU write completes before scanning out. A KMS in-fence
        // would avoid the CPU stall, but wgpu 30 exposes no exportable fence from
        // submit() to hand KMS. The wait is bounded: an unbounded one would freeze
        // the event loop, and a frozen loop can't ack the session pause, which wedges
        // VT switching and strands the user on a dead display with no way out.
        let poll = self.wgpu_state.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(GPU_WAIT_TIMEOUT),
        });
        if let Err(err) = poll {
            tracing::error!(
                "GPU wait failed ({err:?}) after {GPU_WAIT_TIMEOUT:?}; skipping flip \
                 (loop stays responsive -- Super+Shift+q or a VT switch still work)"
            );
            return;
        }

        let drm = self.drm.as_mut().unwrap();
        let (w, h) = drm.mode_size;
        let plane = PlaneState {
            handle: drm.surface.plane(),
            config: Some(PlaneConfig {
                src: Rectangle::from_size(Size::from((w as f64, h as f64))),
                dst: Rectangle::from_size(Size::from((w, h))),
                transform: Transform::Normal,
                alpha: 1.0,
                damage_clips: None,
                fb,
                fence: None,
            }),
        };

        let result = if drm.first_flip_done {
            drm.surface.page_flip([plane], true)
        } else {
            drm.surface.commit([plane], true)
        };
        match result {
            Ok(()) => {
                if !drm.first_flip_done {
                    tracing::info!("initial modeset committed; display should be live");
                }
                drm.front = idx;
                drm.first_flip_done = true;
            }
            Err(err) => tracing::error!("DRM {} failed: {err}", if drm.first_flip_done { "page-flip" } else { "commit" }),
        }

        self.send_frame_callbacks();
        if let Err(err) = self.display_handle.flush_clients() {
            tracing::warn!("failed to flush clients: {err}");
        }
    }

    pub fn vblank_dt_ms(&mut self, now: Option<Duration>) -> f32 {
        let Some(drm) = self.drm.as_mut() else {
            return 16.6;
        };
        match (now, drm.last_vblank) {
            (Some(now), Some(last)) => {
                drm.last_vblank = Some(now);
                now.saturating_sub(last).as_secs_f32() * 1000.0
            }
            (Some(now), None) => {
                drm.last_vblank = Some(now);
                0.0
            }
            (None, _) => 16.6,
        }
    }

    pub fn handle_connector_hotplug(&mut self) {
        let Some(drm) = self.drm.as_mut() else { return };
        let connected = drm
            .device
            .get_connector(drm.connector, true)
            .map(|c| c.state() == connector::State::Connected)
            .unwrap_or(false);
        match (connected, drm.active) {
            (false, true) => {
                tracing::warn!("output disconnected; pausing scanout");
                drm.active = false;
            }
            (true, false) => {
                tracing::info!("output reconnected; restart Spatil to re-initialize it");
            }
            _ => {}
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        match self.drm.as_mut() {
            Some(drm) => {
                if let Err(err) = drm.session.change_vt(vt) {
                    tracing::warn!("change_vt({vt}) failed: {err}");
                }
            }
            None => tracing::warn!("change_vt is only available on the DRM backend"),
        }
    }

    pub fn move_hw_cursor(&self) {
        if let Some(drm) = &self.drm
            && let Some(cursor) = &drm.cursor
        {
            let x = self.pointer_location.x as i32 - cursor.hotspot.0;
            let y = self.pointer_location.y as i32 - cursor.hotspot.1;
            #[allow(deprecated)]
            let _ = drm.device.move_cursor(drm.crtc, (x, y));
        }
    }

    #[allow(deprecated)]
    pub fn sync_hw_cursor_visibility(&mut self) {
        let hidden = self.cursor_hidden;
        let Some(drm) = &mut self.drm else {
            return;
        };
        if drm.cursor_hidden == hidden {
            return;
        }
        let Some(cursor) = &drm.cursor else {
            return;
        };
        let armed = if hidden { None } else { Some(&cursor.buffer) };
        match drm.device.set_cursor2(drm.crtc, armed, cursor.hotspot) {
            Ok(()) => drm.cursor_hidden = hidden,
            Err(err) => tracing::warn!("failed to toggle the hardware cursor: {err}"),
        }
    }
}

impl DrmOutput {
    #[allow(deprecated)]
    fn rearm_cursor(&self) {
        if let Some(cursor) = &self.cursor
            && !self.cursor_hidden
            && let Err(err) = self
                .device
                .set_cursor2(self.crtc, Some(&cursor.buffer), cursor.hotspot)
        {
            tracing::warn!("failed to re-arm hardware cursor after VT switch: {err}");
        }
    }
}

#[allow(deprecated)]
fn init_cursor(device: &DrmDevice, crtc: crtc::Handle) -> Option<HwCursor> {
    let size = device.cursor_size();
    let (w, h) = (size.w.max(1), size.h.max(1));
    let mut buffer = match device.create_dumb_buffer((w, h), DrmFourcc::Argb8888, 32) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!("no hardware cursor (create_dumb_buffer failed: {err})");
            return None;
        }
    };
    let drawn = match device.map_dumb_buffer(&mut buffer) {
        Ok(mut map) => {
            draw_cursor(&mut map, w, h);
            true
        }
        Err(err) => {
            tracing::warn!("no hardware cursor (map_dumb_buffer failed: {err})");
            false
        }
    };
    if !drawn {
        let _ = device.destroy_dumb_buffer(buffer);
        return None;
    }
    let hotspot = (0, 0);
    if let Err(err) = device.set_cursor2(crtc, Some(&buffer), hotspot) {
        tracing::warn!("no hardware cursor (set_cursor2 failed: {err}); pointer input still works");
        let _ = device.destroy_dumb_buffer(buffer);
        return None;
    }
    tracing::info!("hardware cursor plane active ({w}x{h})");
    Some(HwCursor { buffer, hotspot })
}

fn draw_cursor(map: &mut [u8], w: u32, h: u32) {
    const ART: &[&str] = &[
        "X",
        "XX",
        "X.X",
        "X..X",
        "X...X",
        "X....X",
        "X.....X",
        "X......X",
        "X.......X",
        "X........X",
        "X.........X",
        "X..........X",
        "X......XXXXXX",
        "X...X..X",
        "X..XX..X",
        "X.X X..X",
        "XX  X..X",
        "X    X..X",
        "     X..X",
        "     X..X",
        "      XX",
    ];
    let pitch = (map.len() / h.max(1) as usize) as u32;
    for (y, row) in ART.iter().enumerate() {
        if y as u32 >= h {
            break;
        }
        for (x, ch) in row.bytes().enumerate() {
            if x as u32 >= w {
                break;
            }
            // Little-endian ARGB8888 in memory is [B, G, R, A].
            let px = match ch {
                b'.' => [0xFF, 0xFF, 0xFF, 0xFF],
                b'X' => [0x00, 0x00, 0x00, 0xFF],
                _ => continue,
            };
            let off = (y as u32 * pitch + x as u32 * 4) as usize;
            if off + 4 <= map.len() {
                map[off..off + 4].copy_from_slice(&px);
            }
        }
    }
}

fn pick_output(
    drm: &DrmDevice,
    cfg: &crate::config::Output,
) -> Result<(connector::Handle, Mode, crtc::Handle), String> {
    let res = drm
        .resource_handles()
        .map_err(|e| format!("resource_handles: {e}"))?;
    let want_connector = (cfg.connector != "auto").then(|| cfg.connector.as_str());
    let want_mode = (cfg.mode != "auto").then(|| parse_mode_spec(&cfg.mode)).flatten();

    for conn_handle in res.connectors() {
        let Ok(conn) = drm.get_connector(*conn_handle, false) else {
            continue;
        };
        if conn.state() != connector::State::Connected {
            continue;
        }
        let name = format!("{}-{}", conn.interface().as_str(), conn.interface_id());
        if let Some(want) = want_connector
            && !name.eq_ignore_ascii_case(want)
        {
            continue;
        }
        let modes = conn.modes();
        if modes.is_empty() {
            continue;
        }
        let mode = want_mode
            .and_then(|(w, h, hz)| {
                modes.iter().find(|m| {
                    m.size() == (w, h) && hz.is_none_or(|hz| m.vrefresh() == hz)
                })
            })
            .or_else(|| modes.iter().find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED)))
            .copied()
            .unwrap_or(modes[0]);
        for enc_handle in conn.encoders() {
            let Ok(enc) = drm.get_encoder(*enc_handle) else {
                continue;
            };
            for crtc in res.filter_crtcs(enc.possible_crtcs()) {
                tracing::info!("selected connector {name}");
                return Ok((*conn_handle, mode, crtc));
            }
        }
    }
    match want_connector {
        Some(want) => Err(format!("configured connector {want:?} not connected / has no CRTC")),
        None => Err("no connected connector with a usable CRTC found".into()),
    }
}

fn parse_mode_spec(spec: &str) -> Option<(u16, u16, Option<u32>)> {
    let (dims, hz) = match spec.split_once('@') {
        Some((d, h)) => (d, h.trim().parse::<u32>().ok()),
        None => (spec, None),
    };
    let (w, h) = dims.trim().split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?, hz))
}

fn fmt_dev_t(dev: u64) -> String {
    format!("{}:{}", (dev >> 8) & 0xfff, dev & 0xff | ((dev >> 12) & !0xff))
}

fn pick_adapter(instance: &wgpu::Instance, render_dev: Option<u64>) -> Result<wgpu::Adapter, String> {
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    if adapters.is_empty() {
        return Err("no Vulkan adapter available".into());
    }
    let Some(target) = render_dev else {
        tracing::warn!("KMS device exposes no render node; using the first Vulkan adapter");
        return Ok(adapters.into_iter().next().unwrap());
    };
    if let Some(adapter) = adapters
        .iter()
        .find(|a| renderer::query_render_node_dev_t(a) == Some(target))
    {
        return Ok(adapter.clone());
    }

    // Rendering on a GPU that doesn't drive the display is a cross-GPU (PRIME) setup:
    // the two GPUs share no renderable modifier, so allocation fails or worse, the
    // render silently hangs. Refuse with something actionable instead currently only single-GPU
    let available = adapters
        .iter()
        .map(|a| {
            let node = renderer::query_render_node_dev_t(a)
                .map_or_else(|| "unknown".to_string(), fmt_dev_t);
            format!("{} (render node {node})", a.get_info().name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "no Vulkan adapter drives this display: the KMS device's render node is {}, but the \
         only Vulkan adapters are [{available}]. Spatil renders and scans out on one GPU, so \
         install the Vulkan driver for the GPU that owns the display (Intel: `vulkan-intel`, \
         AMD: `vulkan-radeon`), or point [output] at a connector on a GPU you can render on.",
        fmt_dev_t(target)
    ))
}

/// Builds the scanout ring, trying each renderable modifier in turn: a modifier can be
/// advertised yet still be unusable end-to-end (GBM may refuse to allocate it, or the
/// driver may reject the imported layout), and that only shows up when we try. Tiled
/// first, `Linear` last, Linear scans out correctly but is the slow path.
fn build_ring(
    device: &wgpu::Device,
    drm_fd: &DrmDeviceFd,
    allocator: &mut GbmAllocator<DrmDeviceFd>,
    modifiers: &[Modifier],
    width: u16,
    height: u16,
) -> Result<Vec<ScanoutBuffer>, String> {
    let mut candidates: Vec<Modifier> = modifiers
        .iter()
        .copied()
        .filter(|m| *m != Modifier::Linear)
        .collect();
    if modifiers.contains(&Modifier::Linear) {
        candidates.push(Modifier::Linear);
    }

    let mut last_err = "no candidate modifiers".to_string();
    for modifier in candidates {
        match try_build_ring(device, drm_fd, allocator, modifier, width, height) {
            Ok(ring) => {
                tracing::info!("scanout modifier: {modifier:?}");
                return Ok(ring);
            }
            Err(err) => {
                tracing::warn!("scanout modifier {modifier:?} unusable: {err}");
                last_err = err;
            }
        }
    }
    Err(format!("no usable scanout modifier ({last_err})"))
}

fn try_build_ring(
    device: &wgpu::Device,
    drm_fd: &DrmDeviceFd,
    allocator: &mut GbmAllocator<DrmDeviceFd>,
    modifier: Modifier,
    width: u16,
    height: u16,
) -> Result<Vec<ScanoutBuffer>, String> {
    let mut ring = Vec::with_capacity(RING_LEN);
    for i in 0..RING_LEN {
        let bo = allocator
            .create_buffer_with_flags(
                width as u32,
                height as u32,
                Fourcc::Xrgb8888,
                &[modifier],
                GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING,
            )
            .map_err(|e| format!("alloc: {e}"))?;
        let dmabuf = bo.export().map_err(|e| format!("export: {e}"))?;
        let chosen = dmabuf.format().modifier;
        // smithay silently falls back to an implicit modifier when allocation fails and
        // the list holds Linear/Invalid; an unknown layout must never reach the GPU
        if chosen != modifier {
            return Err(format!("GBM returned {chosen:?}, not the requested {modifier:?}"));
        }
        if i == 0 {
            tracing::info!(
                ?modifier,
                stride = dmabuf.strides().next().unwrap_or(0),
                offset = dmabuf.offsets().next().unwrap_or(0),
                planes = dmabuf.num_planes(),
                "probing scanout modifier"
            );
        }
        // texture_from_dmabuf_fd imports a single fd, so compressed/CCS modifiers
        // (which carry an auxiliary plane) can't be imported.
        if dmabuf.num_planes() != 1 {
            return Err(format!("{} planes; only single-plane is importable", dmabuf.num_planes()));
        }
        let texture = import_scanout_texture(device, &dmabuf, width as u32, height as u32)
            .map_err(|e| format!("wgpu import: {e}"))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let framebuffer = framebuffer_from_bo(drm_fd, &bo, true)
            .map_err(|e| format!("framebuffer_from_bo: {e}"))?;
        ring.push(ScanoutBuffer {
            _bo: bo,
            view,
            framebuffer,
        });
    }
    Ok(ring)
}

/// Imports a GBM scanout dmabuf as a wgpu `RENDER_ATTACHMENT`, the inverse usage of
/// `WgpuState::import_dmabuf`, sampled client buffers
fn import_scanout_texture(
    device: &wgpu::Device,
    dmabuf: &Dmabuf,
    width: u32,
    height: u32,
) -> Result<wgpu::Texture, String> {
    let fd = dmabuf
        .handles()
        .next()
        .ok_or_else(|| "dmabuf has no plane fd".to_string())?
        .try_clone_to_owned()
        .map_err(|e| format!("dup dmabuf fd: {e}"))?;
    let stride = u64::from(dmabuf.strides().next().unwrap_or(0));
    let offset = u64::from(dmabuf.offsets().next().unwrap_or(0));
    let modifier: u64 = dmabuf.format().modifier.into();

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let hal_desc = wgpu_hal::TextureDescriptor {
        label: Some("scanout render target"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUses::COLOR_TARGET,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: vec![],
    };
    // SAFETY: instance is Backends::VULKAN so the HAL downcast is correct; `fd` is a
    // freshly dup'd OwnedFd whose ownership transfers to Vulkan on success;
    // modifier/stride/offset are the GBM-chosen plane layout from the export.
    let hal_texture = unsafe {
        let hal_device = device
            .as_hal::<wgpu_hal::api::Vulkan>()
            .ok_or_else(|| "device is not Vulkan-backed".to_string())?;
        hal_device
            .texture_from_dmabuf_fd(fd, &hal_desc, modifier, stride, offset)
            .map_err(|e| format!("texture_from_dmabuf_fd: {e:?}"))?
    };
    let desc = wgpu::TextureDescriptor {
        label: Some("scanout render target"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    };
    // SAFETY: hal_texture was created from this device and matches `desc`. It is
    // fresh scanout memory we Clear before the first flip, so UNINITIALIZED is
    // correct (wgpu transitions it from UNDEFINED on first use).
    let texture = unsafe {
        device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(
            hal_texture,
            &desc,
            wgpu::TextureUses::UNINITIALIZED,
        )
    };
    Ok(texture)
}
