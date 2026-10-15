//! Chopper module.
//!
//! Plays a sliced loop in time with the Clock, the way a REX file plays: a
//! break is cut into slices (equal, or where its hits begin), and each
//! slice starts on its own beat, played at its own speed. A faster tempo
//! starts each slice sooner, cutting the one before short; a slower one
//! leaves a little room after each, which a short fade closes. Either way
//! the break keeps its pitch, and its groove, at any tempo.
//!
//! Over the loop runs a row of steps, one per slice. Each step's turn comes
//! where its slice began in the break, and it plays that slice unless it's
//! been told otherwise: another slice, backwards, repeated (a ratchet), at
//! another pitch, or not at all. A Chance per step swaps in a random slice,
//! so the break varies itself, as a chopped break does in a producer's
//! hands.
//!
//! Every start and stop is a short crossfade. A slice played through runs
//! on past its end into the sound after it while the next fades in, so an
//! unedited pattern at the break's own tempo crossfades the recording into
//! itself and plays it back seamlessly.
//!
//! # How a step is stored
//!
//! Each step is one parameter, so a pattern saves, loads, undoes and copies
//! with the patch like any knob. Its value reads as digits: `R V PP SS`, the
//! repeat (1 to 4), 1 if it plays backwards, the pitch offset plus 12, and
//! the slice it plays from 1, or 00 for its own. It's negative while the
//! step rests, so resting a step and playing it again keeps what it was:
//! `101200` plays its own slice, once, forwards, unchanged; `-211205` is a
//! rest that would play slice 5 backwards, twice. The step's Chance is a
//! parameter of its own. See [`Step`].
//!
//! # Recording a take
//!
//! With something patched into In, Rec records a take into it, as the
//! Sampler does and with the same recorder. A take in bars, with a Clock running,
//! starts on the next bar and lasts exactly that long, so it's already a
//! loop: it plays as soon as it ends, from its top, as many beats long as
//! it was recorded, until the editor has set Beats to match. A free take
//! made while a Clock runs is as many beats as it nearly is.

use std::sync::{Arc, LazyLock};

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{Markers, Slices, MAX_SLICES, ONSETS},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    FinishedTake, ParameterDisplay, SampleData, SignalType,
};
use crate::dsp::onsets::DEFAULT_SENSITIVITY;

use super::sampler::{Controls, Recorder, Sampler, NO_THRESHOLD_DB, TAKE_LENGTHS};
use super::sequencer::StepTimer;

/// The Chopper's module ID.
pub const CHOPPER_ID: &str = "source.chopper";

/// The most steps in the row: one per slice.
pub const MAX_STEPS: usize = MAX_SLICES;

/// The most times a step repeats its slice.
pub const MAX_REPEAT: u8 = 4;

/// The furthest a step's pitch moves, in semitones, either way.
pub const MAX_PITCH: i8 = 12;

/// The Pulse menu: what each pulse at Clock is worth, as the Clock's
/// Division names it.
pub const PULSES: &[&str] = &["1", "1/2", "1/4", "1/8", "1/16"];

/// Beats in each Pulse, in menu order.
const PULSE_BEATS: [f64; 5] = [4.0, 2.0, 1.0, 0.5, 0.25];

/// The crossfade between one slice and the next, and the fade in and out
/// of every slice.
const FADE_SECONDS: f32 = 0.003;

/// How long a slice takes to let go when the recording it reads is
/// replaced, or the transport resets.
const STOP_SECONDS: f32 = 0.005;

/// Voices: the one playing, and room for those still fading out.
const VOICES: usize = 4;

/// One step of the row: whether it plays, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Whether it plays; a rest when not.
    pub plays: bool,
    /// The slice it plays, from 1, or 0 for its own.
    pub slice: u8,
    /// Whether it plays its slice backwards, from the end.
    pub reverse: bool,
    /// How many times it plays its slice, spread evenly across its turn.
    pub repeat: u8,
    /// Semitones up or down: tape-style, so it plays faster or slower too.
    pub pitch: i8,
}

impl Step {
    /// A step as it starts: its own slice, once, forwards, unchanged.
    pub const DEFAULT: Step = Step { plays: true, slice: 0, reverse: false, repeat: 1, pitch: 0 };

    /// Reads a step from its parameter value (see the module docs).
    pub fn decode(value: f32) -> Self {
        let digits = value.abs().round() as u32;
        Self {
            plays: value > 0.0,
            slice: (digits % 100).min(MAX_STEPS as u32) as u8,
            pitch: (((digits / 100) % 100).min(2 * MAX_PITCH as u32) as i32 - MAX_PITCH as i32) as i8,
            reverse: (digits / 10_000) % 10 == 1,
            repeat: (digits / 100_000).clamp(1, MAX_REPEAT as u32) as u8,
        }
    }

    /// The parameter value that stores this step.
    pub fn encode(self) -> f32 {
        let digits = self.repeat.clamp(1, MAX_REPEAT) as u32 * 100_000
            + u32::from(self.reverse) * 10_000
            + (self.pitch.clamp(-MAX_PITCH, MAX_PITCH) + MAX_PITCH) as u32 * 100
            + self.slice.min(MAX_STEPS as u8) as u32;
        if self.plays { digits as f32 } else { -(digits as f32) }
    }

    /// The slice (from 0) step `index` plays, of `count`.
    pub fn slice_of(self, index: usize, count: usize) -> usize {
        let slice = match self.slice {
            0 => index,
            n => n as usize - 1,
        };
        slice % count.max(1)
    }

    /// Whether it plays its own slice, once, forwards, unchanged.
    pub fn is_plain(self, index: usize, count: usize) -> bool {
        self.plays && !self.reverse && self.repeat == 1 && self.pitch == 0 && self.slice_of(index, count) == index
    }
}

/// The largest value a step's parameter takes, either way.
const STEP_VALUE_LIMIT: f32 = 412_464.0;

/// Parameter names made up at start-up: they must be `'static`.
struct Names {
    step_ids: Vec<&'static str>,
    step_names: Vec<&'static str>,
    chance_ids: Vec<&'static str>,
    chance_names: Vec<&'static str>,
}

fn keep(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

static NAMES: LazyLock<Names> = LazyLock::new(|| {
    let mut names = Names { step_ids: Vec::new(), step_names: Vec::new(), chance_ids: Vec::new(), chance_names: Vec::new() };
    for step in 1..=MAX_STEPS {
        names.step_ids.push(keep(format!("step_{step}")));
        names.step_names.push(keep(format!("Step {step:02}")));
        names.chance_ids.push(keep(format!("chance_{step}")));
        names.chance_names.push(keep(format!("Chance {step:02}")));
    }
    names
});

/// One playback of a slice.
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    active: bool,
    /// Where it reads, in frames of the recording. Past either end it reads
    /// round from the other, as the loop does.
    position: f64,
    /// Frames it moves each sample: below zero, backwards.
    velocity: f64,
    /// The slice, in frames.
    from: f64,
    to: f64,
    envelope: f32,
    rise: f32,
    /// What the envelope falls by each sample, once releasing.
    fall: f32,
    releasing: bool,
    /// Whether it fades out before the end of its slice, as its turn is
    /// longer than the slice: otherwise it plays on past the end, into
    /// the next slice's crossfade.
    fades_at_end: bool,
    /// Whether it reads the recording being replaced, while it fades out.
    outgoing: bool,
    /// What it played last, for the readout.
    gain: f32,
}

impl Voice {
    /// Lets go over `samples`, from wherever the envelope is, unless it's
    /// already letting go faster.
    fn release(&mut self, samples: f32) {
        if !self.active {
            return;
        }
        let fall = self.envelope / samples.max(1.0);
        if !self.releasing || self.fall < fall {
            self.releasing = true;
            self.fall = fall;
        }
    }

    /// Plays one sample, after the envelope.
    #[inline]
    fn step(&mut self, data: &SampleData, end_fade: f64) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        if self.releasing {
            self.envelope -= self.fall;
            if self.envelope <= 0.0 {
                *self = Voice::default();
                return (0.0, 0.0);
            }
        } else if self.envelope < 1.0 {
            self.envelope = (self.envelope + self.rise).min(1.0);
        }

        let forwards = self.velocity >= 0.0;
        let speed = self.velocity.abs().max(1e-9);
        // Fade into the end of the slice when there's room after it
        let edge = if self.fades_at_end {
            let left = if forwards { self.to - self.position } else { self.position - self.from };
            ((left / speed - 1.0) / end_fade).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        let frames = data.frames() as f64;
        let (left, right) = data.read(self.position.rem_euclid(frames));
        let gain = self.envelope * edge;
        self.gain = gain;

        self.position += self.velocity;
        let past = if forwards { self.position >= self.to } else { self.position <= self.from };
        if past {
            if self.fades_at_end {
                *self = Voice::default();
            } else if !self.releasing {
                // Played through: on into what follows, while the next slice fades in
                self.release(end_fade as f32);
            }
        }
        (left * gain, right * gain)
    }
}

/// Plays a sliced loop in time with the Clock, rearranged step by step.
///
/// # Inputs
/// - **Clock** (Gate): each rising edge is a pulse, worth what **Pulse**
///   says; the loop plays from the first after a reset.
/// - **Reset** (Gate): the next pulse starts the loop from its top.
/// - **Slice** (Control, 0–1 across the slices): picks the slice each step
///   plays as its turn comes, in place of its own.
/// - **In L**, **In R** (Audio): what Rec records; one side patched records
///   on both.
/// - **Rec** (Gate): records while high; patched, it takes over from the
///   button.
///
/// # Outputs
/// - **L**, **R** (Audio): a mono recording plays on both.
/// - **Gate** (Gate): high for half of each slice played, so other modules
///   can follow the chop.
///
/// # Parameters
/// - **Pulse** (1 to 1/16): what each pulse at Clock is worth, as the
///   Clock's Division.
/// - **Beats** (1–64): how long the loop is.
/// - **Slices** (2–64, Onsets): equal slices, or one from each marker.
/// - **Level** (0–1).
/// - **Sensitivity** (0–1): how strong a hit must be to get a marker, when
///   sliced by Onsets. The editor finds them; here it's only kept.
/// - **Step 01–64**, **Chance 01–64**: the row (see [`Step`]).
/// - **Take** (Free, 1–16 bars): with a Clock running, a take starts on the
///   next bar and lasts that long.
/// - **Rec Button**: the face's Rec, pressed to start a take and again to stop.
pub struct Chopper {
    sample: Option<Arc<SampleData>>,
    markers: Markers,
    /// The recording it replaced, kept while voices still fade out of it.
    outgoing: Option<Arc<SampleData>>,
    /// Recordings finished with, waiting to be handed back.
    retired: [Option<Arc<SampleData>>; 4],
    sample_rate: f32,
    voices: [Voice; VOICES],
    level: SmoothedValue,
    timer: StepTimer,
    /// Pulses since the first after a reset, or `None` before it.
    pulses: Option<u64>,
    prev_clock: bool,
    prev_reset: bool,
    /// The last hit played: the pass through the loop, the step, and which
    /// of its repeats.
    last_hit: Option<(u64, usize, u8)>,
    /// The slice the step playing plays, kept for its repeats.
    step_slice: usize,
    /// The step and slice heard last, from 0, for the face.
    shown_step: Option<usize>,
    shown_slice: Option<usize>,
    /// Where the loop is, 0 to 1, for the face.
    phase: Option<f32>,
    /// Samples the Gate stays high for.
    gate: usize,
    /// The Gate drops for a sample, so a hit over a gate still high starts
    /// with a rising edge.
    gate_dip: bool,
    random_state: u32,
    /// Records takes from In.
    recorder: Recorder,
    /// The take it plays, by count, until it's swapped for the same
    /// recording made to measure.
    holding_take: Option<u32>,
    /// A take's beats, played in place of Beats until Beats moves (as the
    /// editor sets it to match), with the Beats it was recorded under.
    take_beats: Option<(f64, f32)>,
    /// Beats, as last seen.
    beats_param: f32,
    /// The beat the loop's top falls on: a take plays from its top on the
    /// beat it ended.
    top_beat: f64,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Chopper {
    pub const PORT_CLOCK: usize = 0;
    pub const PORT_RESET: usize = 1;
    pub const PORT_SLICE: usize = 2;
    pub const PORT_IN_L: usize = 3;
    pub const PORT_IN_R: usize = 4;
    pub const PORT_REC: usize = 5;
    pub const OUT_L: usize = 0;
    pub const OUT_R: usize = 1;
    pub const OUT_GATE: usize = 2;

    // The menu first: the node editor places jacks a row too high after a
    // parameter with nothing to show beside them
    pub const PARAM_PULSE: usize = 0;
    pub const PARAM_BEATS: usize = 1;
    pub const PARAM_SLICES: usize = 2;
    pub const PARAM_LEVEL: usize = 3;
    pub const PARAM_SENSITIVITY: usize = 4;
    pub const PARAM_STEPS: usize = 5;
    pub const PARAM_CHANCE: usize = Self::PARAM_STEPS + MAX_STEPS;
    // Recording, after the row, so patches from before it load unchanged
    pub const PARAM_TAKE: usize = Self::PARAM_CHANCE + MAX_STEPS;
    pub const PARAM_REC_BUTTON: usize = Self::PARAM_TAKE + 1;
    pub const PARAM_COUNT: usize = Self::PARAM_REC_BUTTON + 1;

    /// Where the readout holds the playhead: its place and level, packed as
    /// the Sampler packs them ([`Sampler::pack_playhead`]).
    pub const READOUT_PLAYHEAD: usize = 0;
    /// The step heard last, from 1 (0 before the first).
    pub const READOUT_STEP: usize = 1;
    /// The slice it played, from 1.
    pub const READOUT_SLICE: usize = 2;
    /// Where the loop is, 0 to 1, or -1 waiting for a pulse.
    pub const READOUT_PHASE: usize = 3;
    /// The recorder, where the Sampler shows it, so the face reads both alike.
    pub const READOUT_REC_STATE: usize = Sampler::READOUT_REC_STATE;
    pub const READOUT_REC_SECONDS: usize = Sampler::READOUT_REC_SECONDS;
    pub const READOUT_TAKES: usize = Sampler::READOUT_TAKES;
    pub const READOUT_ROOM: usize = Sampler::READOUT_ROOM;
    pub const READOUT_REC_SCALE: usize = Sampler::READOUT_REC_SCALE;
    pub const READOUT_REC_LEVEL: usize = Sampler::READOUT_REC_LEVEL;

    const GATE_THRESHOLD: f32 = 0.5;
    const SEED: u32 = 0x2545_F491;

    /// The parameter of step `index` (from 0).
    pub const fn step_param(index: usize) -> usize {
        Self::PARAM_STEPS + index
    }

    /// The parameter of step `index`'s Chance.
    pub const fn chance_param(index: usize) -> usize {
        Self::PARAM_CHANCE + index
    }

    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let mut parameters = vec![
            ParameterDefinition::choice("pulse", "Pulse", PULSES, 4)
                .describe("What each pulse at Clock is worth: set it to the Clock's Division, 1/16 for sixteenths"),
            ParameterDefinition::new("beats", "Beats", 1.0, 64.0, 4.0, ParameterDisplay::stepped("beats"))
                .describe("How long the loop is: 4 for a bar of 4/4, 16 for four. The break then follows the patch's tempo, whatever it was played at"),
            ParameterDefinition::new("slices", "Slices", 2.0, ONSETS, 16.0, ParameterDisplay::stepped("slices"))
                .describe("Cuts the loop into equal slices, or past 64 at Onsets, a marker where each hit begins. Each slice gets a step, and starts on the beat it started on in the break"),
            ParameterDefinition::new("level", "Level", 0.0, 1.0, 0.8, ParameterDisplay::linear(""))
                .describe("Output level"),
            ParameterDefinition::new("sensitivity", "Sensitivity", 0.0, 1.0, DEFAULT_SENSITIVITY, ParameterDisplay::linear("%"))
                .describe("Sliced by Onsets, how strong a hit must be to get a marker: up for ghost notes, down for only the loudest hits. Turning it finds them afresh, letting go of markers moved by hand"),
        ];
        let names = &*NAMES;
        for index in 0..MAX_STEPS {
            parameters.push(
                ParameterDefinition::new(
                    names.step_ids[index],
                    names.step_names[index],
                    -STEP_VALUE_LIMIT,
                    STEP_VALUE_LIMIT,
                    Step::DEFAULT.encode(),
                    ParameterDisplay::stepped(""),
                )
                .describe("One step: repeat, reverse, pitch + 12 and slice as digits, negative while it rests"),
            );
        }
        for index in 0..MAX_STEPS {
            parameters.push(
                ParameterDefinition::new(names.chance_ids[index], names.chance_names[index], 0.0, 100.0, 0.0, ParameterDisplay::linear("%"))
                    .describe("How often this step swaps its slice for a random one"),
            );
        }
        parameters.push(
            ParameterDefinition::choice("take", "Take", TAKE_LENGTHS, 1)
                .describe("Free records until Rec is pressed again. In bars, with a Clock running, a take starts on the next bar and lasts exactly that long, and Beats is set to match, so it loops in time"),
        );
        parameters.push(
            ParameterDefinition::new("rec_button", "Rec Button", 0.0, 1.0, 0.0, ParameterDisplay::on_off())
                .describe("The Rec button on the face: pressed to start a take, and again to stop it"),
        );
        debug_assert_eq!(parameters.len(), Self::PARAM_COUNT);

        Self {
            sample: None,
            markers: Markers::default(),
            outgoing: None,
            retired: Default::default(),
            sample_rate,
            voices: [Voice::default(); VOICES],
            level: SmoothedValue::with_default_smoothing(0.8, sample_rate),
            timer: StepTimer::new(),
            pulses: None,
            prev_clock: false,
            prev_reset: false,
            last_hit: None,
            step_slice: 0,
            shown_step: None,
            shown_slice: None,
            phase: None,
            gate: 0,
            gate_dip: false,
            random_state: Self::SEED,
            recorder: Recorder::default(),
            holding_take: None,
            take_beats: None,
            beats_param: 4.0,
            top_beat: 0.0,
            ports: vec![
                PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0)
                    .describe("Each rising edge is a pulse, worth what Pulse says; patch the Clock gate that drives the sequencers"),
                PortDefinition::input_with_default("reset", "Reset", SignalType::Gate, 0.0)
                    .describe("A rising edge starts over: the next pulse plays the loop from its top"),
                PortDefinition::input_with_default("slice", "Slice", SignalType::Control, 0.0)
                    .describe("Picks the slice each step plays as its turn comes, 0 to 1 across them, in place of its own: an Arranger lane or a Sample & Hold here rearranges the break"),
                PortDefinition::input_with_default("in_l", "In L", SignalType::Audio, 0.0)
                    .describe("What Rec records: an Audio Input to record yourself, or the patch's own sound to resample it and chop it up. One side patched records on both"),
                PortDefinition::input_with_default("in_r", "In R", SignalType::Audio, 0.0).describe("The right side of what Rec records"),
                PortDefinition::input_with_default("rec", "Rec", SignalType::Gate, 0.0)
                    .describe("Records while high: the take starts as it rises (or on the next bar, with Take in bars) and ends as it falls. Patched, it takes over from the button"),
                PortDefinition::output("out_l", "L", SignalType::Audio).describe("Left; a mono recording plays the same on both sides"),
                PortDefinition::output("out_r", "R", SignalType::Audio).describe("Right"),
                PortDefinition::output("gate", "Gate", SignalType::Gate)
                    .describe("High for half of each slice played, repeats too, so an envelope or a drum can follow the chop"),
            ],
            parameters,
        }
    }

    /// The recording it plays, if one is loaded.
    pub fn sample(&self) -> Option<&Arc<SampleData>> {
        self.sample.as_ref()
    }

    /// Beats in each pulse.
    pub fn pulse_beats(value: f32) -> f64 {
        PULSE_BEATS[(value.round().max(0.0) as usize).min(PULSE_BEATS.len() - 1)]
    }

    /// The loop's length, in beats.
    pub fn beats(value: f32) -> f64 {
        value.round().clamp(1.0, 64.0) as f64
    }

    /// How the loop is cut, given its markers, as fractions of it: every
    /// Slices setting cuts it, at least into two.
    pub fn slices(value: f32, markers: &[f32]) -> Slices {
        Slices::from_param(value.max(2.0), markers, 0.0, 1.0).unwrap_or(Slices::equal(2))
    }

    /// The tempo, in BPM, at which a recording `seconds` long plays as a
    /// loop `beats` long: the break's own tempo.
    pub fn own_tempo(seconds: f32, beats: f64) -> f32 {
        (beats * 60.0 / seconds.max(1e-3) as f64) as f32
    }

    fn retire(&mut self, sample: Arc<SampleData>) {
        match self.retired.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(sample),
            None => {
                debug_assert!(false, "no room to retire a recording");
                drop(sample);
            }
        }
    }

    fn retire_outgoing_if_unused(&mut self) {
        if !self.voices.iter().any(|v| v.active && v.outgoing) {
            if let Some(old) = self.outgoing.take() {
                self.retire(old);
            }
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

    /// Lets every voice go over `seconds`.
    fn release_all(&mut self, seconds: f32) {
        let samples = seconds * self.sample_rate;
        for voice in &mut self.voices {
            voice.release(samples);
        }
    }

    /// Plays a take just recorded from now on, as a file loaded would be:
    /// from its top, `beats` long if a Clock measured it.
    fn play_take(&mut self, take: Arc<SampleData>, beats: Option<u32>, beat_now: Option<f64>) {
        if let Some(old) = self.load_sample(Some(take)) {
            self.retire(old);
        }
        self.holding_take = Some(self.recorder.takes());
        self.take_beats = beats.map(|beats| (Self::beats(beats as f32), self.beats_param));
        // The beat it ended on (a round trip late) is where its top comes round
        if let Some(beat) = beat_now {
            self.top_beat = beat.round();
        }
        self.last_hit = None;
    }

    /// How long the loop is, in beats: a take's own, until Beats moves.
    fn loop_beats(&mut self, beats_param: f32) -> f64 {
        self.beats_param = beats_param;
        match self.take_beats {
            Some((beats, under)) if under == beats_param => beats,
            _ => {
                self.take_beats = None;
                Self::beats(beats_param)
            }
        }
    }

    /// The beat it's on, counted from the first pulse after a reset, by
    /// the pulse's measured length (waiting at its end for a late one); or
    /// `None` before that pulse.
    fn beat(&self, pulse_beats: f64, context: &ProcessContext) -> Option<f64> {
        let pulses = self.pulses?;
        let pulse = self.pulse_samples(pulse_beats, context);
        let into = self.timer.since_clock().map_or(0.0, |s| (s as f64 / pulse.max(1.0)).min(0.9999));
        Some((pulses as f64 + into) * pulse_beats)
    }

    /// Starts over: the next pulse plays the loop from its top, as it
    /// first did.
    fn rewind(&mut self) {
        self.pulses = None;
        self.top_beat = 0.0;
        self.last_hit = None;
        self.timer.forget_gap();
        self.gate = 0;
        self.random_state = Self::SEED;
    }

    /// How long a pulse is, in samples: as the clock has measured it, or
    /// before two pulses, at the transport's tempo (120 BPM without one).
    fn pulse_samples(&self, pulse_beats: f64, context: &ProcessContext) -> f64 {
        match self.timer.coming_step() {
            Some(samples) => samples as f64,
            None => {
                let bpm = context.transport.tempo_bpm.filter(|&bpm| bpm > 0.0).unwrap_or(120.0) as f64;
                self.sample_rate as f64 * 60.0 / bpm * pulse_beats
            }
        }
    }

    /// Plays hit `hit` of step `index`: its slice, or the one Slice or
    /// Chance chose, for `turn` samples, starting `late` samples after the
    /// hit was due (less than one, unless the clock jumped).
    #[allow(clippy::too_many_arguments)]
    fn strike(&mut self, params: &[f32], slices: &Slices, index: usize, hit: u8, turn: f64, late: f64, slice_cv: Option<f32>) {
        let step = Step::decode(params[Self::step_param(index.min(MAX_STEPS - 1))]);
        let count = slices.count();
        if hit == 0 {
            let mut slice = match slice_cv {
                Some(cv) => slices.by_control(cv),
                None => step.slice_of(index, count),
            };
            let chance = params[Self::chance_param(index.min(MAX_STEPS - 1))].clamp(0.0, 100.0);
            if chance > 0.0 && (self.next_random() % 10_000) as f32 / 100.0 < chance {
                slice = self.next_random() as usize % count;
            }
            self.step_slice = slice;
        }
        self.shown_step = Some(index);
        if !step.plays {
            // A rest: whatever plays lets go
            self.release_all(FADE_SECONDS);
            self.shown_slice = None;
            return;
        }
        self.shown_slice = Some(self.step_slice);

        // The Gate, for half the hit
        let turn = turn.max(2.0);
        self.gate_dip = self.gate > 0;
        self.gate = ((turn / 2.0).round() as usize).clamp(1, turn as usize - 1);

        let Some(data) = self.sample.as_deref() else { return };
        let frames = data.frames() as f64;
        if frames < 2.0 {
            return;
        }
        let (from, to) = slices.span(self.step_slice, 0.0, frames);
        if to - from < 1.0 {
            return;
        }
        let pitch = (step.pitch as f64 / 12.0).exp2();
        let speed = (data.sample_rate() / self.sample_rate) as f64 * pitch;
        let fade = FADE_SECONDS * self.sample_rate;
        // Room after the slice before the next turn: it fades out before its end
        let fades_at_end = turn - (to - from) / speed > fade as f64;
        // Where it would be had it started between samples, as the slice
        // before ran on to exactly there; from the top if the hit came late
        let ahead = if late < 2.0 { late.max(0.0) * speed } else { 0.0 };

        // The voice playing hands over, crossfading
        for voice in &mut self.voices {
            voice.release(fade);
        }
        let slot = self.voices.iter().position(|v| !v.active).unwrap_or_else(|| {
            // All four still sounding: the quietest makes way
            (0..VOICES).min_by(|&a, &b| self.voices[a].envelope.total_cmp(&self.voices[b].envelope)).unwrap_or(0)
        });
        self.voices[slot] = Voice {
            active: true,
            // Backwards from just inside the end
            position: if step.reverse { (to - 1e-6 - ahead).max(from) } else { from + ahead },
            velocity: if step.reverse { -speed } else { speed },
            from,
            to,
            envelope: 0.0,
            rise: 1.0 / fade.max(1.0),
            fall: 0.0,
            releasing: false,
            fades_at_end,
            outgoing: false,
            gain: 0.0,
        };
    }
}

impl Default for Chopper {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Chopper {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: CHOPPER_ID,
            name: "Chopper",
            category: ModuleCategory::Source,
            description: "Plays a sliced break in time with the Clock, at any tempo, rearranged step by step",
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
        self.level.set_sample_rate(sample_rate);
        // A pulse measured at another rate is the wrong number of samples
        self.timer.forget();
        // Off the audio thread: room for another rate can go here
        self.recorder.prepare(sample_rate);
        drop(self.recorder.take_let_go());
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [out_l, out_r, out_gate, ..] = outputs else {
            return;
        };
        let clock = connected_input(inputs, Self::PORT_CLOCK);
        let reset = connected_input(inputs, Self::PORT_RESET);
        let slice_in = connected_input(inputs, Self::PORT_SLICE);
        let read = |buffer: Option<&SignalBuffer>, i: usize| buffer.map_or(0.0, |b| b.samples.get(i).copied().unwrap_or(0.0));

        let pulse_beats = Self::pulse_beats(params[Self::PARAM_PULSE]);
        let mut loop_beats = self.loop_beats(params[Self::PARAM_BEATS]);

        // Recording: a take that ends is the loop from now on
        let controls = Controls {
            left: connected_input(inputs, Self::PORT_IN_L),
            right: connected_input(inputs, Self::PORT_IN_R),
            gate: connected_input(inputs, Self::PORT_REC),
            button: params.get(Self::PARAM_REC_BUTTON).is_some_and(|&v| v >= 0.5),
            threshold_db: NO_THRESHOLD_DB,
            length: params.get(Self::PARAM_TAKE).copied().unwrap_or(0.0),
        };
        if let Some(take) = self.recorder.run(&controls, context) {
            let beats = self.recorder.finished_beats();
            self.play_take(take, beats, self.beat(pulse_beats, context));
            loop_beats = self.loop_beats(params[Self::PARAM_BEATS]);
        }
        if let Some(room) = self.recorder.take_let_go() {
            self.retire(room);
        }
        let slices = Self::slices(params[Self::PARAM_SLICES], self.markers.as_slice());
        self.level.set_target(params[Self::PARAM_LEVEL].clamp(0.0, 1.0));
        let end_fade = (FADE_SECONDS * self.sample_rate) as f64;

        for i in 0..context.block_size {
            let reset_high = read(reset, i) > Self::GATE_THRESHOLD;
            if reset_high && !self.prev_reset {
                self.rewind();
                self.release_all(FADE_SECONDS);
            }
            self.prev_reset = reset_high;

            let clock_high = read(clock, i) > Self::GATE_THRESHOLD;
            if clock_high && !self.prev_clock {
                self.timer.clock();
                self.pulses = Some(self.pulses.map_or(0, |n| n + 1));
            }
            self.prev_clock = clock_high;

            // Where the loop is: the beats since its top came round (a take
            // that ends just before the beat waits for it)
            let since_top = self.beat(pulse_beats, context).map(|beat| beat - self.top_beat).filter(|&beat| beat >= 0.0);
            if let Some(beat) = since_top {
                let pulse = self.pulse_samples(pulse_beats, context);
                let pass = (beat / loop_beats).floor();
                let phase = (beat / loop_beats - pass).clamp(0.0, 1.0);
                self.phase = Some(phase as f32);

                let index = slices.at(phase, 0.0, 1.0).unwrap_or(0);
                let (from, to) = slices.span(index, 0.0, 1.0);
                let step = Step::decode(params[Self::step_param(index.min(MAX_STEPS - 1))]);
                let repeat = step.repeat.max(1);
                let along = ((phase - from) / (to - from).max(1e-9)).clamp(0.0, 1.0);
                let hit = ((along * repeat as f64) as u8).min(repeat - 1);
                let key = (pass as u64, index, hit);
                if self.last_hit != Some(key) {
                    self.last_hit = Some(key);
                    let loop_samples = loop_beats * pulse / pulse_beats;
                    let turn = (to - from) * loop_samples / repeat as f64;
                    let due = from + (to - from) * hit as f64 / repeat as f64;
                    let late = (phase - due) * loop_samples;
                    let cv = slice_in.map(|_| read(slice_in, i));
                    self.strike(params, &slices, index, hit, turn, late, cv);
                }
            }

            let mut left = 0.0;
            let mut right = 0.0;
            let (current, previous) = (self.sample.as_deref(), self.outgoing.as_deref());
            for voice in &mut self.voices {
                let data = if voice.outgoing { previous } else { current };
                match data {
                    Some(data) if voice.active => {
                        let (l, r) = voice.step(data, end_fade);
                        left += l;
                        right += r;
                    }
                    _ => *voice = Voice::default(),
                }
            }
            let gain = self.level.next();
            out_l.samples[i] = left * gain;
            out_r.samples[i] = right * gain;
            out_gate.samples[i] = if self.gate > 0 && !self.gate_dip { 1.0 } else { 0.0 };
            self.gate_dip = false;
            self.gate = self.gate.saturating_sub(1);
            self.timer.tick();
        }

        if self.outgoing.is_some() {
            self.retire_outgoing_if_unused();
        }
    }

    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
        self.rewind();
        self.phase = None;
        self.shown_step = None;
        self.shown_slice = None;
        if self.outgoing.is_some() {
            self.retire_outgoing_if_unused();
        }
        // A take under way ends with the transport, and plays from the top
        if let Some(take) = self.recorder.stop() {
            let beats = self.recorder.finished_beats();
            self.play_take(take, beats, None);
        }
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let frames = self.sample.as_ref().map_or(0, |s| s.frames()).max(1) as f64;
        // The slice playing, or the one letting go if that's all there is
        let voice = self.voices.iter().filter(|v| v.active && !v.outgoing).max_by(|a, b| a.gain.total_cmp(&b.gain));
        if let Some(voice) = voice {
            let place = (voice.position.rem_euclid(frames) / frames) as f32;
            readout.values[Self::READOUT_PLAYHEAD] = Sampler::pack_playhead(place, voice.gain);
        }
        readout.values[Self::READOUT_STEP] = self.shown_step.map_or(0.0, |s| (s + 1) as f32);
        readout.values[Self::READOUT_SLICE] = self.shown_slice.map_or(0.0, |s| (s + 1) as f32);
        readout.values[Self::READOUT_PHASE] = self.phase.unwrap_or(-1.0);
        let recorder = &self.recorder;
        let rate = self.sample_rate.max(1.0);
        readout.values[Self::READOUT_REC_STATE] = recorder.state().code();
        readout.values[Self::READOUT_REC_SECONDS] = recorder.recorded() as f32 / rate;
        readout.values[Self::READOUT_TAKES] = recorder.takes() as f32;
        readout.values[Self::READOUT_ROOM] = recorder.room() as f32 / rate;
        readout.values[Self::READOUT_REC_SCALE] = recorder.scale_seconds();
        readout.values[Self::READOUT_REC_LEVEL] = recorder.meter();
        Some(readout)
    }

    fn take_scope_data(&mut self) -> Option<(&[f32], &[f32], bool)> {
        self.recorder.take_overview().map(|(highs, lows)| (highs, lows, false))
    }

    fn load_sample(&mut self, sample: Option<Arc<SampleData>>) -> Option<Arc<SampleData>> {
        self.holding_take = None;
        self.take_beats = None;
        // Still fading out of an older recording: let those voices go now
        if let Some(older) = self.outgoing.take() {
            for voice in &mut self.voices {
                if voice.outgoing {
                    *voice = Voice::default();
                }
            }
            self.retire(older);
        }
        let old = std::mem::replace(&mut self.sample, sample);
        let stop = STOP_SECONDS * self.sample_rate;
        let mut playing = false;
        for voice in self.voices.iter_mut().filter(|v| v.active) {
            voice.outgoing = true;
            voice.release(stop);
            playing = true;
        }
        if playing {
            // Kept until those voices have faded
            self.outgoing = old;
            None
        } else {
            old
        }
    }

    fn take_retired_sample(&mut self) -> Option<Arc<SampleData>> {
        self.retired.iter_mut().find_map(Option::take)
    }

    fn set_markers(&mut self, markers: Markers) {
        self.markers = markers;
    }

    fn give_room(&mut self, room: Arc<SampleData>) -> Option<Arc<SampleData>> {
        self.recorder.give_room(room)
    }

    fn take_finished_take(&mut self) -> Option<FinishedTake> {
        self.recorder.take_finished()
    }

    fn settle_take(&mut self, take: u32, sample: Arc<SampleData>) -> Option<Arc<SampleData>> {
        let same = self.holding_take == Some(take)
            && self.sample.as_ref().is_some_and(|held| held.frames() == sample.frames() && held.sample_rate() == sample.sample_rate());
        if !same {
            return Some(sample);
        }
        // The same sound, frame for frame: voices read on where they are
        self.holding_take = None;
        self.sample.replace(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::peak;
    use crate::dsp::context::TransportState;
    use std::f32::consts::TAU;

    const SAMPLE_RATE: f32 = 48000.0;
    const BLOCK: usize = 256;
    /// A pulse of sixteenths at 120 BPM, in samples.
    const SIXTEENTH: usize = 6000;

    fn params(changes: &[(usize, f32)]) -> Vec<f32> {
        let mut params: Vec<f32> = Chopper::new().parameters().iter().map(|p| p.default).collect();
        params[Chopper::PARAM_LEVEL] = 1.0;
        for &(index, value) in changes {
            params[index] = value;
        }
        params
    }

    fn with_step(mut params: Vec<f32>, index: usize, step: Step) -> Vec<f32> {
        params[Chopper::step_param(index)] = step.encode();
        params
    }

    /// Two seconds: a bar of 4/4 at 120 BPM.
    fn bar_of(f: impl Fn(f32) -> f32) -> Arc<SampleData> {
        let frames = (2.0 * SAMPLE_RATE) as usize;
        Arc::new(SampleData::mono((0..frames).map(|n| f(n as f32 / SAMPLE_RATE)).collect(), SAMPLE_RATE))
    }

    /// A ramp from -0.5 to 0.5 across the bar: every slice is its own
    /// level, so a jump from one to another is a click.
    fn ramp() -> Arc<SampleData> {
        bar_of(|t| 0.5 * (t - 1.0))
    }

    fn sine(hz: f32) -> Arc<SampleData> {
        bar_of(move |t| 0.5 * (TAU * hz * t).sin())
    }

    fn chopper(sample: Arc<SampleData>) -> Chopper {
        let mut chopper = Chopper::new();
        chopper.prepare(SAMPLE_RATE, BLOCK);
        assert!(chopper.load_sample(Some(sample)).is_none());
        chopper
    }

    /// What it played: left, and Gate.
    struct Played {
        left: Vec<f32>,
        gate: Vec<f32>,
    }

    /// Plays `seconds` with a pulse every `pulse` samples from sample 0,
    /// Slice held at `slice` if patched.
    fn play(chopper: &mut Chopper, params: &[f32], seconds: f32, pulse: usize, slice: Option<f32>) -> Played {
        let total = (seconds * SAMPLE_RATE) as usize;
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut clock = SignalBuffer::gate(BLOCK);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut slice_cv = SignalBuffer::control(BLOCK);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK), SignalBuffer::gate(BLOCK)];
        let mut played = Played { left: Vec::with_capacity(total + BLOCK), gate: Vec::with_capacity(total + BLOCK) };
        let mut start = 0;
        while start < total {
            for i in 0..BLOCK {
                // High for the first half of each pulse
                clock.samples[i] = if (start + i) % pulse < pulse / 2 { 1.0 } else { 0.0 };
            }
            slice_cv.samples.fill(slice.unwrap_or(0.0));
            let slice_input = if slice.is_some() { &slice_cv } else { &unpatched };
            let inputs = [&clock, &unpatched, slice_input];
            chopper.process(&inputs, &mut outputs, params, &ctx);
            played.left.extend_from_slice(&outputs[0].samples);
            played.gate.extend_from_slice(&outputs[2].samples);
            start += BLOCK;
        }
        played.left.truncate(total);
        played.gate.truncate(total);
        played
    }

    /// The biggest jump from one sample to the next.
    fn largest_step(samples: &[f32]) -> f32 {
        samples.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    /// The frequency of a signal from its first and last rising zero crossings.
    fn frequency(samples: &[f32]) -> f32 {
        let crossings: Vec<f32> = samples
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, w)| i as f32 + w[0] / (w[0] - w[1]))
            .collect();
        let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
        (crossings.len() - 1) as f32 * SAMPLE_RATE / (last - first)
    }

    /// Rising edges of a gate, one high from the start among them.
    fn rises(gate: &[f32]) -> Vec<usize> {
        (0..gate.len()).filter(|&i| gate[i] >= 0.5 && (i == 0 || gate[i - 1] < 0.5)).collect()
    }

    /// A step 40 dB below full scale: the most a start or stop may jump.
    const CLICK: f32 = 0.01;

    /// Past the first slice's fade in.
    const SETTLED: usize = 200;

    #[test]
    fn test_ports_and_parameters() {
        let chopper = Chopper::new();
        assert_eq!(chopper.info().id, CHOPPER_ID);
        assert_eq!(chopper.info().category, ModuleCategory::Source);
        let names: Vec<&str> = chopper.ports().iter().map(|p| p.name).collect();
        // Recording's after the rest, so patches from before it load unchanged
        assert_eq!(names, ["Clock", "Reset", "Slice", "In L", "In R", "Rec", "L", "R", "Gate"]);
        assert_eq!(chopper.parameters().len(), Chopper::PARAM_COUNT);
        let params = chopper.parameters();
        assert_eq!(params[Chopper::PARAM_PULSE].name, "Pulse");
        assert_eq!(params[Chopper::PARAM_SLICES].name, "Slices");
        assert_eq!(params[Chopper::PARAM_SENSITIVITY].name, "Sensitivity");
        assert_eq!(params[Chopper::step_param(0)].name, "Step 01");
        assert_eq!(params[Chopper::step_param(63)].name, "Step 64");
        assert_eq!(params[Chopper::chance_param(0)].name, "Chance 01");
        assert_eq!(params[Chopper::chance_param(63)].name, "Chance 64");
        assert_eq!(params[Chopper::PARAM_TAKE].name, "Take");
        assert_eq!(params[Chopper::PARAM_COUNT - 1].name, "Rec Button");
    }

    #[test]
    fn test_steps_round_trip_through_their_parameter() {
        assert_eq!(Step::DEFAULT.encode(), 101_200.0);
        assert_eq!(Step::decode(101_200.0), Step::DEFAULT);
        let odd = Step { plays: false, slice: 5, reverse: true, repeat: 2, pitch: -12 };
        assert_eq!(odd.encode(), -210_005.0);
        for step in [odd, Step { plays: true, slice: 64, reverse: false, repeat: 4, pitch: 12 }, Step { pitch: -3, ..Step::DEFAULT }] {
            assert_eq!(Step::decode(step.encode()), step);
            assert!(step.encode().abs() <= STEP_VALUE_LIMIT);
        }
        // Its own slice, or the one named, round the slices there are
        assert_eq!(Step::DEFAULT.slice_of(3, 8), 3);
        assert_eq!(Step { slice: 2, ..Step::DEFAULT }.slice_of(3, 8), 1);
        assert_eq!(Step { slice: 10, ..Step::DEFAULT }.slice_of(0, 8), 1);
    }

    #[test]
    fn test_silent_without_a_clock_or_a_recording() {
        let mut idle = chopper(ramp());
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Gate);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK), SignalBuffer::gate(BLOCK)];
        for _ in 0..50 {
            idle.process(&[&unpatched, &unpatched, &unpatched], &mut outputs, &params(&[]), &ctx);
            assert_eq!(peak(&outputs[0].samples), 0.0);
        }
        let mut empty = Chopper::new();
        empty.prepare(SAMPLE_RATE, BLOCK);
        let played = play(&mut empty, &params(&[]), 0.5, SIXTEENTH, None);
        assert_eq!(peak(&played.left), 0.0);
        // The Gate still follows the chop
        assert!(!rises(&played.gate).is_empty());
    }

    #[test]
    fn test_unedited_at_its_own_tempo_it_plays_the_break_back_seamlessly() {
        let source = sine(441.0);
        let original = source.left().to_vec();
        // Two passes, sixteen slices, sixteenths at 120 BPM: the bar's own tempo
        let played = play(&mut chopper(source), &params(&[]), 4.0, SIXTEENTH, None);
        let worst = played.left[SETTLED..]
            .iter()
            .enumerate()
            .map(|(n, &out)| (out - original[(n + SETTLED) % original.len()]).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-3, "strays {worst} from the recording");
    }

    #[test]
    fn test_onsets_cut_slices_that_still_play_seamlessly() {
        let source = sine(300.0);
        let original = source.left().to_vec();
        let mut chopper = chopper(source);
        // Hits that don't fall on sixteenths, as a swung break's don't
        chopper.set_markers(Markers::between(&[0.07, 0.22, 0.5, 0.61, 0.9], 0.0, 1.0));
        let played = play(&mut chopper, &params(&[(Chopper::PARAM_SLICES, ONSETS)]), 2.0, SIXTEENTH, None);
        let worst = played.left[SETTLED..].iter().zip(&original[SETTLED..]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3, "strays {worst} from the recording");
        // A Gate for each slice
        assert_eq!(rises(&played.gate).len(), 6);
    }

    #[test]
    fn test_faster_or_slower_it_keeps_its_pitch_and_never_clicks() {
        for bpm in [96.0, 144.0] {
            let pulse = (SAMPLE_RATE * 60.0 / bpm / 4.0) as usize;
            let played = play(&mut chopper(sine(1000.0)), &params(&[]), 3.0, pulse, None);
            // The third slice's turn, clear of its fades
            let turn = pulse * 2;
            let hz = frequency(&played.left[turn + 400..turn + 4400]);
            assert!((hz - 1000.0).abs() < 1.0, "{bpm} BPM: {hz} Hz");

            let played = play(&mut chopper(ramp()), &params(&[]), 3.0, pulse, None);
            let jump = largest_step(&played.left);
            assert!(jump < CLICK, "{bpm} BPM: a jump of {jump}");
            assert!(peak(&played.left) > 0.4);
        }
    }

    #[test]
    fn test_slower_leaves_a_gap_after_each_slice_that_fades_rather_than_cuts() {
        // Half speed: each slice plays out in half its turn
        let played = play(&mut chopper(ramp()), &params(&[]), 1.0, 2 * SIXTEENTH, None);
        let turn = 2 * SIXTEENTH;
        // The slice plays, then fades, then nothing until the next turn
        assert!(played.left[turn + SIXTEENTH / 2].abs() > 0.1);
        assert!(played.left[turn + SIXTEENTH + 300..2 * turn].iter().all(|v| v.abs() < 1e-6));
        assert!(largest_step(&played.left) < CLICK);
    }

    #[test]
    fn test_a_rest_is_silent_and_sends_no_gate() {
        let rest = with_step(params(&[]), 2, Step { plays: false, ..Step::DEFAULT });
        let played = play(&mut chopper(ramp()), &rest, 2.0, SIXTEENTH, None);
        let turn = 2 * SIXTEENTH;
        assert!(played.left[turn + 300..turn + SIXTEENTH].iter().all(|v| v.abs() < 1e-6));
        // Before and after it, the break plays on
        assert!(played.left[turn - 300].abs() > 0.1 && played.left[turn + SIXTEENTH + 300].abs() > 0.1);
        let gates = rises(&played.gate);
        assert_eq!(gates.len(), 15, "{gates:?}");
        assert!(!gates.iter().any(|&at| at.abs_diff(turn) < 10));
        assert!(largest_step(&played.left) < CLICK);
    }

    #[test]
    fn test_reverse_plays_its_slice_backwards_from_its_end() {
        let source = ramp();
        let original = source.left().to_vec();
        let backwards = with_step(params(&[]), 4, Step { reverse: true, ..Step::DEFAULT });
        let played = play(&mut chopper(source), &backwards, 2.0, SIXTEENTH, None);
        let turn = 4 * SIXTEENTH;
        // Falling through the slice's own levels, from its end
        let (early, late) = (played.left[turn + 500], played.left[turn + 5000]);
        assert!(early > late, "{early} then {late}");
        assert!((early - original[turn + SIXTEENTH - 500]).abs() < 1e-3);
        assert!((late - original[turn + SIXTEENTH - 5000]).abs() < 1e-3);
        assert!(largest_step(&played.left) < CLICK);
    }

    #[test]
    fn test_repeat_plays_the_slice_from_its_start_again_and_gates_each_time() {
        let source = ramp();
        let original = source.left().to_vec();
        let twice = with_step(params(&[]), 4, Step { repeat: 2, ..Step::DEFAULT });
        let played = play(&mut chopper(source), &twice, 2.0, SIXTEENTH, None);
        let turn = 4 * SIXTEENTH;
        let half = SIXTEENTH / 2;
        // Halfway through its turn, the slice's start again
        assert!((played.left[turn + half + 400] - original[turn + 400]).abs() < 1e-3);
        let gates = rises(&played.gate);
        assert_eq!(gates.len(), 17);
        assert!(gates.iter().any(|&at| at.abs_diff(turn + half) <= 1), "{gates:?}");
        assert!(largest_step(&played.left) < CLICK);
    }

    #[test]
    fn test_a_step_can_play_another_slice() {
        let source = ramp();
        let original = source.left().to_vec();
        let swapped = with_step(params(&[]), 1, Step { slice: 9, ..Step::DEFAULT });
        let mut chopper = chopper(source);
        let played = play(&mut chopper, &swapped, 2.0, SIXTEENTH, None);
        // Step 2's turn plays slice 9
        let at = SIXTEENTH + 3000;
        assert!((played.left[at] - original[8 * SIXTEENTH + 3000]).abs() < 1e-3);
    }

    #[test]
    fn test_pitch_moves_the_slice_up_or_down() {
        let up = with_step(params(&[]), 2, Step { pitch: 12, ..Step::DEFAULT });
        let played = play(&mut chopper(sine(500.0)), &up, 1.0, SIXTEENTH, None);
        let turn = 2 * SIXTEENTH;
        // An octave up, and so done in half the turn
        let hz = frequency(&played.left[turn + 300..turn + SIXTEENTH / 2 - 400]);
        assert!((hz - 1000.0).abs() < 2.0, "{hz}");
        assert!(played.left[turn + SIXTEENTH / 2 + 200..turn + SIXTEENTH - 10].iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn test_chance_swaps_in_random_slices_and_none_without_it() {
        let source = ramp();
        let original = source.left().to_vec();
        let mut all = params(&[]);
        for index in 0..16 {
            all[Chopper::chance_param(index)] = 100.0;
        }
        let played = play(&mut chopper(source.clone()), &all, 2.0, SIXTEENTH, None);
        // Each turn plays some slice: compare the middle of each with its own
        let moved = (0..16).filter(|k| (played.left[k * SIXTEENTH + 3000] - original[k * SIXTEENTH + 3000]).abs() > 1e-3).count();
        assert!(moved >= 10, "only {moved} of 16 moved");
        // And plays the same again from a reset
        let again = play(&mut chopper(source.clone()), &all, 2.0, SIXTEENTH, None);
        assert_eq!(played.left, again.left);

        let none = play(&mut chopper(source), &params(&[]), 2.0, SIXTEENTH, None);
        let worst = none.left[SETTLED..].iter().zip(&original[SETTLED..]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3);
    }

    #[test]
    fn test_the_slice_input_picks_the_slice_for_every_step() {
        let source = ramp();
        let original = source.left().to_vec();
        // 0.5 of the way across sixteen: slice 9, whatever the step
        let played = play(&mut chopper(source), &params(&[]), 1.0, SIXTEENTH, Some(0.5));
        for k in 0..6 {
            let at = k * SIXTEENTH + 3000;
            assert!((played.left[at] - original[8 * SIXTEENTH + 3000]).abs() < 1e-3, "step {}", k + 1);
        }
    }

    #[test]
    fn test_beats_and_pulse_set_how_long_the_loop_is() {
        let source = ramp();
        let original = source.left().to_vec();
        // Pulses worth eighths: the bar plays in a second, each slice's turn
        // half as long as it is, so each is cut short where the next begins.
        // (Until the second pulse it can't know how fast they come.)
        let eighths = params(&[(Chopper::PARAM_PULSE, 3.0)]);
        let played = play(&mut chopper(source), &eighths, 1.0, SIXTEENTH, None);
        let turn = SIXTEENTH / 2;
        for k in [5, 11, 14] {
            let at = k * turn + 1000;
            assert!((played.left[at] - original[k * SIXTEENTH + 1000]).abs() < 1e-3, "slice {}", k + 1);
        }
    }

    #[test]
    fn test_a_reset_starts_the_loop_from_its_top() {
        let source = ramp();
        let mut chopper = chopper(source);
        let params = params(&[]);
        let _ = play(&mut chopper, &params, 0.7, SIXTEENTH, None);
        chopper.reset();
        let played = play(&mut chopper, &params, 0.2, SIXTEENTH, None);
        assert!((played.left[1000] - 0.5 * (1000.0 / SAMPLE_RATE - 1.0)).abs() < 1e-3);
        let readout = chopper.readout(&params).unwrap();
        assert_eq!(readout.values[Chopper::READOUT_STEP], 2.0);
        assert_eq!(readout.values[Chopper::READOUT_SLICE], 2.0);
        assert!(readout.values[Chopper::READOUT_PHASE] > 0.0);
    }

    #[test]
    fn test_loading_mid_loop_fades_the_old_recording_out_then_hands_it_back() {
        let mut chopper = chopper(ramp());
        let params = params(&[]);
        let _ = play(&mut chopper, &params, 0.3, SIXTEENTH, None);
        assert!(chopper.load_sample(Some(sine(200.0))).is_none(), "kept while it fades");
        let _ = play(&mut chopper, &params, 0.1, SIXTEENTH, None);
        let old = chopper.take_retired_sample().expect("handed back");
        assert_eq!(old.frames(), ramp().frames());
        assert!(chopper.take_retired_sample().is_none());
    }

    /// What a player played at frame `p`: the frame itself, small enough
    /// to be read back exactly.
    fn played(p: usize) -> f32 {
        p as f32 / 1_000_000.0
    }

    /// Runs from sample `from` to `to` (whole blocks) with a Clock at 120
    /// BPM from sample 0, sixteenths at Clock and the transport with it,
    /// `played` at In L, and Rec pressed on the face for the block at each
    /// of `presses`. Returns L.
    fn record(chopper: &mut Chopper, params: &mut [f32], from: usize, to: usize, presses: &[usize]) -> Vec<f32> {
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut clock = SignalBuffer::gate(BLOCK);
        let mut in_l = SignalBuffer::audio(BLOCK);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK), SignalBuffer::gate(BLOCK)];
        let mut left = Vec::new();
        let mut start = from;
        while start < to {
            for i in 0..BLOCK {
                let t = start + i;
                clock.samples[i] = if t % SIXTEENTH < SIXTEENTH / 2 { 1.0 } else { 0.0 };
                in_l.samples[i] = played(t);
            }
            params[Chopper::PARAM_REC_BUTTON] = if presses.contains(&start) { 1.0 } else { 0.0 };
            let transport = TransportState { beat_position: Some(start as f64 * 2.0 / SAMPLE_RATE as f64), ..TransportState::playing_at(120.0) };
            let ctx = ProcessContext::with_transport(SAMPLE_RATE, BLOCK, transport);
            let inputs = [&clock, &unpatched, &unpatched, &in_l, &unpatched, &unpatched];
            chopper.process(&inputs, &mut outputs, params, &ctx);
            left.extend_from_slice(&outputs[0].samples);
            start += BLOCK;
        }
        left
    }

    #[test]
    fn test_a_take_in_bars_becomes_the_loop_from_its_top() {
        const BAR: usize = 96000;
        let mut chopper = chopper(ramp());
        let room = Arc::new(SampleData::with_room(5 * SAMPLE_RATE as usize, SAMPLE_RATE));
        assert!(chopper.give_room(room).is_none(), "taken");
        // Two bars, from the next: Beats is still the old break's 4
        let mut params = params(&[(Chopper::PARAM_TAKE, 2.0)]);
        let pressed = 118 * BLOCK;
        let _ = record(&mut chopper, &mut params, 0, 3 * BAR, &[pressed]);
        let take = chopper.take_finished_take().expect("a take");
        assert_eq!(take.beats, Some(8), "two bars of 4/4");
        assert_eq!(take.sample.frames(), 2 * BAR);
        assert_eq!(take.sample.left()[0], played(BAR), "from the bar line");

        // It plays at once, from its top on the bar it ended, as eight beats
        // before Beats says so, and goes round
        let heard = record(&mut chopper, &mut params, 3 * BAR, 6 * BAR, &[]);
        for n in [3000, 50_000, 150_000, 2 * BAR + 3000, 2 * BAR + 50_000] {
            let expected = played(BAR + n % (2 * BAR));
            assert!((heard[n] - expected).abs() < 1e-4, "{n}: {} for {expected}", heard[n]);
        }

        // The editor sets Beats to match: the same loop
        params[Chopper::PARAM_BEATS] = 8.0;
        let heard = record(&mut chopper, &mut params, 6 * BAR, 7 * BAR, &[]);
        assert!((heard[3000] - played(BAR + BAR + 3000)).abs() < 1e-4);

        // Beats turned by hand is the loop's length again
        params[Chopper::PARAM_BEATS] = 4.0;
        assert_eq!(chopper.loop_beats(params[Chopper::PARAM_BEATS]), 4.0);
    }

    #[test]
    fn test_without_a_clock_a_take_has_no_beats() {
        let mut chopper = Chopper::new();
        chopper.prepare(SAMPLE_RATE, BLOCK);
        assert!(chopper.give_room(Arc::new(SampleData::with_room(48000, SAMPLE_RATE))).is_none());
        let mut params = params(&[]);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut in_l = SignalBuffer::audio(BLOCK);
        in_l.fill(0.25);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK), SignalBuffer::gate(BLOCK)];
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        for block in 0..40 {
            params[Chopper::PARAM_REC_BUTTON] = if block == 1 || block == 30 { 1.0 } else { 0.0 };
            chopper.process(&[&unpatched, &unpatched, &unpatched, &in_l, &unpatched, &unpatched], &mut outputs, &params, &ctx);
        }
        let take = chopper.take_finished_take().expect("a take, in bars or not: there's no Clock to count them");
        assert_eq!(take.beats, None);
        assert_eq!(take.sample.frames(), 29 * BLOCK);
        assert_eq!(chopper.sample().map(|s| s.frames()), Some(29 * BLOCK), "it plays the take");
        assert_eq!(chopper.readout(&params).unwrap().values[Chopper::READOUT_ROOM], 0.0, "the room is the take now");
    }

    #[test]
    fn test_own_tempo() {
        assert!((Chopper::own_tempo(2.0, 4.0) - 120.0).abs() < 1e-3);
        assert!((Chopper::own_tempo(7.06, 16.0) - 136.0).abs() < 0.1);
    }

    #[test]
    fn test_chopper_registry_instantiation() {
        let registry = crate::engine::audio_processor::create_module_registry();
        assert!(registry.contains(CHOPPER_ID));
    }
}
