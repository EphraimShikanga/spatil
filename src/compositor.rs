use smithay::delegate_compositor;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};

use crate::client::ClientState;
use crate::state::SpatilState;

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
        // No-op until SHM/ImportMem and XdgShell land; this is where buffer
        // import and scene-graph surface updates will hook in.
    }
}

delegate_compositor!(SpatilState);
