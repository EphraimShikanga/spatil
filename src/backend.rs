pub mod drm;
pub mod winit;

use std::sync::Arc;

use smithay::reexports::calloop::EventLoop as CalloopEventLoop;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::wayland::socket::ListeningSocketSource;

use crate::config::{self, SpatilConfig};
use crate::ipc;
use crate::state::SpatilState;
use crate::state::client::ClientState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Winit,
    Drm,
}

pub fn select() -> Backend {
    match std::env::var("SPATIL_BACKEND").ok().as_deref() {
        Some("winit") => Backend::Winit,
        Some("drm") => Backend::Drm,
        Some(other) => {
            tracing::warn!("unknown SPATIL_BACKEND={other:?}; auto-detecting");
            autodetect()
        }
        None => autodetect(),
    }
}

fn autodetect() -> Backend {
    let in_session =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if in_session {
        Backend::Winit
    } else {
        Backend::Drm
    }
}

pub fn run(config: SpatilConfig) {
    match select() {
        Backend::Winit => {
            tracing::info!("backend: winit (nested dev)");
            winit::run(config);
        }
        Backend::Drm => {
            tracing::info!("backend: drm (native)");
            drm::run(config);
        }
    }
}

/// The calloop loop plus the wayland/IPC/config-reload sources every backend needs;
/// the GPU and output are backend-specific and set up by the caller.
pub struct Harness {
    pub calloop: CalloopEventLoop<'static, SpatilState>,
    pub display_handle: DisplayHandle,
    /// Kept alive to hold the inotify watch open; dropping it stops hot-reload.
    pub config_watcher: Option<notify::RecommendedWatcher>,
}

pub fn harness(config: &SpatilConfig) -> Harness {
    let calloop = CalloopEventLoop::<'static, SpatilState>::try_new().unwrap();

    let (config_tx, config_channel) =
        smithay::reexports::calloop::channel::channel::<SpatilConfig>();
    calloop
        .handle()
        .insert_source(config_channel, |event, _, state: &mut SpatilState| {
            if let smithay::reexports::calloop::channel::Event::Msg(new_config) = event {
                state.apply_config(new_config);
            }
        })
        .expect("failed to insert the config-reload event source");
    let config_watcher = config::spawn_config_watcher(config_tx);

    if let Err(err) = ipc::init_ipc(&calloop.handle(), config) {
        tracing::warn!("failed to start IPC control socket: {err}; scripting disabled");
    }

    let wayland_display = Display::<SpatilState>::new().unwrap();
    let display_handle = wayland_display.handle();

    let display_source = Generic::new(wayland_display, Interest::READ, Mode::Level);
    calloop
        .handle()
        .insert_source(display_source, |_, display, state: &mut SpatilState| {
            // SAFETY: this Generic source owns the Display for its entire life.
            unsafe { display.get_mut() }.dispatch_clients(state).unwrap();
            Ok(PostAction::Continue)
        })
        .expect("failed to init the wayland display event source");

    let listening_socket =
        ListeningSocketSource::new_auto().expect("failed to init wayland socket");
    let socket_name = listening_socket
        .socket_name()
        .to_string_lossy()
        .into_owned();
    tracing::info!("Listening on WAYLAND_DISPLAY={socket_name}");
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket_name);
    }

    calloop
        .handle()
        .insert_source(listening_socket, |stream, _, state: &mut SpatilState| {
            if let Err(err) = state
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                tracing::warn!("Error adding wayland client: {err}");
            }
        })
        .expect("failed to init the wayland socket source");

    Harness {
        calloop,
        display_handle,
        config_watcher,
    }
}
