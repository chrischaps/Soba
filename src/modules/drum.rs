//! Drum voice module.
//!
//! One analog-style voice that plays any of ten drums, the way an 808 or a
//! 909 makes each of its sounds from a few oscillators, a noise source,
//! filters and decays. Every type reads the same five knobs (Tune, Decay,
//! Tone, Snap, Level), each meaning what it would on that drum's own panel,
//! so a MIDI mapping keeps working when the Type changes.
//!
//! A hit is shaped by a [`Voicing`]: the envelopes and the pitch sweep, in
//! closed form. The voice follows it sample by sample, and the node display
//! draws it, so the picture is the sound.

use std::f32::consts::{LN_2, TAU};

use super::noise::{soft_ceiling, Pcg32};
use crate::dsp::{
    connected_input,
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{prewarp, BlepDelay},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// The drums, in the Type menu's order. Saved patches store the index, so
/// new types go at the end.
pub const DRUM_TYPES: &[&str] = &["Kick", "Snare", "Tom", "Clap", "Closed Hat", "Open Hat", "Cymbal", "Rim", "Cowbell", "Shaker"];

/// Which drum a voice plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrumType {
    Kick,
    Snare,
    Tom,
    Clap,
    ClosedHat,
    OpenHat,
    Cymbal,
    Rim,
    Cowbell,
    Shaker,
}

impl DrumType {
    /// Every type, in menu order.
    pub const ALL: [DrumType; 10] = [
        DrumType::Kick,
        DrumType::Snare,
        DrumType::Tom,
        DrumType::Clap,
        DrumType::ClosedHat,
        DrumType::OpenHat,
        DrumType::Cymbal,
        DrumType::Rim,
        DrumType::Cowbell,
        DrumType::Shaker,
    ];

    /// The type a Type parameter value selects.
    pub fn from_param(value: f32) -> Self {
        Self::ALL[(value.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }

    /// Its name in the Type menu.
    pub fn name(self) -> &'static str {
        DRUM_TYPES[self as usize]
    }

    /// The frequencies of its square oscillators, as ratios of the lowest,
    /// or nothing for the types without any.
    pub fn partials(self) -> &'static [f32] {
        match self {
            DrumType::ClosedHat | DrumType::OpenHat | DrumType::Cymbal => &METAL_RATIOS,
            DrumType::Cowbell => &COWBELL_RATIOS,
            _ => &[],
        }
    }

    /// Whether a hit glides down in pitch, as skins do.
    pub fn is_swept(self) -> bool {
        matches!(self, DrumType::Kick | DrumType::Snare | DrumType::Tom | DrumType::Rim)
    }

    /// Whether it's noise in one band, whose centre is its pitch.
    pub fn is_band(self) -> bool {
        matches!(self, DrumType::Clap | DrumType::Shaker)
    }
}

/// The 808's six cymbal and hi-hat squares (205.3, 304.4, 369.6, 522.7,
/// 540 and 800 Hz) as ratios of the lowest. No two are in a simple ratio,
/// so their sum rings like metal rather than a chord.
const METAL_RATIOS: [f32; 6] = [1.0, 1.4827, 1.8003, 2.5460, 2.6303, 3.8968];
const METAL_HZ: f32 = 205.3;

/// The 808 cowbell: two squares a sixth apart, 540 and 800 Hz.
const COWBELL_RATIOS: [f32; 2] = [1.0, 1.4815];
const COWBELL_HZ: f32 = 540.0;

/// Where a clap's four noise bursts start, in seconds. A little uneven, as
/// a few hands never quite clap together. The last starts the tail.
pub const CLAP_BURSTS: [f32; 4] = [0.0, 0.0105, 0.0195, 0.0300];

/// The centre of a Shaker's band at Tune 0 and Tone 50%: Tone moves it
/// from 5 to 9 kHz.
const SHAKER_HZ: f32 = 6708.0;
const SHAKER_TONE_SPAN: f32 = 1.8;

/// How long a Shaker's grain takes to swell in, in seconds, at full Accent
/// and at none: a looser wrist lets the beads arrive more slowly.
const SHAKER_ATTACK: (f32, f32) = (0.003, 0.008);

/// How far apart a Shaker's beads land, in seconds: each one moves the
/// grain's level, by as much as Snap allows.
const BEAD_GAP: (f32, f32) = (0.0006, 0.0022);

/// The Snare's second drumhead mode, as a ratio of its first.
const SNARE_MODE_RATIO: f32 = 1.78;

/// The Rim's low mode, as a ratio of its high one (455 and 1667 Hz on the 808).
const RIM_MODE_RATIO: f32 = 0.273;

/// Time constant of a choke: a rising Choke takes a voice down 62 dB in 5 ms.
const CHOKE_TAU: f32 = 0.0007;

/// Time constant over which a voice hit while still ringing lets go of
/// what it was playing, so the restart doesn't click.
const DECLICK_TAU: f32 = 0.0015;

/// Below this the voice has finished and stops working until the next hit.
const SILENCE: f32 = 1e-5;

/// Seconds from a decay time to its time constant: a decay is the time to
/// fall 60 dB.
const T60_PER_TAU: f32 = 6.907_755;

/// A decay of `decay` (0–1) between `shortest` and `longest`, in seconds to
/// fall 60 dB, on a logarithmic sweep.
fn span(decay: f32, shortest: f32, longest: f32) -> f32 {
    shortest * (longest / shortest).powf(decay.clamp(0.0, 1.0))
}

/// One exponential decay: starts at `gain` and falls by e every `tau` seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Decay {
    pub gain: f32,
    pub tau: f32,
}

impl Decay {
    fn new(gain: f32, seconds_to_silence: f32) -> Self {
        Self { gain, tau: seconds_to_silence / T60_PER_TAU }
    }

    /// Its level `t` seconds into the hit.
    pub fn at(&self, t: f32) -> f32 {
        if self.gain == 0.0 || t < 0.0 {
            0.0
        } else {
            self.gain * (-t / self.tau.max(1e-6)).exp()
        }
    }

    /// The per-sample multiplier that follows it.
    fn coefficient(&self, sample_rate: f32) -> f32 {
        (-1.0 / (self.tau.max(1e-6) * sample_rate)).exp()
    }
}

/// The knob settings a hit is struck with, after CV.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strike {
    /// Semitones from the drum's own pitch.
    pub tune: f32,
    /// 0 to 1, shortest to longest.
    pub decay: f32,
    /// 0 to 1, dark to bright.
    pub tone: f32,
    /// 0 to 1: the noise or click in the sound.
    pub snap: f32,
    /// 0 to 1: how hard it's hit.
    pub accent: f32,
}

impl Default for Strike {
    fn default() -> Self {
        Self { tune: 0.0, decay: 0.5, tone: 0.5, snap: 0.5, accent: 1.0 }
    }
}

/// Everything about how one hit sounds: its pitch and sweep, its three
/// decays, and its colour. Built once per hit, from the type and the knobs.
///
/// What the three decays carry depends on the drum: for a kick, `body` is
/// the swept sine and `click` the beater; for a snare, `noise` is the
/// wires; for a clap, `click` is the bursts and `noise` the tail. The node
/// display only needs their sum, [`amplitude_at`](Self::amplitude_at).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voicing {
    pub kind: DrumType,
    /// The resting pitch: the body's, or the lowest square's.
    pub pitch_hz: f32,
    /// How far above `pitch_hz` the hit starts, in octaves.
    pub sweep_octaves: f32,
    /// Time constant of the fall from there, in seconds.
    pub sweep_tau: f32,
    pub body: Decay,
    pub noise: Decay,
    pub click: Decay,
    /// Time constant of a swell into the hit, in seconds, or 0 for a hit
    /// that starts at full level. The decays are scaled by `1 - e^(-t/swell)`.
    pub swell: f32,
    /// Saturation of a sine body: 0 is clean.
    pub drive: f32,
    /// The Tone and Snap the hit uses, after Accent.
    pub tone: f32,
    pub snap: f32,
    /// The whole hit's gain: Accent times the type's own level.
    pub gain: f32,
}

impl Voicing {
    /// The voicing of a hit on `kind` struck with `strike`.
    pub fn new(kind: DrumType, strike: Strike) -> Self {
        let accent = strike.accent.clamp(0.0, 1.0);
        // Softer hits are darker and less snappy, as on a real drum
        let tone = (strike.tone - 0.25 * (1.0 - accent)).clamp(0.0, 1.0);
        let snap = strike.snap.clamp(0.0, 1.0) * (0.5 + 0.5 * accent);
        let decay = strike.decay;
        let transpose = (strike.tune.clamp(-48.0, 48.0) / 12.0).exp2();

        let mut v = Self {
            kind,
            pitch_hz: 0.0,
            sweep_octaves: 0.0,
            sweep_tau: 0.01,
            body: Decay::default(),
            noise: Decay::default(),
            click: Decay::default(),
            swell: 0.0,
            drive: 0.0,
            tone,
            snap,
            gain: accent * Self::level_of(kind),
        };
        match kind {
            DrumType::Kick => {
                v.pitch_hz = 48.0 * transpose;
                // Snap is the beater: a deeper drop and a sharper click.
                // Hit softly, the drop is a little shallower
                v.sweep_octaves = (0.6 + 3.4 * snap) * (0.8 + 0.2 * accent);
                v.sweep_tau = 0.011;
                v.body = Decay::new(1.0, span(decay, 0.12, 2.5));
                v.click = Decay::new(0.45 * snap, 0.012);
                v.drive = 0.3 + 3.2 * tone * tone;
            }
            DrumType::Snare => {
                v.pitch_hz = 185.0 * transpose;
                v.sweep_octaves = 0.35;
                v.sweep_tau = 0.006;
                v.body = Decay::new(1.0 - 0.35 * snap, 0.6 * span(decay, 0.07, 0.6));
                v.noise = Decay::new(0.15 + 1.1 * snap, span(decay, 0.08, 0.8));
            }
            DrumType::Tom => {
                v.pitch_hz = 110.0 * transpose;
                v.sweep_octaves = 0.5;
                v.sweep_tau = 0.035;
                v.body = Decay::new(1.0, span(decay, 0.12, 1.8));
                v.click = Decay::new(0.5 * snap, 0.025);
                v.drive = 0.2 + 1.8 * tone * tone;
            }
            DrumType::Clap => {
                // Tune moves the band the hands ring in
                v.pitch_hz = 1150.0 * transpose;
                v.click = Decay::new(0.35 + 0.65 * snap, 0.02);
                v.noise = Decay::new(0.55, span(decay, 0.08, 1.0));
            }
            DrumType::ClosedHat | DrumType::OpenHat => {
                v.pitch_hz = METAL_HZ * transpose;
                let length = if kind == DrumType::ClosedHat { span(decay, 0.025, 0.3) } else { span(decay, 0.15, 1.8) };
                v.body = Decay::new(1.0, length);
            }
            DrumType::Cymbal => {
                v.pitch_hz = METAL_HZ * transpose;
                // A bright splash over a long wash
                v.click = Decay::new(0.6, 0.25);
                v.body = Decay::new(0.5, span(decay, 0.6, 5.0));
            }
            DrumType::Rim => {
                v.pitch_hz = 1667.0 * transpose;
                v.sweep_octaves = 0.15;
                v.sweep_tau = 0.003;
                v.body = Decay::new(1.0, span(decay, 0.015, 0.2));
                v.click = Decay::new(0.6 * snap, 0.008);
                v.drive = 0.5 + 2.0 * tone;
            }
            DrumType::Cowbell => {
                v.pitch_hz = COWBELL_HZ * transpose;
                // Snap is the clank of the strike, over the ring
                v.click = Decay::new(0.3 + 0.7 * snap, 0.05);
                v.body = Decay::new(0.45, span(decay, 0.1, 1.2));
            }
            DrumType::Shaker => {
                // Tune and Tone both move the band the beads hiss in
                v.pitch_hz = SHAKER_HZ * transpose * SHAKER_TONE_SPAN.powf(tone - 0.5);
                v.body = Decay::new(1.0, span(decay, 0.04, 0.2));
                // Nearly at full level after three time constants
                let (sharpest, softest) = SHAKER_ATTACK;
                v.swell = (sharpest + (softest - sharpest) * (1.0 - accent)) / 3.0;
            }
        }
        v
    }

    /// Each type's level at full Accent and default knobs: peaks of 0.8 for
    /// the kick, 0.75 for snare and tom, 0.7 for the clap, and 0.5 to 0.6
    /// for the metal and the rim, which sit back in a kit. Below the output's
    /// soft ceiling (from 0.8), so the knobs have room before it rounds them.
    fn level_of(kind: DrumType) -> f32 {
        match kind {
            DrumType::Kick => 0.74,
            DrumType::Snare => 0.64,
            DrumType::Tom => 0.71,
            DrumType::Clap => 1.57,
            DrumType::ClosedHat | DrumType::OpenHat => 0.87,
            DrumType::Cymbal => 0.82,
            DrumType::Rim => 0.53,
            DrumType::Cowbell => 0.69,
            DrumType::Shaker => 1.0,
        }
    }

    /// How loud the hit's envelope is `t` seconds in, before the gain.
    pub fn amplitude_at(&self, t: f32) -> f32 {
        if t < 0.0 {
            return 0.0;
        }
        let rise = if self.swell > 0.0 { 1.0 - (-t / self.swell).exp() } else { 1.0 };
        rise * match self.kind {
            DrumType::Clap => {
                // Each burst strikes afresh; the tail starts with the last
                let last = CLAP_BURSTS.iter().rev().find(|&&start| t >= start).copied().unwrap_or(0.0);
                self.click.at(t - last) + self.noise.at(t - CLAP_BURSTS[3])
            }
            _ => self.body.at(t) + self.noise.at(t) + self.click.at(t),
        }
    }

    /// The body's pitch `t` seconds in, for the drums that sweep.
    pub fn pitch_at(&self, t: f32) -> Option<f32> {
        self.kind
            .is_swept()
            .then(|| self.pitch_hz * (self.sweep_octaves * (-t.max(0.0) / self.sweep_tau).exp()).exp2())
    }

    /// How long the hit lasts, in seconds: until it's 50 dB below its start.
    pub fn length(&self) -> f32 {
        let longest = [self.body, self.noise, self.click]
            .iter()
            .filter(|d| d.gain > 0.0)
            .map(|d| d.tau * 50.0 / 20.0 * std::f32::consts::LN_10 + if self.kind == DrumType::Clap { CLAP_BURSTS[3] } else { 0.0 })
            .fold(0.0, f32::max);
        longest.max(0.01)
    }
}

/// A topology-preserving state-variable filter, giving lowpass, bandpass
/// and highpass at once.
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

/// Coefficients for an [`Svf`] at one cutoff and damping.
#[derive(Clone, Copy, Debug, Default)]
struct SvfTuning {
    a1: f32,
    a2: f32,
    a3: f32,
    /// Damping, 1/Q.
    k: f32,
}

impl SvfTuning {
    fn new(cutoff_hz: f32, q: f32, sample_rate: f32) -> Self {
        let g = prewarp(cutoff_hz, sample_rate);
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2, k }
    }
}

/// The three outputs of one [`Svf`] step. `band` is scaled to unit gain at
/// the cutoff.
#[derive(Clone, Copy, Debug)]
struct SvfOut {
    low: f32,
    band: f32,
    high: f32,
}

impl Svf {
    #[inline]
    fn tick(&mut self, x: f32, c: &SvfTuning) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut { low: v2, band: c.k * v1, high: x - c.k * v1 - v2 }
    }
}

/// Up to six free-running square waves summed, with band-limited edges.
///
/// They run on between hits, as the 808's do, so no two hits of a hat
/// catch the squares at quite the same place.
#[derive(Clone, Copy, Debug, Default)]
struct SquareBank {
    phase: [f32; 6],
    /// Cycles per sample of each square; only the first `count` sound.
    increment: [f32; 6],
    count: usize,
    blep: BlepDelay,
}

impl SquareBank {
    fn tune(&mut self, lowest_hz: f32, ratios: &[f32], sample_rate: f32) {
        self.count = ratios.len().min(6);
        for (increment, ratio) in self.increment.iter_mut().zip(ratios) {
            *increment = (lowest_hz * ratio / sample_rate).min(0.45);
        }
    }

    /// The next sample of the sum, between -1 and 1.
    #[inline]
    fn next(&mut self) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        let scale = 1.0 / self.count as f32;
        let mut naive = 0.0;
        for (phase, &increment) in self.phase.iter_mut().zip(&self.increment).take(self.count) {
            let before = *phase;
            let mut after = before + increment;
            // High for the first half of each cycle, low for the second
            if before < 0.5 && after >= 0.5 {
                self.blep.step((0.5 - before) / increment, -2.0 * scale);
            }
            if after >= 1.0 {
                after -= 1.0;
                self.blep.step((1.0 - before) / increment, 2.0 * scale);
            }
            *phase = after;
            naive += if after < 0.5 { scale } else { -scale };
        }
        self.blep.push(naive)
    }
}

/// Soft saturation of a sine body: `drive` 0 is clean; more rounds it
/// towards a square, keeping its peak at 1.
#[inline]
fn saturate(x: f32, drive: f32) -> f32 {
    if drive < 0.01 {
        x
    } else {
        (x * drive).tanh() / drive.tanh()
    }
}

/// A drum voice: kick, snare, tom, clap, closed and open hat, cymbal, rim,
/// cowbell and shaker.
///
/// # Ports
///
/// **Inputs:**
/// - **Trig** (Gate): A rising edge strikes the drum.
/// - **Accent** (Control): How hard each hit is, 0 to 1, read as it lands.
///   Softer hits are quieter and darker. Unpatched, every hit is full.
/// - **Choke** (Gate): A rising edge damps the voice within 5 ms.
/// - **Tune** (Control): V/Oct added to the Tune knob, read at each hit.
/// - **Decay** (Control): Added to the Decay knob, read at each hit.
///
/// **Outputs:**
/// - **Out** (Audio): The drum.
///
/// # Parameters
///
/// - **Type**: Which drum.
/// - **Tune** (±24 st), **Decay**, **Tone**, **Snap** (0-1): Read at each
///   hit, so a hit keeps its sound however the knobs turn while it rings.
/// - **Level** (0-1): The output level, which follows the knob at once.
pub struct Drum {
    rng: Pcg32,
    sample_rate: f32,

    /// The hit playing.
    voicing: Voicing,
    playing: bool,
    /// Samples since the hit, and since the last trigger for the readout.
    elapsed: u64,
    struck: bool,
    accent: f32,
    /// When the Choke landed, in samples after the hit.
    choked_at: Option<u64>,

    // The hit's envelopes, each stepped by its own multiplier
    body: f32,
    noise: f32,
    click: f32,
    body_step: f32,
    noise_step: f32,
    click_step: f32,
    /// Octaves above the resting pitch, falling.
    sweep: f32,
    sweep_step: f32,
    /// How far the hit has still to swell in: 1 at the strike, falling to 0.
    swell: f32,
    swell_step: f32,
    /// A shaker grain's level as its beads land: where it is, where the
    /// last bead sent it, and the samples until the next.
    rattle: f32,
    rattle_target: f32,
    bead_in: u32,
    /// Which clap burst comes next, and the sample it starts on.
    next_burst: usize,
    burst_samples: [u64; 4],

    // Sound sources and filters
    phase: [f32; 2],
    squares: SquareBank,
    filter: [Svf; 2],
    tuning: [SvfTuning; 2],
    /// A gentle lowpass over the metal, so hats and cymbals shimmer
    /// rather than fizz: its state, and its coefficient.
    air: f32,
    air_step: f32,

    /// Damping from a Choke: 1, until it falls.
    choke: f32,
    choke_step: f32,
    /// What a retriggered voice was still playing, let go of smoothly.
    declick: f32,
    declick_step: f32,
    last_out: f32,

    prev_trig: bool,
    prev_choke: bool,
    level: SmoothedValue,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Drum {
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let mut drum = Self {
            rng: Pcg32::next_stream(),
            sample_rate,
            voicing: Voicing::new(DrumType::Kick, Strike::default()),
            playing: false,
            elapsed: 0,
            struck: false,
            accent: 1.0,
            choked_at: None,
            body: 0.0,
            noise: 0.0,
            click: 0.0,
            body_step: 0.0,
            noise_step: 0.0,
            click_step: 0.0,
            sweep: 0.0,
            sweep_step: 0.0,
            swell: 0.0,
            swell_step: 0.0,
            rattle: 1.0,
            rattle_target: 1.0,
            bead_in: 0,
            next_burst: CLAP_BURSTS.len(),
            burst_samples: [0; 4],
            phase: [0.0; 2],
            squares: SquareBank::default(),
            filter: [Svf::default(); 2],
            tuning: [SvfTuning::default(); 2],
            air: 0.0,
            air_step: 1.0,
            choke: 1.0,
            choke_step: 0.0,
            declick: 0.0,
            declick_step: 0.0,
            last_out: 0.0,
            prev_trig: false,
            prev_choke: false,
            level: SmoothedValue::with_default_smoothing(0.8, sample_rate),
            ports: vec![
                PortDefinition::input_with_default("trig", "Trig", SignalType::Gate, 0.0).describe("A rising edge strikes the drum; patch a sequencer's Gate"),
                PortDefinition::input_with_default("accent", "Accent", SignalType::Control, 1.0).describe("How hard each hit is, 0 to 1, read as it lands: softer is quieter and darker. Unpatched, every hit is full"),
                PortDefinition::input_with_default("choke", "Choke", SignalType::Gate, 0.0).describe("A rising edge damps the drum within 5 ms; patch the closed hat's trigger into the open hat's Choke"),
                PortDefinition::input_with_default("tune_cv", "Tune", SignalType::Control, 0.0).describe("V/Oct added to the Tune knob at each hit; a sequencer's Pitch plays tuned toms"),
                PortDefinition::input_with_default("decay_cv", "Decay", SignalType::Control, 0.0).describe("CV added to the Decay knob at each hit"),
                PortDefinition::output("out", "Out", SignalType::Audio).describe("The drum"),
            ],
            parameters: vec![
                ParameterDefinition::choice("type", "Type", DRUM_TYPES, 0).describe("Which drum the voice plays; the knobs keep their settings"),
                ParameterDefinition::new("tune", "Tune", -24.0, 24.0, 0.0, ParameterDisplay::linear("st"))
                    .describe("Pitch in semitones from the drum's own: the body of a kick, snare or tom, the band of a clap, the metal of hats and bells"),
                ParameterDefinition::new("decay", "Decay", 0.0, 1.0, 0.5, ParameterDisplay::linear("%"))
                    .describe("How long the drum rings, from tight to long, over each drum's own range"),
                ParameterDefinition::new("tone", "Tone", 0.0, 1.0, 0.5, ParameterDisplay::linear("%"))
                    .describe("Dark to bright: the drive of a kick or tom, the wires of a snare, the filters of claps, hats and bells"),
                ParameterDefinition::new("snap", "Snap", 0.0, 1.0, 0.5, ParameterDisplay::linear("%"))
                    .describe("The noise or click in the hit: a kick's beater and pitch drop, a snare's wires, a clap's bursts, the hiss in the metal"),
                ParameterDefinition::new("level", "Level", 0.0, 1.0, 0.8, ParameterDisplay::linear(""))
                    .describe("Output level"),
            ],
        };
        drum.prepare(sample_rate, 0);
        drum
    }

    const PORT_TRIG: usize = 0;
    const PORT_ACCENT: usize = 1;
    const PORT_CHOKE: usize = 2;
    const PORT_TUNE_CV: usize = 3;
    const PORT_DECAY_CV: usize = 4;

    const PARAM_TYPE: usize = 0;
    const PARAM_TUNE: usize = 1;
    const PARAM_DECAY: usize = 2;
    const PARAM_TONE: usize = 3;
    const PARAM_SNAP: usize = 4;
    const PARAM_LEVEL: usize = 5;

    /// Readout slots: seconds since the last hit (−1 before the first), its
    /// Accent, seconds from the hit to its choke (−1 if it rang out), and
    /// the envelope now.
    pub const READOUT_SINCE: usize = 0;
    pub const READOUT_ACCENT: usize = 1;
    pub const READOUT_CHOKED: usize = 2;
    pub const READOUT_LEVEL: usize = 3;

    const GATE_THRESHOLD: f32 = 0.5;

    /// The type and strike the knobs set, before any CV.
    pub fn knobs(params: &[f32]) -> (DrumType, Strike) {
        let get = |index: usize, default: f32| params.get(index).copied().unwrap_or(default);
        let kind = DrumType::from_param(get(Self::PARAM_TYPE, 0.0));
        let strike = Strike {
            tune: get(Self::PARAM_TUNE, 0.0),
            decay: get(Self::PARAM_DECAY, 0.5),
            tone: get(Self::PARAM_TONE, 0.5),
            snap: get(Self::PARAM_SNAP, 0.5),
            accent: 1.0,
        };
        (kind, strike)
    }

    /// Strikes the drum with `voicing`.
    fn strike(&mut self, voicing: Voicing) {
        let rate = self.sample_rate;
        // Let go of whatever was still ringing rather than cutting it off
        self.declick = self.last_out;

        self.voicing = voicing;
        self.playing = true;
        self.elapsed = 0;
        self.struck = true;
        self.choked_at = None;
        self.choke = 1.0;

        self.body = voicing.body.gain;
        self.noise = voicing.noise.gain;
        self.click = voicing.click.gain;
        self.body_step = voicing.body.coefficient(rate);
        self.noise_step = voicing.noise.coefficient(rate);
        self.click_step = voicing.click.coefficient(rate);
        self.sweep = voicing.sweep_octaves;
        self.sweep_step = (-1.0 / (voicing.sweep_tau * rate)).exp();
        if voicing.swell > 0.0 {
            self.swell = 1.0;
            self.swell_step = (-1.0 / (voicing.swell * rate)).exp();
        } else {
            self.swell = 0.0;
        }
        self.rattle = 1.0;
        self.rattle_target = 1.0;
        self.bead_in = 0;
        self.phase = [0.0; 2];

        let tone = voicing.tone;
        let pitch = voicing.pitch_hz;
        let filters = match voicing.kind {
            // The beater: a short burst of noise in a bright band
            DrumType::Kick => [SvfTuning::new(2500.0 + 4000.0 * tone, 0.8, rate), SvfTuning::default()],
            // The wires: noise under a lowpass Tone opens, over a highpass
            DrumType::Snare => [SvfTuning::new(2500.0 * (2.6 * tone).exp2(), 0.7, rate), SvfTuning::new(900.0, 0.7, rate)],
            // The stick on the head, dark or bright
            DrumType::Tom => [SvfTuning::new(1200.0 * (2.5 * tone).exp2(), 0.7, rate), SvfTuning::default()],
            // A band of noise, narrower when dark
            DrumType::Clap => [SvfTuning::new(pitch * (1.4 * (tone - 0.5)).exp2(), 1.0 + 1.2 * (1.0 - tone), rate), SvfTuning::default()],
            // A band well above the metal's fundamentals, where the 808's
            // hats live, and hiss above it
            DrumType::ClosedHat | DrumType::OpenHat => {
                let corner = 6000.0 * (pitch / METAL_HZ) * (1.6 * (tone - 0.5)).exp2();
                [SvfTuning::new(corner, 1.2, rate), SvfTuning::new(corner * 1.5, 0.6, rate)]
            }
            // A low band and a high band, Tone moving between them
            DrumType::Cymbal => [SvfTuning::new(4200.0 * (pitch / METAL_HZ), 0.9, rate), SvfTuning::new(7500.0, 0.7, rate)],
            DrumType::Rim => [SvfTuning::new(300.0, 0.7, rate), SvfTuning::new(5000.0, 1.0, rate)],
            DrumType::Cowbell => [SvfTuning::new(pitch * 1.7 * (2.0 * (tone - 0.5)).exp2(), 1.6, rate), SvfTuning::default()],
            // A band of hiss, its low side steepened by a highpass
            DrumType::Shaker => {
                let centre = pitch.min(0.4 * rate);
                [SvfTuning::new(centre, 1.3, rate), SvfTuning::new(0.55 * centre, 0.7, rate)]
            }
        };
        self.tuning = filters;
        let air_hz = match voicing.kind {
            DrumType::ClosedHat | DrumType::OpenHat => 11_000.0 * (1.2 * (tone - 0.5)).exp2(),
            DrumType::Cymbal => 9000.0 * (tone - 0.5).exp2(),
            _ => 0.0,
        };
        self.air_step = if air_hz > 0.0 { 1.0 - (-TAU * air_hz.min(0.45 * rate) / rate).exp() } else { 1.0 };

        self.squares.tune(pitch, voicing.kind.partials(), rate);

        self.next_burst = if voicing.kind == DrumType::Clap { 1 } else { CLAP_BURSTS.len() };
        for (samples, &seconds) in self.burst_samples.iter_mut().zip(&CLAP_BURSTS) {
            *samples = (seconds * rate).round() as u64;
        }
        if voicing.kind == DrumType::Clap {
            // The tail waits for the last burst
            self.noise = 0.0;
        }
    }

    /// The next sample of the hit, before Level, choke and declick.
    #[inline]
    fn render(&mut self) -> f32 {
        let v = self.voicing;
        let inv_rate = 1.0 / self.sample_rate;
        let white = self.rng.bipolar();
        let [first, second] = &mut self.filter;
        let [first_tuning, second_tuning] = &self.tuning;

        let out = match v.kind {
            DrumType::Kick | DrumType::Tom => {
                let freq = v.pitch_hz * (self.sweep * LN_2).exp();
                self.phase[0] = (self.phase[0] + freq * inv_rate).fract();
                let body = saturate((TAU * self.phase[0]).sin(), v.drive) * self.body;
                let beater = if v.kind == DrumType::Kick {
                    first.tick(white, first_tuning).band
                } else {
                    first.tick(white, first_tuning).low
                };
                body + beater * self.click
            }
            DrumType::Snare => {
                let freq = v.pitch_hz * (self.sweep * LN_2).exp();
                self.phase[0] = (self.phase[0] + freq * inv_rate).fract();
                self.phase[1] = (self.phase[1] + freq * SNARE_MODE_RATIO * inv_rate).fract();
                let body = 0.65 * (TAU * self.phase[0]).sin() + 0.35 * (TAU * self.phase[1]).sin();
                let wires = second.tick(first.tick(white, first_tuning).low, second_tuning).high;
                body * self.body + wires * self.noise
            }
            DrumType::Clap => {
                if self.next_burst < CLAP_BURSTS.len() && self.elapsed >= self.burst_samples[self.next_burst] {
                    self.click = v.click.gain;
                    if self.next_burst == CLAP_BURSTS.len() - 1 {
                        self.noise = v.noise.gain;
                    }
                    self.next_burst += 1;
                }
                first.tick(white, first_tuning).band * (self.click + self.noise)
            }
            DrumType::ClosedHat | DrumType::OpenHat => {
                let metal = first.tick(self.squares.next(), first_tuning).band;
                let hiss = second.tick(white, second_tuning).band;
                self.air += self.air_step * (metal * (1.0 - 0.6 * v.snap) + hiss * (0.2 + 0.8 * v.snap) - self.air);
                self.air * self.body
            }
            DrumType::Cymbal => {
                let bands = first.tick(self.squares.next(), first_tuning);
                let metal = bands.band * (1.0 - v.tone) + bands.high * (0.4 + v.tone);
                let sizzle = second.tick(white, second_tuning).high;
                self.air += self.air_step * (metal * (1.0 - 0.5 * v.snap) + sizzle * (0.1 + 0.4 * v.snap) - self.air);
                self.air * (self.body + self.click)
            }
            DrumType::Rim => {
                let freq = v.pitch_hz * (self.sweep * LN_2).exp();
                self.phase[0] = (self.phase[0] + freq * inv_rate).fract();
                self.phase[1] = (self.phase[1] + freq * RIM_MODE_RATIO * inv_rate).fract();
                let ring = 0.6 * (TAU * self.phase[0]).sin() + 0.5 * (TAU * self.phase[1]).sin();
                let ring = first.tick(saturate(ring, v.drive), first_tuning).high;
                let crack = second.tick(white, second_tuning).band;
                ring * self.body + crack * self.click
            }
            DrumType::Cowbell => first.tick(self.squares.next(), first_tuning).band * (self.body + self.click),
            DrumType::Shaker => {
                // Each bead that lands moves the grain's level a little,
                // more of it with Snap: the rattle inside the swell
                if self.bead_in == 0 {
                    let (soonest, latest) = BEAD_GAP;
                    let gap = soonest + (latest - soonest) * (0.5 + 0.5 * self.rng.bipolar());
                    self.bead_in = (gap * self.sample_rate).max(1.0) as u32;
                    self.rattle_target = 1.0 - 0.85 * v.snap * (0.5 + 0.5 * self.rng.bipolar());
                }
                self.bead_in -= 1;
                self.rattle += 0.08 * (self.rattle_target - self.rattle);
                let hiss = second.tick(first.tick(white, first_tuning).band, second_tuning).high;
                hiss * self.rattle * self.body
            }
        };

        self.body *= self.body_step;
        self.noise *= self.noise_step;
        self.click *= self.click_step;
        self.sweep *= self.sweep_step;
        let rise = 1.0 - self.swell;
        self.swell *= self.swell_step;
        out * rise
    }

    /// The envelope now, before the gain.
    fn envelope(&self) -> f32 {
        (self.body + self.noise + self.click) * (1.0 - self.swell) * self.choke
    }
}

impl Default for Drum {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Drum {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "source.drum",
            name: "Drum",
            category: ModuleCategory::Source,
            description: "An analog-style drum voice: kick, snare, tom, clap, hats, cymbal, rim, cowbell or shaker",
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
        self.choke_step = (-1.0 / (CHOKE_TAU * sample_rate)).exp();
        self.declick_step = (-1.0 / (DECLICK_TAU * sample_rate)).exp();
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [out, ..] = outputs else {
            return;
        };
        let trig = connected_input(inputs, Self::PORT_TRIG);
        let accent = connected_input(inputs, Self::PORT_ACCENT);
        let choke = connected_input(inputs, Self::PORT_CHOKE);
        let tune_cv = connected_input(inputs, Self::PORT_TUNE_CV);
        let decay_cv = connected_input(inputs, Self::PORT_DECAY_CV);
        let at = |buffer: Option<&SignalBuffer>, i: usize, default: f32| {
            buffer.map_or(default, |buf| buf.samples.get(i).copied().unwrap_or(default))
        };
        let (kind, knobs) = Self::knobs(params);
        self.level.set_target(params[Self::PARAM_LEVEL].clamp(0.0, 1.0));

        for i in 0..context.block_size {
            let trig_high = at(trig, i, 0.0) > Self::GATE_THRESHOLD;
            if trig_high && !self.prev_trig {
                let hit = at(accent, i, 1.0).clamp(0.0, 1.0);
                let strike = Strike {
                    tune: knobs.tune + 12.0 * at(tune_cv, i, 0.0),
                    decay: (knobs.decay + at(decay_cv, i, 0.0)).clamp(0.0, 1.0),
                    accent: hit,
                    ..knobs
                };
                self.accent = hit;
                self.strike(Voicing::new(kind, strike));
            }
            self.prev_trig = trig_high;

            let choke_high = at(choke, i, 0.0) > Self::GATE_THRESHOLD;
            if choke_high && !self.prev_choke && self.playing && self.choked_at.is_none() {
                self.choked_at = Some(self.elapsed);
            }
            self.prev_choke = choke_high;

            let level = self.level.next();
            let mut sample = 0.0;
            if self.playing {
                if self.choked_at.is_some() {
                    self.choke *= self.choke_step;
                }
                let hit = soft_ceiling(self.render() * self.voicing.gain);
                sample = hit * self.choke * level;
                self.elapsed += 1;
                let clap_waiting = self.next_burst < CLAP_BURSTS.len();
                if !clap_waiting && self.envelope() < SILENCE {
                    self.playing = false;
                }
            } else {
                self.elapsed = self.elapsed.saturating_add(1);
            }
            sample += self.declick;
            self.declick *= self.declick_step;
            if self.declick.abs() < SILENCE {
                self.declick = 0.0;
            }
            self.last_out = sample;
            out.samples[i] = sample;
        }
    }

    fn reset(&mut self) {
        self.playing = false;
        self.struck = false;
        self.choked_at = None;
        self.body = 0.0;
        self.noise = 0.0;
        self.click = 0.0;
        self.sweep = 0.0;
        self.swell = 0.0;
        self.rattle = 1.0;
        self.rattle_target = 1.0;
        self.bead_in = 0;
        self.next_burst = CLAP_BURSTS.len();
        self.phase = [0.0; 2];
        self.filter = [Svf::default(); 2];
        self.air = 0.0;
        self.squares = SquareBank::default();
        self.choke = 1.0;
        self.declick = 0.0;
        self.last_out = 0.0;
        self.prev_trig = false;
        self.prev_choke = false;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let seconds = |samples: u64| samples as f32 / self.sample_rate;
        let mut readout = Readout::default();
        readout.values[Self::READOUT_SINCE] = if self.struck { seconds(self.elapsed) } else { -1.0 };
        readout.values[Self::READOUT_ACCENT] = self.accent;
        readout.values[Self::READOUT_CHOKED] = self.choked_at.map_or(-1.0, seconds);
        readout.values[Self::READOUT_LEVEL] = if self.playing { self.envelope() } else { 0.0 };
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{amp_to_db, peak, rms};
    use crate::dsp::Poly;

    const SAMPLE_RATE: f32 = 48000.0;
    const BLOCK: usize = 256;

    /// Default knobs with the type and level given.
    fn params(kind: DrumType) -> Vec<f32> {
        vec![kind as usize as f32, 0.0, 0.5, 0.5, 0.5, 1.0]
    }

    /// Plays `seconds` of a Drum: `trig_at` and `choke_at` are the samples
    /// their inputs go high (for a millisecond), `accent` the Accent.
    fn play(drum: &mut Drum, params: &[f32], seconds: f32, trig_at: &[usize], choke_at: &[usize], accent: Option<f32>) -> Vec<f32> {
        drum.prepare(SAMPLE_RATE, BLOCK);
        let total = (seconds * SAMPLE_RATE) as usize;
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let pulse = |starts: &[usize], at: usize| starts.iter().any(|&s| at >= s && at < s + 48);
        let mut trig = SignalBuffer::gate(BLOCK);
        let mut choke = SignalBuffer::gate(BLOCK);
        let mut accent_in = SignalBuffer::control(BLOCK);
        accent_in.fill(accent.unwrap_or(1.0));
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let accent_ref = if accent.is_some() { &accent_in } else { &unpatched };
        let mut outputs = vec![SignalBuffer::audio(BLOCK)];
        let mut rendered = Vec::with_capacity(total + BLOCK);
        while rendered.len() < total {
            let start = rendered.len();
            for i in 0..BLOCK {
                trig.samples[i] = if pulse(trig_at, start + i) { 1.0 } else { 0.0 };
                choke.samples[i] = if pulse(choke_at, start + i) { 1.0 } else { 0.0 };
            }
            drum.process(&[&trig, accent_ref, &choke, &unpatched, &unpatched], &mut outputs, params, &ctx);
            rendered.extend_from_slice(&outputs[0].samples);
        }
        rendered.truncate(total);
        rendered
    }

    /// One hit of `kind` at default settings, two seconds long.
    fn hit(kind: DrumType) -> Vec<f32> {
        play(&mut Drum::new(), &params(kind), 2.0, &[0], &[], None)
    }

    #[test]
    #[ignore = "prints each type's raw peak, for tuning Voicing::level_of"]
    fn print_raw_peaks() {
        for kind in DrumType::ALL {
            let mut worst: f32 = 0.0;
            for _ in 0..8 {
                let mut drum = Drum::new();
                drum.prepare(SAMPLE_RATE, BLOCK);
                drum.strike(Voicing::new(kind, Strike::default()));
                for _ in 0..(SAMPLE_RATE as usize) {
                    worst = worst.max(drum.render().abs());
                    drum.elapsed += 1;
                }
            }
            println!("{:<11} raw peak {:.3}  level_of for 0.75: {:.2}", kind.name(), worst, 0.75 / worst);
        }
    }

    #[test]
    #[ignore = "prints each type's level, for tuning Voicing::level_of"]
    fn print_levels() {
        for kind in DrumType::ALL {
            let out = hit(kind);
            let first = &out[..(0.1 * SAMPLE_RATE) as usize];
            println!("{:<11} peak {:>6.1} dB   rms(100ms) {:>6.1} dB", kind.name(), amp_to_db(peak(&out)), amp_to_db(rms(first)));
        }
    }

    /// RMS of the slope over RMS of the signal: higher is brighter. Twice
    /// the frequency of a sine doubles it.
    fn brightness(samples: &[f32]) -> f32 {
        let slope: Vec<f32> = samples.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&slope) / rms(samples)
    }

    /// The frequency of a signal from the first and last of its rising
    /// zero crossings, each placed between samples.
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

    fn ms(milliseconds: f32) -> usize {
        (milliseconds * SAMPLE_RATE / 1000.0) as usize
    }

    #[test]
    fn test_drum_info_ports_and_parameters() {
        let drum = Drum::new();
        assert_eq!(drum.info().id, "source.drum");
        assert_eq!(drum.info().category, ModuleCategory::Source);
        let ids: Vec<_> = drum.ports().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["trig", "accent", "choke", "tune_cv", "decay_cv", "out"]);
        let names: Vec<_> = drum.parameters().iter().map(|p| p.name).collect();
        assert_eq!(names, ["Type", "Tune", "Decay", "Tone", "Snap", "Level"]);
        assert_eq!(drum.parameters()[0].display, ParameterDisplay::discrete(DRUM_TYPES));
        assert_eq!(DRUM_TYPES.len(), DrumType::ALL.len());
        for (index, kind) in DrumType::ALL.iter().enumerate() {
            assert_eq!(DrumType::from_param(index as f32), *kind);
            assert_eq!(kind.name(), DRUM_TYPES[index]);
        }
    }

    #[test]
    fn test_every_type_plays_finite_unclipped_audio_and_stops() {
        for kind in DrumType::ALL {
            // Several hits: the noise and the free-running metal differ each time
            let out = play(&mut Drum::new(), &params(kind), 6.0, &[0, 96_000, 192_000], &[], None);
            assert!(out.iter().all(|s| s.is_finite()), "{}: not finite", kind.name());
            let loudest = amp_to_db(peak(&out));
            assert!(loudest < -0.5, "{}: peak {loudest:.1} dB", kind.name());
            assert!(loudest > -16.0, "{}: barely audible at {loudest:.1} dB", kind.name());

            // Rings out within its length, then is silent
            let length = Voicing::new(kind, Strike::default()).length();
            let end = ((length + 0.05) * SAMPLE_RATE) as usize;
            assert!(end < 96_000, "{}: rings {length} s", kind.name());
            let tail = peak(&out[end..96_000]);
            assert!(tail < 0.01, "{}: still at {tail} after {length} s", kind.name());
        }
    }

    #[test]
    fn test_every_type_is_silent_until_struck() {
        for kind in DrumType::ALL {
            let out = play(&mut Drum::new(), &params(kind), 0.1, &[], &[], None);
            assert!(out.iter().all(|&s| s == 0.0), "{}", kind.name());
        }
    }

    #[test]
    fn test_softer_hits_are_quieter_and_darker() {
        // Both hits draw the same noise, so only the accent tells them apart
        let drum = || Drum { rng: Pcg32::new(1), ..Drum::new() };
        for kind in DrumType::ALL {
            let full = play(&mut drum(), &params(kind), 0.15, &[0], &[], Some(1.0));
            let soft = play(&mut drum(), &params(kind), 0.15, &[0], &[], Some(0.4));
            let drop = amp_to_db(rms(&soft)) - amp_to_db(rms(&full));
            assert!((-11.0..-5.0).contains(&drop), "{}: accent 0.4 is {drop:.1} dB", kind.name());
            assert!(brightness(&soft) < brightness(&full), "{}: a soft hit isn't darker", kind.name());
        }
    }

    #[test]
    fn test_accent_zero_is_a_rest() {
        let out = play(&mut Drum::new(), &params(DrumType::Snare), 0.2, &[0], &[], Some(0.0));
        assert!(peak(&out) < 1e-6);
    }

    #[test]
    fn test_choke_damps_an_open_hat_within_5_ms() {
        let choke = ms(100.0);
        let mut params = params(DrumType::OpenHat);
        params[Drum::PARAM_DECAY] = 1.0;
        let out = play(&mut Drum::new(), &params, 0.3, &[0], &[choke], None);
        let before = rms(&out[choke - ms(5.0)..choke]);
        let after = peak(&out[choke + ms(5.0)..]);
        assert!(before > 0.02, "the hat had died down already: {before}");
        let damped = amp_to_db(after) - amp_to_db(before);
        assert!(damped < -40.0, "5 ms after the choke it is only {damped:.1} dB down");

        // Unchoked, it would still ring
        let free = play(&mut Drum::new(), &params, 0.3, &[0], &[], None);
        assert!(rms(&free[choke + ms(5.0)..choke + ms(25.0)]) > 0.5 * before);
    }

    #[test]
    fn test_choke_waits_for_a_hit() {
        // A choke before the hit doesn't silence the hit that follows
        let out = play(&mut Drum::new(), &params(DrumType::OpenHat), 0.2, &[4800], &[0], None);
        assert!(peak(&out[4800..]) > 0.1);
    }

    #[test]
    fn test_a_ringing_drum_retriggers_without_a_click() {
        // A long kick, hit again mid-swing: the steps between samples stay
        // within what the kick's own attack makes
        let mut params = params(DrumType::Kick);
        params[Drum::PARAM_DECAY] = 1.0;
        params[Drum::PARAM_SNAP] = 0.0;
        let again = 7_000;
        let out = play(&mut Drum::new(), &params, 0.4, &[0, again], &[], None);
        let jump = |range: std::ops::Range<usize>| out[range].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        let attack = jump(0..400);
        let restart = jump(again - 2..again + 400);
        assert!(restart < 1.3 * attack, "restart steps {restart}, attack {attack}");
        assert!(out[again - 1].abs() > 0.2, "the kick should still be ringing loud when hit again");
    }

    #[test]
    fn test_tune_knob_and_cv_move_the_kick() {
        let late = |out: &[f32]| frequency(&out[ms(150.0)..ms(350.0)]);
        let mut params = params(DrumType::Kick);
        params[Drum::PARAM_DECAY] = 1.0;
        let base = late(&play(&mut Drum::new(), &params, 0.4, &[0], &[], None));
        assert!((base - 48.0).abs() < 3.0, "kick rests at {base} Hz");

        params[Drum::PARAM_TUNE] = 12.0;
        let octave = late(&play(&mut Drum::new(), &params, 0.4, &[0], &[], None));
        assert!((octave / base - 2.0).abs() < 0.01, "+12 st gave {octave} Hz");

        // One volt of Tune CV is an octave too
        params[Drum::PARAM_TUNE] = 0.0;
        let mut drum = Drum::new();
        drum.prepare(SAMPLE_RATE, BLOCK);
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut trig = SignalBuffer::gate(BLOCK);
        trig.samples[..48].fill(1.0);
        let mut tune = SignalBuffer::control(BLOCK);
        tune.fill(1.0);
        let idle = SignalBuffer::gate(BLOCK);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::audio(BLOCK)];
        let mut out = Vec::new();
        for block in 0..80 {
            let trig = if block == 0 { &trig } else { &idle };
            drum.process(&[trig, &unpatched, &idle, &tune, &unpatched], &mut outputs, &params, &ctx);
            out.extend_from_slice(&outputs[0].samples);
        }
        let cv = late(&out);
        assert!((cv / base - 2.0).abs() < 0.01, "1 V of Tune CV gave {cv} Hz");
    }

    #[test]
    fn test_decay_lengthens_every_type() {
        for kind in DrumType::ALL {
            let short = Voicing::new(kind, Strike { decay: 0.0, ..Strike::default() });
            let long = Voicing::new(kind, Strike { decay: 1.0, ..Strike::default() });
            assert!(long.length() > 2.0 * short.length(), "{}: {} to {} s", kind.name(), short.length(), long.length());
        }
    }

    #[test]
    fn test_the_voicing_is_the_envelope_the_voice_plays() {
        // What the display draws is what the voice does, sample by sample
        for kind in DrumType::ALL {
            let mut drum = Drum::new();
            drum.prepare(SAMPLE_RATE, BLOCK);
            let voicing = Voicing::new(kind, Strike::default());
            drum.strike(voicing);
            for n in 0..ms(500.0) {
                // Clear of the clap's burst boundaries, where the envelope
                // jumps between one sample and the next
                if n % 240 == 7 {
                    let expected = voicing.amplitude_at(n as f32 / SAMPLE_RATE);
                    let actual = drum.envelope();
                    assert!(
                        (actual - expected).abs() <= 2e-3 * expected.max(1e-2),
                        "{} at {n}: plays {actual}, draws {expected}",
                        kind.name()
                    );
                }
                drum.render();
                drum.elapsed += 1;
            }
        }
    }

    #[test]
    fn test_the_pitch_trace_falls_to_the_resting_pitch() {
        let kick = Voicing::new(DrumType::Kick, Strike::default());
        let start = kick.pitch_at(0.0).unwrap();
        assert!((start / kick.pitch_hz - kick.sweep_octaves.exp2()).abs() < 1e-3);
        assert!((kick.pitch_at(1.0).unwrap() - kick.pitch_hz).abs() < 0.01);
        assert!(Voicing::new(DrumType::ClosedHat, Strike::default()).pitch_at(0.0).is_none());
    }

    #[test]
    fn test_clap_bursts_then_tails() {
        let out = play(&mut Drum::new(), &params(DrumType::Clap), 0.2, &[0], &[], None);
        // Each burst lands as a fresh peak after the one before has fallen
        for pair in CLAP_BURSTS.windows(2) {
            let (start, next) = (ms(pair[0] * 1000.0), ms(pair[1] * 1000.0));
            let opening = peak(&out[start..start + ms(2.0)]);
            let fallen = peak(&out[next - ms(2.0)..next]);
            assert!(fallen < 0.5 * opening, "burst at {start} only fell to {fallen} from {opening}");
        }
        // And the tail rings on after the last
        assert!(rms(&out[ms(60.0)..ms(80.0)]) > 0.01);
    }

    /// Where a hit's level peaks, in samples, from its RMS over 0.25 ms windows.
    fn loudest_at(out: &[f32]) -> usize {
        let window = ms(0.25);
        (0..out.len() / window).map(|w| (w * window, rms(&out[w * window..(w + 1) * window]))).fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a }).0
    }

    #[test]
    fn test_a_shaker_grain_swells_in() {
        // A hat is loudest at once; a shaker's beads take a few ms to arrive
        let drum = || Drum { rng: Pcg32::new(3), ..Drum::new() };
        let hat = play(&mut drum(), &params(DrumType::ClosedHat), 0.1, &[0], &[], None);
        let shaker = play(&mut drum(), &params(DrumType::Shaker), 0.1, &[0], &[], None);
        assert!(loudest_at(&hat) < ms(1.0), "the hat peaks at {} samples", loudest_at(&hat));
        let peak_at = loudest_at(&shaker);
        assert!((ms(1.5)..ms(8.0)).contains(&peak_at), "the shaker peaks {} ms in", peak_at as f32 * 1000.0 / SAMPLE_RATE);
        let opening = rms(&shaker[..ms(0.5)]);
        assert!(opening < 0.5 * rms(&shaker[peak_at..peak_at + ms(1.0)]), "the shaker starts at {opening}");
    }

    #[test]
    fn test_a_softer_shake_swells_in_more_slowly() {
        let full = Voicing::new(DrumType::Shaker, Strike { accent: 1.0, ..Strike::default() });
        let soft = Voicing::new(DrumType::Shaker, Strike { accent: 0.3, ..Strike::default() });
        assert!((3.0 * full.swell - 0.003).abs() < 1e-6);
        assert!(soft.swell > 1.5 * full.swell);
        // So a soft grain is further from its peak 2 ms in
        let rise = |v: &Voicing| v.amplitude_at(0.002) / v.amplitude_at(0.01);
        assert!(rise(&soft) < rise(&full));
    }

    #[test]
    fn test_shaker_snap_rattles_the_grain() {
        // The grain's level, in 1 ms steps, wavers more with Snap than without
        let roughness = |snap: f32| {
            let mut params = params(DrumType::Shaker);
            params[Drum::PARAM_SNAP] = snap;
            params[Drum::PARAM_DECAY] = 1.0;
            let out = play(&mut Drum { rng: Pcg32::new(5), ..Drum::new() }, &params, 0.1, &[0], &[], None);
            let levels: Vec<f32> = out[ms(10.0)..ms(60.0)].chunks(ms(1.0)).map(rms).collect();
            let steps: Vec<f32> = levels.windows(2).map(|w| (w[1] / w[0]).ln().abs()).collect();
            steps.iter().sum::<f32>() / steps.len() as f32
        };
        let (smooth, rattling) = (roughness(0.0), roughness(1.0));
        assert!(rattling > 1.5 * smooth, "snap 0 wavers {smooth}, snap 1 {rattling}");
    }

    #[test]
    fn test_shaker_tone_moves_the_band_from_5_to_9_khz() {
        let band = |tone: f32| Voicing::new(DrumType::Shaker, Strike { tone, ..Strike::default() }).pitch_hz;
        assert!((band(0.0) - 5000.0).abs() < 10.0, "{}", band(0.0));
        assert!((band(1.0) - 9000.0).abs() < 20.0, "{}", band(1.0));
    }

    #[test]
    fn test_metal_squares_are_band_limited() {
        // A high-tuned hat's squares, without the filters: little energy
        // folds back below the lowest square
        let mut bank = SquareBank::default();
        bank.tune(METAL_HZ * 4.0, &METAL_RATIOS, SAMPLE_RATE);
        let samples: Vec<f32> = (0..65_536).map(|_| bank.next()).collect();
        let spectrum = crate::dsp::analysis::Spectrum::of(&samples, SAMPLE_RATE);
        let below = (METAL_HZ as f64 * 4.0 * 0.8 / spectrum.bin_hz) as usize;
        let folded: f64 = spectrum.magnitudes[2..below].iter().map(|m| m * m).sum();
        let total: f64 = spectrum.magnitudes.iter().map(|m| m * m).sum();
        let db = 10.0 * (folded / total).log10();
        assert!(db < -45.0, "aliasing at {db:.1} dB");
    }

    #[test]
    fn test_level_zero_is_silent_from_the_first_block() {
        let mut params = params(DrumType::Kick);
        params[Drum::PARAM_LEVEL] = 0.0;
        let out = play(&mut Drum::new(), &params, 0.05, &[0], &[], None);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_knobs_turned_while_ringing_wait_for_the_next_hit() {
        let mut drum = Drum::new();
        let mut short = params(DrumType::OpenHat);
        short[Drum::PARAM_DECAY] = 0.0;
        let first = play(&mut drum, &short, 0.1, &[0], &[], None);
        let mut long = short.clone();
        long[Drum::PARAM_DECAY] = 1.0;
        let rest = play(&mut drum, &long, 0.3, &[], &[], None);
        assert!(peak(&first) > 0.05);
        assert!(peak(&rest[4800..]) < 0.001, "the short hat grew long when the knob turned");
    }

    #[test]
    fn test_readout_tracks_the_hit_and_its_choke() {
        let mut drum = Drum::new();
        assert_eq!(drum.readout(&[]).unwrap().values[Drum::READOUT_SINCE], -1.0);

        play(&mut drum, &params(DrumType::OpenHat), 0.1, &[0], &[2400], Some(0.7));
        let readout = drum.readout(&[]).unwrap();
        let since = readout.values[Drum::READOUT_SINCE];
        assert!((since - 0.1).abs() < 0.006, "since {since}");
        assert!((readout.values[Drum::READOUT_ACCENT] - 0.7).abs() < 1e-6);
        assert!((readout.values[Drum::READOUT_CHOKED] - 0.05).abs() < 0.001);
        assert_eq!(readout.values[Drum::READOUT_LEVEL], 0.0);
    }

    #[test]
    fn test_poly_voices_play_round_robin() {
        // A two-channel gate, one hit on each: two kicks, the second not
        // cutting off the first
        let mut poly = Poly::<Drum>::default();
        poly.prepare(SAMPLE_RATE, BLOCK);
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut trig = SignalBuffer::polyphonic(BLOCK, SignalType::Gate);
        trig.set_channels(2);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let idle = SignalBuffer::unconnected(BLOCK, SignalType::Gate);
        let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let params = params(DrumType::Kick);
        let mut voices = [Vec::new(), Vec::new()];
        for block in 0..40 {
            trig.channel_mut(0).fill(if block == 0 { 1.0 } else { 0.0 });
            trig.channel_mut(1).fill(if block == 10 { 1.0 } else { 0.0 });
            poly.process(&[&trig, &unpatched, &idle, &unpatched, &unpatched], &mut outputs, &params, &ctx);
            assert_eq!(outputs[0].channels(), 2);
            voices[0].extend_from_slice(&outputs[0].voice(0).samples);
            voices[1].extend_from_slice(&outputs[0].voice(1).samples);
        }
        let second_hit = 10 * BLOCK;
        assert_eq!(peak(&voices[1][..second_hit]), 0.0);
        assert!(peak(&voices[1][second_hit..]) > 0.3);
        // The first kick rings on through the second
        assert!(rms(&voices[0][second_hit..second_hit + 2400]) > 0.05);
    }

    #[test]
    fn test_drum_registry_instantiation() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create("source.drum").expect("Drum is registered");
        assert_eq!(module.info().name, "Drum");
        assert!(module.polyphonic());
    }
}
