//! Drives the real shell state headlessly through the orb's states and snapshots each one.

use std::path::Path;
use std::time::{Duration, Instant};

use smithay::backend::input::ButtonState;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

use crate::config::SpatilConfig;
use crate::input::drag::BTN_LEFT;
use crate::renderer::orb::Corner;
use crate::renderer::snapshot::{self, HEIGHT, Snapshotter, WIDTH};
use crate::state::SpatilState;

const FRAME_MS: f32 = 1000.0 / 60.0;

/// The time every mockup shows: hour hand at 76 degrees, minute hand at 192.
const MOCKUP_TIME: f32 = 14.0 * 3600.0 + 32.0 * 60.0;

fn pump(state: &mut SpatilState, event_loop: &mut EventLoop<'static, SpatilState>, wall: Duration) {
    pump_with(state, event_loop, wall, |_, _| {});
}

fn pump_with(
    state: &mut SpatilState,
    event_loop: &mut EventLoop<'static, SpatilState>,
    wall: Duration,
    mut each: impl FnMut(&mut SpatilState, f32),
) {
    let start = Instant::now();
    let until = start + wall;
    while Instant::now() < until {
        let _ = event_loop.dispatch(Some(Duration::from_millis(1)), state);
        each(state, start.elapsed().as_secs_f32());
        state.tick_animations(FRAME_MS);
        std::thread::sleep(Duration::from_millis(FRAME_MS as u64));
    }
}

/// A stand-in voice: syllable-rate swells, never silent, never pinned at full.
fn speech(t: f32) -> f32 {
    (0.45 + 0.3 * (t * 9.0).sin() + 0.2 * (t * 23.0 + 1.0).sin()).clamp(0.0, 1.0)
}

/// Pumps as if the user were present, so ambient never creeps into a state capture.
fn awake(
    state: &mut SpatilState,
    event_loop: &mut EventLoop<'static, SpatilState>,
    wall: Duration,
    mut each: impl FnMut(&mut SpatilState, f32),
) {
    pump_with(state, event_loop, wall, |s, t| {
        s.last_input = Instant::now();
        each(s, t);
    });
}

fn agent_states(
    state: &mut SpatilState,
    event_loop: &mut EventLoop<'static, SpatilState>,
    shot: &dyn Fn(&mut SpatilState, &str),
) {
    use crate::shell::agent::AgentPhase;
    state.dispatch_pointer_motion_absolute(450.0, 120.0);
    let calm = Duration::from_millis(900);

    // VoiceProto's sample conversation.
    let said = "Group my terminal and browser into a research cluster, push it back, and bring my editor to me";
    state.agent.set_plan(&[
        "Group kitty and Firefox into a new constellation, Research".to_string(),
        "Push Research two layers back".to_string(),
        "Bring Code (orb.wgsl) in front of you".to_string(),
    ]);
    state.agent.set_answer("Done. Research is behind you, Code is in front.");
    state.agent.set_transcript(&said[..62]);

    state.agent.set_phase(AgentPhase::Listening);
    awake(state, event_loop, Duration::from_millis(90), |s, t| s.agent.set_levels(speech(t), 0.0));
    shot(state, "10-listening-opening");
    awake(state, event_loop, calm, |s, t| s.agent.set_levels(speech(t), 0.0));
    shot(state, "10-listening");

    state.agent.set_levels(0.0, 0.0);
    state.agent.set_transcript(said);
    state.agent.set_phase(AgentPhase::Thinking);
    awake(state, event_loop, Duration::from_millis(1250), |_, _| {});
    shot(state, "11-thinking");

    state.agent.set_phase(AgentPhase::WaitingForYou);
    awake(state, event_loop, calm, |_, _| {});
    shot(state, "12-waiting");
    state.preview_orb_corner(Corner::TopLeft);
    awake(state, event_loop, calm, |_, _| {});
    shot(state, "12-waiting-top-left");
    state.preview_orb_corner(Corner::BottomRight);
    awake(state, event_loop, calm, |_, _| {});

    state.agent.set_phase(AgentPhase::Working { step: 1, total: 3 });
    awake(state, event_loop, calm, |_, _| {});
    shot(state, "13-working");

    state.agent.set_phase(AgentPhase::Answering);
    awake(state, event_loop, calm, |s, t| s.agent.set_levels(0.0, speech(t)));
    shot(state, "14-answering");

    state.agent.set_levels(0.0, 0.0);
    state.agent.set_phase(AgentPhase::Done);
    awake(state, event_loop, Duration::from_millis(250), |_, _| {});
    shot(state, "15-done-sweeping");
    awake(state, event_loop, Duration::from_millis(1400), |_, _| {});
    shot(state, "16-done-idle");

    state.agent.alerts = 2;
    awake(state, event_loop, calm, |_, _| {});
    shot(state, "17-alert");

    state.agent.focus = true;
    awake(state, event_loop, calm, |_, _| {});
    shot(state, "18-focus");

    state.agent.alerts = 0;
    state.agent.focus = false;
    state.agent.privacy = true;
    awake(state, event_loop, calm, |_, _| {});
    shot(state, "19-privacy");
    state.agent.privacy = false;
}

pub fn run(out_dir: &Path) {
    let mut config = SpatilConfig::load();
    config.display.width = WIDTH;
    config.display.height = HEIGHT;

    crate::text::warm_fonts(|| {});
    while !crate::text::fonts_ready() {
        std::thread::sleep(Duration::from_millis(10));
    }

    let gpu = match snapshot::headless(&config) {
        Ok(gpu) => gpu,
        Err(err) => {
            eprintln!("hud-snapshot: {err}");
            return;
        }
    };
    let shooter = match Snapshotter::new(&gpu, out_dir) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("hud-snapshot: {err}");
            return;
        }
    };

    let mut event_loop = EventLoop::<'static, SpatilState>::try_new().expect("event loop");
    let display = Display::<SpatilState>::new().expect("display");
    let mut state = SpatilState::new(
        display.handle(),
        event_loop.handle(),
        event_loop.get_signal(),
        gpu,
        config,
    );
    state.preview_orb_corner(Corner::BottomRight);
    state.freeze_orb_clock(MOCKUP_TIME);
    state.rearrange_layers();

    let shot = |state: &mut SpatilState, name: &str| {
        match shooter.capture(&mut state.wgpu_state, &mut state.world, name) {
            Ok(path) => println!("{}", path.display()),
            Err(err) => eprintln!("hud-snapshot {name}: {err}"),
        }
    };

    state.dispatch_pointer_motion_absolute(200.0, 200.0);
    pump(&mut state, &mut event_loop, Duration::from_millis(900));
    shot(&mut state, "1-rest");

    let Some(rect) = state.orb_rect() else {
        eprintln!("hud-snapshot: the orb has no rect at {WIDTH}x{HEIGHT}");
        return;
    };
    let centre = (
        rect.loc.x as f64 + rect.size.w as f64 * 0.5,
        rect.loc.y as f64 + rect.size.h as f64 * 0.5,
    );
    state.dispatch_pointer_motion_absolute(centre.0, centre.1);
    pump(&mut state, &mut event_loop, Duration::from_millis(400));
    shot(&mut state, "2-hover");

    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);
    for step in 1..=3 {
        pump(&mut state, &mut event_loop, Duration::from_millis(60));
        shot(&mut state, &format!("3-opening-{step}"));
    }
    pump(&mut state, &mut event_loop, Duration::from_millis(700));
    shot(&mut state, "3-open");

    if let Some((_, r)) = state.module_slots.first().cloned() {
        state.dispatch_pointer_motion_absolute((r[0] + r[2] * 0.5) as f64, (r[1] + r[3] * 0.5) as f64);
        pump(&mut state, &mut event_loop, Duration::from_millis(300));
        shot(&mut state, "4-tile-hover");
    }
    if let Some(r) = state.card_destination_rect(crate::shell::card::Destination::Calendar) {
        state.dispatch_pointer_motion_absolute((r[0] + r[2] * 0.5) as f64, (r[1] + r[3] * 0.5) as f64);
        state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
        state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);
        pump(&mut state, &mut event_loop, Duration::from_millis(400));
        shot(&mut state, "5-placeholder");
    }

    state.dispatch_pointer_motion_absolute(centre.0, centre.1);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);
    pump(&mut state, &mut event_loop, Duration::from_millis(600));
    println!("orb click while open closes the card: {}", !state.orb_expanded());
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);

    state.preview_orb_corner(Corner::TopLeft);
    pump(&mut state, &mut event_loop, Duration::from_millis(900));
    shot(&mut state, "6-top-left");
    state.preview_orb_corner(Corner::BottomRight);

    // Off the card first: a click on a tile would launch its control application.
    state.dispatch_pointer_motion_absolute(450.0, 120.0);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);
    state.last_input = Instant::now() - Duration::from_secs(3600);
    pump(&mut state, &mut event_loop, Duration::from_millis(1500));
    shot(&mut state, "7-idle");

    state.last_input = Instant::now();
    agent_states(&mut state, &mut event_loop, &shot);
    gestures(&mut state, &mut event_loop, centre);
}

fn gestures(
    state: &mut SpatilState,
    event_loop: &mut EventLoop<'static, SpatilState>,
    centre: (f64, f64),
) {
    use crate::shell::agent::AgentPhase;
    state.agent.set_phase(AgentPhase::Idle);
    state.dispatch_pointer_motion_absolute(centre.0, centre.1);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
    awake(state, event_loop, Duration::from_millis(450), |_, _| {});
    println!("holding the orb listens: {}", state.agent.phase() == AgentPhase::Listening);
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);
    println!("letting go sends: {}", state.agent.phase() == AgentPhase::Thinking);

    state.agent.set_phase(AgentPhase::Idle);
    let was_open = state.orb_expanded();
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
    awake(state, event_loop, Duration::from_millis(120), |_, _| {});
    state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);
    println!("a quick click toggles the card: {}", state.orb_expanded() != was_open);
    state.close_orb();
    awake(state, event_loop, Duration::from_millis(500), |_, _| {});

    use crate::shell::banner::BannerButton;
    let click = |state: &mut SpatilState, r: [f32; 4]| {
        state.dispatch_pointer_motion_absolute((r[0] + r[2] * 0.5) as f64, (r[1] + r[3] * 0.5) as f64);
        state.dispatch_pointer_button(BTN_LEFT, ButtonState::Pressed);
        state.dispatch_pointer_button(BTN_LEFT, ButtonState::Released);
    };
    state.agent.set_phase(AgentPhase::WaitingForYou);
    awake(state, event_loop, Duration::from_millis(900), |_, _| {});
    if let Some(r) = state.banner_button(BannerButton::DoIt) {
        click(state, r);
    }
    println!(
        "Do it starts working: {}",
        matches!(state.agent.phase(), AgentPhase::Working { step: 0, total: 3 })
    );
    state.agent.set_phase(AgentPhase::Answering);
    awake(state, event_loop, Duration::from_millis(900), |_, _| {});
    if let Some(r) = state.banner_button(BannerButton::Undo) {
        click(state, r);
    }
    println!("Undo ends the answer: {}", state.agent.phase() == AgentPhase::Done);
}
