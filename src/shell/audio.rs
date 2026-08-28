use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::Arc;

use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, Sender, channel};

use super::icon_source;
use super::launch::on_path;
use super::module::{ModuleId, ModuleState};
use crate::state::SpatilState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioStatus {
    pub volume: f32,
    pub muted: bool,
}

impl AudioStatus {
    pub fn icon_name(&self) -> &'static str {
        if self.muted || self.volume <= 0.001 {
            return "audio-volume-muted-symbolic";
        }
        match (self.volume * 100.0).round() as i64 {
            67..=i64::MAX => "audio-volume-high-symbolic",
            34..=66 => "audio-volume-medium-symbolic",
            _ => "audio-volume-low-symbolic",
        }
    }

    pub fn level(&self) -> Option<f32> {
        Some(self.volume.clamp(0.0, 1.0))
    }

    pub fn summary(&self) -> String {
        if self.muted {
            return "Volume muted".to_string();
        }
        format!("Volume {}%", (self.volume * 100.0).round() as i64)
    }
}

pub fn parse_wpctl(line: &str) -> Option<AudioStatus> {
    let rest = line.split_once("Volume:")?.1;
    let muted = rest.contains("MUTED");
    let value = rest.split_whitespace().next()?.parse::<f32>().ok()?;
    Some(AudioStatus { volume: value, muted })
}

fn read_status() -> Option<AudioStatus> {
    let mut child = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    let _ = child.try_wait();
    parse_wpctl(&text)
}

pub fn init_audio(loop_handle: &LoopHandle<'static, SpatilState>) {
    if !on_path("wpctl") {
        tracing::info!("wpctl not found; the audio module is unavailable");
        return;
    }
    let (tx, rx) = channel::<ModuleState>();
    let inserted = loop_handle.insert_source(rx, |event, _, state: &mut SpatilState| {
        if let Event::Msg(module) = event {
            state.audio = Some(module);
        }
    });
    if let Err(err) = inserted {
        tracing::warn!("could not insert the audio event source: {err}");
        return;
    }
    if let Err(err) = std::thread::Builder::new()
        .name("spatil-audio".into())
        .spawn(move || run(tx))
    {
        tracing::warn!("could not start the audio thread: {err}");
    }
}

fn run(tx: Sender<ModuleState>) {
    let mut last: Option<AudioStatus> = None;
    publish(&tx, &mut last);

    if !on_path("pactl") {
        tracing::info!("pactl not found; volume will not update until Spatil restarts");
        return;
    }
    let child = Command::new("pactl")
        .arg("subscribe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        tracing::warn!("could not watch pactl; volume will not update");
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if line.contains("sink") || line.contains("server") {
            publish(&tx, &mut last);
        }
    }
    let _ = child.wait();
}

fn publish(tx: &Sender<ModuleState>, last: &mut Option<AudioStatus>) {
    let Some(status) = read_status() else {
        return;
    };
    if *last == Some(status) {
        return;
    }
    tracing::debug!("audio: {}", status.summary());

    let mut module = ModuleState::new(ModuleId::Audio);
    module.icon = icon_source(status.icon_name()).map(Arc::new);
    module.level = status.level();
    module.active = !status.muted;
    module.label = status.summary();
    module.tooltip = status.summary();
    let _ = tx.send(module);
    *last = Some(status);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpctl_output_parses_with_and_without_the_mute_marker() {
        let plain = parse_wpctl("Volume: 0.68\n").expect("plain");
        assert!((plain.volume - 0.68).abs() < 1e-6);
        assert!(!plain.muted);

        let muted = parse_wpctl("Volume: 0.35 [MUTED]\n").expect("muted");
        assert!((muted.volume - 0.35).abs() < 1e-6);
        assert!(muted.muted);
    }

    #[test]
    fn unexpected_wpctl_output_is_refused_rather_than_guessed_at() {
        assert!(parse_wpctl("").is_none());
        assert!(parse_wpctl("Node 42 not found").is_none());
        assert!(parse_wpctl("Volume: banana").is_none());
    }

    #[test]
    fn the_icon_follows_the_volume_and_mute_wins_outright() {
        let at = |volume, muted| AudioStatus { volume, muted }.icon_name();
        assert_eq!(at(0.9, false), "audio-volume-high-symbolic");
        assert_eq!(at(0.5, false), "audio-volume-medium-symbolic");
        assert_eq!(at(0.1, false), "audio-volume-low-symbolic");
        assert_eq!(at(0.0, false), "audio-volume-muted-symbolic");
        assert_eq!(at(0.9, true), "audio-volume-muted-symbolic", "muted at any volume");
    }

    #[test]
    fn the_summary_says_muted_rather_than_a_percentage() {
        assert_eq!(AudioStatus { volume: 0.68, muted: false }.summary(), "Volume 68%");
        assert_eq!(AudioStatus { volume: 0.68, muted: true }.summary(), "Volume muted");
    }

    #[test]
    fn every_state_names_an_icon_that_exists_in_a_real_theme() {
        for volume in [0.0, 0.2, 0.5, 0.9] {
            let name = AudioStatus { volume, muted: false }.icon_name();
            if super::super::icon_source(name).is_none() {
                eprintln!("no icon theme provides {name}; skipping");
                return;
            }
        }
    }
}
