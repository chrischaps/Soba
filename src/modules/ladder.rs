//! Ladder Filter module.
//!
//! A Moog-style 4-pole lowpass: four saturating one-pole stages in a
//! feedback loop, run at twice the sample rate.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{prewarp, Downsampler2x, NoiseFloor, SoftSaturator, TptIntegrator, Upsampler2x},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// The four saturating stages and their feedback loop, at whatever rate they
/// are ticked.
///
/// Each stage is a one-pole lowpass whose input passes through `tanh`, as in
/// the transistor pairs of the analog ladder: `y' = ωc·(tanh(v) − y)`. The
/// first stage's input is the filter input minus `k` times the last stage's
/// output, so the same `tanh` saturates the resonance feedback. Each stage's
/// output is a lowpass of something bounded by ±1, so the whole filter is
/// bounded by ±1 however hard it is driven.
///
/// The loop is solved with zero delay (TPT). The `tanh`s are linearised as
/// gains around an estimate of this sample's stage inputs, which comes from
/// one solve with the previous sample's gains; the second solve is then exact
/// for those gains (Mystran's "cheap" nonlinear ZDF, with the one-pass
/// estimate that keeps it in tune at high cutoffs).
#[derive(Clone, Debug)]
struct LadderCore {
    stages: [TptIntegrator; 4],
    /// Each stage's input on the previous tick: the first guess at where to
    /// linearise its saturator.
    last_inputs: [f32; 4],
    saturator: SoftSaturator,
}

impl LadderCore {
    fn new() -> Self {
        Self {
            stages: [TptIntegrator::new(); 4],
            last_inputs: [0.0; 4],
            saturator: SoftSaturator::new(1.0),
        }
    }

    /// Solves the loop for the stage inputs, with each stage's saturator
    /// replaced by the gain in `gains`.
    ///
    /// `input` is the bass-compensated filter input, `g` the prewarped
    /// integrator gain and `k` the resonance feedback.
    #[inline]
    fn solve(&self, input: f32, g: f32, k: f32, gains: [f32; 4]) -> [f32; 4] {
        // A stage is y = G·a·v + S, with G = g/(1+g) and S = s/(1+g). Chained,
        // the last output is y4 = A·u + B; feedback u = input − k·y4 closes it.
        let big_g = g / (1.0 + g);
        let mut a = 1.0;
        let mut b = 0.0;
        for (stage, &gain) in self.stages.iter().zip(&gains) {
            a *= big_g * gain;
            b = big_g * gain * b + stage.state() / (1.0 + g);
        }
        let u = (input - k * b) / (1.0 + k * a);

        let mut inputs = [u; 4];
        for i in 1..4 {
            let v = inputs[i - 1];
            inputs[i] = big_g * gains[i - 1] * v + self.stages[i - 1].state() / (1.0 + g);
        }
        inputs
    }

    /// Runs one sample. Returns the outputs of the second stage (12 dB/oct)
    /// and the fourth (24 dB/oct).
    #[inline]
    fn tick(&mut self, input: f32, g: f32, k: f32) -> (f32, f32) {
        let sat = self.saturator;
        let estimate = self.solve(input, g, k, self.last_inputs.map(|v| sat.gain_at(v)));
        let gains = estimate.map(|v| sat.gain_at(v));
        let inputs = self.solve(input, g, k, gains);

        let mut outputs = [0.0; 4];
        for (i, stage) in self.stages.iter_mut().enumerate() {
            // Integrate ωc·(tanh(v) − y), with tanh(v) as the same gain the
            // loop was solved with, so the feedback the solve assumed is the
            // feedback the stages produce.
            let v = gains[i] * inputs[i];
            let y = (stage.state() + g * v) / (1.0 + g);
            outputs[i] = stage.tick(v - y, g);
        }
        self.last_inputs = inputs;
        (outputs[1], outputs[3])
    }

    fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
        self.last_inputs = [0.0; 4];
    }
}

/// A 4-pole transistor-ladder lowpass filter.
///
/// This is the filter people mean when they say a synth sounds "analog": a
/// steep 24 dB/oct slope, a resonance that thins and then sings, and a
/// saturation that rounds off everything pushed into it. Four one-pole
/// stages, each with a `tanh` at its input, sit in a loop whose feedback sets
/// the resonance. At full resonance the loop self-oscillates into a sine at
/// the cutoff frequency, held in check by the same saturators.
///
/// The classic ladder loses bass as resonance rises, because the feedback
/// subtracts the passband from the input. This one adds the input back in
/// proportion to the feedback (bass compensation), so turning up resonance
/// adds a peak without hollowing out the low end.
///
/// The saturators create harmonics, and harmonics above Nyquist fold back as
/// inharmonic aliases. The core runs at twice the sample rate between
/// halfband up- and downsamplers, which keeps a hard-driven high note clean.
///
/// # Ports
///
/// - **In** (Audio, Input): The audio signal to filter.
/// - **Cutoff** (Control, Input): Cutoff CV, in octaves per unit times the
///   Cutoff CV knob. At the default of 1, patch a keyboard's pitch here and
///   the cutoff tracks the notes.
/// - **Resonance** (Control, Input): CV modulation for resonance.
/// - **LP24** (Audio, Output): 4-pole lowpass, 24 dB/oct.
/// - **LP12** (Audio, Output): 2-pole lowpass tapped from the same loop, 12 dB/oct.
///
/// # Parameters
///
/// - **Cutoff** (20-20000 Hz): Filter cutoff frequency.
/// - **Resonance** (0-1): Emphasis at cutoff. Self-oscillates at the top of the range.
/// - **Drive** (1-10): Input gain into the saturating stages.
/// - **Cutoff CV** (-4 to +4 oct): Octaves per unit of Cutoff CV, so a 0-1
///   envelope can sweep up to four octaves. Negative values close the filter.
pub struct LadderFilter {
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// The filter, running at twice the sample rate.
    core: LadderCore,
    upsampler: Upsampler2x,
    lp24_down: Downsampler2x,
    lp12_down: Downsampler2x,
    /// Circuit noise that seeds self-oscillation.
    noise: NoiseFloor,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
    /// Smoothed cutoff, in log2(Hz).
    log_cutoff_smooth: SmoothedValue,
    /// Smoothed resonance parameter.
    resonance_smooth: SmoothedValue,
    /// Smoothed drive parameter.
    drive_smooth: SmoothedValue,
    /// Smoothed Cutoff CV depth, in octaves per unit of CV.
    cv_depth_smooth: SmoothedValue,
    /// The cutoff and resonance it ran at, after CV, at the last sample.
    running: (f32, f32),
}

impl LadderFilter {
    /// Creates a new ladder filter.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            sample_rate,
            core: LadderCore::new(),
            upsampler: Upsampler2x::new(),
            lp24_down: Downsampler2x::new(),
            lp12_down: Downsampler2x::new(),
            noise: NoiseFloor::default(),
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in", "In", SignalType::Audio, 0.0).describe("Audio to filter"),
                PortDefinition::input_with_default("cutoff_cv", "Cutoff", SignalType::Control, 0.0).describe("CV that sweeps the cutoff, by the Cutoff CV knob's octaves per unit"),
                PortDefinition::input_with_default("res_cv", "Resonance", SignalType::Control, 0.0).describe("CV that raises or lowers the resonance"),
                // Output ports
                PortDefinition::output("lp24", "LP24", SignalType::Audio).describe("Four-pole lowpass, 24 dB per octave"),
                PortDefinition::output("lp12", "LP12", SignalType::Audio).describe("Two-pole lowpass from the same filter, 12 dB per octave"),
            ],
            parameters: vec![
                ParameterDefinition::frequency("cutoff", "Cutoff", 20.0, 20000.0, 1000.0).describe("Where the filter starts cutting, in Hz"),
                ParameterDefinition::new("resonance", "Resonance", 0.0, 1.0, 0.5, ParameterDisplay::Linear { unit: "" }).describe("Resonant peak at the cutoff; high values self-oscillate"),
                ParameterDefinition::new("drive", "Drive", 1.0, 10.0, 1.0, ParameterDisplay::Linear { unit: "x" }).describe("Input gain into the saturating stages; higher is grittier"),
                ParameterDefinition::new("cutoff_cv_depth", "Cutoff CV", -4.0, 4.0, 1.0, ParameterDisplay::Linear { unit: "oct" }).describe("Octaves the cutoff moves per unit at the Cutoff input; negative closes it"),
            ],
            log_cutoff_smooth: SmoothedValue::with_default_smoothing(1000.0f32.log2(), sample_rate),
            resonance_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            drive_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            cv_depth_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            running: (1000.0, 0.5),
        }
    }

    /// Port index constants.
    const PORT_IN: usize = 0;
    const PORT_CUTOFF_CV: usize = 1;
    const PORT_RES_CV: usize = 2;

    /// Parameter index constants.
    const PARAM_CUTOFF: usize = 0;
    const PARAM_RESONANCE: usize = 1;
    const PARAM_DRIVE: usize = 2;
    const PARAM_CUTOFF_CV: usize = 3;

    /// Lowest cutoff the filter will run at, after CV.
    const MIN_CUTOFF_HZ: f32 = 20.0;
    /// Highest cutoff, as a fraction of the sample rate. Kept inside the
    /// oversampler's passband so a resonant peak is never filtered away.
    const MAX_CUTOFF_RATIO: f32 = 0.42;
    /// Feedback at full resonance. A linear ladder oscillates at exactly 4,
    /// so the top fifth of the knob self-oscillates, about where a
    /// Minimoog's Emphasis starts to sing. How far past 4 it goes sets how
    /// hard the saturators must work to hold the oscillation, and so how
    /// loud it is: about -20 dBFS at full resonance.
    const MAX_FEEDBACK: f32 = 5.0;
    /// How much of the feedback's bass loss is added back at the input
    /// (1 = full: the passband stays level at every resonance).
    const BASS_COMPENSATION: f32 = 1.0;

    /// Feedback `k` for a resonance setting.
    #[inline]
    fn feedback(resonance: f32) -> f32 {
        resonance.clamp(0.0, 1.0) * Self::MAX_FEEDBACK
    }

    /// Small-signal LP24 gain in dB at `freq` for the given knob settings,
    /// from the analog prototype the filter is modelled on. Used to draw the
    /// response curve on the node, so the picture matches the sound.
    pub fn lowpass_response_db(cutoff_hz: f32, resonance: f32, freq: f32) -> f32 {
        Self::tap_response_db(cutoff_hz, resonance, freq, 4)
    }

    /// Small-signal LP12 gain in dB, as [`Self::lowpass_response_db`]: the
    /// same loop, tapped after its second stage.
    pub fn lp12_response_db(cutoff_hz: f32, resonance: f32, freq: f32) -> f32 {
        Self::tap_response_db(cutoff_hz, resonance, freq, 2)
    }

    /// The gain at the output of stage `tap` (1-4) of the loop.
    fn tap_response_db(cutoff_hz: f32, resonance: f32, freq: f32, tap: i32) -> f32 {
        let w = (freq / cutoff_hz.max(1.0)) as f64;
        let k = Self::feedback(resonance) as f64;
        // One stage is 1/(1 + jw); four in a row is 1/(1 + jw)^4
        let (re, im) = {
            let (mut re, mut im) = (1.0f64, 0.0f64);
            for _ in 0..4 {
                (re, im) = (re - im * w, im + re * w);
            }
            (re, im)
        };
        // At stage n: H = (1 + c·k)(1 + jw)^(4-n) / ((1 + jw)^4 + k)
        let skipped = (1.0 + w * w).sqrt().powi(4 - tap);
        let gain = (1.0 + Self::BASS_COMPENSATION as f64 * k) * skipped / (re + k).hypot(im);
        (20.0 * gain.max(1e-9).log10()) as f32
    }

    /// Readout indices: the cutoff (Hz) and resonance it's running at,
    /// after CV, as of the end of the last block.
    pub const READOUT_CUTOFF: usize = 0;
    pub const READOUT_RESONANCE: usize = 1;

    /// The resonance past which the ladder rings on its own (k = 4).
    pub const SELF_OSCILLATION: f32 = 4.0 / Self::MAX_FEEDBACK;

    /// The cutoff the filter is currently running at, in Hz, before CV.
    #[cfg(test)]
    fn current_cutoff_hz(&self) -> f32 {
        self.log_cutoff_smooth.current().exp2()
    }
}

impl Default for LadderFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for LadderFilter {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "filter.ladder",
            name: "Ladder Filter",
            category: ModuleCategory::Filter,
            description: "Moog-style 4-pole lowpass with saturating stages, 24 and 12 dB/oct outputs",
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
        self.log_cutoff_smooth.set_sample_rate(sample_rate);
        self.resonance_smooth.set_sample_rate(sample_rate);
        self.drive_smooth.set_sample_rate(sample_rate);
        self.cv_depth_smooth.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let cutoff_param = params[Self::PARAM_CUTOFF].max(Self::MIN_CUTOFF_HZ);
        self.log_cutoff_smooth.set_target(cutoff_param.log2());
        self.resonance_smooth.set_target(params[Self::PARAM_RESONANCE]);
        self.drive_smooth.set_target(params[Self::PARAM_DRIVE].clamp(1.0, 10.0));
        let cv_depth = params.get(Self::PARAM_CUTOFF_CV).copied().unwrap_or(1.0);
        self.cv_depth_smooth.set_target(cv_depth.clamp(-4.0, 4.0));

        let audio_in = inputs.get(Self::PORT_IN);
        let cutoff_cv = inputs.get(Self::PORT_CUTOFF_CV);
        let res_cv = inputs.get(Self::PORT_RES_CV);

        let [lp24_out, lp12_out, ..] = outputs else {
            return;
        };

        let max_cutoff = self.sample_rate * Self::MAX_CUTOFF_RATIO;
        let oversampled_rate = self.sample_rate * 2.0;
        let sample = |buf: Option<&&SignalBuffer>, i: usize| {
            buf.map(|b| b.samples.get(i).copied().unwrap_or(0.0)).unwrap_or(0.0)
        };

        for i in 0..context.block_size {
            let log_cutoff = self.log_cutoff_smooth.next();
            let base_resonance = self.resonance_smooth.next();
            let drive = self.drive_smooth.next();

            // Cutoff CV is in octaves, scaled by its depth, added in the log domain
            let cv_depth = self.cv_depth_smooth.next();
            let cutoff = (log_cutoff + sample(cutoff_cv, i) * cv_depth)
                .exp2()
                .clamp(Self::MIN_CUTOFF_HZ, max_cutoff);
            let resonance = (base_resonance + sample(res_cv, i) * 0.5).clamp(0.0, 1.0);
            self.running = (cutoff, resonance);

            let g = prewarp(cutoff, oversampled_rate);
            let k = Self::feedback(resonance);
            // Bass compensation: u = x − k·(y4 − c·x)
            let input_gain = drive * (1.0 + Self::BASS_COMPENSATION * k);
            let input = sample(audio_in, i) * input_gain + self.noise.sample();

            let mut lp12 = [0.0; 2];
            let mut lp24 = [0.0; 2];
            for (j, x) in self.upsampler.process(input).into_iter().enumerate() {
                (lp12[j], lp24[j]) = self.core.tick(x, g, k);
            }
            lp24_out.samples[i] = self.lp24_down.process(lp24);
            lp12_out.samples[i] = self.lp12_down.process(lp12);
        }
    }

    fn reset(&mut self) {
        self.core.reset();
        self.upsampler.reset();
        self.lp24_down.reset();
        self.lp12_down.reset();
        self.log_cutoff_smooth.reset(self.log_cutoff_smooth.target());
        self.resonance_smooth.reset(self.resonance_smooth.target());
        self.drive_smooth.reset(self.drive_smooth.target());
        self.cv_depth_smooth.reset(self.cv_depth_smooth.target());
    }

    /// The cutoff and resonance after CV, for the display.
    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_CUTOFF] = self.running.0;
        readout.values[Self::READOUT_RESONANCE] = self.running.1;
        Some(readout)
    }

    /// The display draws what goes in and what the heard output lets out.
    fn spectrum_ports(&self) -> Option<crate::dsp::SpectrumPorts> {
        Some(crate::dsp::SpectrumPorts { input: "in", outputs: &["lp24", "lp12"] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{amp_to_db, peak, rms, Spectrum};
    use std::f64::consts::PI;

    const SR: f32 = 48000.0;

    fn outputs(n: usize) -> Vec<SignalBuffer> {
        (0..2).map(|_| SignalBuffer::audio(n)).collect()
    }

    /// A sine computed in f64, so long test tones stay spectrally pure.
    fn sine(freq: f32, amplitude: f32, n: usize) -> SignalBuffer {
        let mut buf = SignalBuffer::audio(n);
        for i in 0..n {
            buf.samples[i] = amplitude * (2.0 * PI * freq as f64 * i as f64 / SR as f64).sin() as f32;
        }
        buf
    }

    /// A filter with its smoothing already settled on `params`.
    fn settled(params: [f32; 3]) -> LadderFilter {
        let mut filter = LadderFilter::new();
        filter.prepare(SR, 256);
        filter.log_cutoff_smooth.set_target(params[0].log2());
        filter.resonance_smooth.set_target(params[1]);
        filter.drive_smooth.set_target(params[2]);
        filter
    }

    /// Runs `input` through a settled filter (with optional cutoff CV) and
    /// returns both outputs.
    fn run(params: [f32; 3], input: &SignalBuffer, cutoff_cv: f32) -> Vec<SignalBuffer> {
        let n = input.samples.len();
        let mut filter = settled(params);
        let mut cv = SignalBuffer::control(n);
        cv.fill(cutoff_cv);
        let mut outs = outputs(n);
        filter.process(&[input, &cv], &mut outs, &params, &ProcessContext::new(SR, n));
        outs
    }

    /// Magnitude of the component at `freq` in the second half of `samples`.
    /// Measured in the spectrum, so a tone far down the slope still reads
    /// above the filter's -120 dB noise floor.
    fn tone_level(samples: &[f32], freq: f32) -> f32 {
        let spectrum = Spectrum::of(&samples[samples.len() / 2..], SR);
        let bin = (freq as f64 / spectrum.bin_hz).round() as usize;
        spectrum.magnitudes[bin - 2..=bin + 2].iter().cloned().fold(0.0, f64::max) as f32
    }

    /// Steady-state gain of output `port` for a quiet sine at `freq`.
    fn gain(port: usize, cutoff: f32, resonance: f32, freq: f32) -> f32 {
        let n = 24000;
        // Quiet enough that the saturators stay linear, even inside a
        // resonant peak
        let input = sine(freq, 0.001, n);
        let outs = run([cutoff, resonance, 1.0], &input, 0.0);
        tone_level(&outs[port].samples, freq) / tone_level(&input.samples, freq)
    }

    #[test]
    fn test_ladder_info_and_ports() {
        let filter = LadderFilter::new();
        assert_eq!(filter.info().id, "filter.ladder");
        assert_eq!(filter.info().category, ModuleCategory::Filter);
        let ports: Vec<_> = filter.ports().iter().map(|p| (p.is_input(), p.name, p.signal_type)).collect();
        assert_eq!(
            ports,
            vec![
                (true, "In", SignalType::Audio),
                (true, "Cutoff", SignalType::Control),
                (true, "Resonance", SignalType::Control),
                (false, "LP24", SignalType::Audio),
                (false, "LP12", SignalType::Audio),
            ]
        );
        let params: Vec<_> = filter.parameters().iter().map(|p| (p.name, p.min, p.max, p.default)).collect();
        assert_eq!(
            params,
            vec![
                ("Cutoff", 20.0, 20000.0, 1000.0),
                ("Resonance", 0.0, 1.0, 0.5),
                ("Drive", 1.0, 10.0, 1.0),
                ("Cutoff CV", -4.0, 4.0, 1.0),
            ]
        );
    }

    #[test]
    fn test_ladder_registry_instantiation() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create("filter.ladder").expect("ladder is registered");
        assert_eq!(module.info().name, "Ladder Filter");
    }

    #[test]
    fn test_lp24_slope_is_24_db_per_octave() {
        // Two and three octaves above a 500 Hz cutoff the response falls
        // 24 dB per octave
        let at_2k = amp_to_db(gain(0, 500.0, 0.0, 2000.0));
        let at_4k = amp_to_db(gain(0, 500.0, 0.0, 4000.0));
        let at_8k = amp_to_db(gain(0, 500.0, 0.0, 8000.0));
        for (slope, band) in [(at_4k - at_2k, "2-4 kHz"), (at_8k - at_4k, "4-8 kHz")] {
            assert!((slope + 24.0).abs() < 1.0, "{}: {:.2} dB/oct", band, slope);
        }
    }

    #[test]
    fn test_lp12_slope_is_12_db_per_octave() {
        let at_4k = amp_to_db(gain(1, 500.0, 0.0, 4000.0));
        let at_8k = amp_to_db(gain(1, 500.0, 0.0, 8000.0));
        assert!((at_8k - at_4k + 12.0).abs() < 1.0, "{:.2} dB/oct", at_8k - at_4k);
    }

    #[test]
    fn test_ladder_gain_at_cutoff() {
        // Four poles at the cutoff with no resonance: 4 × -3 dB = -12 dB
        for fc in [100.0, 1000.0, 8000.0] {
            let db = amp_to_db(gain(0, fc, 0.0, fc));
            assert!((db + 12.04).abs() < 0.5, "{} Hz cutoff: {:.2} dB at cutoff", fc, db);
        }
    }

    #[test]
    fn test_resonance_keeps_the_bass() {
        // The uncompensated ladder loses 1/(1+k) of its passband: -14 dB at
        // full resonance. With compensation the bass stays put.
        let flat = amp_to_db(gain(0, 2000.0, 0.0, 100.0));
        for res in [0.5, 0.75] {
            let db = amp_to_db(gain(0, 2000.0, res, 100.0));
            assert!((db - flat).abs() < 1.0, "res {}: {:.2} dB vs {:.2} dB", res, db, flat);
        }
        // ...while the resonance raises a peak at the cutoff
        let peak = amp_to_db(gain(0, 2000.0, 0.75, 2000.0));
        assert!(peak > 6.0, "peak at cutoff only {:.2} dB", peak);
    }

    #[test]
    fn test_response_curve_matches_the_filter() {
        for &(fc, res, f) in &[(1000.0, 0.0, 1000.0), (1000.0, 0.5, 1000.0), (500.0, 0.7, 1500.0), (2000.0, 0.3, 700.0), (300.0, 0.7, 300.0)] {
            let drawn = LadderFilter::lowpass_response_db(fc, res, f);
            let measured = amp_to_db(gain(0, fc, res, f));
            assert!((drawn - measured).abs() < 0.75, "fc {} res {} at {} Hz: drawn {:.2} dB, measured {:.2} dB", fc, res, f, drawn, measured);
        }
    }

    #[test]
    fn test_lp12_curve_matches_the_filter() {
        for &(fc, res, f) in &[(1000.0, 0.0, 1000.0), (1000.0, 0.5, 1000.0), (500.0, 0.7, 1500.0), (2000.0, 0.3, 700.0), (300.0, 0.7, 300.0)] {
            let drawn = LadderFilter::lp12_response_db(fc, res, f);
            let measured = amp_to_db(gain(1, fc, res, f));
            assert!((drawn - measured).abs() < 0.75, "fc {fc} res {res} at {f} Hz: drawn {drawn:.2} dB, measured {measured:.2} dB");
        }
    }

    #[test]
    fn test_cutoff_cv_is_one_per_octave() {
        // +1 CV doubles a 500 Hz cutoff: at 1 kHz the gain is the -12 dB a
        // ladder has at its cutoff
        let n = 19200;
        let input = sine(1000.0, 0.01, n);
        for (cutoff, cv) in [(500.0, 1.0), (2000.0, -1.0)] {
            let outs = run([cutoff, 0.0, 1.0], &input, cv);
            let db = amp_to_db(tone_level(&outs[0].samples, 1000.0) / tone_level(&input.samples, 1000.0));
            assert!((db + 12.04).abs() < 0.5, "{} Hz, CV {}: {:.2} dB", cutoff, cv, db);
        }
    }

    #[test]
    fn test_cutoff_cv_depth_scales_the_octaves() {
        // At 3 oct/unit, +1 CV takes a 125 Hz cutoff to 1 kHz; at -2, it
        // closes a 4 kHz cutoff to 1 kHz. Either way 1 kHz sits at the cutoff
        let n = 19200;
        let input = sine(1000.0, 0.01, n);
        for (cutoff, depth) in [(125.0, 3.0), (4000.0, -2.0)] {
            let params = [cutoff, 0.0, 1.0, depth];
            let mut filter = settled([cutoff, 0.0, 1.0]);
            filter.cv_depth_smooth.reset(depth);
            let mut cv = SignalBuffer::control(n);
            cv.fill(1.0);
            let mut outs = outputs(n);
            filter.process(&[&input, &cv], &mut outs, &params, &ProcessContext::new(SR, n));
            let db = amp_to_db(tone_level(&outs[0].samples, 1000.0) / tone_level(&input.samples, 1000.0));
            assert!((db + 12.04).abs() < 0.5, "{} Hz at {} oct/unit: {:.2} dB", cutoff, depth, db);
        }
    }

    /// Runs a filter with nothing patched in and returns the LP24 output
    /// after `settle` seconds, `n` samples long.
    fn free_running(cutoff: f32, resonance: f32, settle: f32, n: usize) -> Vec<f32> {
        let block = 256;
        let params = [cutoff, resonance, 1.0];
        let mut filter = settled(params);
        let silence = SignalBuffer::unconnected(block, SignalType::Audio);
        let mut outs = outputs(block);
        let ctx = ProcessContext::new(SR, block);
        let skip = (settle * SR) as usize;
        let mut out = Vec::with_capacity(n);
        let mut done = 0;
        while out.len() < n {
            filter.process(&[&silence], &mut outs, &params, &ctx);
            for &s in &outs[0].samples {
                if done >= skip && out.len() < n {
                    out.push(s);
                }
                done += 1;
            }
        }
        out
    }

    #[test]
    fn test_self_oscillates_at_max_resonance() {
        for fc in [110.0, 440.0, 1760.0, 5000.0] {
            let out = free_running(fc, 1.0, 2.0, 32768);
            let level = rms(&out);
            assert!(level > 0.08, "{} Hz: self-oscillation too quiet ({:.3} rms)", fc, level);
            assert!(peak(&out) <= 1.0, "{} Hz: peaks at {}", fc, peak(&out));

            let f = Spectrum::of(&out, SR).dominant_frequency();
            let cents = 1200.0 * (f / fc as f64).log2();
            assert!(cents.abs() < 5.0, "{} Hz: oscillates at {:.2} Hz ({:+.1} cents)", fc, f, cents);
        }
    }

    #[test]
    fn test_high_resonance_without_input_stays_quiet() {
        // Just below the self-oscillation threshold (k = 3.75)
        let out = free_running(880.0, 0.75, 1.0, 4800);
        assert!(peak(&out) < 1e-3, "res 0.75 should not self-oscillate, peak {}", peak(&out));
    }

    #[test]
    fn test_drive_is_bounded() {
        // A square wave driven 10x into full resonance stays within ±1
        let n = 48000;
        let mut input = SignalBuffer::audio(n);
        for i in 0..n {
            input.samples[i] = if i % 200 < 100 { 1.0 } else { -1.0 };
        }
        let outs = run([800.0, 1.0, 10.0], &input, 0.0);
        for (name, out) in ["LP24", "LP12"].iter().zip(&outs) {
            assert!(out.samples.iter().all(|s| s.is_finite()), "{} not finite", name);
            assert!(peak(&out.samples) <= 1.05, "{} peaks at {}", name, peak(&out.samples));
        }
    }

    /// Energy below 20 kHz that is not a harmonic of `fundamental`, in dB
    /// relative to the total: the audible aliasing.
    fn audible_alias_db(samples: &[f32], fundamental: f64) -> f64 {
        let spectrum = Spectrum::of(samples, SR);
        let top = (20000.0 / spectrum.bin_hz) as usize;
        let mut total = 0.0;
        let mut alias = 0.0;
        for (k, &mag) in spectrum.magnitudes.iter().enumerate().take(top).skip(1) {
            let freq = k as f64 * spectrum.bin_hz;
            let harmonic = (freq / fundamental).round().max(1.0) * fundamental;
            total += mag * mag;
            // Wide enough that the window's leakage around each harmonic
            // (about -55 dB at 6 bins) isn't counted as aliasing
            if (freq - harmonic).abs() > 50.0 * spectrum.bin_hz {
                alias += mag * mag;
            }
        }
        10.0 * (alias.max(1e-30) / total).log10()
    }

    /// Audible alias level of a 5 kHz full-scale sine through the ladder at
    /// `params`: (oversampled module, the same core at the base rate).
    fn alias_levels(params: [f32; 3]) -> (f64, f64) {
        let n = 32768;
        let settle = 4096;
        let input = sine(5000.0, 1.0, n + settle);
        let oversampled = run(params, &input, 0.0);

        let [fc, res, drive] = params;
        let mut core = LadderCore::new();
        let (g, k) = (prewarp(fc, SR), LadderFilter::feedback(res));
        let input_gain = drive * (1.0 + LadderFilter::BASS_COMPENSATION * k);
        let base_rate: Vec<f32> = input.samples.iter().map(|&x| core.tick(x * input_gain, g, k).1).collect();

        (
            audible_alias_db(&oversampled[0].samples[settle..], 5000.0),
            audible_alias_db(&base_rate[settle..], 5000.0),
        )
    }

    #[test]
    fn test_oversampling_suppresses_aliasing() {
        // A 5 kHz tone driven into the ladder. Its odd harmonics at 35, 45,
        // 55 kHz... fold to 13, 3, 7 kHz at 48 kHz: inharmonic, easy to hear
        // and easy to measure.
        for params in [[8000.0, 0.0, 2.0], [4000.0, 0.0, 2.0], [12000.0, 0.0, 2.0], [8000.0, 0.3, 1.0]] {
            let (with, without) = alias_levels(params);
            assert!(
                with < without - 30.0,
                "{:?}: aliasing {:.1} dB oversampled vs {:.1} dB without",
                params,
                with,
                without
            );
        }
    }

    #[test]
    fn test_cutoff_sweeps_evenly_in_octaves() {
        let samples_to_midpoint = |from: f32, to: f32| {
            let mut filter = settled([from, 0.0, 1.0]);
            let silence = SignalBuffer::audio(1);
            let mut outs = outputs(1);
            let ctx = ProcessContext::new(SR, 1);
            let mid = (from * to).sqrt();
            (1..48000usize)
                .find(|_| {
                    filter.process(&[&silence], &mut outs, &[to, 0.0, 1.0], &ctx);
                    let fc = filter.current_cutoff_hz();
                    (from < to && fc >= mid) || (from > to && fc <= mid)
                })
                .expect("cutoff reaches the midpoint")
        };
        let up = samples_to_midpoint(100.0, 6400.0);
        let down = samples_to_midpoint(6400.0, 100.0);
        assert!(up.abs_diff(down) <= 1, "up {} samples, down {} samples", up, down);
    }

    #[test]
    fn test_reset_clears_state() {
        let mut filter = settled([1000.0, 0.5, 1.0]);
        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);
        let mut outs = outputs(256);
        let ctx = ProcessContext::new(SR, 256);
        filter.process(&[&input], &mut outs, &[1000.0, 0.5, 1.0], &ctx);
        filter.reset();
        let silence = SignalBuffer::audio(256);
        filter.process(&[&silence], &mut outs, &[1000.0, 0.5, 1.0], &ctx);
        assert!(peak(&outs[0].samples) < 1e-4, "peak after reset {}", peak(&outs[0].samples));
    }

    #[test]
    fn test_self_oscillation_renders_through_the_patch() {
        use crate::engine::OfflineRenderer;
        use crate::persistence::{ConnectionData, NamedParameter, NodeData, ParameterValue, Patch};

        let mut patch = Patch::new("ladder self-oscillation");
        let mut filter = NodeData::new(1, "filter.ladder", (0.0, 0.0));
        filter.parameters = vec![
            NamedParameter::new("Cutoff", ParameterValue::Frequency(440.0)),
            NamedParameter::new("Resonance", ParameterValue::Number(1.0)),
        ];
        patch.nodes.push(filter);
        patch.nodes.push(NodeData::new(2, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "LP24", 2, "Mono"));

        let (mut r, compiled) = OfflineRenderer::from_patch(&patch, SR, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        r.render_seconds(2.0);
        let out = r.render(65536);
        assert!(rms(&out.left) > 0.05, "should be audible, rms {}", rms(&out.left));
        let f = Spectrum::of(&out.left, SR).dominant_frequency();
        let cents = 1200.0 * (f / 440.0).log2();
        assert!(cents.abs() < 5.0, "oscillates at {:.2} Hz ({:+.1} cents)", f, cents);
    }

    #[test]
    fn test_ladder_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<LadderFilter>();
    }
}

