mod commands;
pub mod protocol;

pub use protocol::SpatilEvent;

use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};

use crate::config::SpatilConfig;
use crate::state::SpatilState;
use protocol::{SpatilCommand, socket_path};

/// Reject an unterminated command line longer than this, bounding the buffer.
const MAX_COMMAND_SIZE: usize = 64 * 1024;

/// Binds the control socket (0600, owner-only) and registers it — plus every
/// accepted connection — with `calloop`. A stale socket from a prior run is
/// removed first.
pub fn init_ipc(
    loop_handle: &LoopHandle<'static, SpatilState>,
    config: &SpatilConfig,
) -> std::io::Result<PathBuf> {
    let path = socket_path(&config.general.socket_name);
    std::fs::remove_file(&path).ok();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("IPC control socket listening at {}", path.display());

    let source = Generic::new(listener, Interest::READ, Mode::Level);
    loop_handle
        .insert_source(source, |_, listener, state| {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => accept_ipc_client(state, stream),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                    // A transient accept error must never tear down the compositor.
                    Err(err) => {
                        tracing::warn!("IPC accept error: {err}");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        })
        .map_err(|err| std::io::Error::other(format!("failed to insert IPC source: {err}")))?;

    Ok(path)
}

/// Registers an accepted connection as its own `calloop` source. The read buffer
/// lives in the callback (calloop's `NoIoDrop` only lends a shared ref); a
/// write-side clone goes into `state.ipc_clients` for broadcasts.
fn accept_ipc_client(state: &mut SpatilState, stream: UnixStream) {
    if let Err(err) = stream.set_nonblocking(true) {
        tracing::warn!("IPC: set_nonblocking failed: {err}");
        return;
    }
    match stream.try_clone() {
        Ok(write_half) => state.ipc_clients.push(write_half),
        Err(err) => {
            tracing::warn!("IPC: failed to clone client stream: {err}");
            return;
        }
    }

    let mut buffer: Vec<u8> = Vec::with_capacity(256);
    let source = Generic::new(stream, Interest::READ, Mode::Level);
    let registered = state.loop_handle.insert_source(source, move |_, stream, state| {
        Ok(state.pump_ipc_client(&**stream, &mut buffer))
    });
    if let Err(err) = registered {
        tracing::warn!("IPC: failed to register client connection: {err}");
    }
}

impl SpatilState {
    /// Writes an event to every connected script, dropping any whose write fails.
    pub fn broadcast_event(&mut self, event: &SpatilEvent) {
        if self.ipc_clients.is_empty() {
            return;
        }
        let mut line = match serde_json::to_vec(event) {
            Ok(line) => line,
            Err(err) => {
                tracing::warn!("failed to serialize IPC event: {err}");
                return;
            }
        };
        line.push(b'\n');
        self.ipc_clients.retain_mut(|client| match client.write_all(&line) {
            Ok(()) => true,
            Err(err) => {
                tracing::debug!("dropping IPC client after write error: {err}");
                false
            }
        });
    }

    /// Drains a client stream, applying each complete command line. `buffer`
    /// carries a partial trailing line between wakeups.
    pub fn pump_ipc_client(&mut self, mut stream: &UnixStream, buffer: &mut Vec<u8>) -> PostAction {
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => return PostAction::Remove,
                Ok(n) => {
                    buffer.extend_from_slice(&chunk[..n]);
                    if buffer.len() > MAX_COMMAND_SIZE {
                        tracing::warn!("IPC command line too large; dropping client");
                        return PostAction::Remove;
                    }
                    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buffer.drain(..=pos).collect();
                        self.process_ipc_line(&line[..line.len() - 1]);
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => return PostAction::Continue,
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => {
                    tracing::debug!("IPC client read error, dropping: {err}");
                    return PostAction::Remove;
                }
            }
        }
    }

    fn process_ipc_line(&mut self, line: &[u8]) {
        if line.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        match serde_json::from_slice::<SpatilCommand>(line) {
            Ok(cmd) => self.apply_ipc_command(cmd),
            Err(err) => tracing::warn!("ignoring malformed IPC command: {err}"),
        }
    }
}
