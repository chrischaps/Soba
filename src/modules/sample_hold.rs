//! Sample and Hold module.
//!
//! Captures the input signal value when triggered and holds that value
//! until the next trigger. Essential for creating stepped random sequences,
//! staircase LFO patterns, and quantized modulation.

use crate::dsp::{
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

/// The module's ID, as patches name it.
pub const ID: &str = "util.sample_hold";

/// A Sample and Hold module.
///
/// Samples the input signal on rising edge of the trigger and holds
/// that value until the next trigger. Optionally applies slew (glide)
/// to smooth transitions between sampled values.
///
/// # Use Cases
///
/// - LFO → S&H → Filter Cutoff = stepped random modulation
/// - Noise → S&H (clocked) = random sequence generator
/// - Slow LFO → S&H (fast clock) = staircase pattern
///
/// # Ports
///
/// **Inputs:**
/// - **In** (Control): Signal to sample.
/// - **Trigger** (Gate): Samples input on rising edge.
///
/// **Outputs:**
/// - **Out** (Control): Held (and optionally slewed) value.
///
/// # Parameters
///
/// - **Slew** (0-1s): Time to glide from current value to newly sampled value.
///   At 0, values change instantly. Higher values create smooth portamento-like
///   transitions between sampled values.
pub struct SampleHold {
    /// The currently held value (target for slew).
    held_value: f32,
    /// Current output value (may be slewing toward held_value).
    current_value: f32,
    /// The trigger input, for its rising edges.
    trigger: Edge,
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl SampleHold {
    /// Creates a new Sample and Hold module.
    pub fn new() -> Self {
        Self {
            held_value: 0.0,
            current_value: 0.0,
            trigger: Edge::LOW,
            sample_rate: 44100.0,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in", "In", SignalType::Control, 0.0).describe("Signal to sample, such as noise or an LFO"),
                PortDefinition::input_with_default("trigger", "Trig", SignalType::Gate, 0.0).describe("Each rising edge captures the input value"),
                // Output port
                PortDefinition::output("out", "Out", SignalType::Control).describe("The held value, steady until the next trigger"),
            ],
            parameters: vec![
                // Slew time in seconds (0-1s)
                ParameterDefinition::new(
                    "slew",
                    "Slew",
                    0.0,
                    1.0,
                    0.0, // Default: no slew (instant)
                    ParameterDisplay::linear("s"),
                ).describe("Glide time to each new value; 0 jumps instantly"),
            ],
        }
    }

    /// Port index constants.
    const PORT_IN: usize = 0;
    const PORT_TRIGGER: usize = 1;
    const PORT_OUT: usize = 0;

    /// Parameter index constants.
    const PARAM_SLEW: usize = 0;
}

impl Default for SampleHold {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for SampleHold {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Sample & Hold",
            category: ModuleCategory::Utility,
            description: "Sample input on trigger, hold until next trigger",
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
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let slew_time = params[Self::PARAM_SLEW];

        // Get input buffers
        let signal_in = inputs.get(Self::PORT_IN);
        let trigger_in = inputs.get(Self::PORT_TRIGGER);

        // Get output buffer
        let output = &mut outputs[Self::PORT_OUT];

        // Calculate slew rate (value change per sample)
        // If slew_time is 0, we change instantly
        // Otherwise, we calculate how much to change per sample to complete
        // a full 0->1 transition in slew_time seconds
        let slew_rate = if slew_time > 0.0 {
            1.0 / (slew_time * self.sample_rate)
        } else {
            f32::INFINITY // Instant change
        };

        // Process each sample
        for i in 0..context.block_size {
            // Get current input signal value
            let input_value = signal_in.map_or(0.0, |buf| buf.at(i));

            // Get trigger value and detect rising edge
            let trigger_rising = self.trigger.rising(trigger_in.map_or(0.0, |buf| buf.at(i)));

            // On rising edge, sample the input
            if trigger_rising {
                self.held_value = input_value;
            }

            // Apply slew toward held value
            if slew_rate == f32::INFINITY {
                // Instant change
                self.current_value = self.held_value;
            } else {
                // Linear slew toward target
                let diff = self.held_value - self.current_value;
                if diff.abs() <= slew_rate {
                    // Close enough, snap to target
                    self.current_value = self.held_value;
                } else if diff > 0.0 {
                    // Slew up
                    self.current_value += slew_rate;
                } else {
                    // Slew down
                    self.current_value -= slew_rate;
                }
            }

            // Output the current (potentially slewing) value
            output.samples[i] = self.current_value;
        }
    }

    fn reset(&mut self) {
        self.held_value = 0.0;
        self.current_value = 0.0;
        self.trigger.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_hold_info() {
        let sh = SampleHold::new();
        assert_eq!(sh.info().id, "util.sample_hold");
        assert_eq!(sh.info().name, "Sample & Hold");
        assert_eq!(sh.info().category, ModuleCategory::Utility);
    }

    #[test]
    fn test_sample_hold_ports() {
        let sh = SampleHold::new();
        let ports = sh.ports();

        // 2 inputs + 1 output = 3 ports
        assert_eq!(ports.len(), 3);

        // Signal input
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in");
        assert_eq!(ports[0].signal_type, SignalType::Control);

        // Trigger input
        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "trigger");
        assert_eq!(ports[1].signal_type, SignalType::Gate);

        // Output
        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "out");
        assert_eq!(ports[2].signal_type, SignalType::Control);
    }

    #[test]
    fn test_sample_hold_parameters() {
        let sh = SampleHold::new();
        let params = sh.parameters();

        assert_eq!(params.len(), 1);

        // Slew parameter
        assert_eq!(params[0].id, "slew");
        assert!((params[0].min - 0.0).abs() < f32::EPSILON);
        assert!((params[0].max - 1.0).abs() < f32::EPSILON);
        assert!((params[0].default - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sample_hold_samples_on_trigger() {
        let mut sh = SampleHold::new();
        sh.prepare(44100.0, 256);

        // Create signal input: constant 0.75
        let mut signal_in = SignalBuffer::control(256);
        signal_in.fill(0.75);

        // Create trigger: rising edge at sample 100
        let mut trigger_in = SignalBuffer::control(256);
        trigger_in.fill(0.0);
        for i in 100..256 {
            trigger_in.samples[i] = 1.0;
        }

        let mut outputs = vec![SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // No slew (instant)
        sh.process(&[&signal_in, &trigger_in], &mut outputs, &[0.0], &ctx);

        // Before trigger (sample 99), output should be initial value (0)
        assert!(
            outputs[0].samples[99].abs() < 0.01,
            "Before trigger should be 0, got {}",
            outputs[0].samples[99]
        );

        // After trigger (sample 100+), output should be 0.75
        assert!(
            (outputs[0].samples[100] - 0.75).abs() < 0.01,
            "After trigger should be 0.75, got {}",
            outputs[0].samples[100]
        );
    }

    #[test]
    fn test_sample_hold_holds_value() {
        let mut sh = SampleHold::new();
        sh.prepare(44100.0, 256);

        // Create changing signal input
        let mut signal_in = SignalBuffer::control(256);
        for i in 0..256 {
            signal_in.samples[i] = i as f32 / 256.0;
        }

        // Trigger at sample 50
        let mut trigger_in = SignalBuffer::control(256);
        trigger_in.fill(0.0);
        trigger_in.samples[50] = 1.0;

        let mut outputs = vec![SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        sh.process(&[&signal_in, &trigger_in], &mut outputs, &[0.0], &ctx);

        // Value sampled at 50 should be 50/256 ≈ 0.195
        let expected = 50.0 / 256.0;

        // All samples after trigger should hold this value
        for i in 51..256 {
            assert!(
                (outputs[0].samples[i] - expected).abs() < 0.01,
                "Sample {} should hold {}, got {}",
                i,
                expected,
                outputs[0].samples[i]
            );
        }
    }

    #[test]
    fn test_sample_hold_multiple_triggers() {
        let mut sh = SampleHold::new();
        sh.prepare(44100.0, 256);

        // Signal: first half 0.25, second half 0.75
        let mut signal_in = SignalBuffer::control(256);
        for i in 0..128 {
            signal_in.samples[i] = 0.25;
        }
        for i in 128..256 {
            signal_in.samples[i] = 0.75;
        }

        // Trigger at sample 50 and 150
        let mut trigger_in = SignalBuffer::control(256);
        trigger_in.fill(0.0);
        trigger_in.samples[50] = 1.0;
        trigger_in.samples[150] = 1.0;

        let mut outputs = vec![SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        sh.process(&[&signal_in, &trigger_in], &mut outputs, &[0.0], &ctx);

        // After first trigger (50-149): should hold 0.25
        assert!(
            (outputs[0].samples[100] - 0.25).abs() < 0.01,
            "After first trigger should be 0.25"
        );

        // After second trigger (150+): should hold 0.75
        assert!(
            (outputs[0].samples[200] - 0.75).abs() < 0.01,
            "After second trigger should be 0.75"
        );
    }

    #[test]
    fn test_sample_hold_slew() {
        let mut sh = SampleHold::new();
        let sample_rate = 44100.0;
        sh.prepare(sample_rate, 44100);

        // Signal: constant 1.0
        let mut signal_in = SignalBuffer::control(44100);
        signal_in.fill(1.0);

        // Trigger at sample 0
        let mut trigger_in = SignalBuffer::control(44100);
        trigger_in.fill(0.0);
        trigger_in.samples[0] = 1.0;

        let mut outputs = vec![SignalBuffer::control(44100)];
        let ctx = ProcessContext::new(sample_rate, 44100);

        // 0.5 second slew time
        sh.process(&[&signal_in, &trigger_in], &mut outputs, &[0.5], &ctx);

        // With linear slew over 0.5s, after 0.25s we should be at 0.5
        let quarter_slew = (0.25 * sample_rate) as usize;
        assert!(
            (outputs[0].samples[quarter_slew] - 0.5).abs() < 0.02,
            "At quarter slew time should be ~0.5, got {}",
            outputs[0].samples[quarter_slew]
        );

        // After full slew time (0.5s), should be at 1.0
        let full_slew = (0.5 * sample_rate) as usize;
        assert!(
            (outputs[0].samples[full_slew] - 1.0).abs() < 0.01,
            "At full slew time should be ~1.0, got {}",
            outputs[0].samples[full_slew]
        );
    }

    #[test]
    fn test_sample_hold_slew_both_directions() {
        let mut sh = SampleHold::new();
        let sample_rate = 44100.0;
        sh.prepare(sample_rate, 88200);

        // Start with held value at 1.0
        sh.held_value = 1.0;
        sh.current_value = 1.0;

        // Signal alternates: 0 for first half, 1 for second
        let mut signal_in = SignalBuffer::control(88200);
        for i in 0..44100 {
            signal_in.samples[i] = 0.0;
        }
        for i in 44100..88200 {
            signal_in.samples[i] = 1.0;
        }

        // Trigger at samples 0 and 44100
        let mut trigger_in = SignalBuffer::control(88200);
        trigger_in.fill(0.0);
        trigger_in.samples[0] = 1.0;
        trigger_in.samples[44100] = 1.0;

        let mut outputs = vec![SignalBuffer::control(88200)];
        let ctx = ProcessContext::new(sample_rate, 88200);

        // 0.5 second slew time
        sh.process(&[&signal_in, &trigger_in], &mut outputs, &[0.5], &ctx);

        // After first trigger, slewing down from 1.0 to 0.0
        // At 0.25s, should be at ~0.5
        let quarter = (0.25 * sample_rate) as usize;
        assert!(
            (outputs[0].samples[quarter] - 0.5).abs() < 0.02,
            "Slewing down: at quarter should be ~0.5, got {}",
            outputs[0].samples[quarter]
        );

        // After second trigger at 1s, slewing up from 0.0 to 1.0
        // At 1.25s, should be at ~0.5
        let second_quarter = 44100 + quarter;
        assert!(
            (outputs[0].samples[second_quarter] - 0.5).abs() < 0.02,
            "Slewing up: at quarter should be ~0.5, got {}",
            outputs[0].samples[second_quarter]
        );
    }

    #[test]
    fn test_sample_hold_edge_detection() {
        let mut sh = SampleHold::new();
        sh.prepare(44100.0, 256);

        // Signal: 0.5
        let mut signal_in = SignalBuffer::control(256);
        signal_in.fill(0.5);

        // Trigger: stays high the whole time (should only sample once on rising edge)
        let mut trigger_in = SignalBuffer::control(256);
        trigger_in.fill(1.0);

        let mut outputs = vec![SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        sh.process(&[&signal_in, &trigger_in], &mut outputs, &[0.0], &ctx);

        // Should sample at first sample
        assert!(
            (outputs[0].samples[0] - 0.5).abs() < 0.01,
            "Should sample on first rising edge"
        );

        // Now process again with signal at 0.8, trigger still high
        signal_in.fill(0.8);
        let mut outputs2 = vec![SignalBuffer::control(256)];

        sh.process(&[&signal_in, &trigger_in], &mut outputs2, &[0.0], &ctx);

        // Should NOT sample again (no rising edge), so should still output 0.5
        assert!(
            (outputs2[0].samples[0] - 0.5).abs() < 0.01,
            "Should not re-sample while trigger high, got {}",
            outputs2[0].samples[0]
        );
    }

    #[test]
    fn test_sample_hold_reset() {
        let mut sh = SampleHold::new();
        sh.prepare(44100.0, 256);

        // Set some state
        sh.held_value = 0.75;
        sh.current_value = 0.5;
        sh.trigger.rising(1.0);

        // Reset
        sh.reset();

        assert!(sh.held_value.abs() < f32::EPSILON);
        assert!(sh.current_value.abs() < f32::EPSILON);
        assert!(!sh.trigger.is_high());
    }

    #[test]
    fn test_sample_hold_no_inputs() {
        let mut sh = SampleHold::new();
        sh.prepare(44100.0, 256);

        let mut outputs = vec![SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process with no inputs (should use defaults)
        sh.process(&[], &mut outputs, &[0.0], &ctx);

        // Should output 0 (initial held value)
        assert!(
            outputs[0].samples[0].abs() < f32::EPSILON,
            "Without inputs should output 0"
        );
    }

    #[test]
    fn test_sample_hold_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SampleHold>();
    }

    #[test]
    fn test_sample_hold_default() {
        let sh = SampleHold::default();
        assert_eq!(sh.info().id, "util.sample_hold");
    }

    #[test]
    fn test_sample_hold_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<SampleHold>();

        assert!(registry.contains("util.sample_hold"));

        let module = registry.create("util.sample_hold");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "util.sample_hold");
        assert_eq!(module.info().name, "Sample & Hold");
        assert_eq!(module.ports().len(), 3); // 2 inputs + 1 output
        assert_eq!(module.parameters().len(), 1); // Slew
    }
}
