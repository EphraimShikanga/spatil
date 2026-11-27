use std::time::Duration;

use smithay::delegate_xdg_activation;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use crate::state::SpatilState;

// Long enough for a single-instance app to hand a relaunch to its running copy,
// short enough that a stale token cannot steal focus later.
const TOKEN_LIFETIME: Duration = Duration::from_secs(10);

impl XdgActivationHandler for SpatilState {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.timestamp.elapsed() > TOKEN_LIFETIME {
            tracing::debug!("ignoring stale activation token for {:?}", surface.id());
            return;
        }
        let Some(entity) = self.entity_for_surface(&surface.id()) else {
            return;
        };
        self.activate_window(entity);
    }
}

delegate_xdg_activation!(SpatilState);
