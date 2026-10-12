//! Sampler module.
//!
//! Plays a recording loaded from a WAV file: one-shots from a gate (drum
//! hits, vocal chops), or, with V/Oct and a polyphonic gate, an instrument
//! played across the keyboard, each channel of the cable its own voice.
//!
//! The recording arrives as an `Arc<SampleData>` through
//! [`DspModule::load_sample`], built off the audio thread. Voices read it
//! between samples on a Hermite curve, at a rate set tape-style by pitch
//! and Speed together, so a recording at another sample rate than the
//! engine's still plays in tune. Every start and stop is shaped by a short
//! envelope, so nothing clicks: a gate, a release, a voice stolen by a new
//! note, the end of the recording, even a new file loaded mid-note.
//!
//! Sliced, Start–End is cut into equal parts and each note plays one of
//! them: Root plays slice 1, a semitone up slice 2, as a chopped break is
//! laid across the keys. The note then picks the slice instead of the pitch.

use std::sync::Arc;

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{Slices, MAX_SLICES},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SampleData, SignalType, MAX_CHANNELS, MAX_READOUT,
};

/// The Sampler's module ID.
pub const SAMPLER_ID: &str = "source.sampler";

/// The Loop menu, in saved order.
pub const LOOP_MODES: &[&str] = &["Off", "Forward", "Ping-Pong"];

/// The Mode menu, in saved order.
pub const PLAY_MODES: &[&str] = &["One-Shot", "Gated"];

/// How a voice loops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    Off,
    /// Round and round, jumping from Loop End back to Loop Start.
    Forward,
    /// Back and forth between Loop Start and Loop End.
    PingPong,
}

impl LoopMode {
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => LoopMode::Forward,
            2 => LoopMode::PingPong,
            _ => LoopMode::Off,
        }
    }
}

/// How long a voice takes to let go when it's stolen by a new note, or
/// when the recording it reads is replaced.
const STEAL_FADE_SECONDS: f32 = 0.005;

/// How long before the end of the recording (or of Start–End) a voice
/// fades to nothing, so a recording cut mid-sound doesn't click.
const END_FADE_SECONDS: f32 = 0.003;

/// The fastest a voice reads, in recorded frames per output frame. Past
/// about 2 it aliases; this is just a bound.
const MAX_RATE: f64 = 16.0;

/// How a voice's envelope is moving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    Attack,
    Hold,
    /// Falling to nothing: a release, or a fade when stolen or reloaded.
    Release,
}

/// One playback of the recording.
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    stage: Stage,
    /// Where it reads, in frames of the recording.
    position: f64,
    /// 1, or -1 while a ping-pong loop plays backwards.
    direction: f64,
    /// The envelope, 0 to 1.
    envelope: f32,
    /// What the envelope falls by each sample while releasing.
    fall: f32,
    /// The gate's velocity, latched as the note starts.
    velocity: f32,
    /// The slice it plays, from 0, when the recording is sliced.
    slice: Option<usize>,
    /// Whether it reads the recording being replaced, while it fades out.
    outgoing: bool,
    /// What it played last, before velocity and level, for the readout.
    gain: f32,
}

impl Voice {
    fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    /// Lets go over `seconds`, from wherever the envelope is.
    fn release(&mut self, seconds: f32, sample_rate: f32) {
        if self.is_active() {
            self.stage = Stage::Release;
            self.fall = self.envelope / (seconds * sample_rate).max(1.0);
        }
    }

    /// Lets go at least as fast as a stolen voice does.
    fn fade(&mut self, sample_rate: f32) {
        let fall = self.envelope / (STEAL_FADE_SECONDS * sample_rate).max(1.0);
        if self.stage != Stage::Release || self.fall < fall {
            self.release(STEAL_FADE_SECONDS, sample_rate);
        }
    }
}

/// A channel of the polyphonic cable: its voice, and the one it's fading
/// out of after a retrigger or a steal.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    voice: Voice,
    tail: Voice,
    prev_gate: bool,
}

/// Where in the recording a voice plays, in frames.
#[derive(Clone, Copy, Debug)]
struct Region {
    start: f64,
    end: f64,
    loop_mode: LoopMode,
    loop_start: f64,
    loop_end: f64,
}

impl Region {
    /// The region the knobs set over a recording `frames` long, with
    /// `start_offset` (0–1) added to Start.
    fn new(params: &[f32], start_offset: f32, frames: usize) -> Self {
        let frames = frames as f64;
        let at = |value: f32| (value.clamp(0.0, 1.0) as f64) * frames;
        let mut start = at(params[Sampler::PARAM_START] + start_offset);
        let mut end = at(params[Sampler::PARAM_END]);
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        let mut loop_start = at(params[Sampler::PARAM_LOOP_START]).clamp(start, end);
        let mut loop_end = at(params[Sampler::PARAM_LOOP_END]).clamp(start, end);
        if loop_end < loop_start {
            std::mem::swap(&mut loop_start, &mut loop_end);
        }
        // A loop shorter than a couple of frames can't play
        let loop_mode = if loop_end - loop_start < 2.0 {
            LoopMode::Off
        } else {
            LoopMode::from_param(params[Sampler::PARAM_LOOP])
        };
        Self { start, end, loop_mode, loop_start, loop_end }
    }

    fn is_playable(&self) -> bool {
        self.end - self.start >= 1.0
    }

    /// Slice `index` of this region, cut into `slices`: it plays from the
    /// slice's start to its end, and a loop goes round the whole slice.
    fn slice(&self, params: &[f32], slices: Slices, index: usize) -> Self {
        let (start, end) = slices.span(index, self.start, self.end);
        let loop_mode = if end - start < 2.0 { LoopMode::Off } else { LoopMode::from_param(params[Sampler::PARAM_LOOP]) };
        Self { start, end, loop_mode, loop_start: start, loop_end: end }
    }

    /// The region a voice plays: its slice, while the recording is sliced.
    fn for_voice(&self, params: &[f32], slices: Option<Slices>, voice: &Voice) -> Self {
        match (slices, voice.slice) {
            (Some(slices), Some(index)) => self.slice(params, slices, index),
            _ => *self,
        }
    }
}

/// Plays a recording from a WAV file.
///
/// # Inputs
/// - **Gate** (Gate, poly): a rising edge starts a voice; in Gated mode the
///   falling edge releases it.
/// - **V/Oct** (Control): pitch, 1 V an octave; 0 V (C4) plays the Root note.
///   Sliced, it picks the slice instead: Root plays slice 1.
/// - **Velocity** (Control, 0–1): each note's level, read as it starts.
/// - **Start**, **Speed** (Control): added to their knobs. Sliced, 1 V of
///   Start is the whole slice.
/// - **Slice** (Control, 0–1 across the slices): added to the note's
///   slice as it starts.
///
/// # Outputs
/// - **L**, **R** (Audio, poly): a mono recording plays on both.
///
/// # Parameters
/// - **Start**, **End** (0–1 of the recording): what plays.
/// - **Loop** (Off / Forward / Ping-Pong), **Loop Start**, **Loop End**
///   (0–1 of the recording, kept inside Start–End).
/// - **Mode**: One-Shot plays on to the end whatever the gate does; Gated
///   releases when the gate falls.
/// - **Tune** (±24 st), **Fine** (±100 cents), **Root** (the note the
///   recording plays at unchanged).
/// - **Speed** (−2 to 2): tape speed, so pitch follows it; below 0 it plays
///   backwards, from End to Start.
/// - **Attack**, **Release** (s): the amplitude envelope.
/// - **Level** (0–1).
/// - **Slices** (Off, 2–64): cuts Start–End into equal slices, played by note.
pub struct Sampler {
    /// The recording voices start on.
    sample: Option<Arc<SampleData>>,
    /// The recording it replaced, kept while voices still fade out of it.
    outgoing: Option<Arc<SampleData>>,
    /// Recordings finished with, waiting to be handed back.
    retired: [Option<Arc<SampleData>>; 4],
    sample_rate: f32,
    channels: [Channel; MAX_CHANNELS],
    /// How many channels ran in the last block.
    active: usize,
    level: SmoothedValue,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Sampler {
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            sample: None,
            outgoing: None,
            retired: Default::default(),
            sample_rate,
            channels: [Channel::default(); MAX_CHANNELS],
            active: 1,
            level: SmoothedValue::with_default_smoothing(0.8, sample_rate),
            ports: vec![
                PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0)
                    .describe("A rising edge starts a note; in Gated mode the falling edge releases it. A polyphonic gate plays a voice per channel"),
                PortDefinition::input_with_default("voct", "V/Oct", SignalType::Control, 0.0)
                    .describe("Pitch, 1 V an octave: 0 V (C4) plays the Root note as recorded. Sliced, it picks the slice instead: Root plays slice 1, a semitone up slice 2"),
                PortDefinition::input_with_default("velocity", "Velocity", SignalType::Control, 1.0)
                    .describe("Each note's level, 0 to 1, read as it starts. Unpatched, every note is full"),
                PortDefinition::input_with_default("start_cv", "Start", SignalType::Control, 0.0)
                    .describe("Added to Start as each note begins (1 V is the whole recording, or the whole slice when sliced): an LFO here scrubs where notes start"),
                PortDefinition::input_with_default("speed_cv", "Speed", SignalType::Control, 0.0)
                    .describe("Added to Speed while it plays, as on tape: pitch follows, and below zero it plays backwards"),
                PortDefinition::input_with_default("slice_cv", "Slice", SignalType::Control, 0.0)
                    .describe("Sliced, picks a slice as each note begins: 0 to 1 across them, added to the note's slice. An Arranger lane or a Sample & Hold here rearranges a break"),
                PortDefinition::output("out_l", "L", SignalType::Audio).describe("Left; a mono recording plays the same on both sides"),
                PortDefinition::output("out_r", "R", SignalType::Audio).describe("Right"),
            ],
            parameters: vec![
                ParameterDefinition::new("start", "Start", 0.0, 1.0, 0.0, ParameterDisplay::linear("%"))
                    .describe("Where notes start, through the recording"),
                ParameterDefinition::new("speed", "Speed", -2.0, 2.0, 1.0, ParameterDisplay::linear("x"))
                    .describe("Tape speed: 2 is twice as fast and an octave up, below 0 plays backwards from End to Start"),
                ParameterDefinition::choice("loop", "Loop", LOOP_MODES, 0)
                    .describe("Off plays once; Forward goes round from Loop End to Loop Start; Ping-Pong goes back and forth"),
                ParameterDefinition::choice("mode", "Mode", PLAY_MODES, 0)
                    .describe("One-Shot plays to the end whatever the gate does; Gated lets go when the gate falls"),
                ParameterDefinition::new("end", "End", 0.0, 1.0, 1.0, ParameterDisplay::linear("%"))
                    .describe("Where notes end, through the recording"),
                ParameterDefinition::new("loop_start", "Loop Start", 0.0, 1.0, 0.0, ParameterDisplay::linear("%"))
                    .describe("Where the loop begins, kept inside Start and End"),
                ParameterDefinition::new("loop_end", "Loop End", 0.0, 1.0, 1.0, ParameterDisplay::linear("%"))
                    .describe("Where the loop turns back, kept inside Start and End"),
                ParameterDefinition::new("tune", "Tune", -24.0, 24.0, 0.0, ParameterDisplay::stepped("st"))
                    .describe("Transpose in semitones"),
                ParameterDefinition::new("fine", "Fine", -100.0, 100.0, 0.0, ParameterDisplay::linear("ct"))
                    .describe("Fine tuning in cents"),
                ParameterDefinition::new("root", "Root", 0.0, 127.0, 60.0, ParameterDisplay::stepped("note"))
                    .describe("The note the recording was made at: playing it plays the recording unchanged. Sliced, the note that plays slice 1"),
                ParameterDefinition::new("attack", "Attack", 0.001, 2.0, 0.002, ParameterDisplay::logarithmic("s"))
                    .describe("How long each note takes to fade in"),
                ParameterDefinition::new("release", "Release", 0.001, 5.0, 0.01, ParameterDisplay::logarithmic("s"))
                    .describe("How long a note takes to fade out once let go"),
                ParameterDefinition::new("level", "Level", 0.0, 1.0, 0.8, ParameterDisplay::linear(""))
                    .describe("Output level"),
                ParameterDefinition::new("slices", "Slices", 1.0, MAX_SLICES as f32, 1.0, ParameterDisplay::stepped("slices"))
                    .describe("Cuts Start–End into equal slices, each played by its own note from Root up, and only to its own end. Off plays the recording across the keyboard"),
            ],
        }
    }

    const PORT_GATE: usize = 0;
    const PORT_VOCT: usize = 1;
    const PORT_VELOCITY: usize = 2;
    const PORT_START_CV: usize = 3;
    const PORT_SPEED_CV: usize = 4;
    const PORT_SLICE_CV: usize = 5;

    // The parameters with something to show beside the jacks (the knobs
    // CV can turn, and the menus) come first: the node editor places jacks
    // a row too high after one that has nothing to show there
    pub const PARAM_START: usize = 0;
    pub const PARAM_SPEED: usize = 1;
    pub const PARAM_LOOP: usize = 2;
    pub const PARAM_MODE: usize = 3;
    pub const PARAM_END: usize = 4;
    pub const PARAM_LOOP_START: usize = 5;
    pub const PARAM_LOOP_END: usize = 6;
    pub const PARAM_TUNE: usize = 7;
    pub const PARAM_FINE: usize = 8;
    pub const PARAM_ROOT: usize = 9;
    pub const PARAM_ATTACK: usize = 10;
    pub const PARAM_RELEASE: usize = 11;
    pub const PARAM_LEVEL: usize = 12;
    pub const PARAM_SLICES: usize = 13;

    /// Where the readout holds the slice each channel's voice plays, from 1
    /// (0 for none), after the playheads.
    pub const READOUT_SLICES: usize = MAX_CHANNELS;

    const GATE_THRESHOLD: f32 = 0.5;

    /// The recording voices start on, if one is loaded.
    pub fn sample(&self) -> Option<&Arc<SampleData>> {
        self.sample.as_ref()
    }

    /// Holds a recording to hand back, or drops it here if there is no room
    /// (which the four slots shouldn't allow).
    fn retire(&mut self, sample: Arc<SampleData>) {
        match self.retired.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(sample),
            None => {
                debug_assert!(false, "no room to retire a recording");
                drop(sample);
            }
        }
    }

    /// Hands the outgoing recording back once no voice reads it.
    fn retire_outgoing_if_unused(&mut self) {
        let in_use = self.channels.iter().any(|c| {
            (c.voice.is_active() && c.voice.outgoing) || (c.tail.is_active() && c.tail.outgoing)
        });
        if !in_use {
            if let Some(old) = self.outgoing.take() {
                self.retire(old);
            }
        }
    }

    /// Plays one sample of `voice`, moving it on at `rate` (recorded frames
    /// per output frame, signed). Returns the (left, right) it plays, after
    /// the envelope but before velocity and level.
    #[inline]
    fn step_voice(voice: &mut Voice, data: &SampleData, region: &Region, rate: f64, attack_rise: f32, end_fade: f64) -> (f32, f32) {
        // The envelope
        match voice.stage {
            Stage::Idle => return (0.0, 0.0),
            Stage::Attack => {
                voice.envelope += attack_rise;
                if voice.envelope >= 1.0 {
                    voice.envelope = 1.0;
                    voice.stage = Stage::Hold;
                }
            }
            Stage::Hold => {}
            Stage::Release => {
                voice.envelope -= voice.fall;
                if voice.envelope <= 0.0 {
                    *voice = Voice::default();
                    return (0.0, 0.0);
                }
            }
        }

        let (left, right) = data.read(voice.position);
        let velocity = rate * voice.direction;

        // Fade into the end of what plays, unless a loop turns it back first
        let looping = region.loop_mode != LoopMode::Off;
        let edge = if looping || velocity == 0.0 {
            1.0
        } else {
            let left_to_play = if velocity > 0.0 { region.end - voice.position } else { voice.position - region.start };
            // Reaching nothing on the last sample before it stops
            ((left_to_play / velocity.abs() - 1.0) / end_fade).clamp(0.0, 1.0) as f32
        };
        let gain = voice.envelope * edge;
        voice.gain = gain;

        // Move on, turning at the loop's ends
        let before = voice.position;
        voice.position += velocity;
        if looping {
            let span = region.loop_end - region.loop_start;
            if velocity > 0.0 && voice.position >= region.loop_end && before < region.loop_end {
                match region.loop_mode {
                    LoopMode::PingPong => {
                        voice.position = 2.0 * region.loop_end - voice.position;
                        voice.direction = -voice.direction;
                    }
                    _ => voice.position = region.loop_start + (voice.position - region.loop_end) % span,
                }
            } else if velocity < 0.0 && voice.position <= region.loop_start && before > region.loop_start {
                match region.loop_mode {
                    LoopMode::PingPong => {
                        voice.position = 2.0 * region.loop_start - voice.position;
                        voice.direction = -voice.direction;
                    }
                    _ => voice.position = region.loop_end - (region.loop_start - voice.position) % span,
                }
            }
        }
        // Past either end of what plays, the note is over
        if voice.position >= region.end || voice.position < region.start {
            *voice = Voice::default();
        }
        (left * gain, right * gain)
    }

    /// Packs a voice's place in the recording (0–1) and level (0–1) into
    /// one readout value, exactly: 0 is a silent voice.
    pub fn pack_playhead(place: f32, level: f32) -> f32 {
        let place = (place.clamp(0.0, 1.0) * 65535.0).round() as u32;
        let level = (level.clamp(0.0, 1.0) * 255.0).round().max(1.0) as u32;
        (level * 65536 + place) as f32
    }

    /// The (place, level) a readout value holds, or `None` for a silent voice.
    pub fn unpack_playhead(value: f32) -> Option<(f32, f32)> {
        let packed = value.max(0.0) as u32;
        let level = packed / 65536;
        (level > 0).then(|| ((packed % 65536) as f32 / 65535.0, level as f32 / 255.0))
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Sampler {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: SAMPLER_ID,
            name: "Sampler",
            category: ModuleCategory::Source,
            description: "Plays a WAV file: one-shots from a gate, an instrument across the keyboard, or its slices by note",
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
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [out_l, out_r, ..] = outputs else {
            return;
        };
        let gate = connected_input(inputs, Self::PORT_GATE);
        let voct = connected_input(inputs, Self::PORT_VOCT);
        let velocity = connected_input(inputs, Self::PORT_VELOCITY);
        let start_cv = connected_input(inputs, Self::PORT_START_CV);
        let speed_cv = connected_input(inputs, Self::PORT_SPEED_CV);
        let slice_cv = connected_input(inputs, Self::PORT_SLICE_CV);

        // A voice per channel of the widest cable in, as far as there's room
        let wanted = [gate, voct, velocity].iter().flatten().map(|buffer| buffer.channels()).max().unwrap_or(1);
        let channels = wanted.min(out_l.max_channels()).min(out_r.max_channels()).clamp(1, MAX_CHANNELS);
        out_l.set_channels(channels);
        out_r.set_channels(channels);
        // Channels the cable dropped stop where they are
        for channel in &mut self.channels[channels..self.active.max(channels)] {
            *channel = Channel::default();
        }
        self.active = channels;

        let sample_rate = self.sample_rate;
        let gated = params[Self::PARAM_MODE].round() as i32 == 1;
        let attack_rise = 1.0 / (params[Self::PARAM_ATTACK].max(0.0005) * sample_rate);
        let release = params[Self::PARAM_RELEASE].max(0.0005);
        let end_fade = (END_FADE_SECONDS * sample_rate) as f64;
        self.level.set_target(params[Self::PARAM_LEVEL].clamp(0.0, 1.0));
        // Pitch from the knobs, in semitones, before V/Oct. Sliced, the
        // note picks the slice instead, from Root up, and only Tune and Fine
        // move the pitch
        let slices = Slices::from_param(params[Self::PARAM_SLICES]);
        let root = params[Self::PARAM_ROOT].round();
        let transpose = params[Self::PARAM_TUNE].round() + params[Self::PARAM_FINE] / 100.0;
        let tuning = transpose + (60.0 - root);
        let speed_knob = params[Self::PARAM_SPEED];

        let Self { sample, outgoing, channels: voices, level, .. } = self;
        let current = sample.as_deref();
        let previous = outgoing.as_deref();
        // Each recording's rate against the engine's, so either plays in tune
        let rate_of = |data: Option<&SampleData>| data.map_or(1.0, |d| (d.sample_rate() / sample_rate) as f64);
        let (current_rate, previous_rate) = (rate_of(current), rate_of(previous));
        let start_offset = |c: usize, i: usize| start_cv.map_or(0.0, |cv| cv.voice(c).samples.get(i).copied().unwrap_or(0.0));
        let unmoved = Region::new(params, 0.0, current.map_or(0, SampleData::frames));
        let previous_region = Region::new(params, 0.0, previous.map_or(0, SampleData::frames));

        for i in 0..context.block_size {
            let gain = level.next();
            for (c, channel) in voices.iter_mut().enumerate().take(channels) {
                let read = |buffer: Option<&SignalBuffer>, default: f32| {
                    buffer.map_or(default, |b| b.voice(c).samples.get(i).copied().unwrap_or(default))
                };
                let speed = (speed_knob + read(speed_cv, 0.0)).clamp(-4.0, 4.0) as f64;
                let semitones = if slices.is_some() { transpose } else { tuning + 12.0 * read(voct, 0.0) };
                let pitch = (semitones / 12.0).clamp(-8.0, 8.0).exp2() as f64;
                let rate = (speed * pitch).clamp(-MAX_RATE, MAX_RATE);

                // The gate
                let gate_high = read(gate, 0.0) > Self::GATE_THRESHOLD;
                if gate_high && !channel.prev_gate {
                    if let Some(data) = current {
                        let note = match slices {
                            // The note's slice, if it has one, with Start CV
                            // moving where in it the note starts
                            Some(slices) => {
                                let key = 12.0 * read(voct, 0.0) + 60.0 - root;
                                slices.by_note(key + slices.by_control(read(slice_cv, 0.0)) as f32).map(|index| {
                                    let mut region = unmoved.slice(params, slices, index);
                                    region.start += start_offset(c, i).clamp(0.0, 1.0) as f64 * (region.end - region.start);
                                    (region, Some(index))
                                })
                            }
                            None if start_cv.is_some() => Some((Region::new(params, start_offset(c, i), data.frames()), None)),
                            None => Some((unmoved, None)),
                        };
                        if let Some((region, slice)) = note.filter(|(region, _)| region.is_playable()) {
                            // A voice still sounding hands over to the tail, which lets go quickly
                            if channel.voice.is_active() {
                                channel.tail = channel.voice;
                                channel.tail.fade(sample_rate);
                            }
                            let backwards = rate < 0.0;
                            channel.voice = Voice {
                                stage: Stage::Attack,
                                // Backwards from the end; just inside it, as the end is where a note stops
                                position: if backwards { (region.end - 1e-6).max(region.start) } else { region.start },
                                direction: 1.0,
                                envelope: 0.0,
                                fall: 0.0,
                                velocity: read(velocity, 1.0).clamp(0.0, 1.0),
                                slice,
                                outgoing: false,
                                gain: 0.0,
                            };
                        }
                    }
                } else if !gate_high && channel.prev_gate && gated {
                    channel.voice.release(release, sample_rate);
                }
                channel.prev_gate = gate_high;

                let mut left = 0.0;
                let mut right = 0.0;
                for voice in [&mut channel.voice, &mut channel.tail] {
                    if !voice.is_active() {
                        continue;
                    }
                    let (data, region, scale) = if voice.outgoing {
                        (previous, &previous_region, previous_rate)
                    } else {
                        (current, &unmoved, current_rate)
                    };
                    let Some(data) = data else {
                        *voice = Voice::default();
                        continue;
                    };
                    let region = region.for_voice(params, slices, voice);
                    let (l, r) = Self::step_voice(voice, data, &region, rate * scale, attack_rise, end_fade);
                    left += l * voice.velocity;
                    right += r * voice.velocity;
                }
                out_l.channel_mut(c)[i] = left * gain;
                out_r.channel_mut(c)[i] = right * gain;
            }
        }

        if self.outgoing.is_some() {
            self.retire_outgoing_if_unused();
        }
    }

    fn reset(&mut self) {
        self.channels = [Channel::default(); MAX_CHANNELS];
        if self.outgoing.is_some() {
            self.retire_outgoing_if_unused();
        }
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let frames = self.sample.as_ref().map_or(0, |s| s.frames()).max(1) as f64;
        for (c, channel) in self.channels.iter().enumerate().take(self.active.min(MAX_READOUT - Self::READOUT_SLICES)) {
            // The note playing, or the one letting go if that's all there is
            let voice = [channel.voice, channel.tail].into_iter().find(|v| v.is_active() && !v.outgoing);
            if let Some(voice) = voice {
                readout.values[c] = Self::pack_playhead((voice.position / frames) as f32, voice.gain * voice.velocity);
                readout.values[Self::READOUT_SLICES + c] = voice.slice.map_or(0.0, |index| (index + 1) as f32);
            }
        }
        Some(readout)
    }

    fn polyphonic(&self) -> bool {
        true
    }

    fn load_sample(&mut self, sample: Option<Arc<SampleData>>) -> Option<Arc<SampleData>> {
        // Still fading out of an older recording: let those voices go now
        if let Some(older) = self.outgoing.take() {
            for channel in &mut self.channels {
                for voice in [&mut channel.voice, &mut channel.tail] {
                    if voice.outgoing {
                        *voice = Voice::default();
                    }
                }
            }
            self.retire(older);
        }

        let old = std::mem::replace(&mut self.sample, sample);
        let sample_rate = self.sample_rate;
        let mut playing = false;
        for channel in &mut self.channels {
            for voice in [&mut channel.voice, &mut channel.tail] {
                if voice.is_active() {
                    voice.outgoing = true;
                    voice.fade(sample_rate);
                    playing = true;
                }
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::peak;
    use std::f32::consts::TAU;

    const SAMPLE_RATE: f32 = 48000.0;
    const BLOCK: usize = 256;

    fn default_params() -> Vec<f32> {
        Sampler::new().parameters().iter().map(|p| p.default).collect()
    }

    fn with(changes: &[(usize, f32)]) -> Vec<f32> {
        let mut params = default_params();
        params[Sampler::PARAM_LEVEL] = 1.0;
        for &(index, value) in changes {
            params[index] = value;
        }
        params
    }

    fn sine(hz: f32, rate: f32, seconds: f32) -> Arc<SampleData> {
        let samples = (0..(rate * seconds) as usize).map(|n| (TAU * hz * n as f32 / rate).sin()).collect();
        Arc::new(SampleData::mono(samples, rate))
    }

    fn dc(value: f32, seconds: f32) -> Arc<SampleData> {
        Arc::new(SampleData::mono(vec![value; (SAMPLE_RATE * seconds) as usize], SAMPLE_RATE))
    }

    fn sampler(sample: Arc<SampleData>) -> Sampler {
        let mut sampler = Sampler::new();
        sampler.prepare(SAMPLE_RATE, BLOCK);
        assert!(sampler.load_sample(Some(sample)).is_none());
        sampler
    }

    /// What one channel of the inputs does over time.
    #[derive(Clone, Default)]
    struct Lane {
        /// (from, to) sample ranges the gate is high.
        gates: Vec<(usize, usize)>,
        voct: f32,
        velocity: Option<f32>,
        /// What the Start and Slice inputs hold, when patched.
        start: Option<f32>,
        slice: Option<f32>,
    }

    /// Plays `seconds` of `lanes`, one channel each, returning the left
    /// output of each channel.
    fn play_lanes(sampler: &mut Sampler, params: &[f32], seconds: f32, lanes: &[Lane]) -> Vec<Vec<f32>> {
        let total = (seconds * SAMPLE_RATE) as usize;
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut gate = SignalBuffer::polyphonic(BLOCK, SignalType::Gate);
        gate.set_channels(lanes.len());
        let mut voct = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        voct.set_channels(lanes.len());
        let mut velocity = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        velocity.set_channels(lanes.len());
        let velocity_patched = lanes.iter().any(|lane| lane.velocity.is_some());
        let mut start_cv = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        start_cv.set_channels(lanes.len());
        let start_patched = lanes.iter().any(|lane| lane.start.is_some());
        let mut slice_cv = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        slice_cv.set_channels(lanes.len());
        let slice_patched = lanes.iter().any(|lane| lane.slice.is_some());
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio), SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let mut rendered = vec![Vec::with_capacity(total + BLOCK); lanes.len()];
        let mut start = 0;
        while start < total {
            for (c, lane) in lanes.iter().enumerate() {
                for i in 0..BLOCK {
                    let at = start + i;
                    let high = lane.gates.iter().any(|&(from, to)| at >= from && at < to);
                    gate.channel_mut(c)[i] = if high { 1.0 } else { 0.0 };
                }
                voct.channel_mut(c).fill(lane.voct);
                velocity.channel_mut(c).fill(lane.velocity.unwrap_or(1.0));
                start_cv.channel_mut(c).fill(lane.start.unwrap_or(0.0));
                slice_cv.channel_mut(c).fill(lane.slice.unwrap_or(0.0));
            }
            let patched = |buffer, patched| if patched { buffer } else { &unpatched };
            let inputs = [
                &gate,
                &voct,
                patched(&velocity, velocity_patched),
                patched(&start_cv, start_patched),
                &unpatched,
                patched(&slice_cv, slice_patched),
            ];
            sampler.process(&inputs, &mut outputs, params, &ctx);
            assert_eq!(outputs[0].channels(), lanes.len());
            for (c, out) in rendered.iter_mut().enumerate() {
                out.extend_from_slice(&outputs[0].voice(c).samples);
            }
            start += BLOCK;
        }
        for out in &mut rendered {
            out.truncate(total);
        }
        rendered
    }

    /// One channel, gated over `gates`, at `voct`.
    fn play(sampler: &mut Sampler, params: &[f32], seconds: f32, gates: &[(usize, usize)], voct: f32) -> Vec<f32> {
        let lane = Lane { gates: gates.to_vec(), voct, ..Lane::default() };
        play_lanes(sampler, params, seconds, &[lane]).remove(0)
    }

    /// The frequency of a signal from its first and last rising zero
    /// crossings, each placed between samples.
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

    /// The biggest jump from one sample to the next.
    fn largest_step(samples: &[f32]) -> f32 {
        samples.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    fn ms(milliseconds: f32) -> usize {
        (milliseconds * SAMPLE_RATE / 1000.0) as usize
    }

    /// A step 40 dB below full scale: the most a start or stop may jump.
    const CLICK: f32 = 0.01;

    #[test]
    fn test_ports_and_parameters() {
        let sampler = Sampler::new();
        assert_eq!(sampler.info().id, SAMPLER_ID);
        assert_eq!(sampler.info().category, ModuleCategory::Source);
        let names: Vec<&str> = sampler.ports().iter().map(|p| p.name).collect();
        assert_eq!(names, ["Gate", "V/Oct", "Velocity", "Start", "Speed", "Slice", "L", "R"]);
        assert_eq!(sampler.parameters().len(), 14);
        // Old patches, saved without Slices, play the whole recording
        assert_eq!(Slices::from_param(sampler.parameters()[Sampler::PARAM_SLICES].default), None);
        assert!(sampler.polyphonic());
    }

    #[test]
    fn test_silent_without_a_recording_or_a_gate() {
        let mut empty = Sampler::new();
        empty.prepare(SAMPLE_RATE, BLOCK);
        assert_eq!(peak(&play(&mut empty, &with(&[]), 0.1, &[(0, 4800)], 0.0)), 0.0);
        let mut idle = sampler(sine(1000.0, SAMPLE_RATE, 1.0));
        assert_eq!(peak(&play(&mut idle, &with(&[]), 0.1, &[], 0.0)), 0.0);
    }

    #[test]
    fn test_plays_at_its_own_pitch_and_an_octave_up_at_one_volt() {
        let source = sine(1000.0, SAMPLE_RATE, 1.0);
        let params = with(&[]);
        let unison = play(&mut sampler(source.clone()), &params, 0.4, &[(0, 1)], 0.0);
        let hz = frequency(&unison[ms(10.0)..ms(300.0)]);
        assert!((hz - 1000.0).abs() < 0.5, "{hz}");
        let octave = play(&mut sampler(source), &params, 0.4, &[(0, 1)], 1.0);
        let hz = frequency(&octave[ms(10.0)..ms(300.0)]);
        assert!((hz - 2000.0).abs() < 1.0, "{hz}");
    }

    #[test]
    fn test_root_tune_and_fine_set_the_pitch() {
        let source = sine(1000.0, SAMPLE_RATE, 1.0);
        // Recorded at C5: C4 plays it an octave down
        let low = play(&mut sampler(source.clone()), &with(&[(Sampler::PARAM_ROOT, 72.0)]), 0.4, &[(0, 1)], 0.0);
        assert!((frequency(&low[ms(10.0)..ms(300.0)]) - 500.0).abs() < 0.5);
        // A fifth up, less 50 cents
        let tuned = play(&mut sampler(source), &with(&[(Sampler::PARAM_TUNE, 7.0), (Sampler::PARAM_FINE, -50.0)]), 0.4, &[(0, 1)], 0.0);
        let expected = 1000.0 * (6.5f32 / 12.0).exp2();
        assert!((frequency(&tuned[ms(10.0)..ms(300.0)]) - expected).abs() < 1.0);
    }

    #[test]
    fn test_a_recording_at_another_rate_plays_in_tune() {
        // 44.1 kHz in a 48 kHz engine, before or without resampling
        let source = sine(1000.0, 44100.0, 1.0);
        let out = play(&mut sampler(source), &with(&[]), 0.4, &[(0, 1)], 0.0);
        let hz = frequency(&out[ms(10.0)..ms(300.0)]);
        assert!((hz - 1000.0).abs() < 0.5, "{hz}");
    }

    #[test]
    fn test_a_device_rate_change_keeps_the_pitch() {
        // Loaded at 48 kHz, then the device moves to 44.1 kHz before the
        // editor has resampled it: the voice reads it faster, still in tune
        let mut s = sampler(sine(1000.0, SAMPLE_RATE, 1.0));
        s.prepare(44100.0, BLOCK);
        let ctx = ProcessContext::new(44100.0, BLOCK);
        let mut gate = SignalBuffer::gate(BLOCK);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio), SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let mut out = Vec::new();
        for block in 0..60 {
            gate.fill(if block == 0 { 1.0 } else { 0.0 });
            s.process(&[&gate, &unpatched, &unpatched, &unpatched, &unpatched], &mut outputs, &with(&[]), &ctx);
            out.extend_from_slice(&outputs[0].samples);
        }
        // Counted at 44.1 kHz
        let crossings: Vec<f32> = out[441..13230]
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, w)| i as f32 + w[0] / (w[0] - w[1]))
            .collect();
        let hz = (crossings.len() - 1) as f32 * 44100.0 / (crossings[crossings.len() - 1] - crossings[0]);
        assert!((hz - 1000.0).abs() < 0.5, "{hz}");
    }

    #[test]
    fn test_negative_speed_plays_backwards_from_the_end() {
        // A rising ramp, played backwards, falls
        let ramp: Vec<f32> = (0..4800).map(|n| n as f32 / 4800.0).collect();
        let source = Arc::new(SampleData::mono(ramp, SAMPLE_RATE));
        let out = play(&mut sampler(source), &with(&[(Sampler::PARAM_SPEED, -1.0)]), 0.12, &[(0, 1)], 0.0);
        // Past the attack, each sample is the recording read from its end
        for n in [ms(5.0), ms(20.0), ms(50.0), ms(90.0)] {
            let expected = (4800 - n) as f32 / 4800.0;
            assert!((out[n] - expected).abs() < 0.002, "at {n}: {} vs {expected}", out[n]);
        }
        assert_eq!(peak(&out[ms(101.0)..]), 0.0, "stops at Start");

        // Reversed, a sine is a sine at the same pitch
        let reversed = play(&mut sampler(sine(1000.0, SAMPLE_RATE, 1.0)), &with(&[(Sampler::PARAM_SPEED, -1.0)]), 0.4, &[(0, 1)], 0.0);
        assert!((frequency(&reversed[ms(10.0)..ms(300.0)]) - 1000.0).abs() < 0.5);
    }

    #[test]
    fn test_start_and_end_choose_what_plays() {
        // 100 ms of DC: play 20% to 60% of it, 40 ms
        let params = with(&[(Sampler::PARAM_START, 0.2), (Sampler::PARAM_END, 0.6)]);
        let out = play(&mut sampler(dc(0.5, 0.1)), &params, 0.1, &[(0, 1)], 0.0);
        assert!((out[ms(20.0)] - 0.5).abs() < 1e-4);
        // ...fading over its last 3 ms
        assert!(out[ms(36.5)] > 0.49);
        assert!(out[ms(39.0)] < 0.25);
        assert_eq!(peak(&out[ms(40.5)..]), 0.0);
    }

    #[test]
    fn test_loops_wrap_without_a_jump() {
        // Exactly 48 samples a cycle: a loop over whole cycles is seamless
        let source = sine(1000.0, SAMPLE_RATE, 0.5);
        let natural = play(&mut sampler(source.clone()), &with(&[]), 0.3, &[(0, 1)], 0.0);
        let natural_step = largest_step(&natural[ms(5.0)..ms(250.0)]);
        // Loop 10% to 30%: 2400 samples, 50 whole cycles
        for mode in [1.0, 2.0] {
            let params = with(&[(Sampler::PARAM_LOOP, mode), (Sampler::PARAM_LOOP_START, 0.1), (Sampler::PARAM_LOOP_END, 0.3)]);
            let out = play(&mut sampler(source.clone()), &params, 1.0, &[(0, 1)], 0.0);
            let looped = &out[ms(5.0)..];
            // Still playing a second in, well past the half-second recording
            assert!(peak(&out[ms(900.0)..]) > 0.9, "mode {mode}");
            assert!(largest_step(looped) <= natural_step * 1.01, "mode {mode}: {} vs {natural_step}", largest_step(looped));
            // Forward goes round at the same pitch (Ping-Pong turns back on
            // itself at each end, which zero crossings can't count)
            if mode == 1.0 {
                assert!((frequency(&looped[..ms(900.0)]) - 1000.0).abs() < 1.0);
            }
        }
    }

    #[test]
    fn test_gated_releases_when_the_gate_falls_and_one_shot_plays_on() {
        let source = dc(0.5, 1.0);
        let gates = [(0, ms(100.0))];
        let gated = play(&mut sampler(source.clone()), &with(&[(Sampler::PARAM_MODE, 1.0)]), 0.3, &gates, 0.0);
        assert!(gated[ms(99.0)] > 0.49);
        // Release 10 ms
        assert_eq!(peak(&gated[ms(111.0)..]), 0.0);
        let one_shot = play(&mut sampler(source), &with(&[]), 0.3, &gates, 0.0);
        assert!(one_shot[ms(250.0)] > 0.49);
        for out in [&gated, &one_shot] {
            assert!(largest_step(out) < CLICK, "{}", largest_step(out));
        }
    }

    #[test]
    fn test_the_end_of_the_recording_fades_rather_than_clicks() {
        let out = play(&mut sampler(dc(0.9, 0.05)), &with(&[]), 0.1, &[(0, 1)], 0.0);
        assert!(out[ms(25.0)] > 0.89);
        assert!(largest_step(&out) < CLICK, "{}", largest_step(&out));
        assert_eq!(peak(&out[ms(50.0)..]), 0.0);
    }

    #[test]
    fn test_velocity_scales_each_note() {
        let lane = Lane { gates: vec![(0, 1)], velocity: Some(0.25), ..Lane::default() };
        let out = play_lanes(&mut sampler(dc(0.8, 1.0)), &with(&[]), 0.05, &[lane]).remove(0);
        assert!((out[ms(20.0)] - 0.2).abs() < 1e-4);
    }

    #[test]
    fn test_poly_gates_play_independent_voices_and_a_steal_does_not_click() {
        let source = dc(0.5, 2.0);
        // Four notes, a chord built up 10 ms apart, then the first stolen
        // for a fifth note at 200 ms
        let lanes: Vec<Lane> = (0..4)
            .map(|c| {
                let mut gates = vec![(ms(10.0 * c as f32), ms(10.0 * c as f32) + 1)];
                if c == 0 {
                    gates.push((ms(200.0), ms(200.0) + 1));
                }
                Lane { gates, voct: c as f32 / 12.0, ..Lane::default() }
            })
            .collect();
        let out = play_lanes(&mut sampler(source), &with(&[]), 0.4, &lanes);
        for (c, channel) in out.iter().enumerate() {
            let starts = ms(10.0 * c as f32);
            assert_eq!(peak(&channel[..starts]), 0.0, "voice {c} silent until its note");
            assert!(channel[starts + ms(5.0)] > 0.49, "voice {c} plays");
            assert!(largest_step(channel) < CLICK, "voice {c}: {}", largest_step(channel));
        }
        // The stolen voice dips no further than its two fades cross
        let around_steal = &out[0][ms(199.0)..ms(210.0)];
        assert!(around_steal.iter().all(|&s| s > 0.1), "{around_steal:?}");
    }

    #[test]
    fn test_loading_mid_note_fades_the_old_recording_out_then_hands_it_back() {
        let first = dc(0.5, 1.0);
        let mut s = sampler(first.clone());
        let params = with(&[]);
        let before = play(&mut s, &params, 0.05, &[(0, 1)], 0.0);
        assert!(before[ms(40.0)] > 0.49);

        // Swapped mid-note: kept until its voice has faded
        let second = dc(-0.5, 1.0);
        assert!(s.load_sample(Some(second.clone())).is_none());
        assert!(s.take_retired_sample().is_none());
        let after = play(&mut s, &params, 0.05, &[], 0.0);
        let mut joined = before.clone();
        joined.extend_from_slice(&after);
        assert!(largest_step(&joined) < CLICK, "{}", largest_step(&joined));
        assert_eq!(peak(&after[ms(6.0)..]), 0.0);
        let retired = s.take_retired_sample().expect("the first recording comes back");
        assert!(Arc::ptr_eq(&retired, &first));
        assert!(s.take_retired_sample().is_none());

        // With nothing playing, the old recording comes straight back
        let back = s.load_sample(None).expect("handed back at once");
        assert!(Arc::ptr_eq(&back, &second));
    }

    #[test]
    fn test_a_new_note_after_a_load_plays_the_new_recording() {
        let mut s = sampler(dc(0.5, 1.0));
        s.load_sample(Some(dc(-0.25, 1.0)));
        let out = play(&mut s, &with(&[]), 0.05, &[(0, 1)], 0.0);
        assert!((out[ms(20.0)] + 0.25).abs() < 1e-4);
    }

    #[test]
    fn test_readout_shows_each_voices_place() {
        let lanes = [
            Lane { gates: vec![(0, 1)], ..Lane::default() },
            Lane::default(),
        ];
        let mut s = sampler(dc(0.5, 1.0));
        play_lanes(&mut s, &with(&[]), 0.25, &lanes);
        let readout = s.readout(&with(&[])).unwrap();
        let (place, level) = Sampler::unpack_playhead(readout.values[0]).expect("playing");
        assert!((place - 0.25).abs() < 0.01, "{place}");
        assert!((level - 1.0).abs() < 0.01);
        assert!(Sampler::unpack_playhead(readout.values[1]).is_none());
    }

    #[test]
    fn test_playhead_packing_round_trips() {
        for (place, level) in [(0.0, 1.0), (0.5, 0.5), (1.0, 0.004), (0.123, 0.9)] {
            let (p, l) = Sampler::unpack_playhead(Sampler::pack_playhead(place, level)).unwrap();
            assert!((p - place).abs() < 1e-4 && (l - level).abs() < 0.004, "{place} {level}: {p} {l}");
        }
        assert!(Sampler::unpack_playhead(0.0).is_none());
    }

    #[test]
    fn test_reset_stops_every_voice_and_lets_go_of_the_old_recording() {
        let first = dc(0.5, 1.0);
        let mut s = sampler(first.clone());
        play(&mut s, &with(&[]), 0.02, &[(0, 1)], 0.0);
        s.load_sample(Some(dc(0.1, 1.0)));
        s.reset();
        assert!(Arc::ptr_eq(&s.take_retired_sample().unwrap(), &first));
        assert_eq!(peak(&play(&mut s, &with(&[]), 0.02, &[], 0.0)), 0.0);
    }

    /// A recording in `parts` steps of 100 ms, step `k` held at (k + 1) / 10.
    fn staircase(parts: usize) -> Arc<SampleData> {
        let samples = (0..parts * ms(100.0)).map(|n| (n / ms(100.0) + 1) as f32 / 10.0).collect();
        Arc::new(SampleData::mono(samples, SAMPLE_RATE))
    }

    /// One note, struck at the start, with `lane`'s other inputs.
    fn note(sampler: &mut Sampler, params: &[f32], seconds: f32, lane: Lane) -> Vec<f32> {
        play_lanes(sampler, params, seconds, &[Lane { gates: vec![(0, 1)], ..lane }]).remove(0)
    }

    /// A lane `k` semitones above C4.
    fn semitone(k: f32) -> Lane {
        Lane { voct: k / 12.0, ..Lane::default() }
    }

    #[test]
    fn test_sliced_each_note_plays_its_own_slice_and_nothing_past_its_end() {
        let params = with(&[(Sampler::PARAM_SLICES, 8.0)]);
        for k in 0..8 {
            let out = note(&mut sampler(staircase(8)), &params, 0.3, semitone(k as f32));
            let value = (k + 1) as f32 / 10.0;
            // All of it, past the attack and before the end's fade
            for (n, &sample) in out.iter().enumerate().take(ms(96.0)).skip(ms(3.0)) {
                assert!((sample - value).abs() < 1e-4, "slice {k} at {n}: {sample}");
            }
            // Never a neighbour, even in the fade
            assert!(out.iter().all(|&sample| sample <= value + 1e-3), "slice {k}");
            assert_eq!(peak(&out[ms(100.0)..]), 0.0, "slice {k} stops at its end");
        }
    }

    #[test]
    fn test_sliced_notes_below_root_or_past_the_last_slice_play_nothing() {
        let params = with(&[(Sampler::PARAM_SLICES, 8.0)]);
        for k in [-1.0, 8.0, 20.0] {
            assert_eq!(peak(&note(&mut sampler(staircase(8)), &params, 0.2, semitone(k))), 0.0, "{k}");
        }
        // Root moves the first slice's key: C3 plays slice 1
        let low = with(&[(Sampler::PARAM_SLICES, 8.0), (Sampler::PARAM_ROOT, 48.0)]);
        let out = note(&mut sampler(staircase(8)), &low, 0.2, Lane { voct: -1.0, ..Lane::default() });
        assert!((out[ms(50.0)] - 0.1).abs() < 1e-4);
    }

    #[test]
    fn test_sliced_the_note_picks_the_slice_not_the_pitch() {
        // Four slices of a steady sine: any of them is 1 kHz
        let source = sine(1000.0, SAMPLE_RATE, 1.0);
        let sliced = with(&[(Sampler::PARAM_SLICES, 4.0)]);
        let out = note(&mut sampler(source.clone()), &sliced, 0.3, semitone(2.0));
        assert!((frequency(&out[ms(10.0)..ms(240.0)]) - 1000.0).abs() < 0.5);
        // ...but Tune still transposes, and the slice goes by twice as fast
        let up = with(&[(Sampler::PARAM_SLICES, 4.0), (Sampler::PARAM_TUNE, 12.0)]);
        let out = note(&mut sampler(source), &up, 0.3, semitone(2.0));
        assert!((frequency(&out[ms(10.0)..ms(120.0)]) - 2000.0).abs() < 1.0);
        assert_eq!(peak(&out[ms(126.0)..]), 0.0);
    }

    #[test]
    fn test_sliced_gated_stops_at_the_slice_end_and_a_loop_goes_round_it() {
        let held = |params: &[f32]| {
            let lane = Lane { gates: vec![(0, ms(500.0))], voct: 3.0 / 12.0, ..Lane::default() };
            play_lanes(&mut sampler(staircase(8)), params, 0.6, &[lane]).remove(0)
        };
        let gated = held(&with(&[(Sampler::PARAM_SLICES, 8.0), (Sampler::PARAM_MODE, 1.0)]));
        assert_eq!(peak(&gated[ms(100.0)..]), 0.0, "held past its end, it still stops");
        let looped = held(&with(&[(Sampler::PARAM_SLICES, 8.0), (Sampler::PARAM_MODE, 1.0), (Sampler::PARAM_LOOP, 1.0)]));
        assert!((looped[ms(350.0)] - 0.4).abs() < 1e-4, "{}", looped[ms(350.0)]);
        assert!(looped.iter().all(|&sample| sample <= 0.4 + 1e-3));
        assert_eq!(peak(&looped[ms(520.0)..]), 0.0, "and lets go with the gate");
    }

    #[test]
    fn test_sliced_backwards_plays_the_slice_from_its_end() {
        let ramp: Vec<f32> = (0..4800).map(|n| n as f32 / 4800.0).collect();
        let source = Arc::new(SampleData::mono(ramp, SAMPLE_RATE));
        let params = with(&[(Sampler::PARAM_SLICES, 4.0), (Sampler::PARAM_SPEED, -1.0)]);
        // Slice 2 is frames 1200 to 2400
        let out = note(&mut sampler(source), &params, 0.1, semitone(1.0));
        for n in [ms(5.0), ms(15.0)] {
            let expected = (2400 - n) as f32 / 4800.0;
            assert!((out[n] - expected).abs() < 0.002, "at {n}: {} vs {expected}", out[n]);
        }
        assert_eq!(peak(&out[ms(25.0)..]), 0.0);
    }

    #[test]
    fn test_slices_divide_start_to_end() {
        // The second half of eight steps, in four slices: slice 1 is step 5
        let params = with(&[(Sampler::PARAM_SLICES, 4.0), (Sampler::PARAM_START, 0.5)]);
        let out = note(&mut sampler(staircase(8)), &params, 0.3, semitone(0.0));
        assert!((out[ms(50.0)] - 0.5).abs() < 1e-4);
        assert_eq!(peak(&out[ms(100.0)..]), 0.0);
    }

    #[test]
    fn test_the_slice_input_picks_a_slice_and_adds_to_the_note() {
        let params = with(&[(Sampler::PARAM_SLICES, 8.0)]);
        for (voct, cv, slice) in [(0.0, 0.5, 4), (1.0, 0.5, 5), (0.0, 1.0, 7)] {
            let lane = Lane { slice: Some(cv), ..semitone(voct) };
            let out = note(&mut sampler(staircase(8)), &params, 0.2, lane);
            let value = (slice + 1) as f32 / 10.0;
            assert!((out[ms(50.0)] - value).abs() < 1e-4, "{voct} st, {cv}: {}", out[ms(50.0)]);
        }
        // Pushed past the last slice by the note, there's nothing to play
        let lane = Lane { slice: Some(1.0), ..semitone(1.0) };
        assert_eq!(peak(&note(&mut sampler(staircase(8)), &params, 0.2, lane)), 0.0);
    }

    #[test]
    fn test_sliced_start_cv_moves_the_start_inside_the_slice() {
        let ramp: Vec<f32> = (0..4800).map(|n| n as f32 / 4800.0).collect();
        let source = Arc::new(SampleData::mono(ramp, SAMPLE_RATE));
        // Halfway into slice 1 (frames 0 to 1200), so it plays 600 to 1200
        let params = with(&[(Sampler::PARAM_SLICES, 4.0)]);
        let out = note(&mut sampler(source), &params, 0.1, Lane { start: Some(0.5), ..Lane::default() });
        let n = ms(5.0);
        assert!((out[n] - (600 + n) as f32 / 4800.0).abs() < 0.002, "{}", out[n]);
        assert_eq!(peak(&out[ms(12.5)..]), 0.0);
    }

    #[test]
    fn test_readout_shows_each_voices_slice() {
        let lanes = [semitone(5.0), Lane::default()].map(|lane| Lane { gates: vec![(0, 1)], ..lane });
        let params = with(&[(Sampler::PARAM_SLICES, 8.0)]);
        let mut s = sampler(staircase(8));
        play_lanes(&mut s, &params, 0.05, &lanes);
        let readout = s.readout(&params).unwrap();
        assert_eq!(readout.values[Sampler::READOUT_SLICES], 6.0);
        assert_eq!(readout.values[Sampler::READOUT_SLICES + 1], 1.0);
        let (place, _) = Sampler::unpack_playhead(readout.values[0]).expect("playing");
        assert!((place - 0.6875).abs() < 0.01, "{place}");
        // Unsliced, no slice
        let mut s = sampler(staircase(8));
        play_lanes(&mut s, &with(&[]), 0.05, &lanes);
        assert_eq!(s.readout(&with(&[])).unwrap().values[Sampler::READOUT_SLICES], 0.0);
    }

    #[test]
    fn test_sampler_registry_instantiation() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create(SAMPLER_ID).expect("Sampler is registered");
        assert_eq!(module.info().name, "Sampler");
        assert!(module.polyphonic());
    }
}
