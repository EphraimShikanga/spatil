use smithay::delegate_xdg_activation;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::SpatilState;

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
        // Here we could focus the window, but we will leave it as a stub
        // to satisfy GLFW and avoid the warning.
        tracing::debug!(
            "Activation request for surface {:?} with token {:?}",
            surface,
            token_data
        );
    }

}

delegate_xdg_activation!(SpatilState);
