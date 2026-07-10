//! Pointer: raw `winit` cursor/button/wheel events → the seat's `wl_pointer`.

use smithay::backend::input::{Axis, AxisSource, ButtonState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

use crate::state::SpatilState;

impl SpatilState {
    pub fn handle_pointer_motion(&mut self, position: PhysicalPosition<f64>) {
        self.pointer_location = (position.x, position.y).into();
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        // Focus origin at (0,0): the surface-local coord equals the pointer
        // location (see the coordinate note in this module's `mod.rs`).
        let focus = self
            .focus_surface()
            .map(|s| (s, Point::<f64, Logical>::from((0.0, 0.0))));
        let location = self.pointer_location;
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();
        pointer.motion(self, focus, &MotionEvent { location, serial, time });
        pointer.frame(self);
    }

    pub fn handle_pointer_button(&mut self, button: MouseButton, state: ElementState) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        // Button codes as defined in linux/input-event-codes.h (BTN_*), matching
        // Smithay's own winit backend mapping.
        let button_code = match button {
            MouseButton::Left => 0x110,
            MouseButton::Right => 0x111,
            MouseButton::Middle => 0x112,
            MouseButton::Forward => 0x115,
            MouseButton::Back => 0x116,
            MouseButton::Other(b) => b as u32,
        };
        let button_state = match state {
            ElementState::Pressed => ButtonState::Pressed,
            ElementState::Released => ButtonState::Released,
        };
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();

        // Click-to-focus: on press, give the pointed-at window keyboard focus.
        if button_state == ButtonState::Pressed
            && let Some(keyboard) = self.seat.get_keyboard()
        {
            let focus = self.focus_surface();
            keyboard.set_focus(self, focus, serial);
        }

        pointer.button(
            self,
            &ButtonEvent {
                serial,
                time,
                button: button_code,
                state: button_state,
            },
        );
        pointer.frame(self);
    }

    pub fn handle_pointer_axis(&mut self, delta: MouseScrollDelta) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let time = self.input_time();
        // Sign/scaling mirror Smithay's own winit backend: a wheel notch is 120
        // v120 units; pixel deltas are passed through negated.
        let frame = match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
                if x != 0.0 {
                    frame = frame.v120(Axis::Horizontal, (-x * 120.0) as i32);
                }
                if y != 0.0 {
                    frame = frame.v120(Axis::Vertical, (-y * 120.0) as i32);
                }
                frame
            }
            MouseScrollDelta::PixelDelta(pos) => {
                let mut frame = AxisFrame::new(time).source(AxisSource::Continuous);
                if pos.x != 0.0 {
                    frame = frame.value(Axis::Horizontal, -pos.x);
                }
                if pos.y != 0.0 {
                    frame = frame.value(Axis::Vertical, -pos.y);
                }
                frame
            }
        };
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}
