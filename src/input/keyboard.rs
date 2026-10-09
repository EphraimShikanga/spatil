use smithay::backend::input::KeyState;
use smithay::input::keyboard::{FilterResult, Keycode, Keysym, keysyms};
use smithay::utils::SERIAL_COUNTER;
use winit::event::{ElementState, KeyEvent};
use winit::platform::scancode::PhysicalKeyExtScancode;

use crate::config::{Action, Direction, KeyCombo, SpatilConfig};
use crate::scene::systems;
use crate::state::SpatilState;

/// A keyboard nudge is one step of the equivalent drag, in that gesture's own units.
const ORBIT_STEP_PIXELS: f32 = 30.0;
const TILT_STEP_DEGREES: f32 = 5.0;

enum Handled {
    Act(Action),
    Menu(Keysym),
}

fn vt_switch_target(sym: Keysym) -> Option<i32> {
    let raw = sym.raw();
    (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12)
        .contains(&raw)
        .then(|| (raw - keysyms::KEY_XF86Switch_VT_1 + 1) as i32)
}

impl SpatilState {
    pub fn handle_keyboard(&mut self, event: KeyEvent) {
        // Clients run their own repeat from the advertised repeat_info; forwarding
        // OS repeats too would double up.
        if event.repeat {
            return;
        }
        // winit gives a linux/evdev scancode; xkb keycodes are that + 8.
        let Some(scancode) = event.physical_key.to_scancode() else {
            return;
        };
        let key_state = match event.state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        };
        self.dispatch_key(Keycode::new(scancode + 8), key_state);
    }

    pub fn dispatch_key(&mut self, keycode: Keycode, key_state: KeyState) {
        self.note_input();
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();

        // A bound press is intercepted (its action returned); releases forward.
        let handled = keyboard.input::<Handled, _>(self, keycode, key_state, serial, time, |data, mods, handle| {
            if key_state != KeyState::Pressed {
                return FilterResult::Forward;
            }
            let sym = handle.modified_sym();
            // An open menu grabs the keyboard outright, so arrow keys drive it
            // instead of reaching whichever client still holds focus underneath.
            if data.menu_is_open() {
                return FilterResult::Intercept(Handled::Menu(sym));
            }
            if let Some(vt) = vt_switch_target(sym) {
                return FilterResult::Intercept(Handled::Act(Action::ChangeVt(vt)));
            }
            let combo = KeyCombo::from_state(mods, sym);
            match data.keybindings.get(&combo) {
                Some(action) => FilterResult::Intercept(Handled::Act(action.clone())),
                None => FilterResult::Forward,
            }
        });

        match handled {
            Some(Handled::Act(action)) => self.run_action(action),
            Some(Handled::Menu(sym)) => self.menu_key(sym),
            None => {}
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Exec(cmd) => {
                if let Err(err) = crate::shell::launch::shell_command(&cmd).spawn() {
                    tracing::warn!("failed to exec {cmd:?}: {err}");
                }
            }
            Action::Hook(spec) => self.run_hook(&spec),
            Action::CloseWindow => self.close_focused_window(),
            Action::Quit => self.should_quit = true,
            Action::ToggleScene => self.toggle_scene(),
            Action::SetScene(mode) => self.set_scene(mode),
            Action::ChangeVt(vt) => self.change_vt(vt),
            Action::Focus(direction) => self.focus_in_direction(direction),
            Action::MoveWindow(direction) => self.move_focused_window(direction),
            Action::Pan(direction) => self.pan_view(direction),
            Action::Present => self.toggle_present(),
            Action::Dismiss => {
                self.dismiss_modal();
            }
            Action::Fullscreen => self.toggle_fullscreen(),
            Action::Zoom(adjust) => self.zoom_centered(adjust.sign()),
            Action::ResetView => self.wgpu_state.camera_controller.reset(),
            Action::Orbit(direction) => {
                let step = self.config.input.orbit_speed.to_degrees() * ORBIT_STEP_PIXELS;
                let (sx, sy) = direction.screen_step();
                self.wgpu_state.camera_controller.orbit(-sx * step, -sy * step);
            }
            Action::Tilt(adjust) => self.tilt_windows(adjust.sign() * TILT_STEP_DEGREES),
            Action::OrbCorner(corner) => self.set_orb_corner(corner),
        }
    }

    fn menu_key(&mut self, sym: Keysym) {
        match sym.raw() {
            keysyms::KEY_Escape => {
                if !self.menu_back() {
                    self.close_menu();
                }
            }
            keysyms::KEY_Up | keysyms::KEY_ISO_Left_Tab => self.menu_step(-1),
            keysyms::KEY_Down | keysyms::KEY_Tab => self.menu_step(1),
            keysyms::KEY_Left => {
                self.menu_back();
            }
            keysyms::KEY_Right => {
                if let Some(index) = self.menu_highlighted() {
                    self.menu_activate(index);
                }
            }
            keysyms::KEY_Return | keysyms::KEY_KP_Enter | keysyms::KEY_space => {
                if let Some(index) = self.menu_highlighted() {
                    self.menu_activate(index);
                }
            }
            keysyms::KEY_Home => {
                self.menu_home(false);
            }
            keysyms::KEY_End => {
                self.menu_home(true);
            }
            _ => {}
        }
    }

    fn focus_in_direction(&mut self, direction: Direction) {
        if self.world_is_modal() {
            return;
        }
        let Some(from) = self.focused_entity() else {
            return;
        };
        let Some(next) =
            systems::focus_in_direction(&self.world, &self.wgpu_state.camera, from, direction)
        else {
            return;
        };
        self.focus_entity(next);
    }

    /// the script must be executable.
    fn run_hook(&self, spec: &str) {
        let Some(hooks_dir) = SpatilConfig::config_dir().map(|d| d.join("hooks")) else {
            tracing::warn!("cannot run hook {spec:?}: no config dir");
            return;
        };
        let spawn = crate::shell::launch::shell_command(spec)
            .current_dir(&hooks_dir)
            .spawn();
        if let Err(err) = spawn {
            tracing::warn!("failed to run hook {spec:?}: {err}");
        }
    }

    fn close_focused_window(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let Some(focused) = keyboard.current_focus() else {
            return;
        };
        if let Some(toplevel) = self.toplevels.iter().find(|t| *t.wl_surface() == focused) {
            toplevel.send_close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vt_keysyms_map_to_terminals() {
        assert_eq!(vt_switch_target(Keysym::new(keysyms::KEY_XF86Switch_VT_1)), Some(1));
        assert_eq!(vt_switch_target(Keysym::new(keysyms::KEY_XF86Switch_VT_12)), Some(12));
        // Plain F1 is not a VT switch: the keymap only yields XF86Switch_VT_n with Ctrl+Alt.
        assert_eq!(vt_switch_target(Keysym::new(keysyms::KEY_F1)), None);
        assert_eq!(vt_switch_target(Keysym::new(keysyms::KEY_a)), None);
    }
}
