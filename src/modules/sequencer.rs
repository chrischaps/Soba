//! Step Sequencer module.
//!
//! A 16-step sequencer with per-step pitch, gate, and velocity.
//! Advances on clock input, outputs CV/Gate signals for driving oscillators and envelopes.
//!
//! It holds four patterns, A to D, played in the order a Chain gives as on
//! the Trigger Sequencer, so "A A B C" plays a 64-step line, and a Pattern
//! CV can pick them instead. Pattern A's parameters are the ones the
//! sequencer has always had ("Step 3 Pitch"); B to D's come after all of
//! them ("Step B3 Pitch"), so patches saved before there were patterns load
//! with only A and play as they did.
//!
//! Any step can be a slide: Pitch glides into it from the note before and
//! the gate stays high across the join, as on a 303. The Glide time and the
//! slides of all four patterns come after everything else.
//!
//! A note written in on the face's piano is heard as it's written, on the
//! sequencer's own outputs (see [`DspModule::audition`]): struck at once,
//! clock or no clock, for at least the Gate Length and for as long as the
//! key's held.
//!
//! A step can play only some of the time, by its Chance, and Wander lets
//! the line drift from what's written: each note struck may step up or
//! down a few degrees of a scale, while Keep holds step 1 (or the accents)
//! where they are. These came last of all, Wander first among the
//! parameters only so its jack sits with the others.

use std::sync::LazyLock;

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, Pulse, GATE_THRESHOLD},
    primitives::Glide,
    module_trait::{Audition, DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    ParameterDisplay, Readout, SignalType,
};

use super::noise::Pcg32;
use super::quantizer::{in_key, rotate_to_key, scale_mask, CUSTOM_SCALE, NOTE_NAMES, SCALE_NAMES};
use super::trigger_sequencer::{chain_from, TriggerSequencer, CHAIN_CHOICES, CHAIN_IDS, CHAIN_NAMES, CHAIN_SLOTS};
pub use super::trigger_sequencer::{PATTERNS, PATTERN_NAMES};

/// Maximum number of steps in the sequencer.
pub const MAX_STEPS: usize = 16;

// Static parameter IDs and names for each step (must be 'static for ParameterDefinition)
static STEP_PITCH_IDS: [&str; MAX_STEPS] = [
    "step_1_pitch", "step_2_pitch", "step_3_pitch", "step_4_pitch",
    "step_5_pitch", "step_6_pitch", "step_7_pitch", "step_8_pitch",
    "step_9_pitch", "step_10_pitch", "step_11_pitch", "step_12_pitch",
    "step_13_pitch", "step_14_pitch", "step_15_pitch", "step_16_pitch",
];

static STEP_PITCH_NAMES: [&str; MAX_STEPS] = [
    "Step 1 Pitch", "Step 2 Pitch", "Step 3 Pitch", "Step 4 Pitch",
    "Step 5 Pitch", "Step 6 Pitch", "Step 7 Pitch", "Step 8 Pitch",
    "Step 9 Pitch", "Step 10 Pitch", "Step 11 Pitch", "Step 12 Pitch",
    "Step 13 Pitch", "Step 14 Pitch", "Step 15 Pitch", "Step 16 Pitch",
];

static STEP_GATE_IDS: [&str; MAX_STEPS] = [
    "step_1_gate", "step_2_gate", "step_3_gate", "step_4_gate",
    "step_5_gate", "step_6_gate", "step_7_gate", "step_8_gate",
    "step_9_gate", "step_10_gate", "step_11_gate", "step_12_gate",
    "step_13_gate", "step_14_gate", "step_15_gate", "step_16_gate",
];

static STEP_GATE_NAMES: [&str; MAX_STEPS] = [
    "Step 1 Gate", "Step 2 Gate", "Step 3 Gate", "Step 4 Gate",
    "Step 5 Gate", "Step 6 Gate", "Step 7 Gate", "Step 8 Gate",
    "Step 9 Gate", "Step 10 Gate", "Step 11 Gate", "Step 12 Gate",
    "Step 13 Gate", "Step 14 Gate", "Step 15 Gate", "Step 16 Gate",
];

static STEP_VELOCITY_IDS: [&str; MAX_STEPS] = [
    "step_1_velocity", "step_2_velocity", "step_3_velocity", "step_4_velocity",
    "step_5_velocity", "step_6_velocity", "step_7_velocity", "step_8_velocity",
    "step_9_velocity", "step_10_velocity", "step_11_velocity", "step_12_velocity",
    "step_13_velocity", "step_14_velocity", "step_15_velocity", "step_16_velocity",
];

static STEP_VELOCITY_NAMES: [&str; MAX_STEPS] = [
    "Step 1 Velocity", "Step 2 Velocity", "Step 3 Velocity", "Step 4 Velocity",
    "Step 5 Velocity", "Step 6 Velocity", "Step 7 Velocity", "Step 8 Velocity",
    "Step 9 Velocity", "Step 10 Velocity", "Step 11 Velocity", "Step 12 Velocity",
    "Step 13 Velocity", "Step 14 Velocity", "Step 15 Velocity", "Step 16 Velocity",
];

static STEP_TIE_IDS: [&str; MAX_STEPS] = [
    "step_1_tie", "step_2_tie", "step_3_tie", "step_4_tie",
    "step_5_tie", "step_6_tie", "step_7_tie", "step_8_tie",
    "step_9_tie", "step_10_tie", "step_11_tie", "step_12_tie",
    "step_13_tie", "step_14_tie", "step_15_tie", "step_16_tie",
];

static STEP_TIE_NAMES: [&str; MAX_STEPS] = [
    "Step 1 Tie", "Step 2 Tie", "Step 3 Tie", "Step 4 Tie",
    "Step 5 Tie", "Step 6 Tie", "Step 7 Tie", "Step 8 Tie",
    "Step 9 Tie", "Step 10 Tie", "Step 11 Tie", "Step 12 Tie",
    "Step 13 Tie", "Step 14 Tie", "Step 15 Tie", "Step 16 Tie",
];

static STEP_SLIDE_IDS: [&str; MAX_STEPS] = [
    "step_1_slide", "step_2_slide", "step_3_slide", "step_4_slide",
    "step_5_slide", "step_6_slide", "step_7_slide", "step_8_slide",
    "step_9_slide", "step_10_slide", "step_11_slide", "step_12_slide",
    "step_13_slide", "step_14_slide", "step_15_slide", "step_16_slide",
];

static STEP_SLIDE_NAMES: [&str; MAX_STEPS] = [
    "Step 1 Slide", "Step 2 Slide", "Step 3 Slide", "Step 4 Slide",
    "Step 5 Slide", "Step 6 Slide", "Step 7 Slide", "Step 8 Slide",
    "Step 9 Slide", "Step 10 Slide", "Step 11 Slide", "Step 12 Slide",
    "Step 13 Slide", "Step 14 Slide", "Step 15 Slide", "Step 16 Slide",
];

static STEP_CHANCE_IDS: [&str; MAX_STEPS] = [
    "step_1_chance", "step_2_chance", "step_3_chance", "step_4_chance",
    "step_5_chance", "step_6_chance", "step_7_chance", "step_8_chance",
    "step_9_chance", "step_10_chance", "step_11_chance", "step_12_chance",
    "step_13_chance", "step_14_chance", "step_15_chance", "step_16_chance",
];

static STEP_CHANCE_NAMES: [&str; MAX_STEPS] = [
    "Step 1 Chance", "Step 2 Chance", "Step 3 Chance", "Step 4 Chance",
    "Step 5 Chance", "Step 6 Chance", "Step 7 Chance", "Step 8 Chance",
    "Step 9 Chance", "Step 10 Chance", "Step 11 Chance", "Step 12 Chance",
    "Step 13 Chance", "Step 14 Chance", "Step 15 Chance", "Step 16 Chance",
];

/// The things each step holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepField {
    Pitch = 0,
    Gate = 1,
    Velocity = 2,
    Tie = 3,
    /// Came after patterns, so its parameters are in a block of their own.
    Slide = 4,
    /// How often the step plays, in percent. Last of all, in a block of its own.
    Chance = 5,
}

impl StepField {
    /// Every field, as copying a pattern copies them.
    pub const ALL: [StepField; 6] = [StepField::Pitch, StepField::Gate, StepField::Velocity, StepField::Tie, StepField::Slide, StepField::Chance];
}

/// The four fields patterns B to D keep side by side, a step at a time.
const FIELDS: [StepField; 4] = [StepField::Pitch, StepField::Gate, StepField::Velocity, StepField::Tie];
const FIELD_NAMES: [&str; 4] = ["Pitch", "Gate", "Velocity", "Tie"];

/// Patterns B to D's parameter ids and names, made once and kept: they must
/// be `'static`, and there are 240 of them. In the order the parameters
/// are: pattern, then step, then field; and then the slides, pattern by
/// pattern; and then the chances, the same way.
struct PatternNames {
    ids: Vec<&'static str>,
    names: Vec<&'static str>,
    slide_ids: Vec<&'static str>,
    slide_names: Vec<&'static str>,
    chance_ids: Vec<&'static str>,
    chance_names: Vec<&'static str>,
}

static PATTERN_PARAM_NAMES: LazyLock<PatternNames> = LazyLock::new(|| {
    let keep = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    let mut names = PatternNames {
        ids: Vec::new(),
        names: Vec::new(),
        slide_ids: Vec::new(),
        slide_names: Vec::new(),
        chance_ids: Vec::new(),
        chance_names: Vec::new(),
    };
    for letter in &PATTERN_NAMES[1..] {
        for step in 1..=MAX_STEPS {
            for field in FIELD_NAMES {
                names.ids.push(keep(format!("step_{}{step}_{}", letter.to_lowercase(), field.to_lowercase())));
                names.names.push(keep(format!("Step {letter}{step} {field}")));
            }
        }
    }
    for letter in &PATTERN_NAMES[1..] {
        for step in 1..=MAX_STEPS {
            names.slide_ids.push(keep(format!("step_{}{step}_slide", letter.to_lowercase())));
            names.slide_names.push(keep(format!("Step {letter}{step} Slide")));
            names.chance_ids.push(keep(format!("step_{}{step}_chance", letter.to_lowercase())));
            names.chance_names.push(keep(format!("Step {letter}{step} Chance")));
        }
    }
    names
});

/// What Gate Length is a share of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateMode {
    /// The time between the last two clock pulses.
    Step = 0,
    /// A fixed 100 ms, whatever the tempo. Patches saved before Gate Mode
    /// existed load with this.
    Fixed = 1,
}

impl GateMode {
    /// Convert from parameter value (0-1) to mode.
    pub fn from_param(value: f32) -> Self {
        if value >= 0.5 { GateMode::Fixed } else { GateMode::Step }
    }
}

/// Which notes Wander leaves as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// Every note may wander.
    Off = 0,
    /// Step 1 always plays as written, so the line keeps its downbeat.
    First = 1,
    /// Accents (a Velocity of [`Keep::ACCENT`] or more) play as written.
    Accents = 2,
}

impl Keep {
    /// The Velocity from which a step is an accent: above the 100 steps start at.
    pub const ACCENT: f32 = 110.0;

    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Keep::First,
            2 => Keep::Accents,
            _ => Keep::Off,
        }
    }
}

/// `note` moved `degrees` notes of `key` up (or down, when negative). A note
/// off the scale counts its first step as the nearest scale note that way.
/// `key` must not be empty.
pub(crate) fn wander_note(note: u8, degrees: i32, key: u16) -> u8 {
    let step = degrees.signum();
    let mut at = note as i32;
    for _ in 0..degrees.abs() {
        at += step;
        while !in_key(key, at - 60) {
            at += step;
        }
    }
    at.clamp(0, 127) as u8
}

/// Direction modes for sequence playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceDirection {
    /// Play steps in order: 1, 2, 3, ..., N, 1, 2, 3, ...
    Forward = 0,
    /// Play steps in reverse: N, N-1, ..., 2, 1, N, N-1, ...
    Backward = 1,
    /// Bounce back and forth: 1, 2, ..., N, N-1, ..., 2, 1, 2, ...
    PingPong = 2,
    /// Random step selection
    Random = 3,
}

impl SequenceDirection {
    /// Convert from parameter value (0-3) to direction.
    pub fn from_param(value: f32) -> Self {
        match value as usize {
            0 => SequenceDirection::Forward,
            1 => SequenceDirection::Backward,
            2 => SequenceDirection::PingPong,
            3 => SequenceDirection::Random,
            _ => SequenceDirection::Forward,
        }
    }

    /// The step a pattern starts from: the last one when playing backward,
    /// otherwise the first.
    pub(crate) fn start_step(self, num_steps: usize) -> usize {
        match self {
            SequenceDirection::Backward => num_steps - 1,
            _ => 0,
        }
    }
}

/// Measures a clock's steps: the time between its rising edges, in samples.
///
/// Shared by the sequencers, so a gate that is a share of the step, or a
/// ratchet that divides it, follows the clock the same way in both.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StepTimer {
    /// Samples since the last clock edge, or `None` before the first (and
    /// after a reset, so a stopped clock's gap isn't taken for a step).
    since_clock: Option<usize>,
    /// The time between the last two clock edges, once known.
    step_samples: Option<usize>,
    /// The two steps measured before that, newest first.
    earlier_steps: [Option<usize>; 2],
}

impl StepTimer {
    pub(crate) const fn new() -> Self {
        Self { since_clock: None, step_samples: None, earlier_steps: [None; 2] }
    }

    /// A rising clock edge: measures the step it ends.
    pub(crate) fn clock(&mut self) {
        if let Some(samples) = self.since_clock {
            self.earlier_steps = [self.step_samples, self.earlier_steps[0]];
            self.step_samples = Some(samples.max(1));
        }
        self.since_clock = Some(0);
    }

    /// One sample has passed.
    pub(crate) fn tick(&mut self) {
        if let Some(samples) = self.since_clock.as_mut() {
            *samples = samples.saturating_add(1);
        }
    }

    /// A reset: the time until the next clock isn't a step, though the steps
    /// measured so far still are.
    pub(crate) fn forget_gap(&mut self) {
        self.since_clock = None;
    }

    /// Samples since the last clock edge, or `None` before the first (and
    /// after a reset).
    pub(crate) fn since_clock(&self) -> Option<usize> {
        self.since_clock
    }

    /// Forgets everything measured, as at another sample rate.
    pub(crate) fn forget(&mut self) {
        *self = Self::new();
    }

    /// How long the step starting now will last, in samples: usually the
    /// last step measured. A swung clock's steps alternate long and short,
    /// though, so when the step before last matches the last, the rhythm
    /// repeats every two steps and the coming step is the one before last.
    /// A steady clock gives the same answer either way.
    pub(crate) fn coming_step(&self) -> Option<usize> {
        let last = self.step_samples?;
        match self.earlier_steps {
            [Some(before), Some(third)] if last.abs_diff(third) <= last / 32 + 1 => Some(before),
            _ => Some(last),
        }
    }
}

/// Convert a MIDI note number (0-127), or a pitch between notes, to V/Oct
/// control signal. C4 (note 60) = 0V, each semitone = 1/12 V
fn note_to_voct(note: f32) -> f32 {
    (note - 60.0) / 12.0
}

/// Convert a note number to a note name for display.
pub fn note_to_name(note: u8) -> String {
    const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = (note / 12) as i32 - 1;
    let name = NOTES[(note % 12) as usize];
    format!("{}{}", name, octave)
}

/// The module's ID, as patches name it.
pub const ID: &str = "seq.step";

/// A step sequencer module with 16 steps.
///
/// Outputs pitch CV, gate, and velocity for each step, advancing on clock input.
///
/// # Ports
///
/// **Inputs:**
/// - **Clock** (Gate): Advances to the next step on rising edge.
/// - **Reset** (Gate): Returns to step 1 on rising edge.
/// - **Run** (Gate): Enables/disables sequencer advancement.
/// - **Pattern** (Control): Picks the pattern for each pass in place of the
///   Chain, in four equal zones of 0 to 1 (as on the Trigger Sequencer).
/// - **Wander** (Control): Added to the Wander knob, 1 for its whole range.
///
/// **Outputs:**
/// - **Pitch** (Control): V/Oct pitch CV from current step.
/// - **Gate** (Gate): Gate output for current step.
/// - **Velocity** (Control): Velocity (0-1) from current step.
/// - **Step** (Control): Current step as 0-1 value (for visualization).
/// - **EOC** (Gate): End-of-cycle trigger pulse, at the end of the chain.
///
/// # Parameters
///
/// - **Wander** (0-4): How many degrees of the scale a note may stray from
///   what's written (see Chance and Wander, below). At 0 every note plays as
///   written.
/// - **Steps** (1-16): Number of active steps in the sequence.
/// - **Direction** (0-3): Playback direction (Forward, Backward, PingPong, Random).
/// - **Gate Length** (1-100%): Gate duration as a share of the step (or of
///   100 ms in the Fixed gate mode). At 100% of the step the gate holds until
///   the next clock.
/// - **Step 1-16 Pitch** (0-127): MIDI note number for each step.
/// - **Step 1-16 Gate** (0/1): Gate on/off for each step.
/// - **Step 1-16 Velocity** (0-127): Velocity for each step.
/// - **Gate Mode** (Step / 100 ms): What Gate Length is a share of.
/// - **Step 1-16 Tie** (0/1): Holds the step's gate into the next step,
///   which then continues the note instead of starting a new one.
/// - **Chain 1-8** (– / A-D): The patterns played, a pass each, in order.
/// - **Step B1-D16 Pitch, Gate, Velocity, Tie**: Patterns B to D's steps.
/// - **Glide** (0-1 s): How long a slide takes to reach its note.
/// - **Step 1-16 Slide**, **Step B1-D16 Slide** (0/1): The step is slurred
///   into from the note before: Pitch glides there over the Glide time, and
///   the gate stays high across the join, as on a 303.
/// - **Scale**, **Root**: The scale Wander steps along.
/// - **Keep** (Off / Step 1 / Accents): Notes Wander leaves as written.
/// - **Step 1-16 Chance**, **Step B1-D16 Chance** (0-100%): How often the
///   step plays. One that doesn't is a rest.
///
/// # Patterns
///
/// A pass through a pattern ends where the sequence wraps (or turns, in
/// ping-pong), or after Steps clocks in random order. The next pass plays
/// the Chain's next pattern, or the one the Pattern CV picks at that clock.
/// The playing note's tie carries into the next pattern's first step, and
/// EOC fires once the whole chain has played.
///
/// In the Step gate mode a note always starts with a rising edge: when a new
/// note begins while the gate is still high, the gate drops for one sample
/// first so envelopes retrigger. Only a tie or a slide carries the gate
/// across unbroken.
///
/// # Slides
///
/// A slide needs a note to come from: after a rest or a reset, a slide step
/// is struck and starts on its own pitch. The note before a slide holds its
/// gate until the slide begins, whatever the Gate Length, so the two join;
/// it looks ahead to the step the next clock will play, in the pattern that
/// step will be in. A tie carries a slide's glide on into the next step.
///
/// # Chance and Wander
///
/// Each step's dice are thrown as the clock reaches it: one that loses is a
/// rest, ending a note tied into it. Then each note struck may wander: with
/// Wander at `w`, it moves `x` degrees of the scale, `x` drawn evenly from
/// `-w` to `w` and rounded up or down at random in proportion, so a Wander
/// of 0.2 moves a note now and then by one degree and one of 2 up to two
/// either way. A tie continues its note where it wandered to. Nothing is
/// drawn while Wander is 0 and every Chance 100%, so a patch that uses
/// neither plays the same notes it always did.
///
/// # Auditions
///
/// A note played in on the piano sounds on Pitch, Gate and Velocity in
/// place of the sequence: its gate rises at once (dipping for a sample
/// first), stays high while the key's held and for at least a Gate Length,
/// then falls. Another key struck under the same hand moves Pitch without
/// striking again, as it's written tied. Once let go, the note the clock
/// struck meanwhile isn't heard: Pitch stays on the audition, as a rest
/// leaves it, until the sequence strikes its own next note.
pub struct StepSequencer {
    /// Current step index (0-based).
    current_step: usize,
    /// Direction for ping-pong mode (+1 or -1).
    ping_pong_direction: i32,
    /// Set by a reset (and at the start): the next clock plays the start step
    /// instead of advancing past it.
    reset_pending: bool,
    /// The clock input, for its rising edges.
    clock: Edge,
    /// The reset input, for its rising edges.
    reset: Edge,
    /// Gate timer (samples remaining in gate).
    gate_timer: usize,
    /// The note playing is tied into the next step.
    tied: bool,
    /// The gate output was high on the last sample.
    gate_high: bool,
    /// The last step played was a note, not a rest, so a slide has
    /// somewhere to come from.
    playing_note: bool,
    /// Pitch, in semitones, as it glides.
    glide: Glide,
    /// The note playing slides to its pitch; otherwise Pitch lands on it.
    sliding: bool,
    /// How long the clock's steps are.
    timer: StepTimer,
    /// The end-of-cycle trigger.
    eoc: Pulse,
    /// Simple PRNG state for random mode.
    random_state: u32,
    /// The random number for the next step, once drawn to see whether it
    /// slides. Taken by the next advance, so the steps play in the same
    /// order as when nothing looked ahead.
    next_random: Option<u32>,
    /// A note played in from the face, on the outputs while the hand's down
    /// or its gate is still ringing, and left on Pitch after (see
    /// [`DspModule::audition`]).
    audition: Option<Audition>,
    /// The hand's down on the audition.
    auditioning: bool,
    /// A new audition is struck at the next block: its gate dips for a
    /// sample, then holds for at least a Gate Length.
    audition_struck: bool,
    /// Samples left of the audition's Gate Length.
    audition_gate: usize,
    /// The pattern playing.
    pattern: usize,
    /// Which Chain entry is playing.
    chain_slot: usize,
    /// Clocks into this pass through the pattern (random order counts them).
    pass_clocks: usize,
    /// The pattern the next pass will play, for the display.
    next_pattern: usize,
    /// Pattern was patched in the last block.
    pattern_cv: bool,
    /// Throws each step's dice and each note's wander.
    rng: Pcg32,
    /// The degrees the note playing has wandered, which a tie carries on.
    wander: i32,
    /// The last step the clock reached lost its dice and rests.
    dropped: bool,
    /// Sample rate from prepare().
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl StepSequencer {
    /// Creates a new StepSequencer with default values.
    pub fn new() -> Self {
        let ports = vec![
            // Input ports
            PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0).describe("Each rising edge advances to the next step; patch a Clock gate here"),
            PortDefinition::input_with_default("reset", "Reset", SignalType::Gate, 0.0).describe("A rising edge jumps back to step 1"),
            PortDefinition::input_with_default("run", "Run", SignalType::Gate, 1.0).describe("Steps only advance while high; runs when unpatched"),
            PortDefinition::input_with_default("pattern", "Pattern", SignalType::Control, 0.0).describe("Picks the pattern for each pass in place of the Chain: 0 to 0.25 is A, then B, C and D"),
            PortDefinition::input_with_default("wander_cv", "Wander", SignalType::Control, 0.0).describe("CV added to the Wander knob: 1 more is its whole range"),
            // Output ports
            PortDefinition::output("pitch", "Pitch", SignalType::Control).describe("Pitch of the current step as V/Oct; patch into an oscillator"),
            PortDefinition::output("gate", "Gate", SignalType::Gate).describe("Pulses on each clock when the step's gate is on; patch into an envelope"),
            PortDefinition::output("velocity", "Velocity", SignalType::Control).describe("Velocity of the current step, 0 to 1"),
            PortDefinition::output("step_out", "Step", SignalType::Control).describe("Current step position as a 0 to 1 ramp"),
            PortDefinition::output("eoc", "EOC", SignalType::Gate).describe("Short pulse when the sequence reaches its end"),
        ];

        let mut parameters = vec![
            // Wander came last, but its jack has to come before the knob-only
            // parameters, and patches load parameters by name
            ParameterDefinition::new("wander", "Wander", 0.0, Self::WANDER_MAX, 0.0, ParameterDisplay::linear(""))
                .describe("How far notes stray from what's written, in degrees of the scale; 0 plays them as written"),
            // Global sequencer parameters
            ParameterDefinition::new(
                "steps",
                "Steps",
                1.0,
                16.0,
                8.0,
                ParameterDisplay::linear(""),
            ).describe("How many steps play before the sequence loops"),
            ParameterDefinition::choice(
                "direction",
                "Direction",
                &["Fwd", "Bwd", "P-P", "Rnd"],
                0,
            ).describe("Playback order: forward, backward, ping-pong or random"),
            ParameterDefinition::new(
                "gate_length",
                "Gate Length",
                1.0,
                100.0,
                50.0,
                ParameterDisplay::linear("%"),
            ).describe("How long each gate stays high, as a share of the step; 100% holds it until the next clock"),
        ];

        // Add per-step parameters: pitch, gate, velocity for each of 16 steps
        for i in 0..MAX_STEPS {
            // Pitch: MIDI note number (default to C4 = 60)
            parameters.push(ParameterDefinition::new(
                STEP_PITCH_IDS[i],
                STEP_PITCH_NAMES[i],
                0.0,
                127.0,
                60.0,
                ParameterDisplay::linear(""),
            ).describe("Note for this step as a MIDI number; 60 is middle C"));

            // Gate: on/off toggle (default on)
            parameters.push(ParameterDefinition::toggle(
                STEP_GATE_IDS[i],
                STEP_GATE_NAMES[i],
                true,
            ).describe("Plays this step when on; silent when off"));

            // Velocity: 0-127 (default 100)
            parameters.push(ParameterDefinition::new(
                STEP_VELOCITY_IDS[i],
                STEP_VELOCITY_NAMES[i],
                0.0,
                127.0,
                100.0,
                ParameterDisplay::linear(""),
            ).describe("Velocity for this step, 0 to 127"));
        }

        // Added after the per-step parameters, so older parameter indices
        // stay where they were
        parameters.push(ParameterDefinition::choice(
            "gate_mode",
            "Gate Mode",
            &["Step", "100 ms"],
            GateMode::Step as usize,
        ).describe("What Gate Length is a share of: the time between clock pulses, or a fixed 100 ms"));
        for i in 0..MAX_STEPS {
            parameters.push(ParameterDefinition::toggle(
                STEP_TIE_IDS[i],
                STEP_TIE_NAMES[i],
                false,
            ).describe("Holds this step's note into the next step, which continues it without a new attack"));
        }

        // The Chain and patterns B to D, after everything the sequencer had
        // before it had patterns
        for slot in 0..CHAIN_SLOTS {
            parameters.push(
                ParameterDefinition::choice(CHAIN_IDS[slot], CHAIN_NAMES[slot], &CHAIN_CHOICES, usize::from(slot == 0))
                    .describe("A pattern in the chain, which plays its patterns a pass each, in order, then repeats"),
            );
        }
        let names = &*PATTERN_PARAM_NAMES;
        for (index, field) in FIELDS.iter().cycle().take((PATTERNS - 1) * MAX_STEPS * 4).enumerate() {
            let (id, name) = (names.ids[index], names.names[index]);
            parameters.push(match field {
                StepField::Pitch => ParameterDefinition::new(id, name, 0.0, 127.0, 60.0, ParameterDisplay::linear(""))
                    .describe("Note for this step as a MIDI number; 60 is middle C"),
                StepField::Gate => ParameterDefinition::toggle(id, name, true).describe("Plays this step when on; silent when off"),
                StepField::Velocity => ParameterDefinition::new(id, name, 0.0, 127.0, 100.0, ParameterDisplay::linear(""))
                    .describe("Velocity for this step, 0 to 127"),
                StepField::Tie => ParameterDefinition::toggle(id, name, false)
                    .describe("Holds this step's note into the next step, which continues it without a new attack"),
                StepField::Slide | StepField::Chance => unreachable!("slides and chances have their own blocks"),
            });
        }

        // Glide and every pattern's slides, after the patterns
        parameters.push(
            ParameterDefinition::new("glide", "Glide", 0.0, 1.0, 0.06, ParameterDisplay::logarithmic("s"))
                .describe("How long a slide step takes to glide to its note"),
        );
        let slides = STEP_SLIDE_IDS.iter().zip(STEP_SLIDE_NAMES).chain(names.slide_ids.iter().zip(names.slide_names.iter().copied()));
        for (id, name) in slides {
            parameters.push(ParameterDefinition::toggle(id, name, false).describe(
                "Slurs into this step: Pitch glides from the note before and the gate stays high, so the envelope isn't struck again",
            ));
        }

        // Wander's scale, and then every pattern's chances, last of all
        parameters.push(
            ParameterDefinition::choice("scale", "Scale", &SCALE_NAMES[..CUSTOM_SCALE], 6)
                .describe("The scale Wander moves notes along"),
        );
        parameters.push(ParameterDefinition::choice("root", "Root", &NOTE_NAMES, 0).describe("The scale's first note"));
        parameters.push(
            ParameterDefinition::choice("keep", "Keep", &["Off", "Step 1", "Accents"], Keep::First as usize)
                .describe("Notes Wander leaves as written: step 1, or steps with a Velocity of 110 or more"),
        );
        let chances = STEP_CHANCE_IDS.iter().zip(STEP_CHANCE_NAMES).chain(names.chance_ids.iter().zip(names.chance_names.iter().copied()));
        for (id, name) in chances {
            parameters.push(
                ParameterDefinition::new(id, name, 0.0, 100.0, 100.0, ParameterDisplay::linear("%"))
                    .describe("How often this step plays; when it doesn't, it's a rest"),
            );
        }
        debug_assert_eq!(parameters.len(), Self::PARAM_COUNT);

        Self {
            current_step: 0,
            ping_pong_direction: 1,
            reset_pending: true,
            clock: Edge::LOW,
            reset: Edge::LOW,
            gate_timer: 0,
            tied: false,
            gate_high: false,
            playing_note: false,
            glide: Glide::NEW,
            sliding: false,
            timer: StepTimer::new(),
            eoc: Pulse::IDLE,
            random_state: 12345, // Seed for PRNG
            next_random: None,
            audition: None,
            auditioning: false,
            audition_struck: false,
            audition_gate: 0,
            pattern: 0,
            chain_slot: 0,
            pass_clocks: 0,
            next_pattern: 0,
            pattern_cv: false,
            // The graph seeds it, so a patch wanders the same way everywhere
            rng: Pcg32::new(0),
            wander: 0,
            dropped: false,
            sample_rate: 44100.0,
            ports,
            parameters,
        }
    }

    /// Port index constants.
    const PORT_CLOCK: usize = 0;
    const PORT_RESET: usize = 1;
    const PORT_RUN: usize = 2;
    const PORT_PATTERN: usize = 3;
    const PORT_WANDER: usize = 4;
    const PORT_PITCH: usize = 0;
    const PORT_GATE: usize = 1;
    const PORT_VELOCITY: usize = 2;
    const PORT_STEP: usize = 3;
    const PORT_EOC: usize = 4;

    /// Parameter index constants for global params. Wander is first, for
    /// its jack's sake.
    const PARAM_WANDER: usize = 0;
    pub const PARAM_STEPS: usize = 1;
    const PARAM_DIRECTION: usize = 2;
    const PARAM_GATE_LENGTH: usize = 3;
    /// The per-step pitch, gate and velocity start here.
    const PARAM_STEPS_A: usize = 4;
    /// The most degrees a note can wander.
    const WANDER_MAX: f32 = 4.0;

    /// Get parameter index for step pitch (0-indexed step).
    const fn step_pitch_param(step: usize) -> usize {
        Self::PARAM_STEPS_A + step * 3
    }

    /// Get parameter index for step gate (0-indexed step).
    const fn step_gate_param(step: usize) -> usize {
        Self::PARAM_STEPS_A + step * 3 + 1
    }

    /// Get parameter index for step velocity (0-indexed step).
    const fn step_velocity_param(step: usize) -> usize {
        Self::PARAM_STEPS_A + step * 3 + 2
    }

    /// Gate Mode comes after the per-step pitch, gate and velocity.
    const PARAM_GATE_MODE: usize = Self::PARAM_STEPS_A + MAX_STEPS * 3;

    /// Get parameter index for step tie (0-indexed step).
    const fn step_tie_param(step: usize) -> usize {
        Self::PARAM_GATE_MODE + 1 + step
    }

    /// The first of the eight Chain slots, after the ties.
    pub const PARAM_CHAIN: usize = Self::step_tie_param(MAX_STEPS);
    /// Patterns B to D's steps, four parameters a step, after the Chain.
    const PARAM_PATTERNS: usize = Self::PARAM_CHAIN + CHAIN_SLOTS;
    /// Glide, after the patterns.
    const PARAM_GLIDE: usize = Self::PARAM_PATTERNS + (PATTERNS - 1) * MAX_STEPS * 4;
    /// Every pattern's slides, A's first, after Glide.
    const PARAM_SLIDES: usize = Self::PARAM_GLIDE + 1;
    /// Wander's Scale, Root and Keep, after the slides.
    const PARAM_SCALE: usize = Self::PARAM_SLIDES + PATTERNS * MAX_STEPS;
    const PARAM_ROOT: usize = Self::PARAM_SCALE + 1;
    const PARAM_KEEP: usize = Self::PARAM_ROOT + 1;
    /// Every pattern's chances, A's first, last of all.
    const PARAM_CHANCES: usize = Self::PARAM_KEEP + 1;
    pub const PARAM_COUNT: usize = Self::PARAM_CHANCES + PATTERNS * MAX_STEPS;

    /// The parameter holding one of a step's fields (all 0-based).
    pub const fn step_param(pattern: usize, step: usize, field: StepField) -> usize {
        if let StepField::Slide = field {
            Self::PARAM_SLIDES + pattern * MAX_STEPS + step
        } else if let StepField::Chance = field {
            Self::PARAM_CHANCES + pattern * MAX_STEPS + step
        } else if pattern == 0 {
            match field {
                StepField::Pitch => Self::step_pitch_param(step),
                StepField::Gate => Self::step_gate_param(step),
                StepField::Velocity => Self::step_velocity_param(step),
                StepField::Tie => Self::step_tie_param(step),
                StepField::Slide | StepField::Chance => unreachable!(),
            }
        } else {
            Self::PARAM_PATTERNS + ((pattern - 1) * MAX_STEPS + step) * 4 + field as usize
        }
    }

    /// The name of the parameter holding a step's field: "Step 3 Pitch" in
    /// pattern A, "Step B3 Pitch" in B.
    pub fn step_param_name(pattern: usize, step: usize, field: StepField) -> &'static str {
        if pattern == 0 {
            match field {
                StepField::Pitch => STEP_PITCH_NAMES[step],
                StepField::Gate => STEP_GATE_NAMES[step],
                StepField::Velocity => STEP_VELOCITY_NAMES[step],
                StepField::Tie => STEP_TIE_NAMES[step],
                StepField::Slide => STEP_SLIDE_NAMES[step],
                StepField::Chance => STEP_CHANCE_NAMES[step],
            }
        } else if field == StepField::Slide {
            PATTERN_PARAM_NAMES.slide_names[(pattern - 1) * MAX_STEPS + step]
        } else if field == StepField::Chance {
            PATTERN_PARAM_NAMES.chance_names[(pattern - 1) * MAX_STEPS + step]
        } else {
            PATTERN_PARAM_NAMES.names[((pattern - 1) * MAX_STEPS + step) * 4 + field as usize]
        }
    }

    /// The patterns the Chain plays, in order, and how many.
    pub fn chain(params: &[f32]) -> ([usize; CHAIN_SLOTS], usize) {
        chain_from(&params[Self::PARAM_CHAIN..Self::PARAM_CHAIN + CHAIN_SLOTS])
    }

    /// The pattern for the Chain slot playing, or for the Pattern CV.
    fn pick(&mut self, params: &[f32], cv: Option<f32>) -> usize {
        match cv {
            Some(cv) => TriggerSequencer::pattern_for_cv(cv),
            None => {
                let (chain, len) = Self::chain(params);
                self.chain_slot %= len;
                chain[self.chain_slot]
            }
        }
    }

    /// The pattern the next pass will play.
    fn coming_pattern(&self, params: &[f32], cv: Option<f32>) -> usize {
        match cv {
            Some(cv) => TriggerSequencer::pattern_for_cv(cv),
            None => {
                let (chain, len) = Self::chain(params);
                if self.reset_pending { chain[0] } else { chain[(self.chain_slot + 1) % len] }
            }
        }
    }

    /// Readout values: the pattern, + 4 × the Chain slot, + 32 once a step
    /// has played.
    pub const READOUT_PATTERN: usize = 0;
    /// The next pass's pattern, + 64 while Pattern is patched.
    pub const READOUT_NEXT: usize = 1;
    /// How many degrees the note playing has wandered from what's written.
    pub const READOUT_WANDER: usize = 2;
    /// 1 when the step the clock last reached lost its dice and rests.
    pub const READOUT_DROPPED: usize = 3;

    /// Whether a step with this Chance (in percent) plays this time. Only
    /// throws the dice when it might not.
    fn plays(&mut self, chance: f32) -> bool {
        chance >= 100.0 || (chance > 0.0 && self.rng.chance(f64::from(chance) / 100.0))
    }

    /// How many degrees a note struck now wanders, at Wander `amount`: `x`
    /// evenly from `-amount` to `amount`, then rounded down or up at random,
    /// in proportion, so a small amount moves a note one degree now and then.
    fn wander_degrees(&mut self, amount: f32) -> i32 {
        if amount <= 0.0 {
            return 0;
        }
        let x = self.rng.bipolar() * amount;
        let up = 0.5 + 0.5 * self.rng.bipolar();
        (x + up).floor() as i32
    }

    /// How long a new note's gate stays high, in samples.
    ///
    /// Gate Length is a share of the coming step (see `coming_step`), or of
    /// 100 ms in the Fixed mode and until two clock edges have been seen. A
    /// gate held until the next clock (a tie, or 100% of the step) still
    /// ends after two steps, so a clock that stops doesn't leave a note
    /// hanging.
    fn gate_samples(&self, mode: GateMode, gate_length: f32, tied: bool) -> usize {
        let fixed = self.sample_rate * 0.1;
        let step = self.timer.coming_step();
        let held = 2 * step.unwrap_or(fixed as usize);
        if tied {
            return held;
        }
        let samples = match (mode, step) {
            (GateMode::Step, Some(_)) if gate_length >= 1.0 => held,
            (GateMode::Step, Some(step)) => (step as f32 * gate_length) as usize,
            _ => (fixed * gate_length) as usize,
        };
        samples.max(1)
    }

    /// How long an audition's gate holds at least: a Gate Length of the
    /// step the clock last measured, or of 100 ms. Never more than the
    /// step, so a click at 100% is one step's note.
    fn audition_samples(&self, mode: GateMode, gate_length: f32) -> usize {
        let samples = match (mode, self.timer.coming_step()) {
            (GateMode::Step, Some(step)) => step as f32 * gate_length.min(1.0),
            _ => self.sample_rate * 0.1 * gate_length.min(1.0),
        };
        (samples as usize).max(1)
    }

    /// Gate threshold for edge detection.
    /// How long the EOC trigger lasts, in seconds.
    const EOC_SECONDS: f32 = 0.001;

    /// Simple xorshift PRNG for random mode.
    fn next_random(&mut self) -> u32 {
        let mut x = self.random_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random_state = x;
        x
    }

    /// The random number the next step will be chosen by, drawn only once.
    fn coming_random(&mut self) -> u32 {
        match self.next_random {
            Some(r) => r,
            None => {
                let r = self.next_random();
                self.next_random = Some(r);
                r
            }
        }
    }

    /// Where the next clock takes the sequence, without moving it there: the
    /// step, the ping-pong direction after it, and whether it passes the end.
    fn following_step(&mut self, num_steps: usize, direction: SequenceDirection) -> (usize, i32, bool) {
        let current = self.current_step;
        let ping_pong = self.ping_pong_direction;
        match direction {
            SequenceDirection::Forward => ((current + 1) % num_steps, ping_pong, current >= num_steps - 1),
            SequenceDirection::Backward => {
                let step = if current == 0 { num_steps - 1 } else { current - 1 };
                (step, ping_pong, current == 0)
            }
            SequenceDirection::PingPong => {
                let next = current as i32 + ping_pong;
                if next >= num_steps as i32 {
                    // Hit end, reverse direction
                    (if num_steps > 1 { num_steps - 2 } else { 0 }, -1, true)
                } else if next < 0 {
                    // Hit start, reverse direction
                    (if num_steps > 1 { 1 } else { 0 }, 1, true)
                } else {
                    (next as usize, ping_pong, false)
                }
            }
            // No EOC in random mode
            SequenceDirection::Random => ((self.coming_random() as usize) % num_steps, ping_pong, false),
        }
    }

    /// Advance to the next step based on direction mode.
    fn advance_step(&mut self, num_steps: usize, direction: SequenceDirection) -> bool {
        let (step, ping_pong, was_at_end) = self.following_step(num_steps, direction);
        self.current_step = step;
        self.ping_pong_direction = ping_pong;
        if direction == SequenceDirection::Random {
            self.next_random = None;
        }
        was_at_end
    }

    /// Whether the next clock plays a note that slides in from this one: its
    /// step, in the pattern it will be in. The Pattern CV is read as it is
    /// now, so one that moves before then can still change it.
    fn slide_follows(&mut self, params: &[f32], num_steps: usize, direction: SequenceDirection, cv: Option<f32>) -> bool {
        let (step, _, hit_end) = self.following_step(num_steps, direction);
        let pass_over = match direction {
            SequenceDirection::Random => self.pass_clocks + 1 >= num_steps,
            _ => hit_end,
        };
        let pattern = if pass_over { self.coming_pattern(params, cv) } else { self.pattern };
        step < MAX_STEPS
            && params[Self::step_param(pattern, step, StepField::Gate)] > 0.5
            && params[Self::step_param(pattern, step, StepField::Slide)] > 0.5
    }

    /// Get the current step's data.
    pub fn current_step(&self) -> usize {
        self.current_step
    }
}

impl Default for StepSequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for StepSequencer {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Step Sequencer",
            category: ModuleCategory::Utility,
            description: "16-step sequencer with pitch, gate, and velocity per step, in four patterns played in a chain",
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
        // Get global parameters
        let num_steps = (params[Self::PARAM_STEPS] as usize).clamp(1, MAX_STEPS);
        let direction = SequenceDirection::from_param(params[Self::PARAM_DIRECTION]);
        let gate_length_percent = params[Self::PARAM_GATE_LENGTH] / 100.0;
        let gate_mode = GateMode::from_param(params[Self::PARAM_GATE_MODE]);
        let glide = Glide::coefficient(params[Self::PARAM_GLIDE], self.sample_rate);
        let scale = (params[Self::PARAM_SCALE].round().max(0.0) as usize).min(CUSTOM_SCALE - 1);
        let key = rotate_to_key(scale_mask(scale, 0), params[Self::PARAM_ROOT].round() as i32);
        let keep = Keep::from_param(params[Self::PARAM_KEEP]);

        // Get input buffers
        let clock_in = inputs.get(Self::PORT_CLOCK);
        let reset_in = inputs.get(Self::PORT_RESET);
        let run_in = inputs.get(Self::PORT_RUN);
        let pattern_in = connected_input(inputs, Self::PORT_PATTERN);
        let wander_in = connected_input(inputs, Self::PORT_WANDER);
        self.pattern_cv = pattern_in.is_some();
        let mut audition_dip = std::mem::take(&mut self.audition_struck);
        if audition_dip {
            self.audition_gate = self.audition_samples(gate_mode, gate_length_percent);
        }

        // Process each sample
        for i in 0..context.block_size {
            // Get input values
            let clock_rising = self.clock.rising(clock_in.map_or(0.0, |buf| buf.at(i)));
            let reset_rising = self.reset.rising(reset_in.map_or(0.0, |buf| buf.at(i)));
            let is_running = run_in.map_or(1.0, |buf| buf.at(i)) > GATE_THRESHOLD;
            let cv = pattern_in.map(|buf| buf.at(i));

            // Handle reset
            if reset_rising {
                self.current_step = direction.start_step(num_steps);
                self.ping_pong_direction = 1;
                self.reset_pending = true;
                self.gate_timer = 0;
                self.tied = false;
                // The step after a reset has no note to slide from
                self.playing_note = false;
                self.sliding = false;
                self.timer.forget_gap();
            }

            // Every clock edge measures the step, running or not
            if clock_rising {
                self.timer.clock();
            }
            let mut retrigger = false;

            // Handle clock advance. The first clock after a reset plays the
            // start step of the chain's first pattern rather than moving
            // past it, so a reset on the downbeat puts step 1 on the downbeat
            if clock_rising && is_running {
                let end_of_chain = if self.reset_pending {
                    self.reset_pending = false;
                    self.current_step = direction.start_step(num_steps);
                    self.chain_slot = 0;
                    self.pass_clocks = 0;
                    self.pattern = self.pick(params, cv);
                    false
                } else {
                    let hit_end = self.advance_step(num_steps, direction);
                    // A pass through the pattern ends where the sequence
                    // wraps or turns; in random order, after Steps clocks
                    self.pass_clocks += 1;
                    let pass_over = match direction {
                        SequenceDirection::Random => self.pass_clocks >= num_steps,
                        _ => hit_end,
                    };
                    let mut chain_over = false;
                    if pass_over {
                        self.pass_clocks = 0;
                        self.chain_slot += 1;
                        if self.chain_slot >= Self::chain(params).1 {
                            self.chain_slot = 0;
                            chain_over = true;
                        }
                        self.pattern = self.pick(params, cv);
                    }
                    // With the Pattern CV choosing, every pass is the whole
                    // sequence
                    hit_end && (chain_over || cv.is_some())
                };
                let pattern = self.pattern;

                // A step that plays starts a note, or continues the last one
                // if that was tied, or slides on from it. A rest ends the
                // note, and so does a step whose dice say it rests this time
                let step_gate = params[Self::step_param(pattern, self.current_step, StepField::Gate)] > 0.5;
                self.dropped = step_gate && !self.plays(params[Self::step_param(pattern, self.current_step, StepField::Chance)]);
                if step_gate && !self.dropped {
                    let tie = params[Self::step_param(pattern, self.current_step, StepField::Tie)] > 0.5;
                    let slide = params[Self::step_param(pattern, self.current_step, StepField::Slide)] > 0.5;
                    // Fixed gates never dipped, so old patches whose notes
                    // overlap still run them together
                    retrigger = gate_mode == GateMode::Step && self.gate_high && !self.tied && !slide;
                    // A slide glides from the note before. A note held on by
                    // a tie keeps gliding if it was
                    self.sliding = (slide && self.playing_note) || (self.tied && self.sliding);
                    // A tie carries on where its note wandered to
                    if !self.tied {
                        let velocity = params[Self::step_param(pattern, self.current_step, StepField::Velocity)];
                        let kept = match keep {
                            Keep::Off => false,
                            Keep::First => self.current_step == 0,
                            Keep::Accents => velocity >= Keep::ACCENT,
                        };
                        let amount = params[Self::PARAM_WANDER] + wander_in.map_or(0.0, |buf| buf.at(i) * Self::WANDER_MAX);
                        self.wander = if kept { 0 } else { self.wander_degrees(amount.clamp(0.0, Self::WANDER_MAX)) };
                    }
                    let written = params[Self::step_param(pattern, self.current_step, StepField::Pitch)] as u8;
                    let note = wander_note(written, self.wander, key);
                    self.glide.start(note as f32, self.sliding);
                    // A note held into a slide joins it with no gap, like a tie
                    let held = tie || self.slide_follows(params, num_steps, direction, cv);
                    self.gate_timer = self.gate_samples(gate_mode, gate_length_percent, held);
                    self.tied = tie;
                    self.playing_note = true;
                    // The sequence takes over from an audition let go of
                    if !self.auditioning {
                        self.audition = None;
                        self.audition_gate = 0;
                    }
                } else {
                    self.gate_timer = 0;
                    self.tied = false;
                    self.playing_note = false;
                    self.sliding = false;
                    self.wander = 0;
                }

                // Fire EOC pulse once the chain has played through (never in
                // random order, which has no end)
                if end_of_chain {
                    self.eoc.fire(Self::EOC_SECONDS, self.sample_rate);
                }
            }

            // Ensure current step is within bounds (in case num_steps changed)
            if self.current_step >= num_steps {
                self.current_step = 0;
            }

            // Get current step's data
            let written = params[Self::step_param(self.pattern, self.current_step, StepField::Pitch)] as u8;
            let step_pitch = if self.wander == 0 { written } else { wander_note(written, self.wander, key) };
            let step_gate_enabled = params[Self::step_param(self.pattern, self.current_step, StepField::Gate)] > 0.5;
            let step_velocity = params[Self::step_param(self.pattern, self.current_step, StepField::Velocity)] / 127.0;

            // Generate outputs (access directly by index to avoid multiple mutable borrows).
            // Pitch lands on the note unless it's sliding there
            let coefficient = if self.sliding { glide } else { 1.0 };
            outputs[Self::PORT_PITCH].samples[i] = match self.audition {
                // A slide after it glides from the audition's note
                Some(audition) => {
                    self.glide.start(audition.notes[0] as f32, false);
                    note_to_voct(audition.notes[0] as f32)
                }
                None => note_to_voct(self.glide.next(step_pitch as f32, coefficient)),
            };

            // Gate output: high if timer > 0 and step gate is enabled. A new
            // note over a gate that's still high dips for this one sample,
            // so it starts with a rising edge
            let gate_active = match self.audition {
                Some(_) => (self.auditioning || self.audition_gate > 0) && !std::mem::take(&mut audition_dip),
                None => self.gate_timer > 0 && step_gate_enabled && !retrigger,
            };
            outputs[Self::PORT_GATE].samples[i] = if gate_active { 1.0 } else { 0.0 };
            self.gate_high = gate_active;

            outputs[Self::PORT_VELOCITY].samples[i] = self.audition.map_or(step_velocity, |audition| audition.velocity);

            // Step output: current step as 0-1 value
            outputs[Self::PORT_STEP].samples[i] = self.current_step as f32 / (num_steps - 1).max(1) as f32;

            // EOC output
            outputs[Self::PORT_EOC].samples[i] = self.eoc.sample();

            // Decrement timers
            if self.gate_timer > 0 {
                self.gate_timer -= 1;
            }
            self.audition_gate = self.audition_gate.saturating_sub(1);
            self.timer.tick();
        }

        let last_cv = pattern_in.and_then(|buf| buf.samples.get(context.block_size.saturating_sub(1)).copied());
        self.next_pattern = self.coming_pattern(params, last_cv);
    }

    fn reset(&mut self) {
        self.current_step = 0;
        self.ping_pong_direction = 1;
        self.reset_pending = true;
        self.clock.reset();
        self.reset.reset();
        self.gate_timer = 0;
        self.tied = false;
        self.gate_high = false;
        self.playing_note = false;
        self.glide = Glide::NEW;
        self.sliding = false;
        self.timer.forget();
        self.eoc.clear();
        self.audition = None;
        self.auditioning = false;
        self.audition_struck = false;
        self.audition_gate = 0;
        self.pattern = 0;
        self.chain_slot = 0;
        self.pass_clocks = 0;
        self.wander = 0;
        self.dropped = false;
    }

    fn seed(&mut self, seed: u64) {
        self.rng = Pcg32::new(seed);
    }

    /// A note played in on the face's piano sounds at once, with the clock
    /// stopped too, and holds while the key's down. A new key under the
    /// same hand moves Pitch without striking again. When it lifts, the
    /// sequence's own gate is let go of, so the next note it strikes, not
    /// one struck while the hand was down, is what's heard after.
    fn audition(&mut self, note: Option<Audition>) {
        match note.filter(|note| !note.notes().is_empty()) {
            Some(note) => {
                if !self.auditioning {
                    self.audition_struck = true;
                }
                self.audition = Some(note);
                self.auditioning = true;
            }
            None if self.auditioning => {
                self.auditioning = false;
                self.gate_timer = 0;
                self.tied = false;
            }
            None => {}
        }
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let position = PatternPosition {
            pattern: self.pattern,
            chain_slot: self.chain_slot,
            started: !self.reset_pending,
            next_pattern: self.next_pattern,
            pattern_cv: self.pattern_cv,
        };
        position.write(&mut readout, Self::READOUT_PATTERN);
        readout.values[Self::READOUT_WANDER] = self.wander as f32;
        readout.values[Self::READOUT_DROPPED] = if self.dropped { 1.0 } else { 0.0 };
        Some(readout)
    }
}

/// Where a Step or Chord Sequencer is in its chain, read back from its
/// [`Readout`]: two values, the pattern and then the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatternPosition {
    pub pattern: usize,
    pub chain_slot: usize,
    /// A step has played since the start or the last reset.
    pub started: bool,
    pub next_pattern: usize,
    /// Pattern is patched, so the CV picks patterns rather than the Chain.
    pub pattern_cv: bool,
}

impl PatternPosition {
    /// A Step Sequencer's.
    pub fn from_readout(readout: &Readout) -> Self {
        Self::read(readout, StepSequencer::READOUT_PATTERN)
    }

    /// Written into a readout from `at`: the pattern, plus 4 × the Chain
    /// slot, plus 32 once a step has played; then the next pass's pattern,
    /// plus 64 while Pattern is patched.
    pub fn write(&self, readout: &mut Readout, at: usize) {
        let started = if self.started { 32 } else { 0 };
        readout.values[at] = (self.pattern + 4 * self.chain_slot + started) as f32;
        let patched = if self.pattern_cv { 64 } else { 0 };
        readout.values[at + 1] = (self.next_pattern + patched) as f32;
    }

    /// Read back from a readout, from `at`, as [`Self::write`] puts it.
    pub fn read(readout: &Readout, at: usize) -> Self {
        let now = readout.values[at] as usize;
        let next = readout.values[at + 1] as usize;
        Self {
            pattern: now % 4,
            chain_slot: (now / 4) % 8,
            started: now & 32 != 0,
            next_pattern: next % 4,
            pattern_cv: next & 64 != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every parameter at its default (C4, gate on, velocity 100, no ties,
    /// Step gate mode), with the given Steps, Direction and Gate Length.
    fn params_with(steps: f32, direction: f32, gate_length: f32) -> Vec<f32> {
        let mut params: Vec<f32> = StepSequencer::new().parameters().iter().map(|p| p.default).collect();
        params[StepSequencer::PARAM_STEPS] = steps;
        params[StepSequencer::PARAM_DIRECTION] = direction;
        params[StepSequencer::PARAM_GATE_LENGTH] = gate_length;
        params
    }

    #[test]
    fn test_sequencer_info() {
        let seq = StepSequencer::new();
        assert_eq!(seq.info().id, "seq.step");
        assert_eq!(seq.info().name, "Step Sequencer");
        assert_eq!(seq.info().category, ModuleCategory::Utility);
    }

    #[test]
    fn test_sequencer_ports() {
        let seq = StepSequencer::new();
        let ports = seq.ports();

        // 5 inputs + 5 outputs = 10 ports
        assert_eq!(ports.len(), 10);

        // Inputs
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "clock");
        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "reset");
        assert!(ports[2].is_input());
        assert_eq!(ports[2].id, "run");
        assert!(ports[3].is_input());
        assert_eq!(ports[3].id, "pattern");
        assert!(ports[4].is_input());
        assert_eq!(ports[4].id, "wander_cv");

        // Outputs
        assert!(ports[5].is_output());
        assert_eq!(ports[5].id, "pitch");
        assert!(ports[6].is_output());
        assert_eq!(ports[6].id, "gate");
        assert!(ports[7].is_output());
        assert_eq!(ports[7].id, "velocity");
        assert!(ports[8].is_output());
        assert_eq!(ports[8].id, "step_out");
        assert!(ports[9].is_output());
        assert_eq!(ports[9].id, "eoc");
    }

    #[test]
    fn test_sequencer_parameters() {
        let seq = StepSequencer::new();
        let params = seq.parameters();

        // Wander, 3 global + 16 steps * 3 params each, then Gate Mode and 16
        // ties, then the Chain and patterns B to D, then Glide and every
        // slide, then Scale, Root, Keep and every chance
        assert_eq!(params.len(), StepSequencer::PARAM_COUNT);
        assert_eq!(params.len(), 4 + MAX_STEPS * 3 + 1 + MAX_STEPS + CHAIN_SLOTS + 3 * MAX_STEPS * 4 + 1 + 4 * MAX_STEPS + 3 + 4 * MAX_STEPS);
        assert_eq!(params[StepSequencer::PARAM_GATE_MODE].id, "gate_mode");
        assert_eq!(params[StepSequencer::step_tie_param(0)].id, "step_1_tie");
        assert_eq!(params[StepSequencer::step_tie_param(15)].id, "step_16_tie");
        assert_eq!(params[StepSequencer::PARAM_CHAIN].name, "Chain 1");
        assert_eq!(params[StepSequencer::PARAM_CHAIN].default, 1.0, "the chain starts as just A");
        assert_eq!(params[StepSequencer::PARAM_CHAIN + 7].default, 0.0);

        // Global params
        assert_eq!(params[0].id, "wander");
        assert_eq!(params[1].id, "steps");
        assert_eq!(params[2].id, "direction");
        assert_eq!(params[3].id, "gate_length");

        // First step params
        assert_eq!(params[4].id, "step_1_pitch");
        assert_eq!(params[5].id, "step_1_gate");
        assert_eq!(params[6].id, "step_1_velocity");
        assert_eq!(params[StepSequencer::PARAM_SCALE].id, "scale");
        assert_eq!(params[StepSequencer::PARAM_KEEP].id, "keep");
    }

    #[test]
    fn test_direction_conversion() {
        assert_eq!(SequenceDirection::from_param(0.0), SequenceDirection::Forward);
        assert_eq!(SequenceDirection::from_param(1.0), SequenceDirection::Backward);
        assert_eq!(SequenceDirection::from_param(2.0), SequenceDirection::PingPong);
        assert_eq!(SequenceDirection::from_param(3.0), SequenceDirection::Random);
        assert_eq!(SequenceDirection::from_param(99.0), SequenceDirection::Forward);
    }

    #[test]
    fn test_note_to_voct() {
        // C4 (60) = 0V
        assert!((note_to_voct(60.0) - 0.0).abs() < 0.001);
        // C5 (72) = +1V
        assert!((note_to_voct(72.0) - 1.0).abs() < 0.001);
        // C3 (48) = -1V
        assert!((note_to_voct(48.0) - -1.0).abs() < 0.001);
    }

    #[test]
    fn test_sequencer_advances_on_clock() {
        let mut seq = StepSequencer::new();
        seq.prepare(44100.0, 256);

        // Create clock pulse
        let mut clock = SignalBuffer::control(256);
        // Rising edge at sample 50
        for i in 50..100 {
            clock.samples[i] = 1.0;
        }

        let mut outputs = vec![
            SignalBuffer::control(256), // pitch
            SignalBuffer::control(256), // gate
            SignalBuffer::control(256), // velocity
            SignalBuffer::control(256), // step
            SignalBuffer::control(256), // eoc
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        // Default params: 8 steps, forward, 50% gate
        let params = params_with(8.0, 0.0, 50.0);

        // The first clock plays the first step rather than moving past it
        seq.process(&[&clock], &mut outputs, &params, &ctx);
        assert_eq!(seq.current_step(), 0);
        assert_eq!(outputs[1].samples[60], 1.0, "step 1's gate fires");

        // The next clock advances
        seq.process(&[&clock], &mut outputs, &params, &ctx);
        assert_eq!(seq.current_step(), 1);
    }

    /// One sample of clock and reset into a one-sample block.
    fn tick(seq: &mut StepSequencer, params: &[f32], clock: bool, reset: bool) -> f32 {
        let mut clock_buf = SignalBuffer::control(1);
        clock_buf.samples[0] = if clock { 1.0 } else { 0.0 };
        let mut reset_buf = SignalBuffer::control(1);
        reset_buf.samples[0] = if reset { 1.0 } else { 0.0 };
        let mut outputs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(1)).collect();
        seq.process(&[&clock_buf, &reset_buf], &mut outputs, params, &ProcessContext::new(44100.0, 1));
        outputs[1].samples[0]
    }

    #[test]
    fn test_first_clock_after_reset_plays_start_step() {
        for (direction, start) in [(0.0, 0), (1.0, 3), (2.0, 0), (3.0, 0)] {
            let mut seq = StepSequencer::new();
            seq.prepare(44100.0, 1);
            let params = params_with(4.0, direction, 50.0);

            // Run a few steps in, then reset
            for _ in 0..3 {
                tick(&mut seq, &params, true, false);
                tick(&mut seq, &params, false, false);
            }
            tick(&mut seq, &params, false, true);
            tick(&mut seq, &params, false, false);

            // The next clock sounds the start step
            let gate = tick(&mut seq, &params, true, false);
            assert_eq!(seq.current_step(), start, "direction {direction}");
            assert_eq!(gate, 1.0, "direction {direction}");
        }
    }

    #[test]
    fn test_sequencer_reset() {
        let mut seq = StepSequencer::new();
        seq.prepare(44100.0, 256);

        // Manually advance
        seq.current_step = 5;

        // Create reset pulse
        let mut reset = SignalBuffer::control(256);
        for i in 50..100 {
            reset.samples[i] = 1.0;
        }

        let clock = SignalBuffer::control(256);
        let mut outputs = vec![
            SignalBuffer::control(256),
            SignalBuffer::control(256),
            SignalBuffer::control(256),
            SignalBuffer::control(256),
            SignalBuffer::control(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        let params = params_with(8.0, 0.0, 50.0);

        seq.process(&[&clock, &reset], &mut outputs, &params, &ctx);

        // Should be back at step 0
        assert_eq!(seq.current_step(), 0);
    }

    #[test]
    fn test_sequencer_backward_direction() {
        let mut seq = StepSequencer::new();
        seq.prepare(44100.0, 1);

        let params = params_with(4.0, 1.0, 50.0); // 4 steps, backward

        // Advance through sequence
        let mut clock_high = SignalBuffer::control(1);
        clock_high.samples[0] = 1.0;
        let clock_low = SignalBuffer::control(1);
        let mut outputs = vec![
            SignalBuffer::control(1),
            SignalBuffer::control(1),
            SignalBuffer::control(1),
            SignalBuffer::control(1),
            SignalBuffer::control(1),
        ];
        let ctx = ProcessContext::new(44100.0, 1);

        // Initial state: step 0
        assert_eq!(seq.current_step(), 0);

        // First clock pulse: backward starts from the last step
        seq.process(&[&clock_high], &mut outputs, &params, &ctx);
        assert_eq!(seq.current_step(), 3);

        // Clock low (no advance)
        seq.process(&[&clock_low], &mut outputs, &params, &ctx);
        assert_eq!(seq.current_step(), 3);

        // Second clock pulse: backward from 3 goes to 2
        seq.process(&[&clock_high], &mut outputs, &params, &ctx);
        assert_eq!(seq.current_step(), 2);

        // Third clock pulse: backward from 2 goes to 1
        seq.process(&[&clock_low], &mut outputs, &params, &ctx);
        seq.process(&[&clock_high], &mut outputs, &params, &ctx);
        assert_eq!(seq.current_step(), 1);
    }

    /// Runs the sequencer at 1 kHz (a sample a millisecond) for `ms`, with a
    /// 1 ms clock pulse at each time in `clocks` and a reset at each time in
    /// `resets`, and returns the Gate output.
    fn gate_over(params: &[f32], ms: usize, clocks: &[usize], resets: &[usize]) -> Vec<f32> {
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, ms);
        let mut clock = SignalBuffer::control(ms);
        let mut reset = SignalBuffer::control(ms);
        for &t in clocks {
            clock.samples[t] = 1.0;
        }
        for &t in resets {
            reset.samples[t] = 1.0;
        }
        let mut outputs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock, &reset], &mut outputs, params, &ProcessContext::new(1000.0, ms));
        outputs[1].samples.clone()
    }

    /// Times the gate rises.
    fn rises(gate: &[f32]) -> Vec<usize> {
        (0..gate.len()).filter(|&t| gate[t] > 0.5 && (t == 0 || gate[t - 1] < 0.5)).collect()
    }

    /// How long the gate stays high from `t`.
    fn high_for(gate: &[f32], t: usize) -> usize {
        gate[t..].iter().take_while(|&&g| g > 0.5).count()
    }

    #[test]
    fn test_gate_length_is_a_share_of_the_step() {
        // 60 BPM in quarter notes is a clock a second; 50% is 500 ms
        let params = params_with(8.0, 0.0, 50.0);
        let gate = gate_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);
        assert_eq!(rises(&gate), [0, 1000, 2000, 3000]);
        // Before a second clock there's no step to measure: 50% of 100 ms
        assert_eq!(high_for(&gate, 0), 50);
        assert_eq!(high_for(&gate, 1000), 500);
        assert_eq!(high_for(&gate, 2000), 500);
    }

    #[test]
    fn test_gates_follow_a_swung_clock() {
        // Straight, a step would be 500 ms; swung at 66%, the steps go
        // 660, 340, 660, 340
        let params = params_with(8.0, 0.0, 50.0);
        let clocks = [0, 660, 1000, 1660, 2000, 2660, 3000, 3660];
        let gate = gate_over(&params, 4000, &clocks, &[]);
        assert_eq!(rises(&gate), clocks);
        // Once the rhythm shows, each note is half of its own step: long on
        // the grid, short on the swung step
        assert_eq!(high_for(&gate, 2000), 330);
        assert_eq!(high_for(&gate, 2660), 170);
        assert_eq!(high_for(&gate, 3000), 330);
    }

    #[test]
    fn test_fixed_gate_mode_keeps_100_ms() {
        let mut params = params_with(8.0, 0.0, 50.0);
        params[StepSequencer::PARAM_GATE_MODE] = GateMode::Fixed as usize as f32;
        let gate = gate_over(&params, 3000, &[0, 1000, 2000], &[]);
        assert_eq!(high_for(&gate, 1000), 50);
        assert_eq!(high_for(&gate, 2000), 50);

        // Notes that overlap the next clock run together, as they always did
        params[StepSequencer::PARAM_GATE_LENGTH] = 99.0;
        let gate = gate_over(&params, 300, &[0, 60, 120], &[]);
        assert_eq!(rises(&gate), [0]);
        assert_eq!(high_for(&gate, 0), 120 + 99);
    }

    #[test]
    fn test_gate_follows_the_tempo() {
        let params = params_with(8.0, 0.0, 50.0);
        let gate = gate_over(&params, 2000, &[0, 1000, 1200, 1400], &[]);
        // Step 2's 500 ms gate is still up at 1200, so step 3 dips it first
        assert_eq!(gate[1200], 0.0);
        assert_eq!(high_for(&gate, 1400), 100, "the step got shorter, so did the gate");
    }

    #[test]
    fn test_full_gate_holds_until_the_next_clock_and_retriggers() {
        let mut params = params_with(4.0, 0.0, 100.0);
        params[StepSequencer::step_gate_param(2)] = 0.0;
        let gate = gate_over(&params, 5000, &[0, 1000, 2000, 3000, 4000], &[]);

        // Step 2 holds right up to step 3's clock, which is a rest
        assert_eq!(high_for(&gate, 1000), 1000);
        assert_eq!(gate[2000], 0.0);
        // Step 4 holds into step 1, which starts a new note: the gate dips
        // for one sample so an envelope sees a new rising edge
        assert_eq!(high_for(&gate, 3000), 1000);
        assert_eq!(gate[4000], 0.0);
        assert_eq!(gate[4001], 1.0);
        assert_eq!(rises(&gate), [0, 1000, 3000, 4001]);
    }

    #[test]
    fn test_tie_carries_the_gate_without_retriggering() {
        let mut params = params_with(4.0, 0.0, 50.0);
        params[StepSequencer::step_tie_param(1)] = 1.0;
        params[StepSequencer::step_pitch_param(2)] = 67.0;
        let gate = gate_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);

        // Step 2 ties into step 3: one rising edge for both, held through
        // step 2 and then for step 3's own 50%
        assert_eq!(rises(&gate), [0, 1000, 3000]);
        assert_eq!(high_for(&gate, 1000), 1500);
    }

    #[test]
    fn test_envelope_is_not_retriggered_across_a_tie() {
        use crate::modules::envelope::AdsrEnvelope;

        // A rest, then step 2 tied into step 3
        let mut params = params_with(3.0, 0.0, 50.0);
        params[StepSequencer::step_gate_param(0)] = 0.0;
        params[StepSequencer::step_tie_param(1)] = 1.0;
        let gate = gate_over(&params, 3000, &[0, 1000, 2000], &[]);

        // A 1.5 s attack from step 2 crosses into step 3. Any dip in the
        // gate would start a release, and a retrigger would restart the
        // attack from where it was and reach the peak late
        let mut env = AdsrEnvelope::new();
        env.prepare(1000.0, 3000);
        let mut env_params: Vec<f32> = env.parameters().iter().map(|p| p.default).collect();
        for (i, p) in env.parameters().iter().enumerate() {
            match p.name {
                "Attack" => env_params[i] = 1.5,
                "Sustain" => env_params[i] = 1.0,
                _ => {}
            }
        }
        let mut gate_buf = SignalBuffer::control(3000);
        gate_buf.samples.copy_from_slice(&gate);
        let mut out: Vec<SignalBuffer> = (0..env.ports().iter().filter(|p| p.is_output()).count())
            .map(|_| SignalBuffer::control(3000))
            .collect();
        env.process(&[&gate_buf], &mut out, &env_params, &ProcessContext::new(1000.0, 3000));
        let level = &out[0].samples;
        assert!((1001..2500).all(|t| level[t] >= level[t - 1]), "rises without a break across the tie");
        assert!(level[2499] > 0.95, "reaches the peak on time: {}", level[2499]);
    }

    #[test]
    fn test_held_gate_ends_when_the_clock_stops() {
        let params = params_with(4.0, 0.0, 100.0);
        // The clock stops after its third pulse
        let gate = gate_over(&params, 5000, &[0, 1000, 2000], &[]);
        assert_eq!(high_for(&gate, 2001), 1999, "held for two steps, then let go");
        assert!(gate[4000..].iter().all(|&g| g == 0.0));
    }

    #[test]
    fn test_reset_does_not_measure_a_stopped_clock() {
        let params = params_with(4.0, 0.0, 50.0);
        // Stops for 3 s and restarts with a reset, at the same tempo
        let gate = gate_over(&params, 6000, &[0, 1000, 4000, 5000], &[4000]);
        assert_eq!(high_for(&gate, 4000), 500);
    }

    #[test]
    fn every_step_field_names_its_own_parameter() {
        let seq = StepSequencer::new();
        let params = seq.parameters();
        for pattern in 0..PATTERNS {
            for step in 0..MAX_STEPS {
                for field in StepField::ALL {
                    let index = StepSequencer::step_param(pattern, step, field);
                    assert_eq!(params[index].name, StepSequencer::step_param_name(pattern, step, field));
                }
            }
        }
        assert_eq!(StepSequencer::step_param_name(0, 2, StepField::Pitch), "Step 3 Pitch");
        assert_eq!(StepSequencer::step_param_name(1, 2, StepField::Pitch), "Step B3 Pitch");
        assert_eq!(StepSequencer::step_param_name(3, 15, StepField::Tie), "Step D16 Tie");
        assert_eq!(params[StepSequencer::step_param(2, 9, StepField::Velocity)].id, "step_c10_velocity");
        assert_eq!(params[StepSequencer::step_param(0, 0, StepField::Slide)].id, "step_1_slide");
        assert_eq!(params[StepSequencer::step_param(3, 15, StepField::Slide)].id, "step_d16_slide");
        assert_eq!(params[StepSequencer::step_param(3, 15, StepField::Chance)].id, "step_d16_chance");
        assert_eq!(params[StepSequencer::PARAM_COUNT - 1].name, "Step D16 Chance");
        // Unique, or saving by name would mix them up
        let mut names: Vec<&str> = params.iter().map(|p| p.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), params.len());
    }

    fn set_chain(params: &mut [f32], patterns: &str) {
        for slot in 0..CHAIN_SLOTS {
            params[StepSequencer::PARAM_CHAIN + slot] = 0.0;
        }
        for (slot, letter) in patterns.chars().enumerate() {
            params[StepSequencer::PARAM_CHAIN + slot] = (letter as u8 - b'A' + 1) as f32;
        }
    }

    /// Gives every step of a pattern one note, so the Pitch output says
    /// which pattern is playing.
    fn fill_pattern(params: &mut [f32], pattern: usize, note: u8) {
        for step in 0..MAX_STEPS {
            params[StepSequencer::step_param(pattern, step, StepField::Pitch)] = note as f32;
        }
    }

    /// Runs the sequencer at 1 kHz with a clock every `period` ms and an
    /// optional Pattern CV, and returns every output.
    fn run_all(params: &[f32], ms: usize, period: usize, pattern_cv: Option<&[f32]>) -> Vec<Vec<f32>> {
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, ms);
        let mut clock = SignalBuffer::control(ms);
        for t in (0..ms).step_by(period) {
            clock.samples[t] = 1.0;
        }
        let reset = SignalBuffer::control(ms);
        let mut run = SignalBuffer::control(ms);
        run.samples.fill(1.0);
        let pattern = match pattern_cv {
            Some(cv) => {
                let mut buf = SignalBuffer::control(ms);
                buf.samples.copy_from_slice(cv);
                buf
            }
            None => SignalBuffer::unconnected(ms, SignalType::Control),
        };
        let mut outputs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock, &reset, &run, &pattern], &mut outputs, params, &ProcessContext::new(1000.0, ms));
        outputs.into_iter().map(|b| b.samples).collect()
    }

    /// The note sounding at each step's clock.
    fn notes_at(pitch: &[f32], period: usize, steps: usize) -> Vec<i32> {
        (0..steps).map(|step| (pitch[step * period] * 12.0).round() as i32 + 60).collect()
    }

    #[test]
    fn a_chain_plays_its_patterns_in_order() {
        let mut params = params_with(4.0, 0.0, 50.0);
        for (pattern, note) in [(0, 60), (1, 62), (2, 64), (3, 65)] {
            fill_pattern(&mut params, pattern, note);
        }
        set_chain(&mut params, "AABC");
        let out = run_all(&params, 32 * 100, 100, None);
        let notes = notes_at(&out[0], 100, 32);
        let line: Vec<i32> = [60, 60, 62, 64].iter().flat_map(|&n| [n; 4]).collect();
        assert_eq!(notes[..16], line[..], "A A B C, four steps each");
        assert_eq!(notes[16..], line[..], "then round again");
    }

    #[test]
    fn eoc_fires_at_the_end_of_the_chain() {
        let mut params = params_with(4.0, 0.0, 50.0);
        set_chain(&mut params, "AB");
        let out = run_all(&params, 32 * 100, 100, None);
        // Steps 0-3 are A, 4-7 B; the chain's end is the clock that wraps
        // back to A
        assert_eq!(rises(&out[4]), [800, 1600, 2400]);

        // A chain of one is the old pattern-length cycle
        set_chain(&mut params, "A");
        let out = run_all(&params, 16 * 100, 100, None);
        assert_eq!(rises(&out[4]), [400, 800, 1200]);
    }

    #[test]
    fn eoc_lasts_a_millisecond_at_any_sample_rate() {
        let params = params_with(4.0, 0.0, 50.0);
        for sample_rate in [44_100.0, 96_000.0] {
            let len = 1000;
            let mut seq = StepSequencer::new();
            seq.prepare(sample_rate, len);
            let mut clock = SignalBuffer::control(len);
            for t in (0..len).step_by(200) {
                clock.samples[t] = 1.0;
            }
            let reset = SignalBuffer::control(len);
            let mut run = SignalBuffer::control(len);
            run.samples.fill(1.0);
            let pattern = SignalBuffer::unconnected(len, SignalType::Control);
            let mut outputs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(len)).collect();
            seq.process(&[&clock, &reset, &run, &pattern], &mut outputs, &params, &ProcessContext::new(sample_rate, len));
            let eoc = &outputs[StepSequencer::PORT_EOC].samples;
            assert_eq!(rises(eoc), [800]);
            assert_eq!(high_for(eoc, 800), (sample_rate / 1000.0).round() as usize, "at {sample_rate} Hz");
        }
    }

    #[test]
    fn a_tie_carries_across_a_pattern_boundary() {
        let mut params = params_with(4.0, 0.0, 50.0);
        fill_pattern(&mut params, 1, 67);
        set_chain(&mut params, "AB");
        // A's last step ties into B's first
        params[StepSequencer::step_param(0, 3, StepField::Tie)] = 1.0;
        let out = run_all(&params, 800, 100, None);
        let gate = &out[1];
        assert_eq!(rises(gate), [0, 100, 200, 300, 500, 600, 700]);
        assert_eq!(high_for(gate, 300), 150, "held through A's last step and half of B's first");
        assert_eq!((out[0][400] * 12.0).round() as i32 + 60, 67, "on B's note");
    }

    #[test]
    fn patterns_follow_the_direction() {
        let mut params = params_with(3.0, 1.0, 50.0);
        for pattern in 0..2 {
            for step in 0..3 {
                params[StepSequencer::step_param(pattern, step, StepField::Pitch)] = (60 + 10 * pattern + step) as f32;
            }
        }
        set_chain(&mut params, "AB");
        let out = run_all(&params, 6 * 100, 100, None);
        // Backward through A, then backward through B
        assert_eq!(notes_at(&out[0], 100, 6), [62, 61, 60, 72, 71, 70]);

        // Ping-pong turns into the next pattern
        params[StepSequencer::PARAM_DIRECTION] = 2.0;
        let out = run_all(&params, 7 * 100, 100, None);
        assert_eq!(notes_at(&out[0], 100, 7), [60, 61, 62, 71, 70, 61, 62]);
    }

    #[test]
    fn random_order_moves_along_the_chain_every_steps_clocks() {
        let mut params = params_with(4.0, 3.0, 50.0);
        fill_pattern(&mut params, 1, 72);
        set_chain(&mut params, "AB");
        let out = run_all(&params, 16 * 100, 100, None);
        let notes = notes_at(&out[0], 100, 16);
        assert_eq!(notes, [[60; 4], [72; 4], [60; 4], [72; 4]].concat());
        assert!(rises(&out[4]).is_empty(), "random order has no end");
    }

    #[test]
    fn pattern_cv_picks_the_pattern_at_each_pass() {
        let mut params = params_with(4.0, 0.0, 50.0);
        for (pattern, note) in [(0, 60), (1, 62), (2, 64), (3, 65)] {
            fill_pattern(&mut params, pattern, note);
        }
        set_chain(&mut params, "AB");
        // The CV moves to C (0.6) halfway through the first pass, and the
        // pattern only changes where the next pass starts
        let mut cv = vec![0.1; 1200];
        cv[200..].fill(0.6);
        let out = run_all(&params, 1200, 100, Some(&cv));
        assert_eq!(notes_at(&out[0], 100, 12), [[60; 4], [64; 4], [64; 4]].concat());
        // Every pass ends the sequence, with no chain to play through
        assert_eq!(rises(&out[4]), [400, 800]);
    }

    #[test]
    fn pattern_cv_changes_on_the_clock_that_starts_a_pass() {
        let mut params = params_with(4.0, 0.0, 50.0);
        fill_pattern(&mut params, 1, 62);
        // An Arranger lane jumps from A to B on the same sample as the
        // clock that starts the second pass
        let mut cv = vec![0.0; 800];
        cv[400..].fill(0.3);
        let out = run_all(&params, 800, 100, Some(&cv));
        assert_eq!((out[0][399] * 12.0).round() as i32 + 60, 60);
        assert_eq!((out[0][400] * 12.0).round() as i32 + 60, 62, "B's first step on the boundary's sample");
    }

    #[test]
    fn reset_starts_the_chain_over() {
        let mut params = params_with(2.0, 0.0, 50.0);
        fill_pattern(&mut params, 1, 62);
        set_chain(&mut params, "AB");
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, 1);
        // Into B (clocks 3 and 4), then reset
        for _ in 0..3 {
            tick(&mut seq, &params, true, false);
            tick(&mut seq, &params, false, false);
        }
        assert_eq!(seq.pattern, 1);
        tick(&mut seq, &params, false, true);
        tick(&mut seq, &params, false, false);
        tick(&mut seq, &params, true, false);
        assert_eq!((seq.pattern, seq.current_step()), (0, 0));
    }

    #[test]
    fn readout_reports_the_pattern_and_the_next() {
        let mut params = params_with(2.0, 0.0, 50.0);
        set_chain(&mut params, "BCD");
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, 1);
        let before = PatternPosition::from_readout(&seq.readout(&params).unwrap());
        assert!(!before.started);
        // Clocks 1-2 play B, 3-4 C
        for _ in 0..3 {
            tick(&mut seq, &params, true, false);
            tick(&mut seq, &params, false, false);
        }
        let at = PatternPosition::from_readout(&seq.readout(&params).unwrap());
        assert!(at.started && !at.pattern_cv);
        assert_eq!((at.pattern, at.chain_slot, at.next_pattern), (2, 1, 3));
    }

    /// Like `gate_over`, but returns the Pitch output (in semitones from C4)
    /// and the Gate output.
    fn play_over(params: &[f32], ms: usize, clocks: &[usize], resets: &[usize]) -> (Vec<f32>, Vec<f32>) {
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, ms);
        let mut clock = SignalBuffer::control(ms);
        let mut reset = SignalBuffer::control(ms);
        for &t in clocks {
            clock.samples[t] = 1.0;
        }
        for &t in resets {
            reset.samples[t] = 1.0;
        }
        let mut outputs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock, &reset], &mut outputs, params, &ProcessContext::new(1000.0, ms));
        let semitones = outputs[0].samples.iter().map(|v| v * 12.0).collect();
        (semitones, outputs[1].samples.clone())
    }

    /// Four steps of C4, with step 3 an octave up and slid into over 100 ms.
    fn slide_params() -> Vec<f32> {
        let mut params = params_with(4.0, 0.0, 50.0);
        params[StepSequencer::PARAM_GLIDE] = 0.1;
        params[StepSequencer::step_pitch_param(2)] = 72.0;
        params[StepSequencer::step_param(0, 2, StepField::Slide)] = 1.0;
        params
    }

    #[test]
    fn test_slides_come_last_and_are_off() {
        let seq = StepSequencer::new();
        let params = seq.parameters();
        assert_eq!(params[StepSequencer::PARAM_GLIDE].id, "glide");
        assert_eq!(params[StepSequencer::PARAM_GLIDE].default, 0.06);
        assert_eq!(StepSequencer::PARAM_GLIDE, StepSequencer::PARAM_PATTERNS + (PATTERNS - 1) * MAX_STEPS * 4, "after everything before slides");
        for pattern in 0..PATTERNS {
            for step in 0..MAX_STEPS {
                assert_eq!(params[StepSequencer::step_param(pattern, step, StepField::Slide)].default, 0.0, "off, so old patches play as before");
            }
        }
    }

    #[test]
    fn test_slide_glides_and_keeps_the_gate_high() {
        let (pitch, gate) = play_over(&slide_params(), 4000, &[0, 1000, 2000, 3000], &[]);

        // Step 2 holds past its 50% to join step 3, which isn't struck
        assert_eq!(rises(&gate), [0, 1000, 3000]);
        assert_eq!(high_for(&gate, 1000), 1000 + 500);

        // Pitch leaves C4 at the clock and climbs to C5 over the Glide time
        assert!(pitch[2000] > 0.0 && pitch[2000] < 1.0, "starts from the note before: {}", pitch[2000]);
        assert!((2001..2200).all(|t| pitch[t] >= pitch[t - 1]), "rises all the way");
        assert!((pitch[2100] - 12.0).abs() < 0.13, "99% there after the Glide time: {}", pitch[2100]);
        assert!(pitch[2050] < 11.9, "not there halfway: {}", pitch[2050]);
    }

    #[test]
    fn test_plain_step_jumps_and_retriggers() {
        let mut params = slide_params();
        params[StepSequencer::step_param(0, 2, StepField::Slide)] = 0.0;
        params[StepSequencer::PARAM_GATE_LENGTH] = 100.0;
        let (pitch, gate) = play_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);

        // Lands on C5 at the clock, and the full gate dips to strike it
        assert_eq!(pitch[2000], 12.0);
        assert_eq!(pitch[1999], 0.0);
        assert_eq!(gate[2000], 0.0);
        assert_eq!(rises(&gate), [0, 1000, 2001, 3001]);
    }

    #[test]
    fn test_slide_lands_and_the_next_plain_step_jumps() {
        let (pitch, gate) = play_over(&slide_params(), 4000, &[0, 1000, 2000, 3000], &[]);
        assert_eq!(pitch[3000], 0.0, "step 4 is back on C4 at once");
        assert_eq!(gate[3000], 1.0, "and struck");
        assert_eq!(gate[2999], 0.0);
    }

    #[test]
    fn test_slide_after_a_rest_is_struck() {
        let mut params = slide_params();
        params[StepSequencer::step_gate_param(1)] = 0.0;
        let (pitch, gate) = play_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);

        // No note to slide from: a new attack, on its own pitch
        assert_eq!(rises(&gate), [0, 2000, 3000]);
        assert_eq!(pitch[2000], 12.0);
    }

    #[test]
    fn test_slide_after_a_reset_is_struck() {
        // Step 1 slides, so step 2 would carry round into it, but a reset
        // comes in between
        let mut params = params_with(2.0, 0.0, 50.0);
        params[StepSequencer::step_param(0, 0, StepField::Slide)] = 1.0;
        params[StepSequencer::step_pitch_param(0)] = 72.0;
        params[StepSequencer::step_pitch_param(1)] = 64.0;
        let (pitch, gate) = play_over(&params, 3000, &[0, 1000, 2000], &[1500]);
        assert_eq!(gate[1500], 0.0, "the reset lets go");
        assert_eq!(pitch[2000], 12.0, "step 1 starts on its own pitch");
        assert_eq!(rises(&gate), [0, 1000, 2000]);
    }

    #[test]
    fn test_slide_carries_round_the_loop() {
        let mut params = params_with(2.0, 0.0, 50.0);
        params[StepSequencer::PARAM_GLIDE] = 0.1;
        params[StepSequencer::step_pitch_param(0)] = 72.0;
        params[StepSequencer::step_param(0, 0, StepField::Slide)] = 1.0;
        let (pitch, gate) = play_over(&params, 3000, &[0, 1000, 2000], &[]);
        // The first step has nothing to come from; the second time round it
        // slides from step 2
        assert_eq!(pitch[0], 12.0);
        assert!(pitch[2000] < 1.0);
        assert_eq!(rises(&gate), [0, 1000]);
    }

    #[test]
    fn test_zero_glide_slide_is_legato_with_a_jump() {
        let mut params = slide_params();
        params[StepSequencer::PARAM_GLIDE] = 0.0;
        let (pitch, gate) = play_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);
        assert_eq!(pitch[2000], 12.0);
        assert_eq!(rises(&gate), [0, 1000, 3000]);
    }

    #[test]
    fn test_tie_keeps_a_slide_gliding() {
        // A one-second slide into step 3, tied on into step 4 on the same note
        let mut params = slide_params();
        params[StepSequencer::PARAM_GLIDE] = 1.0;
        params[StepSequencer::step_tie_param(2)] = 1.0;
        params[StepSequencer::step_pitch_param(3)] = 72.0;
        let (pitch, gate) = play_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);
        assert!(pitch[2999] < 11.9 && pitch[3000] < 11.95, "still on its way: {}", pitch[3000]);
        assert!((3001..4000).all(|t| pitch[t] >= pitch[t - 1]));
        assert_eq!(rises(&gate), [0, 1000]);
    }

    #[test]
    fn test_envelope_is_not_retriggered_across_a_slide() {
        use crate::modules::envelope::AdsrEnvelope;

        // A 303's filter envelope: fast attack, no sustain
        let (_, gate) = play_over(&slide_params(), 3000, &[0, 1000, 2000], &[]);
        let mut env = AdsrEnvelope::new();
        env.prepare(1000.0, 3000);
        let mut env_params: Vec<f32> = env.parameters().iter().map(|p| p.default).collect();
        for (i, p) in env.parameters().iter().enumerate() {
            match p.name {
                "Attack" => env_params[i] = 0.001,
                "Decay" => env_params[i] = 0.3,
                "Sustain" => env_params[i] = 0.0,
                _ => {}
            }
        }
        let mut gate_buf = SignalBuffer::control(3000);
        gate_buf.samples.copy_from_slice(&gate);
        let mut out: Vec<SignalBuffer> = (0..env.ports().iter().filter(|p| p.is_output()).count())
            .map(|_| SignalBuffer::control(3000))
            .collect();
        env.process(&[&gate_buf], &mut out, &env_params, &ProcessContext::new(1000.0, 3000));
        let level = &out[0].samples;
        assert!((1010..3000).all(|t| level[t] <= level[t - 1]), "one decay from step 2, never struck again");
    }

    #[test]
    fn test_looking_ahead_keeps_the_random_order() {
        // Each note looks ahead to see if the next step slides, which in Rnd
        // draws the next step early. Rests don't look, so a pattern of rests
        // draws only as it goes. Both must visit the same steps
        let order = |gate: f32| {
            let mut seq = StepSequencer::new();
            seq.prepare(1000.0, 1);
            let mut params = params_with(16.0, 3.0, 50.0);
            for step in 0..MAX_STEPS {
                params[StepSequencer::step_gate_param(step)] = gate;
            }
            (0..64)
                .map(|_| {
                    tick(&mut seq, &params, true, false);
                    tick(&mut seq, &params, false, false);
                    seq.current_step()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(order(1.0), order(0.0));
    }

    #[test]
    fn test_slide_into_the_next_pattern() {
        // A then B, two steps each: B's first step slides up an octave from
        // A's last, and A's last note holds to meet it
        let mut params = params_with(2.0, 0.0, 50.0);
        set_chain(&mut params, "AB");
        params[StepSequencer::PARAM_GLIDE] = 0.1;
        fill_pattern(&mut params, 1, 72);
        params[StepSequencer::step_param(1, 0, StepField::Slide)] = 1.0;
        let (pitch, gate) = play_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);
        assert_eq!(rises(&gate), [0, 1000, 3000]);
        assert!(pitch[2000] < 1.0, "glides from A's C4: {}", pitch[2000]);
        assert!((pitch[2100] - 12.0).abs() < 0.13);

        // The same slide set in A instead doesn't play, since B is next
        let mut params = params_with(2.0, 0.0, 50.0);
        set_chain(&mut params, "AB");
        fill_pattern(&mut params, 1, 72);
        params[StepSequencer::step_param(0, 0, StepField::Slide)] = 1.0;
        let (pitch, gate) = play_over(&params, 4000, &[0, 1000, 2000, 3000], &[]);
        assert_eq!(rises(&gate), [0, 1000, 2000, 3000]);
        assert_eq!(pitch[2000], 12.0);
    }

    /// C major pentatonic, absolute.
    fn pentatonic() -> u16 {
        rotate_to_key(scale_mask(6, 0), 0)
    }

    /// Runs `params` at 1 kHz, a clock every 100 ms for `clocks` clocks,
    /// with `wander_cv` in the Wander jack if it's given. The note struck at
    /// each clock, and the gate.
    fn wandering(params: &[f32], clocks: usize, wander_cv: Option<f32>) -> (Vec<i32>, Vec<f32>) {
        let ms = clocks * 100;
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, ms);
        let mut clock = SignalBuffer::control(ms);
        for t in (0..ms).step_by(100) {
            clock.samples[t] = 1.0;
        }
        let unpatched = SignalBuffer::unconnected(ms, SignalType::Control);
        let wander = wander_cv.map_or_else(
            || unpatched.clone(),
            |cv| {
                let mut buf = SignalBuffer::control(ms);
                buf.samples.fill(cv);
                buf
            },
        );
        let reset = SignalBuffer::control(ms);
        let mut run = SignalBuffer::control(ms);
        run.samples.fill(1.0);
        let mut outputs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock, &reset, &run, &unpatched, &wander], &mut outputs, params, &ProcessContext::new(1000.0, ms));
        (notes_at(&outputs[0].samples, 100, clocks), outputs[1].samples.clone())
    }

    /// Eight steps of C4 in C major pentatonic, wandering `amount`, keeping `keep`.
    fn wander_params(amount: f32, keep: Keep) -> Vec<f32> {
        let mut params = params_with(8.0, 0.0, 50.0);
        params[StepSequencer::PARAM_WANDER] = amount;
        params[StepSequencer::PARAM_KEEP] = keep as usize as f32;
        params
    }

    #[test]
    fn wander_steps_along_the_scale() {
        let key = pentatonic();
        assert_eq!(wander_note(60, 0, key), 60);
        assert_eq!(wander_note(60, 1, key), 62);
        assert_eq!(wander_note(60, 3, key), 67);
        assert_eq!(wander_note(60, -1, key), 57);
        assert_eq!(wander_note(60, -2, key), 55);
        // Off the scale, the first step is to the scale note that way
        assert_eq!(wander_note(61, 1, key), 62);
        assert_eq!(wander_note(61, -1, key), 60);
        assert_eq!(wander_note(61, 0, key), 61, "no wander leaves a note as written");
        assert_eq!(wander_note(127, 4, key), 127);
        assert_eq!(wander_note(0, -4, key), 0);
    }

    #[test]
    fn wander_comes_last_but_its_knob_is_first() {
        let seq = StepSequencer::new();
        let params = seq.parameters();
        assert_eq!(params[StepSequencer::PARAM_WANDER].default, 0.0, "a new sequencer plays as written");
        assert_eq!(params[StepSequencer::step_param(2, 5, StepField::Chance)].default, 100.0);
        assert_eq!(params[StepSequencer::PARAM_KEEP].default, Keep::First as usize as f32);
        assert_eq!(params[StepSequencer::PARAM_SCALE].name, "Scale");
        assert_eq!(seq.ports()[StepSequencer::PORT_WANDER].name, params[StepSequencer::PARAM_WANDER].name, "the jack pairs with the knob");
    }

    #[test]
    fn wandering_notes_stay_on_the_scale_and_within_reach() {
        let (notes, _) = wandering(&wander_params(2.0, Keep::Off), 96, None);
        for note in &notes {
            assert!([55, 57, 60, 62, 64].contains(note), "two degrees either way of C4: {notes:?}");
        }
        let mut heard = notes.clone();
        heard.sort();
        heard.dedup();
        assert_eq!(heard, [55, 57, 60, 62, 64], "and it goes everywhere it can");
    }

    #[test]
    fn a_little_wander_moves_a_note_now_and_then() {
        let (notes, _) = wandering(&wander_params(0.2, Keep::Off), 400, None);
        let moved = notes.iter().filter(|&&n| n != 60).count();
        // Half of 0.2 on average: a tenth of the notes, by one degree
        assert!((15..=70).contains(&moved), "{moved} of 400 moved");
        assert!(notes.iter().all(|n| [57, 60, 62].contains(n)));
    }

    #[test]
    fn no_wander_plays_as_written() {
        let mut params = wander_params(0.0, Keep::Off);
        params[StepSequencer::step_pitch_param(3)] = 61.0;
        let (notes, _) = wandering(&params, 16, None);
        assert_eq!(notes[..8], [60, 60, 60, 61, 60, 60, 60, 60], "C#4 isn't in the scale, but nothing moves it");
    }

    #[test]
    fn keep_holds_step_1_or_the_accents() {
        let (notes, _) = wandering(&wander_params(4.0, Keep::First), 160, None);
        assert!(notes.iter().step_by(8).all(|&n| n == 60), "step 1 as written: {notes:?}");
        assert!(notes.iter().skip(1).step_by(8).any(|&n| n != 60), "the others wander");

        let mut params = wander_params(4.0, Keep::Accents);
        params[StepSequencer::step_velocity_param(4)] = 120.0;
        let (notes, _) = wandering(&params, 160, None);
        assert!(notes.iter().skip(4).step_by(8).all(|&n| n == 60), "the accent as written: {notes:?}");
        assert!(notes.iter().step_by(8).any(|&n| n != 60), "step 1 wanders");
    }

    #[test]
    fn a_tie_carries_on_where_its_note_wandered() {
        let mut params = wander_params(4.0, Keep::Off);
        for step in [0, 2, 4, 6] {
            params[StepSequencer::step_tie_param(step)] = 1.0;
        }
        let (notes, _) = wandering(&params, 160, None);
        for pair in notes.chunks(2) {
            assert_eq!(pair[0], pair[1], "{notes:?}");
        }
        assert!(notes.iter().any(|&n| n != 60));
    }

    #[test]
    fn the_wander_jack_adds_to_the_knob() {
        let (notes, _) = wandering(&wander_params(0.0, Keep::Off), 80, Some(0.5));
        assert!(notes.iter().any(|&n| n != 60), "half the range: up to two degrees");
        assert!(notes.iter().all(|n| [55, 57, 60, 62, 64].contains(n)), "{notes:?}");
        let (notes, _) = wandering(&wander_params(0.0, Keep::Off), 80, Some(0.0));
        assert!(notes.iter().all(|&n| n == 60));
    }

    #[test]
    fn chance_plays_a_step_some_of_the_time() {
        let with_chance = |chance: f32| {
            let mut params = wander_params(0.0, Keep::Off);
            for step in 0..MAX_STEPS {
                params[StepSequencer::step_param(0, step, StepField::Chance)] = chance;
            }
            rises(&wandering(&params, 400, None).1).len()
        };
        assert_eq!(with_chance(100.0), 400);
        assert_eq!(with_chance(0.0), 0);
        let half = with_chance(50.0);
        assert!((150..=250).contains(&half), "{half} of 400");
    }

    #[test]
    fn a_step_that_loses_its_dice_ends_a_tie_into_it() {
        let mut params = params_with(2.0, 0.0, 50.0);
        params[StepSequencer::step_tie_param(0)] = 1.0;
        params[StepSequencer::step_param(0, 1, StepField::Chance)] = 0.0;
        let (_, gate) = wandering(&params, 4, None);
        assert_eq!(gate[50], 1.0, "step 1's note holds toward step 2");
        assert_eq!(gate[101], 0.0, "but step 2 rests");
        assert_eq!(gate[201], 1.0, "and step 1 plays again");
    }

    #[test]
    fn readout_reports_the_wander_and_a_lost_step() {
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, 1);
        let mut params = params_with(1.0, 0.0, 50.0);
        params[StepSequencer::step_param(0, 0, StepField::Chance)] = 0.0;
        tick(&mut seq, &params, true, false);
        let readout = seq.readout(&params).unwrap();
        assert_eq!(readout.values[StepSequencer::READOUT_DROPPED], 1.0);
        assert_eq!(readout.values[StepSequencer::READOUT_WANDER], 0.0);
        seq.wander = -2;
        assert_eq!(seq.readout(&params).unwrap().values[StepSequencer::READOUT_WANDER], -2.0);
    }

    #[test]
    fn test_sequencer_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<StepSequencer>();
    }

    #[test]
    fn test_sequencer_default() {
        let seq = StepSequencer::default();
        assert_eq!(seq.info().id, "seq.step");
    }

    #[test]
    fn test_note_to_name() {
        assert_eq!(note_to_name(60), "C4");
        assert_eq!(note_to_name(69), "A4");
        assert_eq!(note_to_name(72), "C5");
        assert_eq!(note_to_name(48), "C3");
    }

    /// A one-note audition of `note`, as the piano sends it.
    fn note(note: u8, velocity: f32) -> Audition {
        let mut audition = Audition { len: 1, root: note, bass: note, velocity, ..Audition::default() };
        audition.notes[0] = note;
        audition
    }

    /// Runs `seq` at 1 kHz for `ms`, with a clock pulse at each time in
    /// `clocks`, and returns its Pitch (as a MIDI note), Gate and Velocity.
    fn run(seq: &mut StepSequencer, params: &[f32], ms: usize, clocks: &[usize]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut clock = SignalBuffer::control(ms);
        for &t in clocks {
            clock.samples[t] = 1.0;
        }
        let mut outputs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(ms)).collect();
        seq.process(&[&clock], &mut outputs, params, &ProcessContext::new(1000.0, ms));
        let pitch = outputs[0].samples.iter().map(|v| (v * 12.0 + 60.0).round()).collect();
        (pitch, outputs[1].samples.clone(), outputs[2].samples.clone())
    }

    fn stopped() -> StepSequencer {
        let mut seq = StepSequencer::new();
        seq.prepare(1000.0, 1000);
        seq
    }

    #[test]
    fn a_key_held_with_the_transport_stopped_sounds_until_its_lifted() {
        let params = params_with(8.0, 0.0, 50.0);
        let mut seq = stopped();
        let (_, gate, _) = run(&mut seq, &params, 100, &[]);
        assert!(gate.iter().all(|&g| g == 0.0), "silent before a key");

        seq.audition(Some(note(64, 0.5)));
        let (pitch, gate, velocity) = run(&mut seq, &params, 300, &[]);
        assert_eq!(gate[0], 0.0, "dips a sample so the envelope's struck");
        assert!(gate[1..].iter().all(|&g| g == 1.0), "held past the Gate Length while the key is");
        assert!(pitch.iter().all(|&p| p == 64.0), "E4 on Pitch: {:?}", &pitch[..4]);
        assert!(velocity.iter().all(|&v| v == 0.5));

        seq.audition(None);
        let (pitch, gate, _) = run(&mut seq, &params, 100, &[]);
        assert!(gate.iter().all(|&g| g == 0.0), "falls as the key lifts");
        assert!(pitch.iter().all(|&p| p == 64.0), "Pitch stays put, as after a rest");
    }

    #[test]
    fn a_click_is_a_blip_one_gate_length_long() {
        // 50% of the 100 ms a stopped clock's gates are a share of
        let params = params_with(8.0, 0.0, 50.0);
        let mut seq = stopped();
        seq.audition(Some(note(67, 0.8)));
        seq.audition(None);
        let (pitch, gate, _) = run(&mut seq, &params, 200, &[]);
        assert_eq!(rises(&gate), vec![1]);
        assert_eq!(high_for(&gate, 1), 49);
        assert_eq!(pitch[20], 67.0);
    }

    #[test]
    fn a_click_rings_a_gate_length_of_the_clocks_step() {
        // A clock every 200 ms, then stopped: a 25% gate is 50 ms
        let params = params_with(8.0, 0.0, 25.0);
        let mut seq = stopped();
        run(&mut seq, &params, 600, &[0, 200, 400]);
        seq.audition(Some(note(60, 0.8)));
        seq.audition(None);
        let (_, gate, _) = run(&mut seq, &params, 200, &[]);
        assert_eq!(high_for(&gate, 1), 49);
    }

    #[test]
    fn a_key_struck_under_the_hand_moves_pitch_without_striking() {
        let params = params_with(8.0, 0.0, 50.0);
        let mut seq = stopped();
        seq.audition(Some(note(60, 0.8)));
        run(&mut seq, &params, 50, &[]);
        seq.audition(Some(note(62, 0.8)));
        let (pitch, gate, _) = run(&mut seq, &params, 50, &[]);
        assert!(gate.iter().all(|&g| g == 1.0), "tied, as it's written");
        assert_eq!(pitch[0], 62.0);
    }

    #[test]
    fn once_let_go_the_sequence_takes_over_at_its_next_note() {
        let mut params = params_with(8.0, 0.0, 50.0);
        params[StepSequencer::step_param(0, 1, StepField::Pitch)] = 72.0;
        let mut seq = stopped();
        // The hand comes down on G4 while step 1's C4 plays
        run(&mut seq, &params, 50, &[0]);
        seq.audition(Some(note(67, 0.8)));
        let (pitch, gate, _) = run(&mut seq, &params, 100, &[]);
        assert!(pitch.iter().all(|&p| p == 67.0) && gate[1..].iter().all(|&g| g == 1.0));
        // Let go: step 1's note isn't heard again, and step 2 strikes C5
        seq.audition(None);
        let (pitch, gate, _) = run(&mut seq, &params, 200, &[100]);
        assert!(gate[..100].iter().all(|&g| g == 0.0), "the note under the hand stays let go of");
        assert_eq!(pitch[50], 67.0);
        assert_eq!(rises(&gate), vec![100]);
        assert_eq!(pitch[120], 72.0);
    }

    #[test]
    fn stop_forgets_an_audition() {
        let params = params_with(8.0, 0.0, 50.0);
        let mut seq = stopped();
        seq.audition(Some(note(64, 0.5)));
        seq.reset();
        let (pitch, gate, _) = run(&mut seq, &params, 50, &[]);
        assert!(gate.iter().all(|&g| g == 0.0));
        assert_eq!(pitch[0], 60.0);
    }

    #[test]
    fn test_sequencer_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<StepSequencer>();

        assert!(registry.contains("seq.step"));

        let module = registry.create("seq.step");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "seq.step");
        assert_eq!(module.info().name, "Step Sequencer");
    }
}
