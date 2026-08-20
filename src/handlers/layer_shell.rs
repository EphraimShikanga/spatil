use smithay::delegate_layer_shell;
use smithay::desktop::{self, PopupKind, layer_map_for_output};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::PopupSurface;

use crate::state::SpatilState;

impl WlrLayerShellHandler for SpatilState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        if matches!(layer, Layer::Background | Layer::Bottom) {
            tracing::warn!(
                %namespace,
                ?layer,
                "background/bottom layer surfaces are arranged but not composited"
            );
        }

        let mapped = desktop::LayerSurface::new(surface, namespace);
        let mut map = layer_map_for_output(&self.output);
        if let Err(err) = map.map_layer(&mapped) {
            tracing::warn!("failed to map layer surface: {err}");
        }
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let wl_surface = surface.wl_surface().clone();
        self.despawn_layer_popups(&wl_surface);
        {
            let mut map = layer_map_for_output(&self.output);
            let mapped = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            if let Some(mapped) = mapped {
                map.unmap_layer(&mapped);
            }
        }
        self.wgpu_state.remove_texture(&wl_surface.id());
        self.refresh_layer_focus();
    }

    fn new_popup(&mut self, _parent: LayerSurface, popup: PopupSurface) {
        let popup = PopupKind::Xdg(popup);
        if let Err(err) = self.popups.track_popup(popup.clone()) {
            tracing::warn!("failed to track layer popup: {err}");
            return;
        }
        if let PopupKind::Xdg(surface) = &popup
            && let Err(err) = surface.send_configure()
        {
            tracing::warn!("failed to configure layer popup: {err:?}");
        }
    }
}

delegate_layer_shell!(SpatilState);
