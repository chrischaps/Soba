//! LFO (Low Frequency Oscillator) module.
//!
//! Generates low-frequency control signals for modulation purposes.

use std::f32::consts::TAU;

use crate::dsp::{
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// Waveform shapes for the LFO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfoWaveform {
    Sine = 0,
    Triangle = 1,
    Square = 2,
    Saw = 3,
}

impl LfoWaveform {
    /// Convert from parameter value (0-3) to waveform.
    pub fn from_param(value: f32) -> Self {
        match value as usize {
            0 => LfoWaveform::Sine,
            1 => LfoWaveform::Triangle,
            2 => LfoWaveform::Square,
            3 => LfoWaveform::Saw,
            _ => LfoWaveform::Sine,
        }
    }
}

/// The Tempo Sync choices: a label for the dropdown and the cycle length in
/// beats, or `None` for Off. Bars are four beats. Patches save the index, so
/// new divisions go on the end.
pub const SYNC_DIVISIONS: [(&str, Option<f64>); 17] = [
    ("Off", None),
    ("4 bars", Some(16.0)),
    ("2 bars", Some(8.0)),
    ("1 bar", Some(4.0)),
    ("1/2", Some(2.0)),
    ("1/2D", Some(3.0)),
    ("1/2T", Some(4.0 / 3.0)),
    ("1/4", Some(1.0)),
    ("1/4D", Some(1.5)),
    ("1/4T", Some(2.0 / 3.0)),
    ("1/8", Some(0.5)),
    ("1/8D", Some(0.75)),
    ("1/8T", Some(1.0 / 3.0)),
    ("1/16", Some(0.25)),
    ("1/16D", Some(0.375)),
    ("1/16T", Some(1.0 / 6.0)),
    ("1/32", Some(0.125)),
];

/// The Tempo Sync dropdown's labels.
const SYNC_LABELS: [&str; SYNC_DIVISIONS.len()] = {
    let mut labels = [""; SYNC_DIVISIONS.len()];
    let mut i = 0;
    while i < labels.len() {
        labels[i] = SYNC_DIVISIONS[i].0;
        i += 1;
    }
    labels
};

/// The cycle length in beats for a Tempo Sync value, or `None` when off.
pub fn sync_beats(value: f32) -> Option<f64> {
    SYNC_DIVISIONS.get(value.max(0.0) as usize).and_then(|&(_, beats)| beats)
}

/// A low-frequency oscillator for modulation.
///
/// Generates control-rate signals for modulating other module parameters.
/// Users can select the waveform shape and synchronize multiple LFOs together.
///
/// # Ports
///
/// **Inputs:**
/// - **Rate** (Control): CV input for rate modulation. Exponential scaling.
/// - **Sync** (Gate): Resets phase to offset on rising edge.
///
/// **Outputs:**
/// - **Out** (Control): The modulation signal.
///
/// # Parameters
///
/// - **Rate** (0.01-100 Hz): Speed of the LFO oscillation. Logarithmic display.
/// - **Waveform** (0-3): Shape of the output (Sine, Triangle, Square, Saw).
/// - **Phase** (0-360°): Phase offset for waveform start point.
/// - **Bipolar** (toggle): When on, output is -1 to +1. When off, output is 0 to +1.
/// - **Tempo Sync** (choice): Off runs free at Rate. A division (4 bars to
///   1/32, dotted and triplet) locks the cycle to the patch's beat, so
///   every synced LFO keeps in step with the Clock and restarts on its
///   downbeat.
pub struct Lfo {
    /// Current phase (0.0 to 1.0). In f64: at a hundredth of a hertz the
    /// step per sample is finer than f32 can add to a phase near 1 (#256).
    phase: f64,
    /// While synced: the beat a cycle starts on. 0, the downbeat, until the
    /// Sync input moves it.
    origin: f64,
    /// While synced: the beat at the last sample, to notice the transport
    /// jumping back to the top.
    last_beat: f64,
    /// While synced in a patch without a Clock: a beat of the LFO's own, at
    /// 120 BPM.
    free_beats: f64,
    /// The cycle length while synced, in beats, and the tempo, as of the
    /// last block: what the Rate knob shows.
    synced: Option<(f64, f32)>,
    /// The rate it ran at, after Rate CV, at the end of the last block.
    running_rate: f32,
    /// The sync input, for its rising edges.
    sync: Edge,
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
    /// Smoothed rate parameter.
    rate_smooth: SmoothedValue,
    /// Smoothed phase offset parameter.
    phase_offset_smooth: SmoothedValue,
}

impl Lfo {
    /// Creates a new LFO.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            phase: 0.0,
            origin: 0.0,
            last_beat: 0.0,
            free_beats: 0.0,
            synced: None,
            running_rate: 0.0,
            sync: Edge::LOW,
            sample_rate,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("rate_cv", "Rate", SignalType::Control, 0.0).describe("CV that speeds up or slows down the LFO; 1 doubles the rate"),
                PortDefinition::input_with_default("sync", "Sync", SignalType::Gate, 0.0).describe("A rising edge restarts the cycle"),
                // Output ports
                PortDefinition::output("out", "Out", SignalType::Control).describe("The modulation signal, -1 to 1 or 0 to 1"),
                PortDefinition::output("phase", "Phase", SignalType::Control).describe("Position in the cycle as a 0 to 1 ramp"),
            ],
            parameters: vec![
                // Rate parameter (logarithmic for musical response)
                ParameterDefinition::new(
                    "rate",
                    "Rate",
                    0.01,
                    100.0,
                    1.0, // Default 1 Hz
                    ParameterDisplay::logarithmic("Hz"),
                ).describe("Speed of the cycle, in Hz"),
                // Waveform selection
                ParameterDefinition::choice(
                    "waveform",
                    "Waveform",
                    &["Sine", "Triangle", "Square", "Saw"],
                    0, // Default Sine
                ).describe("Shape of the cycle: smooth, linear, stepped or ramping"),
                // Phase offset (0-360 degrees)
                ParameterDefinition::new(
                    "phase",
                    "Phase",
                    0.0,
                    360.0,
                    0.0,
                    ParameterDisplay::linear("°"),
                ).describe("Where in the cycle the waveform starts, and where Sync restarts it"),
                // Bipolar toggle
                ParameterDefinition::toggle("bipolar", "Bipolar", true).describe("On swings from -1 to 1; off stays between 0 and 1"),
                // Lock to the beat
                ParameterDefinition::choice(
                    "tempo_sync",
                    "Tempo Sync",
                    &SYNC_LABELS,
                    0, // Default Off
                ).describe("Locks the cycle to the Clock's beat, one cycle per division; Off runs free at Rate"),
            ],
            // Initialize smoothed parameters
            rate_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            phase_offset_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
        }
    }

    /// Port index constants.
    const PORT_RATE_CV: usize = 0;
    const PORT_SYNC: usize = 1;
    const PORT_OUT: usize = 0;
    const PORT_PHASE: usize = 1;

    /// Parameter index constants.
    const PARAM_RATE: usize = 0;
    const PARAM_WAVEFORM: usize = 1;
    const PARAM_PHASE: usize = 2;
    const PARAM_BIPOLAR: usize = 3;
    const PARAM_TEMPO_SYNC: usize = 4;

    /// How far the beat must step back to count as the transport starting
    /// over: a MIDI clock tick. Smaller steps back are a block's pacing
    /// overshooting a tick that came late, not a restart.
    const RESTART_BEATS: f64 = 1.0 / 24.0;

    /// Readout value index of the synced rate, 0 while it runs free: see
    /// [`DspModule::readout`].
    pub const READOUT_RATE: usize = 0;
    /// Readout value index of the rate it's running at, synced or free,
    /// after Rate CV, in Hz.
    pub const READOUT_RUNNING_RATE: usize = 1;

    /// Generate a sample for the given phase (0.0-1.0) and waveform.
    /// Returns value in bipolar form (-1 to +1).
    pub(crate) fn generate_sample(phase: f32, waveform: LfoWaveform) -> f32 {
        match waveform {
            LfoWaveform::Sine => (phase * TAU).sin(),
            LfoWaveform::Triangle => {
                // Triangle: 0->0.25 ramps 0->1, 0.25->0.75 ramps 1->-1, 0.75->1 ramps -1->0
                if phase < 0.25 {
                    phase * 4.0
                } else if phase < 0.75 {
                    1.0 - (phase - 0.25) * 4.0
                } else {
                    -1.0 + (phase - 0.75) * 4.0
                }
            }
            LfoWaveform::Square => {
                if phase < 0.5 { 1.0 } else { -1.0 }
            }
            LfoWaveform::Saw => {
                // Sawtooth: linear ramp from -1 to +1
                2.0 * phase - 1.0
            }
        }
    }

    /// Convert bipolar signal (-1 to +1) to unipolar (0 to +1).
    #[inline]
    pub(crate) fn to_unipolar(value: f32) -> f32 {
        (value + 1.0) * 0.5
    }
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Lfo {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "mod.lfo",
            name: "LFO",
            category: ModuleCategory::Modulation,
            description: "Low frequency oscillator for modulation",
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
        self.rate_smooth.set_sample_rate(sample_rate);
        self.phase_offset_smooth.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Set smoothing targets from parameters
        self.rate_smooth.set_target(params[Self::PARAM_RATE]);
        self.phase_offset_smooth.set_target(params[Self::PARAM_PHASE] / 360.0); // Convert degrees to 0-1

        // Discrete parameters don't need smoothing
        let waveform = LfoWaveform::from_param(params[Self::PARAM_WAVEFORM]);
        let is_bipolar = params[Self::PARAM_BIPOLAR] > 0.5;

        // Locked to the beat, at a division
        let sync_beats = sync_beats(params.get(Self::PARAM_TEMPO_SYNC).copied().unwrap_or(0.0));
        let transport = context.transport;
        let tempo = transport.tempo_bpm.unwrap_or(120.0);
        self.synced = sync_beats.map(|beats| (beats, tempo));

        // Get input buffers
        let rate_cv = inputs.get(Self::PORT_RATE_CV);
        let sync_in = inputs.get(Self::PORT_SYNC);

        // Process each sample
        for i in 0..context.block_size {
            // Get smoothed parameter values (per-sample for click-free changes)
            let base_rate = self.rate_smooth.next();
            let phase_offset = self.phase_offset_smooth.next();

            // Check for sync reset (rising edge detection)
            let sync_rising = self.sync.rising(sync_in.map_or(0.0, |buf| buf.at(i)));

            if let Some(cycle_beats) = sync_beats {
                // The phase is where the beat is in the cycle, so it can't
                // drift from the Clock, nor from other synced LFOs
                let beat = transport.beat_at(i, self.sample_rate).unwrap_or(self.free_beats);
                if beat < self.last_beat - Self::RESTART_BEATS {
                    // The transport went back to the top: so do we
                    self.origin = 0.0;
                }
                if sync_rising {
                    // Cycles now start from this beat
                    self.origin = beat;
                }
                self.last_beat = beat;
                self.phase = ((beat - self.origin) / cycle_beats).rem_euclid(1.0);
            } else if sync_rising {
                // Reset phase on sync rising edge
                self.phase = 0.0;
            }

            // Calculate effective phase with offset
            let effective_phase = (self.phase as f32 + phase_offset).fract();

            // Generate the waveform sample
            let sample = Self::generate_sample(effective_phase, waveform);

            // Apply bipolar/unipolar scaling and write to main output
            outputs[Self::PORT_OUT].samples[i] = if is_bipolar {
                sample
            } else {
                Self::to_unipolar(sample)
            };

            // Write current phase to phase output (0-1 range)
            outputs[Self::PORT_PHASE].samples[i] = effective_phase;

            if sync_beats.is_some() {
                // Without a Clock to follow, keep a beat of our own
                if transport.beat_position.is_none() {
                    self.free_beats += tempo as f64 / 60.0 / self.sample_rate as f64;
                }
                continue;
            }

            // Get rate CV modulation
            let rate_cv_value = rate_cv.map_or(0.0, |buf| buf.at(i));

            // Rate CV scales the rate multiplicatively
            // CV of 0 = base rate, CV of +1 = double rate, CV of -1 = half rate
            let rate_multiplier = 2.0_f32.powf(rate_cv_value);
            let final_rate = (base_rate * rate_multiplier).clamp(0.001, 1000.0);
            self.running_rate = final_rate;

            // Advance phase
            let phase_increment = f64::from(final_rate) / f64::from(self.sample_rate);
            self.phase += phase_increment;

            // Wrap phase to [0, 1)
            self.phase = self.phase.fract();
            if self.phase < 0.0 {
                self.phase += 1.0;
            }
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.origin = 0.0;
        self.last_beat = 0.0;
        self.free_beats = 0.0;
        self.sync.reset();
        // Reset smoothed parameters to their current targets
        self.rate_smooth.reset(self.rate_smooth.target());
        self.phase_offset_smooth.reset(self.phase_offset_smooth.target());
    }

    /// While synced, the rate the division comes to at the patch tempo, in
    /// Hz, for the Rate knob to show; and the rate it's running at, for the
    /// display.
    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let synced = self.synced.map(|(beats, tempo)| (tempo as f64 / 60.0 / beats) as f32);
        readout.values[Self::READOUT_RATE] = synced.unwrap_or(0.0);
        readout.values[Self::READOUT_RUNNING_RATE] = synced.unwrap_or(self.running_rate);
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::TransportState;

    #[test]
    fn test_lfo_info() {
        let lfo = Lfo::new();
        assert_eq!(lfo.info().id, "mod.lfo");
        assert_eq!(lfo.info().name, "LFO");
        assert_eq!(lfo.info().category, ModuleCategory::Modulation);
    }

    #[test]
    fn test_lfo_ports() {
        let lfo = Lfo::new();
        let ports = lfo.ports();

        // 2 inputs + 2 outputs = 4 ports
        assert_eq!(ports.len(), 4);

        // Input ports
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "rate_cv");
        assert_eq!(ports[0].signal_type, SignalType::Control);

        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "sync");
        assert_eq!(ports[1].signal_type, SignalType::Gate);

        // Output ports
        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "out");
        assert_eq!(ports[2].signal_type, SignalType::Control);

        assert!(ports[3].is_output());
        assert_eq!(ports[3].id, "phase");
        assert_eq!(ports[3].signal_type, SignalType::Control);
    }

    #[test]
    fn test_lfo_parameters() {
        let lfo = Lfo::new();
        let params = lfo.parameters();

        assert_eq!(params.len(), 5);

        // Rate parameter
        assert_eq!(params[0].id, "rate");
        assert!((params[0].min - 0.01).abs() < f32::EPSILON);
        assert!((params[0].max - 100.0).abs() < f32::EPSILON);
        assert!((params[0].default - 1.0).abs() < f32::EPSILON);

        // Waveform parameter
        assert_eq!(params[1].id, "waveform");
        assert!((params[1].min - 0.0).abs() < f32::EPSILON);
        assert!((params[1].max - 3.0).abs() < f32::EPSILON);

        // Phase parameter
        assert_eq!(params[2].id, "phase");
        assert!((params[2].min - 0.0).abs() < f32::EPSILON);
        assert!((params[2].max - 360.0).abs() < f32::EPSILON);

        // Bipolar parameter
        assert_eq!(params[3].id, "bipolar");
        assert!((params[3].default - 1.0).abs() < f32::EPSILON); // Default on
    }

    #[test]
    fn test_lfo_waveform_conversion() {
        assert_eq!(LfoWaveform::from_param(0.0), LfoWaveform::Sine);
        assert_eq!(LfoWaveform::from_param(1.0), LfoWaveform::Triangle);
        assert_eq!(LfoWaveform::from_param(2.0), LfoWaveform::Square);
        assert_eq!(LfoWaveform::from_param(3.0), LfoWaveform::Saw);
        assert_eq!(LfoWaveform::from_param(99.0), LfoWaveform::Sine); // Out of range defaults to Sine
    }

    #[test]
    fn test_lfo_generates_output() {
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 256);

        let mut outputs = vec![SignalBuffer::control(256), SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // 1 Hz, Sine, 0° phase, bipolar
        lfo.process(&[], &mut outputs, &[1.0, 0.0, 0.0, 1.0], &ctx);

        // Output should have non-zero values
        let has_nonzero = outputs[0].samples.iter().any(|&s| s.abs() > 0.001);
        assert!(has_nonzero, "LFO should produce non-zero signal");
    }

    #[test]
    fn test_lfo_bipolar_range() {
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 44100);

        let mut outputs = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let ctx = ProcessContext::new(44100.0, 44100);

        // 1 Hz, Sine, 0° phase, bipolar
        lfo.process(&[], &mut outputs, &[1.0, 0.0, 0.0, 1.0], &ctx);

        // Output should be within -1 to +1
        for &sample in &outputs[0].samples {
            assert!(
                (-1.0..=1.0).contains(&sample),
                "Bipolar sample {} out of range",
                sample
            );
        }
    }

    #[test]
    fn test_lfo_unipolar_range() {
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 44100);

        let mut outputs = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let ctx = ProcessContext::new(44100.0, 44100);

        // 1 Hz, Sine, 0° phase, unipolar (bipolar = 0)
        lfo.process(&[], &mut outputs, &[1.0, 0.0, 0.0, 0.0], &ctx);

        // Output should be within 0 to +1
        for &sample in &outputs[0].samples {
            assert!(
                (0.0..=1.0).contains(&sample),
                "Unipolar sample {} out of range",
                sample
            );
        }
    }

    #[test]
    fn test_lfo_sync_reset() {
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 1000);

        // Run LFO to advance phase
        let mut outputs = vec![SignalBuffer::control(1000), SignalBuffer::control(1000)];
        let ctx = ProcessContext::new(44100.0, 1000);
        lfo.process(&[], &mut outputs, &[1.0, 0.0, 0.0, 1.0], &ctx);

        // Now send a sync pulse
        let mut sync = SignalBuffer::control(100);
        sync.samples[50] = 1.0; // Rising edge at sample 50

        let rate_cv = SignalBuffer::control(100);
        let mut outputs2 = vec![SignalBuffer::control(100), SignalBuffer::control(100)];
        let ctx2 = ProcessContext::new(44100.0, 100);
        lfo.process(&[&rate_cv, &sync], &mut outputs2, &[1.0, 0.0, 0.0, 1.0], &ctx2);

        // After sync, sine should be near 0 (sin(0) = 0)
        assert!(
            outputs2[0].samples[51].abs() < 0.1,
            "Sine should be near 0 after sync reset, got {}",
            outputs2[0].samples[51]
        );
    }

    #[test]
    fn test_lfo_phase_offset() {
        let mut lfo1 = Lfo::new();
        let mut lfo2 = Lfo::new();
        lfo1.prepare(44100.0, 256);
        lfo2.prepare(44100.0, 256);

        let mut outputs1 = vec![SignalBuffer::control(256), SignalBuffer::control(256)];
        let mut outputs2 = vec![SignalBuffer::control(256), SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process multiple times to let parameter smoothing settle
        // Use very slow rate (0.01 Hz) so phase doesn't advance much
        for _ in 0..20 {
            lfo1.reset();
            lfo1.process(&[], &mut outputs1, &[0.01, 0.0, 0.0, 1.0], &ctx);
        }
        for _ in 0..20 {
            lfo2.reset();
            lfo2.process(&[], &mut outputs2, &[0.01, 0.0, 90.0, 1.0], &ctx);
        }

        // With slow rate and reset, check that outputs show offset effect
        // After smoothing settles, sine with 0° offset should start near 0
        // and sine with 90° offset should start near 1
        assert!(
            outputs1[0].samples[200].abs() < 0.2,
            "Sine at 0° should be near 0, got {}",
            outputs1[0].samples[200]
        );

        // With 90° offset, sine should be near 1
        assert!(
            outputs2[0].samples[200] > 0.8,
            "Sine at 90° should be near 1, got {}",
            outputs2[0].samples[200]
        );
    }

    #[test]
    fn test_lfo_rate_cv_modulation() {
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 44100);

        // Create rate CV input with +1 value (should double the rate)
        let mut rate_cv = SignalBuffer::control(44100);
        rate_cv.fill(1.0);

        let sync = SignalBuffer::control(44100);
        let mut outputs = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let ctx = ProcessContext::new(44100.0, 44100);

        // Base rate 1 Hz with +1 CV = 2 Hz
        lfo.process(&[&rate_cv, &sync], &mut outputs, &[1.0, 0.0, 0.0, 1.0], &ctx);

        // Count zero crossings of sine to verify doubled rate
        let mut zero_crossings = 0;
        for i in 1..44100 {
            if outputs[0].samples[i - 1] <= 0.0 && outputs[0].samples[i] > 0.0 {
                zero_crossings += 1;
            }
        }

        // At 2 Hz for 1 second, expect ~2 cycles
        assert!(
            (1..=3).contains(&zero_crossings),
            "Expected ~2 zero crossings for 2 Hz, got {}",
            zero_crossings
        );
    }

    #[test]
    fn test_lfo_waveform_shapes() {
        // Test that waveform generation produces correct shapes

        // Sine at key phases
        assert!((Lfo::generate_sample(0.0, LfoWaveform::Sine) - 0.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.25, LfoWaveform::Sine) - 1.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.5, LfoWaveform::Sine) - 0.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.75, LfoWaveform::Sine) - (-1.0)).abs() < 0.01);

        // Triangle at key phases
        assert!((Lfo::generate_sample(0.0, LfoWaveform::Triangle) - 0.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.25, LfoWaveform::Triangle) - 1.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.5, LfoWaveform::Triangle) - 0.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.75, LfoWaveform::Triangle) - (-1.0)).abs() < 0.01);

        // Square at key phases
        assert!((Lfo::generate_sample(0.0, LfoWaveform::Square) - 1.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.25, LfoWaveform::Square) - 1.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.5, LfoWaveform::Square) - (-1.0)).abs() < 0.01);
        assert!((Lfo::generate_sample(0.75, LfoWaveform::Square) - (-1.0)).abs() < 0.01);

        // Saw at key phases
        assert!((Lfo::generate_sample(0.0, LfoWaveform::Saw) - (-1.0)).abs() < 0.01);
        assert!((Lfo::generate_sample(0.25, LfoWaveform::Saw) - (-0.5)).abs() < 0.01);
        assert!((Lfo::generate_sample(0.5, LfoWaveform::Saw) - 0.0).abs() < 0.01);
        assert!((Lfo::generate_sample(0.75, LfoWaveform::Saw) - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_lfo_different_waveforms() {
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 44100);

        // Test each waveform produces different output
        let mut sine_out = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let mut tri_out = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let mut sq_out = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let mut saw_out = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let ctx = ProcessContext::new(44100.0, 44100);

        lfo.reset();
        lfo.process(&[], &mut sine_out, &[1.0, 0.0, 0.0, 1.0], &ctx);
        lfo.reset();
        lfo.process(&[], &mut tri_out, &[1.0, 1.0, 0.0, 1.0], &ctx);
        lfo.reset();
        lfo.process(&[], &mut sq_out, &[1.0, 2.0, 0.0, 1.0], &ctx);
        lfo.reset();
        lfo.process(&[], &mut saw_out, &[1.0, 3.0, 0.0, 1.0], &ctx);

        // Verify waveforms are different by comparing at quarter period
        let quarter = 11025; // 1/4 second at 44100 Hz

        // At quarter period: sine=1, tri=1, square=1, saw=-0.5
        assert!((sine_out[0].samples[quarter] - 1.0).abs() < 0.1);
        assert!((tri_out[0].samples[quarter] - 1.0).abs() < 0.1);
        assert!((sq_out[0].samples[quarter] - 1.0).abs() < 0.1);
        assert!((saw_out[0].samples[quarter] - (-0.5)).abs() < 0.1);
    }

    #[test]
    fn test_lfo_reset() {
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 256);

        // Generate some samples
        let mut outputs = vec![SignalBuffer::control(256), SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);
        lfo.process(&[], &mut outputs, &[1.0, 0.0, 0.0, 1.0], &ctx);

        // Reset
        lfo.reset();

        // Generate first sample after reset - sine should be near 0
        let mut outputs2 = vec![SignalBuffer::control(1), SignalBuffer::control(1)];
        let ctx2 = ProcessContext::new(44100.0, 1);
        lfo.process(&[], &mut outputs2, &[1.0, 0.0, 0.0, 1.0], &ctx2);

        assert!(
            outputs2[0].samples[0].abs() < 0.01,
            "Sine after reset should be near 0"
        );
    }

    #[test]
    fn test_lfo_high_rate() {
        // Test that LFO works correctly at higher rates (100 Hz)
        let mut lfo = Lfo::new();
        lfo.prepare(44100.0, 44100);

        let mut outputs = vec![SignalBuffer::control(44100), SignalBuffer::control(44100)];
        let ctx = ProcessContext::new(44100.0, 44100);

        // 100 Hz rate
        lfo.process(&[], &mut outputs, &[100.0, 0.0, 0.0, 1.0], &ctx);

        // Count zero crossings - should be ~100 for 100 Hz
        let mut zero_crossings = 0;
        for i in 1..44100 {
            if outputs[0].samples[i - 1] <= 0.0 && outputs[0].samples[i] > 0.0 {
                zero_crossings += 1;
            }
        }

        // Allow some tolerance
        assert!(
            (95..=105).contains(&zero_crossings),
            "Expected ~100 cycles at 100 Hz, got {}",
            zero_crossings
        );
    }

    #[test]
    fn test_lfo_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Lfo>();
    }

    #[test]
    fn test_lfo_default() {
        let lfo = Lfo::default();
        assert_eq!(lfo.info().id, "mod.lfo");
    }

    #[test]
    fn test_lfo_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Lfo>();

        assert!(registry.contains("mod.lfo"));

        let module = registry.create("mod.lfo");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "mod.lfo");
        assert_eq!(module.info().name, "LFO");
        assert_eq!(module.ports().len(), 4); // 2 inputs + 2 outputs
        assert_eq!(module.parameters().len(), 5); // Rate, Waveform, Phase, Bipolar, Tempo Sync
    }

    // --- Tempo sync ---

    /// The Tempo Sync value of the division labelled `label`.
    fn division(label: &str) -> f32 {
        SYNC_DIVISIONS.iter().position(|&(l, _)| l == label).unwrap() as f32
    }

    #[test]
    fn test_every_sync_division_has_a_label_and_length() {
        let lfo = Lfo::new();
        let sync = &lfo.parameters()[4];
        assert_eq!(sync.id, "tempo_sync");
        assert_eq!(sync.default, 0.0, "Off by default, so old patches run free");
        assert_eq!(sync.max as usize, SYNC_DIVISIONS.len() - 1);
        assert_eq!(sync_beats(0.0), None);
        for i in 1..SYNC_DIVISIONS.len() {
            assert!(sync_beats(i as f32).is_some_and(|beats| beats > 0.0), "{}", SYNC_DIVISIONS[i].0);
        }
        assert_eq!(sync_beats(division("1 bar")), Some(4.0));
        assert_eq!(sync_beats(division("1/8D")), Some(0.75));
        assert_eq!(sync_beats(99.0), None);
    }

    /// Runs a Clock at `bpm` and an LFO synced to `sync`, as a patch would:
    /// before each block, the LFO is handed the Clock's transport. Returns
    /// the sample of every wrap of the LFO's Phase output.
    fn synced_wraps(bpm: f32, sync: f32, sample_rate: f32, seconds: f64) -> Vec<u64> {
        use crate::modules::clock::Clock;
        const BLOCK: usize = 512;
        let clock_params = [bpm, 50.0, 2.0, 1.0, 0.0];
        let lfo_params = [1.0, 3.0, 0.0, 1.0, sync];
        let mut clock = Clock::new();
        let mut lfo = Lfo::new();
        clock.prepare(sample_rate, BLOCK);
        lfo.prepare(sample_rate, BLOCK);
        let mut clock_out = vec![SignalBuffer::control(BLOCK); 3];
        let mut lfo_out = vec![SignalBuffer::control(BLOCK); 2];

        let blocks = (seconds * sample_rate as f64 / BLOCK as f64).ceil() as u64;
        let (mut wraps, mut prev) = (Vec::new(), 0.0);
        for block in 0..blocks {
            let transport = clock.transport(&clock_params).unwrap();
            let ctx = ProcessContext::with_transport(sample_rate, BLOCK, transport);
            clock.process(&[], &mut clock_out, &clock_params, &ctx);
            lfo.process(&[], &mut lfo_out, &lfo_params, &ctx);
            for (i, &phase) in lfo_out[1].samples.iter().enumerate() {
                if phase < prev - 0.5 {
                    wraps.push(block * BLOCK as u64 + i as u64);
                }
                prev = phase;
            }
        }
        wraps
    }

    #[test]
    fn test_synced_lfo_completes_whole_cycles_per_bar() {
        for (bpm, label, per_bar) in [(120.0, "1 bar", 1.0), (97.0, "1/4", 4.0), (133.0, "1/16", 16.0), (90.0, "1/8T", 12.0), (140.0, "2 bars", 0.5)] {
            let sample_rate = 48000.0;
            let bar = 4.0 * 60.0 / bpm as f64 * sample_rate as f64;
            let cycle = bar / per_bar;
            // Eight bars and half a cycle more
            let wraps = synced_wraps(bpm, division(label), sample_rate, (8.0 * bar + cycle / 2.0) / sample_rate as f64);
            // Every wrap (cycle n + 1 starting) within a sample of where the
            // bar says it should be
            assert!(wraps.len() >= (8.0 * per_bar) as usize, "{label} at {bpm} BPM: {} wraps", wraps.len());
            for (n, &wrap) in wraps.iter().enumerate() {
                let ideal = (n + 1) as f64 * cycle;
                assert!((wrap as f64 - ideal).abs() <= 1.0, "{label} at {bpm}: wrap {n} at {wrap}, ideal {ideal}");
            }
        }
    }

    #[test]
    fn test_synced_lfo_stays_phase_locked_for_ten_minutes() {
        // An awkward tempo, so a bar is no whole number of samples
        let sample_rate = 48000.0;
        let bpm = 97.3;
        let bar = 4.0 * 60.0 / bpm as f32 as f64 * sample_rate as f64;
        let wraps = synced_wraps(bpm as f32, division("1 bar"), sample_rate, 600.0);
        assert!(wraps.len() >= 240, "{} bars", wraps.len());
        for (n, &wrap) in wraps.iter().enumerate() {
            let ideal = (n + 1) as f64 * bar;
            assert!((wrap as f64 - ideal).abs() <= 1.0, "bar {n}: wrap at {wrap}, ideal {ideal}");
        }
    }

    #[test]
    fn test_synced_lfos_share_a_phase() {
        // Two LFOs started at different times, both synced, read the same
        // phase from the same beat
        let mut early = Lfo::new();
        let mut late = Lfo::new();
        early.prepare(48000.0, 256);
        late.prepare(48000.0, 256);
        let mut out_early = vec![SignalBuffer::control(256); 2];
        let mut out_late = vec![SignalBuffer::control(256); 2];
        let params = [1.0, 3.0, 0.0, 1.0, division("1/4")];
        let mut transport = TransportState::playing_at(120.0);
        for block in 0..400 {
            transport.beat_position = Some(block as f64 * 256.0 * 2.0 / 48000.0);
            let ctx = ProcessContext::with_transport(48000.0, 256, transport);
            early.process(&[], &mut out_early, &params, &ctx);
            if block >= 123 {
                late.process(&[], &mut out_late, &params, &ctx);
            }
        }
        assert_eq!(out_early[1].samples, out_late[1].samples);
    }

    #[test]
    fn test_synced_lfo_restarts_on_the_downbeat() {
        let mut lfo = Lfo::new();
        lfo.prepare(48000.0, 256);
        let mut out = vec![SignalBuffer::control(256); 2];
        let params = [1.0, 3.0, 0.0, 1.0, division("1 bar")];
        let mut transport = TransportState::playing_at(120.0);

        // A Sync pulse at beat 1 starts cycles from there...
        transport.beat_position = Some(1.0);
        let mut sync = SignalBuffer::control(256);
        sync.samples[0] = 1.0;
        lfo.process(&[&SignalBuffer::control(256), &sync], &mut out, &params, &ProcessContext::with_transport(48000.0, 256, transport));
        assert_eq!(out[1].samples[0], 0.0);

        // ...until the transport goes back to the top
        transport.beat_position = Some(0.0);
        lfo.process(&[], &mut out, &params, &ProcessContext::with_transport(48000.0, 256, transport));
        assert_eq!(out[1].samples[0], 0.0);
        transport.beat_position = Some(2.0);
        lfo.process(&[], &mut out, &params, &ProcessContext::with_transport(48000.0, 256, transport));
        assert!((out[1].samples[0] - 0.5).abs() < 1e-6, "half a bar in: {}", out[1].samples[0]);
    }

    #[test]
    fn test_synced_lfo_holds_while_the_transport_is_stopped() {
        let mut lfo = Lfo::new();
        lfo.prepare(48000.0, 256);
        let mut out = vec![SignalBuffer::control(256); 2];
        let mut transport = TransportState::playing_at(120.0);
        transport.playing = false;
        transport.beat_position = Some(0.5);
        lfo.process(&[], &mut out, &[1.0, 0.0, 0.0, 1.0, division("1/2")], &ProcessContext::with_transport(48000.0, 256, transport));
        assert!(out[1].samples.iter().all(|&phase| (phase - 0.25).abs() < 1e-6));
    }

    #[test]
    fn test_very_slow_lfo_keeps_its_rate() {
        // 0.003 Hz: the step per sample is finer than f32 can add to a phase
        // past 0.5, which ran a 160-bar orbit half again too fast (#256)
        const BLOCK: usize = 4800;
        let mut lfo = Lfo::new();
        lfo.prepare(48000.0, BLOCK);
        let mut rate_cv = SignalBuffer::control(BLOCK);
        rate_cv.fill((0.3f32).log2());
        let sync = SignalBuffer::control(BLOCK);
        let mut out = vec![SignalBuffer::control(BLOCK); 2];
        let ctx = ProcessContext::new(48000.0, BLOCK);
        // 250 s: three quarters of a cycle, through the phases where f32 rounds worst
        for _ in 0..2500 {
            lfo.process(&[&rate_cv, &sync], &mut out, &[0.01, 0.0, 0.0, 1.0, 0.0], &ctx);
        }
        let last = *out[1].samples.last().unwrap();
        assert!((last - 0.75).abs() < 0.75e-3, "phase {last}, not 0.75");
    }

    #[test]
    fn test_synced_lfo_without_a_clock_runs_at_120_bpm() {
        let mut lfo = Lfo::new();
        lfo.prepare(48000.0, 24000);
        let mut out = vec![SignalBuffer::control(24000); 2];
        // Half a second at 120 BPM: one beat, half of a 1/2 cycle
        lfo.process(&[], &mut out, &[5.0, 0.0, 0.0, 1.0, division("1/2")], &ProcessContext::new(48000.0, 24000));
        let last = *out[1].samples.last().unwrap();
        assert!((last - 0.5).abs() < 1e-3, "phase {last}");
    }

    #[test]
    fn test_readout_shows_the_synced_rate() {
        let mut lfo = Lfo::new();
        lfo.prepare(48000.0, 256);
        let mut out = vec![SignalBuffer::control(256); 2];
        let ctx = ProcessContext::with_transport(48000.0, 256, TransportState::playing_at(90.0));

        lfo.process(&[], &mut out, &[1.0, 0.0, 0.0, 1.0, 0.0], &ctx);
        // Free-running, there's no synced rate, but the running rate is the knob's
        let free = lfo.readout(&[]).unwrap().values;
        assert_eq!(free[Lfo::READOUT_RATE], 0.0);
        assert!((free[Lfo::READOUT_RUNNING_RATE] - 1.0).abs() < 1e-3, "{} Hz", free[Lfo::READOUT_RUNNING_RATE]);

        // A dotted quarter at 90 BPM: 1.5 beats of 2/3 s, one cycle a second
        lfo.process(&[], &mut out, &[1.0, 0.0, 0.0, 1.0, division("1/4D")], &ctx);
        let synced = lfo.readout(&[]).unwrap().values;
        let rate = synced[Lfo::READOUT_RATE];
        assert!((rate - 1.0).abs() < 1e-6, "{rate} Hz");
        assert_eq!(synced[Lfo::READOUT_RUNNING_RATE], rate);
    }

    #[test]
    fn test_turning_sync_off_carries_on_from_the_same_phase() {
        let mut lfo = Lfo::new();
        lfo.prepare(48000.0, 256);
        let mut out = vec![SignalBuffer::control(256); 2];
        let mut transport = TransportState::playing_at(120.0);
        transport.beat_position = Some(0.75);
        lfo.process(&[], &mut out, &[1.0, 3.0, 0.0, 1.0, division("1/2")], &ProcessContext::with_transport(48000.0, 256, transport));
        let synced_end = *out[1].samples.last().unwrap();
        lfo.process(&[], &mut out, &[1.0, 3.0, 0.0, 1.0, 0.0], &ProcessContext::with_transport(48000.0, 256, transport));
        assert!((out[1].samples[0] - synced_end).abs() < 1e-3, "{} then {}", synced_end, out[1].samples[0]);
    }
}
