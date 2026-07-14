use smithay::backend::input::{Axis, AxisSource, ButtonState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

use crate::input::raycast::raycast_surface;
use crate::state::SpatilState;

impl SpatilState {
    pub fn handle_pointer_motion(&mut self, position: PhysicalPosition<f64>) {
        self.pointer_location = (position.x, position.y).into();
        // NDC cursor for the Aether parallax camera (y up). Physical position and
        // surface config share the winit-window pixel space.
        let (w, h) = (
            self.wgpu_state.config.width.max(1) as f64,
            self.wgpu_state.config.height.max(1) as f64,
        );
        self.wgpu_state.pointer_ndc = [
            (position.x / w * 2.0 - 1.0) as f32,
            (1.0 - position.y / h * 2.0) as f32,
        ];
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let focus = raycast_surface(self, self.pointer_location);
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

        // Click-to-focus
        if button_state == ButtonState::Pressed
            && let Some(keyboard) = self.seat.get_keyboard()
        {
            let focus = raycast_surface(self, self.pointer_location).map(|(surface, _)| surface);
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
