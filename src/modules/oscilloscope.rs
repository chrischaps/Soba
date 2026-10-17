//! Oscilloscope module.
//!
//! Real-time waveform visualization module for debugging and learning.
//! Displays incoming signals with configurable time scale, amplitude scale,
//! and trigger modes.

use crate::dsp::{
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

/// Number of samples to capture for display.
pub const SCOPE_BUFFER_SIZE: usize = 512;

/// Trigger modes for the oscilloscope.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(u8)]
pub enum TriggerMode {
    /// Auto-triggers if no signal crosses threshold within timeout.
    #[default]
    Auto = 0,
    /// Only triggers on signal crossing threshold.
    Normal = 1,
    /// Single trigger, then holds display.
    Single = 2,
    /// Continuous sweep, no triggering (free running).
    Free = 3,
}

impl TriggerMode {
    fn from_param(value: f32) -> Self {
        match value.round() as u8 {
            0 => TriggerMode::Auto,
            1 => TriggerMode::Normal,
            2 => TriggerMode::Single,
            3 => TriggerMode::Free,
            _ => TriggerMode::Auto,
        }
    }
}

/// Capture state for synchronized triggering.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CaptureState {
    /// Waiting for a trigger event.
    WaitingForTrigger,
    /// Trigger detected, collecting post-trigger samples.
    Capturing { samples_remaining: usize },
    /// Capture complete, ready to send to UI.
    Ready,
}

/// A real-time oscilloscope module for waveform visualization.
///
/// # Features
///
/// - Two input channels for simultaneous display
/// - External trigger input
/// - Configurable trigger mode (Auto, Normal, Single, Free)
/// - Adjustable trigger level
/// - Time and amplitude scaling (handled in UI)
///
/// # Ports
///
/// **Inputs:**
/// - **In 1** (Audio/Control): Primary trace signal
/// - **In 2** (Audio/Control): Secondary trace signal (optional)
/// - **Trigger** (Gate): External trigger input
///
/// **Outputs:** None (display only)
///
/// # Parameters
///
/// - **Trigger Mode**: Auto, Normal, Single, or Free
/// - **Trigger Level**: -1.0 to +1.0, threshold for triggering
pub struct Oscilloscope {
    /// Ring buffer for channel 1 samples.
    buffer1: Vec<f32>,
    /// Ring buffer for channel 2 samples.
    buffer2: Vec<f32>,
    /// Write position in the ring buffer.
    write_pos: usize,
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Previous sample value for trigger edge detection.
    prev_trigger_sample: f32,
    /// The external trigger, for its rising edges.
    ext_trigger: Edge,
    /// Current capture state.
    capture_state: CaptureState,
    /// Position in buffer where trigger occurred.
    trigger_pos: usize,
    /// Samples since last successful capture (for auto-trigger timeout).
    samples_since_capture: usize,
    /// Buffer ready for UI consumption.
    buffer_ready: bool,
    /// Captured buffer for channel 1 (to send to UI).
    captured_ch1: Vec<f32>,
    /// Captured buffer for channel 2 (to send to UI).
    captured_ch2: Vec<f32>,
    /// Whether the captured buffer was triggered or free-running.
    captured_triggered: bool,
    /// Single-shot has fired (for Single trigger mode).
    single_shot_done: bool,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

/// Number of samples to show before the trigger point (pre-trigger).
/// About 25% of the buffer gives good context.
const PRE_TRIGGER_SAMPLES: usize = SCOPE_BUFFER_SIZE / 4;

/// Number of samples to capture after the trigger point.
const POST_TRIGGER_SAMPLES: usize = SCOPE_BUFFER_SIZE - PRE_TRIGGER_SAMPLES;

impl Oscilloscope {
    /// Creates a new oscilloscope module.
    pub fn new() -> Self {
        Self {
            buffer1: vec![0.0; SCOPE_BUFFER_SIZE],
            buffer2: vec![0.0; SCOPE_BUFFER_SIZE],
            write_pos: 0,
            sample_rate: 44100.0,
            prev_trigger_sample: 0.0,
            ext_trigger: Edge::LOW,
            capture_state: CaptureState::WaitingForTrigger,
            trigger_pos: 0,
            samples_since_capture: 0,
            buffer_ready: false,
            captured_ch1: vec![0.0; SCOPE_BUFFER_SIZE],
            captured_ch2: vec![0.0; SCOPE_BUFFER_SIZE],
            captured_triggered: false,
            single_shot_done: false,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in1", "In 1", SignalType::Audio, 0.0).describe("Signal for the first trace; also what triggers the sweep"),
                PortDefinition::input_with_default("in2", "In 2", SignalType::Audio, 0.0).describe("Signal for the second trace"),
                PortDefinition::input_with_default("trigger", "Trig", SignalType::Gate, 0.0).describe("External trigger; a rising edge starts a sweep"),
                // No output ports - display only
            ],
            parameters: vec![
                // Trigger Mode: 0=Auto, 1=Normal, 2=Single, 3=Free
                ParameterDefinition::new(
                    "trigger_mode",
                    "Mode",
                    0.0,
                    3.0,
                    0.0, // Default: Auto
                    ParameterDisplay::discrete(&["Auto", "Normal", "Single", "Free"]),
                ).describe("How sweeps start: Auto, Normal, Single capture or Free run"),
                // Trigger Level: -1 to +1
                ParameterDefinition::new(
                    "trigger_level",
                    "Trigger Level",
                    -1.0,
                    1.0,
                    0.0, // Default: 0.0 (center)
                    ParameterDisplay::linear(""),
                ).describe("Signal level that starts a sweep"),
            ],
        }
    }

    /// Port index constants.
    const PORT_IN1: usize = 0;
    const PORT_IN2: usize = 1;
    const PORT_TRIGGER: usize = 2;

    /// Parameter index constants.
    const PARAM_TRIGGER_MODE: usize = 0;
    const PARAM_TRIGGER_LEVEL: usize = 1;

    /// Auto-trigger timeout in samples (about 50ms at 44100Hz).
    const AUTO_TRIGGER_TIMEOUT: usize = 2205;

    /// Check for rising edge trigger on the signal.
    fn check_signal_trigger(&mut self, sample: f32, trigger_level: f32) -> bool {
        let triggered = self.prev_trigger_sample < trigger_level && sample >= trigger_level;
        self.prev_trigger_sample = sample;
        triggered
    }

    /// Check for rising edge on external trigger.
    fn check_ext_trigger(&mut self, ext_trig_val: f32) -> bool {
        self.ext_trigger.rising(ext_trig_val)
    }

    /// Capture the buffer starting from a specific trigger position.
    /// The trigger point will be at PRE_TRIGGER_SAMPLES from the start.
    fn capture_buffer_from_trigger(&mut self, triggered: bool) {
        // Start reading from (trigger_pos - PRE_TRIGGER_SAMPLES) in the ring buffer
        let start_pos = (self.trigger_pos + SCOPE_BUFFER_SIZE - PRE_TRIGGER_SAMPLES) % SCOPE_BUFFER_SIZE;

        for i in 0..SCOPE_BUFFER_SIZE {
            let idx = (start_pos + i) % SCOPE_BUFFER_SIZE;
            self.captured_ch1[i] = self.buffer1[idx];
            self.captured_ch2[i] = self.buffer2[idx];
        }
        self.captured_triggered = triggered;
        self.buffer_ready = true;
    }

    /// Capture buffer for free-running mode (just copy current buffer).
    fn capture_buffer_free(&mut self) {
        for i in 0..SCOPE_BUFFER_SIZE {
            let idx = (self.write_pos + i) % SCOPE_BUFFER_SIZE;
            self.captured_ch1[i] = self.buffer1[idx];
            self.captured_ch2[i] = self.buffer2[idx];
        }
        self.captured_triggered = false;
        self.buffer_ready = true;
    }
}

impl Default for Oscilloscope {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Oscilloscope {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "util.oscilloscope",
            name: "Oscilloscope",
            category: ModuleCategory::Utility,
            description: "Real-time waveform display for signal visualization",
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
        // Reset state
        self.reset();
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        _outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let trigger_mode = TriggerMode::from_param(params[Self::PARAM_TRIGGER_MODE]);
        let trigger_level = params[Self::PARAM_TRIGGER_LEVEL];

        // Get input buffers
        let input1 = inputs.get(Self::PORT_IN1);
        let input2 = inputs.get(Self::PORT_IN2);
        let ext_trigger = inputs.get(Self::PORT_TRIGGER);

        // Process each sample
        for i in 0..context.block_size {
            // Get sample values
            let sample1 = input1.map_or(0.0, |buf| buf.at(i));
            let sample2 = input2.map_or(0.0, |buf| buf.at(i));
            let ext_trig_val = ext_trigger.map_or(0.0, |buf| buf.at(i));

            // Write to ring buffer
            self.buffer1[self.write_pos] = sample1;
            self.buffer2[self.write_pos] = sample2;

            // Track time for auto-trigger
            self.samples_since_capture += 1;

            // Handle different trigger modes
            match trigger_mode {
                TriggerMode::Free => {
                    // Free-running: capture at regular intervals
                    if self.samples_since_capture >= SCOPE_BUFFER_SIZE {
                        self.capture_buffer_free();
                        self.samples_since_capture = 0;
                    }
                }

                TriggerMode::Single => {
                    if !self.single_shot_done {
                        match self.capture_state {
                            CaptureState::WaitingForTrigger => {
                                let ext_triggered = self.check_ext_trigger(ext_trig_val);
                                let signal_triggered = self.check_signal_trigger(sample1, trigger_level);

                                if ext_triggered || signal_triggered {
                                    // Trigger detected! Record position and start capturing
                                    self.trigger_pos = self.write_pos;
                                    self.capture_state = CaptureState::Capturing {
                                        samples_remaining: POST_TRIGGER_SAMPLES,
                                    };
                                }
                            }
                            CaptureState::Capturing { samples_remaining } => {
                                if samples_remaining <= 1 {
                                    // Done capturing
                                    self.capture_buffer_from_trigger(true);
                                    self.capture_state = CaptureState::Ready;
                                    self.single_shot_done = true;
                                    self.samples_since_capture = 0;
                                } else {
                                    self.capture_state = CaptureState::Capturing {
                                        samples_remaining: samples_remaining - 1,
                                    };
                                }
                            }
                            CaptureState::Ready => {
                                // Single-shot done, stay in ready state
                            }
                        }
                    }
                }

                TriggerMode::Normal => {
                    match self.capture_state {
                        CaptureState::WaitingForTrigger => {
                            let ext_triggered = self.check_ext_trigger(ext_trig_val);
                            let signal_triggered = self.check_signal_trigger(sample1, trigger_level);

                            if ext_triggered || signal_triggered {
                                self.trigger_pos = self.write_pos;
                                self.capture_state = CaptureState::Capturing {
                                    samples_remaining: POST_TRIGGER_SAMPLES,
                                };
                            }
                            // Normal mode: no auto-trigger, just wait
                        }
                        CaptureState::Capturing { samples_remaining } => {
                            if samples_remaining <= 1 {
                                self.capture_buffer_from_trigger(true);
                                self.capture_state = CaptureState::WaitingForTrigger;
                                self.samples_since_capture = 0;
                            } else {
                                self.capture_state = CaptureState::Capturing {
                                    samples_remaining: samples_remaining - 1,
                                };
                            }
                        }
                        CaptureState::Ready => {
                            // Go back to waiting
                            self.capture_state = CaptureState::WaitingForTrigger;
                        }
                    }
                }

                TriggerMode::Auto => {
                    match self.capture_state {
                        CaptureState::WaitingForTrigger => {
                            let ext_triggered = self.check_ext_trigger(ext_trig_val);
                            let signal_triggered = self.check_signal_trigger(sample1, trigger_level);

                            if ext_triggered || signal_triggered {
                                self.trigger_pos = self.write_pos;
                                self.capture_state = CaptureState::Capturing {
                                    samples_remaining: POST_TRIGGER_SAMPLES,
                                };
                            } else if self.samples_since_capture >= Self::AUTO_TRIGGER_TIMEOUT {
                                // Auto-trigger: use current position
                                self.trigger_pos = self.write_pos;
                                self.capture_state = CaptureState::Capturing {
                                    samples_remaining: POST_TRIGGER_SAMPLES,
                                };
                            }
                        }
                        CaptureState::Capturing { samples_remaining } => {
                            if samples_remaining <= 1 {
                                // Check if this was a real trigger or auto-trigger
                                let was_triggered = self.samples_since_capture < Self::AUTO_TRIGGER_TIMEOUT;
                                self.capture_buffer_from_trigger(was_triggered);
                                self.capture_state = CaptureState::WaitingForTrigger;
                                self.samples_since_capture = 0;
                            } else {
                                self.capture_state = CaptureState::Capturing {
                                    samples_remaining: samples_remaining - 1,
                                };
                            }
                        }
                        CaptureState::Ready => {
                            self.capture_state = CaptureState::WaitingForTrigger;
                        }
                    }
                }
            }

            // Advance write position after all processing
            self.write_pos = (self.write_pos + 1) % SCOPE_BUFFER_SIZE;
        }
    }

    fn reset(&mut self) {
        self.buffer1.fill(0.0);
        self.buffer2.fill(0.0);
        self.write_pos = 0;
        self.prev_trigger_sample = 0.0;
        self.ext_trigger.reset();
        self.capture_state = CaptureState::WaitingForTrigger;
        self.trigger_pos = 0;
        self.samples_since_capture = 0;
        self.buffer_ready = false;
        self.single_shot_done = false;
        self.captured_ch1.fill(0.0);
        self.captured_ch2.fill(0.0);
    }

    fn take_scope_data(&mut self) -> Option<(&[f32], &[f32], bool)> {
        if self.buffer_ready {
            self.buffer_ready = false;
            Some((&self.captured_ch1, &self.captured_ch2, self.captured_triggered))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oscilloscope_info() {
        let scope = Oscilloscope::new();
        assert_eq!(scope.info().id, "util.oscilloscope");
        assert_eq!(scope.info().name, "Oscilloscope");
        assert_eq!(scope.info().category, ModuleCategory::Utility);
    }

    #[test]
    fn test_oscilloscope_ports() {
        let scope = Oscilloscope::new();
        let ports = scope.ports();

        // 3 inputs, 0 outputs
        assert_eq!(ports.len(), 3);

        // Input 1
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in1");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        // Input 2
        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "in2");
        assert_eq!(ports[1].signal_type, SignalType::Audio);

        // Trigger
        assert!(ports[2].is_input());
        assert_eq!(ports[2].id, "trigger");
        assert_eq!(ports[2].signal_type, SignalType::Gate);
    }

    #[test]
    fn test_oscilloscope_parameters() {
        let scope = Oscilloscope::new();
        let params = scope.parameters();

        assert_eq!(params.len(), 2);

        // Trigger Mode
        assert_eq!(params[0].id, "trigger_mode");
        assert!((params[0].min - 0.0).abs() < f32::EPSILON);
        assert!((params[0].max - 3.0).abs() < f32::EPSILON);

        // Trigger Level
        assert_eq!(params[1].id, "trigger_level");
        assert!((params[1].min - -1.0).abs() < f32::EPSILON);
        assert!((params[1].max - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_trigger_mode_from_param() {
        assert_eq!(TriggerMode::from_param(0.0), TriggerMode::Auto);
        assert_eq!(TriggerMode::from_param(1.0), TriggerMode::Normal);
        assert_eq!(TriggerMode::from_param(2.0), TriggerMode::Single);
        assert_eq!(TriggerMode::from_param(3.0), TriggerMode::Free);
        assert_eq!(TriggerMode::from_param(0.4), TriggerMode::Auto);
        assert_eq!(TriggerMode::from_param(1.4), TriggerMode::Normal); // 1.4 rounds to 1
        assert_eq!(TriggerMode::from_param(1.6), TriggerMode::Single); // 1.6 rounds to 2
    }

    #[test]
    fn test_oscilloscope_captures_samples() {
        let mut scope = Oscilloscope::new();
        scope.prepare(44100.0, 256);

        // Create a sine wave input
        let mut input1 = SignalBuffer::audio(256);
        for i in 0..256 {
            input1.samples[i] = (i as f32 * 0.1).sin();
        }

        let ctx = ProcessContext::new(44100.0, 256);

        // Process enough blocks to fill the buffer (free-running mode captures every SCOPE_BUFFER_SIZE samples)
        // We need at least SCOPE_BUFFER_SIZE samples, so 2 blocks of 256 = 512 samples
        for _ in 0..3 {
            scope.process(&[&input1], &mut [], &[3.0, 0.0], &ctx); // Mode 3 = Free
        }

        // Should have buffer ready
        let data = scope.take_scope_data();
        assert!(data.is_some());

        let (ch1, ch2, triggered) = data.unwrap();
        assert_eq!(ch1.len(), SCOPE_BUFFER_SIZE);
        assert_eq!(ch2.len(), SCOPE_BUFFER_SIZE);
        assert!(!triggered); // Free mode doesn't trigger

        // Buffer should not be ready again until more processing
        assert!(scope.take_scope_data().is_none());
    }

    #[test]
    fn test_oscilloscope_reset() {
        let mut scope = Oscilloscope::new();
        scope.prepare(44100.0, 256);

        // Write some data
        scope.buffer1[0] = 1.0;
        scope.write_pos = 100;
        scope.capture_state = CaptureState::Capturing { samples_remaining: 50 };

        // Reset
        scope.reset();

        assert_eq!(scope.buffer1[0], 0.0);
        assert_eq!(scope.write_pos, 0);
        assert_eq!(scope.capture_state, CaptureState::WaitingForTrigger);
    }

    #[test]
    fn test_oscilloscope_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Oscilloscope>();
    }

    #[test]
    fn test_oscilloscope_default() {
        let scope = Oscilloscope::default();
        assert_eq!(scope.info().id, "util.oscilloscope");
    }

    #[test]
    fn test_oscilloscope_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Oscilloscope>();

        assert!(registry.contains("util.oscilloscope"));

        let module = registry.create("util.oscilloscope");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "util.oscilloscope");
        assert_eq!(module.info().name, "Oscilloscope");
        assert_eq!(module.ports().len(), 3); // 3 inputs
        assert_eq!(module.parameters().len(), 2); // Mode, Level
    }
}
