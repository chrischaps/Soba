//! Arranger module.
//!
//! A song's timeline: a run of named sections, each some bars long, and
//! eight automation lanes that the sections move. It's clocked like the
//! sequencers, so it counts the same bars they play, swing and all.
//!
//! Each section tells each lane what to do when it starts (a [`Cue`]):
//!
//! - **Hold**: carry on as it was.
//! - **Jump**: go straight to a level, on the downbeat.
//! - **Ramp**: glide to a level over the whole section, or over a number of
//!   bars, on a [`Curve`]: an S-curve, a line, or an exponential or
//!   logarithmic bend. It starts from where the lane is, or from a level of
//!   its own, jumping there on the downbeat. A ramp is smooth enough to drive
//!   a Mixer's Level directly.
//! - **Hit**: go to a level for half a step, then back: a trigger, with the
//!   level as its accent, for a drum or a Looper's button.
//!
//! Each lane has a CV output and a Gate that's high while the lane is above
//! zero, so a lane can open an envelope for a section as well as set a level.
//!
//! # How a cue is stored
//!
//! Every cue is one parameter, so a song saves, loads, undoes and copies
//! like any knob. Its value reads as digits: `M BB LLLL`, the move (0 Hold,
//! 1 Jump, 2 Ramp, 3 Hit, and for ramps on other curves 4 Linear, 5 Exp,
//! 6 Log), the ramp's length in bars (00 for the whole section) and the level
//! in tenths of a percent. `1000700` jumps to 70%; `2080250` ramps to 25%
//! over 8 bars on the S-curve, and `5080250` on Exp. See [`Cue`].
//!
//! A ramp's start level is a second parameter, `Section NN Lane L From`: the
//! level in tenths of a percent, or −1 to start from where the lane is. A
//! patch saved before ramps had curves or starts reads as S-curves from where
//! the lane is, as it always played.

use std::f32::consts::PI;
use std::sync::LazyLock;

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    ParameterDisplay, Readout, SignalType,
};

use super::sequencer::StepTimer;

/// The most sections a song has.
pub const SECTIONS: usize = 32;
/// Automation lanes, each with a CV and a Gate output.
pub const LANES: usize = 8;
/// The longest a section can be, in bars.
pub const MAX_BARS: usize = 64;
/// The most clocks a bar can be.
pub const MAX_STEPS: usize = 96;
/// The longest a lane's Glide is, in seconds.
pub const MAX_GLIDE: f32 = 4.0;

/// What a lane does when a section starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Carry on as it was, ramping if it was ramping.
    Hold,
    /// Go straight to the level on the downbeat.
    Jump,
    /// Glide to the level on an S-curve.
    Ramp,
    /// Go to the level for half a step, then back.
    Hit,
}

impl Move {
    pub const ALL: [Move; 4] = [Move::Hold, Move::Jump, Move::Ramp, Move::Hit];

    pub fn name(self) -> &'static str {
        match self {
            Move::Hold => "Hold",
            Move::Jump => "Jump",
            Move::Ramp => "Ramp",
            Move::Hit => "Hit",
        }
    }

}

/// The shape a ramp takes from its start to its level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    /// Leaves and arrives at rest: a hand on a fader.
    #[default]
    S,
    /// The same pace throughout.
    Linear,
    /// Slow to start, fast to finish: a fifth of the way at half time, for a
    /// sweep or a riser.
    Exp,
    /// Fast to start, slow to finish: four fifths of the way at half time,
    /// for a swell that settles or a fade that tails away.
    Log,
}

impl Curve {
    pub const ALL: [Curve; 4] = [Curve::S, Curve::Linear, Curve::Exp, Curve::Log];

    pub fn name(self) -> &'static str {
        match self {
            Curve::S => "S-curve",
            Curve::Linear => "Linear",
            Curve::Exp => "Exp",
            Curve::Log => "Log",
        }
    }

    /// How far along the ramp is, 0 to 1, at `t` of the way through its time.
    pub fn shape(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::S => ease(t),
            Curve::Linear => t,
            Curve::Exp => bend(t),
            Curve::Log => 1.0 - bend(1.0 - t),
        }
    }
}

/// The exponential bend, 0 to 1: `(16^t - 1) / 15`, so it's a fifth of the
/// way at half time.
fn bend(t: f32) -> f32 {
    (16f32.powf(t) - 1.0) / 15.0
}

/// A cue's move digit.
fn code(kind: Move, curve: Curve) -> u32 {
    match (kind, curve) {
        (Move::Hold, _) => 0,
        (Move::Jump, _) => 1,
        (Move::Ramp, Curve::S) => 2,
        (Move::Hit, _) => 3,
        (Move::Ramp, Curve::Linear) => 4,
        (Move::Ramp, Curve::Exp) => 5,
        (Move::Ramp, Curve::Log) => 6,
    }
}

fn from_code(code: u32) -> (Move, Curve) {
    match code {
        1 => (Move::Jump, Curve::S),
        2 => (Move::Ramp, Curve::S),
        3 => (Move::Hit, Curve::S),
        4 => (Move::Ramp, Curve::Linear),
        5 => (Move::Ramp, Curve::Exp),
        6 => (Move::Ramp, Curve::Log),
        _ => (Move::Hold, Curve::S),
    }
}

/// One section's instruction to one lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    pub kind: Move,
    /// The level, in tenths of a percent, 0 to 1000.
    pub level: u16,
    /// How many bars a ramp takes, or 0 for the whole section.
    pub bars: u8,
    /// A ramp's shape (S for every other move).
    pub curve: Curve,
    /// A ramp's start level, in tenths of a percent, or `None` to start from
    /// where the lane is (always `None` for every other move).
    pub from: Option<u16>,
}

impl Cue {
    /// A section that leaves the lane alone.
    pub const HOLD: Cue = Cue { kind: Move::Hold, level: 0, bars: 0, curve: Curve::S, from: None };

    pub const fn jump(level: u16) -> Self {
        Cue { kind: Move::Jump, level, ..Self::HOLD }
    }

    /// An S-curve from where the lane is.
    pub const fn ramp(level: u16, bars: u8) -> Self {
        Cue { kind: Move::Ramp, level, bars, ..Self::HOLD }
    }

    pub const fn hit(level: u16) -> Self {
        Cue { kind: Move::Hit, level, ..Self::HOLD }
    }

    /// This ramp on another curve.
    pub const fn on(self, curve: Curve) -> Self {
        Cue { curve, ..self }
    }

    /// This ramp, starting from a level of its own.
    pub const fn starting(self, from: u16) -> Self {
        Cue { from: Some(from), ..self }
    }

    /// Reads a cue from its parameter values: the cue's and its From (see the
    /// module docs).
    pub fn decode(value: f32, from: f32) -> Self {
        let digits = value.max(0.0).round() as u32;
        let (kind, curve) = from_code(digits / 1_000_000);
        let from = (kind == Move::Ramp && from >= 0.0).then(|| (from.round() as u32).min(1000) as u16);
        Self {
            kind,
            level: (digits % 10_000).min(1000) as u16,
            bars: ((digits / 10_000) % 100).min(MAX_BARS as u32) as u8,
            curve,
            from,
        }
    }

    /// The value of the cue's parameter.
    pub fn encode(self) -> f32 {
        (code(self.kind, self.curve) * 1_000_000 + (self.bars.min(MAX_BARS as u8) as u32) * 10_000 + self.level.min(1000) as u32) as f32
    }

    /// The value of the cue's From parameter.
    pub fn encode_from(self) -> f32 {
        match (self.kind, self.from) {
            (Move::Ramp, Some(from)) => from.min(1000) as f32,
            _ => FROM_WHERE_IT_IS,
        }
    }

    /// The level, 0 to 1.
    pub fn value(self) -> f32 {
        self.level as f32 / 1000.0
    }

    /// The start level, 0 to 1, if it has one.
    pub fn from_value(self) -> Option<f32> {
        self.from.map(|from| from as f32 / 1000.0)
    }
}

/// The largest value a cue's parameter takes.
const CUE_VALUE_LIMIT: f32 = 6_641_000.0;
/// A From parameter's value for a ramp that starts where the lane is.
pub const FROM_WHERE_IT_IS: f32 = -1.0;

/// What "Loop" can hold: off, or the section to go back to.
/// How wide the editor draws the timeline, in points: Compact, Wide and
/// Extra wide.
pub const WIDTHS: [f32; 3] = [440.0, 760.0, 1080.0];
const WIDTH_NAMES: [&str; 3] = ["Compact", "Wide", "Extra wide"];

pub static LOOP_CHOICES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    std::iter::once("Off").chain((1..=SECTIONS).map(|s| keep(s.to_string()))).collect()
});

/// Parameter names made up at start-up. They must be `'static`, and there
/// are hundreds, so they're built once and kept.
struct Names {
    length_ids: Vec<&'static str>,
    length_names: Vec<&'static str>,
    cue_ids: Vec<&'static str>,
    cue_names: Vec<&'static str>,
    from_ids: Vec<&'static str>,
    from_names: Vec<&'static str>,
}

fn keep(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

static NAMES: LazyLock<Names> = LazyLock::new(|| {
    let mut names = Names {
        length_ids: Vec::new(),
        length_names: Vec::new(),
        cue_ids: Vec::new(),
        cue_names: Vec::new(),
        from_ids: Vec::new(),
        from_names: Vec::new(),
    };
    for section in 1..=SECTIONS {
        names.length_ids.push(keep(format!("length_{section}")));
        names.length_names.push(keep(format!("Length {section:02}")));
        for lane in 1..=LANES {
            names.cue_ids.push(keep(format!("cue_{section}_{lane}")));
            names.cue_names.push(keep(format!("Section {section:02} Lane {lane}")));
            names.from_ids.push(keep(format!("from_{section}_{lane}")));
            names.from_names.push(keep(format!("Section {section:02} Lane {lane} From")));
        }
    }
    names
});

static GLIDE_IDS: [&str; LANES] = ["glide_1", "glide_2", "glide_3", "glide_4", "glide_5", "glide_6", "glide_7", "glide_8"];
static GLIDE_NAMES: [&str; LANES] = ["Glide 1", "Glide 2", "Glide 3", "Glide 4", "Glide 5", "Glide 6", "Glide 7", "Glide 8"];
static LANE_IDS: [&str; LANES] = ["lane_1", "lane_2", "lane_3", "lane_4", "lane_5", "lane_6", "lane_7", "lane_8"];
/// Output names: a lane's CV.
pub static LANE_NAMES: [&str; LANES] = ["Lane 1", "Lane 2", "Lane 3", "Lane 4", "Lane 5", "Lane 6", "Lane 7", "Lane 8"];
static GATE_IDS: [&str; LANES] = ["gate_1", "gate_2", "gate_3", "gate_4", "gate_5", "gate_6", "gate_7", "gate_8"];
/// Output names: a lane's Gate.
pub static GATE_NAMES: [&str; LANES] = ["Gate 1", "Gate 2", "Gate 3", "Gate 4", "Gate 5", "Gate 6", "Gate 7", "Gate 8"];

/// A trigger output: high for a while, dipping first if it was still high,
/// so every trigger starts with a rising edge.
#[derive(Clone, Copy, Debug, Default)]
struct Pulse {
    left: usize,
    dip: bool,
}

impl Pulse {
    fn fire(&mut self, length: usize) {
        self.dip = self.left > 0;
        self.left = length;
    }

    fn high(&self) -> bool {
        self.left > 0 && !self.dip
    }

    fn tick(&mut self) {
        self.dip = false;
        self.left = self.left.saturating_sub(1);
    }
}

/// A lane gliding from one level to another.
#[derive(Clone, Copy, Debug)]
struct Ramp {
    from: f32,
    to: f32,
    curve: Curve,
    /// The clock count it started on.
    start: u64,
    /// How many clocks it takes.
    steps: u64,
}

/// A lane's playing state.
#[derive(Clone, Copy, Debug, Default)]
struct Lane {
    /// Where the cues have put it, before Glide.
    level: f32,
    ramp: Option<Ramp>,
    /// A Hit's level, and how many samples it has left.
    hit: Option<(f32, usize)>,
    /// What it outputs, after Glide.
    out: f32,
    /// The gate drops for this sample, so a new cue over a high gate
    /// starts with a rising edge.
    dip: bool,
}

impl Lane {
    /// The level before Glide, Hit included.
    fn target(&self) -> f32 {
        self.hit.map_or(self.level, |(level, _)| level)
    }
}

/// A song timeline: sections of bars, with eight lanes of automation.
///
/// # Ports
///
/// **Inputs:** Clock (each rising edge is a step; a bar is Steps of them),
/// Reset (the next clock starts section 1), Jump (a rising edge goes to the
/// section Jump To picks, at the next bar line) and Jump To (the section, as
/// its number ÷ 32, the same as the Section output). Follow takes another
/// Arranger's Section, and the editor keeps this one's sections the same as
/// that one's; the engine doesn't use it, as two Arrangers counting one Clock
/// already keep step (see `graph::arranger_follow`).
///
/// **Outputs:** Section (the section playing, its index ÷ 32), Section Trig
/// (on each section's downbeat), Bar (on every downbeat), Last Bar (high
/// through each section's final bar, for fills), End (when the last section
/// finishes), then a CV and a Gate for each lane.
///
/// # Timing
///
/// Everything is counted in clocks: a bar is **Steps** clocks, a section is
/// its Length in bars. A ramp's progress between clocks is measured against
/// the clock's own step, so it moves smoothly yet lands on the clock that
/// ends it, swing or not.
pub struct Arranger {
    /// The next clock starts a section (at the start, and after a reset).
    pending: bool,
    /// The last section has finished, with Loop off.
    ended: bool,
    prev_clock: bool,
    prev_reset: bool,
    prev_jump: bool,
    timer: StepTimer,
    /// Clocks since the first, which ramps count.
    count: u64,
    section: usize,
    /// The bar within the section.
    bar: usize,
    /// The step within the bar.
    step: usize,
    /// A section asked for by Jump, for the next bar line.
    queued: Option<usize>,
    lanes: [Lane; LANES],
    section_trig: Pulse,
    bar_trig: Pulse,
    end_trig: Pulse,
    last_bar: bool,
    last_bar_dip: bool,
    sample_rate: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Arranger {
    pub const PORT_CLOCK: usize = 0;
    pub const PORT_RESET: usize = 1;
    pub const PORT_JUMP: usize = 2;
    pub const PORT_JUMP_TO: usize = 3;
    pub const PORT_FOLLOW: usize = 4;

    pub const OUT_SECTION: usize = 0;
    pub const OUT_SECTION_TRIG: usize = 1;
    pub const OUT_BAR: usize = 2;
    pub const OUT_LAST_BAR: usize = 3;
    pub const OUT_END: usize = 4;
    /// Output index of lane 0's CV. Lane `l`'s CV is `OUT_LANES + 2l`, its
    /// Gate the one after.
    pub const OUT_LANES: usize = 5;
    pub const OUT_COUNT: usize = Self::OUT_LANES + 2 * LANES;

    pub const PARAM_STEPS: usize = 0;
    pub const PARAM_SECTIONS: usize = 1;
    pub const PARAM_LOOP: usize = 2;
    pub const PARAM_GLIDE: usize = 3;
    pub const PARAM_LENGTH: usize = Self::PARAM_GLIDE + LANES;
    pub const PARAM_CUES: usize = Self::PARAM_LENGTH + SECTIONS;
    /// Each cue's From: where its ramp starts.
    pub const PARAM_FROMS: usize = Self::PARAM_CUES + SECTIONS * LANES;
    /// How wide the timeline is drawn. The editor's alone: the module
    /// doesn't read it.
    pub const PARAM_WIDTH: usize = Self::PARAM_FROMS + SECTIONS * LANES;
    pub const PARAM_COUNT: usize = Self::PARAM_WIDTH + 1;

    /// The parameter of a cue (both 0-based).
    pub const fn cue_param(section: usize, lane: usize) -> usize {
        Self::PARAM_CUES + section * LANES + lane
    }

    /// The parameter of a cue's From (both 0-based).
    pub const fn from_param(section: usize, lane: usize) -> usize {
        Self::PARAM_FROMS + section * LANES + lane
    }

    /// Readout values: the section playing.
    pub const READOUT_SECTION: usize = 0;
    /// The bar within it.
    pub const READOUT_BAR: usize = 1;
    /// The step within the bar, with how far through it the clock is.
    pub const READOUT_STEP: usize = 2;
    /// 1 once a section has started, + 2 once the song has ended.
    pub const READOUT_FLAGS: usize = 3;
    /// The section a Jump asked for, + 1, or 0.
    pub const READOUT_QUEUED: usize = 4;

    const GATE_THRESHOLD: f32 = 0.5;

    pub fn new() -> Self {
        let mut ports = vec![
            PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0)
                .describe("Each rising edge is a step, and Steps of them make a bar; patch the Clock gate that drives the sequencers"),
            PortDefinition::input_with_default("reset", "Reset", SignalType::Gate, 0.0)
                .describe("A rising edge starts over: the next clock starts section 1"),
            PortDefinition::input_with_default("jump", "Jump", SignalType::Gate, 0.0)
                .describe("A rising edge goes to the section Jump To picks, at the next bar line"),
            PortDefinition::input_with_default("jump_to", "Jump To", SignalType::Control, 0.0)
                .describe("The section a Jump goes to, as its number ÷ 32: the same as the Section output"),
            PortDefinition::input_with_default("follow", "Follow", SignalType::Control, 0.0)
                .describe("Patch another Arranger's Section here to follow it: its sections, their lengths and names, Steps and Loop become this one's. The lanes stay this one's own"),
            PortDefinition::output("section", "Section", SignalType::Control)
                .describe("The section playing, as its index ÷ 32 (section 1 is 0)"),
            PortDefinition::output("section_trig", "Section Trig", SignalType::Gate)
                .describe("A trigger on each section's downbeat"),
            PortDefinition::output("bar", "Bar", SignalType::Gate)
                .describe("A trigger on every bar's downbeat"),
            PortDefinition::output("last_bar", "Last Bar", SignalType::Gate)
                .describe("High through each section's final bar: for fills"),
            PortDefinition::output("end", "End", SignalType::Gate)
                .describe("A trigger when the last section finishes, whether the song loops or stops"),
        ];
        for lane in 0..LANES {
            ports.push(
                PortDefinition::output(LANE_IDS[lane], LANE_NAMES[lane], SignalType::Control)
                    .describe("This lane's level, 0 to 1: smooth enough on a ramp to drive a Mixer's Level directly"),
            );
            ports.push(
                PortDefinition::output(GATE_IDS[lane], GATE_NAMES[lane], SignalType::Gate)
                    .describe("High while this lane is above zero, and for a moment on each Hit"),
            );
        }

        let mut parameters = vec![
            ParameterDefinition::new("steps", "Steps", 1.0, MAX_STEPS as f32, 16.0, ParameterDisplay::stepped(""))
                .describe("How many clocks make a bar: 16 for a Clock's sixteenths in 4/4"),
            ParameterDefinition::new("sections", "Sections", 1.0, SECTIONS as f32, 4.0, ParameterDisplay::stepped(""))
                .describe("How many sections the song has"),
            ParameterDefinition::choice("loop", "Loop", LOOP_CHOICES.as_slice(), 1)
                .describe("The section to go back to after the last one, or Off to stop there"),
        ];
        for lane in 0..LANES {
            parameters.push(
                ParameterDefinition::new(GLIDE_IDS[lane], GLIDE_NAMES[lane], 0.0, MAX_GLIDE, 0.0, ParameterDisplay::linear("s"))
                    .describe("How long this lane's CV takes to glide the whole way from 0 to 1, so a Jump doesn't click"),
            );
        }
        let names = &*NAMES;
        for section in 0..SECTIONS {
            parameters.push(
                ParameterDefinition::new(names.length_ids[section], names.length_names[section], 1.0, MAX_BARS as f32, 4.0, ParameterDisplay::stepped(" bars"))
                    .describe("How many bars this section lasts"),
            );
        }
        for index in 0..SECTIONS * LANES {
            parameters.push(
                ParameterDefinition::new(names.cue_ids[index], names.cue_names[index], 0.0, CUE_VALUE_LIMIT, 0.0, ParameterDisplay::stepped(""))
                    .describe("What this lane does when the section starts: move, ramp bars and level as digits"),
            );
        }
        for index in 0..SECTIONS * LANES {
            parameters.push(
                ParameterDefinition::new(names.from_ids[index], names.from_names[index], FROM_WHERE_IT_IS, 1000.0, FROM_WHERE_IT_IS, ParameterDisplay::stepped(""))
                    .describe("The level this lane's ramp starts from, in tenths of a percent, or -1 for where the lane is"),
            );
        }
        parameters.push(
            ParameterDefinition::choice("width", "Width", WIDTH_NAMES.as_slice(), 0)
                .describe("How wide the timeline is drawn: Wide or Extra wide for a long song read whole on a big screen"),
        );
        debug_assert_eq!(parameters.len(), Self::PARAM_COUNT);
        debug_assert_eq!(ports.len(), 5 + Self::OUT_COUNT);

        Self { ports, parameters, ..Self::new_state() }
    }

    /// How many clocks make a bar.
    pub fn steps(params: &[f32]) -> usize {
        (params[Self::PARAM_STEPS].round() as usize).clamp(1, MAX_STEPS)
    }

    /// How many sections the song has.
    pub fn sections(params: &[f32]) -> usize {
        (params[Self::PARAM_SECTIONS].round() as usize).clamp(1, SECTIONS)
    }

    /// The section the song goes back to after the last, if it loops.
    pub fn loop_to(params: &[f32]) -> Option<usize> {
        match params[Self::PARAM_LOOP].round() as usize {
            0 => None,
            n => Some((n - 1).min(Self::sections(params) - 1)),
        }
    }

    /// How wide the editor draws the timeline, in points, for the Width
    /// parameter's value.
    pub fn width(choice: f32) -> f32 {
        WIDTHS[(choice.max(0.0).round() as usize).min(WIDTHS.len() - 1)]
    }

    /// A section's length, in bars.
    pub fn length(params: &[f32], section: usize) -> usize {
        (params[Self::PARAM_LENGTH + section].round() as usize).clamp(1, MAX_BARS)
    }

    pub fn cue(params: &[f32], section: usize, lane: usize) -> Cue {
        Cue::decode(params[Self::cue_param(section, lane)], params[Self::from_param(section, lane)])
    }

    /// The section a Jump To CV picks.
    pub fn section_for_cv(cv: f32, sections: usize) -> usize {
        ((cv * SECTIONS as f32).round().max(0.0) as usize).min(sections - 1)
    }

    /// How long the coming step is, in samples: as the clock has measured
    /// it, or before two clock edges, a sixteenth at the transport's tempo
    /// (120 BPM without one).
    fn step_samples(&self, context: &ProcessContext) -> usize {
        self.timer.coming_step().unwrap_or_else(|| {
            let bpm = context.transport.tempo_bpm.filter(|&bpm| bpm > 0.0).unwrap_or(120.0);
            (self.sample_rate * 60.0 / bpm / 4.0) as usize
        })
    }

    /// How far through the current step the clock is, 0 to just under 1.
    fn step_fraction(&self, step_samples: usize) -> f32 {
        match self.timer.since_clock() {
            Some(since) if step_samples > 0 => (since as f32 / step_samples as f32).min(0.999),
            _ => 0.0,
        }
    }

    /// A rising clock edge: the next step, and maybe the next bar or section.
    fn clock(&mut self, params: &[f32], pulse: usize) {
        let sections = Self::sections(params);
        if self.pending {
            self.pending = false;
            self.ended = false;
            self.count = 0;
            let first = self.queued.take().unwrap_or(0).min(sections - 1);
            self.start_section(params, first, pulse);
            return;
        }
        self.count = self.count.wrapping_add(1);
        if self.ended {
            // A Jump wakes a finished song on the next clock
            if let Some(section) = self.queued.take() {
                self.ended = false;
                self.start_section(params, section.min(sections - 1), pulse);
            }
            return;
        }
        self.step += 1;
        if self.step < Self::steps(params) {
            return;
        }
        self.step = 0;
        self.bar += 1;
        if let Some(section) = self.queued.take() {
            self.start_section(params, section.min(sections - 1), pulse);
        } else if self.bar < Self::length(params, self.section) {
            self.downbeat(params, pulse);
        } else if self.section + 1 < sections {
            self.start_section(params, self.section + 1, pulse);
        } else {
            self.end_trig.fire(pulse);
            match Self::loop_to(params) {
                Some(section) => self.start_section(params, section, pulse),
                None => {
                    self.ended = true;
                    self.last_bar = false;
                }
            }
        }
    }

    /// A bar's first step: the Bar trigger, and Last Bar if it's the last.
    fn downbeat(&mut self, params: &[f32], pulse: usize) {
        self.bar_trig.fire(pulse);
        let last = self.bar + 1 >= Self::length(params, self.section);
        self.last_bar_dip = last && self.last_bar;
        self.last_bar = last;
    }

    /// A section's downbeat: its trigger, and every lane's cue.
    fn start_section(&mut self, params: &[f32], section: usize, pulse: usize) {
        self.section = section;
        self.bar = 0;
        self.step = 0;
        self.section_trig.fire(pulse);
        self.downbeat(params, pulse);
        let steps = Self::steps(params) as u64;
        let section_bars = Self::length(params, section) as u64;
        for (index, lane) in self.lanes.iter_mut().enumerate() {
            let cue = Self::cue(params, section, index);
            let was_high = lane.target() > 0.0;
            let level = cue.value();
            match cue.kind {
                Move::Hold => {}
                Move::Jump => {
                    lane.level = level;
                    lane.ramp = None;
                    lane.hit = None;
                    lane.dip = was_high && level > 0.0;
                }
                Move::Ramp => {
                    // A start of its own is a Jump there first
                    if let Some(from) = cue.from_value() {
                        lane.level = from;
                        lane.dip = was_high && from > 0.0;
                    }
                    let bars = if cue.bars == 0 { section_bars } else { cue.bars as u64 };
                    lane.ramp = Some(Ramp { from: lane.level, to: level, curve: cue.curve, start: self.count, steps: (bars * steps).max(1) });
                    lane.hit = None;
                }
                Move::Hit => {
                    lane.hit = Some((level, pulse));
                    lane.dip = was_high && level > 0.0;
                }
            }
        }
    }
}

impl Default for Arranger {
    fn default() -> Self {
        Self::new()
    }
}

/// The S-curve a ramp follows, 0 to 1: it leaves and arrives at rest.
pub fn ease(t: f32) -> f32 {
    0.5 - 0.5 * (PI * t.clamp(0.0, 1.0)).cos()
}

impl DspModule for Arranger {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "seq.arranger",
            name: "Arranger",
            category: ModuleCategory::Utility,
            description: "A song's timeline: named sections of bars, moving eight lanes of automation by jumps, ramps and hits",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        // A step measured at another rate is the wrong number of samples
        self.timer.forget();
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let sample = |port: usize, i: usize| inputs.get(port).and_then(|buf| buf.samples.get(i).copied()).unwrap_or(0.0);
        let jump_to = connected_input(inputs, Self::PORT_JUMP_TO);
        let mut glide_rates = [f32::INFINITY; LANES];
        for (lane, rate) in glide_rates.iter_mut().enumerate() {
            let glide = params[Self::PARAM_GLIDE + lane].clamp(0.0, MAX_GLIDE);
            if glide > 0.0 {
                *rate = 1.0 / (glide * self.sample_rate);
            }
        }

        for i in 0..context.block_size {
            let reset_high = sample(Self::PORT_RESET, i) > Self::GATE_THRESHOLD;
            let reset_rising = reset_high && !self.prev_reset;
            self.prev_reset = reset_high;
            let jump_high = sample(Self::PORT_JUMP, i) > Self::GATE_THRESHOLD;
            let jump_rising = jump_high && !self.prev_jump;
            self.prev_jump = jump_high;
            let clock_high = sample(Self::PORT_CLOCK, i) > Self::GATE_THRESHOLD;
            let clock_rising = clock_high && !self.prev_clock;
            self.prev_clock = clock_high;

            if reset_rising {
                self.pending = true;
                self.ended = false;
                self.timer.forget_gap();
                self.section_trig = Pulse::default();
                self.bar_trig = Pulse::default();
                self.end_trig = Pulse::default();
                self.last_bar = false;
                for lane in &mut self.lanes {
                    lane.hit = None;
                }
            }
            if jump_rising {
                let cv = jump_to.map_or(0.0, |buf| buf.samples.get(i).copied().unwrap_or(0.0));
                self.queued = Some(Self::section_for_cv(cv, Self::sections(params)));
            }
            let step_samples = self.step_samples(context);
            if clock_rising {
                self.timer.clock();
                let step_samples = self.step_samples(context);
                let pulse = (step_samples / 2).max(1);
                self.clock(params, pulse);
            }

            let fraction = self.step_fraction(step_samples);
            for (index, lane) in self.lanes.iter_mut().enumerate() {
                if let Some(ramp) = lane.ramp {
                    let elapsed = self.count.wrapping_sub(ramp.start);
                    if elapsed >= ramp.steps {
                        lane.level = ramp.to;
                        lane.ramp = None;
                    } else {
                        let t = (elapsed as f32 + fraction) / ramp.steps as f32;
                        lane.level = ramp.from + (ramp.to - ramp.from) * ramp.curve.shape(t);
                    }
                }
                let target = lane.target();
                let rate = glide_rates[index];
                if rate.is_infinite() || (target - lane.out).abs() <= rate {
                    lane.out = target;
                } else {
                    lane.out += rate.copysign(target - lane.out);
                }
                outputs[Self::OUT_LANES + 2 * index].samples[i] = lane.out;
                outputs[Self::OUT_LANES + 2 * index + 1].samples[i] = if target > 0.0 && !lane.dip { 1.0 } else { 0.0 };
                lane.dip = false;
                if let Some((_, left)) = lane.hit.as_mut() {
                    *left -= 1;
                    if *left == 0 {
                        lane.hit = None;
                    }
                }
            }

            let gate = |high: bool| if high { 1.0 } else { 0.0 };
            outputs[Self::OUT_SECTION].samples[i] = if self.pending { 0.0 } else { self.section as f32 / SECTIONS as f32 };
            outputs[Self::OUT_SECTION_TRIG].samples[i] = gate(self.section_trig.high());
            outputs[Self::OUT_BAR].samples[i] = gate(self.bar_trig.high());
            outputs[Self::OUT_LAST_BAR].samples[i] = gate(self.last_bar && !self.last_bar_dip);
            outputs[Self::OUT_END].samples[i] = gate(self.end_trig.high());
            self.section_trig.tick();
            self.bar_trig.tick();
            self.end_trig.tick();
            self.last_bar_dip = false;
            self.timer.tick();
        }
    }

    fn reset(&mut self) {
        let (sample_rate, ports, parameters) = (self.sample_rate, std::mem::take(&mut self.ports), std::mem::take(&mut self.parameters));
        *self = Self { sample_rate, ports, parameters, ..Self::new_state() };
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_SECTION] = self.section as f32;
        readout.values[Self::READOUT_BAR] = self.bar as f32;
        let step_samples = self.timer.coming_step().unwrap_or(0);
        readout.values[Self::READOUT_STEP] = self.step as f32 + if self.pending || self.ended { 0.0 } else { self.step_fraction(step_samples) };
        let started = if self.pending { 0 } else { 1 };
        let ended = if self.ended { 2 } else { 0 };
        readout.values[Self::READOUT_FLAGS] = (started + ended) as f32;
        readout.values[Self::READOUT_QUEUED] = self.queued.map_or(0.0, |s| (s + 1) as f32);
        Some(readout)
    }
}

impl Arranger {
    /// The playing state of a new Arranger, without its ports and parameters.
    fn new_state() -> Self {
        Self {
            pending: true,
            ended: false,
            prev_clock: false,
            prev_reset: false,
            prev_jump: false,
            timer: StepTimer::new(),
            count: 0,
            section: 0,
            bar: 0,
            step: 0,
            queued: None,
            lanes: [Lane::default(); LANES],
            section_trig: Pulse::default(),
            bar_trig: Pulse::default(),
            end_trig: Pulse::default(),
            last_bar: false,
            last_bar_dip: false,
            sample_rate: 44100.0,
            ports: Vec::new(),
            parameters: Vec::new(),
        }
    }
}

/// What the display reads back from a [`Readout`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub section: usize,
    pub bar: usize,
    /// The step within the bar, with how far through it the clock is.
    pub step: f32,
    /// A section has started since the start or the last reset.
    pub started: bool,
    /// The song has finished, with Loop off.
    pub ended: bool,
    /// The section a Jump will go to at the next bar line.
    pub queued: Option<usize>,
}

impl Position {
    pub fn from_readout(readout: &Readout) -> Self {
        let flags = readout.values[Arranger::READOUT_FLAGS] as u32;
        let queued = readout.values[Arranger::READOUT_QUEUED] as usize;
        Self {
            section: (readout.values[Arranger::READOUT_SECTION] as usize).min(SECTIONS - 1),
            bar: readout.values[Arranger::READOUT_BAR] as usize,
            step: readout.values[Arranger::READOUT_STEP].max(0.0),
            started: flags & 1 != 0,
            ended: flags & 2 != 0,
            queued: queued.checked_sub(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;

    const RATE: f32 = 1000.0;

    fn defaults() -> Vec<f32> {
        Arranger::new().parameters().iter().map(|p| p.default).collect()
    }

    /// A song of sections this many bars long, Steps clocks to the bar.
    fn song(steps: usize, lengths: &[usize]) -> Vec<f32> {
        let mut params = defaults();
        params[Arranger::PARAM_STEPS] = steps as f32;
        params[Arranger::PARAM_SECTIONS] = lengths.len() as f32;
        for (section, &bars) in lengths.iter().enumerate() {
            params[Arranger::PARAM_LENGTH + section] = bars as f32;
        }
        params
    }

    fn set_cue(params: &mut [f32], section: usize, lane: usize, cue: Cue) {
        params[Arranger::cue_param(section, lane)] = cue.encode();
        params[Arranger::from_param(section, lane)] = cue.encode_from();
    }

    /// Runs the Arranger at 1 kHz for `ms`, clocked at the times given with
    /// 1 ms pulses, with resets and jumps (at a time, to a section), and
    /// returns every output.
    fn run_with(params: &[f32], ms: usize, clocks: &[usize], resets: &[usize], jumps: &[(usize, usize)]) -> Vec<Vec<f32>> {
        let mut arranger = Arranger::new();
        arranger.prepare(RATE, ms);
        let mut clock = SignalBuffer::gate(ms);
        for &t in clocks {
            clock.samples[t] = 1.0;
        }
        let mut reset = SignalBuffer::gate(ms);
        for &t in resets {
            reset.samples[t] = 1.0;
        }
        let mut jump = SignalBuffer::gate(ms);
        let mut jump_to = SignalBuffer::control(ms);
        for &(t, section) in jumps {
            jump.samples[t] = 1.0;
            jump_to.samples[t] = section as f32 / SECTIONS as f32;
        }
        // A tempo to match the clock, for the triggers before it's measured
        let first_step = clocks.get(1).zip(clocks.first()).map_or(10, |(b, a)| b - a);
        let transport = TransportState::playing_at(RATE * 60.0 / (4 * first_step) as f32);
        let mut outputs: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(ms)).collect();
        arranger.process(&[&clock, &reset, &jump, &jump_to], &mut outputs, params, &ProcessContext::with_transport(RATE, ms, transport));
        outputs.into_iter().map(|b| b.samples).collect()
    }

    fn run(params: &[f32], ms: usize, period: usize) -> Vec<Vec<f32>> {
        let clocks: Vec<usize> = (0..ms).step_by(period).collect();
        run_with(params, ms, &clocks, &[], &[])
    }

    fn rises(gate: &[f32]) -> Vec<usize> {
        (0..gate.len()).filter(|&t| gate[t] > 0.5 && (t == 0 || gate[t - 1] < 0.5)).collect()
    }

    fn lane(out: &[Vec<f32>], lane: usize) -> &[f32] {
        &out[Arranger::OUT_LANES + 2 * lane]
    }

    fn gate(out: &[Vec<f32>], lane: usize) -> &[f32] {
        &out[Arranger::OUT_LANES + 2 * lane + 1]
    }

    #[test]
    fn cues_round_trip_through_their_parameter() {
        for kind in Move::ALL {
            for level in [0, 1, 250, 999, 1000] {
                for bars in [0, 1, 8, MAX_BARS as u8] {
                    let cue = Cue { kind, level, bars, ..Cue::HOLD };
                    assert_eq!(Cue::decode(cue.encode(), cue.encode_from()), cue);
                    for curve in Curve::ALL {
                        for from in [None, Some(0), Some(280), Some(1000)] {
                            // A curve and a start are a ramp's alone
                            let cue = Cue { curve, from, ..cue };
                            let read = Cue::decode(cue.encode(), cue.encode_from());
                            if kind == Move::Ramp {
                                assert_eq!(read, cue);
                            } else {
                                assert_eq!(read, Cue { curve: Curve::S, from: None, ..cue });
                            }
                            assert!(cue.encode() <= CUE_VALUE_LIMIT);
                        }
                    }
                }
            }
        }
        assert_eq!(Cue::decode(1_000_700.0, FROM_WHERE_IT_IS), Cue::jump(700));
        assert_eq!(Cue::decode(2_080_250.0, FROM_WHERE_IT_IS), Cue::ramp(250, 8));
        assert_eq!(Cue::decode(5_080_250.0, 100.0), Cue::ramp(250, 8).on(Curve::Exp).starting(100));
        assert_eq!(Cue::HOLD.encode(), 0.0);
        // The largest value is still a whole number an f32 holds exactly
        let largest = Cue { kind: Move::Ramp, level: 1000, bars: MAX_BARS as u8, curve: Curve::Log, from: None };
        assert_eq!(largest.encode(), CUE_VALUE_LIMIT);
        assert!(CUE_VALUE_LIMIT < 16_777_216.0);
    }

    #[test]
    fn a_patch_from_before_curves_reads_as_it_played() {
        // Saved without From parameters, so they keep their default
        let defaults = defaults();
        assert!(defaults[Arranger::PARAM_FROMS..Arranger::PARAM_WIDTH].iter().all(|&v| v == FROM_WHERE_IT_IS));
        let mut params = defaults;
        params[Arranger::cue_param(3, 2)] = 2_080_250.0;
        assert_eq!(Arranger::cue(&params, 3, 2), Cue::ramp(250, 8));
    }

    #[test]
    fn each_curve_passes_its_midpoint_and_lands_exactly() {
        for (curve, middle) in [(Curve::S, 0.5), (Curve::Linear, 0.5), (Curve::Exp, 0.2), (Curve::Log, 0.8)] {
            assert_eq!(curve.shape(0.0), 0.0, "{curve:?}");
            assert!((curve.shape(0.5) - middle).abs() < 1e-6, "{curve:?}: {}", curve.shape(0.5));
            assert!((curve.shape(1.0) - 1.0).abs() < 1e-6, "{curve:?}");
            for n in 1..=100 {
                assert!(curve.shape(n as f32 / 100.0) > curve.shape((n - 1) as f32 / 100.0), "{curve:?} rises");
            }
        }
        // Exp and Log mirror each other
        for n in 0..=10 {
            let t = n as f32 / 10.0;
            assert!((Curve::Exp.shape(t) + Curve::Log.shape(1.0 - t) - 1.0).abs() < 1e-6);
        }
        // The S-curve is the one ramps always had
        assert_eq!(Curve::S.shape(0.3), ease(0.3));
    }

    #[test]
    fn ramps_follow_their_curve_and_land_on_their_clock() {
        // Section 2 ramps lane 1 from 0.2 to 1.0 over its two bars (80 ms)
        for (curve, middle) in [(Curve::S, 0.5), (Curve::Linear, 0.5), (Curve::Exp, 0.2), (Curve::Log, 0.8)] {
            let mut params = song(4, &[1, 2, 1]);
            set_cue(&mut params, 0, 0, Cue::jump(200));
            set_cue(&mut params, 1, 0, Cue::ramp(1000, 0).on(curve));
            let out = run(&params, 160, 10);
            let cv = lane(&out, 0);
            assert!((cv[40] - 0.2).abs() < 1e-6, "{curve:?} starts where it was");
            assert!((cv[80] - (0.2 + 0.8 * middle)).abs() < 1e-5, "{curve:?} half way: {}", cv[80]);
            assert!(cv[119] < 1.0, "{curve:?}");
            assert_eq!(cv[120], 1.0, "{curve:?} arrives on the clock that ends it");
            for t in 41..120 {
                assert!(cv[t] >= cv[t - 1], "{curve:?} falls at {t}");
            }
        }
    }

    #[test]
    fn curved_ramps_follow_a_swung_clock() {
        // 66% swing: steps of 16 and 8 ms, 4 to a bar, so a bar is 48 ms
        for (curve, middle) in [(Curve::Linear, 0.5), (Curve::Exp, 0.2), (Curve::Log, 0.8)] {
            let mut params = song(4, &[1, 2]);
            set_cue(&mut params, 1, 0, Cue::ramp(1000, 0).on(curve));
            let clocks: Vec<usize> = (0..480).step_by(24).flat_map(|t| [t, t + 16]).collect();
            let out = run_with(&params, 480, &clocks, &[], &[]);
            let cv = lane(&out, 0);
            // Half way through its eight clocks is the downbeat of bar 2
            assert!((cv[96] - middle).abs() < 1e-5, "{curve:?} half way: {}", cv[96]);
            assert!(cv[143] < 1.0);
            assert_eq!(cv[144], 1.0, "{curve:?} lands on the clock ending its two bars");
            for t in 49..144 {
                assert!(cv[t] >= cv[t - 1], "{curve:?} falls at {t}");
            }
        }
    }

    #[test]
    fn a_ramp_from_a_level_jumps_there_then_ramps() {
        // The pad enters at 28% and swells to 42% over the section
        let mut params = song(4, &[1, 2]);
        params[Arranger::PARAM_LOOP] = 0.0;
        set_cue(&mut params, 0, 0, Cue::jump(600));
        set_cue(&mut params, 1, 0, Cue::ramp(420, 0).on(Curve::Linear).starting(280));
        let out = run(&params, 140, 10);
        let cv = lane(&out, 0);
        assert_eq!(cv[39], 0.6);
        assert_eq!(cv[40], 0.28, "on the downbeat, to the sample");
        assert!((cv[80] - 0.35).abs() < 1e-5, "half way: {}", cv[80]);
        assert_eq!(cv[120], 0.42);
        // The gate dips on the downbeat, as for a Jump
        assert_eq!(gate(&out, 0)[40], 0.0);
        assert_eq!(rises(gate(&out, 0)), [0, 41]);
    }

    #[test]
    fn a_ramp_from_zero_opens_the_gate_as_it_leaves() {
        let mut params = song(4, &[1, 1]);
        set_cue(&mut params, 0, 0, Cue::jump(500));
        set_cue(&mut params, 1, 0, Cue::ramp(1000, 0).starting(0));
        let out = run(&params, 80, 10);
        assert_eq!(lane(&out, 0)[40], 0.0);
        assert_eq!(gate(&out, 0)[40], 0.0);
        assert_eq!(rises(gate(&out, 0)), [0, 41]);
    }

    #[test]
    fn a_ramp_from_a_level_glides_like_a_jump() {
        let mut params = song(4, &[1, 4]);
        params[Arranger::PARAM_GLIDE] = 0.1;
        set_cue(&mut params, 0, 0, Cue::jump(0));
        set_cue(&mut params, 1, 0, Cue::ramp(500, 0).starting(500));
        let out = run(&params, 200, 10);
        let cv = lane(&out, 0);
        // Gliding up to the start at the lane's rate, not stepping
        assert!((cv[40 + 25] - 0.25).abs() < 0.02, "{}", cv[65]);
        assert!((cv[40 + 50] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parameters_and_ports_are_where_the_constants_say() {
        let arranger = Arranger::new();
        let params = arranger.parameters();
        assert_eq!(params.len(), Arranger::PARAM_COUNT);
        assert_eq!(params[Arranger::PARAM_STEPS].name, "Steps");
        assert_eq!(params[Arranger::PARAM_LOOP].name, "Loop");
        assert_eq!(params[Arranger::PARAM_GLIDE + 7].name, "Glide 8");
        assert_eq!(params[Arranger::PARAM_LENGTH + 31].name, "Length 32");
        assert_eq!(params[Arranger::cue_param(0, 0)].name, "Section 01 Lane 1");
        assert_eq!(params[Arranger::cue_param(31, 7)].name, "Section 32 Lane 8");
        assert_eq!(params[Arranger::PARAM_WIDTH].name, "Width");
        let mut names: Vec<&str> = params.iter().map(|p| p.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), params.len());

        let outputs: Vec<&str> = arranger.ports().iter().filter(|p| p.is_output()).map(|p| p.name).collect();
        assert_eq!(outputs.len(), Arranger::OUT_COUNT);
        assert_eq!(outputs[Arranger::OUT_END], "End");
        assert_eq!(outputs[Arranger::OUT_LANES + 2 * 3], "Lane 4");
        assert_eq!(outputs[Arranger::OUT_LANES + 2 * 3 + 1], "Gate 4");
    }

    #[test]
    fn sections_follow_their_lengths_in_bars_and_loop() {
        // Bars of 4 clocks, 10 ms apart: a bar is 40 ms
        let mut params = song(4, &[2, 1, 3]);
        params[Arranger::PARAM_LOOP] = 2.0;
        let out = run(&params, 400, 10);
        // Sections start at bars 0, 2 and 3; then the song loops to section 2
        // after bar 6, so section 2 again at 240 and section 3 at 280
        assert_eq!(rises(&out[Arranger::OUT_SECTION_TRIG]), [0, 80, 120, 240, 280]);
        assert_eq!(rises(&out[Arranger::OUT_BAR]), (0..400).step_by(40).collect::<Vec<_>>());
        assert_eq!(rises(&out[Arranger::OUT_END]), [240]);
        let section_at = |t: usize| (out[Arranger::OUT_SECTION][t] * SECTIONS as f32).round() as usize;
        assert_eq!([section_at(5), section_at(85), section_at(125), section_at(245), section_at(285)], [0, 1, 2, 1, 2]);
        // Last Bar is high through bar 1 (of section 1), bar 2 (section 2,
        // one bar long: it dips at its start) and bar 5
        let last = &out[Arranger::OUT_LAST_BAR];
        assert!(last[45] > 0.5 && last[79] > 0.5 && last[39] < 0.5);
        assert!(last[80] < 0.5, "dips into the next one-bar section");
        assert!(last[81] > 0.5 && last[119] > 0.5 && last[120] < 0.5);
        // After the loop, the one-bar section 2 follows section 3's last bar
        assert_eq!(rises(last), [40, 81, 200, 241, 360]);
        // Triggers last half a step
        assert_eq!(out[Arranger::OUT_BAR][40..].iter().take_while(|&&g| g > 0.5).count(), 5);
    }

    #[test]
    fn with_loop_off_the_song_ends_and_holds() {
        let mut params = song(2, &[1, 1]);
        params[Arranger::PARAM_LOOP] = 0.0;
        set_cue(&mut params, 1, 0, Cue::jump(600));
        let out = run(&params, 200, 10);
        assert_eq!(rises(&out[Arranger::OUT_SECTION_TRIG]), [0, 20]);
        assert_eq!(rises(&out[Arranger::OUT_END]), [40]);
        assert_eq!(rises(&out[Arranger::OUT_BAR]), [0, 20]);
        assert!(out[Arranger::OUT_LAST_BAR][45..].iter().all(|&g| g < 0.5));
        assert!(lane(&out, 0)[199] == 0.6, "the last section's levels hold");
    }

    #[test]
    fn jump_lands_on_the_downbeat_and_hold_keeps_the_level() {
        let mut params = song(4, &[1, 1, 1]);
        set_cue(&mut params, 0, 0, Cue::jump(500));
        set_cue(&mut params, 1, 0, Cue::HOLD);
        set_cue(&mut params, 2, 0, Cue::jump(0));
        let out = run(&params, 120, 10);
        let cv = lane(&out, 0);
        assert_eq!(cv[0], 0.5);
        assert_eq!(cv[79], 0.5, "held through section 2");
        assert_eq!(cv[80], 0.0, "on section 3's downbeat, to the sample");
        // The gate is high while the lane is above zero
        assert!(gate(&out, 0)[..80].iter().all(|&g| g > 0.5));
        assert!(gate(&out, 0)[80..].iter().all(|&g| g < 0.5));
    }

    #[test]
    fn a_jump_between_levels_dips_the_gate() {
        let mut params = song(2, &[1, 1]);
        set_cue(&mut params, 0, 0, Cue::jump(300));
        set_cue(&mut params, 1, 0, Cue::jump(800));
        let out = run(&params, 40, 10);
        assert_eq!(gate(&out, 0)[20], 0.0);
        assert_eq!(rises(gate(&out, 0)), [0, 21]);
    }

    #[test]
    fn ramp_glides_on_an_s_curve_and_lands_on_its_clock() {
        // Section 2 ramps lane 1 from 0.2 to 1.0 over its two bars
        let mut params = song(4, &[1, 2, 1]);
        set_cue(&mut params, 0, 0, Cue::jump(200));
        set_cue(&mut params, 1, 0, Cue::ramp(1000, 0));
        let out = run(&params, 160, 10);
        let cv = lane(&out, 0);
        assert!((cv[40] - 0.2).abs() < 1e-6, "starts where it was");
        // Half way through, half way there
        assert!((cv[80] - 0.6).abs() < 0.01, "{}", cv[80]);
        assert!(cv[50] - 0.2 < 0.6 * 0.1, "eases out of rest");
        assert_eq!(cv[120], 1.0, "arrives on the clock that ends it");
        // Rising all the way, by no more than a small step each sample
        for t in 41..120 {
            assert!(cv[t] >= cv[t - 1], "falls at {t}");
            assert!(cv[t] - cv[t - 1] < 0.03, "jumps at {t}");
        }
    }

    #[test]
    fn a_ramp_over_some_bars_then_holds() {
        let mut params = song(4, &[4]);
        set_cue(&mut params, 0, 0, Cue::ramp(800, 1));
        let out = run(&params, 160, 10);
        let cv = lane(&out, 0);
        assert_eq!(cv[0], 0.0);
        assert!(cv[20] > 0.35 && cv[20] < 0.45);
        assert_eq!(cv[40], 0.8);
        assert!(cv[40..].iter().all(|&v| v == 0.8));
        // The gate opens as the ramp leaves zero
        assert_eq!(rises(gate(&out, 0)), [1]);
    }

    #[test]
    fn ramps_follow_a_swung_clock() {
        // 66% swing: steps of 16 and 8 ms, 4 to a bar, so a bar is 48 ms
        let mut params = song(4, &[1, 2]);
        set_cue(&mut params, 1, 0, Cue::ramp(1000, 0));
        let clocks: Vec<usize> = (0..480).step_by(24).flat_map(|t| [t, t + 16]).collect();
        let out = run_with(&params, 480, &clocks, &[], &[]);
        let cv = lane(&out, 0);
        assert_eq!(rises(&out[Arranger::OUT_SECTION_TRIG])[..3], [0, 48, 144]);
        // Done exactly on the clock ending its two bars
        assert!(cv[143] < 1.0);
        assert_eq!(cv[144], 1.0);
        for t in 49..144 {
            assert!(cv[t] >= cv[t - 1]);
            assert!(cv[t] - cv[t - 1] < 0.05, "jumps at {t}: {} → {}", cv[t - 1], cv[t]);
        }
    }

    #[test]
    fn hit_fires_for_half_a_step_then_returns() {
        let mut params = song(4, &[1, 1]);
        set_cue(&mut params, 0, 0, Cue::jump(200));
        set_cue(&mut params, 1, 0, Cue::hit(900));
        set_cue(&mut params, 0, 1, Cue::hit(700));
        let out = run(&params, 80, 10);
        // From zero: a plain trigger, with its level on the CV
        assert_eq!(rises(gate(&out, 1)), [0]);
        assert_eq!(gate(&out, 1).iter().filter(|&&g| g > 0.5).count(), 5);
        assert_eq!(lane(&out, 1)[0], 0.7);
        assert_eq!(lane(&out, 1)[5], 0.0);
        // Over a level: it dips, hits, and goes back
        assert_eq!(gate(&out, 0)[40], 0.0);
        assert_eq!(lane(&out, 0)[41], 0.9);
        assert_eq!(lane(&out, 0)[45], 0.2);
    }

    #[test]
    fn glide_slews_like_a_sample_and_hold() {
        let mut params = song(4, &[1, 1]);
        params[Arranger::PARAM_GLIDE] = 0.1;
        params[Arranger::PARAM_LOOP] = 0.0;
        set_cue(&mut params, 0, 0, Cue::jump(0));
        set_cue(&mut params, 1, 0, Cue::jump(500));
        let out = run(&params, 200, 10);
        let cv = lane(&out, 0);
        // The whole way in 100 ms, so half way in 50
        assert!((cv[40 + 25] - 0.25).abs() < 0.02, "{}", cv[65]);
        assert!((cv[40 + 50] - 0.5).abs() < 1e-6);
        // The gate follows the cue, not the glide
        assert_eq!(gate(&out, 0)[40], 1.0);
    }

    #[test]
    fn jump_goes_to_its_section_at_the_next_bar_line() {
        let params = song(4, &[2, 2, 2, 2]);
        // Asked for section 4 during bar 0; it starts on bar 1's downbeat
        let clocks: Vec<usize> = (0..400).step_by(10).collect();
        let out = run_with(&params, 400, &clocks, &[], &[(15, 3)]);
        assert_eq!(rises(&out[Arranger::OUT_SECTION_TRIG])[..2], [0, 40]);
        assert!((out[Arranger::OUT_SECTION][45] * SECTIONS as f32 - 3.0).abs() < 1e-4);
        // Then carries on from there, looping to section 1
        assert_eq!(rises(&out[Arranger::OUT_END]), [120]);
    }

    #[test]
    fn reset_starts_from_section_one() {
        let params = song(2, &[1, 1, 1]);
        let clocks: Vec<usize> = (0..200).step_by(10).collect();
        // In section 2 at 25; reset at 35, so 40 starts section 1 again
        let out = run_with(&params, 200, &clocks, &[35], &[]);
        assert_eq!(rises(&out[Arranger::OUT_SECTION_TRIG])[..3], [0, 20, 40]);
        assert_eq!(out[Arranger::OUT_SECTION][45], 0.0);
        assert!(out[Arranger::OUT_SECTION][25] > 0.0);
    }

    #[test]
    fn transport_reset_forgets_the_song_and_the_lanes() {
        let mut params = song(2, &[1, 1]);
        set_cue(&mut params, 0, 0, Cue::jump(500));
        let mut arranger = Arranger::new();
        arranger.prepare(RATE, 1);
        let mut clock = SignalBuffer::gate(1);
        clock.samples[0] = 1.0;
        let mut outputs: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(1)).collect();
        arranger.process(&[&clock], &mut outputs, &params, &ProcessContext::new(RATE, 1));
        assert_eq!(outputs[Arranger::OUT_LANES].samples[0], 0.5);
        arranger.reset();
        assert_eq!(arranger.parameters().len(), Arranger::PARAM_COUNT);
        clock.samples[0] = 0.0;
        arranger.process(&[&clock], &mut outputs, &params, &ProcessContext::new(RATE, 1));
        assert_eq!(outputs[Arranger::OUT_LANES].samples[0], 0.0);
        assert!(!Position::from_readout(&arranger.readout(&params).unwrap()).started);
    }

    #[test]
    fn readout_reports_where_the_song_is() {
        let params = song(4, &[2, 3]);
        let mut arranger = Arranger::new();
        arranger.prepare(RATE, 1);
        let mut clock = SignalBuffer::gate(1);
        let mut outputs: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(1)).collect();
        let ctx = ProcessContext::new(RATE, 1);
        // 15 clocks: section 2's bar 1, step 2
        for _ in 0..15 {
            clock.samples[0] = 1.0;
            arranger.process(&[&clock], &mut outputs, &params, &ctx);
            clock.samples[0] = 0.0;
            arranger.process(&[&clock], &mut outputs, &params, &ctx);
        }
        let at = Position::from_readout(&arranger.readout(&params).unwrap());
        assert!(at.started && !at.ended);
        assert_eq!((at.section, at.bar), (1, 1));
        assert!((2.0..3.0).contains(&at.step), "{}", at.step);
        assert_eq!(at.queued, None);
    }

    #[test]
    fn is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Arranger>();
    }
}
