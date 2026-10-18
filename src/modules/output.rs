//! Audio output module.
//!
//! This module serves as the final destination in the signal chain,
//! collecting audio and routing it to the system speakers.

use crate::dsp::{
    primitives::knee_clip,
    context::ProcessContext,
    dynamics::{DcBlocker, PeakLimiter},
    module_trait::{DspModule, ModuleCategory, ModuleInfo, OutputLevels},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    SignalType,
};

/// The module's ID, as patches name it.
pub const ID: &str = "output.audio";

/// The audio output module that routes audio to the speakers.
///
/// This is the final destination in an audio graph. It accepts stereo
/// (Left/Right) or mono input and runs it through a mastering-style output
/// stage before it reaches the speakers.
///
/// # Ports
///
/// - **Left** (Audio, Input): Left channel input for stereo output.
/// - **Right** (Audio, Input): Right channel input for stereo output.
/// - **Mono** (Audio, Input): Mono input, routes to both channels.
///
/// # Parameters
///
/// - **Volume** (0.0-1.0): Master volume control, default 0.8.
/// - **Limiter** (Toggle): Transparent lookahead peak limiter with a
///   -0.3 dBFS ceiling, default on.
/// - **Character** (Toggle): Soft-clip saturation before the limiter, for
///   colour rather than protection, default off.
///
/// # Signal Path
///
/// 1. Mix: Left/Right, with Mono added to both channels. Non-finite samples
///    (NaN, infinity) are replaced with silence so they can't reach the DAC.
/// 2. DC blocker (5 Hz highpass) removes any constant offset.
/// 3. Volume.
/// 4. Character soft clip (optional).
/// 5. Peak limiter (optional; when off, the 1 ms lookahead delay remains so
///    toggling it doesn't shift timing).
///
/// Peak levels are measured on both sides of the limiter so the UI can show
/// how hard the patch drives the output and how much the limiter is doing.
pub struct AudioOutput {
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
    /// Internal stereo output buffer for the audio engine to read from.
    /// Index 0 = Left, Index 1 = Right.
    output_buffer: [Vec<f32>; 2],
    /// Smoothed volume parameter.
    volume_smooth: SmoothedValue,
    /// DC blockers, one per channel.
    dc_blockers: [DcBlocker; 2],
    /// Stereo-linked output limiter.
    limiter: PeakLimiter,
    /// Peaks going into the limiter since the levels were last taken.
    pre_peaks: [f32; 2],
    /// Peaks leaving the output stage since the levels were last taken.
    post_peaks: [f32; 2],
}

impl AudioOutput {
    /// Creates a new audio output module.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            sample_rate,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("left", "Left", SignalType::Audio, 0.0).describe("Left speaker signal"),
                PortDefinition::input_with_default("right", "Right", SignalType::Audio, 0.0).describe("Right speaker signal"),
                PortDefinition::input_with_default("mono", "Mono", SignalType::Audio, 0.0).describe("Sent equally to both speakers; easiest way to hear a patch"),
            ],
            // New parameters go at the end: patches restore them by position
            parameters: vec![
                ParameterDefinition::normalized("volume", "Volume", 0.8).describe("Master volume"),
                ParameterDefinition::toggle("limiter", "Limiter", true).describe("Squashes peaks transparently to prevent clipping"),
                ParameterDefinition::toggle("character", "Character", false).describe("Rounds off peaks above -3 dBFS; quieter material passes untouched"),
            ],
            output_buffer: [Vec::new(), Vec::new()],
            volume_smooth: SmoothedValue::with_default_smoothing(0.8, sample_rate),
            dc_blockers: [DcBlocker::new(sample_rate), DcBlocker::new(sample_rate)],
            limiter: PeakLimiter::new(sample_rate),
            pre_peaks: [0.0; 2],
            post_peaks: [0.0; 2],
        }
    }

    /// Port index constants for clarity.
    const PORT_LEFT: usize = 0;
    const PORT_RIGHT: usize = 1;
    const PORT_MONO: usize = 2;

    /// Parameter index constants.
    const PARAM_VOLUME: usize = 0;
    const PARAM_LIMITER: usize = 1;
    const PARAM_CHARACTER: usize = 2;

    /// Level (about -3 dBFS) below which the soft clipper is perfectly transparent.
    const CLIP_KNEE: f32 = 0.7;

    /// Applies a soft clipper that only acts on peaks.
    ///
    /// Below `CLIP_KNEE` the signal passes untouched, so normal-level mixes are
    /// not coloured. Above it, a tanh segment (matched in level and slope at the
    /// knee) rounds peaks off smoothly and never exceeds ±1.
    #[inline]
    fn soft_clip(sample: f32) -> f32 {
        knee_clip(sample, Self::CLIP_KNEE, 1.0)
    }

    /// Replaces NaN and infinity with silence.
    #[inline]
    fn sanitize(sample: f32) -> f32 {
        if sample.is_finite() {
            sample
        } else {
            0.0
        }
    }

    /// Returns the final stereo output buffer.
    ///
    /// This is how the audio engine can access the processed output.
    /// Index 0 = Left channel, Index 1 = Right channel.
    pub fn get_output_buffer(&self) -> &[Vec<f32>; 2] {
        &self.output_buffer
    }

    /// Delay the output stage adds, in samples (the limiter's lookahead).
    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }
}

impl Default for AudioOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for AudioOutput {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Audio Output",
            category: ModuleCategory::Output,
            description: "Master stereo output with DC blocking and a peak limiter",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, sample_rate: f32, max_block_size: usize) {
        self.sample_rate = sample_rate;
        // Pre-allocate output buffers
        self.output_buffer[0].resize(max_block_size, 0.0);
        self.output_buffer[1].resize(max_block_size, 0.0);
        // Update sample rate for smoothed parameters and the output stage
        self.volume_smooth.set_sample_rate(sample_rate);
        for blocker in &mut self.dc_blockers {
            blocker.set_sample_rate(sample_rate);
        }
        self.limiter.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        _outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Set smoothing target from parameter
        self.volume_smooth.set_target(params[Self::PARAM_VOLUME]);

        // Toggles, no smoothing needed
        let limiter_enabled = params[Self::PARAM_LIMITER] > 0.5;
        let character_enabled = params[Self::PARAM_CHARACTER] > 0.5;

        // Get input buffers (may be empty if not connected)
        let left_input = inputs.get(Self::PORT_LEFT);
        let right_input = inputs.get(Self::PORT_RIGHT);
        let mono_input = inputs.get(Self::PORT_MONO);

        let block_size = context.block_size.min(self.output_buffer[0].len());
        for i in 0..block_size {
            // Get smoothed volume (per-sample for click-free changes)
            let volume = self.volume_smooth.next();

            // Get input samples
            let left_sample = left_input
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);

            let right_sample = right_input
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);

            let mono_sample = mono_input
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);

            // Mix: L/R inputs plus mono added to both channels
            let mixed = [
                Self::sanitize(left_sample + mono_sample),
                Self::sanitize(right_sample + mono_sample),
            ];

            // DC blocker, volume, then optional colour
            let mut staged = [0.0; 2];
            for ch in 0..2 {
                let mut sample = self.dc_blockers[ch].process(mixed[ch]) * volume;
                if character_enabled {
                    sample = Self::soft_clip(sample);
                }
                self.pre_peaks[ch] = self.pre_peaks[ch].max(sample.abs());
                staged[ch] = sample;
            }

            let (out_left, out_right) = self.limiter.process(staged[0], staged[1], limiter_enabled);
            self.post_peaks[0] = self.post_peaks[0].max(out_left.abs());
            self.post_peaks[1] = self.post_peaks[1].max(out_right.abs());

            self.output_buffer[0][i] = out_left;
            self.output_buffer[1][i] = out_right;
        }
    }

    fn reset(&mut self) {
        // Clear output buffers
        for buf in &mut self.output_buffer {
            buf.fill(0.0);
        }
        for blocker in &mut self.dc_blockers {
            blocker.reset();
        }
        self.limiter.reset();
        self.pre_peaks = [0.0; 2];
        self.post_peaks = [0.0; 2];
        // Reset smoothed parameters to their current targets
        self.volume_smooth.reset(self.volume_smooth.target());
    }

    fn get_audio_output(&self) -> Option<(&[f32], &[f32])> {
        Some((&self.output_buffer[0], &self.output_buffer[1]))
    }

    fn take_output_levels(&mut self) -> Option<OutputLevels> {
        Some(OutputLevels {
            pre: std::mem::take(&mut self.pre_peaks),
            post: std::mem::take(&mut self.post_peaks),
            limiter_gain: self.limiter.take_min_gain(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::peak;
    use crate::dsp::dynamics::db_to_gain;

    const SR: f32 = 44100.0;
    const BLOCK: usize = 256;

    /// A sine with exactly 8 cycles per block, so repeating the same buffer
    /// block after block is seamless.
    fn looped_sine(amplitude: f32) -> SignalBuffer {
        let mut buf = SignalBuffer::audio(BLOCK);
        for (n, sample) in buf.samples.iter_mut().enumerate() {
            *sample = amplitude * (std::f32::consts::TAU * 8.0 * n as f32 / BLOCK as f32).sin();
        }
        buf
    }

    fn prepared() -> AudioOutput {
        let mut output = AudioOutput::new();
        output.prepare(SR, BLOCK);
        output
    }

    /// Feeds the same (left, right, mono) block `blocks` times.
    fn run(
        output: &mut AudioOutput,
        left: &SignalBuffer,
        right: &SignalBuffer,
        mono: &SignalBuffer,
        params: &[f32],
        blocks: usize,
    ) {
        let ctx = ProcessContext::new(SR, BLOCK);
        for _ in 0..blocks {
            output.process(&[left, right, mono], &mut [], params, &ctx);
        }
    }

    /// Asserts a channel of the last block is `expected`, delayed by the
    /// output stage's latency (the input loops every block).
    fn assert_delayed_copy(output: &AudioOutput, channel: usize, expected: &SignalBuffer) {
        let latency = output.latency();
        let out = &output.get_output_buffer()[channel];
        for i in 0..BLOCK {
            let want = expected.samples[(i + BLOCK - latency % BLOCK) % BLOCK];
            assert!(
                (out[i] - want).abs() < 0.01,
                "channel {} sample {}: expected {}, got {}",
                channel,
                i,
                want,
                out[i]
            );
        }
    }

    fn silence() -> SignalBuffer {
        SignalBuffer::audio(BLOCK)
    }

    #[test]
    fn test_audio_output_info() {
        let output = AudioOutput::new();
        assert_eq!(output.info().id, "output.audio");
        assert_eq!(output.info().name, "Audio Output");
        assert_eq!(output.info().category, ModuleCategory::Output);
    }

    #[test]
    fn test_audio_output_ports() {
        let output = AudioOutput::new();
        let ports = output.ports();

        assert_eq!(ports.len(), 3);

        // All are audio inputs
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "left");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "right");
        assert_eq!(ports[1].signal_type, SignalType::Audio);

        assert!(ports[2].is_input());
        assert_eq!(ports[2].id, "mono");
        assert_eq!(ports[2].signal_type, SignalType::Audio);
    }

    #[test]
    fn test_audio_output_parameters() {
        let output = AudioOutput::new();
        let params = output.parameters();

        assert_eq!(params.len(), 3);

        // Volume parameter
        assert_eq!(params[0].id, "volume");
        assert_eq!(params[0].min, 0.0);
        assert_eq!(params[0].max, 1.0);
        assert_eq!(params[0].default, 0.8);

        // Limiter keeps its id and position, so saved patches still load
        assert_eq!(params[1].id, "limiter");
        assert_eq!(params[1].min, 0.0);
        assert_eq!(params[1].max, 1.0);
        assert_eq!(params[1].default, 1.0); // true = 1.0

        // Character is new, appended, and off by default
        assert_eq!(params[2].id, "character");
        assert_eq!(params[2].default, 0.0);
    }

    #[test]
    fn test_mono_routes_to_both_channels() {
        let mut output = prepared();
        let mono = looped_sine(0.5);
        run(&mut output, &silence(), &silence(), &mono, &[1.0, 1.0, 0.0], 20);

        assert_delayed_copy(&output, 0, &mono);
        assert_delayed_copy(&output, 1, &mono);
    }

    #[test]
    fn test_stereo_inputs() {
        let mut output = prepared();
        let left = looped_sine(0.3);
        let right = looped_sine(0.7);
        run(&mut output, &left, &right, &silence(), &[1.0, 1.0, 0.0], 20);

        assert_delayed_copy(&output, 0, &left);
        assert_delayed_copy(&output, 1, &right);
    }

    #[test]
    fn test_mono_added_to_stereo() {
        let mut output = prepared();
        run(
            &mut output,
            &looped_sine(0.2),
            &looped_sine(0.3),
            &looped_sine(0.1),
            &[1.0, 1.0, 0.0],
            20,
        );

        // L = 0.2 + 0.1, R = 0.3 + 0.1
        assert_delayed_copy(&output, 0, &looped_sine(0.3));
        assert_delayed_copy(&output, 1, &looped_sine(0.4));
    }

    #[test]
    fn test_volume_scales_output() {
        let mut output = prepared();
        run(&mut output, &silence(), &silence(), &looped_sine(1.0), &[0.5, 0.0, 0.0], 20);

        assert_delayed_copy(&output, 0, &looped_sine(0.5));
    }

    #[test]
    fn test_limiter_holds_ceiling_on_plus_12_db_input() {
        let mut output = prepared();
        let ceiling = db_to_gain(PeakLimiter::DEFAULT_CEILING_DB);
        let loud = looped_sine(db_to_gain(12.0));
        let ctx = ProcessContext::new(SR, BLOCK);

        // Every sample of every block, including the onset
        for _ in 0..40 {
            output.process(&[&silence(), &silence(), &loud], &mut [], &[1.0, 1.0, 0.0], &ctx);
            for channel in output.get_output_buffer() {
                for &sample in channel.iter() {
                    assert!(sample.abs() <= ceiling, "{} exceeds ceiling {}", sample, ceiling);
                }
            }
        }

        // Limited, not silenced: peaks sit right at the ceiling
        let last_peak = peak(&output.get_output_buffer()[0]);
        assert!(last_peak > ceiling - 0.01, "peak {} should reach the ceiling", last_peak);
    }

    #[test]
    fn test_limiter_off_allows_clipping() {
        let mut output = prepared();
        let loud = looped_sine(2.0);
        run(&mut output, &silence(), &silence(), &loud, &[1.0, 0.0, 0.0], 20);

        // Output should NOT be limited
        assert_delayed_copy(&output, 0, &loud);
    }

    #[test]
    fn test_character_soft_clips_before_limiter() {
        // Character alone: rounded off at full scale
        let mut output = prepared();
        run(&mut output, &silence(), &silence(), &looped_sine(5.0), &[1.0, 0.0, 1.0], 20);
        let clipped = peak(&output.get_output_buffer()[0]);
        assert!(clipped <= 1.0 && clipped > 0.95, "soft clip peak {}", clipped);

        // Character plus limiter: still held to the ceiling
        let mut output = prepared();
        run(&mut output, &silence(), &silence(), &looped_sine(5.0), &[1.0, 1.0, 1.0], 20);
        let ceiling = db_to_gain(PeakLimiter::DEFAULT_CEILING_DB);
        assert!(peak(&output.get_output_buffer()[0]) <= ceiling);

        // Quiet signals pass through Character untouched
        let mut output = prepared();
        let quiet = looped_sine(0.5);
        run(&mut output, &silence(), &silence(), &quiet, &[1.0, 1.0, 1.0], 20);
        assert_delayed_copy(&output, 0, &quiet);
    }

    #[test]
    fn test_dc_offset_decays_to_zero() {
        let mut output = prepared();
        let mut offset = silence();
        offset.fill(0.5);

        // One second of pure DC
        let blocks = (SR as usize) / BLOCK;
        run(&mut output, &offset, &offset, &silence(), &[1.0, 1.0, 0.0], blocks);

        for channel in output.get_output_buffer() {
            assert!(peak(channel) < 1e-3, "DC should be removed, peak {}", peak(channel));
        }
    }

    #[test]
    fn test_non_finite_input_is_silenced() {
        let mut output = prepared();
        let mut broken = looped_sine(0.5);
        broken.samples[10] = f32::NAN;
        broken.samples[20] = f32::INFINITY;
        run(&mut output, &silence(), &silence(), &broken, &[1.0, 1.0, 0.0], 4);

        for channel in output.get_output_buffer() {
            assert!(channel.iter().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn test_metering_reports_pre_and_post_limiter_peaks() {
        let mut output = prepared();
        run(&mut output, &silence(), &silence(), &looped_sine(2.0), &[1.0, 1.0, 0.0], 20);

        let levels = output.take_output_levels().unwrap();
        let ceiling = db_to_gain(PeakLimiter::DEFAULT_CEILING_DB);
        for ch in 0..2 {
            assert!((levels.pre[ch] - 2.0).abs() < 0.05, "pre peak {}", levels.pre[ch]);
            assert!(levels.post[ch] <= ceiling && levels.post[ch] > ceiling - 0.01);
        }
        // About 6.3 dB of gain reduction
        assert!((levels.limiter_gain - ceiling / 2.0).abs() < 0.02, "gain {}", levels.limiter_gain);

        // Taking the levels starts a new measurement
        let again = output.take_output_levels().unwrap();
        assert_eq!(again, OutputLevels::default());
    }

    #[test]
    fn test_reset() {
        let mut output = prepared();
        run(&mut output, &silence(), &silence(), &looped_sine(0.8), &[1.0, 1.0, 0.0], 1);

        output.reset();

        // Buffers should be cleared
        for sample in &output.get_output_buffer()[0] {
            assert_eq!(*sample, 0.0);
        }
        assert_eq!(output.take_output_levels().unwrap(), OutputLevels::default());

        // And the lookahead delay holds no stale audio
        run(&mut output, &silence(), &silence(), &silence(), &[1.0, 1.0, 0.0], 1);
        assert_eq!(peak(&output.get_output_buffer()[0]), 0.0);
    }

    #[test]
    fn test_audio_output_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AudioOutput>();
    }

    #[test]
    fn test_audio_output_default() {
        let output = AudioOutput::default();
        assert_eq!(output.info().id, "output.audio");
    }

    #[test]
    fn test_audio_output_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<AudioOutput>();

        assert!(registry.contains("output.audio"));

        let module = registry.create("output.audio");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "output.audio");
        assert_eq!(module.info().name, "Audio Output");
        assert_eq!(module.ports().len(), 3);
        assert_eq!(module.parameters().len(), 3);
    }
}
