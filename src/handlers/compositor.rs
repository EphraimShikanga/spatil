use smithay::{delegate_compositor, delegate_shm};
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::shm::{ShmHandler, ShmState};

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

    fn commit(&mut self, _surface: &WlSurface) {
        tracing::info!("Received a buffer commit!");
        // Once buffer/texture import and the scene graph exist, this is where
        // on_commit_buffer_handler and surface updates will hook in.
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
