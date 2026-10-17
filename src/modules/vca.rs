//! VCA (Voltage Controlled Amplifier) module.
//!
//! Multiplies an audio signal by a control voltage, enabling envelope-controlled
//! amplitude shaping, tremolo effects, and other amplitude modulation.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// The module's ID, as patches name it.
pub const ID: &str = "util.vca";

/// A Voltage Controlled Amplifier.
///
/// Multiplies an audio signal by a control voltage, allowing envelopes,
/// LFOs, or other CV sources to shape the amplitude of sounds.
///
/// # Ports
///
/// - **In** (Audio, Input): The audio signal to be amplitude-controlled.
/// - **CV** (Control, Input): Control voltage for amplitude (0.0-1.0 typical).
/// - **Out** (Audio, Output): The amplitude-shaped audio output.
///
/// # Parameters
///
/// - **Level** (0-1): Base amplitude level when CV is not connected or at maximum.
/// - **CV Amount** (0-1): How much the CV input affects the amplitude.
///
/// # Signal Flow
///
/// ```text
/// Output = Input × (Level × (1 - CV_Amount) + CV × CV_Amount × Level)
/// ```
///
/// Simplified: when CV_Amount = 1.0, Output = Input × CV × Level
pub struct Vca {
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
    /// Smoothed level parameter.
    level_smooth: SmoothedValue,
    /// Smoothed CV amount parameter.
    cv_amount_smooth: SmoothedValue,
}

impl Vca {
    /// Creates a new VCA.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            sample_rate,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in", "In", SignalType::Audio, 0.0).describe("Audio to be shaped"),
                PortDefinition::input_with_default("cv", "CV", SignalType::Control, 1.0).describe("Level control from 0 to 1; unpatched means full level"),
                // Output port
                PortDefinition::output("out", "Out", SignalType::Audio).describe("The input scaled by level and CV"),
            ],
            parameters: vec![
                // Level - base amplitude
                ParameterDefinition::new(
                    "level",
                    "Level",
                    0.0,
                    1.0,
                    1.0, // Full level by default
                    ParameterDisplay::linear(""),
                ).describe("Overall output level"),
                // CV Amount - how much CV affects amplitude
                ParameterDefinition::new(
                    "cv_amount",
                    "CV Amount",
                    0.0,
                    1.0,
                    1.0, // Full CV control by default
                    ParameterDisplay::linear(""),
                ).describe("How much the CV input controls the level; 0 ignores it"),
            ],
            // Initialize smoothed parameters
            level_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            cv_amount_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
        }
    }

    /// Port index constants.
    const PORT_IN: usize = 0;
    const PORT_CV: usize = 1;
    const PORT_OUT: usize = 0;

    /// Parameter index constants.
    const PARAM_LEVEL: usize = 0;
    const PARAM_CV_AMOUNT: usize = 1;
}

impl Default for Vca {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Vca {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "VCA",
            category: ModuleCategory::Utility,
            description: "Voltage controlled amplifier for amplitude shaping",
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
        self.level_smooth.set_sample_rate(sample_rate);
        self.cv_amount_smooth.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Set smoothing targets from parameters
        self.level_smooth.set_target(params[Self::PARAM_LEVEL]);
        self.cv_amount_smooth.set_target(params[Self::PARAM_CV_AMOUNT]);

        // Get input buffers
        let audio_in = inputs.get(Self::PORT_IN);
        let cv_in = inputs.get(Self::PORT_CV);

        // Get output buffer
        let output = &mut outputs[Self::PORT_OUT];

        // Process each sample
        for i in 0..context.block_size {
            // Get smoothed parameter values (per-sample for click-free changes)
            let level = self.level_smooth.next();
            let cv_amount = self.cv_amount_smooth.next();

            // Get audio input
            let audio = audio_in
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);

            // Get CV input (defaults to 1.0 when not connected)
            let cv = cv_in
                .map(|buf| buf.samples.get(i).copied().unwrap_or(1.0))
                .unwrap_or(1.0);

            // Clamp CV to 0-1 range for typical VCA behavior
            let cv = cv.clamp(0.0, 1.0);

            // Calculate amplitude:
            // - When cv_amount = 0: amplitude = level (CV ignored)
            // - When cv_amount = 1: amplitude = level * cv (full CV control)
            // This allows smooth blending between manual and CV control
            let amplitude = level * (1.0 - cv_amount + cv * cv_amount);

            // Apply amplitude to audio
            output.samples[i] = audio * amplitude;
        }
    }

    fn reset(&mut self) {
        // Reset smoothed parameters to their current targets
        self.level_smooth.reset(self.level_smooth.target());
        self.cv_amount_smooth.reset(self.cv_amount_smooth.target());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vca_info() {
        let vca = Vca::new();
        assert_eq!(vca.info().id, "util.vca");
        assert_eq!(vca.info().name, "VCA");
        assert_eq!(vca.info().category, ModuleCategory::Utility);
    }

    #[test]
    fn test_vca_ports() {
        let vca = Vca::new();
        let ports = vca.ports();

        assert_eq!(ports.len(), 3);

        // Audio input
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        // CV input
        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "cv");
        assert_eq!(ports[1].signal_type, SignalType::Control);

        // Audio output
        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "out");
        assert_eq!(ports[2].signal_type, SignalType::Audio);
    }

    #[test]
    fn test_vca_parameters() {
        let vca = Vca::new();
        let params = vca.parameters();

        assert_eq!(params.len(), 2);

        // Level
        assert_eq!(params[0].id, "level");
        assert_eq!(params[0].min, 0.0);
        assert_eq!(params[0].max, 1.0);
        assert_eq!(params[0].default, 1.0);

        // CV Amount
        assert_eq!(params[1].id, "cv_amount");
        assert_eq!(params[1].min, 0.0);
        assert_eq!(params[1].max, 1.0);
        assert_eq!(params[1].default, 1.0);
    }

    #[test]
    fn test_vca_passthrough_no_cv() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // Audio input
        let mut audio_in = SignalBuffer::audio(256);
        for i in 0..256 {
            audio_in.samples[i] = (i as f32 / 256.0) * 2.0 - 1.0; // Ramp -1 to 1
        }

        // No CV connected (defaults to 1.0)
        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Level = 1.0, CV Amount = 1.0
        vca.process(&[&audio_in], &mut outputs, &[1.0, 1.0], &ctx);

        // Output should equal input (CV defaults to 1.0)
        for i in 0..256 {
            assert!(
                (outputs[0].samples[i] - audio_in.samples[i]).abs() < 0.0001,
                "Sample {} should pass through unchanged",
                i
            );
        }
    }

    #[test]
    fn test_vca_silence_with_zero_cv() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // Audio input
        let mut audio_in = SignalBuffer::audio(256);
        audio_in.fill(0.5);

        // CV = 0
        let mut cv_in = SignalBuffer::control(256);
        cv_in.fill(0.0);

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Level = 1.0, CV Amount = 1.0
        vca.process(&[&audio_in, &cv_in], &mut outputs, &[1.0, 1.0], &ctx);

        // Output should be silent
        for &sample in &outputs[0].samples {
            assert!(
                sample.abs() < 0.0001,
                "Output should be silent with CV=0, got {}",
                sample
            );
        }
    }

    #[test]
    fn test_vca_half_amplitude() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // Audio input
        let mut audio_in = SignalBuffer::audio(256);
        audio_in.fill(1.0);

        // CV = 0.5
        let mut cv_in = SignalBuffer::control(256);
        cv_in.fill(0.5);

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Level = 1.0, CV Amount = 1.0
        vca.process(&[&audio_in, &cv_in], &mut outputs, &[1.0, 1.0], &ctx);

        // Output should be 0.5
        for &sample in &outputs[0].samples {
            assert!(
                (sample - 0.5).abs() < 0.0001,
                "Output should be 0.5, got {}",
                sample
            );
        }
    }

    #[test]
    fn test_vca_level_parameter() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // Audio input
        let mut audio_in = SignalBuffer::audio(256);
        audio_in.fill(1.0);

        // No CV connected
        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process multiple times to let parameter smoothing settle
        for _ in 0..20 {
            vca.process(&[&audio_in], &mut outputs, &[0.5, 0.0], &ctx);
        }

        // Output should be 0.5 (level controls amplitude) - check last samples
        for i in 200..256 {
            assert!(
                (outputs[0].samples[i] - 0.5).abs() < 0.01,
                "Output should be 0.5, got {}",
                outputs[0].samples[i]
            );
        }
    }

    #[test]
    fn test_vca_cv_amount_blend() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // Audio input
        let mut audio_in = SignalBuffer::audio(256);
        audio_in.fill(1.0);

        // CV = 0.0 (would make silent if fully applied)
        let mut cv_in = SignalBuffer::control(256);
        cv_in.fill(0.0);

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process multiple times to let parameter smoothing settle
        for _ in 0..20 {
            vca.process(&[&audio_in, &cv_in], &mut outputs, &[1.0, 0.5], &ctx);
        }

        // amplitude = level * (1.0 - cv_amount + cv * cv_amount)
        //           = 1.0 * (1.0 - 0.5 + 0.0 * 0.5) = 0.5 (check last samples)
        for i in 200..256 {
            assert!(
                (outputs[0].samples[i] - 0.5).abs() < 0.01,
                "Output should be 0.5 with 50% CV blend, got {}",
                outputs[0].samples[i]
            );
        }
    }

    #[test]
    fn test_vca_envelope_shape() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 100);

        // Constant audio input
        let mut audio_in = SignalBuffer::audio(100);
        audio_in.fill(1.0);

        // Ramping CV (simulating envelope attack)
        let mut cv_in = SignalBuffer::control(100);
        for i in 0..100 {
            cv_in.samples[i] = i as f32 / 99.0; // 0 to 1
        }

        let mut outputs = vec![SignalBuffer::audio(100)];
        let ctx = ProcessContext::new(44100.0, 100);

        vca.process(&[&audio_in, &cv_in], &mut outputs, &[1.0, 1.0], &ctx);

        // Output should follow the CV ramp
        for i in 0..100 {
            let expected = i as f32 / 99.0;
            assert!(
                (outputs[0].samples[i] - expected).abs() < 0.0001,
                "Sample {} should be {}, got {}",
                i,
                expected,
                outputs[0].samples[i]
            );
        }
    }

    #[test]
    fn test_vca_cv_clamping() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // Audio input
        let mut audio_in = SignalBuffer::audio(256);
        audio_in.fill(1.0);

        // CV with out-of-range values
        let mut cv_in = SignalBuffer::control(256);
        for i in 0..128 {
            cv_in.samples[i] = -0.5; // Negative (should clamp to 0)
        }
        for i in 128..256 {
            cv_in.samples[i] = 1.5; // Over 1 (should clamp to 1)
        }

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        vca.process(&[&audio_in, &cv_in], &mut outputs, &[1.0, 1.0], &ctx);

        // First half: CV clamped to 0, output should be 0
        for i in 0..128 {
            assert!(
                outputs[0].samples[i].abs() < 0.0001,
                "Sample {} should be 0 (CV clamped from negative)",
                i
            );
        }

        // Second half: CV clamped to 1, output should be 1
        for i in 128..256 {
            assert!(
                (outputs[0].samples[i] - 1.0).abs() < 0.0001,
                "Sample {} should be 1 (CV clamped from >1)",
                i
            );
        }
    }

    #[test]
    fn test_vca_preserves_audio_shape() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // Sine wave input
        let mut audio_in = SignalBuffer::audio(256);
        for i in 0..256 {
            audio_in.samples[i] = (i as f32 * 0.1).sin();
        }

        // Constant CV
        let mut cv_in = SignalBuffer::control(256);
        cv_in.fill(0.5);

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        vca.process(&[&audio_in, &cv_in], &mut outputs, &[1.0, 1.0], &ctx);

        // Output should be input * 0.5
        for i in 0..256 {
            let expected = audio_in.samples[i] * 0.5;
            assert!(
                (outputs[0].samples[i] - expected).abs() < 0.0001,
                "Sample {} should preserve shape",
                i
            );
        }
    }

    #[test]
    fn test_vca_no_audio_input() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        // No audio input (defaults to 0)
        let mut cv_in = SignalBuffer::control(256);
        cv_in.fill(1.0);

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        vca.process(&[&SignalBuffer::audio(0), &cv_in], &mut outputs, &[1.0, 1.0], &ctx);

        // Output should be silent (0 * anything = 0)
        for &sample in &outputs[0].samples {
            assert_eq!(sample, 0.0, "Output should be silent with no audio input");
        }
    }

    #[test]
    fn test_vca_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Vca>();
    }

    #[test]
    fn test_vca_default() {
        let vca = Vca::default();
        assert_eq!(vca.info().id, "util.vca");
    }

    #[test]
    fn test_vca_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Vca>();

        assert!(registry.contains("util.vca"));

        let module = registry.create("util.vca");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "util.vca");
        assert_eq!(module.info().name, "VCA");
        assert_eq!(module.ports().len(), 3);
        assert_eq!(module.parameters().len(), 2);
    }

    #[test]
    fn test_vca_level_and_cv_combined() {
        let mut vca = Vca::new();
        vca.prepare(44100.0, 256);

        let mut audio_in = SignalBuffer::audio(256);
        audio_in.fill(1.0);

        let mut cv_in = SignalBuffer::control(256);
        cv_in.fill(0.5);

        let mut outputs = vec![SignalBuffer::audio(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process multiple times to let parameter smoothing settle
        for _ in 0..20 {
            vca.process(&[&audio_in, &cv_in], &mut outputs, &[0.5, 1.0], &ctx);
        }

        // Level = 0.5, CV = 0.5, CV Amount = 1.0
        // amplitude = level * cv = 0.5 * 0.5 = 0.25 (check last samples)
        for i in 200..256 {
            assert!(
                (outputs[0].samples[i] - 0.25).abs() < 0.01,
                "Output should be 0.25, got {}",
                outputs[0].samples[i]
            );
        }
    }
}
