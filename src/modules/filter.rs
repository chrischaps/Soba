//! State Variable Filter module.
//!
//! A versatile filter topology providing simultaneous lowpass, highpass,
//! bandpass and notch outputs from a single algorithm.

use crate::dsp::{
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    context::ProcessContext,
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{fast_tanh, prewarp, NoiseFloor, SoftSaturator, TptIntegrator},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    SignalType,
};

/// A State Variable Filter with multiple simultaneous outputs.
///
/// The SVF is a classic filter topology that provides lowpass, highpass,
/// bandpass and notch outputs simultaneously. This implementation uses the
/// topology-preserving transform (TPT / zero-delay feedback) form described
/// by Zavalishin and Simper: it is stable at every cutoff up to Nyquist and
/// tracks the analog response closely, unlike the Chamberlin form.
///
/// Resonance works like the analog circuit. A fixed damping path keeps the
/// filter well behaved, and a resonance path feeds the bandpass back through
/// a soft saturator to cancel that damping. At full resonance the feedback
/// slightly outweighs the damping, so the filter self-oscillates. The
/// saturator then limits the oscillation to a steady, nearly pure sine at the
/// cutoff frequency, and keeps a loud input at high resonance from running
/// away.
///
/// Cutoff lives in octaves: the knob is smoothed in log2(Hz) and CV is added
/// in octaves, so a sweep moves evenly through the musical range instead of
/// rushing through the bass and crawling through the treble. The Cutoff CV
/// knob sets how many octaves one unit of CV moves it: 1 is the V/Oct scale,
/// so pitch into Cutoff tracks the keyboard, and 3 lets a 0-1 envelope sweep
/// three octaves.
///
/// # Ports
///
/// - **In** (Audio, Input): The audio signal to filter.
/// - **Cutoff** (Control, Input): Cutoff CV, in octaves per unit times the
///   Cutoff CV knob (1 by default: the V/Oct scale).
/// - **Resonance** (Control, Input): CV modulation for resonance.
/// - **LowPass** (Audio, Output): Lowpass filtered output.
/// - **HighPass** (Audio, Output): Highpass filtered output.
/// - **BandPass** (Audio, Output): Bandpass filtered output.
/// - **Notch** (Audio, Output): Band-reject output (everything but the cutoff).
///
/// # Parameters
///
/// - **Cutoff** (20-20000 Hz): Filter cutoff frequency.
/// - **Resonance** (0-1): Emphasis at cutoff. Self-oscillates at the top of the range.
/// - **Drive** (1-10): Input gain/saturation for analog-style warmth.
/// - **Cutoff CV** (-4 to +4 oct): Octaves per unit of Cutoff CV. Negative
///   values turn the CV upside down, so an envelope closes the filter.
pub struct SvfFilter {
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// First integrator (its output is the bandpass).
    band_int: TptIntegrator,
    /// Second integrator (its output is the lowpass).
    low_int: TptIntegrator,
    /// Bandpass output of the previous sample: the operating point at which
    /// the resonance saturator is linearised.
    last_band: f32,
    /// Saturator in the resonance feedback path.
    resonance_sat: SoftSaturator,
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

impl SvfFilter {
    /// Creates a new SVF filter.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            sample_rate,
            band_int: TptIntegrator::new(),
            low_int: TptIntegrator::new(),
            last_band: 0.0,
            resonance_sat: SoftSaturator::new(Self::RESONANCE_SAT_LEVEL),
            noise: NoiseFloor::default(),
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in", "In", SignalType::Audio, 0.0).describe("Audio to filter"),
                PortDefinition::input_with_default("cutoff_cv", "Cutoff", SignalType::Control, 0.0).describe("CV that sweeps the cutoff, by the Cutoff CV knob's octaves per unit"),
                PortDefinition::input_with_default("res_cv", "Resonance", SignalType::Control, 0.0).describe("CV that raises or lowers the resonance"),
                // Output ports
                PortDefinition::output("lowpass", "LowPass", SignalType::Audio).describe("Passes frequencies below the cutoff"),
                PortDefinition::output("highpass", "HighPass", SignalType::Audio).describe("Passes frequencies above the cutoff"),
                PortDefinition::output("bandpass", "BandPass", SignalType::Audio).describe("Passes a band of frequencies around the cutoff"),
                PortDefinition::output("notch", "Notch", SignalType::Audio).describe("Removes a band around the cutoff and passes the rest"),
            ],
            parameters: vec![
                ParameterDefinition::frequency("cutoff", "Cutoff", 20.0, 20000.0, 1000.0).describe("Where the filter starts cutting, in Hz"),
                ParameterDefinition::new(
                    "resonance",
                    "Resonance",
                    0.0,
                    1.0,
                    0.5,
                    crate::dsp::ParameterDisplay::Linear { unit: "" },
                ).describe("Resonant peak at the cutoff; high values self-oscillate"),
                ParameterDefinition::new(
                    "drive",
                    "Drive",
                    1.0,
                    10.0,
                    1.0,
                    crate::dsp::ParameterDisplay::Linear { unit: "x" },
                ).describe("Input gain into the filter; higher adds saturation"),
                ParameterDefinition::new(
                    "cutoff_cv_depth",
                    "Cutoff CV",
                    -4.0,
                    4.0,
                    1.0,
                    crate::dsp::ParameterDisplay::Linear { unit: "oct" },
                ).describe("Octaves the cutoff moves per unit at the Cutoff input; negative closes it"),
            ],
            // Initialize smoothed parameters
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
    /// Damping of the fixed path (k = 2 is a Q of 0.5: no peak, -6 dB at cutoff).
    const DAMPING: f32 = 2.0;
    /// Resonance feedback at full resonance. It is a little more than
    /// `DAMPING`, so the top few percent of the knob self-oscillates.
    const MAX_FEEDBACK: f32 = 2.06;
    /// Level the resonance saturator limits the bandpass feedback to.
    const RESONANCE_SAT_LEVEL: f32 = 1.0;

    /// Small-signal damping (1/Q) for a resonance setting: what the filter's
    /// response looks like before the saturator starts to act. It falls from
    /// 2 at resonance 0 to a little below 0 at resonance 1.
    #[inline]
    pub fn small_signal_damping(resonance: f32) -> f32 {
        Self::DAMPING - resonance.clamp(0.0, 1.0) * Self::MAX_FEEDBACK
    }

    /// Small-signal lowpass gain in dB at `freq` for the given knob settings,
    /// from the analog prototype the filter is modelled on. Used to draw the
    /// response curve on the node, so the picture matches the sound.
    pub fn lowpass_response_db(cutoff_hz: f32, resonance: f32, freq: f32) -> f32 {
        Self::response_db(cutoff_hz, resonance, freq, |_| 1.0)
    }

    /// Small-signal highpass gain in dB, as [`Self::lowpass_response_db`].
    pub fn highpass_response_db(cutoff_hz: f32, resonance: f32, freq: f32) -> f32 {
        Self::response_db(cutoff_hz, resonance, freq, |w| w * w)
    }

    /// Small-signal bandpass gain in dB, as [`Self::lowpass_response_db`].
    /// Its peak, at the cutoff, is 1/k: -6 dB with no resonance.
    pub fn bandpass_response_db(cutoff_hz: f32, resonance: f32, freq: f32) -> f32 {
        Self::response_db(cutoff_hz, resonance, freq, |w| w)
    }

    /// Small-signal notch gain in dB, as [`Self::lowpass_response_db`].
    pub fn notch_response_db(cutoff_hz: f32, resonance: f32, freq: f32) -> f32 {
        Self::response_db(cutoff_hz, resonance, freq, |w| (1.0 - w * w).abs())
    }

    /// The gain of an output whose numerator has magnitude `numerator(w)`
    /// at `w` (the frequency over the cutoff), over the denominator all
    /// four share, s^2 + ks + 1.
    fn response_db(cutoff_hz: f32, resonance: f32, freq: f32, numerator: impl Fn(f32) -> f32) -> f32 {
        let w = freq / cutoff_hz.max(1.0);
        let k = Self::small_signal_damping(resonance);
        let denom = (1.0 - w * w).powi(2) + (k * w).powi(2);
        20.0 * numerator(w).max(1e-6).log10() - 10.0 * denom.max(1e-12).log10()
    }

    /// Readout indices: the cutoff (Hz) and resonance it's running at,
    /// after CV, as of the end of the last block.
    pub const READOUT_CUTOFF: usize = 0;
    pub const READOUT_RESONANCE: usize = 1;

    /// The cutoff the filter is currently running at, in Hz, before CV.
    #[cfg(test)]
    fn current_cutoff_hz(&self) -> f32 {
        self.log_cutoff_smooth.current().exp2()
    }
}

impl Default for SvfFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for SvfFilter {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "filter.svf",
            name: "SVF Filter",
            category: ModuleCategory::Filter,
            description: "Multi-mode filter with LP, HP, BP and notch outputs",
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
        // Update sample rate for smoothed parameters
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
        // Set smoothing targets from parameters
        let cutoff_param = params[Self::PARAM_CUTOFF].max(Self::MIN_CUTOFF_HZ);
        self.log_cutoff_smooth.set_target(cutoff_param.log2());
        self.resonance_smooth.set_target(params[Self::PARAM_RESONANCE]);
        self.drive_smooth.set_target(params[Self::PARAM_DRIVE].clamp(1.0, 10.0));
        let cv_depth = params.get(Self::PARAM_CUTOFF_CV).copied().unwrap_or(1.0);
        self.cv_depth_smooth.set_target(cv_depth.clamp(-4.0, 4.0));

        // Get input buffers
        let audio_in = inputs.get(Self::PORT_IN);
        let cutoff_cv = inputs.get(Self::PORT_CUTOFF_CV);
        let res_cv = inputs.get(Self::PORT_RES_CV);

        let [lp_out, hp_out, bp_out, notch_out, ..] = outputs else {
            return;
        };

        let max_cutoff = self.sample_rate * 0.49;

        // Process each sample
        for i in 0..context.block_size {
            // Get smoothed parameter values (per-sample for click-free changes)
            let log_cutoff = self.log_cutoff_smooth.next();
            let base_resonance = self.resonance_smooth.next();
            // Drive is input gain, 1x (clean) to 10x
            let drive = self.drive_smooth.next();

            // Input with drive, plus the noise floor
            let input = audio_in
                .map(|buf| buf.samples.get(i).copied().unwrap_or(0.0))
                .unwrap_or(0.0);
            let input = fast_tanh(input * drive) + self.noise.sample();

            // Cutoff CV adds octaves in the log domain, scaled by its depth
            let cv_depth = self.cv_depth_smooth.next();
            let cutoff_mod = cutoff_cv
                .map(|buf| buf.samples.get(i).copied().unwrap_or(0.0))
                .unwrap_or(0.0)
                * cv_depth;
            let cutoff = (log_cutoff + cutoff_mod)
                .exp2()
                .clamp(Self::MIN_CUTOFF_HZ, max_cutoff);

            // Resonance CV: adds directly to base resonance
            let res_mod = res_cv
                .map(|buf| buf.samples.get(i).copied().unwrap_or(0.0))
                .unwrap_or(0.0);
            let resonance = (base_resonance + res_mod * 0.5).clamp(0.0, 1.0);
            self.running = (cutoff, resonance);

            let g = prewarp(cutoff, self.sample_rate);
            let s1 = self.band_int.state();
            let s2 = self.low_int.state();
            // TPT SVF loop (Zavalishin), solved for the highpass at damping k
            let solve_high = |k: f32| (input - (k + g) * s1 - s2) / (1.0 + g * (k + g));

            // Damping k = DAMPING - feedback * sat(band) / band. The saturator
            // is linearised around an estimate of this sample's bandpass, so
            // the loop solve stays exact (Mystran's "cheap" nonlinear ZDF).
            // The estimate comes from one solve at the previous sample's
            // damping; using the previous bandpass directly instead would
            // delay the nonlinearity by a sample and pull a self-oscillating
            // filter sharp at high cutoffs.
            let feedback = resonance * Self::MAX_FEEDBACK;
            let k_prev = Self::DAMPING - feedback * self.resonance_sat.gain_at(self.last_band);
            let band_estimate = s1 + g * solve_high(k_prev);
            let k = Self::DAMPING - feedback * self.resonance_sat.gain_at(band_estimate);

            // Integrate the solved highpass to get the bandpass and lowpass
            let high = solve_high(k);
            let band = self.band_int.tick(high, g);
            let low = self.low_int.tick(band, g);
            self.last_band = band;

            // Write outputs
            lp_out.samples[i] = low;
            hp_out.samples[i] = high;
            bp_out.samples[i] = band;
            notch_out.samples[i] = input - k * band;
        }
    }

    fn reset(&mut self) {
        self.band_int.reset();
        self.low_int.reset();
        self.last_band = 0.0;
        // Reset smoothed parameters to their current targets
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
        Some(crate::dsp::SpectrumPorts { input: "in", outputs: &["lowpass", "highpass", "bandpass", "notch"] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{amp_to_db, peak, rms, Spectrum};
    use std::f32::consts::PI;

    fn outputs(n: usize) -> Vec<SignalBuffer> {
        (0..4).map(|_| SignalBuffer::audio(n)).collect()
    }

    fn sine(freq: f32, amplitude: f32, sample_rate: f32, n: usize) -> SignalBuffer {
        let mut buf = SignalBuffer::audio(n);
        for i in 0..n {
            buf.samples[i] = amplitude * (2.0 * PI * freq * i as f32 / sample_rate).sin();
        }
        buf
    }

    /// A filter with its smoothing already settled on `params`.
    fn settled_filter(sample_rate: f32, block: usize, params: [f32; 3]) -> SvfFilter {
        let mut filter = SvfFilter::new();
        filter.prepare(sample_rate, block);
        filter.log_cutoff_smooth.set_target(params[0].log2());
        filter.resonance_smooth.set_target(params[1]);
        filter.drive_smooth.set_target(params[2]);
        filter
    }

    #[test]
    fn test_svf_filter_info() {
        let filter = SvfFilter::new();
        assert_eq!(filter.info().id, "filter.svf");
        assert_eq!(filter.info().name, "SVF Filter");
        assert_eq!(filter.info().category, ModuleCategory::Filter);
    }

    #[test]
    fn test_svf_filter_ports() {
        let filter = SvfFilter::new();
        let ports = filter.ports();

        assert_eq!(ports.len(), 7);

        // Input ports
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "cutoff_cv");
        assert_eq!(ports[1].signal_type, SignalType::Control);

        assert!(ports[2].is_input());
        assert_eq!(ports[2].id, "res_cv");
        assert_eq!(ports[2].signal_type, SignalType::Control);

        // Output ports (Notch is appended so existing cables keep their ports)
        let outputs: Vec<_> = ports[3..].iter().map(|p| (p.is_output(), p.id, p.signal_type)).collect();
        assert_eq!(
            outputs,
            vec![
                (true, "lowpass", SignalType::Audio),
                (true, "highpass", SignalType::Audio),
                (true, "bandpass", SignalType::Audio),
                (true, "notch", SignalType::Audio),
            ]
        );
    }

    #[test]
    fn test_svf_filter_parameters() {
        let filter = SvfFilter::new();
        let params = filter.parameters();

        assert_eq!(params.len(), 4);

        // Cutoff parameter
        assert_eq!(params[0].id, "cutoff");
        assert_eq!(params[0].min, 20.0);
        assert_eq!(params[0].max, 20000.0);
        assert_eq!(params[0].default, 1000.0);

        // Resonance parameter
        assert_eq!(params[1].id, "resonance");
        assert_eq!(params[1].min, 0.0);
        assert_eq!(params[1].max, 1.0);
        assert_eq!(params[1].default, 0.5);

        // Drive parameter
        assert_eq!(params[2].id, "drive");
        assert_eq!(params[2].min, 1.0);
        assert_eq!(params[2].max, 10.0);
        assert_eq!(params[2].default, 1.0);

        // Cutoff CV depth: bipolar, and 1 oct/unit unless turned
        assert_eq!(params[3].name, "Cutoff CV");
        assert_eq!((params[3].min, params[3].max, params[3].default), (-4.0, 4.0, 1.0));
    }

    #[test]
    fn test_svf_filter_produces_output() {
        let mut filter = SvfFilter::new();
        filter.prepare(44100.0, 256);

        // Create a simple sine wave input
        let mut input = SignalBuffer::audio(256);
        for i in 0..256 {
            input.samples[i] = (i as f32 * 0.1).sin();
        }

        let mut outputs = outputs(256);
        let ctx = ProcessContext::new(44100.0, 256);

        filter.process(&[&input], &mut outputs, &[1000.0, 0.5, 1.0], &ctx);

        // All outputs should have non-zero values and stay in a sane range
        for (name, output) in ["LP", "HP", "BP", "Notch"].iter().zip(&outputs) {
            assert!(output.samples.iter().any(|&s| s.abs() > 0.001), "{} should produce output", name);
            for &sample in &output.samples {
                assert!((-2.0..=2.0).contains(&sample), "{} sample {} out of range", name, sample);
            }
        }
    }

    #[test]
    fn test_svf_filter_lowpass_attenuates_high_freq() {
        let sample_rate = 44100.0;
        let mut filter = SvfFilter::new();
        filter.prepare(sample_rate, 4410);

        // A high frequency signal (5000 Hz) through a 500 Hz lowpass
        let input = sine(5000.0, 1.0, sample_rate, 4410);
        let mut outputs = outputs(4410);
        let ctx = ProcessContext::new(sample_rate, 4410);
        filter.process(&[&input], &mut outputs, &[500.0, 0.5, 1.0], &ctx);

        // Skip the first ~10ms for filter settling
        let skip = 441;
        let input_rms = rms(&input.samples[skip..]);
        let lp_rms = rms(&outputs[0].samples[skip..]);
        assert!(
            lp_rms < input_rms * 0.5,
            "Lowpass should attenuate high frequencies: input_rms={}, lp_rms={}",
            input_rms,
            lp_rms
        );
    }

    #[test]
    fn test_svf_filter_highpass_attenuates_low_freq() {
        let sample_rate = 44100.0;
        let mut filter = SvfFilter::new();
        filter.prepare(sample_rate, 4410);

        // A low frequency signal (100 Hz) through a 2000 Hz highpass
        let input = sine(100.0, 1.0, sample_rate, 4410);
        let mut outputs = outputs(4410);
        let ctx = ProcessContext::new(sample_rate, 4410);
        filter.process(&[&input], &mut outputs, &[2000.0, 0.5, 1.0], &ctx);

        let skip = 441;
        let input_rms = rms(&input.samples[skip..]);
        let hp_rms = rms(&outputs[1].samples[skip..]);
        assert!(
            hp_rms < input_rms * 0.5,
            "Highpass should attenuate low frequencies: input_rms={}, hp_rms={}",
            input_rms,
            hp_rms
        );
    }

    #[test]
    fn test_svf_filter_reset() {
        let mut filter = SvfFilter::new();
        filter.prepare(44100.0, 256);

        // Process some samples to build up state
        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);
        let mut outs = outputs(256);
        let ctx = ProcessContext::new(44100.0, 256);
        filter.process(&[&input], &mut outs, &[1000.0, 0.5, 1.0], &ctx);

        // Reset should clear internal state
        filter.reset();

        // Process silence - output should be near zero
        let silence = SignalBuffer::audio(256);
        let mut outs = outputs(256);
        filter.process(&[&silence], &mut outs, &[1000.0, 0.5, 1.0], &ctx);
        assert!(
            outs[0].samples[0].abs() < 0.01,
            "Lowpass should be near zero after reset, got {}",
            outs[0].samples[0]
        );
    }

    #[test]
    fn test_svf_filter_stability_high_resonance() {
        let mut filter = SvfFilter::new();
        filter.prepare(44100.0, 4410);

        // A hard square wave, driven, at high resonance
        let mut input = SignalBuffer::audio(4410);
        for i in 0..4410 {
            input.samples[i] = if i % 100 < 50 { 1.0 } else { -1.0 };
        }
        let mut outputs = outputs(4410);
        let ctx = ProcessContext::new(44100.0, 4410);
        filter.process(&[&input], &mut outputs, &[1000.0, 0.95, 10.0], &ctx);

        // Check that no outputs explode (all within reasonable bounds)
        for output in &outputs {
            for &sample in &output.samples {
                assert!(
                    sample.is_finite() && sample.abs() < 10.0,
                    "Filter became unstable: sample={}",
                    sample
                );
            }
        }
    }

    #[test]
    fn test_svf_filter_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SvfFilter>();
    }

    #[test]
    fn test_svf_filter_default() {
        let filter = SvfFilter::default();
        assert_eq!(filter.info().id, "filter.svf");
    }

    #[test]
    fn test_svf_filter_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<SvfFilter>();

        assert!(registry.contains("filter.svf"));

        let module = registry.create("filter.svf");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "filter.svf");
        assert_eq!(module.info().name, "SVF Filter");
        assert_eq!(module.ports().len(), 7);
        assert_eq!(module.parameters().len(), 4);
    }

    /// Steady-state RMS gain of output `port` for a sine at `freq`, with an
    /// optional constant cutoff CV.
    fn gain(port: usize, cutoff: f32, resonance: f32, freq: f32, cutoff_cv: f32) -> f32 {
        gain_at_depth(port, cutoff, resonance, freq, cutoff_cv, 1.0)
    }

    /// `gain`, with the Cutoff CV knob at `depth` octaves per unit.
    fn gain_at_depth(port: usize, cutoff: f32, resonance: f32, freq: f32, cutoff_cv: f32, depth: f32) -> f32 {
        let sample_rate = 44100.0;
        let n = 8820;
        let params = [cutoff, resonance, 1.0, depth];
        let mut filter = settled_filter(sample_rate, n, [cutoff, resonance, 1.0]);
        filter.cv_depth_smooth.reset(depth);
        let input = sine(freq, 0.1, sample_rate, n);
        let mut cv = SignalBuffer::control(n);
        cv.fill(cutoff_cv);
        let mut outputs = outputs(n);
        let ctx = ProcessContext::new(sample_rate, n);
        filter.process(&[&input, &cv], &mut outputs, &params, &ctx);
        let skip = n / 2;
        rms(&outputs[port].samples[skip..]) / rms(&input.samples[skip..])
    }

    fn lowpass_gain(cutoff: f32, freq: f32) -> f32 {
        // Resonance 0 => k = 2 (-6 dB at cutoff); drive 1 => near unity
        gain(0, cutoff, 0.0, freq, 0.0)
    }

    #[test]
    fn test_svf_cutoff_reaches_high_frequencies() {
        // The old Chamberlin implementation topped out near 6.5 kHz. A 12 kHz
        // cutoff must now pass an 8 kHz tone almost untouched...
        let pass = lowpass_gain(12000.0, 8000.0);
        assert!(pass > 0.7, "8 kHz should pass a 12 kHz lowpass, gain={}", pass);
        // ...while a 2 kHz cutoff attenuates it strongly (12 dB/oct, 2 octaves).
        let stop = lowpass_gain(2000.0, 8000.0);
        assert!(stop < 0.1, "8 kHz should be cut by a 2 kHz lowpass, gain={}", stop);
    }

    #[test]
    fn test_svf_gain_at_cutoff() {
        // With k = 2 the 2-pole response is -6 dB (0.5) at the cutoff frequency.
        for &fc in &[100.0, 1000.0, 10000.0] {
            let g = lowpass_gain(fc, fc);
            assert!((g - 0.5).abs() < 0.05, "gain at cutoff {} Hz = {}", fc, g);
        }
    }

    #[test]
    fn test_damping_curve() {
        assert_eq!(SvfFilter::small_signal_damping(0.0), 2.0);
        // The default sits close to where the old linear map put it (k ≈ 1)
        assert!((SvfFilter::small_signal_damping(0.5) - 1.0).abs() < 0.1);
        // Damping is cancelled only at the very top of the knob
        assert!(SvfFilter::small_signal_damping(0.95) > 0.0);
        assert!(SvfFilter::small_signal_damping(1.0) < 0.0);
    }

    #[test]
    fn test_notch_nulls_at_cutoff() {
        for &(fc, res) in &[(1000.0, 0.0), (1000.0, 0.5), (250.0, 0.8), (6000.0, 0.5)] {
            let null = amp_to_db(gain(3, fc, res, fc, 0.0));
            assert!(null < -30.0, "notch at {} Hz, res {}: {:.1} dB", fc, res, null);
            // ...and passes everything two octaves or more away
            for f in [fc / 4.0, fc * 4.0] {
                let pass = amp_to_db(gain(3, fc, res, f, 0.0));
                assert!(pass > -1.5, "notch at {} Hz passes {} Hz at {:.1} dB", fc, f, pass);
            }
        }
    }

    #[test]
    fn test_notch_is_lowpass_plus_highpass() {
        let mut filter = settled_filter(44100.0, 512, [800.0, 0.7, 1.0]);
        let input = sine(500.0, 0.5, 44100.0, 512);
        let mut outs = outputs(512);
        let ctx = ProcessContext::new(44100.0, 512);
        filter.process(&[&input], &mut outs, &[800.0, 0.7, 1.0], &ctx);
        for i in 0..512 {
            let sum = outs[0].samples[i] + outs[1].samples[i];
            assert!((outs[3].samples[i] - sum).abs() < 1e-5, "sample {}", i);
        }
    }

    #[test]
    fn test_cutoff_cv_is_in_octaves() {
        // +1 CV is +1 octave, the V/Oct scale: a 500 Hz cutoff behaves like 1 kHz...
        let up = gain(0, 500.0, 0.0, 1000.0, 1.0);
        assert!((up - 0.5).abs() < 0.05, "gain at the modulated cutoff = {}", up);
        // ...and -1 CV is -1 octave, the same distance down
        let down = gain(0, 2000.0, 0.0, 1000.0, -1.0);
        assert!((down - 0.5).abs() < 0.05, "gain at the modulated cutoff = {}", down);
    }

    #[test]
    fn test_cutoff_cv_depth_scales_the_octaves() {
        // At 3 oct/unit, +1 CV takes a 250 Hz cutoff three octaves up to 2 kHz
        let up = gain_at_depth(0, 250.0, 0.0, 2000.0, 1.0, 3.0);
        assert!((up - 0.5).abs() < 0.05, "gain at the modulated cutoff = {}", up);
        // A negative depth turns the CV upside down: +1 CV at -2 closes 4 kHz to 1 kHz
        let down = gain_at_depth(0, 4000.0, 0.0, 1000.0, 1.0, -2.0);
        assert!((down - 0.5).abs() < 0.05, "gain at the modulated cutoff = {}", down);
        // At 0 the CV does nothing
        let none = gain_at_depth(0, 1000.0, 0.0, 1000.0, 1.0, 0.0);
        assert!((none - 0.5).abs() < 0.05, "gain at the unmodulated cutoff = {}", none);
    }

    #[test]
    fn test_cv_depth_multiplies_the_cv() {
        // Depth 2 sounds exactly like the CV doubled by hand into depth 1,
        // sample for sample: the knob is a gain on the CV and nothing else
        let n = 4096;
        let input = sine(220.0, 0.8, 44100.0, n);
        let mut cv = SignalBuffer::control(n);
        for i in 0..n {
            cv.samples[i] = (i as f32 / n as f32 * 7.0).sin() * 1.7;
        }
        let mut doubled = cv.clone();
        doubled.samples.iter_mut().for_each(|s| *s *= 2.0);
        let ctx = ProcessContext::new(44100.0, n);
        let lowpass = |cv: &SignalBuffer, depth: f32| {
            let mut filter = SvfFilter::new();
            filter.prepare(44100.0, n);
            filter.cv_depth_smooth.reset(depth);
            let mut outs = outputs(n);
            filter.process(&[&input, cv], &mut outs, &[700.0, 0.6, 2.0, depth], &ctx);
            outs.swap_remove(0).samples
        };
        assert_eq!(lowpass(&cv, 2.0), lowpass(&doubled, 1.0));
    }

    #[test]
    fn test_cv_depth_scales_each_voice_on_its_own() {
        use crate::dsp::poly::Poly;
        // Two voices share a 2 kHz tone, each with its own envelope level on
        // Cutoff. At 3 oct/unit the voice at 1.0 opens from 250 Hz to 2 kHz
        // and passes it; the voice at 0.0 stays at 250 Hz and cuts it
        let sample_rate = 44100.0;
        let n = 2048;
        let mut filter = Poly::new(SvfFilter::new);
        filter.prepare(sample_rate, n);
        let input = sine(2000.0, 0.1, sample_rate, n);
        let mut cv = SignalBuffer::polyphonic(n, SignalType::Control);
        cv.set_channels(2);
        cv.channel_mut(1).fill(1.0);
        let mut outs: Vec<_> = (0..4).map(|_| SignalBuffer::polyphonic(n, SignalType::Audio)).collect();
        let ctx = ProcessContext::new(sample_rate, n);
        // Let the cutoff and depth smoothing settle, then measure
        for _ in 0..4 {
            filter.process(&[&input, &cv], &mut outs, &[250.0, 0.0, 1.0, 3.0], &ctx);
        }
        let level = |voice: usize| rms(&outs[0].voice(voice).samples[n / 2..]) / rms(&input.samples[n / 2..]);
        assert_eq!(outs[0].channels(), 2);
        assert!((level(1) - 0.5).abs() < 0.05, "opened voice: gain {}", level(1));
        assert!(level(0) < 0.03, "closed voice: gain {}", level(0));
    }

    /// Samples taken for the knob's cutoff to pass the geometric midpoint
    /// between `from` and `to`.
    fn samples_to_midpoint(from: f32, to: f32) -> usize {
        let sample_rate = 48000.0;
        let mut filter = settled_filter(sample_rate, 64, [from, 0.0, 1.0]);
        let silence = SignalBuffer::audio(64);
        let mut outs = outputs(64);
        let ctx = ProcessContext::new(sample_rate, 1);
        let mid = (from * to).sqrt();
        for n in 1..48000 {
            filter.process(&[&silence], &mut outs, &[to, 0.0, 1.0], &ctx);
            let fc = filter.current_cutoff_hz();
            if (from < to && fc >= mid) || (from > to && fc <= mid) {
                return n;
            }
        }
        panic!("cutoff never reached {} Hz", mid);
    }

    #[test]
    fn test_cutoff_sweeps_evenly_in_octaves() {
        // Up six octaves and down six octaves cross the middle octave at the
        // same moment. Smoothing in Hz would race through the bass on the way
        // up (reaching 800 Hz almost at once) and crawl on the way down.
        let up = samples_to_midpoint(100.0, 6400.0);
        let down = samples_to_midpoint(6400.0, 100.0);
        assert!(up.abs_diff(down) <= 1, "up {} samples, down {} samples", up, down);
        // The smoother still takes its ~10 ms time constant to get there
        assert!(up > 48000 / 200, "midpoint after only {} samples", up);
    }

    /// Runs a filter with nothing patched into its input and returns the
    /// lowpass output after `settle` seconds, `n` samples long.
    fn free_running(cutoff: f32, resonance: f32, settle: f32, n: usize) -> Vec<f32> {
        let sample_rate = 48000.0;
        let block = 256;
        let mut filter = settled_filter(sample_rate, block, [cutoff, resonance, 1.0]);
        let silence = SignalBuffer::unconnected(block, SignalType::Audio);
        let mut outs = outputs(block);
        let ctx = ProcessContext::new(sample_rate, block);
        let skip = (settle * sample_rate) as usize;
        let mut lp = Vec::with_capacity(n);
        let mut done = 0;
        while lp.len() < n {
            filter.process(&[&silence], &mut outs, &[cutoff, resonance, 1.0], &ctx);
            for &s in &outs[0].samples {
                if done >= skip && lp.len() < n {
                    lp.push(s);
                }
                done += 1;
            }
        }
        lp
    }

    #[test]
    fn test_self_oscillates_at_max_resonance() {
        for &fc in &[110.0, 880.0, 3000.0, 8000.0] {
            let out = free_running(fc, 1.0, 1.5, 32768);
            let level = rms(&out);
            assert!(level > 0.1, "{} Hz: self-oscillation too quiet ({:.3} rms)", fc, level);
            assert!(peak(&out) < 1.0, "{} Hz: self-oscillation peaks at {}", fc, peak(&out));

            let spectrum = Spectrum::of(&out, 48000.0);
            let f = spectrum.dominant_frequency();
            let cents = 1200.0 * (f / fc as f64).log2();
            assert!(cents.abs() < 3.0, "{} Hz: oscillates at {:.2} Hz ({:+.1} cents)", fc, f, cents);
        }
    }

    #[test]
    fn test_just_below_max_resonance_rings_out() {
        // At 0.9 the filter is very resonant but not oscillating: with nothing
        // patched in it stays at the noise floor.
        let out = free_running(880.0, 0.9, 1.0, 4800);
        assert!(peak(&out) < 1e-3, "res 0.9 should not self-oscillate, peak {}", peak(&out));
    }

    #[test]
    fn test_saturation_bounds_a_loud_resonant_input() {
        // A full-scale sine right at a high-resonance cutoff would reach about
        // 25x linearly (Q ≈ 25). The resonance saturator limits it.
        let sample_rate = 48000.0;
        let n = 24000;
        let mut filter = settled_filter(sample_rate, n, [1000.0, 0.95, 1.0]);
        let input = sine(1000.0, 1.0, sample_rate, n);
        let mut outs = outputs(n);
        let ctx = ProcessContext::new(sample_rate, n);
        filter.process(&[&input], &mut outs, &[1000.0, 0.95, 1.0], &ctx);
        for (name, out) in ["LP", "HP", "BP", "Notch"].iter().zip(&outs) {
            assert!(peak(&out.samples) < 2.5, "{} peaks at {}", name, peak(&out.samples));
        }
    }

    #[test]
    fn test_response_curve_matches_the_filter() {
        // The node's drawn response agrees with the measured gain
        for &(fc, res, f) in &[(1000.0, 0.0, 1000.0), (1000.0, 0.5, 1000.0), (500.0, 0.7, 2000.0), (2000.0, 0.3, 700.0)] {
            let drawn = SvfFilter::lowpass_response_db(fc, res, f);
            let measured = amp_to_db(gain(0, fc, res, f, 0.0));
            assert!((drawn - measured).abs() < 0.75, "fc {} res {} at {} Hz: drawn {:.2} dB, measured {:.2} dB", fc, res, f, drawn, measured);
        }
    }

    #[test]
    fn test_every_outputs_curve_matches_the_filter() {
        // HP, BP and Notch, drawn as the filter plays them. The notch's
        // floor, right at the cutoff, is left out: only its walls are checked
        let curves: [(usize, fn(f32, f32, f32) -> f32); 3] = [
            (1, SvfFilter::highpass_response_db),
            (2, SvfFilter::bandpass_response_db),
            (3, SvfFilter::notch_response_db),
        ];
        for (port, curve) in curves {
            for &(fc, res, f) in &[(1000.0, 0.0, 1000.0), (1000.0, 0.5, 1000.0), (500.0, 0.7, 2000.0), (2000.0, 0.3, 700.0), (1000.0, 0.3, 300.0)] {
                if port == 3 && (f / fc - 1.0f32).abs() < 0.2 {
                    continue;
                }
                let drawn = curve(fc, res, f);
                let measured = amp_to_db(gain(port, fc, res, f, 0.0));
                assert!((drawn - measured).abs() < 0.75, "port {port}, fc {fc} res {res} at {f} Hz: drawn {drawn:.2} dB, measured {measured:.2} dB");
            }
        }
    }

    #[test]
    fn test_readout_reports_the_cutoff_after_cv() {
        let n = 512;
        let mut filter = settled_filter(44100.0, n, [500.0, 0.2, 1.0]);
        let input = SignalBuffer::audio(n);
        let mut cv = SignalBuffer::control(n);
        cv.fill(1.0);
        let res_cv = SignalBuffer::control(n);
        let mut outs = outputs(n);
        filter.process(&[&input, &cv, &res_cv], &mut outs, &[500.0, 0.2, 1.0, 1.0], &ProcessContext::new(44100.0, n));
        let values = filter.readout(&[]).unwrap().values;
        assert!((values[SvfFilter::READOUT_CUTOFF] - 1000.0).abs() < 1.0, "{} Hz", values[SvfFilter::READOUT_CUTOFF]);
        assert!((values[SvfFilter::READOUT_RESONANCE] - 0.2).abs() < 1e-4);
    }

    #[test]
    fn test_svf_filter_drive() {
        // Use two separate filter instances to avoid smoothing interference
        let mut filter1 = SvfFilter::new();
        filter1.prepare(44100.0, 256);
        let mut filter2 = SvfFilter::new();
        filter2.prepare(44100.0, 256);

        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);
        let ctx = ProcessContext::new(44100.0, 256);

        // Process multiple times to let parameter smoothing settle
        let mut outputs1 = outputs(256);
        for _ in 0..20 {
            filter1.process(&[&input], &mut outputs1, &[1000.0, 0.5, 1.0], &ctx);
        }
        let mut outputs2 = outputs(256);
        for _ in 0..20 {
            filter2.process(&[&input], &mut outputs2, &[1000.0, 0.5, 5.5], &ctx);
        }

        // With higher drive, the output should be louder (until saturation)
        let rms1 = rms(&outputs1[0].samples[200..]);
        let rms2 = rms(&outputs2[0].samples[200..]);
        assert!(
            rms2 > rms1,
            "Higher drive should produce louder output: rms1={}, rms2={}",
            rms1,
            rms2
        );
    }

    #[test]
    fn test_self_oscillation_renders_through_the_patch() {
        use crate::engine::OfflineRenderer;
        use crate::persistence::{ConnectionData, NamedParameter, NodeData, ParameterValue, Patch};

        // A lone filter at full resonance into the output: it should sing
        let mut patch = Patch::new("self-oscillation");
        let mut filter = NodeData::new(1, "filter.svf", (0.0, 0.0));
        filter.parameters = vec![
            NamedParameter::new("Cutoff", ParameterValue::Frequency(440.0)),
            NamedParameter::new("Resonance", ParameterValue::Number(1.0)),
        ];
        patch.nodes.push(filter);
        patch.nodes.push(NodeData::new(2, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "LowPass", 2, "Mono"));

        let sr = 48000.0;
        let (mut r, compiled) = OfflineRenderer::from_patch(&patch, sr, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        r.render_seconds(1.5);
        let out = r.render(65536);

        assert!(rms(&out.left) > 0.05, "should be audible, rms {}", rms(&out.left));
        let spectrum = Spectrum::of(&out.left, sr);
        let f = spectrum.dominant_frequency();
        let cents = 1200.0 * (f / 440.0).log2();
        assert!(cents.abs() < 3.0, "oscillates at {:.2} Hz ({:+.1} cents)", f, cents);
        // A sine: almost all the energy is in the fundamental, little in harmonics
        let fundamental_bin = (f / spectrum.bin_hz).round() as usize;
        let energy = |range: std::ops::Range<usize>| -> f64 {
            spectrum.magnitudes[range].iter().map(|m| m * m).sum()
        };
        let total = energy(1..spectrum.magnitudes.len());
        let fundamental = energy(fundamental_bin - 4..fundamental_bin + 5);
        let thd_db = 10.0 * ((total - fundamental).max(1e-30) / fundamental).log10();
        assert!(thd_db < -30.0, "self-oscillation should be close to a sine, THD {:.1} dB", thd_db);
    }
}
