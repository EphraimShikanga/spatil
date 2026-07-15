use smithay::backend::input::{Axis, AxisSource, ButtonState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

use crate::input::raycast::raycast_surface;
use crate::state::SpatilState;

impl SpatilState {
    pub fn handle_pointer_motion(&mut self, position: PhysicalPosition<f64>) {
        self.dispatch_pointer_motion_absolute(position.x, position.y);
    }

    pub fn dispatch_pointer_motion_absolute(&mut self, x: f64, y: f64) {
        self.pointer_location = (x, y).into();
        // NDC cursor for the Aether parallax camera (y up), in output pixel space.
        let (w, h) = (
            self.wgpu_state.width.max(1) as f64,
            self.wgpu_state.height.max(1) as f64,
        );
        self.wgpu_state.pointer_ndc = [
            (x / w * 2.0 - 1.0) as f32,
            (1.0 - y / h * 2.0) as f32,
        ];
        self.move_hw_cursor();
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let location = self.pointer_location;
        // Smithay derives the surface-local point as `event.location - focus.1`, so
        // focus.1 must be the surface's *origin*. Our mapping is a 3D raycast with no
        // linear origin, so hand it the offset that makes that subtraction reproduce the
        // hit point the raycast actually computed.
        let focus = raycast_surface(self, location).map(|(surface, hit)| (surface, location - hit));
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();
        pointer.motion(self, focus, &MotionEvent { location, serial, time });
        pointer.frame(self);
    }

    pub fn handle_pointer_button(&mut self, button: MouseButton, state: ElementState) {
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
        self.dispatch_pointer_button(button_code, button_state);
    }

    pub fn dispatch_pointer_button(&mut self, button_code: u32, button_state: ButtonState) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
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
        self.dispatch_pointer_axis(frame);
    }

    pub fn dispatch_pointer_axis(&mut self, frame: AxisFrame) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}
