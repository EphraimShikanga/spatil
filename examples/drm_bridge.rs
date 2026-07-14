//! spike GBM bo -> dmabuf -> wgpu
//! RENDER_ATTACHMENT import -> KMS atomic page-flip.
//!
//! Run from a bare TTY (needs DRM master + a seat): `cargo run --example drm_bridge`.
//! Success = the screen cycles solid colors for a few seconds, driven entirely by
//! vblank page-flips, with no compositor and no scene.

use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Buffer as _, Modifier};
use smithay::backend::drm::gbm::{GbmFramebuffer, framebuffer_from_bo};
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType, PlaneConfig, PlaneState,
};
use smithay::backend::session::Session;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::udev;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc};
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Rectangle, Size, Transform};

const RING_LEN: usize = 3;
/// ~5 seconds at 60 Hz -- long enough to see the colors cycle, then exit cleanly.
const MAX_FRAMES: u64 = 300;

struct RingBuffer {
    /// Keeps the GBM allocation (and its dmabuf/fb) alive for the buffer's lifetime.
    _bo: GbmBuffer,
    view: wgpu::TextureView,
    framebuffer: GbmFramebuffer,
}

struct Spike {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: smithay::backend::drm::DrmSurface,
    ring: Vec<RingBuffer>,
    mode_size: (i32, i32),
    front: usize,
    frames: u64,
    first_flip_done: bool,
    signal: smithay::reexports::calloop::LoopSignal,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    tracing::info!("drm_bridge spike: GBM -> wgpu RENDER_ATTACHMENT -> KMS page-flip");

    if let Err(err) = run() {
        tracing::error!("spike failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (mut session, _session_notifier) = LibSeatSession::new()
        .map_err(|e| format!("failed to create libseat session (run from a TTY?): {e}"))?;
    let seat = session.seat();
    tracing::info!("session on seat {seat}");

    let path = udev::primary_gpu(&seat)
        .map_err(|e| format!("primary_gpu query failed: {e}"))?
        .or_else(|| {
            udev::all_gpus(&seat)
                .ok()
                .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0)))
        })
        .ok_or_else(|| "no GPU found".to_string())?;
    tracing::info!("primary GPU: {}", path.display());

    let kms_node = DrmNode::from_path(&path).map_err(|e| format!("not a DRM node: {e}"))?;
    // The render node dev_t is what wgpu's adapter must match so the dmabuf is shareable.
    let render_dev = kms_node
        .node_with_type(NodeType::Render)
        .and_then(|r| r.ok())
        .map(|n| n.dev_id());
    tracing::info!("KMS node render dev_t: {render_dev:?}");

    let flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let fd = session
        .open(&path, flags)
        .map_err(|e| format!("session failed to open {}: {e}", path.display()))?;
    let drm_fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (mut drm, drm_notifier) =
        DrmDevice::new(drm_fd.clone(), true).map_err(|e| format!("DrmDevice::new: {e}"))?;

    let gbm = GbmDevice::new(drm_fd.clone()).map_err(|e| format!("GbmDevice::new: {e}"))?;
    let mut allocator = GbmAllocator::new(gbm, GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING);

    let (connector, mode, crtc) = pick_output(&drm)?;
    let (mw, mh) = mode.size();
    let mode_size = (mw as i32, mh as i32);
    tracing::info!(
        "output: connector {:?} mode {}x{}@{} crtc {:?}",
        connector,
        mw,
        mh,
        mode.vrefresh(),
        crtc
    );

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pick_adapter(&instance, render_dev)?;
    tracing::info!("wgpu adapter: {}", adapter.get_info().name);

    if !adapter
        .features()
        .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
    {
        return Err("adapter lacks VULKAN_EXTERNAL_MEMORY_DMA_BUF -- dmabuf bridge impossible".into());
    }

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("drm_bridge spike"),
        required_features: wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF,
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        ..Default::default()
    }))
    .map_err(|e| format!("request_device: {e}"))?;

    let mut render_mods = query_render_modifiers(&adapter);
    if render_mods.is_empty() {
        render_mods.push(Modifier::Linear);
    }
    tracing::info!("candidate render+scanout modifiers: {render_mods:?}");

    let surface = drm
        .create_surface(crtc, mode, &[connector])
        .map_err(|e| format!("create_surface: {e}"))?;

    let mut ring = Vec::with_capacity(RING_LEN);
    for i in 0..RING_LEN {
        let bo = allocator
            .create_buffer_with_flags(
                mw as u32,
                mh as u32,
                Fourcc::Xrgb8888,
                &render_mods,
                GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING,
            )
            .map_err(|e| format!("buffer {i} alloc: {e}"))?;

        let dmabuf: Dmabuf = bo.export().map_err(|e| format!("buffer {i} export: {e}"))?;
        tracing::info!(
            "buffer {i}: chosen modifier {:?}, stride {}",
            dmabuf.format().modifier,
            dmabuf.strides().next().unwrap_or(0)
        );

        let texture = import_render_target(&device, &dmabuf, mw as u32, mh as u32)
            .map_err(|e| format!("buffer {i} wgpu import: {e}"))?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // use_opaque = true so XRGB is treated as opaque (no alpha) for scanout.
        let framebuffer = framebuffer_from_bo(&drm_fd, &bo, true)
            .map_err(|e| format!("buffer {i} framebuffer_from_bo: {e}"))?;

        ring.push(RingBuffer {
            _bo: bo,
            view,
            framebuffer,
        });
    }
    tracing::info!("allocated {} scanout buffers", ring.len());

    let mut event_loop: EventLoop<'static, Spike> =
        EventLoop::try_new().map_err(|e| format!("calloop: {e}"))?;
    let signal = event_loop.get_signal();

    event_loop
        .handle()
        .insert_source(drm_notifier, |event, _meta, spike: &mut Spike| {
            if let DrmEvent::VBlank(_crtc) = event {
                spike.frames += 1;
                if spike.frames >= MAX_FRAMES {
                    tracing::info!("reached {MAX_FRAMES} frames; spike succeeded, exiting");
                    spike.signal.stop();
                    return;
                }
                let next = (spike.front + 1) % spike.ring.len();
                if let Err(err) = spike.render_and_flip(next) {
                    tracing::error!("page-flip {} failed: {err}", spike.frames);
                    spike.signal.stop();
                }
            }
        })
        .map_err(|e| format!("insert drm source: {e}"))?;

    let mut spike = Spike {
        device,
        queue,
        surface,
        ring,
        mode_size,
        front: 0,
        frames: 0,
        first_flip_done: false,
        signal,
    };

    // Kick the loop: the first draw does the modeset via commit(); vblanks follow.
    spike
        .render_and_flip(0)
        .map_err(|e| format!("initial commit: {e}"))?;

    event_loop
        .run(Some(Duration::from_millis(16)), &mut spike, |_| {})
        .map_err(|e| format!("event loop: {e}"))?;

    tracing::info!("drm_bridge spike finished cleanly");
    Ok(())
}

impl Spike {
    fn render_and_flip(&mut self, idx: usize) -> Result<(), String> {
        // Cycle R -> G -> B every ~60 frames so motion is obvious on-screen.
        let phase = (self.frames / 60) % 3;
        let t = ((self.frames % 60) as f64) / 60.0;
        let color = match phase {
            0 => wgpu::Color { r: t, g: 0.0, b: 1.0 - t, a: 1.0 },
            1 => wgpu::Color { r: 1.0 - t, g: t, b: 0.0, a: 1.0 },
            _ => wgpu::Color { r: 0.0, g: 1.0 - t, b: t, a: 1.0 },
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spike clear"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("spike clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.ring[idx].view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        // The display engine scans out CPU-side once we flip, so the GPU write
        // must be complete first, a blocking poll is fine for a spike.
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("device poll: {e:?}"))?;

        let (w, h) = self.mode_size;
        let plane_state = PlaneState {
            handle: self.surface.plane(),
            config: Some(PlaneConfig {
                src: Rectangle::from_size(Size::from((w as f64, h as f64))),
                dst: Rectangle::from_size(Size::from((w, h))),
                transform: Transform::Normal,
                alpha: 1.0,
                damage_clips: None,
                fb: *self.ring[idx].framebuffer.as_ref(),
                fence: None,
            }),
        };

        if self.first_flip_done {
            self.surface
                .page_flip([plane_state], true)
                .map_err(|e| format!("page_flip: {e}"))?;
        } else {
            // First frame triggers the modeset.
            self.surface
                .commit([plane_state], true)
                .map_err(|e| format!("commit: {e}"))?;
            self.first_flip_done = true;
        }
        self.front = idx;
        Ok(())
    }
}

/// First connected connector, its preferred (or first) mode, and a usable CRTC.
fn pick_output(
    drm: &DrmDevice,
) -> Result<(connector::Handle, smithay::reexports::drm::control::Mode, crtc::Handle), String> {
    let res = drm
        .resource_handles()
        .map_err(|e| format!("resource_handles: {e}"))?;

    for conn_handle in res.connectors() {
        let conn = match drm.get_connector(*conn_handle, false) {
            Ok(c) => c,
            Err(_) => continue,
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

        // Find a CRTC reachable from any of this connector's encoders.
        for enc_handle in conn.encoders() {
            let enc = match drm.get_encoder(*enc_handle) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for crtc in res.filter_crtcs(enc.possible_crtcs()) {
                return Ok((*conn_handle, mode, crtc));
            }
        }
    }
    Err("no connected connector with a usable CRTC found".into())
}

/// Enumerate Vulkan adapters and pick the one whose DRM render node matches the
/// KMS card (so dmabufs are shareable). Falls back to the first adapter.
fn pick_adapter(
    instance: &wgpu::Instance,
    render_dev: Option<u64>,
) -> Result<wgpu::Adapter, String> {
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    if adapters.is_empty() {
        return Err("no Vulkan adapter available".into());
    }
    if let Some(target) = render_dev {
        for adapter in &adapters {
            if adapter_render_dev(adapter) == Some(target) {
                return Ok(adapter.clone());
            }
        }
        tracing::warn!(
            "no wgpu adapter matches KMS render node {target}; using first adapter (dmabuf sharing may fail on multi-GPU)"
        );
    }
    Ok(adapters.into_iter().next().unwrap())
}

/// The adapter's DRM render-node dev_t via VK_EXT_physical_device_drm (mirrors
/// `renderer::query_render_node_dev_t`, duplicated so the spike stays standalone).
fn adapter_render_dev(adapter: &wgpu::Adapter) -> Option<u64> {
    unsafe {
        let hal = adapter.as_hal::<wgpu_hal::api::Vulkan>()?;
        let shared = hal.shared_instance();
        if shared.instance_api_version() < ash::vk::API_VERSION_1_1 {
            return None;
        }
        let instance = shared.raw_instance();
        let phys = hal.raw_physical_device();
        let has_ext = instance
            .enumerate_device_extension_properties(phys)
            .ok()?
            .iter()
            .any(|ext| {
                ext.extension_name_as_c_str()
                    .is_ok_and(|n| n == ash::ext::physical_device_drm::NAME)
            });
        if !has_ext {
            return None;
        }
        let mut drm_props = ash::vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut props2 = ash::vk::PhysicalDeviceProperties2::default().push_next(&mut drm_props);
        instance.get_physical_device_properties2(phys, &mut props2);
        if drm_props.has_render == 0 {
            return None;
        }
        Some(libc::makedev(
            drm_props.render_major as libc::c_uint,
            drm_props.render_minor as libc::c_uint,
        ))
    }
}

/// DRM modifiers this Vulkan adapter can use as a COLOR_ATTACHMENT for XRGB8888
/// (BGRA byte order). The inverse of `renderer::query_dmabuf_modifiers`, which
/// checks SAMPLED_IMAGE for the client-import (sampling) path.
fn query_render_modifiers(adapter: &wgpu::Adapter) -> Vec<Modifier> {
    unsafe {
        let Some(hal) = adapter.as_hal::<wgpu_hal::api::Vulkan>() else {
            return vec![Modifier::Linear];
        };
        let shared = hal.shared_instance();
        if shared.instance_api_version() < ash::vk::API_VERSION_1_1 {
            return vec![Modifier::Linear];
        }
        let instance = shared.raw_instance();
        let phys = hal.raw_physical_device();

        let mut list = ash::vk::DrmFormatModifierPropertiesListEXT::default();
        {
            let mut fp2 = ash::vk::FormatProperties2::default().push_next(&mut list);
            instance.get_physical_device_format_properties2(
                phys,
                ash::vk::Format::B8G8R8A8_UNORM,
                &mut fp2,
            );
        }
        if list.drm_format_modifier_count == 0 {
            return vec![Modifier::Linear];
        }
        let mut props = vec![
            ash::vk::DrmFormatModifierPropertiesEXT::default();
            list.drm_format_modifier_count as usize
        ];
        list.p_drm_format_modifier_properties = props.as_mut_ptr();
        {
            let mut fp2 = ash::vk::FormatProperties2::default().push_next(&mut list);
            instance.get_physical_device_format_properties2(
                phys,
                ash::vk::Format::B8G8R8A8_UNORM,
                &mut fp2,
            );
        }

        let mut modifiers: Vec<Modifier> = props
            .into_iter()
            .filter(|p| {
                p.drm_format_modifier_tiling_features
                    .contains(ash::vk::FormatFeatureFlags::COLOR_ATTACHMENT)
            })
            .map(|p| Modifier::from(p.drm_format_modifier))
            .collect();
        // Linear last: correct everywhere, but slow -- only if nothing better negotiates.
        if !modifiers.contains(&Modifier::Linear) {
            modifiers.push(Modifier::Linear);
        }
        modifiers
    }
}

/// Import a GBM-allocated scanout dmabuf into wgpu as a RENDER_ATTACHMENT texture.
/// This is the inverse-usage of `renderer::import_dmabuf` (which imports client
/// buffers as sampled RESOURCE): here we render *into* the buffer, so the HAL
/// descriptor uses `COLOR_TARGET` and the wgpu texture uses `RENDER_ATTACHMENT`.
fn import_render_target(
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

    // SAFETY: instance is Backends::VULKAN so the HAL downcast is correct; `fd`
    // is a freshly dup'd OwnedFd whose ownership transfers to Vulkan on success;
    // modifier/stride/offset are the GBM-chosen plane layout from the export above.
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
    // SAFETY: hal_texture was created from this device and matches `desc`. The
    // buffer is fresh scanout memory we will Clear before the first flip, so
    // UNINITIALIZED is correct: wgpu transitions it from UNDEFINED on first use.
    let texture = unsafe {
        device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(
            hal_texture,
            &desc,
            wgpu::TextureUses::UNINITIALIZED,
        )
    };
    Ok(texture)
}
