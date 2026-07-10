use smithay::backend::allocator::Buffer as _;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::delegate_dmabuf;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};

use crate::state::SpatilState;

impl DmabufHandler for SpatilState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // Zero-copy import happens once here, at buffer-creation time; commits
        // then just look the texture up by wl_buffer id. Every path through
        // this function must resolve `notifier` exactly once (successful or
        // failed) -- the protocol requires a per-buffer import verdict, and
        // clients hang waiting for it otherwise.
        match self.wgpu_state.import_dmabuf(&dmabuf) {
            Ok((texture, width, height, uv_scale)) => match notifier.successful::<SpatilState>() {
                Ok(buffer) => {
                    tracing::info!(
                        buffer_id = ?buffer.id(),
                        width = texture.width(),
                        height = texture.height(),
                        fourcc = %dmabuf.format().code,
                        modifier = ?dmabuf.format().modifier,
                        stride = dmabuf.strides().next().unwrap_or(0),
                        offset = dmabuf.offsets().next().unwrap_or(0),
                        y_inverted = dmabuf.y_inverted(),
                        "imported dmabuf as zero-copy texture"
                    );
                    self.wgpu_state.dmabuf_textures.insert(buffer.id(), (texture, width, height, uv_scale));
                }
                Err(_) => {
                    tracing::warn!(
                        fourcc = %dmabuf.format().code,
                        modifier = ?dmabuf.format().modifier,
                        "dmabuf import succeeded but client died before wl_buffer could be cached"
                    );
                }
            },
            Err(err) => {
                tracing::warn!("dmabuf import rejected: {err}");
                notifier.failed();
            }
        }
    }
}

delegate_dmabuf!(SpatilState);
