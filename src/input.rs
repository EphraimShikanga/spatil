pub mod drag;
mod keyboard;
mod libinput;
mod pointer;
pub mod raycast;

use crate::state::SpatilState;

impl SpatilState {
    /// Milliseconds since compositor start, for input event timestamps.
    fn input_time(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }
}
