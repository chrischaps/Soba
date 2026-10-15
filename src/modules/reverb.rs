//! Stereo Reverb effect module.
//!
//! An 8-line feedback delay network (FDN). The input is smeared by a
//! four-step diffuser, then circulates through eight delay lines that are
//! mixed together on every pass, so each echo splits into eight and the tail
//! thickens into a smooth wash instead of a set of ringing combs.
//!
//! ```text
//!  L/R ─ pre-delay ─ 8-channel diffuser ─┬───────────────────────────┬─ early
//!                    (delay, flip,       │  ┌─ 8 modulated lines ─┐   │
//!                     Hadamard) × 4      └─►┤                     ├───┴─ late ─► L/R
//!                                           └ Householder ◄ damp ◄┘
//! ```

use crate::dsp::{
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    context::ProcessContext,
    denormal::flush,
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::FracDelay,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    connected_input, ParameterDisplay, SignalType,
};

/// Channels in the diffuser and the feedback network.
const LINES: usize = 8;

/// Diffuser steps. Each multiplies the number of echoes by eight.
const DIFFUSER_STEPS: usize = 4;

/// Feedback line lengths relative to the shortest. They spread unevenly over
/// a little under an octave, so no two lines share a period and their
/// resonances interleave instead of piling up.
const LINE_RATIOS: [f32; LINES] = [1.000, 1.093, 1.172, 1.297, 1.393, 1.531, 1.657, 1.829];

/// Where each channel's delay sits inside its slot of a diffuser step (0..1).
/// A step divides its span into eight slots, one per channel, so the echoes
/// are spread evenly but never land on a regular grid.
const DIFFUSER_JITTER: [[f32; LINES]; DIFFUSER_STEPS] = [
    [0.42, 0.77, 0.13, 0.58, 0.91, 0.29, 0.66, 0.05],
    [0.71, 0.18, 0.49, 0.86, 0.33, 0.62, 0.09, 0.95],
    [0.24, 0.53, 0.81, 0.11, 0.68, 0.37, 0.92, 0.46],
    [0.87, 0.35, 0.61, 0.22, 0.04, 0.79, 0.51, 0.16],
];

/// Polarity flips before each diffuser step's mix (bit c flips channel c).
/// Without them the Hadamard mix would send a mono input straight back to
/// one channel.
const DIFFUSER_FLIPS: [u8; DIFFUSER_STEPS] = [0b1001_0110, 0b0110_0101, 0b1010_0011, 0b0101_1100];

/// Shortest feedback line at Size 0 and Size 100%, in milliseconds. Size
/// sweeps between them exponentially: a small room to a large hall.
const MIN_ROOM_MS: f32 = 12.0;
const MAX_ROOM_MS: f32 = 100.0;

/// The first diffuser step spans this fraction of the shortest line; each
/// later step spans half the one before. At 1.0 the early reflections are
/// still arriving when the first echoes come back round the lines, so they
/// run straight into the tail. Shorter spans leave a hole between the two
/// that turns a snare into a flam.
const DIFFUSION: f32 = 1.0;

/// Peak delay swing at Mod 100%, in milliseconds.
const MAX_MOD_MS: f32 = 1.0;

/// Modulation rate of each line, in Hz. Slow and unrelated, so the drift
/// reads as air moving in the room rather than as vibrato.
const MOD_RATES: [f32; LINES] = [0.31, 0.43, 0.53, 0.67, 0.73, 0.89, 0.97, 1.13];

/// Damping is set by how fast this frequency dies, in Hz: high enough to leave
/// the body of a sound alone, low enough to be where "bright" and "dark" are heard.
const DAMPING_REF_HZ: f32 = 4000.0;

/// At Damping 100%, [`DAMPING_REF_HZ`] dies away in this fraction of the
/// Decay time.
const MIN_HF_RATIO: f32 = 0.1;

/// How long the network takes to follow the Size knob, in milliseconds. Line
/// lengths glide (and the tail bends in pitch), so this is gentler than the
/// usual parameter smoothing.
const SIZE_SMOOTHING_MS: f32 = 60.0;

/// How often (in samples) the feedback gains and damping filters are
/// recomputed, and the modulation oscillators renormalized.
const COEFF_UPDATE_INTERVAL: usize = 32;

/// Maximum pre-delay in seconds.
const MAX_PREDELAY_SECONDS: f32 = 0.1;

/// Spreads a stereo input over eight channels without changing its energy.
const INPUT_GAIN: f32 = 0.5;

/// Level of the diffused input heard directly as early reflections, relative
/// to the first pass through the feedback lines.
const EARLY_GAIN: f32 = 1.0;

/// Folds four channels into each side. At the default settings the wet
/// signal carries a little more power than the input (about +1.5 dB), some
/// 6 dB below the old Freeverb, which ran hot enough to push Mix into clipping.
const OUTPUT_GAIN: f32 = 1.0;

/// Mixes eight channels with a normalized Hadamard matrix, via a fast
/// Walsh-Hadamard transform. Every output hears every input equally and the
/// total energy is unchanged.
#[inline]
fn hadamard(x: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        for start in (0..LINES).step_by(h * 2) {
            for j in start..start + h {
                let (a, b) = (x[j], x[j + h]);
                x[j] = a + b;
                x[j + h] = a - b;
            }
        }
        h *= 2;
    }
    let norm = 1.0 / (LINES as f32).sqrt();
    for v in x.iter_mut() {
        *v *= norm;
    }
}

/// Mixes eight channels with a Householder reflection, `I - (2/N)·11ᵀ`. Each
/// line keeps most of itself and hands a little to every other line, which
/// keeps the tail dense without washing out the separate lines' character.
/// Energy preserving, and costs one sum.
#[inline]
fn householder(x: &mut [f32; LINES]) {
    let k = x.iter().sum::<f32>() * (2.0 / LINES as f32);
    for v in x.iter_mut() {
        *v -= k;
    }
}

/// Stereo feedback-delay-network reverb.
///
/// # Ports
///
/// - **In L** (Audio, Input): Left channel input.
/// - **In R** (Audio, Input): Right channel input (normalled from L).
/// - **Out L** (Audio, Output): Processed left channel.
/// - **Out R** (Audio, Output): Processed right channel.
///
/// # Parameters
///
/// - **Size** (0-100%): Room size; scales every delay in the network.
/// - **Decay** (0.1-30s): Time for the tail to fall 60 dB (at low frequencies
///   when damped).
/// - **Damping** (0-100%): How much faster the highs die than the lows.
/// - **Pre-Delay** (0-100ms): Initial delay before reverb starts.
/// - **Mix** (0-100%): Wet/dry balance.
/// - **Width** (0-100%): Stereo width of the reverb.
/// - **Mod** (0-100%): Slow drift of the delay lines; a chorus in the tail.
pub struct Reverb {
    /// Sample rate.
    sample_rate: f32,
    /// Pre-delay lines (left, right).
    predelay: [FracDelay; 2],
    /// Diffuser delay lines, one per channel per step.
    diffuser: [[FracDelay; LINES]; DIFFUSER_STEPS],
    /// Diffuser tap positions as a fraction of the shortest feedback line.
    diffuser_taps: [[f32; LINES]; DIFFUSER_STEPS],
    /// The feedback delay lines.
    lines: [FracDelay; LINES],
    /// Per-line damping filter state.
    damp_state: [f32; LINES],
    /// Per-line loop gain at DC, for the Decay time.
    loop_gain: [f32; LINES],
    /// Per-line damping filter pole.
    damp_pole: [f32; LINES],
    /// Per-line modulation oscillator, as a rotating (cos, sin) pair.
    lfo: [(f32, f32); LINES],
    /// Per-sample rotation of each modulation oscillator (cos, sin).
    lfo_step: [(f32, f32); LINES],
    /// Smoothed room size.
    size_smooth: SmoothedValue,
    /// Smoothed decay.
    decay_smooth: SmoothedValue,
    /// Smoothed damping.
    damping_smooth: SmoothedValue,
    /// Smoothed pre-delay time.
    predelay_smooth: SmoothedValue,
    /// Smoothed wet/dry mix.
    mix_smooth: SmoothedValue,
    /// Smoothed stereo width.
    width_smooth: SmoothedValue,
    /// Smoothed modulation depth.
    mod_smooth: SmoothedValue,
    /// The tail's level over the last block (dB, RMS of the eight lines),
    /// for the display.
    tail_db: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl Reverb {
    /// Creates a new reverb.
    pub fn new() -> Self {
        let sample_rate = 44100.0;

        // Step s spans DIFFUSION / 2^s of the shortest line, in eight slots.
        // Channels take the slots in a different order on every step, so no
        // channel is always the longest path.
        let diffuser_taps = Self::diffuser_taps();

        let mut reverb = Self {
            sample_rate,
            predelay: Default::default(),
            diffuser: Default::default(),
            diffuser_taps,
            lines: Default::default(),
            damp_state: [0.0; LINES],
            loop_gain: [0.0; LINES],
            damp_pole: [0.0; LINES],
            lfo: [(1.0, 0.0); LINES],
            lfo_step: [(1.0, 0.0); LINES],
            size_smooth: SmoothedValue::new(0.5, SIZE_SMOOTHING_MS, sample_rate),
            decay_smooth: SmoothedValue::with_default_smoothing(2.0, sample_rate),
            damping_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            predelay_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            mix_smooth: SmoothedValue::with_default_smoothing(0.3, sample_rate),
            width_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            tail_db: -120.0,
            mod_smooth: SmoothedValue::with_default_smoothing(0.25, sample_rate),
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in_l", "In L", SignalType::Audio, 0.0).describe("Left audio to put in the room"),
                PortDefinition::input_with_default("in_r", "In R", SignalType::Audio, 0.0).describe("Right audio to put in the room; copies left when unpatched"),
                // Output ports
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("Left reverb, mixed with the dry signal"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("Right reverb, mixed with the dry signal"),
            ],
            parameters: vec![
                ParameterDefinition::normalized("size", "Size", 0.5).describe("Room size; larger spaces sound deeper and more spread out"),
                ParameterDefinition::new(
                    "decay",
                    "Decay",
                    0.1,
                    30.0,
                    2.0,
                    ParameterDisplay::Logarithmic { unit: "s" },
                ).describe("How long the tail rings out, as RT60 in seconds"),
                ParameterDefinition::normalized("damping", "Damping", 0.5).describe("How fast highs die away in the tail; higher is darker"),
                ParameterDefinition::new(
                    "predelay",
                    "Pre-Delay",
                    0.0,
                    100.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "ms" },
                ).describe("Silence before the reverb starts, in ms"),
                ParameterDefinition::normalized("mix", "Mix", 0.3).describe("Blend from dry (0) to reverb only (1)"),
                ParameterDefinition::normalized("width", "Width", 1.0).describe("Stereo spread of the tail; 0 is mono, 1 is full width"),
                // Appended so positional (v1/v2) patches keep their mapping
                ParameterDefinition::normalized("mod", "Mod", 0.25).describe("Slow movement inside the tail that smooths out ringing"),
            ],
        };
        reverb.allocate();
        reverb
    }

    /// Port index constants.
    const PORT_IN_L: usize = 0;
    const PORT_IN_R: usize = 1;
    const PORT_OUT_L: usize = 0;
    const PORT_OUT_R: usize = 1;

    /// Parameter index constants.
    const PARAM_SIZE: usize = 0;
    const PARAM_DECAY: usize = 1;
    const PARAM_DAMPING: usize = 2;
    const PARAM_PREDELAY: usize = 3;
    const PARAM_MIX: usize = 4;
    const PARAM_WIDTH: usize = 5;
    const PARAM_MOD: usize = 6;

    /// Readout slots: the tail's level (dB), then each feedback line's
    /// modulation phase (0 to 1).
    pub const READOUT_TAIL_DB: usize = 0;
    pub const READOUT_MOD_PHASE: usize = 1;
    /// How many feedback lines (and modulation phases) there are.
    pub const LINES: usize = LINES;

    /// Every diffuser tap, step by step and channel by channel, as a fraction
    /// of the shortest feedback line. Channels take the slots in a different
    /// order on every step, so no channel is always the longest path.
    pub fn diffuser_taps() -> [[f32; LINES]; DIFFUSER_STEPS] {
        let mut diffuser_taps = [[0.0; LINES]; DIFFUSER_STEPS];
        for (step, taps) in diffuser_taps.iter_mut().enumerate() {
            let span = DIFFUSION / (1 << step) as f32;
            for (channel, tap) in taps.iter_mut().enumerate() {
                let slot = (channel * 5 + step * 3) % LINES;
                *tap = span * (slot as f32 + DIFFUSER_JITTER[step][channel]) / LINES as f32;
            }
        }
        diffuser_taps
    }

    /// The shortest feedback line for a Size setting, in milliseconds.
    pub fn room_ms(size: f32) -> f32 {
        MIN_ROOM_MS * (MAX_ROOM_MS / MIN_ROOM_MS).powf(size.clamp(0.0, 1.0))
    }

    /// How long the highs (at 4 kHz) take to fall 60 dB, in seconds, for a
    /// Decay and Damping.
    pub fn hf_decay(decay: f32, damping: f32) -> f32 {
        decay * MIN_HF_RATIO.powf(damping.clamp(0.0, 1.0))
    }

    /// Length of the shortest feedback line for a Size setting, in samples.
    #[inline]
    fn room_samples(size: f32, sample_rate: f32) -> f32 {
        Self::room_ms(size) * 0.001 * sample_rate
    }

    /// Gain per pass through a loop of `delay_samples` so that a signal
    /// circulating in it falls 60 dB in `decay_seconds`.
    ///
    /// After `T` seconds a signal has made `T/d` passes of `d` seconds each,
    /// so g^(T/d) = 10^(-60/20), which gives g = 10^(-3·d/T).
    fn decay_to_feedback(decay_seconds: f32, delay_samples: f32, sample_rate: f32) -> f32 {
        let delay_seconds = delay_samples / sample_rate;
        if decay_seconds <= 0.0 || delay_seconds <= 0.0 {
            return 0.0;
        }
        10.0_f32.powf(-3.0 * delay_seconds / decay_seconds).min(0.9999)
    }

    /// Sets each line's loop gain for the Decay time and its damping filter
    /// for the Damping amount.
    ///
    /// The damping filter is a one-pole lowpass whose DC gain is the loop
    /// gain and whose gain at [`DAMPING_REF_HZ`] is the loop gain for a
    /// shorter decay. Both are worked out from the line's own length, so every
    /// line loses its highs at the same rate per second and the tail darkens
    /// evenly instead of the short lines ringing brighter.
    fn update_loop(&mut self, room: f32, decay: f32, damping: f32) {
        let decay = decay.max(0.01);
        let hf_decay = Self::hf_decay(decay, damping);
        let w = std::f32::consts::TAU * DAMPING_REF_HZ.min(0.4 * self.sample_rate) / self.sample_rate;
        for line in 0..LINES {
            let delay = room * LINE_RATIOS[line];
            let gain = Self::decay_to_feedback(decay, delay, self.sample_rate);
            let hf_gain = Self::decay_to_feedback(hf_decay, delay, self.sample_rate);
            let ratio = if gain > 0.0 { hf_gain / gain } else { 1.0 };
            self.loop_gain[line] = gain;
            self.damp_pole[line] = Self::one_pole_for_gain(ratio, w);
        }
    }

    /// The pole `a` of the unity-DC lowpass `(1-a)/(1 - a·z⁻¹)` whose gain at
    /// `w` radians per sample is `gain` (0 < gain ≤ 1).
    ///
    /// Setting |H(w)|² = R gives (1-R)a² - 2(1-R·cos w)a + (1-R) = 0. The two
    /// roots multiply to 1, so the stable one is (1-R) over the larger root's
    /// denominator, which stays accurate as R approaches 1.
    fn one_pole_for_gain(gain: f32, w: f32) -> f32 {
        let r = gain.clamp(1e-6, 1.0) * gain.clamp(1e-6, 1.0);
        let b = 1.0 - r * w.cos();
        let disc = (b * b - (1.0 - r) * (1.0 - r)).max(0.0);
        (1.0 - r) / (b + disc.sqrt())
    }

    /// Sets the modulation oscillators' rates and spreads their phases.
    fn reset_lfos(&mut self) {
        for line in 0..LINES {
            let w = std::f32::consts::TAU * MOD_RATES[line] / self.sample_rate;
            self.lfo_step[line] = (w.cos(), w.sin());
            // Golden-ratio phase spread, so no two lines swing together
            let phase = std::f32::consts::TAU * (line as f32 * 0.618_034).fract();
            self.lfo[line] = (phase.cos(), phase.sin());
        }
    }

    /// Allocates every delay line for the current sample rate.
    fn allocate(&mut self) {
        let sr = self.sample_rate;
        let max_room = Self::room_samples(1.0, sr);
        let max_mod = MAX_MOD_MS * 0.001 * sr;

        for line in &mut self.predelay {
            line.allocate((MAX_PREDELAY_SECONDS * sr) as usize + 1);
        }
        for (step, lines) in self.diffuser.iter_mut().enumerate() {
            let longest = self.diffuser_taps[step].iter().fold(0.0f32, |a, &b| a.max(b));
            for line in lines.iter_mut() {
                line.allocate((longest * max_room) as usize + 2);
            }
        }
        let longest = LINE_RATIOS.iter().fold(0.0f32, |a, &b| a.max(b));
        for line in &mut self.lines {
            line.allocate((longest * max_room + max_mod) as usize + 2);
        }
        self.damp_state = [0.0; LINES];
        self.reset_lfos();
        self.update_loop(
            Self::room_samples(self.size_smooth.current(), sr),
            self.decay_smooth.current(),
            self.damping_smooth.current(),
        );
    }
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Reverb {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "fx.reverb",
            name: "Reverb",
            category: ModuleCategory::Effect,
            description: "Stereo feedback-delay-network reverb with size, decay, damping and a modulated tail",
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

        // Update sample rate for smoothed values
        self.size_smooth.set_sample_rate(sample_rate);
        self.decay_smooth.set_sample_rate(sample_rate);
        self.damping_smooth.set_sample_rate(sample_rate);
        self.predelay_smooth.set_sample_rate(sample_rate);
        self.mix_smooth.set_sample_rate(sample_rate);
        self.width_smooth.set_sample_rate(sample_rate);
        self.mod_smooth.set_sample_rate(sample_rate);

        self.allocate();
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Set smoothing targets
        self.size_smooth.set_target(params[Self::PARAM_SIZE]);
        self.decay_smooth.set_target(params[Self::PARAM_DECAY]);
        self.damping_smooth.set_target(params[Self::PARAM_DAMPING]);
        self.predelay_smooth.set_target(params[Self::PARAM_PREDELAY]);
        self.mix_smooth.set_target(params[Self::PARAM_MIX]);
        self.width_smooth.set_target(params[Self::PARAM_WIDTH]);
        self.mod_smooth.set_target(params.get(Self::PARAM_MOD).copied().unwrap_or(0.25));

        // Get input buffers
        let in_left = inputs.get(Self::PORT_IN_L);
        // Right is normalled from left when nothing is plugged into it
        let in_right = connected_input(inputs, Self::PORT_IN_R);

        // Split outputs
        let (out_left_slice, out_right_slice) = outputs.split_at_mut(1);
        let out_left = &mut out_left_slice[Self::PORT_OUT_L];
        let out_right = &mut out_right_slice[Self::PORT_OUT_R - 1];

        let ms_to_samples = 0.001 * self.sample_rate;
        let mut tail_energy = 0.0f32;

        for i in 0..context.block_size {
            let size = self.size_smooth.next();
            let decay = self.decay_smooth.next();
            let damping = self.damping_smooth.next();
            let predelay = self.predelay_smooth.next() * ms_to_samples;
            let mix = self.mix_smooth.next();
            let width = self.width_smooth.next();
            let depth = self.mod_smooth.next() * MAX_MOD_MS * ms_to_samples;

            // Size sets every delay in the network; it glides, so work it out per sample
            let room = Self::room_samples(size, self.sample_rate);

            if i % COEFF_UPDATE_INTERVAL == 0 {
                self.update_loop(room, decay, damping);
                // Pull the oscillators back onto the unit circle against rounding drift
                for (c, s) in &mut self.lfo {
                    let k = 1.5 - 0.5 * (*c * *c + *s * *s);
                    *c *= k;
                    *s *= k;
                }
            }

            let dry_left = in_left
                .map(|buf| buf.samples.get(i).copied().unwrap_or(0.0))
                .unwrap_or(0.0);
            let dry_right = match in_right {
                Some(buf) => buf.samples.get(i).copied().unwrap_or(0.0),
                None => dry_left,
            };

            // Pre-delay (one sample at the least, which is far below hearing)
            self.predelay[0].push(dry_left);
            self.predelay[1].push(dry_right);
            let pre_l = self.predelay[0].read(predelay);
            let pre_r = self.predelay[1].read(predelay);

            // Spread the stereo input over eight channels: even from the left, odd from the right
            let mut x = [0.0f32; LINES];
            for (c, v) in x.iter_mut().enumerate() {
                *v = if c % 2 == 0 { pre_l } else { pre_r } * INPUT_GAIN;
            }

            // Diffuser: delay each channel differently, flip some, then mix them all
            for step in 0..DIFFUSER_STEPS {
                for c in 0..LINES {
                    let line = &mut self.diffuser[step][c];
                    line.push(x[c]);
                    let tapped = line.read(self.diffuser_taps[step][c] * room);
                    x[c] = if DIFFUSER_FLIPS[step] >> c & 1 == 1 { -tapped } else { tapped };
                }
                hadamard(&mut x);
            }

            // Feedback network: read each line (gliding slowly), damp, mix, write back
            let mut late = [0.0f32; LINES];
            let mut feedback = [0.0f32; LINES];
            for c in 0..LINES {
                let (cos, sin) = self.lfo[c];
                let (step_cos, step_sin) = self.lfo_step[c];
                self.lfo[c] = (cos * step_cos - sin * step_sin, sin * step_cos + cos * step_sin);

                // The read happens before this sample's write, hence the -1
                late[c] = self.lines[c].read(room * LINE_RATIOS[c] + depth * sin - 1.0);

                let pole = self.damp_pole[c];
                let state = flush(
                    self.loop_gain[c] * (1.0 - pole) * late[c] + pole * self.damp_state[c],
                );
                self.damp_state[c] = state;
                feedback[c] = state;
            }
            tail_energy += late.iter().map(|v| v * v).sum::<f32>();
            householder(&mut feedback);
            for c in 0..LINES {
                self.lines[c].push(feedback[c] + x[c]);
            }

            // Fold the early reflections and the tail down to stereo
            let mut wet_l = 0.0;
            let mut wet_r = 0.0;
            for c in (0..LINES).step_by(2) {
                wet_l += late[c] + EARLY_GAIN * x[c];
                wet_r += late[c + 1] + EARLY_GAIN * x[c + 1];
            }
            wet_l *= OUTPUT_GAIN;
            wet_r *= OUTPUT_GAIN;

            // Stereo width: 0 = mono, 1 = full stereo
            let mid = (wet_l + wet_r) * 0.5;
            let side = (wet_l - wet_r) * 0.5 * width;

            out_left.samples[i] = dry_left * (1.0 - mix) + (mid + side) * mix;
            out_right.samples[i] = dry_right * (1.0 - mix) + (mid - side) * mix;
        }

        let mean = tail_energy / (context.block_size.max(1) * LINES) as f32;
        self.tail_db = 10.0 * mean.max(1e-12).log10();
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_TAIL_DB] = self.tail_db;
        for (line, (cos, sin)) in self.lfo.iter().enumerate() {
            readout.values[Self::READOUT_MOD_PHASE + line] = sin.atan2(*cos) / std::f32::consts::TAU + 0.5;
        }
        Some(readout)
    }

    fn reset(&mut self) {
        for line in &mut self.predelay {
            line.clear();
        }
        for line in self.diffuser.iter_mut().flatten() {
            line.clear();
        }
        for line in &mut self.lines {
            line.clear();
        }
        self.damp_state = [0.0; LINES];
        self.reset_lfos();

        // Reset smoothed values
        self.size_smooth.reset(self.size_smooth.target());
        self.decay_smooth.reset(self.decay_smooth.target());
        self.damping_smooth.reset(self.damping_smooth.target());
        self.predelay_smooth.reset(self.predelay_smooth.target());
        self.mix_smooth.reset(self.mix_smooth.target());
        self.width_smooth.reset(self.width_smooth.target());
        self.mod_smooth.reset(self.mod_smooth.target());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{echo_density, rt60};

    #[test]
    fn test_reverb_info() {
        let reverb = Reverb::new();
        assert_eq!(reverb.info().id, "fx.reverb");
        assert_eq!(reverb.info().name, "Reverb");
        assert_eq!(reverb.info().category, ModuleCategory::Effect);
    }

    #[test]
    fn test_reverb_ports() {
        let reverb = Reverb::new();
        let ports = reverb.ports();

        assert_eq!(ports.len(), 4);

        // Input ports
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in_l");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "in_r");
        assert_eq!(ports[1].signal_type, SignalType::Audio);

        // Output ports
        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "out_l");
        assert_eq!(ports[2].signal_type, SignalType::Audio);

        assert!(ports[3].is_output());
        assert_eq!(ports[3].id, "out_r");
        assert_eq!(ports[3].signal_type, SignalType::Audio);
    }

    #[test]
    fn test_reverb_parameters() {
        let reverb = Reverb::new();
        let params = reverb.parameters();

        // The first six keep their ids and order so existing patches load
        let ids: Vec<&str> = params.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["size", "decay", "damping", "predelay", "mix", "width", "mod"]);
    }

    #[test]
    fn test_hadamard_and_householder_preserve_energy() {
        let input = [0.3, -1.2, 0.7, 0.05, -0.4, 0.9, 0.0, 1.1];
        let energy = |x: &[f32; LINES]| x.iter().map(|v| v * v).sum::<f32>();
        for mix in [hadamard as fn(&mut [f32; LINES]), householder] {
            let mut x = input;
            mix(&mut x);
            assert!((energy(&x) - energy(&input)).abs() < 1e-5);
        }
        // Hadamard spreads one channel evenly over all eight
        let mut x = [0.0; LINES];
        x[3] = 1.0;
        hadamard(&mut x);
        assert!(x.iter().all(|v| (v.abs() - 1.0 / 8f32.sqrt()).abs() < 1e-6));
    }

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;

    fn params(size: f32, decay: f32, damping: f32, modulation: f32) -> [f32; 7] {
        // size, decay, damping, predelay, mix (all wet), width, mod
        [size, decay, damping, 0.0, 1.0, 1.0, modulation]
    }

    /// Renders the stereo wet impulse response, after letting the smoothed
    /// parameters settle.
    fn impulse_response(params: [f32; 7], seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let mut reverb = Reverb::new();
        reverb.prepare(SR, BLOCK);
        let ctx = ProcessContext::new(SR, BLOCK);
        let silence = SignalBuffer::audio(BLOCK);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK)];
        for _ in 0..(0.5 * SR) as usize / BLOCK {
            reverb.process(&[&silence], &mut outputs, &params, &ctx);
        }

        let total = (seconds * SR) as usize;
        let (mut left, mut right) = (Vec::with_capacity(total), Vec::with_capacity(total));
        let mut impulse = SignalBuffer::audio(BLOCK);
        impulse.samples[0] = 1.0;
        while left.len() < total {
            let input = if left.is_empty() { &impulse } else { &silence };
            reverb.process(&[input], &mut outputs, &params, &ctx);
            left.extend_from_slice(&outputs[0].samples);
            right.extend_from_slice(&outputs[1].samples);
        }
        (left, right)
    }

    #[test]
    fn test_reverb_produces_output() {
        let (left, right) = impulse_response(params(0.5, 2.0, 0.5, 0.25), 0.2);
        assert!(left.iter().any(|&s| s.abs() > 0.001), "Expected left output");
        assert!(right.iter().any(|&s| s.abs() > 0.001), "Expected right output");
    }

    #[test]
    fn test_reverb_tail() {
        let (left, _) = impulse_response(params(0.5, 2.0, 0.5, 0.25), 1.0);
        let tail = &left[(0.9 * SR) as usize..];
        assert!(tail.iter().any(|&s| s.abs() > 0.0001), "Expected reverb tail to continue");
    }

    #[test]
    fn test_reverb_reset() {
        let mut reverb = Reverb::new();
        reverb.prepare(44100.0, 256);

        // Fill reverb with signal
        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);
        let mut outputs = vec![SignalBuffer::audio(256), SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);
        let p = params(0.5, 2.0, 0.5, 0.25);
        reverb.process(&[&input, &input], &mut outputs, &p, &ctx);

        reverb.reset();

        // Process silence: nothing should come out
        let silence = SignalBuffer::audio(256);
        reverb.process(&[&silence, &silence], &mut outputs, &p, &ctx);
        assert!(outputs[0].samples.iter().all(|&s| s == 0.0), "Expected silence after reset");
    }

    #[test]
    fn test_reverb_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Reverb>();
    }

    #[test]
    fn test_reverb_default() {
        let reverb = Reverb::default();
        assert_eq!(reverb.info().id, "fx.reverb");
    }

    #[test]
    fn test_decay_to_feedback() {
        let short_feedback = Reverb::decay_to_feedback(0.1, 1000.0, 44100.0);
        let long_feedback = Reverb::decay_to_feedback(10.0, 1000.0, 44100.0);
        assert!(short_feedback < long_feedback, "Longer decay should give higher feedback");
        assert!(long_feedback < 1.0, "Feedback should stay below unity");
        assert!(short_feedback >= 0.0, "Feedback should be non-negative");

        // RT60 identity: g^(T/d) must equal -60 dB
        let (decay, delay, sr) = (2.0, 1400.0, 44100.0);
        let g = Reverb::decay_to_feedback(decay, delay, sr);
        let trips = decay / (delay / sr);
        let level_db = 20.0 * g.powf(trips).log10();
        assert!((level_db + 60.0).abs() < 0.5, "Expected -60 dB after decay, got {}", level_db);
    }

    #[test]
    fn test_one_pole_for_gain_hits_its_target() {
        let w = std::f32::consts::TAU * 4000.0 / 48000.0;
        for target in [1.0, 0.999_99, 0.9, 0.5, 0.1] {
            let a = Reverb::one_pole_for_gain(target, w);
            assert!((0.0..1.0).contains(&a), "pole {a} for gain {target}");
            let gain = (1.0 - a) / (1.0 - 2.0 * a * w.cos() + a * a).sqrt();
            assert!((gain - target).abs() < 1e-4, "gain {gain} for target {target}");
        }
    }

    #[test]
    fn test_rt60_matches_decay_knob() {
        // The issue's bar is ±15%; across sizes, decays and modulation depths
        for &(size, decay, modulation) in &[
            (0.0, 0.5, 0.0),
            (0.2, 1.0, 0.25),
            (0.5, 2.0, 0.25),
            (0.8, 4.0, 1.0),
            (1.0, 8.0, 0.5),
        ] {
            let (left, right) = impulse_response(params(size, decay, 0.0, modulation), decay * 0.8 + 0.3);
            for (side, ir) in [("L", &left), ("R", &right)] {
                let measured = rt60(ir, SR).expect("tail should fall past -35 dB");
                assert!(
                    (measured / decay - 1.0).abs() < 0.15,
                    "Size {size} Decay {decay}s Mod {modulation} {side}: measured RT60 {measured:.3}s"
                );
            }
        }
    }

    #[test]
    fn test_damping_shortens_the_highs_not_the_lows() {
        // Decay 2 s at Damping 70%: 4 kHz should die in 2 * 0.1^0.7 = 0.4 s
        // while the lows keep the full 2 s
        let (ir, _) = impulse_response(params(0.5, 2.0, 0.7, 0.25), 2.0);
        let band = |hz: f32| bandpass(&ir, hz);
        let low_rt = rt60(&band(150.0), SR).unwrap();
        let high_rt = rt60(&band(DAMPING_REF_HZ), SR).unwrap();
        let expected_high = 2.0 * MIN_HF_RATIO.powf(0.7);
        assert!((low_rt / 2.0 - 1.0).abs() < 0.15, "lows should decay in ~2 s, got {low_rt:.2}");
        assert!(
            (high_rt / expected_high - 1.0).abs() < 0.25,
            "4 kHz should decay in ~{expected_high:.2} s, got {high_rt:.2}"
        );

        // Undamped, 4 kHz keeps (nearly) the full decay; the spline reads take a little
        let (bright, _) = impulse_response(params(0.5, 2.0, 0.0, 0.25), 2.0);
        let bright_rt = rt60(&bandpass(&bright, DAMPING_REF_HZ), SR).unwrap();
        assert!(bright_rt > 1.6, "undamped 4 kHz decays in {bright_rt:.2}s");
    }

    /// A narrow band-pass (RBJ biquad, Q = 8) around `hz`.
    fn bandpass(x: &[f32], hz: f32) -> Vec<f32> {
        let w = std::f32::consts::TAU * hz / SR;
        let alpha = w.sin() / (2.0 * 8.0);
        let a0 = 1.0 + alpha;
        let (b0, b2) = (alpha / a0, -alpha / a0);
        let (a1, a2) = (-2.0 * w.cos() / a0, (1.0 - alpha) / a0);
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|&s| {
                let y = b0 * s + b2 * x2 - a1 * y1 - a2 * y2;
                (x2, x1, y2, y1) = (x1, s, y1, y);
                y
            })
            .collect()
    }

    #[test]
    fn test_size_changes_onset_and_spacing() {
        // The first sound out is the first diffuser echo, which scales with Size
        let onset = |size: f32| {
            let (ir, _) = impulse_response(params(size, 1.0, 0.0, 0.0), 0.2);
            ir.iter().position(|s| s.abs() > 1e-6).unwrap()
        };
        let (small, large) = (onset(0.0), onset(1.0));
        assert!(large > small * 6, "Size should scale the room: {small} vs {large} samples");
    }

    #[test]
    fn test_tail_is_dense_not_metallic() {
        // A diffuse tail looks like noise: echo density near 1 from early on.
        // Freeverb's combs sat around 0.6-0.7, which is the grainy, metallic sound.
        let (ir, _) = impulse_response(params(0.5, 2.0, 0.0, 0.0), 0.6);
        let window = |from: f32| &ir[(from * SR) as usize..((from + 0.02) * SR) as usize];
        for from in [0.08, 0.15, 0.3, 0.5] {
            let density = echo_density(window(from));
            assert!(density > 0.85, "echo density {density:.2} at {from}s");
        }

        // And no single echo period dominates: the normalized autocorrelation
        // of the flattened tail stays low at every lag from 1 to 100 ms
        let tail: Vec<f32> = ir[(0.15 * SR) as usize..(0.55 * SR) as usize]
            .chunks(480)
            .flat_map(|c| {
                let level = crate::dsp::analysis::rms(c).max(1e-30);
                c.iter().map(move |s| s / level)
            })
            .collect();
        let energy: f32 = tail.iter().map(|s| s * s).sum();
        let worst = (48..4800)
            .step_by(3)
            .map(|lag| {
                let r: f32 = tail.iter().zip(&tail[lag..]).map(|(a, b)| a * b).sum();
                (r / energy).abs()
            })
            .fold(0.0, f32::max);
        assert!(worst < 0.1, "tail repeats itself: autocorrelation {worst:.3}");
    }

    #[test]
    fn test_stereo_outputs_are_decorrelated() {
        let (left, right) = impulse_response(params(0.5, 2.0, 0.5, 0.25), 1.0);
        let dot: f32 = left.iter().zip(&right).map(|(a, b)| a * b).sum();
        let norm = (left.iter().map(|s| s * s).sum::<f32>() * right.iter().map(|s| s * s).sum::<f32>()).sqrt();
        assert!((dot / norm).abs() < 0.2, "L/R correlation {}", dot / norm);

        // Width 0 folds it to mono
        let mut mono = params(0.5, 2.0, 0.5, 0.25);
        mono[5] = 0.0;
        let (left, right) = impulse_response(mono, 0.3);
        assert!(left.iter().zip(&right).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn test_modulation_moves_the_tail() {
        let (still, _) = impulse_response(params(0.5, 2.0, 0.5, 0.0), 1.0);
        let (moving, _) = impulse_response(params(0.5, 2.0, 0.5, 1.0), 1.0);
        let early = (0.03 * SR) as usize;
        let late = (0.9 * SR) as usize;
        // The diffuser isn't modulated, so the start matches...
        assert!(still[..early].iter().zip(&moving[..early]).all(|(a, b)| (a - b).abs() < 1e-6));
        // ...but by the end the tail has drifted into a different waveform
        let diff: f32 = still[late..].iter().zip(&moving[late..]).map(|(a, b)| (a - b).powi(2)).sum();
        let energy: f32 = still[late..].iter().map(|s| s * s).sum();
        assert!(diff > 0.5 * energy, "modulation should decorrelate the tail");
    }

    #[test]
    fn test_size_sweep_is_click_free() {
        // Sweeping Size while a tail rings must glide, not jump
        let mut reverb = Reverb::new();
        reverb.prepare(SR, BLOCK);
        let ctx = ProcessContext::new(SR, BLOCK);
        let mut input = SignalBuffer::audio(BLOCK);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK)];
        let mut out = Vec::new();
        for block in 0..400 {
            // A 220 Hz tone for the first 0.25 s
            for (i, s) in input.samples.iter_mut().enumerate() {
                let n = block * BLOCK + i;
                *s = if n < (0.25 * SR) as usize {
                    0.3 * (std::f32::consts::TAU * 220.0 * n as f32 / SR).sin()
                } else {
                    0.0
                };
            }
            let size = if block < 100 { 0.2 } else { 0.9 };
            reverb.process(&[&input], &mut outputs, &params(size, 3.0, 0.3, 0.25), &ctx);
            out.extend_from_slice(&outputs[0].samples);
        }
        let level = crate::dsp::analysis::rms(&out);
        let worst_step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // A 220 Hz sine at the tail's level moves at most ~0.03·peak per sample;
        // a click is a step many times that
        assert!(worst_step < 10.0 * level * 0.03 * 3.0, "step {worst_step} at rms {level}");
    }

    #[test]
    fn test_silent_tail_reaches_true_zero() {
        // A decaying tail must land on exact silence instead of lingering in
        // slow denormal arithmetic. Tests run without FTZ, so this exercises
        // the per-line flush on its own.
        let (ir, _) = impulse_response(params(0.5, 0.5, 0.5, 0.25), 4.0);
        assert!(ir.iter().all(|s| !s.is_subnormal()), "tail went denormal");
        let tail = &ir[(3.5 * SR) as usize..];
        assert!(tail.iter().all(|&s| s == 0.0), "tail should be exactly silent by 3.5 s");
    }

    #[test]
    fn test_sample_rate_change_keeps_decay() {
        let mut reverb = Reverb::new();
        reverb.prepare(96000.0, BLOCK);
        reverb.prepare(44100.0, BLOCK);
        assert!(reverb.lines[0].max_delay() < Reverb::room_samples(1.0, 96000.0) * 1.9);
    }

    #[test]
    fn test_reverb_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Reverb>();

        assert!(registry.contains("fx.reverb"));

        let module = registry.create("fx.reverb");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "fx.reverb");
        assert_eq!(module.info().name, "Reverb");
        assert_eq!(module.ports().len(), 4);
        assert_eq!(module.parameters().len(), 7);
    }

    #[test]
    fn test_readout_follows_the_tail_down() {
        // After a hit, the tail's level falls 60 dB per Decay time (here 1 s,
        // undamped so the whole tail dies together)
        let params = params(0.5, 1.0, 0.0, 0.0);
        let mut reverb = Reverb::new();
        reverb.prepare(SR, BLOCK);
        let ctx = ProcessContext::new(SR, BLOCK);
        let silence = SignalBuffer::audio(BLOCK);
        let mut impulse = SignalBuffer::audio(BLOCK);
        impulse.samples[0] = 1.0;
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK)];
        let mut level_at = Vec::new();
        for block in 0..(0.8 * SR) as usize / BLOCK {
            reverb.process(&[if block == 0 { &impulse } else { &silence }], &mut outputs, &params, &ctx);
            level_at.push(reverb.readout(&params).unwrap().values[Reverb::READOUT_TAIL_DB]);
        }
        let at = |seconds: f32| level_at[(seconds * SR) as usize / BLOCK];
        let fell = at(0.2) - at(0.7);
        assert!((fell - 30.0).abs() < 6.0, "fell {fell} dB in half a decay");
        let phases = reverb.readout(&params).unwrap().values;
        assert!(phases[Reverb::READOUT_MOD_PHASE..Reverb::READOUT_MOD_PHASE + Reverb::LINES].iter().all(|p| (0.0..=1.0).contains(p)));
    }
}
