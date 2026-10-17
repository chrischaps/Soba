//! Slicer module.
//!
//! Chops whatever passes through it into a rhythm, in time with the patch,
//! the way a Boss Slicer pedal does: a held chord comes out as a gated
//! sixteenth-note figure, a pad as a pulse, a drone as a groove. A row of
//! sixteen steps says which slices open and how hard, and the rhythm runs on
//! the patch's Clock, so it lands on the beat with everything else.
//!
//! Each step is a rest, a ghost, a hit or an accent, or a tie that holds the
//! slice before it open through its turn. Duty says how much of a step the
//! slice stays open for, and Shape how it opens and closes: crisp at 0,
//! swelling in and out at 1. Swing pushes every second step late.
//!
//! The Filter knob adds the pedal's step filter: each slice opens a
//! resonant lowpass that closes through its turn, brighter the harder the
//! step. With Depth at 0 the gaps aren't cut at all and the steps only
//! open and close the filter, the "wah step" of the pedal's filter patterns.
//! Ping-Pong sends alternate slices left and right, from the left at the
//! top of the row.
//!
//! With nothing patched to Reset the row runs from the transport's first
//! downbeat, so step 1 falls on the bar. Without a playing Clock it keeps a
//! beat of its own at the patch's tempo (120 BPM without one), so it chops
//! live input whether or not the transport runs.
//!
//! # How a step is stored
//!
//! Each step is one parameter, 0 to 4: a rest, a ghost, a hit, an accent
//! or a tie. See [`Slice`].

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    denormal::flush,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::prewarp,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// The module's ID, as patches name it.
pub const ID: &str = "fx.slicer";

/// Steps in the row.
pub const STEPS: usize = 16;

/// The Rate menu: how long a step is.
pub const RATES: &[&str] = &["1/8", "1/16", "1/32", "1/8T", "1/16T"];

/// Beats in a step, in Rate menu order.
const RATE_BEATS: [f64; 5] = [0.5, 0.25, 0.125, 1.0 / 3.0, 1.0 / 6.0];

/// The quickest a slice opens and closes, at Shape 0: short enough to sound
/// cut, long enough not to click.
const CRISP_ATTACK_SECONDS: f32 = 0.0008;
const CRISP_RELEASE_SECONDS: f32 = 0.0025;

/// At Shape 1 a slice swells in over this much of its step, and fades out
/// over this much.
pub const SWELL_ATTACK: f32 = 0.45;
pub const SWELL_RELEASE: f32 = 0.6;

/// The step filter: wide open at the top, and at full Filter this many
/// octaves lower by the end of a soft step.
const FILTER_TOP_HZ: f32 = 20000.0;
const FILTER_OCTAVES: f32 = 8.0;
/// Its resonance, for the vocal edge of a stepped wah, and the trim that
/// keeps the resonant peak from coming out louder than the input.
const FILTER_Q: f32 = 1.5;
const FILTER_TRIM: f32 = 0.8;
/// How much of a step the filter takes to close most of the way.
pub const FILTER_CLOSE: f32 = 0.35;

/// How far the beat must step back to count as the transport starting over.
const RESTART_BEATS: f64 = 0.5;

/// What a step does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slice {
    /// Closed: the sound is cut, as deep as Depth.
    Rest,
    /// Opens softly.
    Ghost,
    /// Opens.
    Hit,
    /// Opens at full level, and brightest through the Filter.
    Accent,
    /// Holds the slice before it open through this step too.
    Tie,
}

impl Slice {
    /// Every kind, in stored order.
    pub const ALL: [Slice; 5] = [Slice::Rest, Slice::Ghost, Slice::Hit, Slice::Accent, Slice::Tie];

    /// Reads a step from its parameter value.
    pub fn decode(value: f32) -> Self {
        Self::ALL[(value.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }

    /// The parameter value that stores this step.
    pub fn encode(self) -> f32 {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0) as f32
    }

    /// How loud it opens, 0 to 1. A tie takes the level of the slice it
    /// holds (see [`Slicer::run_level`]).
    pub fn level(self) -> f32 {
        match self {
            Slice::Rest | Slice::Tie => 0.0,
            Slice::Ghost => 0.45,
            Slice::Hit => 0.8,
            Slice::Accent => 1.0,
        }
    }

    /// The character a pattern writes it as: `-` `.` `x` `X` `=`.
    pub fn symbol(self) -> char {
        match self {
            Slice::Rest => '-',
            Slice::Ghost => '.',
            Slice::Hit => 'x',
            Slice::Accent => 'X',
            Slice::Tie => '=',
        }
    }

    /// The step a pattern's character writes, if it's one.
    pub fn from_symbol(symbol: char) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.symbol() == symbol)
    }

    /// Its name, for menus and tooltips.
    pub fn name(self) -> &'static str {
        match self {
            Slice::Rest => "Rest",
            Slice::Ghost => "Ghost",
            Slice::Hit => "Hit",
            Slice::Accent => "Accent",
            Slice::Tie => "Tie",
        }
    }
}

/// Patterns to start from, each sixteen steps written as [`Slice`]
/// symbols, how many of them play, and the family the menu files it under.
pub const PATTERNS: &[(&str, &str, usize, &str)] = &[
    ("Trance", "X-xXx-x-X-xXx-xx", 16, "Steady"),
    ("Sixteenths", "XxxxXxxxXxxxXxxx", 16, "Steady"),
    ("Eighths", "X=x=X=x=X=x=X=x=", 16, "Steady"),
    ("Quarters", "X=--x=--X=--x=--", 16, "Steady"),
    ("Offbeats", "--X=--X=--X=--X=", 16, "Steady"),
    ("Sidechain", ".X==.X==.X==.X==", 16, "Steady"),
    ("Pulse", "X===----X===----", 16, "Steady"),
    ("Breath", "X=======.=======", 16, "Steady"),
    ("Gallop", "X-xxX-xxX-xxX-xx", 16, "Groove"),
    ("Reverse Gallop", "Xx-xXx-xXx-xXx-x", 16, "Groove"),
    ("Ghosts", "X..xX..xX..xX.xx", 16, "Groove"),
    ("Funk", "X.x..xX.x.xX.x..", 16, "Groove"),
    ("Stutter", "XxXxXxXx-.-.X=--", 16, "Groove"),
    ("Build", "x-x-x-x-xxxxXXXX", 16, "Groove"),
    ("Heartbeat", "Xx------Xx------", 16, "Groove"),
    ("SOS", "x-x-x-X=-X=-X=--", 16, "Groove"),
    ("Tresillo", "X==X==X=X==X==X=", 16, "Clave and kin"),
    ("Son Clave", "X--x--X---x-X---", 16, "Clave and kin"),
    ("Rumba Clave", "X--x---x--x-X---", 16, "Clave and kin"),
    ("Bossa Nova", "X--x--x---x--x--", 16, "Clave and kin"),
    ("Cinquillo", "X-xx-xx-X-xx-xx-", 16, "Clave and kin"),
    ("Dembow", "X--xX-x-X--xX-x-", 16, "Clave and kin"),
    ("Samba", "X--x-x-x--x-x-x-", 16, "Clave and kin"),
    ("Dotted", "X==x==x==x==x=x=", 16, "Across the bar"),
    ("Threes", "X-xX-xX-xX-xX-x-", 12, "Across the bar"),
    ("Fives", "X-xx-X-xx-X-xx--", 15, "Across the bar"),
    ("Sevens", "X-x-x--X-x-x----", 14, "Across the bar"),
];

/// The menu's families, in order.
pub fn pattern_families() -> impl Iterator<Item = &'static str> {
    PATTERNS.iter().enumerate().filter(|&(i, p)| PATTERNS[..i].iter().all(|q| q.3 != p.3)).map(|(_, p)| p.3)
}

/// The pattern a new Slicer plays.
pub const DEFAULT_PATTERN: usize = 0;

/// The steps a pattern writes, rests past its end.
pub fn pattern_steps(symbols: &str) -> [Slice; STEPS] {
    let mut steps = [Slice::Rest; STEPS];
    for (step, symbol) in steps.iter_mut().zip(symbols.chars()) {
        *step = Slice::from_symbol(symbol).unwrap_or(Slice::Rest);
    }
    steps
}

/// Parameter names: they must be `'static`.
static STEP_IDS: [&str; STEPS] = [
    "step_1", "step_2", "step_3", "step_4", "step_5", "step_6", "step_7", "step_8",
    "step_9", "step_10", "step_11", "step_12", "step_13", "step_14", "step_15", "step_16",
];
pub static STEP_NAMES: [&str; STEPS] = [
    "Step 01", "Step 02", "Step 03", "Step 04", "Step 05", "Step 06", "Step 07", "Step 08",
    "Step 09", "Step 10", "Step 11", "Step 12", "Step 13", "Step 14", "Step 15", "Step 16",
];

/// Where the row is: which step, how far through it, and how long it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// Steps since the row began, swing and all: step `n % Steps` of the row.
    pub step: i64,
    /// How far through it, 0 to 1.
    pub along: f64,
    /// How long it is, in steps: longer than 1 on the beat when swung,
    /// shorter off it.
    pub length: f64,
}

/// One channel of the step filter: a TPT state-variable lowpass.
#[derive(Clone, Copy, Debug, Default)]
struct Lowpass {
    ic1: f32,
    ic2: f32,
}

impl Lowpass {
    #[inline]
    fn process(&mut self, x: f32, a1: f32, a2: f32, a3: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        v2
    }
}

/// Chops its input into a rhythm, in time with the patch.
///
/// # Ports
///
/// **Inputs:** In L, In R (normalled from L), Reset (Gate: starts the row
/// over from step 1), Depth (Control: adds to the Depth knob).
///
/// **Outputs:** Out L, Out R, Gate (high while a slice is open), Env (the
/// slices' level, 0 to 1, for a filter or a VCA elsewhere to follow).
pub struct Slicer {
    sample_rate: f32,
    /// The beat kept without a playing transport.
    free_beat: f64,
    /// The beat at the last sample, to notice the transport starting over.
    last_beat: f64,
    /// Whether the last sample's beat was the transport's.
    on_transport: bool,
    /// The beat step 1 falls on: where Reset last fired.
    origin: f64,
    /// The Reset input, for its rising edges.
    reset: Edge,
    /// The step that played last, to notice the next starting.
    last_step: Option<i64>,
    /// The slices' level on each side, following the step's.
    env: [f32; 2],
    /// How open the filter is, 1 as a slice strikes, closing through it.
    bright: f32,
    /// The level of the slice that struck last, which sets how bright.
    struck_level: f32,
    /// Ping-Pong: whether the slice playing is on the right, and whether
    /// the next will be.
    right_side: bool,
    next_right: bool,
    /// The Gate drops for a sample as a slice strikes, so one struck while
    /// the last is still open starts with a rising edge.
    gate_dip: bool,
    gate_high: bool,
    filter: [Lowpass; 2],
    depth: SmoothedValue,
    mix: SmoothedValue,
    filter_amount: SmoothedValue,
    /// For the face: where the row is, the levels, the filter, the input.
    shown_place: Option<Place>,
    shown_cutoff: f32,
    input_peak: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Slicer {
    pub const PORT_IN_L: usize = 0;
    pub const PORT_IN_R: usize = 1;
    pub const PORT_RESET: usize = 2;
    pub const PORT_DEPTH: usize = 3;
    pub const OUT_L: usize = 0;
    pub const OUT_R: usize = 1;
    pub const OUT_GATE: usize = 2;
    pub const OUT_ENV: usize = 3;

    // Depth first, as its jack sits with the others; then the menu and the
    // toggle, so no knob-only parameter comes between jacks
    pub const PARAM_DEPTH: usize = 0;
    pub const PARAM_RATE: usize = 1;
    pub const PARAM_PING_PONG: usize = 2;
    pub const PARAM_STEPS: usize = 3;
    pub const PARAM_DUTY: usize = 4;
    pub const PARAM_SHAPE: usize = 5;
    pub const PARAM_FILTER: usize = 6;
    pub const PARAM_SWING: usize = 7;
    pub const PARAM_MIX: usize = 8;
    pub const PARAM_STEP: usize = 9;
    pub const PARAM_COUNT: usize = Self::PARAM_STEP + STEPS;

    /// The step playing, from 0 (-1 before the row has started).
    pub const READOUT_STEP: usize = 0;
    /// How far through the step, 0 to 1.
    pub const READOUT_ALONG: usize = 1;
    /// The level each side is open at, 0 to 1.
    pub const READOUT_LEFT: usize = 2;
    pub const READOUT_RIGHT: usize = 3;
    /// How closed the filter is, 0 (wide open) to 1 (as low as it goes).
    pub const READOUT_DARK: usize = 4;
    /// The input's peak over the last block.
    pub const READOUT_INPUT: usize = 5;

    /// The parameter of step `index` (from 0).
    pub const fn step_param(index: usize) -> usize {
        Self::PARAM_STEP + index
    }

    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let default = pattern_steps(PATTERNS[DEFAULT_PATTERN].1);
        let mut parameters = vec![
            ParameterDefinition::normalized("depth", "Depth", 1.0)
                .describe("How deep the gaps cut: silence at 1, untouched at 0, where only the Filter steps"),
            ParameterDefinition::choice("rate", "Rate", RATES, 1)
                .describe("How long each step is, in time with the Clock: 1/16 for sixteenths"),
            ParameterDefinition::toggle("ping_pong", "Ping-Pong", false)
                .describe("Sends alternate slices left and right"),
            ParameterDefinition::new("steps", "Steps", 1.0, STEPS as f32, 16.0, ParameterDisplay::stepped("steps"))
                .describe("How many steps the row plays before it starts over: 12 against a bar of 16 drifts across it"),
            ParameterDefinition::new("duty", "Duty", 5.0, 100.0, 55.0, ParameterDisplay::linear("%"))
                .describe("How much of its step a slice stays open: short and choppy low, legato at 100"),
            ParameterDefinition::normalized("shape", "Shape", 0.1)
                .describe("How each slice opens and closes: crisp at 0, swelling in and out at 1"),
            ParameterDefinition::normalized("filter", "Filter", 0.0)
                .describe("Each slice opens a resonant lowpass that closes through its step, brighter the harder the step"),
            ParameterDefinition::new("swing", "Swing", 50.0, 75.0, 50.0, ParameterDisplay::linear("%"))
                .describe("Pushes every second step late: 50 is straight, 66 a triplet shuffle"),
            ParameterDefinition::normalized("mix", "Mix", 1.0)
                .describe("Blend from the input untouched (0) to sliced (1)"),
        ];
        for (index, step) in default.iter().enumerate() {
            parameters.push(
                ParameterDefinition::new(STEP_IDS[index], STEP_NAMES[index], 0.0, 4.0, step.encode(), ParameterDisplay::stepped(""))
                    .describe("One step: 0 rest, 1 ghost, 2 hit, 3 accent, 4 tie"),
            );
        }
        debug_assert_eq!(parameters.len(), Self::PARAM_COUNT);

        Self {
            sample_rate,
            free_beat: 0.0,
            last_beat: 0.0,
            on_transport: false,
            origin: 0.0,
            reset: Edge::LOW,
            last_step: None,
            env: [0.0; 2],
            bright: 0.0,
            struck_level: 0.0,
            right_side: false,
            next_right: false,
            gate_dip: false,
            gate_high: false,
            filter: [Lowpass::default(); 2],
            depth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            mix: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            filter_amount: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            shown_place: None,
            shown_cutoff: FILTER_TOP_HZ,
            input_peak: 0.0,
            ports: vec![
                PortDefinition::input_with_default("in_l", "In L", SignalType::Audio, 0.0)
                    .describe("Left audio to slice: a held chord, a pad, a drone, a live input"),
                PortDefinition::input_with_default("in_r", "In R", SignalType::Audio, 0.0)
                    .describe("Right audio to slice; copies left when unpatched"),
                PortDefinition::input_with_default("reset", "Reset", SignalType::Gate, 0.0)
                    .describe("A rising edge starts the row over from step 1, on that beat"),
                PortDefinition::input_with_default("depth", "Depth", SignalType::Control, 0.0)
                    .describe("CV that adds to Depth: an Arranger lane here brings the slicing in and out"),
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("Left, sliced"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("Right, sliced"),
                PortDefinition::output("gate", "Gate", SignalType::Gate)
                    .describe("High while a slice is open, so an envelope or a drum can follow the chop"),
                PortDefinition::output("env", "Env", SignalType::Control)
                    .describe("The slices' level, 0 to 1, shaped as they are: patch it to a filter's cutoff to make it breathe with them"),
            ],
            parameters,
        }
    }

    /// Beats in a step, for a Rate value.
    pub fn rate_beats(value: f32) -> f64 {
        RATE_BEATS[(value.round().max(0.0) as usize).min(RATE_BEATS.len() - 1)]
    }

    /// How many steps play, for a Steps value.
    pub fn step_count(value: f32) -> usize {
        (value.round() as usize).clamp(1, STEPS)
    }

    /// Where the row is `steps` steps from its start, with the second step
    /// of each pair pushed late by `swing` (0.5 straight, to 0.75).
    pub fn place(steps: f64, swing: f64) -> Place {
        let split = 2.0 * swing.clamp(0.5, 0.75);
        let pair = (steps / 2.0).floor();
        let within = steps - 2.0 * pair;
        if within < split {
            Place { step: 2 * pair as i64, along: within / split, length: split }
        } else {
            Place { step: 2 * pair as i64 + 1, along: (within - split) / (2.0 - split), length: 2.0 - split }
        }
    }

    /// Step `n` of a row of `count` steps.
    fn slice_at(params: &[f32], n: i64, count: usize) -> Slice {
        Slice::decode(params[Self::step_param(n.rem_euclid(count as i64) as usize)])
    }

    /// The level step `n` plays at: its own, or for a tie, the level of the
    /// slice it holds open (0 if that's a rest). A row of nothing but ties
    /// is held open at a hit's level.
    pub fn run_level(params: &[f32], n: i64, count: usize) -> f32 {
        for back in 0..count as i64 {
            let slice = Self::slice_at(params, n - back, count);
            if slice != Slice::Tie {
                return slice.level();
            }
        }
        Slice::Hit.level()
    }

    /// One-pole coefficient for a time constant.
    #[inline]
    fn coefficient(seconds: f32, sample_rate: f32) -> f32 {
        1.0 - (-1.0 / (seconds.max(1e-5) * sample_rate)).exp()
    }
}

impl Default for Slicer {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Slicer {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Slicer",
            category: ModuleCategory::Effect,
            description: "Chops its input into a rhythm in time with the Clock, like a slicer pedal: sixteen steps, a step filter and ping-pong",
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
        self.depth.set_sample_rate(sample_rate);
        self.mix.set_sample_rate(sample_rate);
        self.filter_amount.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [out_l, out_r, out_gate, out_env, ..] = outputs else {
            return;
        };
        let in_l = connected_input(inputs, Self::PORT_IN_L);
        let in_r = connected_input(inputs, Self::PORT_IN_R);
        let reset = connected_input(inputs, Self::PORT_RESET);
        let depth_cv = connected_input(inputs, Self::PORT_DEPTH);
        let read = |buffer: Option<&SignalBuffer>, i: usize| buffer.map_or(0.0, |b| b.at(i));

        let step_beats = Self::rate_beats(params[Self::PARAM_RATE]);
        let count = Self::step_count(params[Self::PARAM_STEPS]);
        let ping_pong = params[Self::PARAM_PING_PONG] > 0.5;
        let duty = (params[Self::PARAM_DUTY] / 100.0).clamp(0.01, 1.0) as f64;
        let shape = params[Self::PARAM_SHAPE].clamp(0.0, 1.0);
        let swing = (params[Self::PARAM_SWING] / 100.0) as f64;
        self.mix.set_target(params[Self::PARAM_MIX].clamp(0.0, 1.0));
        self.filter_amount.set_target(params[Self::PARAM_FILTER].clamp(0.0, 1.0));

        let transport = context.transport;
        let tempo = transport.tempo_bpm.filter(|&bpm| bpm > 0.0).unwrap_or(120.0) as f64;
        let beats_per_sample = tempo / 60.0 / self.sample_rate as f64;
        let sr = self.sample_rate;
        let k = 1.0 / FILTER_Q;
        let swell = shape * shape;
        self.input_peak = 0.0;

        for i in 0..context.block_size {
            // The beat: the transport's while it plays, our own otherwise
            let beat = match transport.beat_position.filter(|_| transport.playing) {
                Some(_) => {
                    let beat = transport.beat_at(i, sr).unwrap_or(0.0);
                    if !self.on_transport || beat < self.last_beat - RESTART_BEATS {
                        // The transport started (over): the row starts with it
                        self.origin = 0.0;
                        self.next_right = false;
                    }
                    self.on_transport = true;
                    beat
                }
                None => {
                    if self.on_transport {
                        // Carry on from where the transport stopped
                        self.free_beat = self.last_beat;
                        self.on_transport = false;
                    }
                    let beat = self.free_beat;
                    self.free_beat += beats_per_sample;
                    beat
                }
            };
            self.last_beat = beat;

            if self.reset.rising(read(reset, i)) {
                self.origin = beat;
                self.last_step = None;
                self.next_right = false;
            }

            let place = Self::place(((beat - self.origin) / step_beats).max(0.0), swing);
            let slice = Self::slice_at(params, place.step, count);
            let level = Self::run_level(params, place.step, count);
            // A tie after it holds it open to the end of its step
            let held_on = Self::slice_at(params, place.step + 1, count) == Slice::Tie;
            let open = level > 0.0 && (held_on || place.along < duty);

            if self.last_step != Some(place.step) {
                self.last_step = Some(place.step);
                if place.step.rem_euclid(count as i64) == 0 {
                    // Every pass starts on the left, so a row plays the same each time round
                    self.next_right = false;
                }
                if slice != Slice::Tie && level > 0.0 {
                    // A slice strikes
                    self.bright = 1.0;
                    self.struck_level = level;
                    self.gate_dip = true;
                    self.right_side = ping_pong && self.next_right;
                    self.next_right = ping_pong && !self.next_right;
                }
            }
            if !ping_pong {
                self.right_side = false;
                self.next_right = false;
            }

            // How it opens and closes: crisp, or swelling over the step
            let step_seconds = (place.length * step_beats / beats_per_sample) as f32 / sr;
            let attack = Self::coefficient(CRISP_ATTACK_SECONDS + swell * SWELL_ATTACK * step_seconds, sr);
            let release = Self::coefficient(CRISP_RELEASE_SECONDS + swell * SWELL_RELEASE * step_seconds, sr);
            let sides = [!ping_pong || !self.right_side, !ping_pong || self.right_side];
            for (env, side_open) in self.env.iter_mut().zip(sides) {
                let target = if open && side_open { level } else { 0.0 };
                let rate = if target > *env { attack } else { release };
                *env = flush(*env + rate * (target - *env));
            }

            // The step filter closes through the slice, from as bright as it struck
            self.bright = flush(self.bright * (1.0 - Self::coefficient(FILTER_CLOSE * step_seconds, sr)));
            let amount = self.filter_amount.next();
            let opening = self.struck_level * (0.25 + 0.75 * self.bright);
            let cutoff = FILTER_TOP_HZ * (-amount * FILTER_OCTAVES * (1.0 - opening)).exp2();
            let g = prewarp(cutoff, sr);
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;
            // Faded in from the bottom of the knob, so turning it up never clicks
            let filtered = (amount * 8.0).min(1.0);

            self.depth.set_target(params[Self::PARAM_DEPTH] + read(depth_cv, i));
            let depth = self.depth.next().clamp(0.0, 1.0);
            let mix = self.mix.next();
            let dry_l = read(in_l, i);
            let dry_r = if in_r.is_some() { read(in_r, i) } else { dry_l };
            self.input_peak = self.input_peak.max(dry_l.abs()).max(dry_r.abs());

            let mut out = [0.0; 2];
            for (side, dry) in [dry_l, dry_r].into_iter().enumerate() {
                let lp = self.filter[side].process(dry, a1, a2, a3);
                let toned = dry + filtered * (FILTER_TRIM * lp - dry);
                let gain = 1.0 - depth + depth * self.env[side];
                out[side] = dry + mix * (toned * gain - dry);
            }
            out_l.samples[i] = out[0];
            out_r.samples[i] = out[1];

            let env = self.env[0].max(self.env[1]);
            out_env.samples[i] = env;
            // A strike while the Gate is high drops it for a sample
            let gate = open && !(self.gate_dip && self.gate_high);
            out_gate.samples[i] = if gate { 1.0 } else { 0.0 };
            self.gate_high = gate;
            self.gate_dip = false;

            if i + 1 == context.block_size {
                self.shown_place = Some(place);
                self.shown_cutoff = cutoff;
            }
        }
    }

    fn reset(&mut self) {
        self.free_beat = 0.0;
        self.last_beat = 0.0;
        self.on_transport = false;
        self.origin = 0.0;
        self.last_step = None;
        self.env = [0.0; 2];
        self.bright = 0.0;
        self.right_side = false;
        self.next_right = false;
        self.gate_high = false;
        self.filter = [Lowpass::default(); 2];
        self.shown_place = None;
    }

    fn readout(&self, params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let count = Self::step_count(params[Self::PARAM_STEPS]) as i64;
        let (step, along) = self.shown_place.map_or((-1.0, 0.0), |p| (p.step.rem_euclid(count) as f32, p.along as f32));
        readout.values[Self::READOUT_STEP] = step;
        readout.values[Self::READOUT_ALONG] = along;
        readout.values[Self::READOUT_LEFT] = self.env[0];
        readout.values[Self::READOUT_RIGHT] = self.env[1];
        readout.values[Self::READOUT_DARK] = ((FILTER_TOP_HZ / self.shown_cutoff.max(1.0)).log2() / FILTER_OCTAVES).clamp(0.0, 1.0);
        readout.values[Self::READOUT_INPUT] = self.input_peak;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{analysis::rms, context::TransportState};

    const SR: f32 = 48000.0;
    const BLOCK: usize = 480;
    /// A sixteenth at 120 BPM.
    const SIXTEENTH: usize = 6000;

    /// The defaults, with `changes`.
    fn params(changes: &[(usize, f32)]) -> Vec<f32> {
        let mut params: Vec<f32> = Slicer::new().parameters().iter().map(|p| p.default).collect();
        for &(index, value) in changes {
            params[index] = value;
        }
        params
    }

    /// The defaults playing `pattern`, with `changes`.
    fn playing(pattern: &str, changes: &[(usize, f32)]) -> Vec<f32> {
        let mut p = params(changes);
        for (index, step) in pattern_steps(pattern).iter().enumerate() {
            p[Slicer::step_param(index)] = step.encode();
        }
        p
    }

    struct Played {
        left: Vec<f32>,
        right: Vec<f32>,
        gate: Vec<f32>,
        env: Vec<f32>,
    }

    /// Runs `input(i)` (mono) through a new Slicer for `frames` samples,
    /// at 120 BPM: on a playing transport from beat `from`, or with none.
    /// Reset rises at each of `resets`.
    fn play(params: &[f32], input: impl Fn(usize) -> f32, frames: usize, from: Option<f64>, resets: &[usize]) -> Played {
        let mut slicer = Slicer::new();
        slicer.prepare(SR, BLOCK);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Audio);
        let no_cv = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK), SignalBuffer::gate(BLOCK), SignalBuffer::control(BLOCK)];
        let mut played = Played { left: vec![], right: vec![], gate: vec![], env: vec![] };
        for start in (0..frames).step_by(BLOCK) {
            let mut audio = SignalBuffer::audio(BLOCK);
            let mut reset = SignalBuffer::gate(BLOCK);
            for i in 0..BLOCK {
                audio.samples[i] = input(start + i);
                reset.samples[i] = if resets.iter().any(|&r| (r..r + 100).contains(&(start + i))) { 1.0 } else { 0.0 };
            }
            let transport = match from {
                Some(beat) => TransportState { beat_position: Some(beat + start as f64 * 2.0 / SR as f64), ..TransportState::playing_at(120.0) },
                None => TransportState::new(),
            };
            let ctx = ProcessContext::with_transport(SR, BLOCK, transport);
            slicer.process(&[&audio, &unpatched, &reset, &no_cv], &mut outputs, params, &ctx);
            played.left.extend_from_slice(&outputs[0].samples);
            played.right.extend_from_slice(&outputs[1].samples);
            played.gate.extend_from_slice(&outputs[2].samples);
            played.env.extend_from_slice(&outputs[3].samples);
        }
        played
    }

    fn sine(i: usize) -> f32 {
        (std::f32::consts::TAU * 220.0 * i as f32 / SR).sin() * 0.5
    }

    /// The level of the sine, whole.
    fn whole() -> f32 {
        rms(&(0..2400).map(sine).collect::<Vec<_>>())
    }

    /// The RMS over the middle of sixteenth `n`, away from its edges.
    fn middle(signal: &[f32], n: usize) -> f32 {
        rms(&signal[n * SIXTEENTH + 600..n * SIXTEENTH + 3000])
    }

    /// The RMS over the end of sixteenth `n`, past a 55% duty.
    fn end(signal: &[f32], n: usize) -> f32 {
        rms(&signal[n * SIXTEENTH + 4500..n * SIXTEENTH + 5900])
    }

    fn rises(gate: &[f32]) -> Vec<usize> {
        (1..gate.len()).filter(|&i| gate[i] > 0.5 && gate[i - 1] <= 0.5).chain((gate[0] > 0.5).then_some(0)).collect::<std::collections::BTreeSet<_>>().into_iter().collect()
    }

    #[test]
    fn test_ports_and_parameters() {
        let slicer = Slicer::new();
        assert_eq!(slicer.info().id, ID);
        assert_eq!(slicer.info().category, ModuleCategory::Effect);
        assert_eq!(slicer.parameters().len(), Slicer::PARAM_COUNT);
        let ports = slicer.ports();
        assert_eq!(ports[Slicer::PORT_DEPTH].name, slicer.parameters()[Slicer::PARAM_DEPTH].name, "Depth's jack pairs with its knob");
        assert_eq!(ports.iter().filter(|p| p.is_output()).count(), 4);
        // The default row is the first pattern
        let p = params(&[]);
        assert_eq!(pattern_steps(PATTERNS[DEFAULT_PATTERN].1).map(Slice::encode).as_slice(), &p[Slicer::PARAM_STEP..]);
    }

    #[test]
    fn test_steps_round_trip_and_every_pattern_is_written_out() {
        for slice in Slice::ALL {
            assert_eq!(Slice::decode(slice.encode()), slice);
            assert_eq!(Slice::from_symbol(slice.symbol()), Some(slice));
        }
        for (name, symbols, count, _) in PATTERNS {
            assert_eq!(symbols.chars().count(), STEPS, "{name}");
            assert!(symbols.chars().all(|c| Slice::from_symbol(c).is_some()), "{name}");
            assert!((1..=STEPS).contains(count), "{name}");
            assert_ne!(symbols.chars().next(), Some('='), "{name} starts with a tie");
        }
        // Each is its own row and its own name, so the face can name it
        for (i, (name, symbols, count, _)) in PATTERNS.iter().enumerate() {
            for (other, theirs, their_count, _) in &PATTERNS[..i] {
                assert_ne!(name, other);
                assert!(count != their_count || pattern_steps(symbols)[..*count] != pattern_steps(theirs)[..*count], "{name} is {other}");
            }
        }
        // Each family's patterns sit together, so the menu reads in table order
        let families: Vec<_> = pattern_families().collect();
        assert_eq!(families, ["Steady", "Groove", "Clave and kin", "Across the bar"]);
        let order: Vec<_> = PATTERNS.iter().map(|p| families.iter().position(|&f| f == p.3).unwrap()).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn test_rests_cut_and_hits_pass_in_time_with_the_transport() {
        let p = playing("X-x-------------", &[]);
        let played = play(&p, sine, 4 * SIXTEENTH, Some(0.0), &[]);
        assert!((middle(&played.left, 0) - whole()).abs() < 0.02, "accent passes whole: {}", middle(&played.left, 0));
        assert!(middle(&played.left, 1) < 1e-3, "a rest is silent: {}", middle(&played.left, 1));
        assert!((middle(&played.left, 2) - 0.8 * whole()).abs() < 0.02, "a hit at 80%: {}", middle(&played.left, 2));
        assert_eq!(rises(&played.gate), vec![0, 2 * SIXTEENTH]);
        // Duty closes each slice partway through its step
        assert!(end(&played.left, 0) < 1e-3, "open past its duty: {}", end(&played.left, 0));
        // Right copies left when unpatched
        assert_eq!(played.left, played.right);
    }

    #[test]
    fn test_it_follows_the_transport_from_where_it_is() {
        // The transport a beat in: the row is at step 5
        let p = playing("----X-----------", &[]);
        let played = play(&p, sine, SIXTEENTH, Some(1.0), &[]);
        assert_eq!(rises(&played.gate), vec![0]);
        let played = play(&p, sine, SIXTEENTH, Some(0.0), &[]);
        assert!(rises(&played.gate).is_empty());
    }

    #[test]
    fn test_without_a_transport_it_keeps_its_own_beat() {
        let p = playing("X---X---X---X---", &[]);
        let played = play(&p, sine, 16 * SIXTEENTH, None, &[]);
        assert_eq!(rises(&played.gate), vec![0, 4 * SIXTEENTH, 8 * SIXTEENTH, 12 * SIXTEENTH]);
    }

    #[test]
    fn test_duty_sets_how_long_a_slice_stays_open() {
        for duty in [25.0, 55.0, 90.0] {
            let p = playing("x---------------", &[(Slicer::PARAM_DUTY, duty)]);
            let played = play(&p, sine, SIXTEENTH, Some(0.0), &[]);
            let open = played.gate.iter().filter(|&&g| g > 0.5).count() as f32 / SIXTEENTH as f32;
            assert!((open - duty / 100.0).abs() < 0.01, "duty {duty}: open {open}");
        }
    }

    #[test]
    fn test_a_tie_holds_the_slice_open_without_striking_again() {
        let p = playing("X===x-----------", &[]);
        let played = play(&p, sine, 6 * SIXTEENTH, Some(0.0), &[]);
        // One slice across four steps: open to the end of the third, at the accent's level
        for n in 0..4 {
            assert!((middle(&played.left, n) - whole()).abs() < 0.02, "step {n}: {}", middle(&played.left, n));
        }
        assert!((end(&played.left, 2) - whole()).abs() < 0.02, "held through: {}", end(&played.left, 2));
        assert!(end(&played.left, 3) < 1e-3, "the last tie closes at its duty: {}", end(&played.left, 3));
        assert!(played.env[3 * SIXTEENTH + 3000] > 0.99, "the tie keeps the accent's level");
        // The hit after it strikes anew
        assert_eq!(rises(&played.gate), vec![0, 4 * SIXTEENTH]);
    }

    #[test]
    fn test_a_tie_after_a_rest_stays_shut() {
        let p = playing("x-==------------", &[]);
        let played = play(&p, sine, 4 * SIXTEENTH, Some(0.0), &[]);
        for n in 1..4 {
            assert!(middle(&played.left, n) < 1e-3, "step {n}");
        }
    }

    #[test]
    fn test_back_to_back_slices_at_full_duty_still_strike_each_step() {
        let p = playing("xxxx------------", &[(Slicer::PARAM_DUTY, 100.0)]);
        let played = play(&p, sine, 5 * SIXTEENTH, Some(0.0), &[]);
        assert_eq!(rises(&played.gate), vec![0, SIXTEENTH + 1, 2 * SIXTEENTH + 1, 3 * SIXTEENTH + 1]);
    }

    #[test]
    fn test_steps_shortens_the_row() {
        let p = playing("X--x------------", &[(Slicer::PARAM_STEPS, 4.0)]);
        let played = play(&p, sine, 8 * SIXTEENTH, Some(0.0), &[]);
        assert_eq!(rises(&played.gate), (0..8).filter(|n| n % 4 == 0 || n % 4 == 3).map(|n| n * SIXTEENTH).collect::<Vec<_>>());
    }

    #[test]
    fn test_rate_sets_the_step_length() {
        let p = playing("xxxx------------", &[(Slicer::PARAM_RATE, 0.0)]);
        let played = play(&p, sine, 8 * SIXTEENTH, Some(0.0), &[]);
        assert_eq!(rises(&played.gate), vec![0, 2 * SIXTEENTH, 4 * SIXTEENTH, 6 * SIXTEENTH], "eighths");
    }

    #[test]
    fn test_swing_pushes_every_second_step_late() {
        let p = playing("xxxx------------", &[(Slicer::PARAM_SWING, 66.0)]);
        let played = play(&p, sine, 4 * SIXTEENTH, Some(0.0), &[]);
        let late = (0.32 * SIXTEENTH as f32) as usize;
        let found = rises(&played.gate);
        assert_eq!(found.len(), 4, "{found:?}");
        assert_eq!(found[0], 0);
        assert!(found[1].abs_diff(SIXTEENTH + late) <= 2, "{found:?}");
        assert_eq!(found[2], 2 * SIXTEENTH);
        assert!(found[3].abs_diff(3 * SIXTEENTH + late) <= 2, "{found:?}");
    }

    #[test]
    fn test_depth_sets_how_deep_the_gaps_cut() {
        for depth in [0.0, 0.5, 1.0] {
            let p = playing("X---------------", &[(Slicer::PARAM_DEPTH, depth)]);
            let played = play(&p, sine, 2 * SIXTEENTH, Some(0.0), &[]);
            let gap = middle(&played.left, 1);
            assert!((gap - (1.0 - depth) * whole()).abs() < 0.01, "depth {depth}: gap {gap}");
        }
        // At 0, and no Filter, it passes the input untouched
        let p = playing("X-x.X-x.X-x.X-x.", &[(Slicer::PARAM_DEPTH, 0.0)]);
        let played = play(&p, sine, 4 * SIXTEENTH, Some(0.0), &[]);
        let worst = (0..played.left.len()).map(|i| (played.left[i] - sine(i)).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn test_mix_brings_back_the_input() {
        let p = playing("X---------------", &[(Slicer::PARAM_MIX, 0.0)]);
        let played = play(&p, sine, 2 * SIXTEENTH, Some(0.0), &[]);
        let worst = (0..played.left.len()).map(|i| (played.left[i] - sine(i)).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn test_crisp_edges_do_not_click_and_shape_swells_them() {
        let p = playing("x-x-x-x-x-x-x-x-", &[(Slicer::PARAM_SHAPE, 0.0)]);
        let played = play(&p, sine, 8 * SIXTEENTH, Some(0.0), &[]);
        // A 220 Hz sine at 0.5 moves at most 0.015 a sample
        let worst = played.left.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.04, "step of {worst} at an edge");

        // Swelling, it's still rising a fifth of the way into the step
        let p = playing("x-x-x-x-x-x-x-x-", &[(Slicer::PARAM_SHAPE, 1.0)]);
        let played = play(&p, sine, 8 * SIXTEENTH, Some(0.0), &[]);
        assert!(played.env[SIXTEENTH / 5] < 0.6, "{}", played.env[SIXTEENTH / 5]);
        assert!(played.env[SIXTEENTH / 2] > played.env[SIXTEENTH / 5]);
    }

    #[test]
    fn test_ping_pong_sends_alternate_slices_left_and_right() {
        let p = playing("x-x-x-x---------", &[(Slicer::PARAM_PING_PONG, 1.0)]);
        let played = play(&p, sine, 8 * SIXTEENTH, Some(0.0), &[]);
        let sides: Vec<(bool, bool)> = [0, 2, 4, 6].iter().map(|&n| (middle(&played.left, n) > 0.1, middle(&played.right, n) > 0.1)).collect();
        assert_eq!(sides, vec![(true, false), (false, true), (true, false), (false, true)]);
        // The Gate follows every slice, whichever side
        assert_eq!(rises(&played.gate).len(), 4);
    }

    #[test]
    fn test_ping_pong_starts_each_pass_on_the_left() {
        // Three slices a pass: the first of each is on the left
        let p = playing("x-x-x-----------", &[(Slicer::PARAM_PING_PONG, 1.0), (Slicer::PARAM_STEPS, 6.0)]);
        let played = play(&p, sine, 12 * SIXTEENTH, Some(0.0), &[]);
        let sides: Vec<bool> = [0, 2, 4, 6, 8, 10].iter().map(|&n| middle(&played.right, n) > 0.1).collect();
        assert_eq!(sides, vec![false, true, false, false, true, false]);
    }

    #[test]
    fn test_the_filter_closes_through_each_slice_and_steps_alone_at_depth_zero() {
        // Noise through the step filter, uncut: each step starts bright and darkens
        let mut seed = 7u32;
        let noise: Vec<f32> = (0..4 * SIXTEENTH)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as i32 as f32) / i32::MAX as f32 * 0.3
            })
            .collect();
        let p = playing("XxXx------------", &[(Slicer::PARAM_DEPTH, 0.0), (Slicer::PARAM_FILTER, 1.0)]);
        let played = play(&p, |i| noise[i], 4 * SIXTEENTH, Some(0.0), &[]);
        // High-passed by differencing: what's left is the top end
        let top = |from: usize, to: usize| rms(&played.left[from..to].windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
        for n in 0..4 {
            let start = top(n * SIXTEENTH + 50, n * SIXTEENTH + 600);
            let late = top(n * SIXTEENTH + 4000, n * SIXTEENTH + 5900);
            assert!(late < start * 0.5, "step {n}: start {start}, late {late}");
        }
        // An accent strikes brighter than a hit
        assert!(top(50, 600) > top(SIXTEENTH + 50, SIXTEENTH + 600));
        // Nothing passes much louder than it went in, resonance and all
        let peak = played.left.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak < 0.6, "{peak}");
    }

    #[test]
    fn test_reset_starts_the_row_over_on_that_beat() {
        let p = playing("X---------------", &[]);
        let at = 5 * SIXTEENTH + 1234;
        let played = play(&p, sine, 8 * SIXTEENTH, Some(0.0), &[at]);
        assert_eq!(rises(&played.gate), vec![0, at]);
    }

    #[test]
    fn test_place_with_swing() {
        assert_eq!(Slicer::place(2.5, 0.5), Place { step: 2, along: 0.5, length: 1.0 });
        let swung = Slicer::place(1.25, 0.75);
        assert_eq!((swung.step, swung.length), (0, 1.5));
        let off = Slicer::place(1.75, 0.75);
        assert_eq!(off.step, 1);
        assert!((off.along - 0.5).abs() < 1e-9 && (off.length - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_registry_instantiation() {
        let mut registry = crate::dsp::ModuleRegistry::new();
        registry.register::<Slicer>();
        let module = registry.create(ID).unwrap();
        assert_eq!(module.parameters().len(), Slicer::PARAM_COUNT);
    }
}
