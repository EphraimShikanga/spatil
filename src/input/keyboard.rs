//! Keyboard: raw `winit` key events → the seat's `wl_keyboard`.

use smithay::backend::input::KeyState;
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::utils::SERIAL_COUNTER;
use winit::event::{ElementState, KeyEvent};
use winit::platform::scancode::PhysicalKeyExtScancode;

use crate::state::SpatilState;

impl SpatilState {
    pub fn handle_keyboard(&mut self, event: KeyEvent) {
        // Skip OS-generated key repeats. Wayland clients run their own repeat
        // from the `repeat_info` the keyboard advertises, so forwarding these
        // too would double up and make typing feel erratic.
        if event.repeat {
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // winit gives a linux/evdev scancode; xkb keycodes are that + 8.
        let Some(scancode) = event.physical_key.to_scancode() else {
            return;
        };
        let keycode = Keycode::new(scancode + 8);
        let state = match event.state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        };
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();
        // Forward everything to the focused client; no compositor shortcuts yet.
        keyboard.input::<(), _>(self, keycode, state, serial, time, |_, _, _| {
            FilterResult::Forward
        });
    }
}
