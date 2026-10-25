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
//!
//! # Repeats
//!
//! A section can play ×1 to ×16, and ×N is exactly N copies of it: every
//! pass gives its cues again, fires Section Trig, Bar and Last Bar, and a
//! ramp over "the section" takes one pass. Only the Section output (and so
//! Jump To and Loop) counts it as one section. A Jump goes to its first pass.
//!
//! # What comes next
//!
//! A section can wander. Its **Next** weighs going on as written against up
//! to two other places: a section, or anywhere (any other section, at
//! random). "Window: on 1, Rain 2, Home 1" goes to Rain half the time. The
//! dice are thrown on the downbeat of the section's last bar, so the editor
//! can show where it's going a bar ahead, and they're seeded, so a render
//! plays the same song every time. All weights at 0 is plain "as written".
//!
//! It's one parameter, read as digits like a cue: `O SSW SSW`, the weight of
//! going on (0 to 9), then each other place's section (01 to 32, or 00 for
//! anywhere) and weight. `1062051` is on 1, section 6 ×2, section 5 ×1. See
//! [`Next`].
//!
//! An Arranger with another's Section in **Follow** throws no dice: on each
//! bar line it goes where its leader is, so the two stay in step however the
//! leader wanders or jumps.
//!
//! The **Next** output says where the next bar line goes, as soon as that's
//! known: the section playing, until its last bar throws its dice or a Jump
//! is asked for. A Setlist reads it to tell the room ahead of the bar (#445).

use std::f32::consts::PI;
use std::sync::LazyLock;

use crate::dsp::{
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    ParameterDisplay, Readout, SignalType,
};

use super::noise::Pcg32;
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
/// The most times a section repeats.
pub const MAX_REPEATS: usize = 16;

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

/// Where a section may go instead of on as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    /// Any other section, at random.
    Anywhere,
    /// This section (0-based).
    Section(usize),
}

/// One place a section may go, and how likely it is: its weight against
/// the others', 0 to 9.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Way {
    pub to: To,
    pub weight: u8,
}

impl Way {
    pub const NONE: Way = Way { to: To::Anywhere, weight: 0 };

    fn code(self) -> u32 {
        let section = match self.to {
            To::Anywhere => 0,
            To::Section(section) => section.min(SECTIONS - 1) as u32 + 1,
        };
        section * 10 + self.weight.min(9) as u32
    }

    fn from_code(code: u32) -> Self {
        let to = match (code / 10).min(SECTIONS as u32) {
            0 => To::Anywhere,
            section => To::Section(section as usize - 1),
        };
        Way { to, weight: (code % 10) as u8 }
    }
}

/// What a section does when it finishes: go on as written, or somewhere
/// else, by weight. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Next {
    /// The weight of going on as written, 0 to 9.
    pub on: u8,
    pub ways: [Way; 2],
}

impl Next {
    /// Always on as written.
    pub const WRITTEN: Next = Next { on: 0, ways: [Way::NONE; 2] };

    pub fn decode(value: f32) -> Self {
        let digits = value.max(0.0).round() as u32;
        Next {
            on: ((digits / 1_000_000) % 10) as u8,
            ways: [Way::from_code((digits / 1000) % 1000), Way::from_code(digits % 1000)],
        }
    }

    pub fn encode(self) -> f32 {
        (self.on.min(9) as u32 * 1_000_000 + self.ways[0].code() * 1000 + self.ways[1].code()) as f32
    }

    /// Whether it ever leaves the written order.
    pub fn wanders(self) -> bool {
        self.ways.iter().any(|way| way.weight > 0)
    }

    /// The chance, 0 to 1, of each place: on as written, then each way.
    pub fn chances(self) -> [f32; 3] {
        if !self.wanders() {
            return [1.0, 0.0, 0.0];
        }
        let total = (self.on + self.ways[0].weight + self.ways[1].weight) as f32;
        [self.on as f32 / total, self.ways[0].weight as f32 / total, self.ways[1].weight as f32 / total]
    }

    /// Where a throw lands, `roll` 0 to just under 1: None for on as
    /// written, else the way taken.
    pub fn pick(self, roll: f32) -> Option<To> {
        let [on, first, _] = self.chances();
        if roll < on {
            None
        } else if roll < on + first {
            Some(self.ways[0].to)
        } else {
            Some(self.ways[1].to)
        }
    }

    /// Whether it may go back to its own section, `section`.
    pub fn may_go_to(self, section: usize) -> bool {
        self.ways.iter().any(|way| way.weight > 0 && way.to == To::Section(section))
    }
}

/// The largest value a Next parameter takes.
const NEXT_VALUE_LIMIT: f32 = 9_329_329.0;

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
    repeat_ids: Vec<&'static str>,
    repeat_names: Vec<&'static str>,
    next_ids: Vec<&'static str>,
    next_names: Vec<&'static str>,
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
        repeat_ids: Vec::new(),
        repeat_names: Vec::new(),
        next_ids: Vec::new(),
        next_names: Vec::new(),
    };
    for section in 1..=SECTIONS {
        names.length_ids.push(keep(format!("length_{section}")));
        names.length_names.push(keep(format!("Length {section:02}")));
        names.repeat_ids.push(keep(format!("repeat_{section}")));
        names.repeat_names.push(keep(format!("Repeat {section:02}")));
        names.next_ids.push(keep(format!("next_{section}")));
        names.next_names.push(keep(format!("Next {section:02}")));
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

/// The module's ID, as patches name it.
pub const ID: &str = "seq.arranger";

/// A song timeline: sections of bars, with eight lanes of automation.
///
/// # Ports
///
/// **Inputs:** Clock (each rising edge is a step; a bar is Steps of them),
/// Reset (the next clock starts section 1), Jump (a rising edge goes to the
/// section Jump To picks, at the next bar line) and Jump To (the section, as
/// its number ÷ 32, the same as the Section output). Follow takes another
/// Arranger's Section, and the editor keeps this one's sections the same as
/// that one's (see `graph::arranger_follow`). Two Arrangers counting one
/// Clock already keep step, but a leader's Next throws dice and its Jump
/// can be patched alone, so on each bar line a follower goes to the section
/// its leader is in.
///
/// **Outputs:** Section (the section playing, its index ÷ 32), Section Trig
/// (on each section's downbeat, every pass), Bar (on every downbeat), Last
/// Bar (high through each pass's final bar, for fills), End (when the last
/// section finishes), Next (the section the next bar line goes to, as soon
/// as that's known), then a CV and a Gate for each lane.
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
    clock: Edge,
    reset: Edge,
    jump: Edge,
    timer: StepTimer,
    /// Clocks since the first, which ramps count.
    count: u64,
    section: usize,
    /// Which time through the section this is, from 0.
    pass: usize,
    /// The bar within the pass.
    bar: usize,
    /// The step within the bar.
    step: usize,
    /// A section asked for by Jump, for the next bar line.
    queued: Option<usize>,
    /// Where the section's Next sent it, thrown on its last bar: None to go
    /// on as written.
    next: Option<usize>,
    /// The dice Next throws. Kept through a reset, so the song wanders on
    /// somewhere new, and seeded, so a render is the same every time.
    rng: Pcg32,
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
    pub const OUT_NEXT: usize = 5;
    /// Output index of lane 0's CV. Lane `l`'s CV is `OUT_LANES + 2l`, its
    /// Gate the one after.
    pub const OUT_LANES: usize = 6;
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
    /// Each section's Repeat. After the rest, so older patches keep their
    /// order and load as ×1.
    pub const PARAM_REPEAT: usize = Self::PARAM_WIDTH + 1;
    /// Each section's Next: where it goes when it finishes.
    pub const PARAM_NEXT: usize = Self::PARAM_REPEAT + SECTIONS;
    pub const PARAM_COUNT: usize = Self::PARAM_NEXT + SECTIONS;

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
    /// Which time through the section, from 0.
    pub const READOUT_PASS: usize = 5;
    /// Where the section's Next has sent it, + 1, or 0 for on as written.
    pub const READOUT_NEXT: usize = 6;

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
                .describe("Patch another Arranger's Section here to follow it: its sections, their lengths and names, Steps, Loop and Next become this one's, and on each bar line this one goes where it is. The lanes stay this one's own"),
            PortDefinition::output("section", "Section", SignalType::Control)
                .describe("The section playing, as its index ÷ 32 (section 1 is 0)"),
            PortDefinition::output("section_trig", "Section Trig", SignalType::Gate)
                .describe("A trigger on each section's downbeat, on every pass of a repeated one"),
            PortDefinition::output("bar", "Bar", SignalType::Gate)
                .describe("A trigger on every bar's downbeat"),
            PortDefinition::output("last_bar", "Last Bar", SignalType::Gate)
                .describe("High through each section's final bar, on every pass: for fills"),
            PortDefinition::output("end", "End", SignalType::Gate)
                .describe("A trigger when the last section finishes, whether the song loops or stops"),
            PortDefinition::output("next", "Next", SignalType::Control)
                .describe("The section the next bar line goes to, as its index ÷ 32: the one playing until its last bar throws its Next or a Jump is asked for. Patch it to a Setlist, which tells the hardware ahead of the bar"),
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
        for section in 0..SECTIONS {
            parameters.push(
                ParameterDefinition::new(names.repeat_ids[section], names.repeat_names[section], 1.0, MAX_REPEATS as f32, 1.0, ParameterDisplay::stepped("×"))
                    .describe("How many times this section plays before the next: ×N is the same as N copies of it"),
            );
        }
        for section in 0..SECTIONS {
            parameters.push(
                ParameterDefinition::new(names.next_ids[section], names.next_names[section], 0.0, NEXT_VALUE_LIMIT, 0.0, ParameterDisplay::stepped(""))
                    .describe("Where this section may go when it finishes, by weight: on as written, or to another section or anywhere, as digits"),
            );
        }
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

    /// How many times a section plays.
    pub fn repeats(params: &[f32], section: usize) -> usize {
        params.get(Self::PARAM_REPEAT + section).map_or(1, |&r| Self::repeat_count(r))
    }

    /// How many times a section plays, from its Repeat parameter's value.
    pub fn repeat_count(value: f32) -> usize {
        (value.round() as usize).clamp(1, MAX_REPEATS)
    }

    /// Where a section may go when it finishes.
    pub fn next(params: &[f32], section: usize) -> Next {
        params.get(Self::PARAM_NEXT + section).map_or(Next::WRITTEN, |&value| Next::decode(value))
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
    /// `leader` is the section the Arranger this one follows is in, if it
    /// follows one.
    fn clock(&mut self, params: &[f32], pulse: usize, leader: Option<usize>) {
        let sections = Self::sections(params);
        if self.pending {
            self.pending = false;
            self.ended = false;
            self.count = 0;
            let first = self.queued.take().or(leader).unwrap_or(0).min(sections - 1);
            self.start_section(params, first, pulse, leader);
            return;
        }
        self.count = self.count.wrapping_add(1);
        if self.ended {
            // A Jump wakes a finished song on the next clock
            if let Some(section) = self.queued.take() {
                self.ended = false;
                self.start_section(params, section.min(sections - 1), pulse, leader);
            }
            return;
        }
        self.step += 1;
        if self.step < Self::steps(params) {
            return;
        }
        self.step = 0;
        self.bar += 1;
        let finished = self.bar >= Self::length(params, self.section) && self.pass + 1 >= Self::repeats(params, self.section);
        if let Some(section) = self.queued.take() {
            self.start_section(params, section.min(sections - 1), pulse, leader);
        } else if let Some(section) = leader.filter(|&section| section != self.section && !finished) {
            // The leader jumped
            self.start_section(params, section, pulse, leader);
        } else if self.bar < Self::length(params, self.section) {
            self.downbeat(params, pulse, leader);
        } else if self.pass + 1 < Self::repeats(params, self.section) {
            self.pass += 1;
            self.start_pass(params, pulse, leader);
        } else {
            self.finish_section(params, pulse, leader);
        }
    }

    /// Where the song goes as written after this section: the next, or
    /// after the last, back to Loop's (None with Loop off).
    fn written(&self, params: &[f32]) -> Option<usize> {
        if self.section + 1 >= Self::sections(params) { Self::loop_to(params) } else { Some(self.section + 1) }
    }

    /// Where the end of this section's last pass goes: on as written, or
    /// where its Next sent it, or where the leader went. None to stop.
    fn going(&self, params: &[f32], leader: Option<usize>) -> Option<usize> {
        let written = self.written(params);
        match leader {
            // A leader still in this section went back to it by its Next,
            // or is on as written to it (a song of one section), or stopped.
            // The Section can't tell the last two apart for a last section
            // with Loop off that may go back to itself: the follower plays on
            Some(section) if section == self.section && written != Some(section) && !Self::next(params, section).may_go_to(section) => written,
            Some(section) => Some(section),
            None => self.next.or(written),
        }
    }

    /// The section the next bar line goes to, as far as it's known: the
    /// one playing, but before the first clock, after a Jump, and in a
    /// section's last bar, once its Next has thrown.
    fn coming(&self, params: &[f32], leader: Option<usize>) -> usize {
        let sections = Self::sections(params);
        if self.pending {
            return self.queued.or(leader).unwrap_or(0).min(sections - 1);
        }
        if let Some(section) = self.queued {
            return section.min(sections - 1);
        }
        if self.ended {
            return self.section;
        }
        let finishing = self.bar + 1 >= Self::length(params, self.section) && self.pass + 1 >= Self::repeats(params, self.section);
        match leader {
            Some(section) if section != self.section && !finishing => section,
            _ if finishing => self.going(params, leader).map_or(self.section, |section| section.min(sections - 1)),
            _ => self.section,
        }
    }

    /// The end of a section's last pass: on as written, or where its Next
    /// sent it, or where the leader went.
    fn finish_section(&mut self, params: &[f32], pulse: usize, leader: Option<usize>) {
        let sections = Self::sections(params);
        let last = self.section + 1 >= sections;
        let written = self.written(params);
        let go = self.going(params, leader);
        self.next = None;
        // The song as written ends here, wherever it goes next
        if last && go == written {
            self.end_trig.fire(pulse);
        }
        match go {
            Some(section) => self.start_section(params, section.min(sections - 1), pulse, leader),
            None => {
                self.ended = true;
                self.last_bar = false;
            }
        }
    }

    /// A bar's first step: the Bar trigger, and Last Bar if it's the last.
    /// On the last pass's last bar, an Arranger that follows none throws its
    /// section's Next.
    fn downbeat(&mut self, params: &[f32], pulse: usize, leader: Option<usize>) {
        self.bar_trig.fire(pulse);
        let last = self.bar + 1 >= Self::length(params, self.section);
        self.last_bar_dip = last && self.last_bar;
        self.last_bar = last;
        if last && self.pass + 1 >= Self::repeats(params, self.section) && leader.is_none() {
            self.next = self.throw(params);
        }
    }

    /// Where this section's Next sends it: None for on as written.
    fn throw(&mut self, params: &[f32]) -> Option<usize> {
        let next = Self::next(params, self.section);
        if !next.wanders() {
            return None;
        }
        let sections = Self::sections(params);
        let roll = |rng: &mut Pcg32| (rng.bipolar() * 0.5 + 0.5).clamp(0.0, 0.999_999);
        match next.pick(roll(&mut self.rng))? {
            To::Section(section) => (section < sections).then_some(section),
            // Any section but this one
            To::Anywhere if sections > 1 => {
                let other = (roll(&mut self.rng) * (sections - 1) as f32) as usize;
                Some(if other >= self.section { other + 1 } else { other }.min(sections - 1))
            }
            To::Anywhere => None,
        }
    }

    /// A section's first downbeat.
    fn start_section(&mut self, params: &[f32], section: usize, pulse: usize, leader: Option<usize>) {
        self.section = section;
        self.pass = 0;
        self.next = None;
        self.start_pass(params, pulse, leader);
    }

    /// The downbeat of a pass through the section: its trigger, and every
    /// lane's cue, just as if it were a copy of the section.
    fn start_pass(&mut self, params: &[f32], pulse: usize, leader: Option<usize>) {
        let section = self.section;
        self.bar = 0;
        self.step = 0;
        self.section_trig.fire(pulse);
        self.downbeat(params, pulse, leader);
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
            id: ID,
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
        let sample = |port: usize, i: usize| inputs.get(port).map_or(0.0, |buf| buf.at(i));
        let jump_to = connected_input(inputs, Self::PORT_JUMP_TO);
        let follow = connected_input(inputs, Self::PORT_FOLLOW);
        let mut glide_rates = [f32::INFINITY; LANES];
        for (lane, rate) in glide_rates.iter_mut().enumerate() {
            let glide = params[Self::PARAM_GLIDE + lane].clamp(0.0, MAX_GLIDE);
            if glide > 0.0 {
                *rate = 1.0 / (glide * self.sample_rate);
            }
        }

        for i in 0..context.block_size {
            let reset_rising = self.reset.rising(sample(Self::PORT_RESET, i));
            let jump_rising = self.jump.rising(sample(Self::PORT_JUMP, i));
            let clock_rising = self.clock.rising(sample(Self::PORT_CLOCK, i));

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
                let cv = jump_to.map_or(0.0, |buf| buf.at(i));
                self.queued = Some(Self::section_for_cv(cv, Self::sections(params)));
            }
            let step_samples = self.step_samples(context);
            if clock_rising {
                self.timer.clock();
                let step_samples = self.step_samples(context);
                let pulse = (step_samples / 2).max(1);
                let leader = follow.map(|buf| Self::section_for_cv(buf.at(i), Self::sections(params)));
                self.clock(params, pulse, leader);
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
            let leader = follow.map(|buf| Self::section_for_cv(buf.at(i), Self::sections(params)));
            outputs[Self::OUT_NEXT].samples[i] = self.coming(params, leader) as f32 / SECTIONS as f32;
            self.section_trig.tick();
            self.bar_trig.tick();
            self.end_trig.tick();
            self.last_bar_dip = false;
            self.timer.tick();
        }
    }

    fn reset(&mut self) {
        let (sample_rate, rng, ports, parameters) = (self.sample_rate, self.rng, std::mem::take(&mut self.ports), std::mem::take(&mut self.parameters));
        *self = Self { sample_rate, rng, ports, parameters, ..Self::new_state() };
    }

    fn seed(&mut self, seed: u64) {
        self.rng = Pcg32::new(seed);
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
        readout.values[Self::READOUT_PASS] = self.pass as f32;
        readout.values[Self::READOUT_NEXT] = self.next.map_or(0.0, |s| (s + 1) as f32);
        Some(readout)
    }
}

impl Arranger {
    /// The playing state of a new Arranger, without its ports and parameters.
    fn new_state() -> Self {
        Self {
            pending: true,
            ended: false,
            clock: Edge::LOW,
            reset: Edge::LOW,
            jump: Edge::LOW,
            timer: StepTimer::new(),
            count: 0,
            section: 0,
            pass: 0,
            bar: 0,
            step: 0,
            queued: None,
            next: None,
            rng: Pcg32::new(0),
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
    /// Which time through the section, from 0.
    pub pass: usize,
    /// The bar within the pass.
    pub bar: usize,
    /// The step within the bar, with how far through it the clock is.
    pub step: f32,
    /// A section has started since the start or the last reset.
    pub started: bool,
    /// The song has finished, with Loop off.
    pub ended: bool,
    /// The section a Jump will go to at the next bar line.
    pub queued: Option<usize>,
    /// Where the section's Next has sent it, once thrown on its last bar.
    pub next: Option<usize>,
}

impl Position {
    pub fn from_readout(readout: &Readout) -> Self {
        let flags = readout.values[Arranger::READOUT_FLAGS] as u32;
        let queued = readout.values[Arranger::READOUT_QUEUED] as usize;
        Self {
            section: (readout.values[Arranger::READOUT_SECTION] as usize).min(SECTIONS - 1),
            pass: (readout.values[Arranger::READOUT_PASS] as usize).min(MAX_REPEATS - 1),
            bar: readout.values[Arranger::READOUT_BAR] as usize,
            step: readout.values[Arranger::READOUT_STEP].max(0.0),
            started: flags & 1 != 0,
            ended: flags & 2 != 0,
            queued: queued.checked_sub(1),
            next: (readout.values[Arranger::READOUT_NEXT] as usize).checked_sub(1),
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
        const { assert!(CUE_VALUE_LIMIT < 16_777_216.0) };
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
        assert_eq!(outputs[Arranger::OUT_NEXT], "Next");
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

    /// A song with a section ×3 (a ramp longer than a pass, a hit, a jump
    /// and a gliding lane), and the same song with it as three copies.
    fn repeated_and_copied() -> (Vec<f32>, Vec<f32>) {
        let cues = |params: &mut [f32], section: usize| {
            set_cue(params, section, 0, Cue::ramp(1000, 3));
            set_cue(params, section, 1, Cue::hit(700));
            set_cue(params, section, 2, Cue::jump(500));
        };
        let mut repeated = song(4, &[2, 1]);
        repeated[Arranger::PARAM_REPEAT] = 3.0;
        cues(&mut repeated, 0);
        let mut copied = song(4, &[2, 2, 2, 1]);
        for section in 0..3 {
            cues(&mut copied, section);
        }
        for params in [&mut repeated, &mut copied] {
            params[Arranger::PARAM_GLIDE + 2] = 0.02;
        }
        // The last section brings lanes 0 and 2 back down
        set_cue(&mut repeated, 1, 0, Cue::jump(0));
        set_cue(&mut repeated, 1, 2, Cue::ramp(0, 0));
        set_cue(&mut copied, 3, 0, Cue::jump(0));
        set_cue(&mut copied, 3, 2, Cue::ramp(0, 0));
        (repeated, copied)
    }

    #[test]
    fn a_section_repeated_plays_exactly_as_its_copies() {
        let (repeated, copied) = repeated_and_copied();
        let straight: Vec<usize> = (0..700).step_by(10).collect();
        let swung: Vec<usize> = (0..700).step_by(24).flat_map(|t| [t, t + 16]).filter(|&t| t < 700).collect();
        for clocks in [straight, swung] {
            // Over two times round the song, as it loops
            let a = run_with(&repeated, 700, &clocks, &[], &[]);
            let b = run_with(&copied, 700, &clocks, &[], &[]);
            for output in 0..Arranger::OUT_COUNT {
                // These count sections, which a repeat is one of
                if output == Arranger::OUT_SECTION || output == Arranger::OUT_NEXT {
                    continue;
                }
                assert_eq!(a[output], b[output], "output {output} differs");
            }
            assert!(a[Arranger::OUT_LANES].iter().any(|&v| v > 0.0), "the test moves something");
        }
    }

    #[test]
    fn every_pass_fires_the_section_triggers() {
        let (params, _) = repeated_and_copied();
        let out = run(&params, 280, 10);
        // Passes start at bars 0, 2 and 4, then the last section at 6
        assert_eq!(rises(&out[Arranger::OUT_SECTION_TRIG]), [0, 80, 160, 240]);
        assert_eq!(rises(&out[Arranger::OUT_LAST_BAR]), [40, 120, 200, 241]);
        assert_eq!(rises(gate(&out, 1)), [0, 80, 160], "a hit on every pass");
        // The Section output counts it as one
        let section_at = |t: usize| (out[Arranger::OUT_SECTION][t] * SECTIONS as f32).round() as usize;
        assert_eq!([section_at(5), section_at(205), section_at(245)], [0, 0, 1]);
    }

    #[test]
    fn readout_counts_the_passes_and_a_jump_goes_to_the_first() {
        let mut params = song(4, &[1, 1]);
        params[Arranger::PARAM_REPEAT + 1] = 4.0;
        // Bar 1 is section 1, bars 2 to 5 section 2's four passes: 14
        // clocks is the third pass. Then a jump to section 2, from inside
        // it, and three more clocks to the next bar line
        let ms = 2 * (14 + 3);
        let clocks: Vec<usize> = (0..ms).step_by(2).collect();
        let mut arranger = Arranger::new();
        arranger.prepare(RATE, 1);
        let mut outputs: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(1)).collect();
        let ctx = ProcessContext::new(RATE, 1);
        let mut at_third_pass = None;
        for t in 0..ms {
            let mut clock = SignalBuffer::gate(1);
            clock.samples[0] = if clocks.contains(&t) { 1.0 } else { 0.0 };
            let mut jump = SignalBuffer::gate(1);
            let mut jump_to = SignalBuffer::control(1);
            if t == 27 {
                jump.samples[0] = 1.0;
                jump_to.samples[0] = 1.0 / SECTIONS as f32;
                at_third_pass = Some(Position::from_readout(&arranger.readout(&params).unwrap()));
            }
            arranger.process(&[&clock, &SignalBuffer::gate(1), &jump, &jump_to], &mut outputs, &params, &ctx);
        }
        let at = at_third_pass.unwrap();
        assert_eq!((at.section, at.pass, at.bar), (1, 2, 0));
        let at = Position::from_readout(&arranger.readout(&params).unwrap());
        assert_eq!((at.section, at.pass, at.bar), (1, 0, 0));
    }

    #[test]
    fn an_old_patch_without_repeats_plays_each_section_once() {
        let mut params = song(4, &[1, 1]);
        params.truncate(Arranger::PARAM_REPEAT);
        assert_eq!(Arranger::repeats(&params, 0), 1);
        assert_eq!(Arranger::new().parameters()[Arranger::PARAM_REPEAT + 31].name, "Repeat 32");
    }

    /// Runs a seeded Arranger clocked every `period` ms, following `follow`
    /// if given, and returns every output.
    fn run_seeded(params: &[f32], ms: usize, period: usize, seed: u64, follow: Option<&[f32]>) -> Vec<Vec<f32>> {
        let mut arranger = Arranger::new();
        arranger.seed(seed);
        arranger.prepare(RATE, ms);
        let mut clock = SignalBuffer::gate(ms);
        for t in (0..ms).step_by(period) {
            clock.samples[t] = 1.0;
        }
        let (reset, jump, jump_to) = (SignalBuffer::gate(ms), SignalBuffer::gate(ms), SignalBuffer::control(ms));
        let mut leader = SignalBuffer::control(ms);
        if let Some(follow) = follow {
            leader.samples.copy_from_slice(follow);
        }
        let mut inputs = vec![&clock, &reset, &jump, &jump_to];
        if follow.is_some() {
            inputs.push(&leader);
        }
        let transport = TransportState::playing_at(RATE * 60.0 / (4 * period) as f32);
        let mut outputs: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(ms)).collect();
        arranger.process(&inputs, &mut outputs, params, &ProcessContext::with_transport(RATE, ms, transport));
        outputs.into_iter().map(|b| b.samples).collect()
    }

    /// The section each Section Trig starts, in order.
    fn sections_played(out: &[Vec<f32>]) -> Vec<usize> {
        rises(&out[Arranger::OUT_SECTION_TRIG]).into_iter().map(|t| (out[Arranger::OUT_SECTION][t] * SECTIONS as f32).round() as usize).collect()
    }

    fn way(section: usize, weight: u8) -> Way {
        Way { to: To::Section(section), weight }
    }

    #[test]
    fn next_round_trips_through_its_parameter() {
        assert_eq!(Next::decode(0.0), Next::WRITTEN);
        assert!(!Next::WRITTEN.wanders());
        // On 1, section 6 ×2, section 5 ×1
        let window = Next { on: 1, ways: [way(5, 2), way(4, 1)] };
        assert_eq!(window.encode(), 1_062_051.0);
        assert_eq!(Next::decode(1_062_051.0), window);
        for on in [0, 1, 9] {
            for to in [To::Anywhere, To::Section(0), To::Section(SECTIONS - 1)] {
                for weight in [0, 1, 9] {
                    let next = Next { on, ways: [Way { to, weight }, way(3, 9 - weight)] };
                    assert_eq!(Next::decode(next.encode()), next);
                    assert!(next.encode() <= NEXT_VALUE_LIMIT);
                }
            }
        }
        let most = Next { on: 9, ways: [way(SECTIONS - 1, 9); 2] };
        assert_eq!(most.encode(), NEXT_VALUE_LIMIT);
        const { assert!(NEXT_VALUE_LIMIT < 16_777_216.0) };
        // The chances add up, and a throw lands where they say
        assert_eq!(window.chances(), [0.25, 0.5, 0.25]);
        assert_eq!([window.pick(0.1), window.pick(0.3), window.pick(0.9)], [None, Some(To::Section(5)), Some(To::Section(4))]);
        assert_eq!(Next::WRITTEN.chances(), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_section_that_always_wanders_goes_where_it_says() {
        // Section 1 always goes to section 3, skipping 2; Loop goes to 1
        let mut params = song(4, &[1, 1, 1]);
        params[Arranger::PARAM_NEXT] = Next { on: 0, ways: [way(2, 1), Way::NONE] }.encode();
        let out = run_seeded(&params, 400, 10, 7, None);
        assert_eq!(sections_played(&out), [0, 2, 0, 2, 0, 2, 0, 2, 0, 2]);
        assert_eq!(rises(&out[Arranger::OUT_BAR]).len(), 10);
        // Section 3 is the last, and goes on as written: the song ends there
        assert_eq!(rises(&out[Arranger::OUT_END]), [80, 160, 240, 320]);
    }

    #[test]
    fn the_last_section_wandering_is_not_the_end() {
        let mut params = song(4, &[1, 1]);
        params[Arranger::PARAM_LOOP] = 0.0;
        params[Arranger::PARAM_NEXT + 1] = Next { on: 0, ways: [way(0, 1), Way::NONE] }.encode();
        let out = run_seeded(&params, 200, 10, 1, None);
        assert_eq!(sections_played(&out), [0, 1, 0, 1, 0]);
        assert!(rises(&out[Arranger::OUT_END]).is_empty());
    }

    #[test]
    fn sections_wander_by_their_weights_and_anywhere_is_somewhere_else() {
        // Section 1: on 1 (to 2), section 4 ×3. Then 2, 3 and 4 go back to 1
        let mut params = song(1, &[1, 1, 1, 1]);
        params[Arranger::PARAM_NEXT] = Next { on: 1, ways: [way(3, 3), Way::NONE] }.encode();
        for section in 1..4 {
            params[Arranger::PARAM_NEXT + section] = Next { on: 0, ways: [way(0, 1), Way::NONE] }.encode();
        }
        let played = sections_played(&run_seeded(&params, 8000, 2, 3, None));
        let after_first: Vec<usize> = played.windows(2).filter(|w| w[0] == 0).map(|w| w[1]).collect();
        let to_four = after_first.iter().filter(|&&s| s == 3).count() as f32 / after_first.len() as f32;
        assert!(after_first.len() > 1000 && after_first.iter().all(|&s| s == 1 || s == 3));
        assert!((to_four - 0.75).abs() < 0.05, "went to 4 {to_four} of the time");

        // Anywhere: never itself, and everywhere else
        let mut params = song(1, &[1, 1, 1, 1]);
        for section in 0..4 {
            params[Arranger::PARAM_NEXT + section] = Next { on: 0, ways: [Way { to: To::Anywhere, weight: 1 }, Way::NONE] }.encode();
        }
        let played = sections_played(&run_seeded(&params, 4000, 2, 3, None));
        assert!(played.windows(2).all(|w| w[0] != w[1]));
        for section in 0..4 {
            assert!(played.iter().filter(|&&s| s == section).count() > 150, "section {} plays", section + 1);
        }
    }

    #[test]
    fn a_seed_plays_the_same_song_every_time() {
        let mut params = song(1, &[1, 1, 1, 1, 1]);
        for section in 0..5 {
            params[Arranger::PARAM_NEXT + section] = Next { on: 1, ways: [Way { to: To::Anywhere, weight: 2 }, Way::NONE] }.encode();
        }
        let a = sections_played(&run_seeded(&params, 1000, 2, 11, None));
        assert_eq!(a, sections_played(&run_seeded(&params, 1000, 2, 11, None)));
        assert_ne!(a, sections_played(&run_seeded(&params, 1000, 2, 12, None)));
    }

    #[test]
    fn the_throw_shows_on_the_last_bar() {
        // Two clocks a bar, two bars in section 1: the throw is on bar 2
        let mut params = song(2, &[2, 1, 1]);
        params[Arranger::PARAM_NEXT] = Next { on: 0, ways: [way(2, 1), Way::NONE] }.encode();
        let mut arranger = Arranger::new();
        arranger.prepare(RATE, 1);
        let mut outputs: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(1)).collect();
        let ctx = ProcessContext::new(RATE, 1);
        let mut nexts = Vec::new();
        for _ in 0..5 {
            for high in [1.0, 0.0] {
                let mut clock = SignalBuffer::gate(1);
                clock.samples[0] = high;
                arranger.process(&[&clock], &mut outputs, &params, &ctx);
            }
            let at = Position::from_readout(&arranger.readout(&params).unwrap());
            nexts.push((at.section, at.next));
        }
        // Clocks 1-2 are bar 1, 3-4 bar 2 (thrown), 5 is section 3
        assert_eq!(nexts, [(0, None), (0, None), (0, Some(2)), (0, Some(2)), (2, None)]);
    }

    /// The section Next names at `t`.
    fn next_at(out: &[Vec<f32>], t: usize) -> usize {
        (out[Arranger::OUT_NEXT][t] * SECTIONS as f32).round() as usize
    }

    #[test]
    fn next_names_where_the_bar_line_goes_once_its_known() {
        // Two bars of four 10 ms clocks, then one bar, then one, looping
        let params = song(4, &[2, 1, 1]);
        let out = run(&params, 400, 10);
        // Before the first clock it names the section that clock starts
        assert_eq!(next_at(&out, 0), 0);
        // Section 1's first bar goes on into its second, which goes to 2
        assert_eq!(next_at(&out, 35), 0);
        assert_eq!(next_at(&out, 40), 1, "from the last bar's downbeat");
        assert_eq!(next_at(&out, 79), 1);
        // A section a bar long names the next all through it
        assert_eq!(next_at(&out, 80), 2);
        assert_eq!(next_at(&out, 120), 0, "the last loops back");
        // And Section only changes on the bar line Next foretold
        assert!((out[Arranger::OUT_SECTION][79] * SECTIONS as f32).round() == 0.0);
        assert!((out[Arranger::OUT_SECTION][80] * SECTIONS as f32 - 1.0).abs() < 1e-4);
    }

    #[test]
    fn next_follows_a_jump_and_the_dice_and_holds_at_the_end() {
        // A Jump asked for mid-bar shows at once, for the next bar line
        let params = song(4, &[4, 4, 4]);
        let clocks: Vec<usize> = (0..200).step_by(10).collect();
        let out = run_with(&params, 200, &clocks, &[], &[(15, 2)]);
        assert_eq!((next_at(&out, 14), next_at(&out, 15)), (0, 2));
        assert_eq!(next_at(&out, 45), 2, "then the jumped-to section goes on");
        // Asked for before the first clock, it's where the song starts
        let out = run_with(&params, 50, &[10, 20], &[], &[(0, 1)]);
        assert_eq!(next_at(&out, 5), 1);
        // The dice: section 1 always goes to 3, which Next says on its last bar
        let mut params = song(4, &[2, 1, 1]);
        params[Arranger::PARAM_NEXT] = Next { on: 0, ways: [way(2, 1), Way::NONE] }.encode();
        let out = run_seeded(&params, 120, 10, 3, None);
        assert_eq!((next_at(&out, 39), next_at(&out, 40)), (0, 2));
        // With Loop off the end goes nowhere: Next holds the last section
        let mut params = song(4, &[1, 1]);
        params[Arranger::PARAM_LOOP] = 0.0;
        let out = run(&params, 120, 10);
        assert_eq!(next_at(&out, 45), 1);
        assert_eq!(next_at(&out, 100), 1);
    }

    #[test]
    fn a_follower_goes_where_its_leader_wanders_and_jumps() {
        let mut params = song(2, &[1, 2, 1, 1]);
        for section in 0..4 {
            params[Arranger::PARAM_NEXT + section] = Next { on: 1, ways: [Way { to: To::Anywhere, weight: 3 }, Way::NONE] }.encode();
        }
        let leader = run_seeded(&params, 2000, 5, 21, None);
        let follower = run_seeded(&params, 2000, 5, 99, Some(&leader[Arranger::OUT_SECTION]));
        assert!(sections_played(&leader).len() > 50);
        for output in [Arranger::OUT_SECTION, Arranger::OUT_SECTION_TRIG, Arranger::OUT_BAR, Arranger::OUT_LAST_BAR, Arranger::OUT_END] {
            assert_eq!(leader[output], follower[output], "output {output} differs");
        }

        // A leader that jumps mid-section on a Jump of its own: the follower
        // goes with it on the bar line
        let params = song(2, &[4, 4, 4]);
        let clocks: Vec<usize> = (0..300).step_by(5).collect();
        let leader = run_with(&params, 300, &clocks, &[], &[(12, 2)]);
        let follower = run_seeded(&params, 300, 5, 0, Some(&leader[Arranger::OUT_SECTION]));
        assert_eq!(sections_played(&leader)[..2], [0, 2]);
        assert_eq!(leader[Arranger::OUT_SECTION], follower[Arranger::OUT_SECTION]);
        assert_eq!(leader[Arranger::OUT_SECTION_TRIG], follower[Arranger::OUT_SECTION_TRIG]);
    }

    #[test]
    fn a_follower_of_a_section_that_goes_back_to_itself_goes_back_too() {
        // Sections 2 and 3 (the last) go back to themselves half the time:
        // the follower sees its leader still in the section, and asks the
        // Next they share whether that was a choice
        let mut params = song(2, &[1, 1, 1]);
        for section in 1..3 {
            params[Arranger::PARAM_NEXT + section] = Next { on: 1, ways: [way(section, 1), Way::NONE] }.encode();
        }
        let leader = run_seeded(&params, 1000, 5, 4, None);
        let follower = run_seeded(&params, 1000, 5, 5, Some(&leader[Arranger::OUT_SECTION]));
        let played = sections_played(&leader);
        assert!(played.windows(2).any(|w| w == [1, 1]) && played.windows(2).any(|w| w == [2, 2]), "{played:?}");
        assert!(played.windows(2).any(|w| w == [1, 2]) && played.windows(2).any(|w| w == [2, 0]), "{played:?}");
        for output in [Arranger::OUT_SECTION_TRIG, Arranger::OUT_LAST_BAR, Arranger::OUT_END] {
            assert_eq!(leader[output], follower[output], "output {output} differs");
        }
    }

    #[test]
    fn an_old_patch_without_next_plays_as_written() {
        let mut params = song(4, &[1, 1, 1]);
        params.truncate(Arranger::PARAM_NEXT);
        assert_eq!(Arranger::next(&params, 0), Next::WRITTEN);
        let out = run(&params, 120, 10);
        assert_eq!(rises(&out[Arranger::OUT_SECTION_TRIG]), [0, 40, 80]);
    }

    #[test]
    fn is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Arranger>();
    }
}
