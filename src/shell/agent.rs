//! Familiar's agent phases, voice levels and complications; driven over IPC until an agent exists.

use crate::shell::motion::{EASE_IN_OUT, Tween, cubic_bezier};
use crate::shell::orb::Eased;

pub const MARKS: usize = 12;
const MARK_SAMPLE_MS: f32 = 40.0;
pub const THINK_STEP_MS: f32 = 500.0;
pub const DONE_MS: f32 = 1200.0;
const TRANSCRIPT_CHARS: usize = 2048;
/// A level set over IPC drives the marks for this long; after that the stand-in voice does.
const LEVEL_FRESH_MS: f32 = 1000.0;
/// `.arc{transition:stroke-dasharray .7s cubic-bezier(.3,.8,.2,1)}`
const ARC_MS: f32 = 700.0;
const ARC_CURVE: [f32; 4] = [0.3, 0.8, 0.2, 1.0];
const SWEEP_CURVE: [f32; 4] = [0.3, 0.85, 0.2, 1.0];

/// `VoiceProto.dc.html`'s sample conversation and timers, standing in for the agent.
pub mod mock {
    pub const SAID: &str =
        "Group my terminal and browser into a research cluster, push it back, and bring my editor to me";
    pub const STEPS: [&str; 3] = [
        "Group kitty and Firefox into a new constellation, Research",
        "Push Research two layers back",
        "Bring Code (orb.wgsl) in front of you",
    ];
    pub const ANSWER: &str = "Done. Research is behind you, Code is in front.";
    pub const WORD_MS: f32 = 210.0;
    pub const THINK_MS: f32 = 1500.0;
    pub const STEP_MS: f32 = 1100.0;
    pub const WORK_MS: f32 = 3700.0;
    pub const ANSWER_MS: f32 = 4500.0;
}

/// The `vu` keyframes: dash length along a voice mark, in design units, per keyframe offset.
const VU_KEYS: [(f32, f32); 5] = [(0.0, 2.5), (0.3, 8.0), (0.55, 4.0), (0.8, 11.0), (1.0, 2.5)];
/// `.vu0`..`.vu3`: each mark's class is `(i * 7) % 4`.
const VU_SECONDS: [f32; 4] = [0.9, 1.15, 0.8, 1.3];
const VU_DELAY: [f32; 4] = [-0.2, -0.55, -0.1, -0.7];

/// The mockup's stand-in voice for mark `i` at `t` seconds into the phase, as a 0..1 level.
pub fn vu_level(i: usize, t: f32) -> f32 {
    let class = (i * 7) % 4;
    let p = ((t - VU_DELAY[class]) / VU_SECONDS[class]).rem_euclid(1.0);
    let k = VU_KEYS.windows(2).find(|w| p <= w[1].0).unwrap_or(&VU_KEYS[3..5]);
    let local = (p - k[0].0) / (k[1].0 - k[0].0);
    let dash = k[0].1 + (k[1].1 - k[0].1) * cubic_bezier(EASE_IN_OUT, local);
    (dash - 2.5) / 12.3
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentPhase {
    #[default]
    Idle,
    Listening,
    Thinking,
    WaitingForYou,
    Working { step: u32, total: u32 },
    Answering,
    Done,
}

impl AgentPhase {
    pub fn parse(name: &str, args: &[f32]) -> Option<Self> {
        let count = |i: usize| args.get(i).map_or(0, |v| v.max(0.0).round() as u32);
        Some(match name.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "idle" => Self::Idle,
            "listening" => Self::Listening,
            "thinking" => Self::Thinking,
            "waiting" | "waiting_for_you" | "waitingforyou" | "plan" => Self::WaitingForYou,
            "working" => Self::Working { step: count(0), total: count(1) },
            "answering" => Self::Answering,
            "done" => Self::Done,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::WaitingForYou => "waiting",
            Self::Working { .. } => "working",
            Self::Answering => "answering",
            Self::Done => "done",
        }
    }

    /// Holding the orb only starts talking when nothing is mid-flight.
    pub fn can_talk(self) -> bool {
        matches!(self, Self::Idle | Self::Answering | Self::Done)
    }

    fn same_kind(self, other: Self) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEvent {
    Hold,
    Release,
    Cancel,
    DoIt,
    Change,
    Undo,
}

/// The user-driven edges of `VoiceProto`'s state machine; the agent drives the rest over IPC.
pub fn transition(phase: AgentPhase, event: AgentEvent, plan_len: usize) -> Option<AgentPhase> {
    use AgentEvent::*;
    use AgentPhase::*;
    match (phase, event) {
        (p, Hold) if p.can_talk() => Some(Listening),
        (Listening, Release) => Some(Thinking),
        (Listening, Cancel) => Some(Idle),
        (WaitingForYou, DoIt) => Some(Working { step: 0, total: plan_len as u32 }),
        (WaitingForYou, Change) => Some(Listening),
        (Answering, Undo) => Some(Done),
        _ => None,
    }
}

/// `cubic-bezier(.3,.85,.2,1)`, the Done sweep's curve.
pub fn sweep_curve(t: f32) -> f32 {
    cubic_bezier(SWEEP_CURVE, t)
}

/// Everything the orb shader needs from the agent for one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AgentLook {
    pub fold: f32,
    pub park: f32,
    pub voice: f32,
    pub tint: f32,
    pub dim: f32,
    pub step_alpha: f32,
    pub step_angle: f32,
    pub ring_hub: f32,
    pub solid_hub: f32,
    pub arc: f32,
    pub progress: f32,
    pub marks: [f32; MARKS],
    pub alert: f32,
    pub focus: f32,
    pub privacy: f32,
}

#[derive(Default)]
pub struct AgentRuntime {
    phase: AgentPhase,
    phase_ms: f32,
    pub mic_level: f32,
    pub voice_level: f32,
    level: Eased,
    history: [f32; MARKS],
    sample_ms: f32,
    pub transcript: String,
    pub answer: String,
    pub plan: Vec<String>,
    pub privacy: bool,
    pub focus: bool,
    pub alerts: u32,
    fold: Eased,
    park: Eased,
    voice: Eased,
    tint: Eased,
    dim: Eased,
    step: Eased,
    step_angle: f32,
    ring_hub: Eased,
    solid_hub: Eased,
    arc: Eased,
    progress: Tween,
    alert: Eased,
    focus_ring: Eased,
    privacy_light: Eased,
    /// The phase came from IPC, so the stand-in timeline keeps its hands off.
    scripted: bool,
    level_fresh_ms: f32,
}

impl AgentRuntime {
    pub fn phase(&self) -> AgentPhase {
        self.phase
    }

    /// A phase set from outside (IPC, the agent later): the stand-in timeline stops.
    pub fn script_phase(&mut self, phase: AgentPhase) {
        self.scripted = true;
        self.set_phase(phase);
    }

    pub fn set_phase(&mut self, phase: AgentPhase) {
        if !phase.same_kind(self.phase) {
            self.phase_ms = 0.0;
        }
        if matches!(phase, AgentPhase::Working { .. }) && !matches!(self.phase, AgentPhase::Working { .. }) {
            self.progress = Tween::new(0.0, ARC_MS, ARC_CURVE);
        }
        // The Done sweep starts with the hands at twelve, at full length.
        if phase == AgentPhase::Done && self.phase != AgentPhase::Done {
            self.park.snap(1.0);
            self.fold.snap(0.0);
        }
        if phase != self.phase {
            tracing::info!("familiar: {} -> {}", self.phase.as_str(), phase.as_str());
        }
        self.phase = phase;
    }

    pub fn apply(&mut self, event: AgentEvent) -> bool {
        match transition(self.phase, event, self.plan.len()) {
            Some(next) => {
                if event == AgentEvent::Undo {
                    tracing::info!("familiar: undo requested (placeholder; no agent yet)");
                }
                self.scripted = false;
                if next == AgentPhase::Listening {
                    self.transcript.clear();
                }
                self.set_phase(next);
                true
            }
            None => false,
        }
    }

    pub fn set_levels(&mut self, mic: f32, voice: f32) {
        self.mic_level = mic.clamp(0.0, 1.0);
        self.voice_level = voice.clamp(0.0, 1.0);
        self.level_fresh_ms = LEVEL_FRESH_MS;
    }

    pub fn set_transcript(&mut self, text: &str) {
        self.transcript = crate::text::sanitize_to(text, TRANSCRIPT_CHARS);
    }

    pub fn set_answer(&mut self, text: &str) {
        self.answer = crate::text::sanitize_to(text, TRANSCRIPT_CHARS);
    }

    pub fn set_plan(&mut self, lines: &[String]) {
        self.plan = lines
            .iter()
            .map(|l| crate::text::sanitize(l))
            .filter(|l| !l.is_empty())
            .collect();
    }

    fn live_level(&self) -> f32 {
        match self.phase {
            AgentPhase::Listening => self.mic_level,
            AgentPhase::Answering => self.voice_level,
            _ => 0.0,
        }
    }

    fn sample(&mut self, dt_ms: f32) {
        self.sample_ms += dt_ms;
        while self.sample_ms >= MARK_SAMPLE_MS {
            self.sample_ms -= MARK_SAMPLE_MS;
            self.history.rotate_right(1);
            self.history[0] = self.level.value.clamp(0.0, 1.0);
        }
    }

    /// Mark 0 (twelve) is live; mark i is the level i samples ago, so a voice runs clockwise.
    /// With no level source yet, the marks play the mockup's own `vu` keyframes.
    pub fn marks(&self) -> [f32; MARKS] {
        if self.level_fresh_ms <= 0.0 {
            let t = self.phase_ms / 1000.0;
            return std::array::from_fn(|i| vu_level(i, t).clamp(0.0, 1.0));
        }
        let f = (self.sample_ms / MARK_SAMPLE_MS).clamp(0.0, 1.0);
        std::array::from_fn(|i| {
            let v = if i == 0 {
                self.level.value
            } else {
                self.history[i] + (self.history[i - 1] - self.history[i]) * f
            };
            v.clamp(0.0, 1.0)
        })
    }

    pub fn tick(&mut self, dt_ms: f32, stiffness: f32, damping: f32) -> AgentLook {
        use AgentPhase::*;
        let dt = dt_ms / 1000.0;
        self.phase_ms += dt_ms;
        self.level_fresh_ms = (self.level_fresh_ms - dt_ms).max(0.0);
        if self.phase == Done && self.phase_ms >= DONE_MS {
            self.set_phase(Idle);
        }
        if !self.scripted {
            self.stand_in();
        }
        let phase = self.phase;
        let on = |b: bool| b as u8 as f32;
        let spring = |e: &mut Eased, target: f32| e.tick(target, stiffness, damping, dt);

        let live = self.live_level();
        spring(&mut self.level, live);
        spring(&mut self.fold, on(matches!(phase, Listening | Thinking | Answering)));
        spring(&mut self.voice, on(matches!(phase, Listening | Answering)));
        spring(&mut self.tint, on(phase == Answering));
        spring(&mut self.dim, on(phase == Thinking));
        spring(&mut self.step, on(phase == Thinking));
        spring(&mut self.ring_hub, on(matches!(phase, Listening | Thinking | WaitingForYou)));
        spring(&mut self.solid_hub, on(phase == Answering));
        spring(&mut self.arc, on(matches!(phase, Working { .. })));
        spring(&mut self.alert, on(self.alerts > 0));
        spring(&mut self.focus_ring, on(self.focus));
        spring(&mut self.privacy_light, on(self.privacy));
        if let Working { step, total } = phase {
            let fraction = if total == 0 { 0.0 } else { (step.min(total) as f32) / total as f32 };
            self.progress.set(fraction);
        }
        self.progress.tick(dt_ms);
        if phase == Done {
            self.park.snap(1.0 - sweep_curve(self.phase_ms / DONE_MS));
        } else {
            spring(&mut self.park, on(phase == WaitingForYou));
        }
        if phase == Thinking {
            let tick = (self.phase_ms / THINK_STEP_MS).floor() as u32 % MARKS as u32;
            self.step_angle = std::f32::consts::TAU * tick as f32 / MARKS as f32;
        }
        self.sample(dt_ms);

        let unit = |e: &Eased| e.value.clamp(0.0, 1.0);
        AgentLook {
            fold: unit(&self.fold),
            park: unit(&self.park),
            voice: unit(&self.voice),
            tint: unit(&self.tint),
            dim: unit(&self.dim),
            step_alpha: unit(&self.step),
            step_angle: self.step_angle,
            ring_hub: unit(&self.ring_hub),
            solid_hub: unit(&self.solid_hub),
            arc: unit(&self.arc),
            progress: self.progress.value().clamp(0.0, 1.0),
            marks: self.marks(),
            alert: unit(&self.alert),
            focus: unit(&self.focus_ring),
            privacy: unit(&self.privacy_light),
        }
    }

    /// `VoiceProto`'s timers: words arrive while listening, a plan after thinking, steps tick
    /// by while working, then the answer, then the sweep home.
    fn stand_in(&mut self) {
        use AgentPhase::*;
        let words: Vec<&str> = mock::SAID.split(' ').collect();
        match self.phase {
            Listening => {
                let n = ((self.phase_ms / mock::WORD_MS) as usize).min(words.len());
                self.transcript = words[..n].join(" ");
            }
            Thinking => {
                self.transcript = mock::SAID.to_string();
                if self.phase_ms >= mock::THINK_MS {
                    if self.plan.is_empty() {
                        self.plan = mock::STEPS.iter().map(|s| s.to_string()).collect();
                    }
                    self.set_phase(WaitingForYou);
                }
            }
            Working { total, .. } => {
                let step = ((self.phase_ms / mock::STEP_MS) as u32).min(total);
                self.phase = Working { step, total };
                if self.phase_ms >= mock::WORK_MS {
                    if self.answer.is_empty() {
                        self.answer = mock::ANSWER.to_string();
                    }
                    self.set_phase(Answering);
                }
            }
            Answering if self.phase_ms >= mock::ANSWER_MS => self.set_phase(Done),
            _ => {}
        }
    }

    pub fn report(&self) -> serde_json::Value {
        let (step, total) = match self.phase {
            AgentPhase::Working { step, total } => (Some(step), Some(total)),
            _ => (None, None),
        };
        serde_json::json!({
            "phase": self.phase.as_str(),
            "stand_in": !self.scripted,
            "phase_ms": self.phase_ms,
            "step": step,
            "total": total,
            "mic_level": self.mic_level,
            "voice_level": self.voice_level,
            "marks": self.marks(),
            "transcript": self.transcript,
            "answer": self.answer,
            "plan": self.plan,
            "privacy": self.privacy,
            "focus": self.focus,
            "alerts": self.alerts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentEvent::*;
    use AgentPhase::*;

    #[test]
    fn the_prototype_flow_runs_end_to_end() {
        let mut p = Idle;
        for (event, expect) in [(Hold, Listening), (Release, Thinking)] {
            p = transition(p, event, 3).expect("edge exists");
            assert_eq!(p, expect);
        }
        p = WaitingForYou;
        p = transition(p, DoIt, 3).expect("do it");
        assert_eq!(p, Working { step: 0, total: 3 });
        assert_eq!(transition(Answering, Undo, 3), Some(Done));
    }

    #[test]
    fn holding_only_talks_when_nothing_is_in_flight() {
        for p in [Idle, Answering, Done] {
            assert_eq!(transition(p, Hold, 0), Some(Listening), "{p:?}");
        }
        for p in [Thinking, WaitingForYou, Working { step: 1, total: 3 }, Listening] {
            assert_eq!(transition(p, Hold, 0), None, "{p:?}");
        }
    }

    #[test]
    fn escape_cancels_only_listening() {
        assert_eq!(transition(Listening, Cancel, 0), Some(Idle));
        assert_eq!(transition(Thinking, Cancel, 0), None);
        assert_eq!(transition(WaitingForYou, Change, 2), Some(Listening));
        assert_eq!(transition(Thinking, DoIt, 2), None, "nothing executes before the plan");
    }

    #[test]
    fn done_sweeps_then_returns_to_idle() {
        let mut agent = AgentRuntime::default();
        agent.set_phase(Done);
        let first = agent.tick(16.0, 220.0, 22.0);
        assert!(first.park > 0.9, "the sweep starts at twelve: {}", first.park);
        assert_eq!(first.fold, 0.0, "the hands are full length for the sweep");
        let mut last = first.park;
        for _ in 0..80 {
            let look = agent.tick(16.0, 220.0, 22.0);
            assert!(look.park <= last + 1e-6, "the sweep never turns back");
            last = look.park;
        }
        assert_eq!(agent.phase(), Idle, "Done hands over to Idle after 1.2s");
        assert!(last < 0.01, "the hands reached the time: {last}");
    }

    #[test]
    fn the_sweep_curve_is_an_ease_out() {
        assert!(sweep_curve(0.0).abs() < 1e-3);
        assert!((sweep_curve(1.0) - 1.0).abs() < 1e-3);
        assert!(sweep_curve(0.25) > 0.5, "fast start: {}", sweep_curve(0.25));
        let mut prev = 0.0;
        for i in 1..=20 {
            let y = sweep_curve(i as f32 / 20.0);
            assert!(y >= prev - 1e-4);
            prev = y;
        }
    }

    #[test]
    fn thinking_ticks_twelve_discrete_steps_per_six_seconds() {
        let mut agent = AgentRuntime::default();
        agent.script_phase(Thinking);
        let mut angles = Vec::new();
        for _ in 0..360 {
            let look = agent.tick(1000.0 / 60.0, 220.0, 22.0);
            if angles.last() != Some(&look.step_angle) {
                angles.push(look.step_angle);
            }
        }
        assert_eq!(angles.len(), 12, "one jump every 500ms: {angles:?}");
        assert_eq!(angles[0], 0.0, "the bright mark starts at twelve");
    }

    #[test]
    fn marks_carry_the_voice_clockwise_from_twelve() {
        let mut agent = AgentRuntime::default();
        agent.script_phase(Listening);
        agent.set_levels(1.0, 0.0);
        for _ in 0..30 {
            agent.tick(1000.0 / 60.0, 220.0, 22.0);
        }
        agent.set_levels(0.0, 0.0);
        for _ in 0..10 {
            agent.tick(1000.0 / 60.0, 220.0, 22.0);
        }
        let marks = agent.marks();
        assert!(marks[0] < marks[6], "twelve has already gone quiet: {marks:?}");
        assert!(marks.iter().all(|m| (0.0..=1.0).contains(m)));
    }

    #[test]
    fn working_progress_eases_to_each_step() {
        let mut agent = AgentRuntime::default();
        agent.script_phase(Working { step: 1, total: 4 });
        let mut look = AgentLook::default();
        for _ in 0..120 {
            look = agent.tick(1000.0 / 60.0, 220.0, 22.0);
        }
        assert!((look.progress - 0.25).abs() < 0.01, "{}", look.progress);
        assert!(look.arc > 0.99);
    }

    #[test]
    fn holding_the_orb_plays_the_prototype_through_without_an_agent() {
        let mut agent = AgentRuntime::default();
        let frame = 1000.0 / 60.0;
        let run = |agent: &mut AgentRuntime, ms: f32| {
            for _ in 0..(ms / frame) as usize {
                agent.tick(frame, 220.0, 22.0);
            }
        };
        agent.apply(Hold);
        run(&mut agent, 1000.0);
        assert!(agent.transcript.split(' ').count() >= 4, "words arrive: {:?}", agent.transcript);
        agent.apply(Release);
        assert_eq!(agent.phase(), Thinking);
        run(&mut agent, 1600.0);
        assert_eq!(agent.phase(), WaitingForYou, "a plan after 1.5s");
        assert_eq!(agent.plan.len(), 3);
        agent.apply(DoIt);
        run(&mut agent, 1200.0);
        assert_eq!(agent.phase(), Working { step: 1, total: 3 }, "a step every 1.1s");
        run(&mut agent, 2600.0);
        assert_eq!(agent.phase(), Answering, "the answer at 3.7s");
        run(&mut agent, 4600.0);
        assert_eq!(agent.phase(), Done, "the sweep home after 4.5s");
    }

    #[test]
    fn a_scripted_phase_stays_put() {
        let mut agent = AgentRuntime::default();
        agent.script_phase(Thinking);
        for _ in 0..200 {
            agent.tick(16.0, 220.0, 22.0);
        }
        assert_eq!(agent.phase(), Thinking);
    }

    #[test]
    fn without_a_level_the_marks_play_the_vu_keyframes() {
        assert!(vu_level(0, -0.2).abs() < 1e-3, "vu0 starts its cycle at 2.5");
        let peak = (0.8 * 0.9) - 0.2;
        assert!((vu_level(0, peak) - 1.0 / 12.3 * 8.5).abs() < 1e-3, "11 at 80%: {}", vu_level(0, peak));
        let mut agent = AgentRuntime::default();
        agent.script_phase(Listening);
        agent.tick(16.0, 220.0, 22.0);
        let marks = agent.marks();
        assert!(marks.iter().any(|m| (m - marks[0]).abs() > 0.05), "marks differ: {marks:?}");
    }

    #[test]
    fn phases_parse_from_ipc_names() {
        assert_eq!(AgentPhase::parse("Working", &[2.0, 3.0]), Some(Working { step: 2, total: 3 }));
        assert_eq!(AgentPhase::parse("waiting_for_you", &[]), Some(WaitingForYou));
        assert_eq!(AgentPhase::parse("sleeping", &[]), None);
        for p in [Idle, Listening, Thinking, WaitingForYou, Answering, Done] {
            assert_eq!(AgentPhase::parse(p.as_str(), &[]), Some(p));
        }
    }
}
