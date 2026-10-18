//! 3-Band Parametric EQ module.
//!
//! A versatile equalizer with low shelf, mid parametric, and high shelf bands.
//! Uses biquad filters with RBJ Audio EQ Cookbook formulas.

use std::f32::consts::PI;

use crate::dsp::{
    db_to_gain,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    context::ProcessContext,
    denormal::flush,
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// Samples between coefficient updates while a band's knobs glide. Three
/// biquads' worth of trig every sample was most of the EQ's cost; at 16 the
/// steps are 0.3 ms apart, far finer than the 10 ms smoothing they follow.
const COEFF_INTERVAL: usize = 16;

/// Biquad filter coefficients.
#[derive(Clone, Copy, Default)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoeffs {
    /// The filter's gain at `hz`, in dB: |H(e^jw)| from the coefficients.
    /// Written in φ = 4·sin²(w/2) and in f64, which stay accurate at low
    /// frequencies where the cosine form cancels itself away.
    fn magnitude_db(&self, hz: f32, sample_rate: f32) -> f32 {
        let (b0, b1, b2) = (self.b0 as f64, self.b1 as f64, self.b2 as f64);
        let (a1, a2) = (self.a1 as f64, self.a2 as f64);
        let phi = 4.0 * (std::f64::consts::PI * hz as f64 / sample_rate as f64).sin().powi(2);
        let num = (b0 + b1 + b2).powi(2) - phi * (b0 * b1 + 4.0 * b0 * b2 + b1 * b2) + phi * phi * b0 * b2;
        let den = (1.0 + a1 + a2).powi(2) - phi * (a1 + 4.0 * a2 + a1 * a2) + phi * phi * a2;
        (10.0 * (num.max(1e-18) / den.max(1e-18)).log10()) as f32
    }
}

/// Biquad filter state (delay line).
#[derive(Clone, Copy, Default)]
struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadState {
    /// Process a single sample through the biquad filter.
    #[inline]
    fn process(&mut self, input: f32, coeffs: &BiquadCoeffs) -> f32 {
        let output = flush(
            coeffs.b0 * input + coeffs.b1 * self.x1 + coeffs.b2 * self.x2
                - coeffs.a1 * self.y1
                - coeffs.a2 * self.y2,
        );

        // Update state
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;

        output
    }

    /// Reset the filter state.
    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// The module's ID, as patches name it.
pub const ID: &str = "fx.eq";

/// 3-Band Parametric EQ with low shelf, mid parametric, and high shelf.
///
/// # Ports
///
/// - **In** (Audio, Input): Audio signal to equalize.
/// - **Out** (Audio, Output): Equalized audio output.
///
/// # Parameters
///
/// - **Low Freq** (20-500 Hz): Low shelf center frequency.
/// - **Low Gain** (-15 to +15 dB): Low shelf gain.
/// - **Mid Freq** (100-10000 Hz): Mid band center frequency.
/// - **Mid Gain** (-15 to +15 dB): Mid band gain.
/// - **Mid Q** (0.1-10): Mid band Q factor (bandwidth).
/// - **High Freq** (2000-20000 Hz): High shelf center frequency.
/// - **High Gain** (-15 to +15 dB): High shelf gain.
/// - **Output** (-12 to +12 dB): Output gain (makeup).
pub struct ParametricEq {
    /// Sample rate.
    sample_rate: f32,
    /// Low shelf filter coefficients.
    low_coeffs: BiquadCoeffs,
    /// Mid parametric filter coefficients.
    mid_coeffs: BiquadCoeffs,
    /// High shelf filter coefficients.
    high_coeffs: BiquadCoeffs,
    /// Low shelf filter state.
    low_state: BiquadState,
    /// Mid parametric filter state.
    mid_state: BiquadState,
    /// High shelf filter state.
    high_state: BiquadState,
    /// Set when the coefficients no longer match the knobs (new sample
    /// rate, reset); forces all three bands to be recomputed.
    coeffs_dirty: bool,
    /// Whether each band's knobs were gliding at the last update, so the
    /// update after a glide ends lands the coefficients on its target.
    low_moving: bool,
    mid_moving: bool,
    high_moving: bool,
    /// Output gain in dB and as a factor, cached so the factor is only
    /// recomputed while the Output knob moves.
    output_gain_db: f32,
    output_gain: f32,
    /// Smoothed low frequency.
    low_freq_smooth: SmoothedValue,
    /// Smoothed low gain.
    low_gain_smooth: SmoothedValue,
    /// Smoothed mid frequency.
    mid_freq_smooth: SmoothedValue,
    /// Smoothed mid gain.
    mid_gain_smooth: SmoothedValue,
    /// Smoothed mid Q.
    mid_q_smooth: SmoothedValue,
    /// Smoothed high frequency.
    high_freq_smooth: SmoothedValue,
    /// Smoothed high gain.
    high_gain_smooth: SmoothedValue,
    /// Smoothed output gain.
    output_gain_smooth: SmoothedValue,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl ParametricEq {
    /// Creates a new parametric EQ.
    pub fn new() -> Self {
        let sample_rate = 44100.0;

        Self {
            sample_rate,
            low_coeffs: BiquadCoeffs::default(),
            mid_coeffs: BiquadCoeffs::default(),
            high_coeffs: BiquadCoeffs::default(),
            low_state: BiquadState::default(),
            mid_state: BiquadState::default(),
            high_state: BiquadState::default(),
            coeffs_dirty: true,
            low_moving: false,
            mid_moving: false,
            high_moving: false,
            output_gain_db: 0.0,
            output_gain: 1.0,
            low_freq_smooth: SmoothedValue::with_default_smoothing(100.0, sample_rate),
            low_gain_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            mid_freq_smooth: SmoothedValue::with_default_smoothing(1000.0, sample_rate),
            mid_gain_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            mid_q_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            high_freq_smooth: SmoothedValue::with_default_smoothing(8000.0, sample_rate),
            high_gain_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            output_gain_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            ports: vec![
                PortDefinition::input_with_default("in", "In", SignalType::Audio, 0.0).describe("Audio to equalize"),
                PortDefinition::output("out", "Out", SignalType::Audio).describe("Equalized audio"),
            ],
            parameters: vec![
                ParameterDefinition::new(
                    "low_freq",
                    "Low Freq",
                    20.0,
                    500.0,
                    100.0,
                    ParameterDisplay::Logarithmic { unit: "Hz" },
                ).describe("Corner frequency of the low shelf, in Hz"),
                ParameterDefinition::new(
                    "low_gain",
                    "Low Gain",
                    -15.0,
                    15.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Boost or cut for the lows, in dB"),
                ParameterDefinition::new(
                    "mid_freq",
                    "Mid Freq",
                    100.0,
                    10000.0,
                    1000.0,
                    ParameterDisplay::Logarithmic { unit: "Hz" },
                ).describe("Center frequency of the mid band, in Hz"),
                ParameterDefinition::new(
                    "mid_gain",
                    "Mid Gain",
                    -15.0,
                    15.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Boost or cut for the mids, in dB"),
                ParameterDefinition::new(
                    "mid_q",
                    "Mid Q",
                    0.1,
                    10.0,
                    1.0,
                    ParameterDisplay::Logarithmic { unit: "" },
                ).describe("Mid band width; higher is narrower"),
                ParameterDefinition::new(
                    "high_freq",
                    "High Freq",
                    2000.0,
                    20000.0,
                    8000.0,
                    ParameterDisplay::Logarithmic { unit: "Hz" },
                ).describe("Corner frequency of the high shelf, in Hz"),
                ParameterDefinition::new(
                    "high_gain",
                    "High Gain",
                    -15.0,
                    15.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Boost or cut for the highs, in dB"),
                ParameterDefinition::new(
                    "output_gain",
                    "Output",
                    -12.0,
                    12.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "dB" },
                ).describe("Overall level after EQ, in dB"),
            ],
        }
    }

    /// Port index constants.
    const PORT_IN: usize = 0;
    const PORT_OUT: usize = 0;

    /// Parameter index constants.
    const PARAM_LOW_FREQ: usize = 0;
    const PARAM_LOW_GAIN: usize = 1;
    const PARAM_MID_FREQ: usize = 2;
    const PARAM_MID_GAIN: usize = 3;
    const PARAM_MID_Q: usize = 4;
    const PARAM_HIGH_FREQ: usize = 5;
    const PARAM_HIGH_GAIN: usize = 6;
    const PARAM_OUTPUT_GAIN: usize = 7;

    /// Calculate low shelf biquad coefficients (RBJ Cookbook).
    fn low_shelf(sample_rate: f32, freq: f32, gain_db: f32) -> BiquadCoeffs {
        if gain_db.abs() < 0.01 {
            // Gain is essentially 0 dB, pass through
            return BiquadCoeffs {
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            };
        }

        let a = 10.0_f32.powf(gain_db / 40.0); // sqrt(10^(dB/20))
        let w0 = 2.0 * PI * freq.clamp(20.0, sample_rate * 0.45) / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();

        // Slope = 1.0 for a smooth shelf
        let s = 1.0;
        let alpha = sin_w0 / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);

        // Normalize by a0
        BiquadCoeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Calculate high shelf biquad coefficients (RBJ Cookbook).
    fn high_shelf(sample_rate: f32, freq: f32, gain_db: f32) -> BiquadCoeffs {
        if gain_db.abs() < 0.01 {
            // Gain is essentially 0 dB, pass through
            return BiquadCoeffs {
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            };
        }

        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq.clamp(20.0, sample_rate * 0.45) / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();

        let s = 1.0;
        let alpha = sin_w0 / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);

        BiquadCoeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Calculate peaking (parametric) biquad coefficients (RBJ Cookbook).
    fn peaking(sample_rate: f32, freq: f32, gain_db: f32, q: f32) -> BiquadCoeffs {
        if gain_db.abs() < 0.01 {
            // Gain is essentially 0 dB, pass through
            return BiquadCoeffs {
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            };
        }

        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq.clamp(20.0, sample_rate * 0.45) / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q.max(0.1));

        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;

        BiquadCoeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// The low shelf at this EQ's sample rate.
    fn calc_low_shelf(&self, freq: f32, gain_db: f32) -> BiquadCoeffs {
        Self::low_shelf(self.sample_rate, freq, gain_db)
    }

    /// The high shelf at this EQ's sample rate.
    fn calc_high_shelf(&self, freq: f32, gain_db: f32) -> BiquadCoeffs {
        Self::high_shelf(self.sample_rate, freq, gain_db)
    }

    /// The mid band at this EQ's sample rate.
    fn calc_peaking(&self, freq: f32, gain_db: f32, q: f32) -> BiquadCoeffs {
        Self::peaking(self.sample_rate, freq, gain_db, q)
    }

    /// What the low shelf does to each frequency, in dB, at `sample_rate`:
    /// the band's own biquad, for drawing.
    pub fn low_shelf_response(sample_rate: f32, freq: f32, gain_db: f32) -> impl Fn(f32) -> f32 {
        let coeffs = Self::low_shelf(sample_rate, freq, gain_db);
        move |hz| coeffs.magnitude_db(hz, sample_rate)
    }

    /// What the mid band does to each frequency, in dB, at `sample_rate`.
    pub fn peaking_response(sample_rate: f32, freq: f32, gain_db: f32, q: f32) -> impl Fn(f32) -> f32 {
        let coeffs = Self::peaking(sample_rate, freq, gain_db, q);
        move |hz| coeffs.magnitude_db(hz, sample_rate)
    }

    /// What the high shelf does to each frequency, in dB, at `sample_rate`.
    pub fn high_shelf_response(sample_rate: f32, freq: f32, gain_db: f32) -> impl Fn(f32) -> f32 {
        let coeffs = Self::high_shelf(sample_rate, freq, gain_db);
        move |hz| coeffs.magnitude_db(hz, sample_rate)
    }

    /// Recompute the coefficients of the bands whose knobs are gliding,
    /// plus once more for a band whose glide just ended. A band at rest
    /// keeps its coefficients and costs nothing here.
    fn update_coeffs(&mut self) {
        let dirty = self.coeffs_dirty;

        let low = self.low_freq_smooth.is_smoothing() || self.low_gain_smooth.is_smoothing();
        if dirty || low || self.low_moving {
            self.low_coeffs = self.calc_low_shelf(self.low_freq_smooth.current(), self.low_gain_smooth.current());
        }
        self.low_moving = low;

        let mid = self.mid_freq_smooth.is_smoothing()
            || self.mid_gain_smooth.is_smoothing()
            || self.mid_q_smooth.is_smoothing();
        if dirty || mid || self.mid_moving {
            self.mid_coeffs = self.calc_peaking(
                self.mid_freq_smooth.current(),
                self.mid_gain_smooth.current(),
                self.mid_q_smooth.current(),
            );
        }
        self.mid_moving = mid;

        let high = self.high_freq_smooth.is_smoothing() || self.high_gain_smooth.is_smoothing();
        if dirty || high || self.high_moving {
            self.high_coeffs =
                self.calc_high_shelf(self.high_freq_smooth.current(), self.high_gain_smooth.current());
        }
        self.high_moving = high;

        self.coeffs_dirty = false;
    }
}

impl Default for ParametricEq {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for ParametricEq {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "3-Band EQ",
            category: ModuleCategory::Effect,
            description: "Parametric EQ with low shelf, mid band, and high shelf",
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

        // Update sample rates for smoothed values
        self.low_freq_smooth.set_sample_rate(sample_rate);
        self.low_gain_smooth.set_sample_rate(sample_rate);
        self.mid_freq_smooth.set_sample_rate(sample_rate);
        self.mid_gain_smooth.set_sample_rate(sample_rate);
        self.mid_q_smooth.set_sample_rate(sample_rate);
        self.high_freq_smooth.set_sample_rate(sample_rate);
        self.high_gain_smooth.set_sample_rate(sample_rate);
        self.output_gain_smooth.set_sample_rate(sample_rate);

        // The coefficients depend on the sample rate
        self.coeffs_dirty = true;
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Set smoothing targets
        self.low_freq_smooth.set_target(params[Self::PARAM_LOW_FREQ]);
        self.low_gain_smooth.set_target(params[Self::PARAM_LOW_GAIN]);
        self.mid_freq_smooth.set_target(params[Self::PARAM_MID_FREQ]);
        self.mid_gain_smooth.set_target(params[Self::PARAM_MID_GAIN]);
        self.mid_q_smooth.set_target(params[Self::PARAM_MID_Q]);
        self.high_freq_smooth.set_target(params[Self::PARAM_HIGH_FREQ]);
        self.high_gain_smooth.set_target(params[Self::PARAM_HIGH_GAIN]);
        self.output_gain_smooth.set_target(params[Self::PARAM_OUTPUT_GAIN]);

        // Get input buffer
        let audio_in = inputs.get(Self::PORT_IN);
        let out = &mut outputs[Self::PORT_OUT];

        // Work in short runs: coefficients are brought up to date at the start
        // of each run, the knobs glide sample by sample within it
        let mut start = 0;
        while start < context.block_size {
            let end = (start + COEFF_INTERVAL).min(context.block_size);
            self.update_coeffs();

            for i in start..end {
                self.low_freq_smooth.next();
                self.low_gain_smooth.next();
                self.mid_freq_smooth.next();
                self.mid_gain_smooth.next();
                self.mid_q_smooth.next();
                self.high_freq_smooth.next();
                self.high_gain_smooth.next();

                let output_gain_db = self.output_gain_smooth.next();
                if output_gain_db != self.output_gain_db {
                    self.output_gain_db = output_gain_db;
                    self.output_gain = db_to_gain(output_gain_db);
                }

                // Get input sample
                let input = audio_in
                    .map(|buf| buf.at(i))
                    .unwrap_or(0.0);

                // Cascade through all three filter bands
                let after_low = self.low_state.process(input, &self.low_coeffs);
                let after_mid = self.mid_state.process(after_low, &self.mid_coeffs);
                let after_high = self.high_state.process(after_mid, &self.high_coeffs);

                out.samples[i] = after_high * self.output_gain;
            }

            start = end;
        }
    }

    /// The display draws what goes in and what comes out.
    fn spectrum_ports(&self) -> Option<crate::dsp::SpectrumPorts> {
        Some(crate::dsp::SpectrumPorts { input: "in", outputs: &["out"] })
    }

    fn reset(&mut self) {
        // Reset filter states
        self.low_state.reset();
        self.mid_state.reset();
        self.high_state.reset();

        // Reset smoothed values
        self.low_freq_smooth.reset(self.low_freq_smooth.target());
        self.low_gain_smooth.reset(self.low_gain_smooth.target());
        self.mid_freq_smooth.reset(self.mid_freq_smooth.target());
        self.mid_gain_smooth.reset(self.mid_gain_smooth.target());
        self.mid_q_smooth.reset(self.mid_q_smooth.target());
        self.high_freq_smooth.reset(self.high_freq_smooth.target());
        self.high_gain_smooth.reset(self.high_gain_smooth.target());
        self.output_gain_smooth.reset(self.output_gain_smooth.target());

        // The knobs jumped to their targets
        self.coeffs_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eq_info() {
        let eq = ParametricEq::new();
        assert_eq!(eq.info().id, "fx.eq");
        assert_eq!(eq.info().name, "3-Band EQ");
        assert_eq!(eq.info().category, ModuleCategory::Effect);
    }

    #[test]
    fn test_eq_ports() {
        let eq = ParametricEq::new();
        let ports = eq.ports();

        assert_eq!(ports.len(), 2);

        // Input port
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        // Output port
        assert!(ports[1].is_output());
        assert_eq!(ports[1].id, "out");
        assert_eq!(ports[1].signal_type, SignalType::Audio);
    }

    #[test]
    fn test_eq_parameters() {
        let eq = ParametricEq::new();
        let params = eq.parameters();

        assert_eq!(params.len(), 8);
        assert_eq!(params[0].id, "low_freq");
        assert_eq!(params[1].id, "low_gain");
        assert_eq!(params[2].id, "mid_freq");
        assert_eq!(params[3].id, "mid_gain");
        assert_eq!(params[4].id, "mid_q");
        assert_eq!(params[5].id, "high_freq");
        assert_eq!(params[6].id, "high_gain");
        assert_eq!(params[7].id, "output_gain");
    }

    #[test]
    fn test_eq_passthrough_at_zero_gain() {
        let mut eq = ParametricEq::new();
        eq.prepare(44100.0, 256);

        // Create a test signal
        let mut input = SignalBuffer::audio(256);
        for i in 0..256 {
            input.samples[i] = (i as f32 * 0.1).sin();
        }

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process with all gains at 0 dB and output gain at 0 dB
        // Params: low_freq, low_gain, mid_freq, mid_gain, mid_q, high_freq, high_gain, output_gain
        eq.process(
            &[&input],
            &mut outputs,
            &[100.0, 0.0, 1000.0, 0.0, 1.0, 8000.0, 0.0, 0.0],
            &ctx,
        );

        // Output should closely match input (within tolerance for filter settling)
        let skip = 50; // Skip initial transient
        for i in skip..256 {
            let diff = (input.samples[i] - outputs[0].samples[i]).abs();
            assert!(
                diff < 0.01,
                "Sample {} differs: input={}, output={}, diff={}",
                i,
                input.samples[i],
                outputs[0].samples[i],
                diff
            );
        }
    }

    #[test]
    fn test_eq_low_shelf_boost() {
        let mut eq = ParametricEq::new();
        let sample_rate = 44100.0;
        eq.prepare(sample_rate, 4410);

        // Generate low frequency signal (50 Hz)
        let mut input = SignalBuffer::audio(4410);
        let freq = 50.0;
        for i in 0..4410 {
            input.samples[i] = (2.0 * PI * freq * i as f32 / sample_rate).sin() * 0.5;
        }

        let mut outputs = vec![SignalBuffer::audio(4410)];
        let ctx = ProcessContext::new(sample_rate, 4410);

        // Boost low shelf by 12 dB
        eq.process(
            &[&input],
            &mut outputs,
            &[100.0, 12.0, 1000.0, 0.0, 1.0, 8000.0, 0.0, 0.0],
            &ctx,
        );

        // Calculate RMS after filter settles
        let skip = 441;
        let input_rms: f32 = (input.samples[skip..].iter().map(|s| s * s).sum::<f32>()
            / (4410 - skip) as f32)
            .sqrt();
        let output_rms: f32 = (outputs[0].samples[skip..].iter().map(|s| s * s).sum::<f32>()
            / (4410 - skip) as f32)
            .sqrt();

        // Output should be significantly louder (12 dB = 4x amplitude)
        assert!(
            output_rms > input_rms * 2.0,
            "Low shelf boost should amplify low frequencies: input_rms={}, output_rms={}",
            input_rms,
            output_rms
        );
    }

    #[test]
    fn test_eq_high_shelf_cut() {
        let mut eq = ParametricEq::new();
        let sample_rate = 44100.0;
        eq.prepare(sample_rate, 4410);

        // Generate high frequency signal (10 kHz)
        let mut input = SignalBuffer::audio(4410);
        let freq = 10000.0;
        for i in 0..4410 {
            input.samples[i] = (2.0 * PI * freq * i as f32 / sample_rate).sin() * 0.5;
        }

        let mut outputs = vec![SignalBuffer::audio(4410)];
        let ctx = ProcessContext::new(sample_rate, 4410);

        // Cut high shelf by 12 dB
        eq.process(
            &[&input],
            &mut outputs,
            &[100.0, 0.0, 1000.0, 0.0, 1.0, 5000.0, -12.0, 0.0],
            &ctx,
        );

        // Calculate RMS after filter settles
        let skip = 441;
        let input_rms: f32 = (input.samples[skip..].iter().map(|s| s * s).sum::<f32>()
            / (4410 - skip) as f32)
            .sqrt();
        let output_rms: f32 = (outputs[0].samples[skip..].iter().map(|s| s * s).sum::<f32>()
            / (4410 - skip) as f32)
            .sqrt();

        // Output should be quieter (12 dB cut = 1/4 amplitude)
        assert!(
            output_rms < input_rms * 0.5,
            "High shelf cut should attenuate high frequencies: input_rms={}, output_rms={}",
            input_rms,
            output_rms
        );
    }

    #[test]
    fn test_eq_mid_parametric() {
        let mut eq = ParametricEq::new();
        let sample_rate = 44100.0;
        eq.prepare(sample_rate, 4410);

        // Generate mid frequency signal (1 kHz)
        let mut input = SignalBuffer::audio(4410);
        let freq = 1000.0;
        for i in 0..4410 {
            input.samples[i] = (2.0 * PI * freq * i as f32 / sample_rate).sin() * 0.5;
        }

        let mut outputs = vec![SignalBuffer::audio(4410)];
        let ctx = ProcessContext::new(sample_rate, 4410);

        // Boost mid band at 1 kHz by 12 dB with narrow Q
        eq.process(
            &[&input],
            &mut outputs,
            &[100.0, 0.0, 1000.0, 12.0, 2.0, 8000.0, 0.0, 0.0],
            &ctx,
        );

        // Calculate RMS after filter settles
        let skip = 441;
        let input_rms: f32 = (input.samples[skip..].iter().map(|s| s * s).sum::<f32>()
            / (4410 - skip) as f32)
            .sqrt();
        let output_rms: f32 = (outputs[0].samples[skip..].iter().map(|s| s * s).sum::<f32>()
            / (4410 - skip) as f32)
            .sqrt();

        // Output should be louder at the boosted frequency
        assert!(
            output_rms > input_rms * 1.5,
            "Mid boost should amplify at center frequency: input_rms={}, output_rms={}",
            input_rms,
            output_rms
        );
    }

    #[test]
    fn test_eq_output_gain() {
        let mut eq = ParametricEq::new();
        eq.prepare(44100.0, 256);

        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process multiple blocks to let parameter smoothing settle
        for _ in 0..20 {
            eq.process(
                &[&input],
                &mut outputs,
                &[100.0, 0.0, 1000.0, 0.0, 1.0, 8000.0, 0.0, 6.0],
                &ctx,
            );
        }

        // Output should be approximately doubled (6 dB ≈ 2x amplitude)
        let skip = 50;
        let output_avg: f32 = outputs[0].samples[skip..].iter().sum::<f32>() / (256 - skip) as f32;
        assert!(
            output_avg > 0.8 && output_avg < 1.2,
            "6 dB output gain should roughly double amplitude: {}",
            output_avg
        );
    }

    #[test]
    fn test_eq_reset() {
        let mut eq = ParametricEq::new();
        eq.prepare(44100.0, 256);

        // Process some signal
        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);
        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        eq.process(
            &[&input],
            &mut outputs,
            &[100.0, 12.0, 1000.0, 12.0, 1.0, 8000.0, 12.0, 0.0],
            &ctx,
        );

        // Reset
        eq.reset();

        // Process silence
        let silence = SignalBuffer::audio(256);
        let mut outputs2 = vec![SignalBuffer::audio(256)];

        eq.process(
            &[&silence],
            &mut outputs2,
            &[100.0, 12.0, 1000.0, 12.0, 1.0, 8000.0, 12.0, 0.0],
            &ctx,
        );

        // First sample should be near zero after reset
        assert!(
            outputs2[0].samples[0].abs() < 0.01,
            "Output should be near zero after reset: {}",
            outputs2[0].samples[0]
        );
    }

    #[test]
    fn test_eq_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ParametricEq>();
    }

    #[test]
    fn test_eq_default() {
        let eq = ParametricEq::default();
        assert_eq!(eq.info().id, "fx.eq");
    }

    fn coeffs_eq(a: &BiquadCoeffs, b: &BiquadCoeffs) -> bool {
        [a.b0, a.b1, a.b2, a.a1, a.a2] == [b.b0, b.b1, b.b2, b.a1, b.a2]
    }

    #[test]
    fn test_eq_coefficients_land_on_the_knobs() {
        let mut eq = ParametricEq::new();
        eq.prepare(48000.0, 100);
        let input = SignalBuffer::audio(100);
        let mut outputs = vec![SignalBuffer::audio(100)];
        let ctx = ProcessContext::new(48000.0, 100);

        // A first set of knobs, then a turn of every band; odd block size so
        // the 16-sample runs don't line up with blocks
        for params in [
            [80.0, 6.0, 700.0, -4.0, 0.7, 6000.0, 3.0, 0.0],
            [250.0, -9.0, 3000.0, 10.0, 4.0, 12000.0, -6.0, 0.0],
        ] {
            for _ in 0..100 {
                eq.process(&[&input], &mut outputs, &params, &ctx);
            }
            assert!(coeffs_eq(&eq.low_coeffs, &eq.calc_low_shelf(params[0], params[1])));
            assert!(coeffs_eq(&eq.mid_coeffs, &eq.calc_peaking(params[2], params[3], params[4])));
            assert!(coeffs_eq(&eq.high_coeffs, &eq.calc_high_shelf(params[5], params[6])));
        }
    }

    #[test]
    fn test_eq_new_sample_rate_recomputes_coefficients() {
        let mut eq = ParametricEq::new();
        let params = [100.0, 6.0, 1000.0, 6.0, 1.0, 8000.0, 6.0, 0.0];
        let ctx = ProcessContext::new(44100.0, 256);
        let input = SignalBuffer::audio(256);
        let mut outputs = vec![SignalBuffer::audio(256)];
        eq.prepare(44100.0, 256);
        for _ in 0..50 {
            eq.process(&[&input], &mut outputs, &params, &ctx);
        }

        // The knobs haven't moved, but every band's coefficients must follow the rate
        eq.prepare(96000.0, 256);
        eq.process(&[&input], &mut outputs, &params, &ProcessContext::new(96000.0, 256));
        assert!(coeffs_eq(&eq.low_coeffs, &eq.calc_low_shelf(100.0, 6.0)));
        assert!(coeffs_eq(&eq.mid_coeffs, &eq.calc_peaking(1000.0, 6.0, 1.0)));
        assert!(coeffs_eq(&eq.high_coeffs, &eq.calc_high_shelf(8000.0, 6.0)));
    }

    #[test]
    fn test_eq_control_rate_coefficients_match_per_sample_ones() {
        // The old EQ recomputed every band every sample. Sweep all three
        // bands at once over a saw and compare against that reference.
        let sample_rate = 48000.0;
        let block = 128;
        let mut eq = ParametricEq::new();
        eq.prepare(sample_rate, block);
        let mut reference = ParametricEq::new();
        reference.prepare(sample_rate, block);

        let ctx = ProcessContext::new(sample_rate, block);
        let mut input = SignalBuffer::audio(block);
        let mut outputs = vec![SignalBuffer::audio(block)];
        let mut phase = 0.0f32;
        let mut error_energy = 0.0f64;
        let mut signal_energy = 0.0f64;

        for b in 0..400 {
            // Jump the knobs every 40 ms so they are always gliding
            let t = (b / 15) as f32;
            let params = [
                60.0 + 200.0 * (t * 1.3).sin().abs(),
                12.0 * (t * 0.7).sin(),
                300.0 + 4000.0 * (t * 0.9).sin().abs(),
                12.0 * (t * 1.1).cos(),
                0.5 + 3.0 * (t * 0.5).sin().abs(),
                3000.0 + 10000.0 * (t * 0.4).cos().abs(),
                -12.0 * (t * 0.8).sin(),
                0.0,
            ];
            for s in input.samples.iter_mut() {
                *s = 2.0 * phase - 1.0;
                phase = (phase + 110.0 / sample_rate).fract();
            }

            eq.process(&[&input], &mut outputs, &params, &ctx);

            // Reference: per-sample coefficients through the same smoothers
            for (i, s) in input.samples.iter().enumerate() {
                let r = &mut reference;
                r.low_freq_smooth.set_target(params[0]);
                r.low_gain_smooth.set_target(params[1]);
                r.mid_freq_smooth.set_target(params[2]);
                r.mid_gain_smooth.set_target(params[3]);
                r.mid_q_smooth.set_target(params[4]);
                r.high_freq_smooth.set_target(params[5]);
                r.high_gain_smooth.set_target(params[6]);
                // (from the knobs' values before this sample's step, as the module does)
                let (lf, lg) = (r.low_freq_smooth.current(), r.low_gain_smooth.current());
                let (mf, mg, mq) = (r.mid_freq_smooth.current(), r.mid_gain_smooth.current(), r.mid_q_smooth.current());
                let (hf, hg) = (r.high_freq_smooth.current(), r.high_gain_smooth.current());
                r.low_coeffs = r.calc_low_shelf(lf, lg);
                r.mid_coeffs = r.calc_peaking(mf, mg, mq);
                r.high_coeffs = r.calc_high_shelf(hf, hg);
                for smoother in [
                    &mut r.low_freq_smooth,
                    &mut r.low_gain_smooth,
                    &mut r.mid_freq_smooth,
                    &mut r.mid_gain_smooth,
                    &mut r.mid_q_smooth,
                    &mut r.high_freq_smooth,
                    &mut r.high_gain_smooth,
                ] {
                    smoother.next();
                }
                let y = r.low_state.process(*s, &r.low_coeffs);
                let y = r.mid_state.process(y, &r.mid_coeffs);
                let y = r.high_state.process(y, &r.high_coeffs);
                error_energy += ((y - outputs[0].samples[i]) as f64).powi(2);
                signal_energy += (y as f64).powi(2);
            }
        }

        // Every knob jumping up to 24 dB at once is far rougher than a hand on a
        // knob; the stepped glide still stays some 45 dB under the signal
        let error_db = 10.0 * (error_energy / signal_energy).log10();
        assert!(error_db < -40.0, "control-rate EQ strays {:.1} dB from per-sample", error_db);
    }

    #[test]
    fn test_eq_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<ParametricEq>();

        assert!(registry.contains("fx.eq"));

        let module = registry.create("fx.eq");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "fx.eq");
        assert_eq!(module.info().name, "3-Band EQ");
        assert_eq!(module.ports().len(), 2);
        assert_eq!(module.parameters().len(), 8);
    }

    #[test]
    fn test_responses_are_the_bands_own() {
        let sr = 48000.0;
        // A peak is its gain at its centre, and next to nothing far away
        let mid = ParametricEq::peaking_response(sr, 1000.0, 6.0, 1.0);
        assert!((mid(1000.0) - 6.0).abs() < 0.05, "{}", mid(1000.0));
        assert!(mid(30.0).abs() < 0.1 && mid(15000.0).abs() < 0.3);
        // Shelves reach their gain on their own side and leave the other alone
        let low = ParametricEq::low_shelf_response(sr, 200.0, -9.0);
        assert!((low(20.0) + 9.0).abs() < 0.3 && low(10_000.0).abs() < 0.1);
        let high = ParametricEq::high_shelf_response(sr, 4000.0, 12.0);
        assert!((high(18_000.0) - 12.0).abs() < 0.6 && high(100.0).abs() < 0.1);
        // At 0 dB a band is flat
        let flat = ParametricEq::peaking_response(sr, 1000.0, 0.0, 4.0);
        assert!([20.0, 1000.0, 20_000.0].iter().all(|&hz| flat(hz).abs() < 1e-4));
    }

    #[test]
    fn test_response_matches_what_the_filter_does() {
        // A sine through the mid band comes out by the response at its frequency
        let sr = 48000.0;
        let coeffs = ParametricEq::peaking(sr, 800.0, 9.0, 2.0);
        let mut state = BiquadState::default();
        let hz = 1100.0;
        let mut peak = 0.0f32;
        for n in 0..48000 {
            let y = state.process((2.0 * PI * hz * n as f32 / sr).sin(), &coeffs);
            if n > 24000 {
                peak = peak.max(y.abs());
            }
        }
        let expected = ParametricEq::peaking_response(sr, 800.0, 9.0, 2.0)(hz);
        assert!((20.0 * peak.log10() - expected).abs() < 0.1, "{} vs {expected}", 20.0 * peak.log10());
    }
}
