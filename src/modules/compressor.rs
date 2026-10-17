//! Dynamics Compressor effect module.
//!
//! A feed-forward compressor: a level detector (peak or RMS) feeds a
//! soft-knee gain computer, and the gain reduction it asks for is smoothed
//! in dB by the attack and release times. Smoothing the reduction rather
//! than the level keeps the curve honest: a steady tone above threshold
//! comes out where the ratio says, whatever the attack and release.
//! Includes optional sidechain input and gain reduction output for metering.

use std::f32::consts::LN_10;

use crate::dsp::{
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    context::ProcessContext,
    denormal::flush,
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    connected_input, ParameterDisplay, SignalType,
};

/// `20·log10(x) == DB_PER_NEPER · ln(x)`: one `ln` instead of a `log10`.
const DB_PER_NEPER: f32 = 20.0 / LN_10;

/// `10^(db/20) == exp(db · NEPER_PER_DB)`: one `exp` instead of a `powf`.
const NEPER_PER_DB: f32 = LN_10 / 20.0;

/// Averaging time of the RMS detector. Long enough to hold steady on a
/// 50 Hz bass note, short enough to catch a drum.
const RMS_WINDOW_MS: f32 = 20.0;

/// Gain reduction below this many dB is called none, so a compressor
/// at rest does no transcendental math at all.
const GR_FLOOR_DB: f32 = 1e-5;

/// How fast the level the display's dot rides falls once the sound has
/// passed, so it glides down the curve instead of blinking off.
const DETECTOR_VIEW_FALL_MS: f32 = 120.0;

/// One-pole coefficient for a time constant in milliseconds.
#[inline]
fn time_coeff(ms: f32, sample_rate: f32) -> f32 {
    (-1.0 / (ms * 0.001 * sample_rate)).exp()
}

/// A one-pole coefficient cached against the time it was computed for,
/// so the `exp` runs only while the knob is moving.
struct CachedCoeff {
    ms: f32,
    coeff: f32,
}

impl CachedCoeff {
    fn new() -> Self {
        // NaN never equals a real time, so the first lookup computes
        Self { ms: f32::NAN, coeff: 0.0 }
    }

    #[inline]
    fn get(&mut self, ms: f32, sample_rate: f32) -> f32 {
        if ms != self.ms {
            self.ms = ms;
            self.coeff = time_coeff(ms, sample_rate);
        }
        self.coeff
    }

    fn invalidate(&mut self) {
        self.ms = f32::NAN;
    }
}

/// The module's ID, as patches name it.
pub const ID: &str = "fx.compressor";

/// Dynamics compressor with sidechain support and gain reduction output.
///
/// # Ports
///
/// - **In** (Audio, Input): Main audio input signal.
/// - **Sidechain** (Audio, Input): External sidechain input (optional, normalled from In).
/// - **Out** (Audio, Output): Compressed audio output.
/// - **GR** (Control, Output): Gain reduction amount (0 to 1, for metering).
///
/// # Parameters
///
/// - **Threshold** (-60dB to 0dB): Level where compression starts.
/// - **Ratio** (1:1 to 20:1): Amount of compression.
/// - **Attack** (0.1ms to 100ms): How fast compression engages.
/// - **Release** (10ms to 1000ms): How fast compression releases.
/// - **Knee** (0dB to 12dB): Soft/hard knee width.
/// - **Makeup** (0dB to +24dB): Output level boost.
/// - **Mix** (0% to 100%): Parallel compression blend.
/// - **Detector** (Peak/RMS): What the level detector measures.
pub struct Compressor {
    /// Sample rate.
    sample_rate: f32,
    /// Running mean square of the sidechain (RMS detector).
    mean_square: f32,
    /// One-pole coefficient of the RMS averaging window.
    rms_coeff: f32,
    /// Gain reduction held at its peaks and let go at the release rate (dB).
    gr_held: f32,
    /// Gain reduction applied, the held value smoothed at the attack rate (dB).
    gr_db: f32,
    /// The level the detector has heard lately (linear), for the display:
    /// its peaks held and let fall at `DETECTOR_VIEW_FALL_MS`.
    detector_view: f32,
    /// Attack and release coefficients, recomputed only when their knob moves.
    attack_coeff: CachedCoeff,
    release_coeff: CachedCoeff,
    /// Linear sidechain level where the knee begins, cached against the
    /// threshold and knee it came from; below it no `ln` is needed.
    knee_start: f32,
    knee_start_for: (f32, f32),
    /// Smoothed threshold parameter.
    threshold_smooth: SmoothedValue,
    /// Smoothed ratio parameter.
    ratio_smooth: SmoothedValue,
    /// Smoothed attack parameter.
    attack_smooth: SmoothedValue,
    /// Smoothed release parameter.
    release_smooth: SmoothedValue,
    /// Smoothed knee parameter.
    knee_smooth: SmoothedValue,
    /// Smoothed makeup gain parameter.
    makeup_smooth: SmoothedValue,
    /// Smoothed mix parameter.
    mix_smooth: SmoothedValue,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl Compressor {
    /// Creates a new dynamics compressor.
    pub fn new() -> Self {
        let sample_rate = 44100.0;

        Self {
            sample_rate,
            mean_square: 0.0,
            rms_coeff: time_coeff(RMS_WINDOW_MS, sample_rate),
            gr_held: 0.0,
            gr_db: 0.0,
            detector_view: 0.0,
            attack_coeff: CachedCoeff::new(),
            release_coeff: CachedCoeff::new(),
            knee_start: 0.0,
            knee_start_for: (f32::NAN, f32::NAN),
            threshold_smooth: SmoothedValue::with_default_smoothing(-20.0, sample_rate),
            ratio_smooth: SmoothedValue::with_default_smoothing(4.0, sample_rate),
            attack_smooth: SmoothedValue::with_default_smoothing(10.0, sample_rate),
            release_smooth: SmoothedValue::with_default_smoothing(100.0, sample_rate),
            knee_smooth: SmoothedValue::with_default_smoothing(6.0, sample_rate),
            makeup_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            mix_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in", "In", SignalType::Audio, 0.0).describe("Audio to compress"),
                PortDefinition::input_with_default("sidechain", "Sidechain", SignalType::Audio, 0.0).describe("Audio that triggers compression; uses In when unpatched"),
                // Output ports
                PortDefinition::output("out", "Out", SignalType::Audio).describe("Compressed audio"),
                PortDefinition::output("gr", "GR", SignalType::Control).describe("Gain reduction as CV, for ducking other modules"),
            ],
            parameters: vec![
                ParameterDefinition::new(
                    "threshold",
                    "Threshold",
                    -60.0,
                    0.0,
                    -20.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Level above which compression starts, in dB"),
                ParameterDefinition::new(
                    "ratio",
                    "Ratio",
                    1.0,
                    20.0,
                    4.0,
                    ParameterDisplay::Logarithmic { unit: ":1" },
                ).describe("How strongly levels over the threshold are reduced"),
                ParameterDefinition::new(
                    "attack",
                    "Attack",
                    0.1,
                    100.0,
                    10.0,
                    ParameterDisplay::Logarithmic { unit: "ms" },
                ).describe("How fast compression clamps down, in ms"),
                ParameterDefinition::new(
                    "release",
                    "Release",
                    10.0,
                    1000.0,
                    100.0,
                    ParameterDisplay::Logarithmic { unit: "ms" },
                ).describe("How fast compression lets go, in ms"),
                ParameterDefinition::new(
                    "knee",
                    "Knee",
                    0.0,
                    12.0,
                    6.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Softens the onset of compression around the threshold, in dB"),
                ParameterDefinition::new(
                    "makeup",
                    "Makeup",
                    0.0,
                    24.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Gain added after compression to restore loudness, in dB"),
                ParameterDefinition::normalized("mix", "Mix", 1.0).describe("Blend from dry (0) to compressed (1), for parallel compression"),
                // Appended, so patches saved before it existed load as Peak,
                // which is how they always sounded
                ParameterDefinition::choice("detector", "Detector", &["Peak", "RMS"], 0).describe("Level sensing: Peak reacts to spikes, RMS follows average loudness"),
            ],
        }
    }

    /// Port index constants.
    const PORT_IN: usize = 0;
    const PORT_SIDECHAIN: usize = 1;
    const PORT_OUT: usize = 0;
    const PORT_GR: usize = 1;

    /// Parameter index constants.
    const PARAM_THRESHOLD: usize = 0;
    const PARAM_RATIO: usize = 1;
    const PARAM_ATTACK: usize = 2;
    const PARAM_RELEASE: usize = 3;
    const PARAM_KNEE: usize = 4;
    const PARAM_MAKEUP: usize = 5;
    const PARAM_MIX: usize = 6;
    const PARAM_DETECTOR: usize = 7;

    /// Detector choices.
    pub const DETECTOR_RMS: f32 = 1.0;

    /// Readout slots: the level the detector hears (dB), and the gain
    /// reduction being applied (dB, 0 or more).
    pub const READOUT_DETECTOR_DB: usize = 0;
    pub const READOUT_GR_DB: usize = 1;

    /// Compute gain reduction in dB for a given input level in dB.
    /// Uses soft knee algorithm for smooth transition into compression.
    #[inline]
    pub fn compute_gain_reduction(level_db: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
        // Distance above threshold
        let over_threshold = level_db - threshold;

        if knee <= 0.0 {
            // Hard knee
            if over_threshold <= 0.0 {
                0.0 // Below threshold, no compression
            } else {
                // Above threshold: reduce by (1 - 1/ratio) * overshoot
                over_threshold * (1.0 - 1.0 / ratio)
            }
        } else {
            // Soft knee
            let half_knee = knee / 2.0;

            if over_threshold <= -half_knee {
                // Below knee region, no compression
                0.0
            } else if over_threshold >= half_knee {
                // Above knee region, full compression
                over_threshold * (1.0 - 1.0 / ratio)
            } else {
                // In knee region - smooth curve
                // Quadratic interpolation through the knee
                let knee_factor = over_threshold + half_knee;
                (1.0 - 1.0 / ratio) * knee_factor * knee_factor / (2.0 * knee)
            }
        }
    }

    /// How much louder (or quieter) the output is than the input, in dB,
    /// with `gr_db` of reduction applied: the makeup, and the dry signal
    /// blended back in by `mix`, as `process` does it.
    pub fn output_gain_db(gr_db: f32, makeup_db: f32, mix: f32) -> f32 {
        let mix = mix.clamp(0.0, 1.0);
        let gain = (1.0 - mix) + mix * ((makeup_db - gr_db) * NEPER_PER_DB).exp();
        DB_PER_NEPER * gain.max(1e-6).ln()
    }

    /// Linear sidechain level where the knee begins, recomputed only when
    /// the threshold or knee has moved.
    #[inline]
    fn knee_start(&mut self, threshold_db: f32, knee_db: f32) -> f32 {
        if (threshold_db, knee_db) != self.knee_start_for {
            self.knee_start_for = (threshold_db, knee_db);
            self.knee_start = ((threshold_db - 0.5 * knee_db) * NEPER_PER_DB).exp();
        }
        self.knee_start
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Compressor {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Compressor",
            category: ModuleCategory::Effect,
            description: "Dynamics compressor with sidechain and gain reduction output",
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
        self.rms_coeff = time_coeff(RMS_WINDOW_MS, sample_rate);
        self.attack_coeff.invalidate();
        self.release_coeff.invalidate();

        // Update sample rate for smoothed values
        self.threshold_smooth.set_sample_rate(sample_rate);
        self.ratio_smooth.set_sample_rate(sample_rate);
        self.attack_smooth.set_sample_rate(sample_rate);
        self.release_smooth.set_sample_rate(sample_rate);
        self.knee_smooth.set_sample_rate(sample_rate);
        self.makeup_smooth.set_sample_rate(sample_rate);
        self.mix_smooth.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Set smoothing targets
        self.threshold_smooth.set_target(params[Self::PARAM_THRESHOLD]);
        self.ratio_smooth.set_target(params[Self::PARAM_RATIO]);
        self.attack_smooth.set_target(params[Self::PARAM_ATTACK]);
        self.release_smooth.set_target(params[Self::PARAM_RELEASE]);
        self.knee_smooth.set_target(params[Self::PARAM_KNEE]);
        self.makeup_smooth.set_target(params[Self::PARAM_MAKEUP]);
        self.mix_smooth.set_target(params[Self::PARAM_MIX]);
        let rms = params.get(Self::PARAM_DETECTOR).copied().unwrap_or(0.0) == Self::DETECTOR_RMS;

        // Get input buffers
        let input = inputs.get(Self::PORT_IN);
        // The detector follows the sidechain when connected, otherwise the input
        let sidechain = connected_input(inputs, Self::PORT_SIDECHAIN);

        // Split outputs
        let (out_slice, gr_slice) = outputs.split_at_mut(Self::PORT_GR);
        let out = &mut out_slice[Self::PORT_OUT];
        let gr_out = &mut gr_slice[0];

        // The loudest the detector hears in this block, for the display
        let mut heard = 0.0f32;

        // Process each sample
        for i in 0..context.block_size {
            // Get smoothed values
            let threshold_db = self.threshold_smooth.next();
            let ratio_val = self.ratio_smooth.next().max(1.0);
            let attack_ms = self.attack_smooth.next().max(0.1);
            let release_ms = self.release_smooth.next().max(10.0);
            let knee_db = self.knee_smooth.next().max(0.0);
            let makeup_db = self.makeup_smooth.next();
            let mix_val = self.mix_smooth.next().clamp(0.0, 1.0);

            // Get input sample
            let dry = input
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);

            // Get sidechain sample (normalled from input if not connected)
            let sidechain_sample = sidechain
                .map(|buf| buf.at(i))
                .unwrap_or(dry);

            // Detect the level, and only take its log once it reaches the
            // knee: below that the curve asks for no reduction anyway
            let knee_start = self.knee_start(threshold_db, knee_db);
            let target_gr = if rms {
                self.mean_square = flush(
                    self.rms_coeff * self.mean_square
                        + (1.0 - self.rms_coeff) * sidechain_sample * sidechain_sample,
                );
                heard = heard.max(self.mean_square);
                if self.mean_square > knee_start * knee_start {
                    // 20·log10(sqrt(ms)) without the sqrt
                    let level_db = 0.5 * DB_PER_NEPER * self.mean_square.ln();
                    Self::compute_gain_reduction(level_db, threshold_db, ratio_val, knee_db)
                } else {
                    0.0
                }
            } else {
                let level = sidechain_sample.abs();
                heard = heard.max(level);
                if level > knee_start {
                    let level_db = DB_PER_NEPER * level.ln();
                    Self::compute_gain_reduction(level_db, threshold_db, ratio_val, knee_db)
                } else {
                    0.0
                }
            };

            // Ballistics in dB. The held value jumps to every peak of the
            // reduction asked for and lets go at the release rate; the
            // applied value follows it at the attack rate. Between the
            // peaks of a steady tone the hold barely moves, so the tone is
            // reduced by what its level calls for.
            let release = self.release_coeff.get(release_ms, self.sample_rate);
            let attack = self.attack_coeff.get(attack_ms, self.sample_rate);
            self.gr_held = target_gr.max(release * self.gr_held + (1.0 - release) * target_gr);
            self.gr_db = attack * self.gr_db + (1.0 - attack) * self.gr_held;
            if self.gr_db < GR_FLOOR_DB && self.gr_held < GR_FLOOR_DB {
                self.gr_db = 0.0;
                self.gr_held = 0.0;
            }

            // Reduction and makeup together, in one exp (none at unity)
            let gain_db = makeup_db - self.gr_db;
            let gain = if gain_db == 0.0 { 1.0 } else { (gain_db * NEPER_PER_DB).exp() };

            // Mix dry and compressed (parallel compression)
            out.samples[i] = dry * (1.0 - mix_val) + dry * gain * mix_val;

            // Output gain reduction as control signal (0 = no GR, 1 = max GR)
            // Normalize GR to 0-1 range: 0dB GR = 0, 60dB GR = 1
            gr_out.samples[i] = (self.gr_db / 60.0).clamp(0.0, 1.0);
        }

        let heard = if rms { heard.sqrt() } else { heard };
        let fall = (-(context.block_size as f32) / (DETECTOR_VIEW_FALL_MS * 0.001 * self.sample_rate)).exp();
        self.detector_view = flush(heard.max(self.detector_view * fall));
    }

    fn key_inputs(&self) -> &'static [usize] {
        &[Self::PORT_SIDECHAIN]
    }

    fn reset(&mut self) {
        self.mean_square = 0.0;
        self.gr_held = 0.0;
        self.gr_db = 0.0;
        self.detector_view = 0.0;

        // Reset smoothed values
        self.threshold_smooth.reset(self.threshold_smooth.target());
        self.ratio_smooth.reset(self.ratio_smooth.target());
        self.attack_smooth.reset(self.attack_smooth.target());
        self.release_smooth.reset(self.release_smooth.target());
        self.knee_smooth.reset(self.knee_smooth.target());
        self.makeup_smooth.reset(self.makeup_smooth.target());
        self.mix_smooth.reset(self.mix_smooth.target());
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_DETECTOR_DB] = DB_PER_NEPER * self.detector_view.max(1e-6).ln();
        readout.values[Self::READOUT_GR_DB] = self.gr_db;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compressor_info() {
        let compressor = Compressor::new();
        assert_eq!(compressor.info().id, "fx.compressor");
        assert_eq!(compressor.info().name, "Compressor");
        assert_eq!(compressor.info().category, ModuleCategory::Effect);
    }

    #[test]
    fn test_compressor_ports() {
        let compressor = Compressor::new();
        let ports = compressor.ports();

        assert_eq!(ports.len(), 4);

        // Input ports
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "sidechain");
        assert_eq!(ports[1].signal_type, SignalType::Audio);

        // Output ports
        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "out");
        assert_eq!(ports[2].signal_type, SignalType::Audio);

        assert!(ports[3].is_output());
        assert_eq!(ports[3].id, "gr");
        assert_eq!(ports[3].signal_type, SignalType::Control);
    }

    #[test]
    fn test_compressor_parameters() {
        let compressor = Compressor::new();
        let params = compressor.parameters();

        assert_eq!(params.len(), 8);
        assert_eq!(params[0].id, "threshold");
        assert_eq!(params[1].id, "ratio");
        assert_eq!(params[2].id, "attack");
        assert_eq!(params[3].id, "release");
        assert_eq!(params[4].id, "knee");
        assert_eq!(params[5].id, "makeup");
        assert_eq!(params[6].id, "mix");
        assert_eq!(params[7].id, "detector");
    }

    #[test]
    fn test_gain_reduction_below_threshold() {
        // Signal below threshold should have no gain reduction
        let gr = Compressor::compute_gain_reduction(-30.0, -20.0, 4.0, 0.0);
        assert_eq!(gr, 0.0);
    }

    #[test]
    fn test_gain_reduction_above_threshold_hard_knee() {
        // Signal 10dB above threshold with 4:1 ratio
        // Should reduce by (1 - 1/4) * 10 = 7.5 dB
        let gr = Compressor::compute_gain_reduction(-10.0, -20.0, 4.0, 0.0);
        assert!((gr - 7.5).abs() < 0.01);
    }

    #[test]
    fn test_gain_reduction_soft_knee() {
        // In the soft knee region, gain reduction should be smooth
        let gr_at_threshold = Compressor::compute_gain_reduction(-20.0, -20.0, 4.0, 6.0);
        let gr_below = Compressor::compute_gain_reduction(-23.0, -20.0, 4.0, 6.0);
        let gr_above = Compressor::compute_gain_reduction(-17.0, -20.0, 4.0, 6.0);

        // At threshold, should have some reduction
        assert!(gr_at_threshold > 0.0);
        // Below knee start, should have no reduction
        assert_eq!(gr_below, 0.0);
        // Above knee end, should have full reduction
        assert!(gr_above > gr_at_threshold);
    }

    #[test]
    fn test_compressor_unity_gain_at_low_levels() {
        let mut compressor = Compressor::new();
        compressor.prepare(44100.0, 256);

        // Create a low-level input signal (should be below threshold)
        let mut input = SignalBuffer::audio(256);
        for sample in input.samples.iter_mut() {
            *sample = 0.01; // Very quiet
        }

        let sidechain = SignalBuffer::unconnected(256, SignalType::Audio); // Nothing plugged in
        let mut outputs = vec![
            SignalBuffer::audio(256), // Out
            SignalBuffer::control(256), // GR
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process with default settings (-20dB threshold)
        // Mix = 100% (1.0), makeup = 0dB
        compressor.process(
            &[&input, &sidechain],
            &mut outputs,
            &[-20.0, 4.0, 10.0, 100.0, 6.0, 0.0, 1.0],
            &ctx,
        );

        // At very low levels, output should be close to input (no compression)
        // Allow for some smoothing artifacts
        for i in 100..256 {
            let ratio = outputs[0].samples[i] / input.samples[i];
            assert!(
                (ratio - 1.0).abs() < 0.5,
                "Expected near-unity gain at low levels, got ratio {} at sample {}",
                ratio,
                i
            );
        }
    }

    #[test]
    fn test_compressor_reduces_loud_signals() {
        let mut compressor = Compressor::new();
        compressor.prepare(44100.0, 256);

        // Create a loud input signal (well above threshold)
        let mut input = SignalBuffer::audio(256);
        for sample in input.samples.iter_mut() {
            *sample = 0.9; // Loud signal
        }

        let sidechain = SignalBuffer::unconnected(256, SignalType::Audio); // Nothing plugged in
        let mut outputs = vec![
            SignalBuffer::audio(256),
            SignalBuffer::control(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process multiple blocks to let envelope settle
        for _ in 0..10 {
            compressor.process(
                &[&input, &sidechain],
                &mut outputs,
                &[-20.0, 4.0, 0.1, 10.0, 0.0, 0.0, 1.0], // Fast attack, hard knee, no makeup
                &ctx,
            );
        }

        // After compression, output should be reduced
        let last_output = outputs[0].samples[255].abs();
        let last_input = input.samples[255].abs();
        assert!(
            last_output < last_input,
            "Expected compression to reduce signal, got output {} vs input {}",
            last_output,
            last_input
        );

        // Gain reduction output should show some compression
        assert!(
            outputs[1].samples[255] > 0.0,
            "Expected gain reduction output, got {}",
            outputs[1].samples[255]
        );
    }

    #[test]
    fn test_compressor_makeup_gain() {
        let mut compressor = Compressor::new();
        compressor.prepare(44100.0, 256);

        // Create a loud signal
        let mut input = SignalBuffer::audio(256);
        for sample in input.samples.iter_mut() {
            *sample = 0.5;
        }

        let sidechain = SignalBuffer::unconnected(256, SignalType::Audio); // Nothing plugged in

        // Process without makeup
        let mut outputs_no_makeup = vec![
            SignalBuffer::audio(256),
            SignalBuffer::control(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        for _ in 0..5 {
            compressor.process(
                &[&input, &sidechain],
                &mut outputs_no_makeup,
                &[-20.0, 4.0, 0.1, 10.0, 0.0, 0.0, 1.0], // No makeup
                &ctx,
            );
        }

        compressor.reset();

        // Process with 12dB makeup
        let mut outputs_with_makeup = vec![
            SignalBuffer::audio(256),
            SignalBuffer::control(256),
        ];

        for _ in 0..5 {
            compressor.process(
                &[&input, &sidechain],
                &mut outputs_with_makeup,
                &[-20.0, 4.0, 0.1, 10.0, 0.0, 12.0, 1.0], // 12dB makeup
                &ctx,
            );
        }

        // Output with makeup should be louder
        let level_no_makeup = outputs_no_makeup[0].samples[255].abs();
        let level_with_makeup = outputs_with_makeup[0].samples[255].abs();
        assert!(
            level_with_makeup > level_no_makeup,
            "Expected makeup gain to increase output"
        );
    }

    #[test]
    fn test_compressor_parallel_compression() {
        let mut compressor = Compressor::new();
        compressor.prepare(44100.0, 256);

        let mut input = SignalBuffer::audio(256);
        for sample in input.samples.iter_mut() {
            *sample = 0.8;
        }

        let sidechain = SignalBuffer::unconnected(256, SignalType::Audio); // Nothing plugged in
        let ctx = ProcessContext::new(44100.0, 256);

        // 100% wet
        let mut outputs_wet = vec![
            SignalBuffer::audio(256),
            SignalBuffer::control(256),
        ];

        for _ in 0..5 {
            compressor.process(
                &[&input, &sidechain],
                &mut outputs_wet,
                &[-20.0, 8.0, 0.1, 10.0, 0.0, 0.0, 1.0], // 100% wet
                &ctx,
            );
        }

        compressor.reset();

        // 50% wet (parallel)
        let mut outputs_parallel = vec![
            SignalBuffer::audio(256),
            SignalBuffer::control(256),
        ];

        for _ in 0..5 {
            compressor.process(
                &[&input, &sidechain],
                &mut outputs_parallel,
                &[-20.0, 8.0, 0.1, 10.0, 0.0, 0.0, 0.5], // 50% wet
                &ctx,
            );
        }

        // Parallel should be between dry and fully compressed
        let wet_level = outputs_wet[0].samples[255].abs();
        let parallel_level = outputs_parallel[0].samples[255].abs();
        let dry_level = input.samples[255].abs();

        // Parallel mix should be louder than full compression (due to dry signal)
        assert!(
            parallel_level > wet_level,
            "Parallel mix {} should be louder than full wet {}",
            parallel_level,
            wet_level
        );
        // But quieter than dry (compression still applied to wet portion)
        assert!(
            parallel_level <= dry_level + 0.01,
            "Parallel mix {} shouldn't exceed dry {}",
            parallel_level,
            dry_level
        );
    }

    #[test]
    fn test_compressor_reset() {
        let mut compressor = Compressor::new();
        compressor.prepare(44100.0, 256);

        // Fill with signal to build up envelope
        let mut input = SignalBuffer::audio(256);
        input.fill(0.9);
        let sidechain = SignalBuffer::unconnected(256, SignalType::Audio); // Nothing plugged in
        let mut outputs = vec![
            SignalBuffer::audio(256),
            SignalBuffer::control(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        compressor.process(
            &[&input, &sidechain],
            &mut outputs,
            &[-20.0, 4.0, 0.1, 10.0, 0.0, 0.0, 1.0],
            &ctx,
        );

        // Reset
        compressor.reset();

        // Process silence
        let silence = SignalBuffer::audio(256);
        let mut outputs2 = vec![
            SignalBuffer::audio(256),
            SignalBuffer::control(256),
        ];

        compressor.process(
            &[&silence, &sidechain],
            &mut outputs2,
            &[-20.0, 4.0, 0.1, 10.0, 0.0, 0.0, 1.0],
            &ctx,
        );

        // Output should be near zero
        assert!(
            outputs2[0].samples[0].abs() < 0.01,
            "Expected near-zero output after reset"
        );
    }

    const PEAK: f32 = 0.0;
    const RMS: f32 = 1.0;

    /// Plays a steady 1 kHz sine of `amplitude` through the compressor for
    /// 1.5 s at 48 kHz. Returns the last 0.25 s of output and of GR.
    fn steady_tone(amplitude: f32, params: [f32; 8]) -> (Vec<f32>, Vec<f32>) {
        let sample_rate = 48000.0;
        let block = 256;
        let mut compressor = Compressor::new();
        compressor.prepare(sample_rate, block);
        let ctx = ProcessContext::new(sample_rate, block);
        let sidechain = SignalBuffer::unconnected(block, SignalType::Audio);
        let mut input = SignalBuffer::audio(block);
        let mut outputs = vec![SignalBuffer::audio(block), SignalBuffer::control(block)];
        let mut out = Vec::new();
        let mut gr = Vec::new();
        let mut n = 0usize;

        for _ in 0..(1.5 * sample_rate) as usize / block {
            for s in input.samples.iter_mut() {
                *s = amplitude * (std::f32::consts::TAU * 1000.0 * n as f32 / sample_rate).sin();
                n += 1;
            }
            compressor.process(&[&input, &sidechain], &mut outputs, &params, &ctx);
            out.extend_from_slice(&outputs[0].samples);
            gr.extend_from_slice(&outputs[1].samples);
        }
        let keep = out.len() - 12000;
        (out.split_off(keep), gr.split_off(keep))
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    #[test]
    fn test_steady_tone_is_reduced_by_the_ratio() {
        // A 0.5 sine: -6.0 dB at its peaks, -9.0 dB RMS. Each detector
        // measures its own kind of level, and the output's level of that
        // kind lands where the curve says, whatever the attack and release.
        let amplitude = 0.5f32;
        for detector in [PEAK, RMS] {
            for (threshold, ratio, knee) in [(-30.0, 2.0, 0.0), (-30.0, 4.0, 0.0), (-24.0, 10.0, 6.0), (-40.0, 20.0, 12.0)] {
                for (attack, release) in [(0.1, 50.0), (10.0, 100.0), (30.0, 400.0)] {
                    let params = [threshold, ratio, attack, release, knee, 0.0, 1.0, detector];
                    let (out, gr) = steady_tone(amplitude, params);

                    let (level_in, level_out) = if detector == RMS {
                        let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
                        (db(amplitude / 2f32.sqrt()), db(rms(&out)))
                    } else {
                        (db(amplitude), db(out.iter().fold(0.0f32, |m, s| m.max(s.abs()))))
                    };
                    let expected = threshold + (level_in - threshold) / ratio;
                    let label = format!(
                        "{} T{} {}:1 knee {} A{} R{}",
                        if detector == RMS { "RMS" } else { "Peak" },
                        threshold,
                        ratio,
                        knee,
                        attack,
                        release
                    );
                    assert!(
                        (level_out - expected).abs() < 0.1,
                        "{}: out {:.2} dB, expected {:.2} dB",
                        label,
                        level_out,
                        expected
                    );

                    // GR reports the same reduction, steadily
                    let gr_db = gr.iter().sum::<f32>() / gr.len() as f32 * 60.0;
                    let expected_gr = (level_in - threshold) * (1.0 - 1.0 / ratio);
                    assert!(
                        (gr_db - expected_gr).abs() < 0.1,
                        "{}: GR {:.2} dB, expected {:.2} dB",
                        label,
                        gr_db,
                        expected_gr
                    );
                }
            }
        }
    }

    #[test]
    fn test_rms_detector_hears_a_sine_3db_quieter() {
        // The same tone and settings: RMS sees -9 dB where Peak sees -6 dB,
        // so at 4:1 it asks for 3 * 3/4 = 2.25 dB less reduction
        let params = |detector| [-30.0, 4.0, 10.0, 100.0, 0.0, 0.0, 1.0, detector];
        let mean = |x: &[f32]| x.iter().sum::<f32>() / x.len() as f32 * 60.0;
        let gr_peak = mean(&steady_tone(0.5, params(PEAK)).1);
        let gr_rms = mean(&steady_tone(0.5, params(RMS)).1);
        assert!((gr_peak - gr_rms - 2.25).abs() < 0.1, "peak {} dB vs rms {} dB", gr_peak, gr_rms);
    }

    #[test]
    fn test_below_the_knee_is_untouched() {
        // A tone under threshold - knee/2 passes bit for bit
        for detector in [PEAK, RMS] {
            let (out, gr) = steady_tone(0.05, [-20.0, 4.0, 10.0, 100.0, 6.0, 0.0, 1.0, detector]);
            let (reference, _) = steady_tone(0.05, [-20.0, 1.0, 10.0, 100.0, 0.0, 0.0, 0.0, detector]);
            assert_eq!(out, reference);
            assert!(gr.iter().all(|&g| g == 0.0));
        }
    }

    #[test]
    fn test_gain_reduction_lets_go_after_the_tone() {
        let mut compressor = Compressor::new();
        compressor.prepare(48000.0, 256);
        let ctx = ProcessContext::new(48000.0, 256);
        let sidechain = SignalBuffer::unconnected(256, SignalType::Audio);
        let mut loud = SignalBuffer::audio(256);
        loud.fill(0.9);
        let mut quiet = SignalBuffer::audio(256);
        quiet.fill(0.01);
        let mut outputs = vec![SignalBuffer::audio(256), SignalBuffer::control(256)];
        let params = [-20.0, 4.0, 1.0, 50.0, 0.0, 0.0, 1.0, PEAK];

        for _ in 0..50 {
            compressor.process(&[&loud, &sidechain], &mut outputs, &params, &ctx);
        }
        assert!(outputs[1].samples[255] > 0.2);

        // 2 s of quiet: well past the release, back to exactly unity
        for _ in 0..375 {
            compressor.process(&[&quiet, &sidechain], &mut outputs, &params, &ctx);
        }
        assert_eq!(outputs[1].samples[255], 0.0);
        assert_eq!(outputs[0].samples[255], 0.01);
    }

    #[test]
    fn test_compressor_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Compressor>();
    }

    #[test]
    fn test_compressor_default() {
        let compressor = Compressor::default();
        assert_eq!(compressor.info().id, "fx.compressor");
    }

    #[test]
    fn test_compressor_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Compressor>();

        assert!(registry.contains("fx.compressor"));

        let module = registry.create("fx.compressor");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "fx.compressor");
        assert_eq!(module.info().name, "Compressor");
        assert_eq!(module.ports().len(), 4);
        assert_eq!(module.parameters().len(), 8);
    }

    /// Runs `blocks` blocks of a 1 kHz sine of `amplitude` (0 for silence),
    /// and returns the readout after them.
    fn readout_after(compressor: &mut Compressor, amplitude: f32, blocks: usize, params: &[f32; 8]) -> Readout {
        let (sample_rate, block) = (48000.0, 256);
        let ctx = ProcessContext::new(sample_rate, block);
        let sidechain = SignalBuffer::unconnected(block, SignalType::Audio);
        let mut input = SignalBuffer::audio(block);
        let mut outputs = vec![SignalBuffer::audio(block), SignalBuffer::control(block)];
        for b in 0..blocks {
            for (i, s) in input.samples.iter_mut().enumerate() {
                let n = (b * block + i) as f32;
                *s = amplitude * (std::f32::consts::TAU * 1000.0 * n / sample_rate).sin();
            }
            compressor.process(&[&input, &sidechain], &mut outputs, params, &ctx);
        }
        compressor.readout(params).unwrap()
    }

    #[test]
    fn test_readout_shows_what_the_detector_hears_and_the_reduction() {
        let mut compressor = Compressor::new();
        compressor.prepare(48000.0, 256);
        let params = [-30.0, 4.0, 1.0, 100.0, 0.0, 0.0, 1.0, PEAK];

        // A 0.5 sine peaks at -6 dB: 24 dB over, 18 dB of it taken away
        let loud = readout_after(&mut compressor, 0.5, 200, &params);
        let heard = loud.values[Compressor::READOUT_DETECTOR_DB];
        let reduction = loud.values[Compressor::READOUT_GR_DB];
        assert!((heard - db(0.5)).abs() < 0.5, "detector heard {heard} dB");
        assert!((reduction - 18.0).abs() < 1.0, "reduced by {reduction} dB");

        // Silence lets both go
        let quiet = readout_after(&mut compressor, 0.0, 200, &params);
        assert!(quiet.values[Compressor::READOUT_DETECTOR_DB] < -60.0);
        assert!(quiet.values[Compressor::READOUT_GR_DB] < 0.1);
    }

    #[test]
    fn test_output_gain_follows_makeup_and_mix() {
        assert!(Compressor::output_gain_db(0.0, 0.0, 1.0).abs() < 1e-4);
        assert!((Compressor::output_gain_db(6.0, 0.0, 1.0) + 6.0).abs() < 1e-3);
        assert!((Compressor::output_gain_db(6.0, 6.0, 1.0)).abs() < 1e-3);
        // Fully dry, nothing the compressor does is heard
        assert!(Compressor::output_gain_db(20.0, 0.0, 0.0).abs() < 1e-4);
        // Half wet: the dry half keeps it from falling far
        let half = Compressor::output_gain_db(40.0, 0.0, 0.5);
        assert!((half - db(0.5 + 0.5 * 0.01)).abs() < 1e-3, "{half}");
    }
}
