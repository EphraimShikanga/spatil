//! Translation of raw `winit` window events into Smithay seat input.
//!
//! Spatil drives `winit` directly (not Smithay's winit backend), so we get raw
//! `WindowEvent`s and forward them into the seat's keyboard/pointer by hand.
//! `main.rs` calls the `handle_*` methods; keyboard lives in [`keyboard`],
//! pointer in [`pointer`].
//!
//! This is deliberately minimal: every key is forwarded to the focused client
//! (no compositor shortcuts yet) and pointer events go to the most-recently
//! opened toplevel. There is no 3D raycast hit-testing yet, so the focused
//! surface is treated as if its origin sits at the compositor origin — pointer
//! coordinates won't line up with where the quad is drawn in 3D. That is fine
//! for making apps interactive now; precise hit-testing is future work.

mod keyboard;
mod pointer;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::SpatilState;

impl SpatilState {
    /// Milliseconds since compositor start, for input event timestamps.
    fn input_time(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    /// The surface input is routed to: the most recently opened toplevel, if any.
    fn focus_surface(&self) -> Option<WlSurface> {
        self.toplevels.last().map(|t| t.wl_surface().clone())
    }
}
