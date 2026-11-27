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

fn pump(state: &mut SpatilState, event_loop: &mut EventLoop<'static, SpatilState>, wall: Duration) {
    let until = Instant::now() + wall;
    while Instant::now() < until {
        let _ = event_loop.dispatch(Some(Duration::from_millis(1)), state);
        state.tick_animations(FRAME_MS);
        std::thread::sleep(Duration::from_millis(FRAME_MS as u64));
    }
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
}
