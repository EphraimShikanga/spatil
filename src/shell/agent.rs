//! Familiar's agent phases, voice levels and complications; driven over IPC until an agent exists.

use crate::shell::orb::Eased;

pub const MARKS: usize = 12;
const MARK_SAMPLE_MS: f32 = 40.0;
pub const THINK_STEP_MS: f32 = 500.0;
pub const DONE_MS: f32 = 1200.0;
const TRANSCRIPT_CHARS: usize = 2048;

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

/// `cubic-bezier(.3,.85,.2,1)`, the Done sweep's curve, solved for y at time `t`.
pub fn sweep_curve(t: f32) -> f32 {
    let (x1, y1, x2, y2) = (0.3_f32, 0.85_f32, 0.2_f32, 1.0_f32);
    let t = t.clamp(0.0, 1.0);
    let bez = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if bez(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) * 0.5)
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
    progress: Eased,
    alert: Eased,
    focus_ring: Eased,
    privacy_light: Eased,
}

impl AgentRuntime {
    pub fn phase(&self) -> AgentPhase {
        self.phase
    }

    pub fn set_phase(&mut self, phase: AgentPhase) {
        if !phase.same_kind(self.phase) {
            self.phase_ms = 0.0;
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
                self.set_phase(next);
                true
            }
            None => false,
        }
    }

    pub fn set_levels(&mut self, mic: f32, voice: f32) {
        self.mic_level = mic.clamp(0.0, 1.0);
        self.voice_level = voice.clamp(0.0, 1.0);
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
    pub fn marks(&self) -> [f32; MARKS] {
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
        if self.phase == Done && self.phase_ms >= DONE_MS {
            self.set_phase(Idle);
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
            spring(&mut self.progress, fraction);
        }
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
            progress: unit(&self.progress),
            marks: self.marks(),
            alert: unit(&self.alert),
            focus: unit(&self.focus_ring),
            privacy: unit(&self.privacy_light),
        }
    }

    pub fn report(&self) -> serde_json::Value {
        let (step, total) = match self.phase {
            AgentPhase::Working { step, total } => (Some(step), Some(total)),
            _ => (None, None),
        };
        serde_json::json!({
            "phase": self.phase.as_str(),
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
        agent.set_phase(Thinking);
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
        agent.set_phase(Listening);
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
        agent.set_phase(Working { step: 1, total: 4 });
        let mut look = AgentLook::default();
        for _ in 0..120 {
            look = agent.tick(1000.0 / 60.0, 220.0, 22.0);
        }
        assert!((look.progress - 0.25).abs() < 0.01, "{}", look.progress);
        assert!(look.arc > 0.99);
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
