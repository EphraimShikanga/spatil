use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Buffer as _, Fourcc, Modifier};
use smithay::backend::drm::gbm::{GbmFramebuffer, framebuffer_from_bo};
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, DrmSurface, NodeType, PlaneConfig, PlaneState,
};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev;
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

struct ScanoutBuffer {
    _bo: GbmBuffer, // owns the GBM allocation the view + fb alias
    view: wgpu::TextureView,
    framebuffer: GbmFramebuffer,
}

pub struct DrmOutput {
    device: DrmDevice,
    _gbm: GbmDevice<DrmDeviceFd>, // keeps the buffers/framebuffers valid
    surface: DrmSurface,
    ring: Vec<ScanoutBuffer>,
    front: usize,
    first_flip_done: bool,
    mode_size: (i32, i32),
    active: bool,
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

    let (connector, mode, crtc) = pick_output(&drm)?;
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

    let mut render_mods = renderer::query_render_modifiers(&adapter);
    if !render_mods.contains(&Modifier::Linear) {
        render_mods.push(Modifier::Linear);
    }
    tracing::info!("candidate scanout modifiers: {render_mods:?}");

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
        device: drm,
        _gbm: gbm,
        surface,
        ring,
        front: 0,
        first_flip_done: false,
        mode_size: (mw as i32, mh as i32),
        active: true,
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
                    state.render_drm_frame(true);
                }
            }
        })
        .map_err(|e| format!("insert session source: {e}"))?;

    calloop
        .handle()
        .insert_source(drm_notifier, |event, _meta, state: &mut SpatilState| {
            if let DrmEvent::VBlank(_crtc) = event {
                state.render_drm_frame(false);
            }
        })
        .map_err(|e| format!("insert drm source: {e}"))?;

    // First frame does the modeset via commit(); vblanks drive the rest.
    state.render_drm_frame(true);

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
    /// Renders the scene into the next scanout buffer and flips to it. `initial` forces
    /// buffer 0 + a modeset `commit` (startup / after VT resume); else round-robin page-flip.
    pub fn render_drm_frame(&mut self, initial: bool) {
        {
            let Some(drm) = self.drm.as_ref() else { return };
            if !drm.active {
                return;
            }
        }

        self.reveal_pending();
        self.tick_animations();

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

        // Scanout reads CPU-side on flip, so the GPU write must finish first. A DRM
        // in-fence will replace this blocking poll.
        if let Err(err) = self.wgpu_state.device.poll(wgpu::PollType::wait_indefinitely()) {
            tracing::error!("device poll failed: {err:?}");
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
}

/// First connected connector, its preferred (or first) mode, and a usable CRTC.
fn pick_output(drm: &DrmDevice) -> Result<(connector::Handle, Mode, crtc::Handle), String> {
    let res = drm
        .resource_handles()
        .map_err(|e| format!("resource_handles: {e}"))?;
    for conn_handle in res.connectors() {
        let Ok(conn) = drm.get_connector(*conn_handle, false) else {
            continue;
        };
        if conn.state() != connector::State::Connected {
            continue;
        }
        let modes = conn.modes();
        if modes.is_empty() {
            continue;
        }
        let mode = *modes
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(&modes[0]);
        for enc_handle in conn.encoders() {
            let Ok(enc) = drm.get_encoder(*enc_handle) else {
                continue;
            };
            for crtc in res.filter_crtcs(enc.possible_crtcs()) {
                return Ok((*conn_handle, mode, crtc));
            }
        }
    }
    Err("no connected connector with a usable CRTC found".into())
}

/// Prefer the adapter matching the KMS render node (shareable dmabufs), else the
/// first,, the NVIDIA driver's render-node match can miss
fn pick_adapter(instance: &wgpu::Instance, render_dev: Option<u64>) -> Result<wgpu::Adapter, String> {
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    if adapters.is_empty() {
        return Err("no Vulkan adapter available".into());
    }
    if let Some(target) = render_dev {
        if let Some(a) = adapters
            .iter()
            .find(|a| renderer::query_render_node_dev_t(a) == Some(target))
        {
            return Ok(a.clone());
        }
        tracing::warn!(
            "no wgpu adapter matches KMS render node {target}; using first adapter \
             (dmabuf sharing may fail on multi-GPU)"
        );
    }
    Ok(adapters.into_iter().next().unwrap())
}

/// Allocates the GBM scanout ring, importing each bo as a wgpu render target + KMS fb.
fn build_ring(
    device: &wgpu::Device,
    drm_fd: &DrmDeviceFd,
    allocator: &mut GbmAllocator<DrmDeviceFd>,
    modifiers: &[Modifier],
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
                modifiers,
                GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING,
            )
            .map_err(|e| format!("buffer {i} alloc: {e}"))?;
        let dmabuf = bo.export().map_err(|e| format!("buffer {i} export: {e}"))?;
        if i == 0 {
            tracing::info!("scanout modifier chosen by GBM: {:?}", dmabuf.format().modifier);
        }
        let texture = import_scanout_texture(device, &dmabuf, width as u32, height as u32)
            .map_err(|e| format!("buffer {i} wgpu import: {e}"))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let framebuffer = framebuffer_from_bo(drm_fd, &bo, true)
            .map_err(|e| format!("buffer {i} framebuffer_from_bo: {e}"))?;
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
