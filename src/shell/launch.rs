use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::config::Shell;

use super::module::ModuleId;

pub fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            Ok(())
        });
    }
    cmd
}

fn candidates(id: &ModuleId) -> &'static [&'static str] {
    match id {
        ModuleId::Network => &[
            "nm-connection-editor",
            "gnome-control-center wifi",
            "plasma-open-settings",
        ],
        ModuleId::Bluetooth => &[
            "blueman-manager",
            "overskride",
            "blueberry",
            "gnome-control-center bluetooth",
        ],
        ModuleId::Audio => &["pwvucontrol", "pavucontrol", "gnome-control-center sound"],
        ModuleId::Battery => &["gnome-power-statistics"],
        ModuleId::Tray(_) => &[],
    }
}

fn configured<'a>(id: &ModuleId, shell: &'a Shell) -> Option<&'a str> {
    let value = match id {
        ModuleId::Network => &shell.commands.network,
        ModuleId::Bluetooth => &shell.commands.bluetooth,
        ModuleId::Audio => &shell.commands.audio,
        ModuleId::Battery => &shell.commands.battery,
        ModuleId::Tray(_) => return None,
    };
    (!value.trim().is_empty()).then_some(value.as_str())
}

pub fn on_path(binary: &str) -> bool {
    if binary.contains('/') {
        return Path::new(binary).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(binary).is_file())
}

pub fn resolve(id: &ModuleId, shell: &Shell) -> Option<String> {
    if let Some(configured) = configured(id, shell) {
        return Some(configured.to_string());
    }
    candidates(id)
        .iter()
        .find(|command| command.split_whitespace().next().is_some_and(on_path))
        .map(|command| command.to_string())
}

pub fn spawn(command: &str) -> bool {
    match shell_command(command).spawn() {
        Ok(_) => {
            tracing::info!("familiar launched {command:?}");
            true
        }
        Err(err) => {
            tracing::warn!("could not launch {command:?}: {err}");
            false
        }
    }
}

pub fn describe_missing(id: &ModuleId) -> String {
    let options = candidates(id);
    if options.is_empty() {
        return format!("no control application is defined for the {id:?} module");
    }
    format!(
        "no control application found for the {id:?} module; install one of {} \
         or set [shell.commands] in spatil.toml",
        options
            .iter()
            .map(|c| c.split_whitespace().next().unwrap_or(c))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Shell {
        Shell::default()
    }

    #[test]
    fn a_configured_command_beats_every_default() {
        let mut config = shell();
        config.commands.network = "my-own-tool --flag".to_string();
        assert_eq!(
            resolve(&ModuleId::Network, &config).as_deref(),
            Some("my-own-tool --flag"),
            "a configured command is used even when it is not on PATH",
        );
    }

    #[test]
    fn blank_configuration_falls_back_to_the_defaults() {
        let mut config = shell();
        config.commands.network = "   ".to_string();
        let resolved = resolve(&ModuleId::Network, &config);
        assert!(
            resolved.is_none() || resolved.as_deref() == Some("nm-connection-editor"),
            "got {resolved:?}",
        );
    }

    #[test]
    fn a_tray_item_never_resolves_to_a_control_application() {
        assert!(resolve(&ModuleId::Tray("x".to_string()), &shell()).is_none());
    }

    #[test]
    fn path_lookup_finds_a_real_binary_and_rejects_nonsense() {
        assert!(on_path("sh"), "sh must be on PATH in any sane environment");
        assert!(!on_path("definitely-not-a-real-binary-xyz"));
        assert!(!on_path("/nonexistent/path/to/tool"));
    }

    #[test]
    fn the_missing_message_names_something_installable() {
        let message = describe_missing(&ModuleId::Bluetooth);
        assert!(message.contains("blueman-manager"), "{message}");
        assert!(message.contains("shell.commands"), "{message}");
    }
}
