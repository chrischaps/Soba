//! Cloud module.
//!
//! Granular synthesis: plays clouds of small grains read from a recording,
//! each a few milliseconds to two seconds long, under a smooth window, so
//! time comes apart. A grain starts at Position (scattered by Spray),
//! plays at its own Pitch and sits somewhere across the stereo field (as
//! wide as Spread). Density says how many start each second. With Spray
//! at nothing they start on a regular beat and read the same place, which
//! buzzes at the Density's rate; Spray scatters them in place and in time
//! until they're a cloud.
//!
//! # Sources
//!
//! A WAV file, loaded as the Sampler loads one and handed over with
//! [`DspModule::load_sample`], or, with something patched into In, the
//! last [`LIVE_SECONDS`] of live audio, kept in a ring. **Freeze** stops
//! the ring being written, so a moment of live input stays to be played.
//! Position runs across the source, oldest on the left; for live audio, 1
//! is just now.
//!
//! # Poly
//!
//! Each channel of the Pitch (or Gate) cable grows its own cloud, at its
//! own pitch, out of one shared pool of [`GRAINS`] grains, so a chord from
//! the Chord Sequencer is a cloud of chords. A grain's pitch, place and pan
//! are set as it starts; it plays them out.
//!
//! # Nothing allocates, nothing clicks, nothing aliases
//!
//! The pool is fixed, and the live ring is set aside in
//! [`DspModule::prepare`], off the audio thread. A grain's window rises
//! from nothing and falls to nothing, at any size. A grain read faster than
//! the recording's own rate would fold what's above the new Nyquist back
//! down as aliasing, so it reads through a lowpass of its own, tuned to its
//! rate, fed every frame it passes over.

use std::sync::Arc;

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SampleData, SignalType, MAX_CHANNELS, MAX_READOUT,
};

use super::noise::Pcg32;
use super::sampler::Sampler;

/// The module's ID, as patches name it.
pub const ID: &str = "source.cloud";

/// The grains it can play at once, set aside up front.
pub const GRAINS: usize = 64;

/// How much live audio it keeps, in seconds.
pub const LIVE_SECONDS: f32 = 6.0;

/// Columns in the live waveform it sends the face.
pub const LIVE_COLUMNS: usize = 128;

/// The grains the face shows, loudest first.
pub const SHOWN: usize = 13;

/// The shortest and longest grain, in milliseconds.
pub const MIN_SIZE_MS: f32 = 2.0;
pub const MAX_SIZE_MS: f32 = 2000.0;

/// The fewest and most grains a second, per channel.
pub const MIN_DENSITY: f32 = 0.5;
pub const MAX_DENSITY: f32 = 200.0;

/// The fastest a grain reads, in recorded frames per output frame: three
/// octaves up.
const MAX_RATE: f64 = 8.0;

/// The slowest: four octaves down.
const MIN_RATE: f64 = 1.0 / 16.0;

/// Frames a grain's lowpass hears before the grain starts, so it starts
/// settled.
const PRE_ROLL: i64 = 24;

/// The most grains a Trig starts on each channel.
const MAX_BURST: usize = 8;

/// How often the live waveform goes to the face, in seconds.
const OVERVIEW_SECONDS: f32 = 1.0 / 30.0;

/// One section of a lowpass: a biquad, transposed direct form II.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// A lowpass at `cutoff` (a fraction of the sample rate) with `q`.
    fn lowpass(cutoff: f64, q: f64) -> Self {
        let w = std::f64::consts::TAU * cutoff.clamp(1e-4, 0.49);
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cos) / a0;
        Self {
            b0: (b1 / 2.0) as f32,
            b1: b1 as f32,
            b2: (b1 / 2.0) as f32,
            a1: (-2.0 * cos / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn next(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The Q of each section of an 8th-order Butterworth lowpass.
const BUTTERWORTH_Q: [f64; 4] = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];

/// What a grain reads faster than its source through: an 8th-order
/// Butterworth lowpass on each side, below the Nyquist of the rate it
/// reads at, and the last four frames out of it to read between.
#[derive(Clone, Copy, Debug, Default)]
struct Lowpass {
    left: [Biquad; 4],
    right: [Biquad; 4],
    /// The next frame of the source to feed it.
    fed: i64,
    /// The last four frames out, oldest first: `fed - 4` to `fed - 1`.
    history: [(f32, f32); 4],
}

impl Lowpass {
    /// For a grain reading `rate` frames per frame, starting at `start`.
    fn new(rate: f64, start: i64) -> Self {
        // A little under the new Nyquist, so the slope has room to fall
        let cutoff = 0.4 / rate;
        let sections = BUTTERWORTH_Q.map(|q| Biquad::lowpass(cutoff, q));
        Self { left: sections, right: sections, fed: start - PRE_ROLL, history: [(0.0, 0.0); 4] }
    }

    /// Feeds it every frame up to `position` and the two after, and reads
    /// what came out at `position`, between frames on a Hermite curve.
    #[inline]
    fn read(&mut self, tape: &Tape, position: f64) -> (f32, f32) {
        let base = position.floor();
        let last = base as i64 + 2;
        let mono = tape.is_mono();
        while self.fed <= last {
            let (l, r) = tape.frame(self.fed);
            let l = self.left.iter_mut().fold(l, |x, section| section.next(x));
            // A mono recording's two sides are one
            let r = if mono { l } else { self.right.iter_mut().fold(r, |x, section| section.next(x)) };
            self.history = [self.history[1], self.history[2], self.history[3], (l, r)];
            self.fed += 1;
        }
        // history[1] is the frame at `base`
        let t = (position - base) as f32;
        let [a, b, c, d] = self.history;
        (hermite(a.0, b.0, c.0, d.0, t), hermite(a.1, b.1, c.1, d.1, t))
    }
}

/// A four-point Hermite curve through `b` and `c`, at `t` between them.
#[inline]
fn hermite(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let c0 = b;
    let c1 = 0.5 * (c - a);
    let c2 = a - 2.5 * b + 2.0 * c - 0.5 * d;
    let c3 = 0.5 * (d - a) + 1.5 * (b - c);
    ((c3 * t + c2) * t + c1) * t + c0
}

/// What a grain reads: a recording, or the live ring.
enum Tape<'a> {
    File(&'a SampleData),
    /// The ring's two sides; frame `n` of live audio is at `n % len`.
    Live { left: &'a [f32], right: &'a [f32] },
}

impl Tape<'_> {
    /// Whether its two sides are one: a mono recording.
    #[inline]
    fn is_mono(&self) -> bool {
        matches!(self, Tape::File(data) if !data.is_stereo())
    }

    /// Frame `n`: round from the other end past either end of a
    /// recording, as a grain longer than it plays it as a loop.
    #[inline]
    fn frame(&self, n: i64) -> (f32, f32) {
        let (left, right) = match self {
            Tape::File(data) => (data.left(), data.right()),
            Tape::Live { left, right } => (*left, *right),
        };
        if left.is_empty() {
            return (0.0, 0.0);
        }
        let i = n.rem_euclid(left.len() as i64) as usize;
        (left[i], right[i])
    }

    /// The frame at `position`, read between frames on a Hermite curve.
    #[inline]
    fn read(&self, position: f64) -> (f32, f32) {
        let base = position.floor();
        let i = base as i64;
        let t = (position - base) as f32;
        let (a, b, c, d) = (self.frame(i - 1), self.frame(i), self.frame(i + 1), self.frame(i + 2));
        (hermite(a.0, b.0, c.0, d.0, t), hermite(a.1, b.1, c.1, d.1, t))
    }
}

/// Where a grain reads from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Source {
    #[default]
    File,
    /// The recording being replaced, while it plays out.
    Outgoing,
    Live,
}

/// One grain.
#[derive(Clone, Copy, Debug, Default)]
struct Grain {
    active: bool,
    source: Source,
    /// Where it reads, in frames of its source (counted from the first
    /// frame ever written, for live audio).
    position: f64,
    /// Frames it moves each sample.
    rate: f64,
    /// Its window, sin²(πt), as the imaginary part of a phasor turning
    /// half a turn over the grain.
    re: f64,
    im: f64,
    turn_re: f64,
    turn_im: f64,
    /// Samples left to play.
    left: u32,
    /// Its gain on each side: pan and loudness together.
    gain_l: f32,
    gain_r: f32,
    /// Where it sits, -1 to 1, for the face.
    pan: f32,
    /// Its window as it last played, for the face.
    level: f32,
    /// Read faster than its source: through a lowpass.
    filtered: bool,
    lowpass: Lowpass,
}

impl Grain {
    /// Plays one sample.
    #[inline]
    fn step(&mut self, tape: &Tape) -> (f32, f32) {
        let window = (self.im * self.im) as f32;
        let (re, im) = (self.re, self.im);
        self.re = re * self.turn_re - im * self.turn_im;
        self.im = re * self.turn_im + im * self.turn_re;
        let (l, r) = if self.filtered { self.lowpass.read(tape, self.position) } else { tape.read(self.position) };
        self.level = window;
        self.position += self.rate;
        self.left -= 1;
        if self.left == 0 {
            self.active = false;
        }
        (l * window * self.gain_l, r * window * self.gain_r)
    }
}

/// A channel's cloud: when its next grain is due.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    /// Samples until the next grain.
    countdown: f64,
    /// The channel's Gate, held open.
    gate: Edge,
}

/// The last few seconds of live audio, and a waveform of it for the face.
#[derive(Debug)]
struct Ring {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Frames written since it was set aside: frame `n` is at `n % len`.
    written: u64,
    /// The loudest and quietest sample in each column of the ring.
    highs: [f32; LIVE_COLUMNS],
    lows: [f32; LIVE_COLUMNS],
    /// The columns, oldest first, as sent to the face.
    sent_highs: [f32; LIVE_COLUMNS],
    sent_lows: [f32; LIVE_COLUMNS],
}

impl Default for Ring {
    fn default() -> Self {
        Self::with_frames(0)
    }
}

impl Ring {
    fn with_frames(frames: usize) -> Self {
        Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            written: 0,
            highs: [0.0; LIVE_COLUMNS],
            lows: [0.0; LIVE_COLUMNS],
            sent_highs: [0.0; LIVE_COLUMNS],
            sent_lows: [0.0; LIVE_COLUMNS],
        }
    }

    fn len(&self) -> usize {
        self.left.len()
    }

    fn column_frames(&self) -> usize {
        (self.len() / LIVE_COLUMNS).max(1)
    }

    /// Writes a frame over the oldest.
    #[inline]
    fn push(&mut self, l: f32, r: f32) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let i = (self.written % len as u64) as usize;
        self.left[i] = l;
        self.right[i] = r;
        let width = self.column_frames();
        let column = (i / width).min(LIVE_COLUMNS - 1);
        let mono = 0.5 * (l + r);
        if i.is_multiple_of(width) {
            self.highs[column] = mono;
            self.lows[column] = mono;
        } else {
            self.highs[column] = self.highs[column].max(mono);
            self.lows[column] = self.lows[column].min(mono);
        }
        self.written += 1;
    }

    /// The live frames it holds, as (first, last + 1).
    fn held(&self) -> (f64, f64) {
        let end = self.written as f64;
        (end - (self.written.min(self.len() as u64)) as f64, end)
    }

    /// How full it is, 0 to 1.
    fn filled(&self) -> f32 {
        if self.len() == 0 {
            0.0
        } else {
            self.written.min(self.len() as u64) as f32 / self.len() as f32
        }
    }

    /// Lines the columns up oldest first, for the face.
    fn line_up(&mut self) {
        let len = self.len().max(1);
        let filled = self.written >= len as u64;
        // Oldest first: from just after the one being written, once round
        let width = self.column_frames();
        let writing = ((self.written % len as u64) as usize / width).min(LIVE_COLUMNS - 1);
        let first = if filled { (writing + 1) % LIVE_COLUMNS } else { 0 };
        for k in 0..LIVE_COLUMNS {
            let column = (first + k) % LIVE_COLUMNS;
            // Not yet written: nothing to show
            let unwritten = !filled && column > writing;
            self.sent_highs[k] = if unwritten { 0.0 } else { self.highs[column] };
            self.sent_lows[k] = if unwritten { 0.0 } else { self.lows[column] };
        }
    }
}

/// Plays clouds of small grains from a recording or from live audio.
///
/// # Inputs
/// - **In L**, **In R** (Audio): live audio to play from in place of the
///   file; one side patched plays on both.
/// - **Freeze** (Gate): holds the live audio while high; patched, it takes
///   over from the switch.
/// - **Gate** (Gate, poly): patched, a channel's cloud grows only while its
///   gate is high.
/// - **Trig** (Gate): patched, grains start only on its rising edges, a
///   burst of as many as would overlap.
/// - **Position**, **Size**, **Density** (Control, poly): move their knobs:
///   Position by its whole range per unit, Size and Density by an octave.
/// - **Pitch** (Control, poly, V/Oct): a channel each, a cloud each.
///
/// # Outputs
/// - **L**, **R** (Audio): every cloud, mixed.
///
/// # Parameters
/// - **Freeze** (off/on), **Position** (0–1), **Size** (2–2000 ms), **Density** (0.5–200 grains
///   a second, per channel), **Spray** (0–1: scatters place and time),
///   **Pitch** (±24 semitones), **Spread** (0–1: how wide the grains sit),
///   **Level** (0–1).
pub struct Cloud {
    sample: Option<Arc<SampleData>>,
    outgoing: Option<Arc<SampleData>>,
    retired: [Option<Arc<SampleData>>; 4],
    ring: Ring,
    sample_rate: f32,
    grains: [Grain; GRAINS],
    channels: [Channel; MAX_CHANNELS],
    active: usize,
    level: SmoothedValue,
    rng: Pcg32,
    trig: Edge,
    freeze: Edge,
    /// Whether the last block played the live audio.
    live: bool,
    frozen: bool,
    /// Where Position put grains last, 0 to 1 after CV, for the face.
    position: f32,
    /// Samples until the live waveform goes to the face.
    overview_countdown: usize,
    overview_ready: bool,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Cloud {
    pub const PORT_IN_L: usize = 0;
    pub const PORT_IN_R: usize = 1;
    pub const PORT_FREEZE: usize = 2;
    pub const PORT_GATE: usize = 3;
    pub const PORT_TRIG: usize = 4;
    pub const PORT_POSITION: usize = 5;
    pub const PORT_SIZE: usize = 6;
    pub const PORT_DENSITY: usize = 7;
    pub const PORT_PITCH: usize = 8;
    pub const OUT_L: usize = 0;
    pub const OUT_R: usize = 1;

    // Freeze first, as its jack comes before the others with knobs: the
    // editor lays paired jacks out in parameter order
    pub const PARAM_FREEZE: usize = 0;
    pub const PARAM_POSITION: usize = 1;
    pub const PARAM_SIZE: usize = 2;
    pub const PARAM_DENSITY: usize = 3;
    pub const PARAM_PITCH: usize = 4;
    // Knobs without jacks after those with, or the jacks lose their rows
    pub const PARAM_SPRAY: usize = 5;
    pub const PARAM_SPREAD: usize = 6;
    pub const PARAM_LEVEL: usize = 7;
    pub const PARAM_COUNT: usize = 8;

    /// The grains the face shows, each its place and level packed as the
    /// Sampler packs a playhead ([`Sampler::pack_playhead`]), loudest first.
    pub const READOUT_GRAINS: usize = 0;
    /// Where each of those sits, -1 to 1.
    pub const READOUT_PANS: usize = SHOWN;
    /// 1 while it plays live audio.
    pub const READOUT_LIVE: usize = 2 * SHOWN;
    /// How full the live ring is, 0 to 1.
    pub const READOUT_FILLED: usize = 2 * SHOWN + 1;
    /// 1 while the live audio is frozen.
    pub const READOUT_FROZEN: usize = 2 * SHOWN + 2;
    /// How many grains are playing.
    pub const READOUT_COUNT: usize = 2 * SHOWN + 3;
    /// Position after CV, 0 to 1.
    pub const READOUT_POSITION: usize = 2 * SHOWN + 4;

    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            sample: None,
            outgoing: None,
            retired: Default::default(),
            ring: Ring::default(),
            sample_rate,
            grains: [Grain::default(); GRAINS],
            channels: [Channel::default(); MAX_CHANNELS],
            active: 1,
            level: SmoothedValue::with_default_smoothing(0.8, sample_rate),
            rng: Pcg32::next_stream(),
            trig: Edge::LOW,
            freeze: Edge::LOW,
            live: false,
            frozen: false,
            position: 0.5,
            overview_countdown: 0,
            overview_ready: false,
            ports: vec![
                PortDefinition::input_with_default("in_l", "In L", SignalType::Audio, 0.0)
                    .describe("Live audio to make clouds of, in place of the file: an Audio Input to play yourself, or the patch's own sound. It keeps the last six seconds. One side patched plays on both"),
                PortDefinition::input_with_default("in_r", "In R", SignalType::Audio, 0.0).describe("The right side of the live audio"),
                PortDefinition::input_with_default("freeze", "Freeze", SignalType::Gate, 0.0)
                    .describe("Holds the live audio while high, so the moment stays to be played; patched, it takes over from the switch"),
                PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0)
                    .describe("Patched, each channel's cloud grows only while its gate is high: a Chord Sequencer's Gate and Pitch play a cloud of each chord"),
                PortDefinition::input_with_default("trig", "Trig", SignalType::Gate, 0.0)
                    .describe("Patched, grains start only on its rising edges, a burst of as many as Density and Size would overlap: patch the Clock for grains in time"),
                PortDefinition::input_with_default("position_cv", "Position", SignalType::Control, 0.0)
                    .describe("Moves Position: a unit is the whole source. A slow LFO here drifts through it"),
                PortDefinition::input_with_default("size_cv", "Size", SignalType::Control, 0.0).describe("Moves Size, an octave a unit: up for longer grains"),
                PortDefinition::input_with_default("density_cv", "Density", SignalType::Control, 0.0)
                    .describe("Moves Density, an octave a unit: up for more grains"),
                PortDefinition::input_with_default("pitch", "Pitch", SignalType::Control, 0.0)
                    .describe("V/Oct on top of the Pitch knob. A polyphonic cable grows a cloud for each channel, at its own pitch"),
                PortDefinition::output("out_l", "L", SignalType::Audio).describe("Left: every cloud, mixed"),
                PortDefinition::output("out_r", "R", SignalType::Audio).describe("Right"),
            ],
            parameters: vec![
                ParameterDefinition::toggle("freeze", "Freeze", false)
                    .describe("Holds the live audio: no more is written, so the moment stays to be played"),
                ParameterDefinition::new("position", "Position", 0.0, 1.0, 0.5, ParameterDisplay::linear(""))
                    .describe("Where grains read, across the source: the start on the left, the end on the right. Playing live audio, the right is just now"),
                ParameterDefinition::new("size", "Size", MIN_SIZE_MS, MAX_SIZE_MS, 120.0, ParameterDisplay::logarithmic("ms"))
                    .describe("How long each grain is: a few milliseconds for a buzz or a texture, a second or two to smear the sound out"),
                ParameterDefinition::new("density", "Density", MIN_DENSITY, MAX_DENSITY, 16.0, ParameterDisplay::logarithmic("/s"))
                    .describe("How many grains start each second, on each channel"),
                ParameterDefinition::new("pitch", "Pitch", -24.0, 24.0, 0.0, ParameterDisplay::linear("st"))
                    .describe("Plays every grain higher or lower, in semitones, tape-style"),
                ParameterDefinition::new("spray", "Spray", 0.0, 1.0, 0.1, ParameterDisplay::linear(""))
                    .describe("Scatters grains around Position, and in time: none starts them on a beat at one place, all the way spreads them over the whole source"),
                ParameterDefinition::new("spread", "Spread", 0.0, 1.0, 0.5, ParameterDisplay::linear(""))
                    .describe("How widely grains sit across the stereo field: none in the middle, all the way from hard left to hard right"),
                ParameterDefinition::new("level", "Level", 0.0, 1.0, 0.8, ParameterDisplay::linear("")).describe("Output level"),
            ],
        }
    }

    /// The recording it plays, if one is loaded.
    pub fn sample(&self) -> Option<&Arc<SampleData>> {
        self.sample.as_ref()
    }

    /// How many grains are playing.
    pub fn playing(&self) -> usize {
        self.grains.iter().filter(|g| g.active).count()
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

    /// Uniform in [0, 1).
    #[inline]
    fn unit(&mut self) -> f64 {
        (self.rng.bipolar() as f64 + 1.0) * 0.5
    }

    /// Starts a grain on a free slot, if there is one. Returns whether it
    /// started.
    #[allow(clippy::too_many_arguments)]
    fn spawn(&mut self, live: bool, position: f32, size_ms: f32, spray: f32, semitones: f32, spread: f32, gain: f32) -> bool {
        let Some(slot) = self.grains.iter().position(|g| !g.active) else { return false };
        let sample_rate = self.sample_rate as f64;
        let length = (size_ms as f64 / 1000.0 * sample_rate).round().max(2.0);
        let (source, own_rate, lo, hi) = if live {
            let (first, end) = self.ring.held();
            (Source::Live, 1.0, first, end)
        } else {
            match self.sample.as_deref() {
                Some(data) if data.frames() > 1 => (Source::File, data.sample_rate() as f64 / sample_rate, 0.0, data.frames() as f64),
                _ => return false,
            }
        };
        let rate = ((semitones as f64 / 12.0).exp2() * own_rate).clamp(MIN_RATE, MAX_RATE);
        let span = length * rate;
        // Where a whole grain fits: live, clear of the frame being written
        // at one end and of the frames it will overwrite at the other
        let (from, to) = if live && !self.frozen {
            (lo + length + 4.0, hi - span - 4.0)
        } else {
            (lo + 1.0, hi - span - 4.0)
        };
        if live && to < from {
            // Not enough live audio yet
            return false;
        }
        let place = (position as f64 + spray as f64 * (self.unit() - 0.5)).clamp(0.0, 1.0);
        let start = if to > from { from + place * (to - from) } else { lo + place * (hi - lo) };
        let pan = spread * self.rng.bipolar();
        // Equal power across the field
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let (turn_im, turn_re) = (std::f64::consts::PI / length).sin_cos();
        let filtered = rate > 1.0001;
        self.grains[slot] = Grain {
            active: true,
            source,
            position: start,
            rate,
            re: 1.0,
            im: 0.0,
            turn_re,
            turn_im,
            left: length as u32,
            gain_l: gain * angle.cos() * std::f32::consts::SQRT_2,
            gain_r: gain * angle.sin() * std::f32::consts::SQRT_2,
            pan,
            level: 0.0,
            filtered,
            lowpass: if filtered { Lowpass::new(rate, start.floor() as i64) } else { Lowpass::default() },
        };
        true
    }

    /// Hands the outgoing recording back once no grain reads it.
    fn retire_outgoing_if_unused(&mut self) {
        if !self.grains.iter().any(|g| g.active && g.source == Source::Outgoing) {
            if let Some(old) = self.outgoing.take() {
                self.retire(old);
            }
        }
    }
}

impl Default for Cloud {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Cloud {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Cloud",
            category: ModuleCategory::Source,
            description: "Granular: clouds of small grains from a WAV file or live audio, with Freeze",
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
        // Off the audio thread: the live ring is set aside here
        let frames = (LIVE_SECONDS * sample_rate) as usize;
        if self.ring.len() != frames {
            self.ring = Ring::with_frames(frames);
            for grain in &mut self.grains {
                if grain.source == Source::Live {
                    grain.active = false;
                }
            }
        }
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let in_l = connected_input(inputs, Self::PORT_IN_L);
        let in_r = connected_input(inputs, Self::PORT_IN_R);
        let freeze_in = connected_input(inputs, Self::PORT_FREEZE);
        let gate = connected_input(inputs, Self::PORT_GATE);
        let trig = connected_input(inputs, Self::PORT_TRIG);
        let position_cv = connected_input(inputs, Self::PORT_POSITION);
        let size_cv = connected_input(inputs, Self::PORT_SIZE);
        let density_cv = connected_input(inputs, Self::PORT_DENSITY);
        let pitch_cv = connected_input(inputs, Self::PORT_PITCH);

        // One side patched plays on both
        let (in_l, in_r) = match (in_l, in_r) {
            (Some(l), None) => (Some(l), Some(l)),
            (None, Some(r)) => (Some(r), Some(r)),
            sides => sides,
        };
        let live = in_l.is_some() && self.ring.len() > 0;
        self.live = live;

        // A cloud per channel of the widest of Pitch and Gate
        let wanted = [gate, pitch_cv].iter().flatten().map(|b| b.channels()).max().unwrap_or(1);
        let channels = wanted.clamp(1, MAX_CHANNELS);
        if channels > self.active {
            for channel in &mut self.channels[self.active..channels] {
                *channel = Channel::default();
            }
        }
        self.active = channels;

        let switch = params[Self::PARAM_FREEZE] >= 0.5;
        let spray = params[Self::PARAM_SPRAY].clamp(0.0, 1.0);
        let spread = params[Self::PARAM_SPREAD].clamp(0.0, 1.0);
        self.level.set_target(params[Self::PARAM_LEVEL].clamp(0.0, 1.0));
        let sample_rate = self.sample_rate as f64;
        let read = |buffer: Option<&SignalBuffer>, c: usize, i: usize| buffer.map_or(0.0, |b| b.voice(c).at(i));

        for out in outputs.iter_mut() {
            out.set_channels(1);
        }

        for i in 0..context.block_size {
            // Live audio in, unless frozen
            self.frozen = match freeze_in {
                Some(buffer) => {
                    self.freeze.tick(buffer.at(i));
                    self.freeze.is_high()
                }
                None => switch,
            };
            if let (Some(l), Some(r), false) = (in_l, in_r, self.frozen) {
                let sum = |buffer: &SignalBuffer| (0..buffer.channels()).map(|c| buffer.voice(c).at(i)).sum::<f32>();
                self.ring.push(sum(l), sum(r));
            }

            // A Trig's edge starts a burst on every channel
            let burst = trig.is_some_and(|t| self.trig.rising(t.at(i)));

            for c in 0..channels {
                let gate_open = gate.is_none_or(|g| {
                    let edge = &mut self.channels[c].gate;
                    edge.tick(g.voice(c).at(i));
                    edge.is_high()
                });
                let size_ms = (params[Self::PARAM_SIZE] * read(size_cv, c, i).exp2()).clamp(MIN_SIZE_MS, MAX_SIZE_MS);
                let density = (params[Self::PARAM_DENSITY] * read(density_cv, c, i).exp2()).clamp(MIN_DENSITY, MAX_DENSITY);
                let due = if trig.is_some() {
                    burst && gate_open
                } else if gate_open {
                    self.channels[c].countdown -= 1.0;
                    self.channels[c].countdown <= 0.0
                } else {
                    // Closed: the next grain comes as it opens
                    self.channels[c].countdown = 0.0;
                    false
                };
                if !due {
                    continue;
                }
                // As loud however many grains overlap, at this density and
                // size; a chord's clouds add up, as a chord's notes do
                let overlap = density * size_ms / 1000.0;
                let gain = 1.0 / overlap.clamp(1.0, GRAINS as f32).sqrt();
                let position = (params[Self::PARAM_POSITION] + read(position_cv, c, i)).clamp(0.0, 1.0);
                if c == 0 {
                    self.position = position;
                }
                let semitones = params[Self::PARAM_PITCH] + 12.0 * read(pitch_cv, c, i);
                let count = if trig.is_some() { (overlap.round() as usize).clamp(1, MAX_BURST) } else { 1 };
                for _ in 0..count {
                    self.spawn(live, position, size_ms, spray, semitones, spread, gain);
                }
                if trig.is_none() {
                    // The next on a beat, or scattered in time by Spray
                    let interval = sample_rate / density as f64;
                    let jitter = 1.0 + 0.9 * spray as f64 * (2.0 * self.unit() - 1.0);
                    self.channels[c].countdown += interval * jitter;
                    if self.channels[c].countdown < 1.0 {
                        self.channels[c].countdown = 1.0;
                    }
                }
            }

            // Every grain
            let Self { grains, sample, outgoing, ring, .. } = self;
            let file = sample.as_deref().map(Tape::File);
            let previous = outgoing.as_deref().map(Tape::File);
            let ring_tape = Tape::Live { left: &ring.left, right: &ring.right };
            let (mut left, mut right) = (0.0, 0.0);
            for grain in grains.iter_mut().filter(|g| g.active) {
                let tape = match grain.source {
                    Source::File => file.as_ref(),
                    Source::Outgoing => previous.as_ref(),
                    Source::Live => Some(&ring_tape),
                };
                let Some(tape) = tape else {
                    grain.active = false;
                    continue;
                };
                let (l, r) = grain.step(tape);
                left += l;
                right += r;
            }
            let gain = self.level.next();
            if let Some(out) = outputs.get_mut(Self::OUT_L) {
                out.samples[i] = left * gain;
            }
            if let Some(out) = outputs.get_mut(Self::OUT_R) {
                out.samples[i] = right * gain;
            }
        }

        if self.outgoing.is_some() {
            self.retire_outgoing_if_unused();
        }

        // The live waveform, a few times a second
        if live {
            self.overview_countdown = self.overview_countdown.saturating_sub(context.block_size);
            if self.overview_countdown == 0 {
                self.ring.line_up();
                self.overview_ready = true;
                self.overview_countdown = (OVERVIEW_SECONDS * self.sample_rate) as usize;
            }
        }
    }

    fn reset(&mut self) {
        // The live audio stays: a frozen moment outlasts the transport
        for grain in &mut self.grains {
            grain.active = false;
        }
        self.channels = [Channel::default(); MAX_CHANNELS];
        self.trig.reset();
        self.freeze.reset();
        if self.outgoing.is_some() {
            self.retire_outgoing_if_unused();
        }
    }

    fn seed(&mut self, seed: u64) {
        self.rng = Pcg32::new(seed);
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        // Where each grain reads, as a fraction of its source
        let (_, live_end) = self.ring.held();
        let len = self.ring.len().max(1) as f64;
        let frames = self.sample.as_ref().map_or(1, |s| s.frames()).max(1) as f64;
        let outgoing_frames = self.outgoing.as_ref().map_or(1, |s| s.frames()).max(1) as f64;
        let place = |grain: &Grain| -> f32 {
            match grain.source {
                // Live: across the ring as the face draws it, oldest on the
                // left; while it fills, from its first frame
                Source::Live => ((grain.position - (live_end - len).max(0.0)) / len) as f32,
                Source::File => (grain.position.rem_euclid(frames) / frames) as f32,
                Source::Outgoing => (grain.position.rem_euclid(outgoing_frames) / outgoing_frames) as f32,
            }
        };
        // The loudest, without sorting the pool
        let mut shown = [usize::MAX; SHOWN];
        for (index, grain) in self.grains.iter().enumerate().filter(|(_, g)| g.active) {
            let loudness = |k: usize| self.grains.get(k).map_or(-1.0, |g| g.level);
            if let Some(quietest) = (0..SHOWN).min_by(|&a, &b| loudness(shown[a]).total_cmp(&loudness(shown[b]))) {
                if loudness(shown[quietest]) < grain.level {
                    shown[quietest] = index;
                }
            }
        }
        for (k, &index) in shown.iter().enumerate() {
            if let Some(grain) = self.grains.get(index) {
                readout.values[Self::READOUT_GRAINS + k] = Sampler::pack_playhead(place(grain), grain.level);
                readout.values[Self::READOUT_PANS + k] = grain.pan;
            }
        }
        readout.values[Self::READOUT_LIVE] = if self.live { 1.0 } else { 0.0 };
        readout.values[Self::READOUT_FILLED] = self.ring.filled();
        readout.values[Self::READOUT_FROZEN] = if self.frozen { 1.0 } else { 0.0 };
        readout.values[Self::READOUT_COUNT] = self.playing() as f32;
        readout.values[Self::READOUT_POSITION] = self.position;
        const { assert!(Self::READOUT_POSITION < MAX_READOUT) };
        Some(readout)
    }

    fn take_scope_data(&mut self) -> Option<(&[f32], &[f32], bool)> {
        if !std::mem::take(&mut self.overview_ready) {
            return None;
        }
        Some((&self.ring.sent_highs, &self.ring.sent_lows, false))
    }

    fn polyphonic(&self) -> bool {
        true
    }

    fn load_sample(&mut self, sample: Option<Arc<SampleData>>) -> Option<Arc<SampleData>> {
        // Still playing out an older recording: those grains stop now
        if let Some(older) = self.outgoing.take() {
            for grain in &mut self.grains {
                if grain.source == Source::Outgoing {
                    grain.active = false;
                }
            }
            self.retire(older);
        }
        let old = std::mem::replace(&mut self.sample, sample);
        // Grains on the old recording play out on it
        let mut playing = false;
        for grain in &mut self.grains {
            if grain.active && grain.source == Source::File {
                grain.source = Source::Outgoing;
                playing = true;
            }
        }
        if playing {
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
    use crate::dsp::analysis::rms;
    use std::f32::consts::TAU;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 128;

    fn params(changes: &[(usize, f32)]) -> Vec<f32> {
        let mut params: Vec<f32> = Cloud::new().parameters().iter().map(|p| p.default).collect();
        for &(index, value) in changes {
            params[index] = value;
        }
        params
    }

    fn sine(hz: f32, seconds: f32) -> Arc<SampleData> {
        let frames = (seconds * SR) as usize;
        Arc::new(SampleData::mono((0..frames).map(|n| (TAU * hz * n as f32 / SR).sin()).collect(), SR))
    }

    fn cloud() -> Cloud {
        let mut cloud = Cloud::new();
        cloud.prepare(SR, BLOCK);
        cloud.seed(7);
        cloud
    }

    type Mono<'a> = &'a dyn Fn(usize) -> f32;
    type Poly<'a> = &'a dyn Fn(usize, usize) -> f32;

    /// What's patched in, sample by sample (and by channel, where it's poly).
    #[derive(Default)]
    struct Patched<'a> {
        input: Option<Mono<'a>>,
        freeze: Option<Mono<'a>>,
        trig: Option<Mono<'a>>,
        gate: Option<(usize, Poly<'a>)>,
        /// Each channel's pitch, held.
        pitch: Option<(usize, Mono<'a>)>,
    }

    /// Plays `seconds`, returning (left, right).
    fn run(cloud: &mut Cloud, params: &[f32], seconds: f32, patched: &Patched) -> (Vec<f32>, Vec<f32>) {
        let total = (seconds * SR) as usize;
        let mono = |patched: bool, signal_type| if patched { SignalBuffer::new(BLOCK, signal_type) } else { SignalBuffer::unconnected(BLOCK, signal_type) };
        let poly = |patched: bool, signal_type| if patched { SignalBuffer::polyphonic(BLOCK, signal_type) } else { SignalBuffer::unconnected(BLOCK, signal_type) };
        let mut input = mono(patched.input.is_some(), SignalType::Audio);
        let in_r = SignalBuffer::unconnected(BLOCK, SignalType::Audio);
        let mut freeze = mono(patched.freeze.is_some(), SignalType::Gate);
        let mut trig = mono(patched.trig.is_some(), SignalType::Gate);
        let mut gate = poly(patched.gate.is_some(), SignalType::Gate);
        let mut pitch = poly(patched.pitch.is_some(), SignalType::Control);
        if let Some((channels, _)) = patched.gate {
            gate.set_channels(channels);
        }
        if let Some((channels, _)) = patched.pitch {
            pitch.set_channels(channels);
        }
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio), SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let context = ProcessContext::new(SR, BLOCK);
        let mut n = 0;
        while n < total {
            for i in 0..BLOCK {
                let at = n + i;
                if let Some(f) = patched.input {
                    input.samples[i] = f(at);
                }
                if let Some(f) = patched.freeze {
                    freeze.samples[i] = f(at);
                }
                if let Some(f) = patched.trig {
                    trig.samples[i] = f(at);
                }
                if let Some((channels, f)) = patched.gate {
                    for c in 0..channels {
                        gate.channel_mut(c)[i] = f(c, at);
                    }
                }
                if let Some((channels, f)) = patched.pitch {
                    for c in 0..channels {
                        pitch.channel_mut(c)[i] = f(c);
                    }
                }
            }
            let inputs: [&SignalBuffer; 9] = [&input, &in_r, &freeze, &gate, &trig, &unpatched, &unpatched, &unpatched, &pitch];
            cloud.process(&inputs, &mut outputs, params, &context);
            assert_eq!(outputs[0].channels(), 1, "a stereo pair, however many clouds");
            left.extend_from_slice(&outputs[0].samples);
            right.extend_from_slice(&outputs[1].samples);
            n += BLOCK;
        }
        (left, right)
    }

    /// How strong `hz` is in `samples`: about 1 for a full-scale sine.
    fn strength(samples: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &x) in samples.iter().enumerate() {
            let phase = (TAU * hz / SR) as f64 * n as f64;
            re += x as f64 * phase.cos();
            im += x as f64 * phase.sin();
        }
        (2.0 * (re * re + im * im).sqrt() / samples.len() as f64) as f32
    }

    /// How strong the band 50 Hz either side of `hz` is. Grains replaying
    /// one place restart its tone at another phase each time, which spreads
    /// it into sidebands at Density's rate either side.
    fn band(samples: &[f32], hz: f32) -> f32 {
        (-10..=10).map(|k| strength(samples, hz + 5.0 * k as f32).powi(2)).sum::<f32>().sqrt()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |m, x| m.max(x.abs()))
    }

    fn largest_step(samples: &[f32]) -> f32 {
        samples.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn test_ports_and_parameters() {
        let cloud = Cloud::new();
        let names: Vec<&str> = cloud.ports().iter().map(|p| p.name).collect();
        assert_eq!(names, ["In L", "In R", "Freeze", "Gate", "Trig", "Position", "Size", "Density", "Pitch", "L", "R"]);
        let params: Vec<&str> = cloud.parameters().iter().map(|p| p.name).collect();
        assert_eq!(params, ["Freeze", "Position", "Size", "Density", "Pitch", "Spray", "Spread", "Level"]);
        assert_eq!(params.len(), Cloud::PARAM_COUNT);
        assert!(cloud.polyphonic());
        const { assert!(Cloud::READOUT_POSITION < MAX_READOUT) };
    }

    #[test]
    fn test_silent_without_a_source() {
        let mut cloud = cloud();
        let (left, right) = run(&mut cloud, &params(&[]), 0.5, &Patched::default());
        assert_eq!(peak(&left) + peak(&right), 0.0);
    }

    #[test]
    fn test_plays_a_file_at_its_pitch_and_up_an_octave() {
        let mut cloud = cloud();
        cloud.load_sample(Some(sine(440.0, 2.0)));
        let (left, _) = run(&mut cloud, &params(&[(Cloud::PARAM_SPREAD, 0.0)]), 1.0, &Patched::default());
        let tail = &left[SR as usize / 4..];
        assert!(rms(tail) > 0.1, "it plays: {}", rms(tail));
        assert!(strength(tail, 440.0) > 4.0 * strength(tail, 880.0));

        let mut cloud = cloud_fresh();
        cloud.load_sample(Some(sine(440.0, 2.0)));
        let (left, _) = run(&mut cloud, &params(&[(Cloud::PARAM_SPREAD, 0.0), (Cloud::PARAM_PITCH, 12.0)]), 1.0, &Patched::default());
        let tail = &left[SR as usize / 4..];
        assert!(strength(tail, 880.0) > 4.0 * strength(tail, 440.0));
    }

    fn cloud_fresh() -> Cloud {
        cloud()
    }

    #[test]
    fn test_small_grains_do_not_click() {
        // A low tone, so any jump would be the window's fault
        for size in [MIN_SIZE_MS, 5.0, 20.0] {
            let mut cloud = cloud();
            cloud.load_sample(Some(sine(100.0, 2.0)));
            let p = params(&[(Cloud::PARAM_SIZE, size), (Cloud::PARAM_DENSITY, MAX_DENSITY), (Cloud::PARAM_SPRAY, 1.0), (Cloud::PARAM_LEVEL, 1.0)]);
            let (left, right) = run(&mut cloud, &p, 1.0, &Patched::default());
            let loudest = peak(&left).max(peak(&right));
            assert!(loudest > 0.01, "{size} ms plays");
            // A window rising over half the grain moves at most this far a
            // sample, times how loud the grain is
            let window_step = std::f32::consts::PI / (size / 1000.0 * SR);
            let step = largest_step(&left).max(largest_step(&right));
            assert!(step < 2.0 * window_step * loudest.max(0.5) + 0.02, "{size} ms: a step of {step}");
        }
    }

    #[test]
    fn test_pitched_up_grains_do_not_alias() {
        // 9 kHz two octaves up is 36 kHz, past Nyquist: all of it would
        // fold back down to 12 kHz. Through the lowpass, almost none is left
        let level = |hz: f32| {
            let mut cloud = cloud();
            cloud.load_sample(Some(sine(hz, 2.0)));
            let p = params(&[(Cloud::PARAM_PITCH, 24.0), (Cloud::PARAM_SPREAD, 0.0), (Cloud::PARAM_SIZE, 80.0)]);
            let (left, _) = run(&mut cloud, &p, 1.0, &Patched::default());
            rms(&left[SR as usize / 4..])
        };
        let passed = level(1000.0);
        let folded = level(9000.0);
        assert!(passed > 0.05, "4 kHz plays: {passed}");
        let db = 20.0 * (folded / passed).log10();
        assert!(db < -40.0, "what folds back is {db:.1} dB");
    }

    #[test]
    fn test_a_poly_chord_grows_a_cloud_of_each_note() {
        let mut cloud = cloud();
        cloud.load_sample(Some(sine(220.0, 2.0)));
        // A major triad: root, third, fifth
        let chord = [0.0, 4.0 / 12.0, 7.0 / 12.0];
        let pitch = |c: usize| chord[c];
        let p = params(&[(Cloud::PARAM_SPREAD, 0.0), (Cloud::PARAM_SIZE, 200.0)]);
        let (left, _) = run(&mut cloud, &p, 1.5, &Patched { pitch: Some((3, &pitch)), ..Default::default() });
        let tail = &left[SR as usize / 2..];
        let between = strength(tail, 250.0);
        for semitones in [0.0, 4.0, 7.0] {
            let hz = 220.0 * (semitones / 12.0f32).exp2();
            assert!(strength(tail, hz) > 4.0 * between, "{hz} Hz is in the cloud");
        }
    }

    #[test]
    fn test_a_closed_gate_holds_its_cloud_back() {
        let mut cloud = cloud();
        cloud.load_sample(Some(sine(220.0, 2.0)));
        let closed = |_: usize, _: usize| 0.0;
        let (left, _) = run(&mut cloud, &params(&[]), 0.5, &Patched { gate: Some((1, &closed)), ..Default::default() });
        assert_eq!(peak(&left), 0.0);
        // Opened a quarter of a second in, it plays from then on
        let opens = |_: usize, n: usize| if n > SR as usize / 4 { 1.0 } else { 0.0 };
        let mut cloud = cloud_fresh();
        cloud.load_sample(Some(sine(220.0, 2.0)));
        let (left, _) = run(&mut cloud, &params(&[]), 0.5, &Patched { gate: Some((1, &opens)), ..Default::default() });
        assert_eq!(peak(&left[..SR as usize / 4]), 0.0);
        assert!(peak(&left[SR as usize / 4..]) > 0.05);
    }

    #[test]
    fn test_trig_starts_a_burst_and_nothing_between() {
        let mut cloud = cloud();
        cloud.load_sample(Some(sine(220.0, 2.0)));
        // One edge at 0.1 s; 20 ms grains are over well before 0.15 s
        let trig = |n: usize| if (4800..4900).contains(&n) { 1.0 } else { 0.0 };
        let p = params(&[(Cloud::PARAM_SIZE, 20.0), (Cloud::PARAM_DENSITY, 200.0)]);
        let (left, _) = run(&mut cloud, &p, 0.5, &Patched { trig: Some(&trig), ..Default::default() });
        assert_eq!(peak(&left[..4800]), 0.0);
        assert!(peak(&left[4800..4800 + 960]) > 0.05);
        assert_eq!(peak(&left[4800 + 2000..]), 0.0);
    }

    #[test]
    fn test_plays_live_input_and_freeze_holds_a_moment_of_it() {
        // 300 Hz for a second, then 500 Hz
        let input = |n: usize| {
            let hz = if n < SR as usize { 300.0 } else { 500.0 };
            (TAU * hz * n as f32 / SR).sin()
        };
        let p = params(&[(Cloud::PARAM_POSITION, 1.0), (Cloud::PARAM_SPRAY, 0.0), (Cloud::PARAM_SPREAD, 0.0)]);

        // Unfrozen, it follows the input to 500 Hz
        let mut cloud = cloud();
        let (left, _) = run(&mut cloud, &p, 2.5, &Patched { input: Some(&input), ..Default::default() });
        assert_eq!(cloud.readout(&p).unwrap().values[Cloud::READOUT_LIVE], 1.0);
        let end = &left[2 * SR as usize..];
        assert!(rms(end) > 0.05, "plays the live input");
        assert!(band(end, 500.0) > 4.0 * band(end, 300.0));

        // Frozen at 1 s, it holds 300 Hz
        let freeze = |n: usize| if n >= SR as usize { 1.0 } else { 0.0 };
        let mut cloud = cloud_fresh();
        let (left, _) = run(&mut cloud, &p, 2.5, &Patched { input: Some(&input), freeze: Some(&freeze), ..Default::default() });
        let end = &left[2 * SR as usize..];
        assert!(rms(end) > 0.05, "plays the frozen moment");
        assert!(band(end, 300.0) > 4.0 * band(end, 500.0), "300 Hz: {}, 500 Hz: {}", band(end, 300.0), band(end, 500.0));
        assert_eq!(cloud.readout(&p).unwrap().values[Cloud::READOUT_FROZEN], 1.0);
    }

    #[test]
    fn test_live_waits_for_a_grains_worth() {
        let input = |n: usize| (TAU * 200.0 * n as f32 / SR).sin();
        let mut cloud = cloud();
        let p = params(&[(Cloud::PARAM_SIZE, 500.0)]);
        let (left, _) = run(&mut cloud, &p, 0.3, &Patched { input: Some(&input), ..Default::default() });
        assert_eq!(peak(&left), 0.0, "not yet half a second recorded");
    }

    #[test]
    fn test_a_new_file_lets_the_old_grains_play_out_then_hands_it_back() {
        let mut cloud = cloud();
        let p = params(&[(Cloud::PARAM_SIZE, 100.0)]);
        cloud.load_sample(Some(sine(220.0, 1.0)));
        run(&mut cloud, &p, 0.3, &Patched::default());
        assert!(cloud.playing() > 0);
        assert!(cloud.load_sample(Some(sine(330.0, 1.0))).is_none(), "kept while its grains play out");
        assert!(cloud.take_retired_sample().is_none());
        run(&mut cloud, &p, 0.2, &Patched::default());
        let old = cloud.take_retired_sample().expect("handed back once they're done");
        assert_eq!(old.frames(), SR as usize);
    }

    #[test]
    fn test_the_same_seed_plays_the_same_cloud() {
        let play = || {
            let mut cloud = cloud();
            cloud.load_sample(Some(sine(220.0, 1.0)));
            run(&mut cloud, &params(&[(Cloud::PARAM_SPRAY, 1.0)]), 0.3, &Patched::default()).0
        };
        assert_eq!(play(), play());
    }

    #[test]
    fn test_readout_shows_the_loudest_grains() {
        let mut cloud = cloud();
        cloud.load_sample(Some(sine(220.0, 1.0)));
        let p = params(&[(Cloud::PARAM_DENSITY, 100.0), (Cloud::PARAM_SIZE, 300.0)]);
        run(&mut cloud, &p, 0.5, &Patched::default());
        let readout = cloud.readout(&p).unwrap();
        let shown: Vec<(f32, f32)> = (0..SHOWN).filter_map(|k| Sampler::unpack_playhead(readout.values[Cloud::READOUT_GRAINS + k])).collect();
        assert_eq!(shown.len(), SHOWN, "more than {SHOWN} play, so as many show");
        assert!(readout.values[Cloud::READOUT_COUNT] as usize > SHOWN);
        assert!(shown.iter().all(|&(place, level)| (0.0..=1.0).contains(&place) && level > 0.0));
    }

    #[test]
    fn test_never_more_than_the_pool_and_never_too_loud() {
        let mut cloud = cloud();
        cloud.load_sample(Some(sine(220.0, 2.0)));
        let chord = |c: usize| c as f32 / 12.0;
        let p = params(&[(Cloud::PARAM_DENSITY, MAX_DENSITY), (Cloud::PARAM_SIZE, MAX_SIZE_MS), (Cloud::PARAM_LEVEL, 1.0)]);
        let (left, _) = run(&mut cloud, &p, 1.0, &Patched { pitch: Some((8, &chord)), ..Default::default() });
        assert_eq!(cloud.playing(), GRAINS);
        assert!(peak(&left) < 4.0, "loudness is kept in hand: {}", peak(&left));
    }
}
