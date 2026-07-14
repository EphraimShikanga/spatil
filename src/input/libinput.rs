use smithay::backend::input::{
    Axis, AxisSource, InputEvent, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent,
};
use smithay::backend::libinput::LibinputInputBackend;
use smithay::input::pointer::AxisFrame;

use crate::state::SpatilState;

impl SpatilState {
    pub fn process_input_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        match event {
            InputEvent::Keyboard { event } => {
                // key_code() is already the xkb keycode (evdev + 8).
                self.dispatch_key(event.key_code(), event.state());
            }
            InputEvent::PointerMotion { event } => {
                // Relative accelerated delta, raw feels sluggish, accumulated
                // into the cursor position and clamped to the output.
                let delta = event.delta();
                let w = self.wgpu_state.width.max(1) as f64;
                let h = self.wgpu_state.height.max(1) as f64;
                let x = (self.pointer_location.x + delta.x).clamp(0.0, w - 1.0);
                let y = (self.pointer_location.y + delta.y).clamp(0.0, h - 1.0);
                self.dispatch_pointer_motion_absolute(x, y);
            }
            InputEvent::PointerButton { event } => {
                self.dispatch_pointer_button(event.button_code(), event.state());
            }
            InputEvent::PointerAxis { event } => {
                let frame = axis_frame(&event, self.input_time());
                self.dispatch_pointer_axis(frame);
            }

            InputEvent::GestureSwipeBegin { .. }
            | InputEvent::GestureSwipeUpdate { .. }
            | InputEvent::GestureSwipeEnd { .. }
            | InputEvent::GesturePinchBegin { .. }
            | InputEvent::GesturePinchUpdate { .. }
            | InputEvent::GesturePinchEnd { .. }
            | InputEvent::GestureHoldBegin { .. }
            | InputEvent::GestureHoldEnd { .. } => {
                tracing::trace!("touchpad gesture (unhandled until phase 3)");
            }
            InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } => {}
            _ => {}
        }
    }
}

fn axis_frame(
    event: &<LibinputInputBackend as smithay::backend::input::InputBackend>::PointerAxisEvent,
    time: u32,
) -> AxisFrame {
    let source = event.source();
    let mut frame = AxisFrame::new(time).source(source);
    for axis in [Axis::Horizontal, Axis::Vertical] {
        if source == AxisSource::Wheel
            && let Some(steps) = event.amount_v120(axis)
        {
            frame = frame.v120(axis, steps as i32);
        }
        match event.amount(axis) {
            Some(0.0) => frame = frame.stop(axis),
            Some(value) => frame = frame.value(axis, value),
            None => {}
        }
    }
    frame
}
