use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer as _, Fourcc, Modifier};

use super::WgpuState;

impl WgpuState {
    pub fn import_dmabuf(
        &self,
        dmabuf: &Dmabuf,
    ) -> Result<(wgpu::Texture, u32, u32, [f32; 2]), String> {
        if dmabuf.num_planes() != 1 {
            return Err(format!(
                "unsupported plane count {} (single-plane only)",
                dmabuf.num_planes()
            ));
        }
        let format = dmabuf.format();
        let texture_format = match format.code {
            Fourcc::Argb8888 | Fourcc::Xrgb8888 => wgpu::TextureFormat::Bgra8UnormSrgb,
            other => return Err(format!("unsupported fourcc {other:?}")),
        };

        tracing::info!(
            fourcc = %format.code,
            modifier = ?format.modifier,
            width = dmabuf.width(),
            height = dmabuf.height(),
            stride = dmabuf.strides().next().unwrap_or(0),
            offset = dmabuf.offsets().next().unwrap_or(0),
            y_inverted = dmabuf.y_inverted(),
            wgpu_format = ?texture_format,
            "attempting dmabuf import"
        );

        // texture_from_dmabuf_fd consumes an OwnedFd (Vulkan takes ownership on
        // success, closes it on failure), Smithay only lends BorrowedFds, so dup
        // first or we double-close.
        let fd = dmabuf
            .handles()
            .next()
            .ok_or_else(|| "dmabuf has no plane fd".to_string())?
            .try_clone_to_owned()
            .map_err(|e| format!("failed to dup dmabuf fd: {e}"))?;
        let stride = u64::from(dmabuf.strides().next().unwrap_or(0));
        let offset = u64::from(dmabuf.offsets().next().unwrap_or(0));

        let client_width = dmabuf.width();
        let client_height = dmabuf.height();

        let bytes_per_pixel = 4;
        let texture_width = if format.modifier == Modifier::Linear && stride > 0 {
            (stride as u32) / bytes_per_pixel
        } else {
            client_width
        };
        let uv_scale_x = client_width as f32 / texture_width as f32;

        let size = wgpu::Extent3d {
            width: texture_width,
            height: client_height,
            depth_or_array_layers: 1,
        };

        if dmabuf.y_inverted() {
            tracing::warn!("dmabuf is y-inverted; content will render flipped (not yet handled)");
        }

        let hal_desc = wgpu_hal::TextureDescriptor {
            label: Some("dmabuf imported texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: vec![],
        };

        // SAFETY: the instance is Backends::VULKAN so the HAL downcast is correct;
        // `fd` is a freshly dup'd OwnedFd whose ownership transfers to Vulkan;
        // modifier/stride/offset are the plane metadata Smithay validated.
        let hal_texture = unsafe {
            let hal_device = self
                .device
                .as_hal::<wgpu_hal::api::Vulkan>()
                .ok_or_else(|| "device is not backed by Vulkan".to_string())?;
            hal_device
                .texture_from_dmabuf_fd(fd, &hal_desc, format.modifier.into(), stride, offset)
                .map_err(|e| format!("Vulkan dmabuf import failed: {e:?}"))?
        };

        let desc = wgpu::TextureDescriptor {
            label: Some("dmabuf imported texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        // SAFETY: hal_texture was created from this device and matches `desc`. The
        // dmabuf holds initialized client pixels, so RESOURCE (not UNINITIALIZED,
        // which would license discarding them) is the correct first-use state.
        let texture = unsafe {
            self.device.create_texture_from_hal::<wgpu_hal::api::Vulkan>(
                hal_texture,
                &desc,
                wgpu::TextureUses::RESOURCE,
            )
        };
        Ok((texture, client_width, client_height, [uv_scale_x, 1.0]))
    }
}

pub(crate) fn query_render_node_dev_t(adapter: &wgpu::Adapter) -> Option<u64> {
    // SAFETY: read-only property queries against the adapter's physical device;
    // the `as_hal` guard is dropped before returning.
    unsafe {
        let hal_adapter = adapter.as_hal::<wgpu_hal::api::Vulkan>()?;
        let shared = hal_adapter.shared_instance();
        if shared.instance_api_version() < ash::vk::API_VERSION_1_1 {
            return None;
        }
        let instance = shared.raw_instance();
        let phys = hal_adapter.raw_physical_device();

        // Chaining an unsupported extension struct is invalid, so probe first.
        let ext_supported = instance
            .enumerate_device_extension_properties(phys)
            .ok()?
            .iter()
            .any(|ext| {
                ext.extension_name_as_c_str()
                    .is_ok_and(|name| name == ash::ext::physical_device_drm::NAME)
            });
        if !ext_supported {
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

fn query_format_modifiers(
    adapter: &wgpu::Adapter,
    tiling_feature: ash::vk::FormatFeatureFlags,
) -> Vec<Modifier> {
    unsafe {
        let Some(hal_adapter) = adapter.as_hal::<wgpu_hal::api::Vulkan>() else {
            return vec![Modifier::Linear];
        };
        let shared = hal_adapter.shared_instance();
        let instance = shared.raw_instance();
        let phys = hal_adapter.raw_physical_device();

        if shared.instance_api_version() < ash::vk::API_VERSION_1_1 {
            return vec![Modifier::Linear];
        }

        let mut modifier_list = ash::vk::DrmFormatModifierPropertiesListEXT::default();
        {
            let mut format_props2 =
                ash::vk::FormatProperties2::default().push_next(&mut modifier_list);
            instance.get_physical_device_format_properties2(
                phys,
                ash::vk::Format::B8G8R8A8_UNORM,
                &mut format_props2,
            );
        }

        if modifier_list.drm_format_modifier_count == 0 {
            return vec![Modifier::Linear];
        }

        let mut modifiers = Vec::with_capacity(modifier_list.drm_format_modifier_count as usize);
        let mut props = vec![
            ash::vk::DrmFormatModifierPropertiesEXT::default();
            modifier_list.drm_format_modifier_count as usize
        ];
        modifier_list.p_drm_format_modifier_properties = props.as_mut_ptr();

        {
            let mut format_props2 =
                ash::vk::FormatProperties2::default().push_next(&mut modifier_list);
            instance.get_physical_device_format_properties2(
                phys,
                ash::vk::Format::B8G8R8A8_UNORM,
                &mut format_props2,
            );
        }

        for prop in props {
            if prop.drm_format_modifier_tiling_features.contains(tiling_feature) {
                modifiers.push(Modifier::from(prop.drm_format_modifier));
            }
        }

        if modifiers.is_empty() {
            modifiers.push(Modifier::Linear);
        }
        modifiers
    }
}

pub fn query_dmabuf_modifiers(adapter: &wgpu::Adapter) -> Vec<Modifier> {
    query_format_modifiers(adapter, ash::vk::FormatFeatureFlags::SAMPLED_IMAGE)
}

pub fn query_render_modifiers(adapter: &wgpu::Adapter) -> Vec<Modifier> {
    query_format_modifiers(adapter, ash::vk::FormatFeatureFlags::COLOR_ATTACHMENT)
}
