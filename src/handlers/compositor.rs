use smithay::{delegate_compositor, delegate_shm};
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
    SurfaceAttributes, with_states,
};
use smithay::wayland::shm::{self, ShmHandler, ShmState};

use crate::state::SpatilState;
use crate::state::client::ClientState;

impl CompositorHandler for SpatilState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("client has no ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        tracing::info!("Received a buffer commit!");

        let has_buffer = with_states(surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer
                .is_some()
        });

        // xdg-shell handshake enforcement: a toplevel client must not attach a
        // buffer until it has acked the initial configure. If it commits pixels
        // early, `ensure_configured` posts a fatal `NotConstructed` protocol error
        // and the client is disconnected. We only check on commits that actually
        // attach a buffer — the initial bufferless commit that kicks off the
        // handshake is legal and must be allowed through.
        if has_buffer {
            if let Some(toplevel) = self.toplevels.iter().find(|t| t.wl_surface() == surface) {
                if !toplevel.ensure_configured() {
                    return;
                }
            }
        } else {
            // No buffer attached this commit -- nothing to import yet.
            return;
        }

        let buffer = with_states(surface, |states| {
            match &states.cached_state.get::<SurfaceAttributes>().current().buffer {
                Some(BufferAssignment::NewBuffer(buffer)) => Some(buffer.clone()),
                _ => None,
            }
        });
        let Some(buffer) = buffer else {
            return;
        };

        let import_result = shm::with_buffer_contents(&buffer, |ptr, len, data| {
            if !matches!(
                data.format,
                wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888
            ) {
                return None;
            }
            // `ptr`/`len` describe the WHOLE mapped SHM pool, not just this
            // buffer -- clients commonly double-buffer inside one pool, so the
            // active buffer's pixels start at `data.offset`, not byte 0.
            let offset = data.offset as usize;
            let required = data.stride as usize * data.height as usize;
            if offset + required > len {
                tracing::warn!(
                    "SHM buffer out of bounds (offset={offset}, required={required}, pool len={len})"
                );
                return None;
            }
            // SAFETY: `ptr` is valid for `len` bytes for the duration of this
            // closure, per `with_buffer_contents`'s contract; `offset + required`
            // was just bounds-checked against `len` above.
            let bytes = unsafe { std::slice::from_raw_parts(ptr.add(offset), required) }.to_vec();
            Some((bytes, data.width as u32, data.height as u32, data.stride as u32))
        });

        match import_result {
            Ok(Some((bytes, width, height, stride))) => {
                self.wgpu_state
                    .upload_shm_texture(surface.id(), width, height, stride, &bytes);
            }
            Ok(None) => {
                tracing::warn!("unsupported SHM buffer format, skipping texture import");
            }
            Err(err) => {
                tracing::warn!("failed to access SHM buffer contents: {err}");
            }
        }
    }
}

impl BufferHandler for SpatilState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for SpatilState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(SpatilState);
delegate_shm!(SpatilState);
