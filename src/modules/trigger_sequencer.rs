//! Trigger Sequencer module.
//!
//! A drum machine's sequencer: eight lanes of sixteen steps, each lane with
//! its own Gate and Velocity outputs, and an accent row over them all. Four
//! patterns (A to D) play in the order a Chain gives, so "A A A B" plays a
//! fill every fourth bar, and a Pattern CV can pick them instead.
//!
//! Each step is on or off and carries a velocity, a probability and a
//! ratchet: 2 to 4 hits spread evenly across the step, for rolls.
//!
//! # How a step is stored
//!
//! Every step is one parameter, so patterns save, load, undo and copy with
//! the patch like any knob. Its value reads as digits: `R PPP VVV`, the
//! ratchet, the probability in percent and the velocity in percent, and it's
//! negative while the step is off, so switching a step off and on again
//! keeps what it was. `1100080` is a single hit, always, at 80%;
//! `-3050100` is an off step that would play three hits, half the time, at
//! full velocity. See [`Step`].

use std::sync::LazyLock;

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, GATE_THRESHOLD},
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    ParameterDisplay, Readout, SignalType,
};

use super::sequencer::StepTimer;

/// Lanes, each with its own Gate and Velocity outputs.
pub const LANES: usize = 8;
/// Steps in a pattern.
pub const STEPS: usize = 16;
/// Patterns, A to D.
pub const PATTERNS: usize = 4;
/// Slots in the Chain.
pub const CHAIN_SLOTS: usize = 8;
/// Pattern letters, in pattern order.
pub const PATTERN_NAMES: [&str; PATTERNS] = ["A", "B", "C", "D"];
/// The most hits a ratchet spreads across one step.
pub const MAX_RATCHET: u8 = 4;

/// One step of a lane: whether it plays, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub on: bool,
    /// Velocity in percent, 0 to 100.
    pub velocity: u8,
    /// The chance it plays when its turn comes, in percent, 0 to 100.
    pub probability: u8,
    /// How many hits it spreads across the step, 1 to [`MAX_RATCHET`].
    pub ratchet: u8,
}

impl Step {
    /// A step not yet played: off, and at 80% so an accent has room above it.
    pub const DEFAULT: Step = Step { on: false, velocity: 80, probability: 100, ratchet: 1 };

    /// Reads a step from its parameter value (see the module docs).
    pub fn decode(value: f32) -> Self {
        let digits = value.abs().round() as u32;
        Self {
            on: value > 0.0,
            velocity: (digits % 1000).min(100) as u8,
            probability: ((digits / 1000) % 1000).min(100) as u8,
            ratchet: (digits / 1_000_000).clamp(1, MAX_RATCHET as u32) as u8,
        }
    }

    /// The parameter value that stores this step.
    pub fn encode(self) -> f32 {
        let digits = self.ratchet.clamp(1, MAX_RATCHET) as u32 * 1_000_000
            + self.probability.min(100) as u32 * 1000
            + self.velocity.min(100) as u32;
        if self.on { digits as f32 } else { -(digits as f32) }
    }

    /// The step's velocity, 0 to 1.
    pub fn level(self) -> f32 {
        self.velocity as f32 / 100.0
    }
}

/// The largest value a step's parameter takes, either way.
const STEP_VALUE_LIMIT: f32 = 4_100_100.0;

/// Parameter and port names made up at start-up. They must be `'static`,
/// and there are hundreds, so they're built once and kept.
struct Names {
    step_ids: Vec<&'static str>,
    step_names: Vec<&'static str>,
    accent_ids: Vec<&'static str>,
    accent_names: Vec<&'static str>,
}

fn keep(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

static NAMES: LazyLock<Names> = LazyLock::new(|| {
    let mut names = Names { step_ids: Vec::new(), step_names: Vec::new(), accent_ids: Vec::new(), accent_names: Vec::new() };
    for (pattern, letter) in PATTERN_NAMES.iter().enumerate() {
        for step in 1..=STEPS {
            names.accent_ids.push(keep(format!("accent_{}_{step}", letter.to_lowercase())));
            names.accent_names.push(keep(format!("Accent {letter} {step:02}")));
        }
        for lane in 1..=LANES {
            for step in 1..=STEPS {
                names.step_ids.push(keep(format!("step_{}{lane}_{step}", letter.to_lowercase())));
                names.step_names.push(keep(format!("Step {letter}{lane} {step:02}")));
            }
        }
        debug_assert_eq!(names.step_ids.len(), (pattern + 1) * LANES * STEPS);
    }
    names
});

pub(crate) static CHAIN_IDS: [&str; CHAIN_SLOTS] = ["chain_1", "chain_2", "chain_3", "chain_4", "chain_5", "chain_6", "chain_7", "chain_8"];
pub(crate) static CHAIN_NAMES: [&str; CHAIN_SLOTS] = ["Chain 1", "Chain 2", "Chain 3", "Chain 4", "Chain 5", "Chain 6", "Chain 7", "Chain 8"];
/// What a Chain slot can hold: nothing, or a pattern.
pub const CHAIN_CHOICES: [&str; PATTERNS + 1] = ["–", "A", "B", "C", "D"];
static LENGTH_IDS: [&str; LANES] = ["length_1", "length_2", "length_3", "length_4", "length_5", "length_6", "length_7", "length_8"];
static LENGTH_NAMES: [&str; LANES] = ["Length 1", "Length 2", "Length 3", "Length 4", "Length 5", "Length 6", "Length 7", "Length 8"];
static GATE_IDS: [&str; LANES] = ["gate_1", "gate_2", "gate_3", "gate_4", "gate_5", "gate_6", "gate_7", "gate_8"];
/// Output names, a Gate and a Vel per lane.
pub static GATE_NAMES: [&str; LANES] = ["Gate 1", "Gate 2", "Gate 3", "Gate 4", "Gate 5", "Gate 6", "Gate 7", "Gate 8"];
static VEL_IDS: [&str; LANES] = ["vel_1", "vel_2", "vel_3", "vel_4", "vel_5", "vel_6", "vel_7", "vel_8"];
pub static VEL_NAMES: [&str; LANES] = ["Vel 1", "Vel 2", "Vel 3", "Vel 4", "Vel 5", "Vel 6", "Vel 7", "Vel 8"];

/// The patterns a Chain's slot values play, in order, and how many. Empty
/// slots are skipped; an empty chain plays A. Shared with the Step
/// Sequencer, whose Chain works the same way.
pub fn chain_from(slots: &[f32]) -> ([usize; CHAIN_SLOTS], usize) {
    let mut chain = [0; CHAIN_SLOTS];
    let mut len = 0;
    for &value in slots.iter().take(CHAIN_SLOTS) {
        let choice = value.round() as usize;
        if (1..=PATTERNS).contains(&choice) {
            chain[len] = choice - 1;
            len += 1;
        }
    }
    (chain, len.max(1))
}

/// A lane's playing state.
#[derive(Clone, Copy, Debug, Default)]
struct Lane {
    /// Samples the gate stays high for.
    gate: usize,
    /// The gate drops for this sample, so a hit over a gate still high
    /// starts with a rising edge.
    dip: bool,
    /// The Velocity output, held from one hit to the next.
    velocity: f32,
    /// Ratchet hits still to come in this step.
    ratchets_left: u8,
    /// Samples between ratchet hits.
    spacing: usize,
    /// Samples until the next ratchet hit.
    until_next: usize,
    /// How long each of this step's gates is.
    gate_length: usize,
}

impl Lane {
    /// A hit: the gate opens for `length` samples, dipping first if it was
    /// still high.
    fn strike(&mut self, length: usize) {
        self.dip = self.gate > 0;
        self.gate = length;
    }
}

/// An eight-lane trigger sequencer with four chained patterns.
///
/// # Ports
///
/// **Inputs:** Clock (each rising edge plays the next step), Reset (the next
/// clock plays step 1 of the chain's first pattern), Run (steps advance only
/// while it's high; unpatched, it runs) and Pattern (CV that picks the
/// pattern for each new bar, in four equal zones of 0 to 1, in place of the
/// Chain).
///
/// **Outputs:** Accent (a gate on accented steps), then a Gate and a Vel for
/// each lane. Vel holds the last hit's velocity, raised on accented steps.
///
/// # Timing
///
/// A bar is **Steps** clocks long; the pattern changes, and the Chain moves
/// on, only where a bar starts. A lane with its own **Length** loops that
/// many steps whatever the bar does, so lanes of different lengths drift
/// against each other and meet again later (polymeter); a lane left at
/// "Bar" follows the bar. All of them count from the same clock, so nothing
/// drifts out of time.
pub struct TriggerSequencer {
    /// The next clock plays the first step (at the start, and after a reset).
    pending: bool,
    /// The clock input, for its rising edges.
    clock: Edge,
    /// The reset input, for its rising edges.
    reset: Edge,
    timer: StepTimer,
    /// Clocks since the first step, which lanes with their own Length count.
    count: u64,
    /// Where the bar is, 0 to Steps − 1.
    bar_step: usize,
    /// Which Chain entry is playing.
    chain_slot: usize,
    /// The pattern playing.
    pattern: usize,
    /// The pattern the next bar will play, for the display.
    next_pattern: usize,
    /// Pattern was patched in the last block.
    pattern_cv: bool,
    lanes: [Lane; LANES],
    accent: Lane,
    random_state: u32,
    sample_rate: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl TriggerSequencer {
    pub const PORT_CLOCK: usize = 0;
    pub const PORT_RESET: usize = 1;
    pub const PORT_RUN: usize = 2;
    pub const PORT_PATTERN: usize = 3;
    /// Output index of Accent. Lane `l`'s Gate is `1 + 2l`, its Vel `2 + 2l`.
    pub const OUT_ACCENT: usize = 0;

    pub const PARAM_STEPS: usize = 0;
    pub const PARAM_GATE_LENGTH: usize = 1;
    pub const PARAM_ACCENT_AMOUNT: usize = 2;
    pub const PARAM_CHAIN: usize = 3;
    pub const PARAM_LENGTH: usize = Self::PARAM_CHAIN + CHAIN_SLOTS;
    pub const PARAM_ACCENTS: usize = Self::PARAM_LENGTH + LANES;
    pub const PARAM_GRID: usize = Self::PARAM_ACCENTS + PATTERNS * STEPS;
    pub const PARAM_COUNT: usize = Self::PARAM_GRID + PATTERNS * LANES * STEPS;

    /// The parameter of a step (all 0-based).
    pub const fn step_param(pattern: usize, lane: usize, step: usize) -> usize {
        Self::PARAM_GRID + (pattern * LANES + lane) * STEPS + step
    }

    /// The parameter of a step on the accent row.
    pub const fn accent_param(pattern: usize, step: usize) -> usize {
        Self::PARAM_ACCENTS + pattern * STEPS + step
    }

    /// Readout values: lanes 1–4's steps, 4 bits each.
    pub const READOUT_LANES_LOW: usize = 0;
    /// Lanes 5–8's steps.
    pub const READOUT_LANES_HIGH: usize = 1;
    /// The pattern, + 4 × the Chain slot, + 32 once a step has played.
    pub const READOUT_PATTERN: usize = 2;
    /// The next bar's pattern, + 4 × the bar step, + 64 while Pattern is patched.
    pub const READOUT_BAR: usize = 3;

    pub fn new() -> Self {
        let mut ports = vec![
            PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0)
                .describe("Each rising edge plays the next step; patch a Clock gate here"),
            PortDefinition::input_with_default("reset", "Reset", SignalType::Gate, 0.0)
                .describe("A rising edge starts over: the next clock plays step 1 of the chain's first pattern"),
            PortDefinition::input_with_default("run", "Run", SignalType::Gate, 1.0)
                .describe("Steps only advance while high; runs when unpatched"),
            PortDefinition::input_with_default("pattern", "Pattern", SignalType::Control, 0.0)
                .describe("Picks the pattern for each new bar in place of the Chain: 0 to 0.25 is A, then B, C and D"),
            PortDefinition::output("accent", "Accent", SignalType::Gate)
                .describe("A gate on every accented step, whichever lanes play"),
        ];
        for lane in 0..LANES {
            ports.push(
                PortDefinition::output(GATE_IDS[lane], GATE_NAMES[lane], SignalType::Gate)
                    .describe("This lane's hits; patch into a Drum's Trig or an envelope"),
            );
            ports.push(
                PortDefinition::output(VEL_IDS[lane], VEL_NAMES[lane], SignalType::Control)
                    .describe("The velocity of this lane's last hit, 0 to 1, raised on accented steps; patch into a Drum's Accent"),
            );
        }

        let mut parameters = vec![
            ParameterDefinition::new("steps", "Steps", 1.0, STEPS as f32, STEPS as f32, ParameterDisplay::stepped(""))
                .describe("How many steps make a bar: the pattern changes, and the chain moves on, where a bar starts"),
            ParameterDefinition::new("gate_length", "Gate Length", 1.0, 100.0, 50.0, ParameterDisplay::linear("%"))
                .describe("How long each gate stays high, as a share of its step (or of its ratchet hit)"),
            ParameterDefinition::new("accent_amount", "Accent Amount", 0.0, 100.0, 50.0, ParameterDisplay::linear("%"))
                .describe("How far an accented step raises every lane's velocity toward full"),
        ];
        for slot in 0..CHAIN_SLOTS {
            parameters.push(
                ParameterDefinition::choice(CHAIN_IDS[slot], CHAIN_NAMES[slot], &CHAIN_CHOICES, usize::from(slot == 0))
                    .describe("A pattern in the chain, which plays its patterns a bar each, in order, then repeats"),
            );
        }
        for lane in 0..LANES {
            parameters.push(
                ParameterDefinition::new(LENGTH_IDS[lane], LENGTH_NAMES[lane], 0.0, STEPS as f32, 0.0, ParameterDisplay::stepped(""))
                    .describe("How many steps this lane loops before starting over; 0 follows the bar"),
            );
        }
        let names = &*NAMES;
        for index in 0..PATTERNS * STEPS {
            parameters.push(
                ParameterDefinition::toggle(names.accent_ids[index], names.accent_names[index], false)
                    .describe("Accents this step: every lane that plays on it plays louder"),
            );
        }
        for index in 0..PATTERNS * LANES * STEPS {
            parameters.push(
                ParameterDefinition::new(
                    names.step_ids[index],
                    names.step_names[index],
                    -STEP_VALUE_LIMIT,
                    STEP_VALUE_LIMIT,
                    Step::DEFAULT.encode(),
                    ParameterDisplay::stepped(""),
                )
                .describe("One step: ratchet, probability % and velocity % as digits, negative while off"),
            );
        }
        debug_assert_eq!(parameters.len(), Self::PARAM_COUNT);

        Self {
            pending: true,
            clock: Edge::LOW,
            reset: Edge::LOW,
            timer: StepTimer::new(),
            count: 0,
            bar_step: 0,
            chain_slot: 0,
            pattern: 0,
            next_pattern: 0,
            pattern_cv: false,
            lanes: [Lane::default(); LANES],
            accent: Lane::default(),
            random_state: Self::SEED,
            sample_rate: 44100.0,
            ports,
            parameters,
        }
    }

    const SEED: u32 = 0x9E37_79B9;

    /// The patterns the Chain plays, in order, and how many. Empty slots are
    /// skipped; an empty chain plays A.
    pub fn chain(params: &[f32]) -> ([usize; CHAIN_SLOTS], usize) {
        chain_from(&params[Self::PARAM_CHAIN..Self::PARAM_CHAIN + CHAIN_SLOTS])
    }

    /// The pattern a Pattern CV picks.
    pub fn pattern_for_cv(cv: f32) -> usize {
        ((cv * PATTERNS as f32).floor().max(0.0) as usize).min(PATTERNS - 1)
    }

    /// A lane's Length: its own, or 0 to follow the bar.
    fn lane_length(params: &[f32], lane: usize) -> usize {
        (params[Self::PARAM_LENGTH + lane].round().max(0.0) as usize).min(STEPS)
    }

    /// The step a lane is on.
    fn lane_step(&self, params: &[f32], lane: usize) -> usize {
        match Self::lane_length(params, lane) {
            0 => self.bar_step,
            length => (self.count % length as u64) as usize,
        }
    }

    fn next_random(&mut self) -> u32 {
        let mut x = self.random_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random_state = x;
        x
    }

    /// Whether a step with this probability plays this time.
    fn plays(&mut self, probability: u8) -> bool {
        probability >= 100 || (probability > 0 && self.next_random() % 100 < probability as u32)
    }

    /// The pattern for the Chain slot given, or for the Pattern CV.
    fn pick(&mut self, params: &[f32], cv: Option<f32>) -> usize {
        match cv {
            Some(cv) => Self::pattern_for_cv(cv),
            None => {
                let (chain, len) = Self::chain(params);
                self.chain_slot %= len;
                chain[self.chain_slot]
            }
        }
    }

    /// Moves to the next step, or to the first after a reset.
    fn advance(&mut self, params: &[f32], cv: Option<f32>) {
        let steps = (params[Self::PARAM_STEPS].round() as usize).clamp(1, STEPS);
        if self.pending {
            self.pending = false;
            self.count = 0;
            self.bar_step = 0;
            self.chain_slot = 0;
            self.pattern = self.pick(params, cv);
            return;
        }
        self.count = self.count.wrapping_add(1);
        self.bar_step += 1;
        if self.bar_step >= steps {
            self.bar_step = 0;
            self.chain_slot += 1;
            self.pattern = self.pick(params, cv);
        }
    }

    /// Plays the step every lane has reached.
    fn play_step(&mut self, params: &[f32], step_samples: usize) {
        let gate_share = (params[Self::PARAM_GATE_LENGTH] / 100.0).clamp(0.01, 1.0);
        let accent_amount = (params[Self::PARAM_ACCENT_AMOUNT] / 100.0).clamp(0.0, 1.0);
        let accented = params[Self::accent_param(self.pattern, self.bar_step)] > 0.5;
        let gate_for = |spacing: usize| ((spacing as f32 * gate_share).round() as usize).clamp(1, spacing.saturating_sub(1).max(1));

        if accented {
            self.accent.strike(gate_for(step_samples));
        }
        for lane in 0..LANES {
            let step = Step::decode(params[Self::step_param(self.pattern, lane, self.lane_step(params, lane))]);
            if !step.on || !self.plays(step.probability) {
                continue;
            }
            let level = step.level();
            let spacing = (step_samples / step.ratchet as usize).max(2);
            let state = &mut self.lanes[lane];
            state.velocity = if accented { level + (1.0 - level) * accent_amount } else { level };
            state.spacing = spacing;
            state.gate_length = gate_for(spacing);
            state.ratchets_left = step.ratchet - 1;
            state.until_next = spacing;
            state.strike(state.gate_length);
        }
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

    /// The pattern the next bar will play.
    fn coming_pattern(&self, params: &[f32], cv: Option<f32>) -> usize {
        match cv {
            Some(cv) => Self::pattern_for_cv(cv),
            None => {
                let (chain, len) = Self::chain(params);
                if self.pending { chain[0] } else { chain[(self.chain_slot + 1) % len] }
            }
        }
    }
}

impl Default for TriggerSequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for TriggerSequencer {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "seq.trigger",
            name: "Trigger Sequencer",
            category: ModuleCategory::Utility,
            description: "Eight lanes of drum triggers, in four patterns played in a chain, with accents, probability and ratchets",
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
        let run_in = connected_input(inputs, Self::PORT_RUN);
        let pattern_in = connected_input(inputs, Self::PORT_PATTERN);
        self.pattern_cv = pattern_in.is_some();

        for i in 0..context.block_size {
            let clock_rising = self.clock.rising(sample(Self::PORT_CLOCK, i));
            let reset_rising = self.reset.rising(sample(Self::PORT_RESET, i));
            let running = run_in.map_or(1.0, |buf| buf.at(i)) > GATE_THRESHOLD;
            let cv = pattern_in.map(|buf| buf.at(i));

            if reset_rising {
                self.pending = true;
                self.timer.forget_gap();
                for lane in self.lanes.iter_mut().chain(std::iter::once(&mut self.accent)) {
                    lane.gate = 0;
                    lane.ratchets_left = 0;
                }
            }

            // Every clock edge measures the step, and ends the last step's
            // ratchets, running or not. Stopped, the sequencer stays on its
            // step and plays nothing
            if clock_rising {
                self.timer.clock();
                for lane in &mut self.lanes {
                    lane.ratchets_left = 0;
                }
            }
            if clock_rising && running {
                self.advance(params, cv);
                let step_samples = self.step_samples(context);
                self.play_step(params, step_samples);
            }

            // Ratchet hits due now
            for lane in &mut self.lanes {
                if lane.ratchets_left > 0 && lane.until_next == 0 {
                    lane.ratchets_left -= 1;
                    lane.until_next = lane.spacing;
                    lane.strike(lane.gate_length);
                }
            }

            let gate = |lane: &Lane| if lane.gate > 0 && !lane.dip { 1.0 } else { 0.0 };
            outputs[Self::OUT_ACCENT].samples[i] = gate(&self.accent);
            for (index, lane) in self.lanes.iter().enumerate() {
                outputs[1 + 2 * index].samples[i] = gate(lane);
                outputs[2 + 2 * index].samples[i] = lane.velocity;
            }

            for lane in self.lanes.iter_mut().chain(std::iter::once(&mut self.accent)) {
                lane.dip = false;
                lane.gate = lane.gate.saturating_sub(1);
                lane.until_next = lane.until_next.saturating_sub(1);
            }
            self.timer.tick();
        }

        let last_cv = pattern_in.and_then(|buf| buf.samples.get(context.block_size.saturating_sub(1)).copied());
        self.next_pattern = self.coming_pattern(params, last_cv);
    }

    fn reset(&mut self) {
        self.pending = true;
        self.clock.reset();
        self.reset.reset();
        self.timer.forget();
        self.count = 0;
        self.bar_step = 0;
        self.chain_slot = 0;
        self.pattern = 0;
        self.lanes = [Lane::default(); LANES];
        self.accent = Lane::default();
        // Each play rolls the same dice, so a take can be played again
        self.random_state = Self::SEED;
    }

    fn readout(&self, params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let mut packed = [0u32; 2];
        for lane in 0..LANES {
            packed[lane / 4] |= (self.lane_step(params, lane) as u32) << (4 * (lane % 4));
        }
        readout.values[Self::READOUT_LANES_LOW] = packed[0] as f32;
        readout.values[Self::READOUT_LANES_HIGH] = packed[1] as f32;
        let started = if self.pending { 0 } else { 32 };
        readout.values[Self::READOUT_PATTERN] = (self.pattern + 4 * self.chain_slot + started) as f32;
        let patched = if self.pattern_cv { 64 } else { 0 };
        readout.values[Self::READOUT_BAR] = (self.next_pattern + 4 * self.bar_step + patched) as f32;
        Some(readout)
    }
}

/// What the display reads back from a [`Readout`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    /// Each lane's step.
    pub lanes: [usize; LANES],
    pub pattern: usize,
    pub chain_slot: usize,
    /// A step has played since the start or the last reset.
    pub started: bool,
    pub next_pattern: usize,
    pub bar_step: usize,
    /// Pattern is patched, so the CV picks patterns rather than the Chain.
    pub pattern_cv: bool,
}

impl Position {
    pub fn from_readout(readout: &Readout) -> Self {
        let lanes_low = readout.values[TriggerSequencer::READOUT_LANES_LOW] as u32;
        let lanes_high = readout.values[TriggerSequencer::READOUT_LANES_HIGH] as u32;
        let pattern = readout.values[TriggerSequencer::READOUT_PATTERN] as usize;
        let bar = readout.values[TriggerSequencer::READOUT_BAR] as usize;
        Self {
            lanes: std::array::from_fn(|lane| {
                let packed = if lane < 4 { lanes_low } else { lanes_high };
                ((packed >> (4 * (lane % 4))) & 0xF) as usize
            }),
            pattern: pattern % 4,
            chain_slot: (pattern / 4) % 8,
            started: pattern & 32 != 0,
            next_pattern: bar % 4,
            bar_step: (bar / 4) % 16,
            pattern_cv: bar & 64 != 0,
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // gaps in the clock, one or several
mod tests {
    use super::*;

    const RATE: f32 = 1000.0;

    fn defaults() -> Vec<f32> {
        TriggerSequencer::new().parameters().iter().map(|p| p.default).collect()
    }

    fn set_step(params: &mut [f32], pattern: usize, lane: usize, step: usize, value: Step) {
        params[TriggerSequencer::step_param(pattern, lane, step)] = value.encode();
    }

    fn hit(velocity: u8) -> Step {
        Step { on: true, velocity, ..Step::DEFAULT }
    }

    fn set_chain(params: &mut [f32], patterns: &str) {
        for slot in 0..CHAIN_SLOTS {
            params[TriggerSequencer::PARAM_CHAIN + slot] = 0.0;
        }
        for (slot, letter) in patterns.chars().enumerate() {
            params[TriggerSequencer::PARAM_CHAIN + slot] = (letter as u8 - b'A' + 1) as f32;
        }
    }

    /// Runs the sequencer at 1 kHz for `ms`, clocked every `period` ms with
    /// 1 ms pulses, with optional reset times and Pattern CV, and returns
    /// every output.
    fn run(params: &[f32], ms: usize, period: usize, resets: &[usize], pattern_cv: Option<f32>) -> Vec<Vec<f32>> {
        run_gated(params, ms, period, resets, pattern_cv, &[])
    }

    /// As [`run`], with Run patched and low over each `stops` range of ms.
    /// With no stops, Run is left unpatched.
    fn run_gated(params: &[f32], ms: usize, period: usize, resets: &[usize], pattern_cv: Option<f32>, stops: &[std::ops::Range<usize>]) -> Vec<Vec<f32>> {
        let mut seq = TriggerSequencer::new();
        seq.prepare(RATE, ms);
        let mut clock = SignalBuffer::gate(ms);
        for t in (0..ms).step_by(period) {
            clock.samples[t] = 1.0;
        }
        let mut reset = SignalBuffer::gate(ms);
        for &t in resets {
            reset.samples[t] = 1.0;
        }
        let run_gate = if stops.is_empty() {
            SignalBuffer::unconnected(ms, SignalType::Gate)
        } else {
            let mut gate = SignalBuffer::gate(ms);
            gate.samples.fill(1.0);
            for stop in stops {
                gate.samples[stop.clone()].fill(0.0);
            }
            gate
        };
        let pattern = match pattern_cv {
            Some(cv) => {
                let mut pattern = SignalBuffer::control(ms);
                pattern.samples.fill(cv);
                pattern
            }
            None => SignalBuffer::unconnected(ms, SignalType::Control),
        };
        let mut outputs: Vec<SignalBuffer> = (0..1 + 2 * LANES).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock, &reset, &run_gate, &pattern], &mut outputs, params, &ProcessContext::new(RATE, ms));
        outputs.into_iter().map(|b| b.samples).collect()
    }

    fn rises(gate: &[f32]) -> Vec<usize> {
        (0..gate.len()).filter(|&t| gate[t] > 0.5 && (t == 0 || gate[t - 1] < 0.5)).collect()
    }

    #[test]
    fn steps_round_trip_through_their_parameter() {
        for on in [true, false] {
            for velocity in [0, 1, 42, 80, 100] {
                for probability in [0, 7, 50, 100] {
                    for ratchet in 1..=MAX_RATCHET {
                        let step = Step { on, velocity, probability, ratchet };
                        assert_eq!(Step::decode(step.encode()), step);
                    }
                }
            }
        }
        assert_eq!(Step::decode(1_100_080.0), hit(80));
        assert_eq!(Step::DEFAULT.encode(), -1_100_080.0);
    }

    #[test]
    fn parameters_are_where_the_constants_say() {
        let seq = TriggerSequencer::new();
        let params = seq.parameters();
        assert_eq!(params.len(), TriggerSequencer::PARAM_COUNT);
        assert_eq!(params[TriggerSequencer::PARAM_STEPS].name, "Steps");
        assert_eq!(params[TriggerSequencer::PARAM_CHAIN].name, "Chain 1");
        assert_eq!(params[TriggerSequencer::PARAM_LENGTH + 7].name, "Length 8");
        assert_eq!(params[TriggerSequencer::accent_param(1, 4)].name, "Accent B 05");
        assert_eq!(params[TriggerSequencer::step_param(0, 0, 0)].name, "Step A1 01");
        assert_eq!(params[TriggerSequencer::step_param(3, 7, 15)].name, "Step D8 16");
        assert_eq!(params[TriggerSequencer::step_param(2, 4, 9)].id, "step_c5_10");
        // Unique, or saving by name would mix them up
        let mut names: Vec<&str> = params.iter().map(|p| p.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), params.len());
    }

    #[test]
    fn lanes_play_their_steps_with_velocity() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 0, hit(100));
        set_step(&mut params, 0, 0, 2, hit(40));
        set_step(&mut params, 0, 3, 1, hit(60));
        let out = run(&params, 1600, 100, &[], None);
        let (gate1, vel1, gate4) = (&out[1], &out[2], &out[7]);
        // Steps 1 and 3 of each 16-step bar, and lane 4's step 2
        assert_eq!(rises(gate1), [0, 200]);
        assert_eq!(rises(gate4), [100]);
        assert_eq!(vel1[0], 1.0);
        assert!((vel1[200] - 0.4).abs() < 1e-6);
        // Velocity holds until the next hit
        assert!((vel1[1500] - 0.4).abs() < 1e-6);
        // Half the step by default, once the clock is measured
        let high = gate1[200..].iter().take_while(|&&g| g > 0.5).count();
        assert_eq!(high, 50);
    }

    #[test]
    fn chain_aaab_plays_b_every_fourth_bar_without_drift() {
        let mut params = defaults();
        // A plays lane 1 on the downbeat, B lane 2
        for pattern in 0..PATTERNS {
            set_step(&mut params, pattern, if pattern == 1 { 1 } else { 0 }, 0, hit(100));
        }
        set_chain(&mut params, "AAAB");
        // 64 bars of sixteenths at 125 BPM: 30 ms a step
        let bars = 64;
        let out = run(&params, bars * 16 * 30, 30, &[], None);
        let bar_of = |t: usize| t / (16 * 30);
        let a: Vec<usize> = rises(&out[1]).into_iter().map(bar_of).collect();
        let b: Vec<usize> = rises(&out[3]).into_iter().map(bar_of).collect();
        let expected_b: Vec<usize> = (0..bars).filter(|bar| bar % 4 == 3).collect();
        let expected_a: Vec<usize> = (0..bars).filter(|bar| bar % 4 != 3).collect();
        assert_eq!(b, expected_b);
        assert_eq!(a, expected_a);
        // Every hit on its bar's downbeat, to the sample
        for t in rises(&out[1]).into_iter().chain(rises(&out[3])) {
            assert_eq!(t % (16 * 30), 0);
        }
    }

    #[test]
    fn ratchet_three_fires_three_evenly_spaced_gates() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 2, Step { ratchet: 3, ..hit(100) });
        // A sixteenth at 125 BPM is 120 ms at 1 kHz; three hits 40 ms apart
        let out = run(&params, 16 * 120, 120, &[], None);
        assert_eq!(rises(&out[1]), [240, 280, 320]);
        // Each gate is half its third of the step
        for start in [240, 280, 320] {
            assert_eq!(out[1][start..].iter().take_while(|&&g| g > 0.5).count(), 20);
        }
    }

    #[test]
    fn ratchets_follow_a_swung_clock() {
        let mut params = defaults();
        for step in 0..STEPS {
            set_step(&mut params, 0, 0, step, Step { ratchet: 2, ..hit(100) });
        }
        // 66% swing: steps of 160 and 80 ms
        let ms = 1200;
        let mut seq = TriggerSequencer::new();
        seq.prepare(RATE, ms);
        let mut clock = SignalBuffer::gate(ms);
        let clocks: Vec<usize> = (0..ms).step_by(240).flat_map(|t| [t, t + 160]).collect();
        for &t in &clocks {
            clock.samples[t] = 1.0;
        }
        let mut outputs: Vec<SignalBuffer> = (0..1 + 2 * LANES).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock], &mut outputs, &params, &ProcessContext::new(RATE, ms));
        let rises = rises(&outputs[1].samples);
        // Once the rhythm shows, each step splits into its own halves
        assert!(rises.contains(&720) && rises.contains(&800), "{rises:?}");
        assert!(rises.contains(&880) && rises.contains(&920), "{rises:?}");
    }

    #[test]
    fn probability_plays_some_of_the_time() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 0, Step { probability: 50, ..hit(100) });
        params[TriggerSequencer::PARAM_STEPS] = 1.0;
        let out = run(&params, 10_000, 10, &[], None);
        let played = rises(&out[1]).len();
        assert!((400..600).contains(&played), "{played} of 1000");

        set_step(&mut params, 0, 0, 0, Step { probability: 0, ..hit(100) });
        assert!(rises(&run(&params, 1000, 10, &[], None)[1]).is_empty());
    }

    #[test]
    fn accent_raises_velocity_and_gates_its_output() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 0, hit(60));
        set_step(&mut params, 0, 0, 1, hit(60));
        params[TriggerSequencer::accent_param(0, 1)] = 1.0;
        params[TriggerSequencer::PARAM_ACCENT_AMOUNT] = 50.0;
        let out = run(&params, 400, 100, &[], None);
        assert!((out[2][0] - 0.6).abs() < 1e-6);
        assert!((out[2][100] - 0.8).abs() < 1e-6, "halfway from 60% to full");
        assert_eq!(rises(&out[0]), [100]);
    }

    #[test]
    fn a_lane_with_its_own_length_drifts_against_the_bar() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 0, hit(100));
        set_step(&mut params, 0, 1, 0, hit(100));
        params[TriggerSequencer::PARAM_LENGTH + 1] = 3.0;
        let out = run(&params, 3200, 100, &[], None);
        // Lane 1 follows the 16-step bar; lane 2 loops every 3 steps
        assert_eq!(rises(&out[1]), [0, 1600]);
        let expected: Vec<usize> = (0..32).step_by(3).map(|step| step * 100).collect();
        assert_eq!(rises(&out[3]), expected);
    }

    #[test]
    fn steps_sets_the_bar_and_the_chain_follows_it() {
        let mut params = defaults();
        params[TriggerSequencer::PARAM_STEPS] = 3.0;
        set_step(&mut params, 0, 0, 0, hit(100));
        set_step(&mut params, 1, 0, 0, hit(50));
        set_chain(&mut params, "AB");
        let out = run(&params, 1200, 100, &[], None);
        assert_eq!(rises(&out[1]), [0, 300, 600, 900]);
        let velocities: Vec<f32> = [0, 300, 600, 900].iter().map(|&t| out[2][t]).collect();
        assert_eq!(velocities, [1.0, 0.5, 1.0, 0.5]);
    }

    #[test]
    fn pattern_cv_picks_the_pattern_for_each_bar() {
        let mut params = defaults();
        for pattern in 0..PATTERNS {
            set_step(&mut params, pattern, pattern, 0, hit(100));
        }
        params[TriggerSequencer::PARAM_STEPS] = 2.0;
        let out = run(&params, 400, 100, &[], Some(0.6));
        // C's lane only
        assert_eq!(rises(&out[5]), [0, 200]);
        assert!(rises(&out[1]).is_empty());
        assert_eq!(TriggerSequencer::pattern_for_cv(-1.0), 0);
        assert_eq!(TriggerSequencer::pattern_for_cv(0.99), 3);
        assert_eq!(TriggerSequencer::pattern_for_cv(5.0), 3);
    }

    #[test]
    fn reset_starts_the_chain_over() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 0, hit(100));
        set_step(&mut params, 1, 1, 0, hit(100));
        set_chain(&mut params, "AB");
        params[TriggerSequencer::PARAM_STEPS] = 4.0;
        // Into B's bar at 400, reset at 550, so 600 is A's first step again
        let out = run(&params, 1000, 100, &[550], None);
        assert_eq!(rises(&out[1]), [0, 600]);
        assert_eq!(rises(&out[3]), [400]);
    }

    #[test]
    fn run_low_holds_the_playhead_and_fires_nothing() {
        let mut params = defaults();
        for step in 0..4 {
            set_step(&mut params, 0, 0, step, hit(100));
            set_step(&mut params, 1, 1, step, hit(100));
        }
        set_chain(&mut params, "AB");
        params[TriggerSequencer::PARAM_STEPS] = 4.0;
        // A's bar, then Run is low for a whole bar, so B's bar comes after it
        // as though those four clocks had never come
        let out = run_gated(&params, 1600, 100, &[], None, &[400..800]);
        assert_eq!(rises(&out[1]), [0, 100, 200, 300, 1200, 1300, 1400, 1500]);
        assert_eq!(rises(&out[3]), [800, 900, 1000, 1100]);
        assert!(out[0].iter().all(|&g| g == 0.0), "no accents were written");
    }

    #[test]
    fn run_low_mid_bar_resumes_on_the_step_it_stopped_at() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 2, hit(100));
        params[TriggerSequencer::PARAM_STEPS] = 4.0;
        // Steps 1 and 2 play, three clocks are skipped, then step 3 plays
        let out = run_gated(&params, 1000, 100, &[], None, &[150..450]);
        assert_eq!(rises(&out[1]), [500, 900]);
    }

    #[test]
    fn a_stop_lets_the_last_step_finish_its_ratchet() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 1, Step { ratchet: 4, ..hit(100) });
        params[TriggerSequencer::PARAM_STEPS] = 4.0;
        // Step 2's roll plays out in full, though Run drops halfway through
        let out = run_gated(&params, 600, 100, &[], None, &[150..600]);
        assert_eq!(rises(&out[1]), [100, 125, 150, 175]);
    }

    #[test]
    fn reset_while_stopped_starts_over_when_run_comes_back() {
        let mut params = defaults();
        set_step(&mut params, 0, 0, 0, hit(100));
        params[TriggerSequencer::PARAM_STEPS] = 4.0;
        // Stopped on step 3, reset, and the first clock back plays step 1
        let out = run_gated(&params, 1000, 100, &[350], None, &[250..600]);
        assert_eq!(rises(&out[1]), [0, 600]);
    }

    #[test]
    fn a_hit_over_a_held_gate_dips_first() {
        let mut params = defaults();
        params[TriggerSequencer::PARAM_GATE_LENGTH] = 100.0;
        params[TriggerSequencer::PARAM_STEPS] = 1.0;
        set_step(&mut params, 0, 0, 0, hit(100));
        // A clock that speeds up leaves the old gate high at the next edge
        let ms = 400;
        let mut seq = TriggerSequencer::new();
        seq.prepare(RATE, ms);
        let mut clock = SignalBuffer::gate(ms);
        for t in [0, 100, 200, 250, 300] {
            clock.samples[t] = 1.0;
        }
        let mut outputs: Vec<SignalBuffer> = (0..1 + 2 * LANES).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock], &mut outputs, &params, &ProcessContext::new(RATE, ms));
        let gate = &outputs[1].samples;
        assert_eq!(gate[250], 0.0, "dips for a sample");
        assert_eq!(rises(gate).len(), 5);
    }

    #[test]
    fn readout_reports_where_each_lane_is() {
        let mut params = defaults();
        params[TriggerSequencer::PARAM_LENGTH + 5] = 3.0;
        set_chain(&mut params, "BC");
        let mut seq = TriggerSequencer::new();
        seq.prepare(RATE, 1);
        let before = Position::from_readout(&seq.readout(&params).unwrap());
        assert!(!before.started);

        let mut clock = SignalBuffer::gate(1);
        let mut outputs: Vec<SignalBuffer> = (0..1 + 2 * LANES).map(|_| SignalBuffer::control(1)).collect();
        let ctx = ProcessContext::new(RATE, 1);
        for _ in 0..21 {
            clock.samples[0] = 1.0;
            seq.process(&[&clock], &mut outputs, &params, &ctx);
            clock.samples[0] = 0.0;
            seq.process(&[&clock], &mut outputs, &params, &ctx);
        }
        let at = Position::from_readout(&seq.readout(&params).unwrap());
        assert!(at.started);
        assert_eq!(at.bar_step, 4);
        assert_eq!(at.lanes[0], 4);
        assert_eq!(at.lanes[5], 20 % 3);
        assert_eq!((at.pattern, at.chain_slot, at.next_pattern), (2, 1, 1));
    }

    #[test]
    fn is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<TriggerSequencer>();
    }
}
