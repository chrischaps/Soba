//! Distortion/saturation effect module.
//!
//! Five characters: soft (tanh), hard clip, a wavefolder, an asymmetric
//! "tube" curve, and a bitcrusher. The four curves run at 4x the sample rate
//! with first-order antiderivative anti-aliasing, so even full drive adds
//! harmonics without the inharmonic "digital fizz" of their aliases. The
//! bitcrusher is meant to alias, but against its own Rate rather than the
//! engine's, so it runs oversampled too.

use crate::dsp::{
    context::ProcessContext,
    dynamics::DcBlocker,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{Adaa1, BiasedTanh, Curve, Downsampler4x, HardClip, OnePole, RoundedFolder, Tanh, Upsampler4x},
    signal::{connected_input, SignalBuffer},
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// Oversampling factor for the waveshapers.
const OVERSAMPLE: usize = 4;

/// How fast the input level the display shows falls once the sound passes.
const INPUT_VIEW_FALL_MS: f32 = 150.0;

/// Distortion algorithm types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistortionType {
    /// Soft clipping - smooth tanh saturation, odd harmonics.
    Soft = 0,
    /// Hard clipping - aggressive, flat-topped.
    Hard = 1,
    /// Wave folding - peaks fold back on themselves as drive rises.
    Fold = 2,
    /// Bit crushing - lo-fi depth and sample-rate reduction.
    Bit = 3,
    /// Tube - asymmetric saturation, even harmonics from the first touch.
    Tube = 4,
}

impl DistortionType {
    /// Convert from parameter index to distortion type.
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => DistortionType::Soft,
            1 => DistortionType::Hard,
            2 => DistortionType::Fold,
            3 => DistortionType::Bit,
            4 => DistortionType::Tube,
            _ => DistortionType::Soft,
        }
    }

    /// Whether the type is a static curve run through ADAA.
    fn is_curve(self) -> bool {
        self != DistortionType::Bit
    }
}

/// Bitcrusher state: a sample-and-hold clock and the current word length.
#[derive(Clone, Copy, Debug)]
struct Crusher {
    /// Hold clock phase, 0..1; a new sample is taken each time it wraps.
    phase: f32,
    held: f32,
    /// Drive the levels below were computed for.
    drive: f32,
    /// Quantization steps per unit, `2^bits`.
    levels: f32,
}

impl Crusher {
    const fn new() -> Self {
        Self { phase: 1.0, held: 0.0, drive: -1.0, levels: 65536.0 }
    }

    /// Crushes one oversampled sample. `step` is the hold clock's increment
    /// per sample (zero for no rate reduction).
    #[inline]
    fn process(&mut self, x: f32, drive: f32, step: f32) -> f32 {
        if drive != self.drive {
            // Bits go from 16 down to 2 as drive increases
            self.drive = drive;
            self.levels = (16.0 - drive * 14.0).max(2.0).exp2();
        }
        let x = if step > 0.0 {
            self.phase += step;
            if self.phase >= 1.0 {
                self.phase -= self.phase.floor();
                self.held = x;
            }
            self.held
        } else {
            x
        };
        (x * self.levels).round() / self.levels
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}

/// The module's ID, as patches name it.
pub const ID: &str = "fx.distortion";

/// Distortion effect module with multiple algorithms.
///
/// # Ports
///
/// - **In** (Audio, Input): Audio signal to distort.
/// - **Drive CV** (Control, Input): Modulates drive amount.
/// - **Out** (Audio, Output): Distorted audio output.
///
/// # Parameters
///
/// - **Drive** (0-100%): Distortion intensity.
/// - **Tone** (0-100%): Post-distortion brightness (lowpass filter).
/// - **Type** (Soft/Hard/Fold/Bit/Tube): Distortion algorithm.
/// - **Mix** (0-100%): Wet/dry blend.
/// - **Output** (-12dB to +12dB): Makeup gain.
/// - **Symmetry** (-100% to 100%): Fold only: offsets the wave into the
///   folder, so its two halves fold differently.
/// - **Rate** (Hz): Bit only: the crushed sample rate. At the top of the
///   range (or above the engine's rate) there is no rate reduction.
pub struct Distortion {
    /// Sample rate.
    sample_rate: f32,
    upsampler: Upsampler4x,
    downsampler: Downsampler4x,
    /// Antiderivative state of the active curve.
    adaa: Adaa1,
    tube: BiasedTanh,
    folder: RoundedFolder,
    crusher: Crusher,
    /// The type processed last block, to re-prime ADAA on a switch.
    active_type: DistortionType,
    /// Last shaper input, at the oversampled rate.
    last_drive_input: f32,
    /// Previous oversampled dry sample, for matching ADAA's half-sample delay.
    prev_dry: f32,
    /// Tone filter (runs oversampled).
    tone_filter: OnePole,
    /// Removes the offset the asymmetric curves leave (runs oversampled).
    dc_blocker: DcBlocker,
    /// Smoothed drive parameter.
    drive_smooth: SmoothedValue,
    /// Smoothed tone filter coefficient (at the oversampled rate).
    tone_coeff_smooth: SmoothedValue,
    /// Smoothed mix parameter.
    mix_smooth: SmoothedValue,
    /// Smoothed output gain, linear.
    output_gain_smooth: SmoothedValue,
    /// Smoothed fold symmetry.
    symmetry_smooth: SmoothedValue,
    /// For the display: the input's recent peak, held and let fall at
    /// `INPUT_VIEW_FALL_MS`, and the drive after CV.
    input_view: f32,
    drive_now: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl Distortion {
    /// Bias of the tube curve: enough to lean the two halves well apart.
    const TUBE_BIAS: f64 = 0.5;
    /// Width over which the folder's corners are rounded.
    const FOLD_ROUNDING: f64 = 0.4;
    /// Rate knob range. The top of the range means "no rate reduction".
    const RATE_MIN_HZ: f32 = 100.0;
    const RATE_MAX_HZ: f32 = 48000.0;

    /// Creates a new Distortion effect.
    pub fn new() -> Self {
        let sample_rate = 44100.0;

        let mut dist = Self {
            sample_rate,
            upsampler: Upsampler4x::new(),
            downsampler: Downsampler4x::new(),
            adaa: Adaa1::new(),
            tube: BiasedTanh::new(Self::TUBE_BIAS),
            folder: RoundedFolder::new(Self::FOLD_ROUNDING),
            crusher: Crusher::new(),
            active_type: DistortionType::Soft,
            last_drive_input: 0.0,
            prev_dry: 0.0,
            tone_filter: OnePole::default(),
            dc_blocker: DcBlocker::new(sample_rate * OVERSAMPLE as f32),
            drive_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            tone_coeff_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            mix_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            output_gain_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            symmetry_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            input_view: 0.0,
            drive_now: 0.5,
            ports: vec![
                PortDefinition::input_with_default("in", "In", SignalType::Audio, 0.0).describe("Audio to distort"),
                PortDefinition::input_with_default("drive_cv", "Drive CV", SignalType::Control, 0.0).describe("CV that adds to the drive amount"),
                PortDefinition::output("out", "Out", SignalType::Audio).describe("Distorted audio"),
            ],
            parameters: vec![
                ParameterDefinition::new(
                    "drive",
                    "Drive",
                    0.0,
                    1.0,
                    0.5,
                    ParameterDisplay::Linear { unit: "%" },
                ).describe("How hard the signal is pushed into the distortion"),
                ParameterDefinition::new(
                    "tone",
                    "Tone",
                    0.0,
                    1.0,
                    0.5,
                    ParameterDisplay::Linear { unit: "%" },
                ).describe("Brightness after distortion; low is dark, high is bright"),
                ParameterDefinition::new(
                    "type",
                    "Type",
                    0.0,
                    4.0,
                    0.0,
                    ParameterDisplay::Discrete {
                        labels: &["Soft", "Hard", "Fold", "Bit", "Tube"],
                    },
                ).describe("Distortion flavor: Soft, Hard, Fold, Bit or Tube"),
                ParameterDefinition::new(
                    "mix",
                    "Mix",
                    0.0,
                    1.0,
                    1.0,
                    ParameterDisplay::Linear { unit: "%" },
                ).describe("Blend from dry (0) to distorted (1)"),
                ParameterDefinition::new(
                    "output_gain",
                    "Output",
                    -12.0,
                    12.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Level after distortion, in dB"),
                ParameterDefinition::new(
                    "symmetry",
                    "Symmetry",
                    -1.0,
                    1.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "%" },
                ).describe("Skews the waveform for added even harmonics"),
                ParameterDefinition::new(
                    "rate",
                    "Rate",
                    Self::RATE_MIN_HZ,
                    Self::RATE_MAX_HZ,
                    Self::RATE_MAX_HZ,
                    ParameterDisplay::Logarithmic { unit: "Hz" },
                ).describe("Bit type only: crushed sample rate in Hz; top is no reduction"),
            ],
        };
        dist.tone_coeff_smooth.reset(dist.tone_to_coefficient(0.5));
        dist
    }

    /// Readout slots: the input's recent peak, and the drive after CV.
    pub const READOUT_INPUT_PEAK: usize = 0;
    pub const READOUT_DRIVE: usize = 1;

    /// Port index constants.
    const PORT_IN: usize = 0;
    const PORT_DRIVE_CV: usize = 1;
    const PORT_OUT: usize = 0;

    /// Parameter index constants.
    const PARAM_DRIVE: usize = 0;
    const PARAM_TONE: usize = 1;
    const PARAM_TYPE: usize = 2;
    const PARAM_MIX: usize = 3;
    const PARAM_OUTPUT: usize = 4;
    const PARAM_SYMMETRY: usize = 5;
    const PARAM_RATE: usize = 6;

    /// Convert dB to linear amplitude.
    #[inline]
    fn db_to_linear(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }

    /// Gain into the curve for a drive of 0..1.
    #[inline]
    fn input_gain(distortion_type: DistortionType, drive: f32) -> f32 {
        match distortion_type {
            // 1x to 11x
            DistortionType::Soft | DistortionType::Tube => 1.0 + drive * 10.0,
            // The clip threshold falls from 1.0 to 0.1
            DistortionType::Hard => 1.0 / (1.0 - drive * 0.9).max(0.1),
            // 1x to 6x: up to about three folds per half-wave at full scale
            DistortionType::Fold => 1.0 + drive * 5.0,
            DistortionType::Bit => 1.0,
        }
    }

    /// Where a steady input `x` comes out, before the tone filter: the
    /// type's curve after its drive (and, for Fold, the symmetry offset),
    /// blended with the dry by `mix` and scaled by `output_db`. Bit is its
    /// staircase of levels; its rate reduction acts in time, not here.
    pub fn transfer(distortion_type: DistortionType, drive: f32, symmetry: f32, mix: f32, output_db: f32, x: f32) -> f32 {
        let gain = Self::input_gain(distortion_type, drive.clamp(0.0, 1.0));
        let u = (x * gain + if distortion_type == DistortionType::Fold { symmetry } else { 0.0 }) as f64;
        let wet = match distortion_type {
            DistortionType::Soft => Tanh.value(u) as f32,
            DistortionType::Hard => HardClip.value(u) as f32,
            DistortionType::Fold => RoundedFolder::new(Self::FOLD_ROUNDING).value(u) as f32,
            DistortionType::Tube => BiasedTanh::new(Self::TUBE_BIAS).value(u) as f32,
            DistortionType::Bit => {
                let levels = (16.0 - drive.clamp(0.0, 1.0) * 14.0).max(2.0).exp2();
                (x * levels).round() / levels
            }
        };
        (x + (wet - x) * mix.clamp(0.0, 1.0)) * Self::db_to_linear(output_db)
    }

    /// How many bits the Bit type keeps at `drive`.
    pub fn bits(drive: f32) -> f32 {
        (16.0 - drive.clamp(0.0, 1.0) * 14.0).max(2.0)
    }

    /// Runs the active curve on one oversampled sample.
    #[inline]
    fn shape(&mut self, distortion_type: DistortionType, u: f32) -> f32 {
        match distortion_type {
            DistortionType::Soft => self.adaa.process(&Tanh, u),
            DistortionType::Hard => self.adaa.process(&HardClip, u),
            DistortionType::Fold => self.adaa.process(&self.folder, u),
            DistortionType::Tube => self.adaa.process(&self.tube, u),
            DistortionType::Bit => u,
        }
    }

    /// Restarts ADAA from the last input under the newly active curve.
    fn prime_adaa(&mut self, distortion_type: DistortionType) {
        let u = self.last_drive_input;
        match distortion_type {
            DistortionType::Soft => self.adaa.prime(&Tanh, u),
            DistortionType::Hard => self.adaa.prime(&HardClip, u),
            DistortionType::Fold => self.adaa.prime(&self.folder, u),
            DistortionType::Tube => self.adaa.prime(&self.tube, u),
            DistortionType::Bit => {}
        }
    }

    /// Tone filter coefficient at the oversampled rate.
    /// Tone 0 = very dark, Tone 1 = bright (no filtering).
    fn tone_to_coefficient(&self, tone: f32) -> f32 {
        // Map tone to cutoff frequency (200 Hz to 20000 Hz)
        let min_freq: f32 = 200.0;
        let max_freq: f32 = 20000.0;
        let freq = min_freq * (max_freq / min_freq).powf(tone.clamp(0.0, 1.0));

        // One-pole coefficient from frequency
        let omega = 2.0 * std::f32::consts::PI * freq / (self.sample_rate * OVERSAMPLE as f32);
        (-omega).exp()
    }
}

impl Default for Distortion {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Distortion {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Distortion",
            category: ModuleCategory::Effect,
            description: "Oversampled distortion: soft, hard, wavefolder, tube, and bit crush",
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
        self.dc_blocker.set_sample_rate(sample_rate * OVERSAMPLE as f32);

        // Update sample rates for smoothed values
        self.drive_smooth.set_sample_rate(sample_rate);
        self.tone_coeff_smooth.set_sample_rate(sample_rate);
        self.mix_smooth.set_sample_rate(sample_rate);
        self.output_gain_smooth.set_sample_rate(sample_rate);
        self.symmetry_smooth.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Everything costly about the parameters is worked out once per block;
        // the per-sample loop only smooths toward these.
        self.drive_smooth.set_target(params[Self::PARAM_DRIVE]);
        self.tone_coeff_smooth.set_target(self.tone_to_coefficient(params[Self::PARAM_TONE]));
        self.mix_smooth.set_target(params[Self::PARAM_MIX]);
        self.output_gain_smooth.set_target(Self::db_to_linear(params[Self::PARAM_OUTPUT]));
        self.symmetry_smooth.set_target(params[Self::PARAM_SYMMETRY].clamp(-1.0, 1.0));

        // Discrete, no smoothing needed
        let distortion_type = DistortionType::from_index(params[Self::PARAM_TYPE] as usize);
        if distortion_type != self.active_type {
            self.active_type = distortion_type;
            self.prime_adaa(distortion_type);
            self.crusher.reset();
        }

        // Hold clock increment per oversampled sample; zero means no reduction
        let rate = params.get(Self::PARAM_RATE).copied().unwrap_or(Self::RATE_MAX_HZ);
        let hold_step = if rate < Self::RATE_MAX_HZ && rate < self.sample_rate {
            rate.max(Self::RATE_MIN_HZ) / (self.sample_rate * OVERSAMPLE as f32)
        } else {
            0.0
        };

        let audio_in = connected_input(inputs, Self::PORT_IN);
        let drive_cv = connected_input(inputs, Self::PORT_DRIVE_CV);
        let out = &mut outputs[Self::PORT_OUT];
        let is_curve = distortion_type.is_curve();
        let mut heard = 0.0f32;

        for i in 0..context.block_size {
            let base_drive = self.drive_smooth.next();
            let tone_coeff = self.tone_coeff_smooth.next();
            let mix = self.mix_smooth.next();
            let output_gain = self.output_gain_smooth.next();
            let symmetry = self.symmetry_smooth.next();

            // Add CV modulation to drive (bipolar CV, -1 to +1 range)
            let drive_mod = drive_cv.map_or(0.0, |buf| buf.at(i));
            let drive = (base_drive + drive_mod * 0.5).clamp(0.0, 1.0);
            let gain = Self::input_gain(distortion_type, drive);
            let offset = if distortion_type == DistortionType::Fold { symmetry } else { 0.0 };

            let dry = audio_in.map_or(0.0, |buf| buf.at(i));
            heard = heard.max(dry.abs());
            self.drive_now = drive;

            let mut mixed = [0.0f32; OVERSAMPLE];
            for (slot, x) in mixed.iter_mut().zip(self.upsampler.process(dry)) {
                let (wet, dry) = if is_curve {
                    let u = x * gain + offset;
                    self.last_drive_input = u;
                    // ADAA's output lands half a sample late; so does this dry
                    let aligned = 0.5 * (x + self.prev_dry);
                    (self.shape(distortion_type, u), aligned)
                } else {
                    (self.crusher.process(x, drive, hold_step), x)
                };
                self.prev_dry = x;

                let wet = self.tone_filter.follow(wet, 1.0 - tone_coeff);
                let wet = self.dc_blocker.process(wet);
                *slot = dry + (wet - dry) * mix;
            }

            let output = self.downsampler.process(mixed) * output_gain;
            out.samples[i] = output.clamp(-2.0, 2.0);
        }

        let fall = (-(context.block_size as f32) / (INPUT_VIEW_FALL_MS * 0.001 * self.sample_rate)).exp();
        self.input_view = heard.max(self.input_view * fall);
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_INPUT_PEAK] = self.input_view;
        readout.values[Self::READOUT_DRIVE] = self.drive_now;
        Some(readout)
    }

    fn reset(&mut self) {
        self.upsampler.reset();
        self.downsampler.reset();
        self.adaa.reset();
        self.crusher.reset();
        self.last_drive_input = 0.0;
        self.prev_dry = 0.0;
        self.tone_filter = OnePole::default();
        self.dc_blocker.reset();

        // Reset smoothed values
        self.drive_smooth.reset(self.drive_smooth.target());
        self.tone_coeff_smooth.reset(self.tone_coeff_smooth.target());
        self.mix_smooth.reset(self.mix_smooth.target());
        self.output_gain_smooth.reset(self.output_gain_smooth.target());
        self.symmetry_smooth.reset(self.symmetry_smooth.target());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{rms, Spectrum};
    use std::f64::consts::PI;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 256;

    /// Parameters: drive, tone, type, mix, output dB, symmetry, rate.
    fn params(drive: f32, tone: f32, ty: DistortionType, mix: f32, out_db: f32) -> [f32; 7] {
        [drive, tone, ty as usize as f32, mix, out_db, 0.0, Distortion::RATE_MAX_HZ]
    }

    /// A test tone computed in f64 (an f32 phase carries -60 dB of noise).
    fn sine(freq: f64, amplitude: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amplitude * (2.0 * PI * freq * i as f64 / SR as f64).sin()) as f32)
            .collect()
    }

    /// Runs a whole signal through the module in blocks.
    fn run(dist: &mut Distortion, input: &[f32], params: &[f32], cv: Option<f32>) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        let mut in_buf = SignalBuffer::audio(BLOCK);
        let cv_buf = match cv {
            Some(cv) => {
                let mut buf = SignalBuffer::control(BLOCK);
                buf.fill(cv);
                buf
            }
            None => SignalBuffer::unconnected(BLOCK, SignalType::Control),
        };
        let mut outputs = vec![SignalBuffer::audio(BLOCK)];
        let ctx = ProcessContext::new(SR, BLOCK);
        for chunk in input.chunks(BLOCK) {
            in_buf.samples[..chunk.len()].copy_from_slice(chunk);
            in_buf.samples[chunk.len()..].fill(0.0);
            dist.process(&[&in_buf, &cv_buf], &mut outputs, params, &ctx);
            out.extend_from_slice(&outputs[0].samples[..chunk.len()]);
        }
        out
    }

    fn prepared() -> Distortion {
        let mut dist = Distortion::new();
        dist.prepare(SR, BLOCK);
        dist
    }

    /// Error between `output` and `reference` delayed by the best lag (the
    /// oversampling filters add a few samples of latency).
    fn best_lag_error(reference: &[f32], output: &[f32]) -> (usize, f32) {
        (0..16)
            .map(|lag| {
                let err = (2000..reference.len())
                    .map(|i| (output[i] - reference[i - lag]).abs())
                    .fold(0.0f32, f32::max);
                (lag, err)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap()
    }

    /// `input` through the module's resampling filters alone, optionally
    /// with the half-sample delay that matches ADAA.
    fn round_trip(input: &[f32], align: bool) -> Vec<f32> {
        let mut up = Upsampler4x::new();
        let mut down = Downsampler4x::new();
        let mut prev = 0.0;
        input
            .iter()
            .map(|&x| {
                let mut quad = up.process(x);
                for s in quad.iter_mut() {
                    let x = *s;
                    if align {
                        *s = 0.5 * (x + prev);
                    }
                    prev = x;
                }
                down.process(quad)
            })
            .collect()
    }

    /// Largest difference once the parameter smoothing has settled.
    fn max_error(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).skip(4096).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn test_distortion_info() {
        let dist = Distortion::new();
        assert_eq!(dist.info().id, "fx.distortion");
        assert_eq!(dist.info().name, "Distortion");
        assert_eq!(dist.info().category, ModuleCategory::Effect);
    }

    #[test]
    fn test_distortion_ports() {
        let dist = Distortion::new();
        let ports = dist.ports();

        assert_eq!(ports.len(), 3);
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in");
        assert_eq!(ports[0].signal_type, SignalType::Audio);
        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "drive_cv");
        assert_eq!(ports[1].signal_type, SignalType::Control);
        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "out");
        assert_eq!(ports[2].signal_type, SignalType::Audio);
    }

    #[test]
    fn test_distortion_parameters() {
        let dist = Distortion::new();
        let ids: Vec<_> = dist.parameters().iter().map(|p| p.id).collect();
        // New parameters go on the end so existing indices keep their meaning
        assert_eq!(ids, ["drive", "tone", "type", "mix", "output_gain", "symmetry", "rate"]);
    }

    #[test]
    fn test_distortion_passthrough_at_zero_drive() {
        let mut dist = prepared();
        let input = sine(441.0, 0.3, 8192);
        let out = run(&mut dist, &input, &params(0.0, 1.0, DistortionType::Soft, 1.0, 0.0), None);
        // tanh(x) is within 1% of x at 0.3; the filters delay a few samples
        let (lag, err) = best_lag_error(&input, &out);
        assert!(lag < 8, "latency {} samples", lag);
        assert!(err < 0.02, "max error {} at lag {}", err, lag);
    }

    /// Inharmonic energy below 20 kHz of a full-scale 3 kHz sine, in dB.
    fn alias_db(output: &[f32], f: f64) -> f64 {
        let n = output.len() / 2;
        Spectrum::of_periodic(&output[n..], SR).alias_energy_db(f, 0.5, 20000.0)
    }

    /// The pre-oversampling curves, applied naively at the base rate.
    fn legacy(ty: DistortionType, x: f32, drive: f32) -> f32 {
        match ty {
            DistortionType::Soft => (x * (1.0 + drive * 10.0)).tanh(),
            DistortionType::Hard => {
                let t = (1.0 - drive * 0.9).max(0.1);
                (x.clamp(-t, t) / t).clamp(-1.0, 1.0)
            }
            DistortionType::Fold => ((x * (1.0 + drive * 4.0)).sin() * 2.0).sin(),
            // No legacy tube existed; compare against its curve run naively
            DistortionType::Tube => {
                use crate::dsp::primitives::Curve;
                BiasedTanh::new(Distortion::TUBE_BIAS).value((x * (1.0 + drive * 10.0)) as f64) as f32
            }
            DistortionType::Bit => x,
        }
    }

    #[test]
    fn test_aliasing_at_full_drive_is_30_db_lower() {
        // 3 kHz, periodic in 16384 samples but not in the sample rate, so
        // every alias lands on a bin between the harmonics
        let n = 16384;
        let f = 1115.0 * SR as f64 / n as f64;
        let input = sine(f, 0.9, n * 2);
        for ty in [DistortionType::Soft, DistortionType::Hard, DistortionType::Fold, DistortionType::Tube] {
            let old: Vec<f32> = input.iter().map(|&x| legacy(ty, x, 1.0)).collect();
            let mut dist = prepared();
            let new = run(&mut dist, &input, &params(1.0, 1.0, ty, 1.0, 0.0), None);
            let (old_db, new_db) = (alias_db(&old, f), alias_db(&new, f));
            assert!(
                new_db < old_db - 30.0,
                "{:?}: alias energy {:.1} dB before, {:.1} dB now",
                ty,
                old_db,
                new_db
            );
        }
    }

    #[test]
    fn test_curves_still_distort() {
        // Oversampling must not tame the harmonics we do want
        let n = 16384;
        let f = 279.0 * SR as f64 / n as f64;
        let input = sine(f, 0.9, n * 2);
        for ty in [DistortionType::Soft, DistortionType::Hard, DistortionType::Fold, DistortionType::Tube] {
            let mut dist = prepared();
            let out = run(&mut dist, &input, &params(0.7, 1.0, ty, 1.0, 0.0), None);
            let spectrum = Spectrum::of_periodic(&out[n..], SR);
            let bin = |h: f64| spectrum.magnitudes[(h * f / spectrum.bin_hz).round() as usize];
            let harmonics: f64 = (2..12).map(|h| bin(h as f64).powi(2)).sum::<f64>().sqrt();
            assert!(harmonics > 0.1 * bin(1.0), "{:?}: harmonics {:.4} vs fundamental {:.4}", ty, harmonics, bin(1.0));
        }
    }

    #[test]
    fn test_tube_makes_even_harmonics_and_no_dc() {
        let n = 16384;
        let f = 279.0 * SR as f64 / n as f64;
        let input = sine(f, 0.5, n * 2);
        let harmonic = |ty: DistortionType, h: f64| {
            let mut dist = prepared();
            let out = run(&mut dist, &input, &params(0.3, 1.0, ty, 1.0, 0.0), None);
            let mean = out[n..].iter().sum::<f32>() / n as f32;
            let spectrum = Spectrum::of_periodic(&out[n..], SR);
            (spectrum.magnitudes[(h * f / spectrum.bin_hz).round() as usize], mean)
        };
        let (tube_h2, tube_dc) = harmonic(DistortionType::Tube, 2.0);
        let (soft_h2, _) = harmonic(DistortionType::Soft, 2.0);
        let (tube_h1, _) = harmonic(DistortionType::Tube, 1.0);
        assert!(tube_h2 > 0.05 * tube_h1, "tube 2nd harmonic {:.4} vs {:.4}", tube_h2, tube_h1);
        assert!(soft_h2 < 1e-4, "soft should stay odd: 2nd harmonic {:.6}", soft_h2);
        assert!(tube_dc.abs() < 1e-3, "tube leaves {:.4} of DC", tube_dc);
    }

    #[test]
    fn test_fold_symmetry_doubles_the_frequency() {
        // Offset to a fold peak, a small sine swings both ways off the peak:
        // the output rises the same way on both half-cycles.
        let n = 16384;
        let f = 139.0 * SR as f64 / n as f64;
        let input = sine(f, 0.15, n * 2);
        let mut p = params(0.0, 1.0, DistortionType::Fold, 1.0, 0.0);
        p[Distortion::PARAM_SYMMETRY] = 1.0;
        let mut dist = prepared();
        let out = run(&mut dist, &input, &p, None);
        let spectrum = Spectrum::of_periodic(&out[n..], SR);
        let bin = |h: f64| spectrum.magnitudes[(h * f / spectrum.bin_hz).round() as usize];
        assert!(bin(2.0) > 10.0 * bin(1.0), "2nd {:.4} vs 1st {:.4}", bin(2.0), bin(1.0));

        // Centred, it is symmetric again: odd harmonics only
        p[Distortion::PARAM_SYMMETRY] = 0.0;
        p[Distortion::PARAM_DRIVE] = 1.0;
        let mut dist = prepared();
        let out = run(&mut dist, &sine(f, 0.9, n * 2), &p, None);
        let spectrum = Spectrum::of_periodic(&out[n..], SR);
        let bin = |h: f64| spectrum.magnitudes[(h * f / spectrum.bin_hz).round() as usize];
        assert!(bin(2.0) < 1e-4 && bin(3.0) > 0.01, "2nd {:.5}, 3rd {:.5}", bin(2.0), bin(3.0));
    }

    #[test]
    fn test_bit_rate_holds_samples() {
        let input = sine(441.0, 0.8, 44100);
        let mut p = params(0.0, 1.0, DistortionType::Bit, 1.0, 0.0);

        // Full rate: a clean 16-bit copy (give or take the tone filter and
        // DC blocker's phase)
        let mut dist = prepared();
        let out = run(&mut dist, &input, &p, None);
        let err = max_error(&round_trip(&input, false), &out);
        assert!(err < 0.01, "full rate error {}", err);

        // 2 kHz: the crushed signal steps 2000 times a second, which the
        // output spectrum shows as an image at 2000 - 441 Hz
        p[Distortion::PARAM_RATE] = 2000.0;
        let mut dist = prepared();
        let out = run(&mut dist, &input, &p, None);
        let spectrum = Spectrum::of(&out[22050..], SR);
        let level = |hz: f64| {
            let bin = (hz / spectrum.bin_hz).round() as usize;
            spectrum.magnitudes[bin - 2..=bin + 2].iter().cloned().fold(0.0, f64::max)
        };
        let image = level(2000.0 - 441.0) / level(441.0);
        assert!(image > 0.1, "image at {:.3} of the fundamental", image);
    }

    #[test]
    fn test_bit_depth_quantizes() {
        let input = sine(441.0, 0.8, 8192);
        let mut dist = prepared();
        // 2 bits: four levels per unit, so the wave is a staircase
        let out = run(&mut dist, &input, &params(1.0, 1.0, DistortionType::Bit, 1.0, 0.0), None);
        let (_, err) = best_lag_error(&input, &out);
        assert!(err > 0.1, "2-bit output should be coarse, error {}", err);
    }

    #[test]
    fn test_distortion_mix() {
        let mut dist = prepared();
        let input = sine(441.0, 0.5, 8192);
        let out = run(&mut dist, &input, &params(1.0, 0.5, DistortionType::Hard, 0.0, 0.0), None);
        // Zero mix is the dry signal, delayed by the filters only
        let err = max_error(&round_trip(&input, true), &out);
        assert!(err < 1e-5, "zero mix should output dry signal: error {}", err);
    }

    #[test]
    fn test_distortion_output_gain() {
        let input = sine(441.0, 0.25, 8192);
        let level = |db: f32| {
            let mut dist = prepared();
            let out = run(&mut dist, &input, &params(0.0, 1.0, DistortionType::Soft, 1.0, db), None);
            rms(&out[4096..])
        };
        let ratio = level(6.0) / level(0.0);
        assert!((ratio - 1.995).abs() < 0.02, "+6 dB gives {}x", ratio);
    }

    #[test]
    fn test_all_distortion_types() {
        let input = sine(441.0, 0.8, 4096);
        let results: Vec<Vec<f32>> = (0..5)
            .map(|ty| {
                let mut dist = prepared();
                let mut p = params(0.7, 1.0, DistortionType::from_index(ty), 1.0, 0.0);
                p[Distortion::PARAM_RATE] = 4000.0;
                run(&mut dist, &input, &p, None)
            })
            .collect();
        for i in 0..results.len() {
            for j in (i + 1)..results.len() {
                let diff = (2048..4096).map(|k| (results[i][k] - results[j][k]).abs()).fold(0.0f32, f32::max);
                assert!(diff > 0.01, "types {} and {} should differ ({})", i, j, diff);
            }
        }
    }

    #[test]
    fn test_type_switch_is_clean() {
        // Switching curves mid-signal must not spike
        let input = sine(441.0, 0.8, 4096);
        let mut dist = prepared();
        let mut out = run(&mut dist, &input[..2048], &params(0.5, 1.0, DistortionType::Soft, 1.0, 0.0), None);
        out.extend(run(&mut dist, &input[2048..], &params(0.5, 1.0, DistortionType::Fold, 1.0, 0.0), None));
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.2), "peak {}", out.iter().fold(0.0f32, |m, s| m.max(s.abs())));
    }

    #[test]
    fn test_distortion_reset() {
        let mut dist = prepared();
        run(&mut dist, &sine(441.0, 0.8, 1024), &params(0.8, 0.2, DistortionType::Tube, 1.0, 0.0), None);
        dist.reset();
        let out = run(&mut dist, &[0.0; 256], &params(0.8, 0.2, DistortionType::Tube, 1.0, 0.0), None);
        assert!(out.iter().all(|s| s.abs() < 1e-6), "output should be silent after reset");
    }

    #[test]
    fn test_distortion_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Distortion>();
    }

    #[test]
    fn test_distortion_default() {
        let dist = Distortion::default();
        assert_eq!(dist.info().id, "fx.distortion");
    }

    #[test]
    fn test_db_to_linear() {
        assert!((Distortion::db_to_linear(0.0) - 1.0).abs() < 0.001);
        assert!((Distortion::db_to_linear(6.0) - 2.0).abs() < 0.1);
        assert!((Distortion::db_to_linear(-6.0) - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_distortion_type_from_index() {
        assert_eq!(DistortionType::from_index(0), DistortionType::Soft);
        assert_eq!(DistortionType::from_index(1), DistortionType::Hard);
        assert_eq!(DistortionType::from_index(2), DistortionType::Fold);
        assert_eq!(DistortionType::from_index(3), DistortionType::Bit);
        assert_eq!(DistortionType::from_index(4), DistortionType::Tube);
        assert_eq!(DistortionType::from_index(99), DistortionType::Soft); // Default
    }

    #[test]
    fn test_distortion_drive_cv_modulation() {
        let input = sine(441.0, 0.5, 4096);
        let p = params(0.3, 1.0, DistortionType::Soft, 1.0, 0.0);
        let mut dist = prepared();
        let with_cv = run(&mut dist, &input, &p, Some(0.5));
        let mut dist = prepared();
        let without_cv = run(&mut dist, &input, &p, None);
        assert!(
            rms(&with_cv[2048..]) > rms(&without_cv[2048..]) + 0.02,
            "Drive CV should push harder: with {} without {}",
            rms(&with_cv[2048..]),
            rms(&without_cv[2048..])
        );
    }

    #[test]
    fn test_distortion_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Distortion>();
        assert!(registry.contains("fx.distortion"));

        let module = registry.create("fx.distortion").unwrap();
        assert_eq!(module.info().id, "fx.distortion");
        assert_eq!(module.info().name, "Distortion");
        assert_eq!(module.ports().len(), 3);
        assert_eq!(module.parameters().len(), 7);
    }

    #[test]
    fn test_transfer_is_the_curve_the_sound_takes() {
        // A sine comes out as loud as the drawn curve makes it (RMS, which the
        // oversampling filters' ripple on a clipped wave doesn't move)
        for ty in [DistortionType::Soft, DistortionType::Hard, DistortionType::Fold, DistortionType::Tube] {
            for (drive, amplitude) in [(0.2, 0.5), (0.6, 0.8)] {
                let mut dist = prepared();
                let input = sine(200.0, amplitude, 44100);
                let p = [drive, 1.0, ty as usize as f32, 1.0, 0.0, 0.0, Distortion::RATE_MAX_HZ];
                let out = run(&mut dist, &input, &p, None);
                // Less the offset Tube's lean leaves, which the DC blocker takes out
                let drawn: Vec<f32> = input[22050..].iter().map(|&x| Distortion::transfer(ty, drive, 0.0, 1.0, 0.0, x)).collect();
                let mean = drawn.iter().sum::<f32>() / drawn.len() as f32;
                let drawn: Vec<f32> = drawn.iter().map(|y| y - mean).collect();
                let (heard, expected) = (rms(&out[22050..]), rms(&drawn));
                assert!((heard / expected - 1.0).abs() < 0.04, "{ty:?} drive {drive}: {heard} vs {expected}");
            }
        }
    }

    #[test]
    fn test_transfer_knows_mix_output_and_bits() {
        let soft = |x| Distortion::transfer(DistortionType::Soft, 0.0, 0.0, 1.0, 0.0, x);
        assert!((soft(0.5) - 0.5f32.tanh()).abs() < 1e-5);
        // Dry passes untouched, and Output scales it
        assert!((Distortion::transfer(DistortionType::Hard, 1.0, 0.0, 0.0, 0.0, 0.3) - 0.3).abs() < 1e-6);
        assert!((Distortion::transfer(DistortionType::Hard, 1.0, 0.0, 0.0, 6.0, 0.3) - 0.3 * 1.9953).abs() < 1e-3);
        // Full drive crushes to 2 bits: steps a quarter apart
        assert_eq!(Distortion::bits(1.0), 2.0);
        assert_eq!(Distortion::transfer(DistortionType::Bit, 1.0, 0.0, 1.0, 0.0, 0.3), 0.25);
    }

    #[test]
    fn test_readout_follows_the_input() {
        let mut dist = prepared();
        let p = params(0.7, 0.5, DistortionType::Soft, 1.0, 0.0);
        run(&mut dist, &sine(200.0, 0.6, 22050), &p, None);
        let r = dist.readout(&p).unwrap().values;
        assert!((r[Distortion::READOUT_INPUT_PEAK] - 0.6).abs() < 0.05, "{:?}", &r[..2]);
        assert!((r[Distortion::READOUT_DRIVE] - 0.7).abs() < 1e-3);
        run(&mut dist, &vec![0.0; 44100], &p, None);
        assert!(dist.readout(&p).unwrap().values[Distortion::READOUT_INPUT_PEAK] < 0.01);
    }
}
