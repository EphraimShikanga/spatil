use smithay::backend::input::{Axis, AxisSource, ButtonState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

use crate::input::drag::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use crate::input::raycast::{raycast_entity, raycast_surface};
use crate::state::SpatilState;

impl SpatilState {
    pub fn handle_pointer_motion(&mut self, position: PhysicalPosition<f64>) {
        self.dispatch_pointer_motion_absolute(position.x, position.y);
    }

    pub fn dispatch_pointer_motion_absolute(&mut self, x: f64, y: f64) {
        self.note_input();
        self.pointer_location = (x, y).into();
        self.pointer_moved = true;
        if self.menu_is_open() {
            self.menu_hover(self.pointer_location);
        }
        let hovered_module = self.module_at(self.pointer_location);
        let on_orb =
            self.orb_contains(self.pointer_location) || self.orb_dragging() || hovered_module.is_some();
        self.set_orb_hover(on_orb);
        self.set_hovered_module(hovered_module);
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
        if self.orb_drag_motion(self.pointer_location) {
            return;
        }
        // A live drag owns the pointer; the client sees nothing until release.
        if self.update_drag(self.pointer_location) {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let location = self.pointer_location;
        let focus = if on_orb {
            None
        } else {
            self.layer_surface_under(location)
                .or_else(|| raycast_surface(self, location))
                .map(|(surface, hit)| (surface, location - hit))
        };
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();
        pointer.motion(self, focus, &MotionEvent { location, serial, time });
        pointer.frame(self);
    }

    pub fn handle_pointer_button(&mut self, button: MouseButton, state: ElementState) {
        let button_code = match button {
            MouseButton::Left => BTN_LEFT,
            MouseButton::Right => BTN_RIGHT,
            MouseButton::Middle => BTN_MIDDLE,
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
        self.note_input();
        if self.menu_is_open() {
            if button_state == ButtonState::Pressed {
                match self.menu_at(self.pointer_location) {
                    Some(index) => self.menu_activate(index),
                    None if !self.menu_contains(self.pointer_location) => {
                        self.close_menu();
                    }
                    None => {}
                }
            }
            return;
        }
        if button_state == ButtonState::Released && self.orb_release() {
            return;
        }
        if button_state == ButtonState::Released && self.end_drag() {
            return;
        }
        if button_state == ButtonState::Pressed
            && self.module_press(self.pointer_location, button_code)
        {
            return;
        }
        if button_state == ButtonState::Released && self.module_at(self.pointer_location).is_some() {
            return;
        }
        if button_state == ButtonState::Pressed
            && button_code == BTN_LEFT
            && self.orb_press(self.pointer_location)
        {
            return;
        }
        let on_hud = self.layer_surface_under(self.pointer_location).is_some();
        if button_state == ButtonState::Pressed && !on_hud {
            let hit = raycast_entity(self, self.pointer_location).map(|h| h.entity);
            if self.dismiss_on_backdrop(hit) {
                return;
            }
            if self.begin_drag(button_code, self.pointer_location) {
                return;
            }
        }

        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();

        // Click-to-focus
        if button_state == ButtonState::Pressed
            && let Some(keyboard) = self.seat.get_keyboard()
            && !keyboard.is_grabbed()
            && self.exclusive_layer().is_none()
        {
            let hud = self
                .layer_surface_under(self.pointer_location)
                .map(|(surface, _)| surface);
            match hud {
                Some(surface) => {
                    if self.layer_accepts_focus(&surface) {
                        keyboard.set_focus(self, Some(surface), serial);
                    }
                }
                None => {
                    let focus =
                        raycast_surface(self, self.pointer_location).map(|(surface, _)| surface);
                    keyboard.set_focus(self, focus, serial);
                }
            }
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
        self.note_input();
        if let Some(steps) = wheel_steps(&frame)
            && self.module_scroll(self.pointer_location, -steps.round() as i32)
        {
            return;
        }
        // Vertical scroll under the world modifier is ours; horizontal and
        // unmodified scroll is the client's.
        if let Some(steps) = wheel_steps(&frame)
            && self.wheel_depth_or_zoom(steps)
        {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}

fn wheel_steps(frame: &AxisFrame) -> Option<f32> {
    if let Some((_, v120)) = frame.v120
        && v120 != 0
    {
        return Some(-v120 as f32 / 120.0);
    }
    // Touchpad scroll is continuous pixels; approximate a notch.
    let (_, value) = frame.axis;
    (value.abs() > f64::EPSILON).then(|| -value as f32 / CONTINUOUS_PER_NOTCH)
}

const CONTINUOUS_PER_NOTCH: f32 = 10.0;
