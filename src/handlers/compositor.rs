use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
    TraversalAction, with_states, with_surface_tree_downward,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::shm::{self, ShmHandler, ShmState};
use smithay::{delegate_compositor, delegate_shm};

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
        let mut target_toplevel = None;
        for toplevel in &self.toplevels {
            if toplevel.wl_surface() == surface {
                target_toplevel = Some(toplevel.clone());
                break;
            }
            let mut found = false;
            with_surface_tree_downward(
                toplevel.wl_surface(),
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |s, _, _| {
                    if s == surface {
                        found = true;
                    }
                },
                |_, _, _| true,
            );
            if found {
                target_toplevel = Some(toplevel.clone());
                break;
            }
        }

        let is_toplevel = target_toplevel
            .as_ref()
            .map(|t| t.wl_surface() == surface)
            .unwrap_or(false);
        tracing::trace!(surface_id = ?surface.id(), is_toplevel, "Received a buffer commit!");

        let Some(toplevel) = target_toplevel else {
            // Not part of any tracked toplevel's surface tree.
            // Temporary until real surface-tree composition lands: we only draw
            // surfaces that belong to a known xdg_toplevel.
            return;
        };
        let target_surface_id = toplevel.wl_surface().id();

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
            if is_toplevel && !toplevel.ensure_configured() {
                return;
            }
        } else {
            // No buffer attached this commit -- nothing to import yet.
            return;
        }

        // First content commit: start the postponement clock and let scripts
        // place the window before it is revealed. Fires once per toplevel.
        if let Some(window_id) = self.mark_committed(&target_surface_id) {
            self.broadcast_event(&crate::ipc::SpatilEvent::new("window_committed", window_id));
        }

        let buffer = with_states(surface, |states| {
            match &states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer
            {
                Some(BufferAssignment::NewBuffer(buffer)) => Some(buffer.clone()),
                _ => None,
            }
        });
        let Some(buffer) = buffer else {
            return;
        };

        // Hardware path: dmabuf-backed buffers were already imported
        // (zero-copy) at buffer-creation time in `dmabuf_imported`; a commit
        // just attaches the cached texture to the committing surface's quad.
        if get_dmabuf(&buffer).is_ok() {
            if !self
                .wgpu_state
                .attach_dmabuf_texture(target_surface_id.clone(), &buffer.id())
            {
                tracing::warn!(surface_id = ?surface.id(), buffer_id = ?buffer.id(), "commit with a dmabuf buffer that was never imported");
            } else {
                tracing::trace!(surface_id = ?surface.id(), buffer_id = ?buffer.id(), "attached imported dmabuf texture to toplevel surface");
            }
            return;
        }

        // Software path: copy SHM bytes out of the client's pool and upload.
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
            Some((
                bytes,
                data.width as u32,
                data.height as u32,
                data.stride as u32,
            ))
        });

        match import_result {
            Ok(Some((bytes, width, height, stride))) => {
                // HACK: Ignore tiny subsurface commits (like Kitty's CSD window decorations)
                // so they don't overwrite the main terminal texture in our single-quad renderer.
                if !is_toplevel && (width < 100 || height < 100) {
                    tracing::info!(surface_id = ?surface.id(), width, height, "ignoring tiny subsurface SHM texture to prevent overwriting the main quad");
                } else {
                    tracing::info!(surface_id = ?surface.id(), width, height, stride, "uploading SHM texture for toplevel surface");
                    self.wgpu_state.upload_shm_texture(
                        target_surface_id,
                        width,
                        height,
                        stride,
                        &bytes,
                    );
                }
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
    fn buffer_destroyed(&mut self, buffer: &WlBuffer) {
        // Release any zero-copy texture imported from this buffer. A quad
        // still displaying it holds its own clone (wgpu refcounts internally),
        // so this only drops our cache entry, but without it we'd leak one
        // GPU texture per destroyed dmabuf buffer.
        self.wgpu_state.remove_dmabuf_texture(&buffer.id());
    }
}

impl ShmHandler for SpatilState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(SpatilState);
delegate_shm!(SpatilState);

use smithay::wayland::output::OutputHandler;
impl OutputHandler for SpatilState {}
smithay::delegate_output!(SpatilState);
