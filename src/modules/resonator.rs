//! Resonator module: a physical model, played by a gate or by any sound.
//!
//! Two models share one set of knobs:
//!
//! - **String**: Karplus-Strong. A burst of noise, or the sound patched into
//!   In, runs round a delay line one period long. A gentle lowpass in the
//!   loop takes a little of the top off every pass, so the string darkens as
//!   it rings, as a real one does.
//! - **Body**: twelve tuned modes, each a two-pole resonator, at the ratios
//!   of a bell, a bar, a glass or a block of wood. A mallet (or In) strikes
//!   them all at once, and each rings away at its own rate.
//!
//! Both are tuned exactly. The string's loop filter is a one-pole lowpass,
//! and the delay line is read between samples, shorter by just the delay
//! the lowpass adds at the note, so damping never detunes it. The modes are
//! tuned from closed form.
//!
//! Out is only the ringing. A String is heard from a pickup on the string,
//! and a Body is its modes, so a drum patched into In is heard only as what
//! it sets ringing (mix the drum in too, to hear both).
//!
//! Nothing here can run away: the loop's gain is below 1 at every
//! frequency, and a soft ceiling sits inside the loop, the way a real string
//! can only swing so far, so even In held at full scale into the longest
//! Decay stays inside ±1.

use std::f32::consts::{PI, TAU};

use super::noise::{soft_ceiling, Pcg32};
use crate::dsp::{
    connected_input,
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::FracDelay,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// The patch file's name for the module.
pub const RESONATOR_ID: &str = "source.resonator";

/// The models, in the Mode menu's order. Saved patches store the index.
pub const MODES: &[&str] = &["String", "Body"];

/// The materials, in the Material menu's order. Saved patches store the
/// index, so new materials go at the end.
pub const MATERIALS: &[&str] = &["Bell", "Bar", "Glass", "Wood"];

/// How many modes a Body rings with.
pub const MODE_COUNT: usize = 12;

/// How many points of the string the readout carries.
pub const STRING_POINTS: usize = 24;

/// The pitch at V/Oct 0 and Tune 0: C4, as the Oscillator.
pub const C4_HZ: f32 = 261.625_58;

/// The lowest pitch, which sets how long the delay line is.
const LOWEST_HZ: f32 = 16.0;

/// Seconds for a signal to fall 60 dB, per unit of time constant.
const T60_PER_TAU: f32 = 6.907_755;

/// The model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    String,
    Body,
}

impl Mode {
    pub fn from_param(value: f32) -> Self {
        if value.round() as i32 == 1 {
            Mode::Body
        } else {
            Mode::String
        }
    }
}

/// What a Body is made of: where its modes sit, how long it rings, and how
/// much faster its high modes die than its low ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Bell,
    Bar,
    Glass,
    Wood,
}

impl Material {
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Material::Bar,
            2 => Material::Glass,
            3 => Material::Wood,
            _ => Material::Bell,
        }
    }

    pub fn name(self) -> &'static str {
        MATERIALS[self as usize]
    }

    /// The modes as ratios of the lowest, which is the note played.
    fn ratios(self) -> &'static [f32; MODE_COUNT] {
        match self {
            // A church bell's partials from its hum: the prime an octave up,
            // the minor tierce that makes a bell sound like a bell, the
            // quint, then the nominal two octaves up (Rayleigh, Hibbert)
            Material::Bell => &[1.0, 2.0, 2.379, 3.012, 4.0, 5.028, 5.324, 6.022, 8.332, 10.866, 13.592, 16.430],
            // A free-free bar: a glockenspiel's bars before any tuning
            Material::Bar => &[1.0, 2.756, 5.404, 8.933, 13.345, 18.638, 24.814, 31.870, 39.810, 48.630, 58.330, 68.920],
            // A wine glass's bending modes, which crowd together less than a
            // bar's do
            Material::Glass => &[1.0, 2.32, 4.25, 6.63, 9.38, 12.49, 15.96, 19.79, 23.98, 28.53, 33.43, 38.69],
            // A wooden bar, thick for its length, so its modes sit closer
            Material::Wood => &[1.0, 2.57, 4.47, 6.60, 7.92, 9.71, 11.61, 13.30, 15.20, 17.40, 19.60, 21.90],
        }
    }

    /// How long it rings, as a multiple of the Decay knob's time.
    pub fn ring(self) -> f32 {
        match self {
            Material::Bell => 1.6,
            Material::Bar => 0.8,
            Material::Glass => 2.0,
            Material::Wood => 0.12,
        }
    }

    /// How much faster a mode dies the higher it sits: its decay time goes
    /// as its ratio to this power, before Damping adds to it.
    fn loss(self) -> f32 {
        match self {
            Material::Bell => 0.35,
            Material::Bar => 0.5,
            Material::Glass => 0.15,
            Material::Wood => 0.9,
        }
    }
}

/// The modes a Body rings at, as ratios of the note: the harmonic series at
/// Inharm 0, the material's own at 0.5, and stretched further apart at 1.
pub fn mode_ratios(material: Material, inharm: f32) -> [f32; MODE_COUNT] {
    let inharm = inharm.clamp(0.0, 1.0);
    let own = material.ratios();
    let mut ratios = [1.0; MODE_COUNT];
    for (i, ratio) in ratios.iter_mut().enumerate() {
        let harmonic = (i + 1) as f32;
        let own = own[i];
        *ratio = if inharm <= 0.5 {
            // Between the two, in the log domain, so each mode glides evenly
            harmonic * (own / harmonic).powf(2.0 * inharm)
        } else {
            own.powf(1.0 + (inharm - 0.5) * 0.6)
        };
    }
    ratios
}

/// How loudly each mode rings when struck at `position` (0 at the edge, 1
/// at the middle) with a mallet as bright as `brightness`, before the
/// mallet's own shape.
pub fn mode_amplitudes(ratios: &[f32; MODE_COUNT], position: f32, brightness: f32) -> [f32; MODE_COUNT] {
    let place = strike_place(position);
    let tilt = 1.6 * (1.0 - brightness.clamp(0.0, 1.0));
    let mut amps = [0.0; MODE_COUNT];
    for (i, amp) in amps.iter_mut().enumerate() {
        // Struck at a node of a mode, that mode barely sounds
        let shape = (PI * (i + 1) as f32 * place).sin().abs();
        *amp = (0.12 + 0.88 * shape) * ratios[i].powf(-tilt);
    }
    amps
}

/// Where along the string, or across the body, Position strikes: from
/// near the end (bright, every partial) to the middle (no even ones).
pub fn strike_place(position: f32) -> f32 {
    0.03 + 0.47 * position.clamp(0.0, 1.0)
}

/// How long the note rings, in seconds to fall 60 dB, from Decay.
pub fn decay_seconds(decay: f32) -> f32 {
    0.08 * 300.0_f32.powf(decay.clamp(0.0, 1.0))
}

/// How a String's loop is set for a note: where the line is read, the
/// loop lowpass's pole, and the gain round the loop.
#[derive(Clone, Copy, Debug, Default)]
struct StringTuning {
    read: f32,
    pole: f32,
    gain: f32,
}

impl StringTuning {
    fn new(period: f32, damping: f32, seconds: f32, sample_rate: f32) -> Self {
        // The lowpass's corner, from far above hearing down to a couple of
        // octaves over the note: a bright wire to felt. It rides up with
        // the note, as a high string rings as long and bright as a low one
        // (any lower and the note itself would die in a few periods)
        let note = sample_rate / period;
        let damping = damping.clamp(0.0, 1.0);
        let cutoff = (16_000.0 * 0.006_f32.powf(damping) + note * (3.0 + 13.0 * (1.0 - damping))).min(0.45 * sample_rate);
        let pole = (-TAU * cutoff / sample_rate).exp();
        // What the lowpass does to the note itself: how late, how quiet
        let w = TAU / period;
        let (sin, cos) = w.sin_cos();
        let late = (pole * sin).atan2(1.0 - pole * cos) / w;
        let passes = (1.0 - pole) / (1.0 - 2.0 * pole * cos + pole * pole).sqrt();
        // The line reads one sample back at its shortest (FracDelay's 0 is the newest)
        let read = (period - 1.0 - late).max(1.0);
        // Each pass keeps what lets the note fall 60 dB in `seconds`, short
        // of 1 at DC so nothing below the note can build up
        let keep = (-T60_PER_TAU * period / (seconds * sample_rate)).exp();
        let gain = (keep / passes).min(0.99995);
        Self { read, pole, gain }
    }
}

/// One mode of a Body: a two-pole resonator, scaled so a unit impulse rings
/// at the mode's amplitude whatever its frequency.
#[derive(Clone, Copy, Debug, Default)]
struct ModeFilter {
    a1: f32,
    a2: f32,
    gain: f32,
    /// sin of the mode's step, for reading its amplitude back.
    sin: f32,
    cos: f32,
    y1: f32,
    y2: f32,
}

impl ModeFilter {
    fn tune(&mut self, hz: f32, seconds: f32, amp: f32, sample_rate: f32) {
        let theta = TAU * hz / sample_rate;
        if theta >= 0.95 * PI {
            // Above Nyquist's reach: silent, and let it ring out
            self.gain = 0.0;
            self.a1 = 0.0;
            self.a2 = 0.0;
            return;
        }
        let r = (-T60_PER_TAU / (seconds.max(0.003) * sample_rate)).exp();
        let (sin, cos) = theta.sin_cos();
        self.a1 = 2.0 * r * cos;
        self.a2 = -r * r;
        self.sin = sin;
        self.cos = cos;
        self.gain = amp * sin;
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.a1 * self.y1 + self.a2 * self.y2 + self.gain * x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// The amplitude it rings at now, read from its last two samples.
    fn amplitude(&self) -> f32 {
        if self.sin <= 1e-6 {
            return 0.0;
        }
        let energy = self.y1 * self.y1 + self.y2 * self.y2 - 2.0 * self.y1 * self.y2 * self.cos;
        energy.max(0.0).sqrt() / self.sin
    }
}

/// A physical model of a plucked string or a struck body.
///
/// # Ports
///
/// **Inputs:**
/// - **Gate** (Gate): A rising edge plucks the string or strikes the body.
/// - **V/Oct** (Control): Pitch, 1 per octave, added to Tune.
/// - **Strike** (Control): How hard each gate plays, 0 to 1. Softer is
///   quieter and darker. Unpatched, every one is full.
/// - **In** (Audio): Any sound to excite it with: a drum, noise, a voice.
/// - **Decay**, **Damping**, **Brightness** (Control): Added to their knobs.
///
/// **Outputs:**
/// - **Out** (Audio): The string or the body.
///
/// # Parameters
/// - **Mode**: String or Body.
/// - **Material**: What a Body is made of (Bell, Bar, Glass, Wood).
/// - **Tune** (±24 st): Pitch from C4.
/// - **Decay** (0-1): How long it rings, from 80 ms to 24 s.
/// - **Damping** (0-1): How much faster the top dies than the note.
/// - **Brightness** (0-1): The pluck or the mallet, from soft to hard.
/// - **Position** (0-1): Where it's played, from the end to the middle.
/// - **Inharm** (0-1): A Body's modes, from harmonic to the material's own
///   (0.5) and beyond.
/// - **Level** (0-1): Output level.
pub struct Resonator {
    rng: Pcg32,
    sample_rate: f32,
    mode: Mode,

    // The string
    line: FracDelay,
    /// A pluck, and In, each held back to cancel itself at the place
    /// it's played.
    pick: FracDelay,
    pick_in: FracDelay,
    /// The loop's lowpass: its state, and how it's tuned for the note.
    lowpass: f32,
    tuning: StringTuning,
    /// The pitch V/Oct last asked for, and the loop length it gave.
    last_octaves: f32,
    period: f32,
    /// The noise burst of a pluck: where it is, how long it lasts, how hard,
    /// and its lowpass.
    burst_at: u32,
    burst_len: u32,
    burst_gain: f32,
    /// How much of the pick's grain a pluck has: less, the softer it is.
    burst_grain: f32,
    burst_lp: f32,
    burst_coeff: f32,
    /// In, softened by Brightness.
    input_lp: f32,

    // The body
    modes: [ModeFilter; MODE_COUNT],
    /// The mallet's pulse: where it is, how wide, how hard.
    mallet_at: u32,
    mallet_len: u32,
    mallet_gain: f32,
    /// Samples until the modes are tuned again.
    retune_in: u32,

    /// The gate input, for its rising edges.
    gate: Edge,
    /// Samples since the last pluck or strike (for the face).
    since: u64,
    struck: bool,
    /// The output's envelope, for the face.
    envelope: f32,
    envelope_fall: f32,
    /// A DC blocker after either model.
    dc_x: f32,
    dc_y: f32,
    dc_coeff: f32,
    level: SmoothedValue,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Resonator {
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let mut resonator = Self {
            rng: Pcg32::next_stream(),
            sample_rate,
            mode: Mode::String,
            line: FracDelay::default(),
            pick: FracDelay::default(),
            pick_in: FracDelay::default(),
            lowpass: 0.0,
            tuning: StringTuning::default(),
            last_octaves: f32::NAN,
            period: sample_rate / C4_HZ,
            burst_at: 0,
            burst_len: 0,
            burst_gain: 0.0,
            burst_grain: 1.0,
            burst_lp: 0.0,
            burst_coeff: 1.0,
            input_lp: 0.0,
            modes: [ModeFilter::default(); MODE_COUNT],
            mallet_at: 0,
            mallet_len: 0,
            mallet_gain: 0.0,
            retune_in: 0,
            gate: Edge::LOW,
            since: 0,
            struck: false,
            envelope: 0.0,
            envelope_fall: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
            dc_coeff: 0.0,
            level: SmoothedValue::with_default_smoothing(0.8, sample_rate),
            ports: vec![
                PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0).describe("A rising edge plucks the string or strikes the body; patch a keyboard's or a sequencer's Gate"),
                PortDefinition::input_with_default("v_oct", "V/Oct", SignalType::Control, 0.0).describe("Pitch in volts per octave, added to Tune; a poly cable rings a chord"),
                PortDefinition::input_with_default("strike", "Strike", SignalType::Control, 1.0).describe("How hard each gate plays, 0 to 1, read as it lands: softer is quieter and darker. Unpatched, every one is full"),
                PortDefinition::input("in", "In", SignalType::Audio).describe("Any sound to play it with: a drum, noise, a voice. It rings at its own pitch with whatever you feed it"),
                PortDefinition::input_with_default("decay_cv", "Decay", SignalType::Control, 0.0).describe("CV added to the Decay knob"),
                PortDefinition::input_with_default("damping_cv", "Damping", SignalType::Control, 0.0).describe("CV added to the Damping knob"),
                PortDefinition::input_with_default("brightness_cv", "Brightness", SignalType::Control, 0.0).describe("CV added to the Brightness knob: read at each pluck or strike, and softening In as it goes"),
                PortDefinition::output("out", "Out", SignalType::Audio).describe("The string or the body ringing"),
            ],
            parameters: vec![
                ParameterDefinition::choice("mode", "Mode", MODES, 0).describe("String plucks a string; Body strikes a bell, a bar, a glass or a block of wood"),
                ParameterDefinition::choice("material", "Material", MATERIALS, 0).describe("What a Body is made of: where its modes sit and how long they ring. A String ignores it"),
                ParameterDefinition::new("decay", "Decay", 0.0, 1.0, 0.6, ParameterDisplay::linear("%")).describe("How long it rings, from 80 ms to 24 seconds"),
                ParameterDefinition::new("damping", "Damping", 0.0, 1.0, 0.4, ParameterDisplay::linear("%"))
                    .describe("How much faster the top dies than the note: none is a bright wire, all the way is nylon or felt"),
                ParameterDefinition::new("brightness", "Brightness", 0.0, 1.0, 0.6, ParameterDisplay::linear("%"))
                    .describe("How hard the pluck or the mallet is, from a thumb to a pick, from felt to steel. It softens what comes into In alike"),
                ParameterDefinition::new("tune", "Tune", -24.0, 24.0, 0.0, ParameterDisplay::linear("st")).describe("Pitch in semitones from C4, before V/Oct"),
                ParameterDefinition::new("position", "Position", 0.0, 1.0, 0.3, ParameterDisplay::linear("%"))
                    .describe("Where it's played, from near the end (thin and bright) to the middle (hollow, no even partials)"),
                ParameterDefinition::new("inharm", "Inharm", 0.0, 1.0, 0.5, ParameterDisplay::linear("%"))
                    .describe("A Body's modes: harmonic at 0, the material's own in the middle, stretched further at 1. A String ignores it"),
                ParameterDefinition::new("level", "Level", 0.0, 1.0, 0.8, ParameterDisplay::linear("")).describe("Output level"),
            ],
        };
        resonator.prepare(sample_rate, 0);
        resonator
    }

    pub const PORT_GATE: usize = 0;
    pub const PORT_V_OCT: usize = 1;
    pub const PORT_STRIKE: usize = 2;
    pub const PORT_IN: usize = 3;
    pub const PORT_DECAY_CV: usize = 4;
    pub const PORT_DAMPING_CV: usize = 5;
    pub const PORT_BRIGHTNESS_CV: usize = 6;

    pub const PARAM_MODE: usize = 0;
    pub const PARAM_MATERIAL: usize = 1;
    pub const PARAM_DECAY: usize = 2;
    pub const PARAM_DAMPING: usize = 3;
    pub const PARAM_BRIGHTNESS: usize = 4;
    pub const PARAM_TUNE: usize = 5;
    pub const PARAM_POSITION: usize = 6;
    pub const PARAM_INHARM: usize = 7;
    pub const PARAM_LEVEL: usize = 8;

    /// Readout slots: the mode (0 String, 1 Body), the output's envelope,
    /// the pitch in Hz, and seconds since the last pluck (−1 before the
    /// first). Then the string's shape, [`STRING_POINTS`] of them from
    /// [`READOUT_SHAPE`](Self::READOUT_SHAPE), or each mode's amplitude from
    /// the same slot.
    pub const READOUT_MODE: usize = 0;
    pub const READOUT_LEVEL: usize = 1;
    pub const READOUT_HZ: usize = 2;
    pub const READOUT_SINCE: usize = 3;
    pub const READOUT_SHAPE: usize = 4;

    /// How strongly In drives either model.
    const STRING_INPUT: f32 = 0.35;
    const BODY_INPUT: f32 = 0.5;

    /// How long between retunings of the modes, in samples.
    const RETUNE_EVERY: u32 = 32;

    /// The pitch Tune and V/Oct ask for, in Hz, inside what the model plays.
    fn pitch_hz(&self, tune: f32, octaves: f32) -> f32 {
        let hz = C4_HZ * (tune / 12.0 + octaves).exp2();
        hz.clamp(LOWEST_HZ, (self.sample_rate / 4.0).min(12_000.0))
    }


    /// How bright a pluck, or In, is let in: a lowpass from 250 Hz at
    /// Brightness 0 to beyond hearing at 1.
    fn excitation_hz(brightness: f32) -> f32 {
        250.0 * (6.5 * brightness).exp2()
    }

    /// A one-pole lowpass's coefficient for a corner.
    fn softness(cutoff: f32, sample_rate: f32) -> f32 {
        1.0 - (-TAU * cutoff.min(0.45 * sample_rate) / sample_rate).exp()
    }

    fn pluck(&mut self, strike: f32, brightness: f32) {
        // One period of noise, as Karplus and Strong filled their table,
        // under a half-sine so it starts and ends at rest
        self.burst_at = 0;
        self.burst_len = (self.period.round() as u32).max(8);
        self.burst_gain = 0.55 * strike;
        self.burst_grain = strike;
        // A softer pluck is darker as well as quieter
        let cutoff = Self::excitation_hz(brightness) * (1.5 * (strike - 1.0)).exp2();
        self.burst_coeff = Self::softness(cutoff, self.sample_rate);
        self.burst_lp = 0.0;
    }

    fn strike(&mut self, strike: f32, brightness: f32) {
        // A felt mallet is wide and a steel one narrow; softer hits dwell
        let seconds = (0.0003 + 0.004 * (1.0 - brightness).powi(2)) * (1.0 + 0.8 * (1.0 - strike));
        self.mallet_at = 0;
        self.mallet_len = ((seconds * self.sample_rate) as u32).max(2);
        // Unit area, so a mode well under the mallet's width rings at its own amplitude
        self.mallet_gain = strike * PI / (2.0 * self.mallet_len as f32);
    }

    #[allow(clippy::too_many_arguments)]
    fn retune(&mut self, hz: f32, material: Material, decay: f32, damping: f32, brightness: f32, position: f32, inharm: f32) {
        let ratios = mode_ratios(material, inharm);
        let amps = mode_amplitudes(&ratios, position, brightness);
        let seconds = decay_seconds(decay) * material.ring();
        let loss = material.loss() + 1.2 * damping.clamp(0.0, 1.0);
        let total: f32 = amps.iter().sum::<f32>().max(1e-3);
        let sample_rate = self.sample_rate;
        for ((mode, ratio), amp) in self.modes.iter_mut().zip(ratios).zip(amps) {
            mode.tune(hz * ratio, seconds * ratio.powf(-loss), amp / total * 1.6, sample_rate);
        }
    }

    /// The string's shape now: one period of the line, from the bridge.
    fn string_shape(&self) -> [f32; STRING_POINTS] {
        let mut shape = [0.0; STRING_POINTS];
        for (i, point) in shape.iter_mut().enumerate() {
            *point = self.line.read(1.0 + i as f32 * (self.period - 2.0).max(1.0) / STRING_POINTS as f32);
        }
        shape
    }

    fn silence(&mut self) {
        self.line.clear();
        self.pick.clear();
        self.pick_in.clear();
        self.lowpass = 0.0;
        self.input_lp = 0.0;
        self.burst_at = 0;
        self.burst_len = 0;
        for mode in &mut self.modes {
            mode.y1 = 0.0;
            mode.y2 = 0.0;
        }
        self.mallet_at = 0;
        self.mallet_len = 0;
    }
}

impl Default for Resonator {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Resonator {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: RESONATOR_ID,
            name: "Resonator",
            category: ModuleCategory::Source,
            description: "A physical model: a plucked string, or a struck bell, bar, glass or block of wood, played by a gate or by any sound",
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
        let longest = (sample_rate / LOWEST_HZ).ceil() as usize + 4;
        self.line.allocate(longest);
        self.pick.allocate(longest);
        self.pick_in.allocate(longest);
        self.level.set_sample_rate(sample_rate);
        self.envelope_fall = (-1.0 / (0.08 * sample_rate)).exp();
        self.dc_coeff = (-TAU * 12.0 / sample_rate).exp();
        self.last_octaves = f32::NAN;
        self.period = sample_rate / C4_HZ;
        self.retune_in = 0;
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
        let gate = connected_input(inputs, Self::PORT_GATE);
        let v_oct = connected_input(inputs, Self::PORT_V_OCT);
        let strike_in = connected_input(inputs, Self::PORT_STRIKE);
        let audio_in = connected_input(inputs, Self::PORT_IN);
        let at = |buffer: Option<&SignalBuffer>, i: usize, default: f32| buffer.map_or(default, |buf| buf.at(i));
        let cv = |port: usize| at(connected_input(inputs, port), 0, 0.0);
        let param = |index: usize, default: f32| params.get(index).copied().unwrap_or(default);

        let mode = Mode::from_param(param(Self::PARAM_MODE, 0.0));
        if mode != self.mode {
            // The other model's ringing doesn't wait to come back later
            self.silence();
            self.mode = mode;
            self.retune_in = 0;
        }
        let material = Material::from_param(param(Self::PARAM_MATERIAL, 0.0));
        let tune = param(Self::PARAM_TUNE, 0.0);
        let decay = (param(Self::PARAM_DECAY, 0.6) + cv(Self::PORT_DECAY_CV)).clamp(0.0, 1.0);
        let damping = (param(Self::PARAM_DAMPING, 0.4) + cv(Self::PORT_DAMPING_CV)).clamp(0.0, 1.0);
        let brightness = (param(Self::PARAM_BRIGHTNESS, 0.6) + cv(Self::PORT_BRIGHTNESS_CV)).clamp(0.0, 1.0);
        let position = param(Self::PARAM_POSITION, 0.3).clamp(0.0, 1.0);
        let inharm = param(Self::PARAM_INHARM, 0.5);
        self.level.set_target(param(Self::PARAM_LEVEL, 0.8).clamp(0.0, 1.0));

        let seconds = decay_seconds(decay);
        let place = strike_place(position);
        let input_coeff = Self::softness(Self::excitation_hz(brightness), self.sample_rate);
        self.tuning = StringTuning::new(self.period, damping, seconds, self.sample_rate);

        for i in 0..context.block_size {
            // The pitch, worked out again only when V/Oct moves
            let octaves = at(v_oct, i, 0.0);
            if octaves != self.last_octaves {
                self.last_octaves = octaves;
                self.period = self.sample_rate / self.pitch_hz(tune, octaves);
                self.tuning = StringTuning::new(self.period, damping, seconds, self.sample_rate);
                self.retune_in = 0;
            }

            if self.gate.rising(at(gate, i, 0.0)) {
                let strike = at(strike_in, i, 1.0).clamp(0.0, 1.0);
                match mode {
                    Mode::String => self.pluck(strike, brightness),
                    Mode::Body => self.strike(strike, brightness),
                }
                self.since = 0;
                self.struck = true;
            }
            // In through the same softness as the pick or the mallet
            self.input_lp += input_coeff * (at(audio_in, i, 0.0) - self.input_lp);
            let input = self.input_lp;

            let sample = match mode {
                Mode::String => {
                    let mut pluck = 0.0;
                    if self.burst_at < self.burst_len {
                        let window = (PI * self.burst_at as f32 / self.burst_len as f32).sin();
                        self.burst_lp += self.burst_coeff * (self.rng.bipolar() - self.burst_lp);
                        // A smooth swell, as a finger pulls the string aside,
                        // so every pluck sounds its note alike; and noise,
                        // the grain of the pick
                        let grain = self.burst_lp * (1.0 / self.burst_coeff.sqrt()).min(4.0);
                        pluck = self.burst_gain * window * (0.6 + self.burst_grain * grain);
                        self.burst_at += 1;
                    }
                    // Played at a place along it: what travels the other way
                    // comes back flipped and cancels the partials with a
                    // node there
                    let reflected = (place * self.period).max(1.0);
                    self.pick.push(pluck);
                    let pluck = pluck - self.pick.read(reflected);
                    self.pick_in.push(Self::STRING_INPUT * input);
                    let heard = Self::STRING_INPUT * input - self.pick_in.read(reflected);

                    // Round the loop: the line, the lowpass, then the gain
                    // that sets how long a period takes to fade
                    let tuning = self.tuning;
                    let u = self.line.read(tuning.read);
                    self.lowpass = u + tuning.pole * (self.lowpass - u);
                    let string = tuning.gain * self.lowpass;
                    self.line.push(soft_ceiling(string + pluck + heard));
                    // A pickup on the string: In is heard only as the string
                    // answers it, a period on, but a pluck sounds at once
                    soft_ceiling(string + pluck)
                }
                Mode::Body => {
                    if self.retune_in == 0 {
                        let hz = self.sample_rate / self.period;
                        self.retune(hz, material, decay, damping, brightness, position, inharm);
                        self.retune_in = Self::RETUNE_EVERY;
                    }
                    self.retune_in -= 1;
                    let mut excite = Self::BODY_INPUT * input;
                    if self.mallet_at < self.mallet_len {
                        excite += self.mallet_gain * (PI * (self.mallet_at as f32 + 0.5) / self.mallet_len as f32).sin();
                        self.mallet_at += 1;
                    }
                    let mut sum = 0.0;
                    for m in &mut self.modes {
                        sum += m.tick(excite);
                    }
                    soft_ceiling(sum)
                }
            };

            // Block DC (a pluck's noise has some), then the level
            self.dc_y = sample - self.dc_x + self.dc_coeff * self.dc_y;
            self.dc_x = sample;
            let y = soft_ceiling(self.dc_y) * self.level.next();
            self.envelope = y.abs().max(self.envelope * self.envelope_fall);
            self.since = self.since.saturating_add(1);
            out.samples[i] = y;
        }
    }

    fn reset(&mut self) {
        self.silence();
        self.gate.reset();
        self.struck = false;
        self.since = 0;
        self.envelope = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.last_octaves = f32::NAN;
        self.retune_in = 0;
    }

    fn seed(&mut self, seed: u64) {
        self.rng = Pcg32::new(seed);
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let v = &mut readout.values;
        v[Self::READOUT_MODE] = if self.mode == Mode::Body { 1.0 } else { 0.0 };
        v[Self::READOUT_LEVEL] = self.envelope;
        v[Self::READOUT_HZ] = self.sample_rate / self.period;
        v[Self::READOUT_SINCE] = if self.struck { self.since as f32 / self.sample_rate } else { -1.0 };
        match self.mode {
            Mode::String => v[Self::READOUT_SHAPE..][..STRING_POINTS].copy_from_slice(&self.string_shape()),
            Mode::Body => {
                for (slot, mode) in v[Self::READOUT_SHAPE..][..MODE_COUNT].iter_mut().zip(&self.modes) {
                    *slot = mode.amplitude();
                }
            }
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{peak, rms, Spectrum};
    use crate::dsp::Poly;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;

    fn params(mode: Mode) -> Vec<f32> {
        let mut params: Vec<f32> = Resonator::new().parameters().iter().map(|p| p.default).collect();
        params[Resonator::PARAM_MODE] = if mode == Mode::Body { 1.0 } else { 0.0 };
        params
    }

    /// What the inputs carry, sample by sample: a gate at the given
    /// samples, a fixed V/Oct, and In.
    struct Play<'a> {
        gates: &'a [usize],
        v_oct: f32,
        input: Option<&'a [f32]>,
    }

    fn play(resonator: &mut Resonator, params: &[f32], seconds: f32, how: Play) -> Vec<f32> {
        resonator.prepare(SR, BLOCK);
        resonator.seed(1);
        let ctx = ProcessContext::new(SR, BLOCK);
        let total = (seconds * SR) as usize;
        let mut gate = SignalBuffer::new(BLOCK, SignalType::Gate);
        let mut v_oct = SignalBuffer::new(BLOCK, SignalType::Control);
        v_oct.samples.fill(how.v_oct);
        let mut input = if how.input.is_some() { SignalBuffer::new(BLOCK, SignalType::Audio) } else { SignalBuffer::unconnected(BLOCK, SignalType::Audio) };
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::new(BLOCK, SignalType::Audio)];
        let mut out = Vec::with_capacity(total);
        let mut start = 0;
        while start < total {
            for (k, sample) in gate.samples.iter_mut().enumerate() {
                let n = start + k;
                *sample = if how.gates.iter().any(|&g| n >= g && n < g + 240) { 1.0 } else { 0.0 };
            }
            if let Some(source) = how.input {
                for (k, sample) in input.samples.iter_mut().enumerate() {
                    *sample = source.get(start + k).copied().unwrap_or(0.0);
                }
            }
            resonator.process(&[&gate, &v_oct, &unpatched, &input, &unpatched, &unpatched, &unpatched], &mut outputs, params, &ctx);
            out.extend_from_slice(&outputs[0].samples);
            start += BLOCK;
        }
        out.truncate(total);
        out
    }

    fn pluck(params: &[f32], v_oct: f32, seconds: f32) -> Vec<f32> {
        play(&mut Resonator::new(), params, seconds, Play { gates: &[0], v_oct, input: None })
    }

    fn cents(f: f64, reference: f64) -> f64 {
        1200.0 * (f / reference).log2()
    }

    /// The strongest partial within a semitone of `near`, refined between bins.
    fn partial_near(samples: &[f32], near: f64) -> f64 {
        let spectrum = Spectrum::of(samples, SR);
        let m = &spectrum.magnitudes;
        let lo = (near * 0.94 / spectrum.bin_hz) as usize;
        let hi = ((near * 1.06 / spectrum.bin_hz) as usize).min(m.len() - 2);
        let k = (lo.max(1)..=hi).max_by(|&a, &b| m[a].total_cmp(&m[b])).unwrap();
        let (a, b, c) = (m[k - 1].max(1e-30).ln(), m[k].max(1e-30).ln(), m[k + 1].max(1e-30).ln());
        let denom = a - 2.0 * b + c;
        let offset = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
        (k as f64 + offset) * spectrum.bin_hz
    }

    fn magnitude_near(samples: &[f32], hz: f64) -> f64 {
        let spectrum = Spectrum::of(samples, SR);
        let k = (hz / spectrum.bin_hz).round() as usize;
        spectrum.magnitudes[k - 2..=k + 2].iter().copied().fold(0.0, f64::max)
    }

    fn note_hz(midi: f32) -> f64 {
        440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0)
    }

    fn v_oct_of(midi: f32) -> f32 {
        (midi - 60.0) / 12.0
    }

    #[test]
    fn test_info_ports_and_parameters() {
        let resonator = Resonator::new();
        assert_eq!(resonator.info().id, "source.resonator");
        assert_eq!(resonator.info().category, ModuleCategory::Source);
        let ids: Vec<_> = resonator.ports().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["gate", "v_oct", "strike", "in", "decay_cv", "damping_cv", "brightness_cv", "out"]);
        let names: Vec<_> = resonator.parameters().iter().map(|p| p.name).collect();
        assert_eq!(names, ["Mode", "Material", "Decay", "Damping", "Brightness", "Tune", "Position", "Inharm", "Level"]);
    }

    #[test]
    fn test_the_string_is_in_tune_across_the_keyboard() {
        let mut p = params(Mode::String);
        p[Resonator::PARAM_DAMPING] = 0.8;
        p[Resonator::PARAM_DECAY] = 0.8;
        for midi in (28..=100).step_by(6) {
            let midi = midi as f32;
            let out = pluck(&p, v_oct_of(midi), 1.2);
            let f = partial_near(&out[9600..], note_hz(midi));
            let c = cents(f, note_hz(midi));
            assert!(c.abs() < 2.0, "MIDI {midi}: {f:.2} Hz is {c:+.2} cents out");
        }
    }

    #[test]
    fn test_damping_and_brightness_leave_the_string_in_tune() {
        for (damping, brightness) in [(0.0, 1.0), (1.0, 0.0), (0.5, 0.5)] {
            let mut p = params(Mode::String);
            p[Resonator::PARAM_DAMPING] = damping;
            p[Resonator::PARAM_BRIGHTNESS] = brightness;
            p[Resonator::PARAM_DECAY] = 0.8;
            let midi = 76.0;
            let f = partial_near(&pluck(&p, v_oct_of(midi), 1.0)[4800..], note_hz(midi));
            assert!(cents(f, note_hz(midi)).abs() < 2.0, "damping {damping}: {f:.2} Hz");
        }
    }

    #[test]
    fn test_tune_shifts_by_semitones() {
        let mut p = params(Mode::String);
        p[Resonator::PARAM_TUNE] = 7.0;
        p[Resonator::PARAM_DAMPING] = 0.8;
        let f = partial_near(&pluck(&p, 0.0, 1.0)[4800..], note_hz(67.0));
        assert!(cents(f, note_hz(67.0)).abs() < 2.0, "{f:.2} Hz");
    }

    #[test]
    fn test_every_body_is_in_tune_across_the_keyboard() {
        for material in 0..MATERIALS.len() {
            let mut p = params(Mode::Body);
            p[Resonator::PARAM_MATERIAL] = material as f32;
            p[Resonator::PARAM_DECAY] = 0.9;
            for midi in (36..=96).step_by(12) {
                let midi = midi as f32;
                let out = pluck(&p, v_oct_of(midi), 1.0);
                let f = partial_near(&out[2400..], note_hz(midi));
                let c = cents(f, note_hz(midi));
                assert!(c.abs() < 2.0, "{} at MIDI {midi}: {f:.2} Hz is {c:+.2} cents out", MATERIALS[material]);
            }
        }
    }

    #[test]
    fn test_a_body_rings_at_its_materials_ratios() {
        // A bell's tierce, a minor third over the octave, is what makes it a bell
        let mut p = params(Mode::Body);
        p[Resonator::PARAM_DECAY] = 0.9;
        p[Resonator::PARAM_BRIGHTNESS] = 1.0;
        let out = pluck(&p, -1.0, 1.0);
        let fundamental = C4_HZ as f64 / 2.0;
        let tierce = partial_near(&out[2400..], fundamental * 2.379);
        assert!(cents(tierce, fundamental * 2.379).abs() < 3.0, "{tierce:.2}");
        // Inharm 0 pulls them into a harmonic series
        p[Resonator::PARAM_INHARM] = 0.0;
        let ratios = mode_ratios(Material::Bell, 0.0);
        assert!(ratios.iter().enumerate().all(|(i, &r)| (r - (i + 1) as f32).abs() < 1e-4));
        let out = pluck(&p, -1.0, 1.0);
        let third = partial_near(&out[2400..], fundamental * 3.0);
        assert!(cents(third, fundamental * 3.0).abs() < 3.0, "{third:.2}");
    }

    #[test]
    fn test_silent_until_played() {
        for mode in [Mode::String, Mode::Body] {
            let out = play(&mut Resonator::new(), &params(mode), 0.5, Play { gates: &[], v_oct: 0.0, input: None });
            assert_eq!(peak(&out), 0.0, "{mode:?}");
        }
    }

    #[test]
    fn test_a_gate_plays_both_models() {
        for mode in [Mode::String, Mode::Body] {
            let out = pluck(&params(mode), 0.0, 1.0);
            assert!(peak(&out) > 0.2, "{mode:?} peaks at {}", peak(&out));
            assert!(peak(&out) <= 1.0);
            // It rings on past the pluck
            assert!(rms(&out[24000..26400]) > 0.005, "{mode:?}: {}", rms(&out[24000..26400]));
        }
    }

    #[test]
    fn test_audio_in_plays_both_models_at_their_pitch() {
        // A click of noise, as a drum might send, rings the note
        let mut rng = Pcg32::new(7);
        let mut hit = vec![0.0; 48000];
        for (n, sample) in hit.iter_mut().enumerate().take(480) {
            *sample = rng.bipolar() * (1.0 - n as f32 / 480.0);
        }
        for mode in [Mode::String, Mode::Body] {
            let mut p = params(mode);
            p[Resonator::PARAM_DAMPING] = 0.8;
            p[Resonator::PARAM_DECAY] = 0.8;
            let out = play(&mut Resonator::new(), &p, 1.0, Play { gates: &[], v_oct: -1.0, input: Some(&hit) });
            assert!(peak(&out) > 0.05, "{mode:?} peaks at {}", peak(&out));
            let f = partial_near(&out[4800..], C4_HZ as f64 / 2.0);
            assert!(cents(f, C4_HZ as f64 / 2.0).abs() < 3.0, "{mode:?}: {f:.2} Hz");
        }
    }

    #[test]
    fn test_full_scale_noise_into_the_longest_decay_stays_inside_one() {
        let mut rng = Pcg32::new(3);
        let noise: Vec<f32> = (0..5 * 48000).map(|_| rng.bipolar()).collect();
        for mode in [Mode::String, Mode::Body] {
            for damping in [0.0, 1.0] {
                let mut p = params(mode);
                p[Resonator::PARAM_DECAY] = 1.0;
                p[Resonator::PARAM_DAMPING] = damping;
                p[Resonator::PARAM_LEVEL] = 1.0;
                // Plucked every 10 ms on top
                let gates: Vec<usize> = (0..500).map(|n| n * 480).collect();
                for v_oct in [-2.5, 0.0, 2.5] {
                    let out = play(&mut Resonator::new(), &p, 5.0, Play { gates: &gates, v_oct, input: Some(&noise) });
                    assert!(out.iter().all(|s| s.is_finite()), "{mode:?}");
                    assert!(peak(&out) <= 1.0, "{mode:?} at {v_oct}: {}", peak(&out));
                    // ...and still a sound, not stuck at the rail
                    assert!(rms(&out[4 * 48000..]) < 0.95);
                }
            }
        }
    }

    #[test]
    fn test_decay_sets_how_long_it_rings() {
        for mode in [Mode::String, Mode::Body] {
            let tail = |decay: f32| {
                let mut p = params(mode);
                p[Resonator::PARAM_DECAY] = decay;
                p[Resonator::PARAM_DAMPING] = 0.0;
                let out = pluck(&p, -1.0, 3.0);
                rms(&out[48000..52800]) / rms(&out[2400..7200])
            };
            assert!(tail(0.8) > 4.0 * tail(0.4), "{mode:?}: {} vs {}", tail(0.8), tail(0.4));
        }
    }

    #[test]
    fn test_the_strings_decay_is_its_t60() {
        // Decay 0.5 rings for 0.08 × 300^0.5 s: one second in, it has
        // fallen by 60 dB × (1 / 1.39)
        let mut p = params(Mode::String);
        p[Resonator::PARAM_DAMPING] = 0.0;
        p[Resonator::PARAM_DECAY] = 0.5;
        let out = pluck(&p, -1.0, 2.0);
        let early = rms(&out[4800..9600]);
        let late = rms(&out[52800..57600]);
        let fallen = 20.0 * (early / late).log10();
        let expected = 60.0 * 1.0 / decay_seconds(0.5);
        assert!((fallen - expected).abs() < 4.0, "fell {fallen:.1} dB, expected {expected:.1}");
    }

    #[test]
    fn test_plucked_in_the_middle_the_even_partials_vanish() {
        let mut p = params(Mode::String);
        p[Resonator::PARAM_DAMPING] = 0.2;
        p[Resonator::PARAM_BRIGHTNESS] = 1.0;
        let mut partials = |position: f32| {
            p[Resonator::PARAM_POSITION] = position;
            let out = pluck(&p, -1.0, 0.6);
            let f0 = C4_HZ as f64 / 2.0;
            (magnitude_near(&out[2400..], 2.0 * f0), magnitude_near(&out[2400..], 3.0 * f0))
        };
        let (second, third) = partials(1.0);
        assert!(second < third * 0.2, "middle: 2nd {second:.4}, 3rd {third:.4}");
        let (second, third) = partials(0.3);
        assert!(second > third * 0.3, "off-centre: 2nd {second:.4}, 3rd {third:.4}");
    }

    #[test]
    fn test_damping_darkens_the_string_as_it_rings() {
        let brightness = |damping: f32| {
            let mut p = params(Mode::String);
            p[Resonator::PARAM_DAMPING] = damping;
            p[Resonator::PARAM_BRIGHTNESS] = 1.0;
            let out = pluck(&p, -1.0, 1.0);
            let window = &out[24000..33600];
            let f0 = C4_HZ as f64 / 2.0;
            magnitude_near(window, 8.0 * f0) / magnitude_near(window, f0)
        };
        assert!(brightness(0.0) > 5.0 * brightness(0.8), "{} vs {}", brightness(0.0), brightness(0.8));
    }

    #[test]
    fn test_a_softer_strike_is_quieter_and_darker() {
        for mode in [Mode::String, Mode::Body] {
            let hit = |strike: f32| {
                let mut resonator = Resonator::new();
                resonator.prepare(SR, BLOCK);
                resonator.seed(1);
                let ctx = ProcessContext::new(SR, BLOCK);
                let mut gate = SignalBuffer::new(BLOCK, SignalType::Gate);
                let mut strike_in = SignalBuffer::new(BLOCK, SignalType::Control);
                strike_in.samples.fill(strike);
                let mut v_oct = SignalBuffer::new(BLOCK, SignalType::Control);
                v_oct.samples.fill(-1.0);
                let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
                let mut outputs = vec![SignalBuffer::new(BLOCK, SignalType::Audio)];
                let mut out = Vec::new();
                for block in 0..60 {
                    gate.samples.fill(if block == 0 { 1.0 } else { 0.0 });
                    resonator.process(&[&gate, &v_oct, &strike_in, &unpatched, &unpatched, &unpatched, &unpatched], &mut outputs, &params(mode), &ctx);
                    out.extend_from_slice(&outputs[0].samples);
                }
                let f0 = C4_HZ as f64 / 2.0;
                let window = &out[1200..9600];
                (rms(window), magnitude_near(window, 6.0 * f0) / magnitude_near(window, f0))
            };
            let (loud, bright) = hit(1.0);
            let (soft, dark) = hit(0.3);
            assert!(soft < 0.5 * loud, "{mode:?}: {soft} vs {loud}");
            assert!(dark < bright, "{mode:?}: {dark} vs {bright}");
        }
    }

    #[test]
    fn test_a_poly_chord_rings_as_a_chord() {
        // C, E and G on one cable, one gate for all three
        let chord = [60.0, 64.0, 67.0];
        let mut poly = Poly::<Resonator>::default();
        poly.prepare(SR, BLOCK);
        poly.seed(1);
        let ctx = ProcessContext::new(SR, BLOCK);
        let mut v_oct = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        v_oct.set_channels(chord.len());
        for (channel, midi) in chord.iter().enumerate() {
            v_oct.channel_mut(channel).fill(v_oct_of(*midi));
        }
        let mut gate = SignalBuffer::new(BLOCK, SignalType::Gate);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let mut p = params(Mode::String);
        p[Resonator::PARAM_DAMPING] = 0.6;
        let mut voices = vec![Vec::new(); chord.len()];
        for block in 0..200 {
            gate.samples.fill(if block == 0 { 1.0 } else { 0.0 });
            poly.process(&[&gate, &v_oct, &unpatched, &unpatched, &unpatched, &unpatched, &unpatched], &mut outputs, &p, &ctx);
            assert_eq!(outputs[0].channels(), chord.len());
            for (channel, voice) in voices.iter_mut().enumerate() {
                voice.extend_from_slice(&outputs[0].voice(channel).samples);
            }
        }
        for (voice, midi) in voices.iter().zip(chord) {
            let f = partial_near(&voice[4800..], note_hz(midi));
            assert!(cents(f, note_hz(midi)).abs() < 2.0, "MIDI {midi}: {f:.2} Hz");
            assert!(rms(&voice[24000..28800]) > 0.001, "MIDI {midi} has stopped ringing");
        }
    }

    #[test]
    fn test_changing_mode_lets_go_of_the_other_model() {
        let mut resonator = Resonator::new();
        let string = params(Mode::String);
        let _ = play(&mut resonator, &string, 0.2, Play { gates: &[0], v_oct: 0.0, input: None });
        let out = play(&mut resonator, &params(Mode::Body), 0.2, Play { gates: &[], v_oct: 0.0, input: None });
        // Only the DC blocker's last breath, gone within 50 ms
        assert!(peak(&out[2400..]) < 1e-3, "{}", peak(&out[2400..]));
    }

    #[test]
    fn test_readout_shows_the_string_and_the_modes() {
        let mut resonator = Resonator::new();
        assert_eq!(resonator.readout(&params(Mode::String)).unwrap().values[Resonator::READOUT_SINCE], -1.0);
        let _ = play(&mut resonator, &params(Mode::String), 0.1, Play { gates: &[0], v_oct: 0.0, input: None });
        let readout = resonator.readout(&params(Mode::String)).unwrap().values;
        assert_eq!(readout[Resonator::READOUT_MODE], 0.0);
        assert!((readout[Resonator::READOUT_HZ] - C4_HZ).abs() < 0.01);
        assert!(readout[Resonator::READOUT_SINCE] > 0.09);
        assert!(readout[Resonator::READOUT_LEVEL] > 0.01);
        assert!(readout[Resonator::READOUT_SHAPE..][..STRING_POINTS].iter().any(|v| v.abs() > 1e-3));

        let body = params(Mode::Body);
        let _ = play(&mut resonator, &body, 0.1, Play { gates: &[0], v_oct: 0.0, input: None });
        let readout = resonator.readout(&body).unwrap().values;
        assert_eq!(readout[Resonator::READOUT_MODE], 1.0);
        let amps = &readout[Resonator::READOUT_SHAPE..][..MODE_COUNT];
        assert!(amps[0] > 0.01, "{amps:?}");
        // The high modes of a bell have died sooner than its hum
        assert!(amps[11] < amps[0], "{amps:?}");
    }

    #[test]
    fn test_resonator_registry_instantiation() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create(RESONATOR_ID).expect("Resonator is registered");
        assert_eq!(module.info().name, "Resonator");
        assert!(module.polyphonic());
    }
}
