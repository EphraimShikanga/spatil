use smithay::backend::input::KeyState;
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::utils::SERIAL_COUNTER;
use winit::event::{ElementState, KeyEvent};
use winit::platform::scancode::PhysicalKeyExtScancode;

use crate::config::{Action, KeyCombo, SpatilConfig};
use crate::state::SpatilState;

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

    /// Matches keybindings inside the seat's `input` filter (using the seat's own xkb
    /// modifier state); non-bound keys forward to the focused client.
    pub fn dispatch_key(&mut self, keycode: Keycode, key_state: KeyState) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();

        // A bound press is intercepted (its action returned); releases forward.
        let action = keyboard.input::<Action, _>(self, keycode, key_state, serial, time, |data, mods, handle| {
            if key_state != KeyState::Pressed {
                return FilterResult::Forward;
            }
            let combo = KeyCombo::from_state(mods, handle.modified_sym());
            match data.keybindings.get(&combo) {
                Some(action) => FilterResult::Intercept(action.clone()),
                None => FilterResult::Forward,
            }
        });

        if let Some(action) = action {
            self.run_action(action);
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Exec(cmd) => {
                if let Err(err) = std::process::Command::new("sh").arg("-c").arg(&cmd).spawn() {
                    tracing::warn!("failed to exec {cmd:?}: {err}");
                }
            }
            Action::Hook(spec) => self.run_hook(&spec),
            Action::CloseWindow => self.close_focused_window(),
            Action::Quit => self.should_quit = true,
            Action::ToggleScene => self.toggle_scene(),
        }
    }

    /// the script must be executable.
    fn run_hook(&self, spec: &str) {
        let Some(hooks_dir) = SpatilConfig::config_dir().map(|d| d.join("hooks")) else {
            tracing::warn!("cannot run hook {spec:?}: no config dir");
            return;
        };
        let spawn = std::process::Command::new("sh")
            .arg("-c")
            .arg(spec)
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
