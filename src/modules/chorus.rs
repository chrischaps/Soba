//! Stereo Chorus effect module.
//!
//! A multi-voice chorus/flanger. Each channel runs through its own delay
//! line, and every voice is a pair of LFO-swept taps, one per line. The
//! right tap's LFO runs half a voice-spacing behind the left's, so even a
//! mono input comes out wide, while a stereo input keeps its image.

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

use std::f32::consts::TAU;

/// Maximum delay time in seconds (for buffer allocation): the 30 ms top of
/// the Delay knob swung by full depth, with room for the interpolator.
const MAX_DELAY_SECONDS: f32 = 0.065;

/// Number of voices the module can run.
const VOICE_SLOTS: usize = 4;

/// How long a voice takes to fade in or out, and to slide to its new LFO
/// phase, when the Voices choice changes.
const VOICE_GLIDE_MS: f32 = 30.0;

/// How long the LFO takes to morph between sine and triangle.
const SHAPE_GLIDE_MS: f32 = 20.0;

/// LFO value at `phase` (0..1), morphed from sine (`triangle` = 0) to
/// triangle (`triangle` = 1). Both start at 0 rising and peak at 1/4, so
/// the morph never jumps.
#[inline]
pub fn lfo(phase: f32, triangle: f32) -> f32 {
    let tri = 1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs();
    if triangle >= 1.0 {
        return tri;
    }
    let sine = (phase * TAU).sin();
    sine + triangle * (tri - sine)
}

/// Where voice `slot` sits on the LFO cycle when `voices` are running:
/// (left phase, right phase). Voices are spread evenly around the cycle,
/// and each right tap sits halfway to the next voice: 180° for one voice,
/// 90° for two. A smaller offset would leave the right channel sweeping
/// the same set of phases as the left once there are four voices.
#[inline]
pub fn voice_phases(slot: usize, voices: usize) -> (f32, f32) {
    let spacing = 1.0 / voices as f32;
    let left = slot as f32 * spacing;
    (left, left + 0.5 * spacing)
}

/// One channel's delay line.
struct DelayLine {
    /// Circular buffer.
    buffer: Vec<f32>,
    /// Current write position.
    write_pos: usize,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
            write_pos: 0,
        }
    }

    /// Read `delay_samples` behind the write head, with linear interpolation.
    #[inline]
    fn read(&self, delay_samples: f32) -> f32 {
        let buffer_size = self.buffer.len();
        let delay_samples = delay_samples.clamp(1.0, (buffer_size - 2) as f32);
        let int_delay = delay_samples as usize;
        let frac = delay_samples - int_delay as f32;

        let read_pos_1 = (self.write_pos + buffer_size - int_delay) % buffer_size;
        let read_pos_2 = if read_pos_1 == 0 { buffer_size - 1 } else { read_pos_1 - 1 };

        let sample_1 = self.buffer[read_pos_1];
        let sample_2 = self.buffer[read_pos_2];
        sample_1 + frac * (sample_2 - sample_1)
    }

    #[inline]
    fn write(&mut self, sample: f32) {
        self.buffer[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % self.buffer.len();
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }
}

/// A voice: how loud it is and where its two taps sit on the LFO cycle,
/// all gliding so a change in the Voices choice never clicks.
struct VoiceSlot {
    gain: SmoothedValue,
    phase_left: SmoothedValue,
    phase_right: SmoothedValue,
}

impl VoiceSlot {
    fn new(slot: usize, voices: usize, sample_rate: f32) -> Self {
        let active = slot < voices;
        let (left, right) = voice_phases(slot, voices.max(slot + 1));
        Self {
            gain: SmoothedValue::new(if active { 1.0 } else { 0.0 }, VOICE_GLIDE_MS, sample_rate),
            phase_left: SmoothedValue::new(left, VOICE_GLIDE_MS, sample_rate),
            phase_right: SmoothedValue::new(right, VOICE_GLIDE_MS, sample_rate),
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.gain.set_sample_rate(sample_rate);
        self.phase_left.set_sample_rate(sample_rate);
        self.phase_right.set_sample_rate(sample_rate);
    }

    fn settle(&mut self) {
        self.gain.reset(self.gain.target());
        self.phase_left.reset(self.phase_left.target());
        self.phase_right.reset(self.phase_right.target());
    }
}

/// Stereo chorus effect with multiple voices.
///
/// # Ports
///
/// - **In L** (Audio, Input): Left channel input.
/// - **In R** (Audio, Input): Right channel input (normalled from L).
/// - **Rate CV** (Control, Input): Modulates LFO rate.
/// - **Depth CV** (Control, Input): Modulates modulation depth.
/// - **Out L** (Audio, Output): Processed left channel.
/// - **Out R** (Audio, Output): Processed right channel.
///
/// # Parameters
///
/// - **Rate** (0.1-10 Hz): LFO speed.
/// - **Depth** (0-100%): Modulation depth.
/// - **Delay** (1-30 ms): Base delay time.
/// - **Feedback** (-50% to +50%): For flanger effect.
/// - **Voices** (1-4): Number of chorus voices.
/// - **Mix** (0-100%): Wet/dry blend.
/// - **Shape** (Sine/Tri): LFO waveform.
pub struct Chorus {
    /// Sample rate.
    sample_rate: f32,
    /// Left and right delay lines, shared by all voices.
    lines: [DelayLine; 2],
    /// Voice slots (up to 4).
    voices: [VoiceSlot; VOICE_SLOTS],
    /// Master LFO phase (0.0 to 1.0); each voice is an offset from it.
    lfo_phase: f32,
    /// Smoothed rate parameter.
    rate_smooth: SmoothedValue,
    /// Smoothed depth parameter.
    depth_smooth: SmoothedValue,
    /// Smoothed delay parameter.
    delay_smooth: SmoothedValue,
    /// Smoothed feedback parameter.
    feedback_smooth: SmoothedValue,
    /// Smoothed mix parameter.
    mix_smooth: SmoothedValue,
    /// LFO shape, 0 = sine to 1 = triangle, smoothed so switching morphs.
    shape_smooth: SmoothedValue,
    /// The rate and depth after CV at the end of the last block, for the display.
    rate_now: f32,
    depth_now: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl Chorus {
    /// Creates a new stereo chorus.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let max_samples = (MAX_DELAY_SECONDS * sample_rate) as usize;
        let default_voices = 2;

        Self {
            sample_rate,
            lines: [DelayLine::new(max_samples), DelayLine::new(max_samples)],
            voices: std::array::from_fn(|slot| VoiceSlot::new(slot, default_voices, sample_rate)),
            lfo_phase: 0.0,
            rate_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            depth_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            delay_smooth: SmoothedValue::new(10.0, 20.0, sample_rate), // 20ms smoothing for delay
            feedback_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            mix_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            shape_smooth: SmoothedValue::new(0.0, SHAPE_GLIDE_MS, sample_rate),
            rate_now: 1.0,
            depth_now: 0.5,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in_l", "In L", SignalType::Audio, 0.0).describe("Left audio to thicken"),
                PortDefinition::input_with_default("in_r", "In R", SignalType::Audio, 0.0).describe("Right audio to thicken; copies left when unpatched"),
                PortDefinition::input_with_default("rate_cv", "Rate CV", SignalType::Control, 0.0).describe("CV that speeds up or slows down the sweep by up to 50%"),
                PortDefinition::input_with_default("depth_cv", "Depth CV", SignalType::Control, 0.0).describe("CV that adds to the sweep depth"),
                // Output ports
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("Left chorused audio, mixed with the dry signal"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("Right chorused audio, mixed with the dry signal"),
            ],
            parameters: vec![
                ParameterDefinition::new(
                    "rate",
                    "Rate",
                    0.1,
                    10.0,
                    1.0,
                    ParameterDisplay::Logarithmic { unit: "Hz" },
                ).describe("Speed of the sweeping delay, in Hz"),
                ParameterDefinition::normalized("depth", "Depth", 0.5).describe("How far the delay sweeps; more gives a deeper wobble"),
                ParameterDefinition::new(
                    "delay",
                    "Delay",
                    1.0,
                    30.0,
                    10.0,
                    ParameterDisplay::Logarithmic { unit: "ms" },
                ).describe("Base delay time the sweep moves around, in ms"),
                ParameterDefinition::new(
                    "feedback",
                    "Feedback",
                    -0.5,
                    0.5,
                    0.0,
                    ParameterDisplay::Linear { unit: "" },
                ).describe("Feeds output back in; negative values give a hollower sound"),
                ParameterDefinition::choice(
                    "voices",
                    "Voices",
                    &["1", "2", "3", "4"],
                    1, // Default: 2 voices
                ).describe("Number of delayed copies layered together, 1 to 4"),
                ParameterDefinition::normalized("mix", "Mix", 0.5).describe("Blend from dry (0) to chorus only (1)"),
                // Appended, so patches saved before it existed keep the sine
                ParameterDefinition::choice("shape", "Shape", &["Sine", "Tri"], 0).describe("Sweep waveform; Sine is smooth, Tri is more even"),
            ],
        }
    }

    /// Readout slots: the master LFO phase (0 to 1), the rate (Hz) and
    /// depth (0 to 1) after CV, the shape (0 sine to 1 triangle), then for
    /// each voice slot its gain and its left and right taps' phase offsets.
    pub const READOUT_PHASE: usize = 0;
    pub const READOUT_RATE: usize = 1;
    pub const READOUT_DEPTH: usize = 2;
    pub const READOUT_SHAPE: usize = 3;
    pub const READOUT_VOICES: usize = 4;
    /// Readout values per voice slot: gain, left offset, right offset.
    pub const READOUT_PER_VOICE: usize = 3;
    /// How many voice slots the readout carries.
    pub const SLOTS: usize = VOICE_SLOTS;

    /// Port index constants.
    const PORT_IN_L: usize = 0;
    const PORT_IN_R: usize = 1;
    const PORT_RATE_CV: usize = 2;
    const PORT_DEPTH_CV: usize = 3;
    const PORT_OUT_L: usize = 0;
    const PORT_OUT_R: usize = 1;

    /// Parameter index constants.
    const PARAM_RATE: usize = 0;
    const PARAM_DEPTH: usize = 1;
    const PARAM_DELAY: usize = 2;
    const PARAM_FEEDBACK: usize = 3;
    const PARAM_VOICES: usize = 4;
    const PARAM_MIX: usize = 5;
    const PARAM_SHAPE: usize = 6;
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Chorus {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "fx.chorus",
            name: "Chorus",
            category: ModuleCategory::Effect,
            description: "Stereo chorus/flanger with multiple voices",
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

        // Resize the delay lines if needed
        let max_samples = (MAX_DELAY_SECONDS * sample_rate) as usize;
        for line in &mut self.lines {
            if line.buffer.len() != max_samples {
                line.buffer.resize(max_samples, 0.0);
                line.reset();
            }
        }

        // Update sample rate for smoothed values
        self.rate_smooth.set_sample_rate(sample_rate);
        self.depth_smooth.set_sample_rate(sample_rate);
        self.delay_smooth.set_sample_rate(sample_rate);
        self.feedback_smooth.set_sample_rate(sample_rate);
        self.mix_smooth.set_sample_rate(sample_rate);
        self.shape_smooth.set_sample_rate(sample_rate);
        for voice in &mut self.voices {
            voice.set_sample_rate(sample_rate);
        }
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Set smoothing targets
        self.rate_smooth.set_target(params[Self::PARAM_RATE]);
        self.depth_smooth.set_target(params[Self::PARAM_DEPTH]);
        self.delay_smooth.set_target(params[Self::PARAM_DELAY]);
        self.feedback_smooth.set_target(params[Self::PARAM_FEEDBACK]);
        self.mix_smooth.set_target(params[Self::PARAM_MIX]);
        let triangle = params.get(Self::PARAM_SHAPE).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        self.shape_smooth.set_target(triangle);

        // Fade voices in and out, and slide the running ones to their
        // places for the new count. A voice fading out keeps its phase.
        let num_voices = (params[Self::PARAM_VOICES] as usize + 1).clamp(1, VOICE_SLOTS);
        for (slot, voice) in self.voices.iter_mut().enumerate() {
            if slot < num_voices {
                let (left, right) = voice_phases(slot, num_voices);
                voice.gain.set_target(1.0);
                voice.phase_left.set_target(left);
                voice.phase_right.set_target(right);
            } else {
                voice.gain.set_target(0.0);
            }
        }

        // Get input buffers
        let in_left = inputs.get(Self::PORT_IN_L);
        // Right is normalled from left when nothing is plugged into it
        let in_right = connected_input(inputs, Self::PORT_IN_R);
        let rate_cv = inputs.get(Self::PORT_RATE_CV);
        let depth_cv = inputs.get(Self::PORT_DEPTH_CV);

        // Split outputs
        let (out_left_slice, out_right_slice) = outputs.split_at_mut(Self::PORT_OUT_R);
        let out_left = &mut out_left_slice[Self::PORT_OUT_L];
        let out_right = &mut out_right_slice[0];

        // Process each sample
        for i in 0..context.block_size {
            // Get smoothed values
            let rate_smoothed = self.rate_smooth.next();
            let depth_smoothed = self.depth_smooth.next();
            let delay_ms_smoothed = self.delay_smooth.next();
            let feedback_smoothed = self.feedback_smooth.next();
            let mix_smoothed = self.mix_smooth.next();
            let shape = self.shape_smooth.next();

            // Apply rate CV modulation (bipolar, +/- 50% range)
            let rate_mod = rate_cv
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);
            let modulated_rate = (rate_smoothed * (1.0 + rate_mod * 0.5)).clamp(0.1, 10.0);

            // Apply depth CV modulation
            let depth_mod = depth_cv
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);
            let modulated_depth = (depth_smoothed + depth_mod * 0.5).clamp(0.0, 1.0);

            self.rate_now = modulated_rate;
            self.depth_now = modulated_depth;

            // Advance the master LFO
            self.lfo_phase += modulated_rate / self.sample_rate;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            // Base delay in samples, and how far the LFO swings it
            let base_delay_samples = (delay_ms_smoothed * 0.001 * self.sample_rate).max(1.0);
            let swing = modulated_depth * base_delay_samples;

            // Get dry input samples
            let dry_left = in_left
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);

            // Right channel normalled from left if not connected
            let dry_right = match in_right {
                Some(buf) => buf.at(i),
                None => dry_left,
            };

            // Each voice taps both lines, each tap swept by its own LFO phase
            let mut wet_left = 0.0;
            let mut wet_right = 0.0;
            let mut total_gain = 0.0;

            for voice in &mut self.voices {
                let gain = voice.gain.next();
                let phase_left = voice.phase_left.next();
                let phase_right = voice.phase_right.next();
                if gain == 0.0 {
                    continue;
                }

                let lfo_left = lfo((self.lfo_phase + phase_left).fract(), shape);
                let lfo_right = lfo((self.lfo_phase + phase_right).fract(), shape);
                wet_left += gain * self.lines[0].read(base_delay_samples + lfo_left * swing);
                wet_right += gain * self.lines[1].read(base_delay_samples + lfo_right * swing);
                total_gain += gain;
            }

            // Feed back the voices' average, so the loop gain never exceeds
            // the Feedback knob however many voices run
            let feedback = feedback_smoothed / total_gain;
            self.lines[0].write(flush(dry_left + wet_left * feedback));
            self.lines[1].write(flush(dry_right + wet_right * feedback));

            // Voices add up like uncorrelated signals
            let voice_scale = 1.0 / total_gain.sqrt();
            wet_left *= voice_scale;
            wet_right *= voice_scale;

            // Mix dry and wet signals
            out_left.samples[i] = dry_left * (1.0 - mix_smoothed) + wet_left * mix_smoothed;
            out_right.samples[i] = dry_right * (1.0 - mix_smoothed) + wet_right * mix_smoothed;
        }
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.reset();
        }
        // Keep LFO phase for continuity

        // Reset smoothed values
        self.rate_smooth.reset(self.rate_smooth.target());
        self.depth_smooth.reset(self.depth_smooth.target());
        self.delay_smooth.reset(self.delay_smooth.target());
        self.feedback_smooth.reset(self.feedback_smooth.target());
        self.mix_smooth.reset(self.mix_smooth.target());
        self.shape_smooth.reset(self.shape_smooth.target());
        for voice in &mut self.voices {
            voice.settle();
        }
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_PHASE] = self.lfo_phase;
        readout.values[Self::READOUT_RATE] = self.rate_now;
        readout.values[Self::READOUT_DEPTH] = self.depth_now;
        readout.values[Self::READOUT_SHAPE] = self.shape_smooth.current();
        for (slot, voice) in self.voices.iter().enumerate() {
            let at = Self::READOUT_VOICES + slot * Self::READOUT_PER_VOICE;
            readout.values[at] = voice.gain.current();
            readout.values[at + 1] = voice.phase_left.current();
            readout.values[at + 2] = voice.phase_right.current();
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chorus_info() {
        let chorus = Chorus::new();
        assert_eq!(chorus.info().id, "fx.chorus");
        assert_eq!(chorus.info().name, "Chorus");
        assert_eq!(chorus.info().category, ModuleCategory::Effect);
    }

    #[test]
    fn test_chorus_ports() {
        let chorus = Chorus::new();
        let ports = chorus.ports();

        assert_eq!(ports.len(), 6);

        // Input ports
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in_l");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "in_r");
        assert_eq!(ports[1].signal_type, SignalType::Audio);

        assert!(ports[2].is_input());
        assert_eq!(ports[2].id, "rate_cv");
        assert_eq!(ports[2].signal_type, SignalType::Control);

        assert!(ports[3].is_input());
        assert_eq!(ports[3].id, "depth_cv");
        assert_eq!(ports[3].signal_type, SignalType::Control);

        // Output ports
        assert!(ports[4].is_output());
        assert_eq!(ports[4].id, "out_l");
        assert_eq!(ports[4].signal_type, SignalType::Audio);

        assert!(ports[5].is_output());
        assert_eq!(ports[5].id, "out_r");
        assert_eq!(ports[5].signal_type, SignalType::Audio);
    }

    #[test]
    fn test_chorus_parameters() {
        let chorus = Chorus::new();
        let params = chorus.parameters();

        assert_eq!(params.len(), 7);
        assert_eq!(params[0].id, "rate");
        assert_eq!(params[1].id, "depth");
        assert_eq!(params[2].id, "delay");
        assert_eq!(params[3].id, "feedback");
        assert_eq!(params[4].id, "voices");
        assert_eq!(params[5].id, "mix");
        assert_eq!(params[6].id, "shape");
    }

    #[test]
    fn test_chorus_produces_output() {
        let mut chorus = Chorus::new();
        chorus.prepare(44100.0, 256);

        // Create a constant input signal
        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);

        let empty_cv = SignalBuffer::control(256);
        let mut outputs = vec![
            SignalBuffer::audio(256), // Out L
            SignalBuffer::audio(256), // Out R
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process with default settings: 1Hz rate, 50% depth, 10ms delay, 0 feedback, 2 voices, 50% mix
        chorus.process(
            &[&input, &input, &empty_cv, &empty_cv],
            &mut outputs,
            &[1.0, 0.5, 10.0, 0.0, 1.0, 0.5],
            &ctx,
        );

        // Output should have signal
        let has_output = outputs[0].samples.iter().any(|&s| s.abs() > 0.1);
        assert!(has_output, "Expected output signal");
    }

    #[test]
    fn test_chorus_stereo_spread() {
        let mut chorus = Chorus::new();
        let block_size = 1024;
        chorus.prepare(44100.0, block_size);

        // Create a test tone
        let mut input = SignalBuffer::audio(block_size);
        for (i, sample) in input.samples.iter_mut().enumerate() {
            *sample = (i as f32 * 0.1).sin();
        }

        let empty_cv = SignalBuffer::control(block_size);
        let mut outputs = vec![
            SignalBuffer::audio(block_size),
            SignalBuffer::audio(block_size),
        ];
        let ctx = ProcessContext::new(44100.0, block_size);

        // Process multiple blocks to let delay lines fill up
        for _ in 0..5 {
            chorus.process(
                &[&input, &input, &empty_cv, &empty_cv],
                &mut outputs,
                &[1.0, 0.5, 10.0, 0.0, 1.0, 1.0], // 100% wet
                &ctx,
            );
        }

        // Left and right should be different due to stereo spread
        let mut diff_count = 0;
        for i in 0..block_size {
            if (outputs[0].samples[i] - outputs[1].samples[i]).abs() > 0.001 {
                diff_count += 1;
            }
        }
        assert!(diff_count > 100, "Expected stereo difference, got {} diffs", diff_count);
    }

    #[test]
    fn test_chorus_reset() {
        let mut chorus = Chorus::new();
        chorus.prepare(44100.0, 256);

        // Fill buffer with signal
        let mut input = SignalBuffer::audio(256);
        input.fill(1.0);
        let empty_cv = SignalBuffer::control(256);
        let mut outputs = vec![
            SignalBuffer::audio(256),
            SignalBuffer::audio(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        chorus.process(
            &[&input, &input, &empty_cv, &empty_cv],
            &mut outputs,
            &[1.0, 0.5, 10.0, 0.5, 1.0, 1.0],
            &ctx,
        );

        // Reset
        chorus.reset();

        // Process silence
        let silence = SignalBuffer::audio(256);
        let mut outputs2 = vec![
            SignalBuffer::audio(256),
            SignalBuffer::audio(256),
        ];

        chorus.process(
            &[&silence, &silence, &empty_cv, &empty_cv],
            &mut outputs2,
            &[1.0, 0.5, 10.0, 0.0, 1.0, 1.0], // No feedback
            &ctx,
        );

        // Output should be near zero (buffers cleared)
        assert!(
            outputs2[0].samples[0].abs() < 0.01,
            "Expected near-zero output after reset, got {}",
            outputs2[0].samples[0]
        );
    }

    /// Plays `seconds` of audio through a chorus at 48 kHz in 256-sample
    /// blocks. `left(n)` and `right(n)` give the input at sample n; `right`
    /// None leaves In R unpatched. `params(block)` gives each block's knobs.
    /// Returns (out L, out R).
    fn play(
        seconds: f32,
        left: impl Fn(usize) -> f32,
        right: Option<&dyn Fn(usize) -> f32>,
        params: impl Fn(usize) -> [f32; 7],
    ) -> (Vec<f32>, Vec<f32>) {
        let block = 256;
        let mut chorus = Chorus::new();
        chorus.prepare(48000.0, block);
        let ctx = ProcessContext::new(48000.0, block);
        let cv = SignalBuffer::control(block);
        let mut in_l = SignalBuffer::audio(block);
        let mut in_r = match right {
            Some(_) => SignalBuffer::audio(block),
            None => SignalBuffer::unconnected(block, SignalType::Audio),
        };
        let mut outputs = vec![SignalBuffer::audio(block), SignalBuffer::audio(block)];
        let (mut out_l, mut out_r) = (Vec::new(), Vec::new());

        for b in 0..(seconds * 48000.0) as usize / block {
            for j in 0..block {
                let n = b * block + j;
                in_l.samples[j] = left(n);
                if let Some(right) = right {
                    in_r.samples[j] = right(n);
                }
            }
            chorus.process(&[&in_l, &in_r, &cv, &cv], &mut outputs, &params(b), &ctx);
            out_l.extend_from_slice(&outputs[0].samples);
            out_r.extend_from_slice(&outputs[1].samples);
        }
        (out_l, out_r)
    }

    fn sine(freq: f32) -> impl Fn(usize) -> f32 {
        move |n| 0.5 * (TAU * freq * n as f32 / 48000.0).sin()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn test_chorus_keeps_left_and_right_apart() {
        // A tone on the left only, silence patched into the right, fully
        // wet with feedback: nothing of the left may reach Out R
        let silence = |_| 0.0;
        for voices in 0..4 {
            let (out_l, out_r) =
                play(0.5, sine(440.0), Some(&silence), |_| [1.0, 0.8, 10.0, 0.4, voices as f32, 1.0, 0.0]);
            assert!(peak(&out_l) > 0.2, "{} voices: left went quiet", voices + 1);
            assert_eq!(peak(&out_r), 0.0, "{} voices: left leaked into the right", voices + 1);
        }
    }

    #[test]
    fn test_chorus_mono_input_comes_out_wide() {
        // In R unpatched: In L is normalled across, and the right taps'
        // LFO phases still make the two sides differ, at every voice count
        for voices in 0..4 {
            let (out_l, out_r) = play(1.0, sine(440.0), None, |_| [1.0, 0.8, 10.0, 0.0, voices as f32, 1.0, 0.0]);
            let tail = out_l.len() / 2;
            let side: Vec<f32> = out_l[tail..].iter().zip(&out_r[tail..]).map(|(l, r)| l - r).collect();
            assert!(
                peak(&side) > 0.1 * peak(&out_l[tail..]),
                "{} voices: L and R nearly identical (side peak {})",
                voices + 1,
                peak(&side)
            );
        }
    }

    #[test]
    fn test_lfo_shapes() {
        for (phase, value) in [(0.0, 0.0), (0.25, 1.0), (0.5, 0.0), (0.75, -1.0)] {
            assert!((lfo(phase, 0.0) - value).abs() < 1e-6);
            assert!((lfo(phase, 1.0) - value).abs() < 1e-6);
        }
        // Halfway up, the triangle is a straight line, the sine already bowed
        assert!((lfo(0.125, 1.0) - 0.5).abs() < 1e-6);
        assert!((lfo(0.125, 0.0) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((lfo(0.125, 0.5) - (0.5 + std::f32::consts::FRAC_1_SQRT_2) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_triangle_lfo_changes_the_sound() {
        let params = |shape: f32| move |_| [2.0, 1.0, 10.0, 0.0, 0.0, 1.0, shape];
        let (sine_out, _) = play(0.5, sine(440.0), None, params(0.0));
        let (tri_out, _) = play(0.5, sine(440.0), None, params(1.0));
        let diff: Vec<f32> = sine_out.iter().zip(&tri_out).map(|(a, b)| a - b).collect();
        assert!(peak(&diff) > 0.05);
    }

    /// Largest sample-to-sample step: a click shows up as a jump far
    /// bigger than anything the chorused tone makes on its own.
    fn largest_step(x: &[f32]) -> f32 {
        x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
    }

    #[test]
    fn test_changing_voices_or_shape_does_not_click() {
        // Steady for 0.5 s, then a switch every 0.25 s
        let switches = |block: usize| {
            let step = (block * 256) / 12000;
            let voices = [1.0, 3.0, 0.0, 2.0, 3.0, 1.0][step.min(5)];
            let shape = [0.0, 0.0, 1.0, 1.0, 0.0, 0.0][step.min(5)];
            [1.5, 0.6, 12.0, 0.3, voices, 1.0, shape]
        };
        let (out_l, out_r) = play(1.5, sine(220.0), None, switches);
        let steady = 12000..24000;
        let ceiling_l = 1.5 * largest_step(&out_l[steady.clone()]);
        let ceiling_r = 1.5 * largest_step(&out_r[steady]);
        assert!(largest_step(&out_l[24000..]) < ceiling_l, "left clicked");
        assert!(largest_step(&out_r[24000..]) < ceiling_r, "right clicked");
    }

    #[test]
    fn test_full_depth_at_longest_delay_stays_in_the_buffer() {
        // 30 ms swung by full depth plus depth CV reaches 60 ms
        let mut chorus = Chorus::new();
        chorus.prepare(48000.0, 256);
        let ctx = ProcessContext::new(48000.0, 256);
        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);
        let mut depth_cv = SignalBuffer::control(256);
        depth_cv.fill(1.0);
        let rate_cv = SignalBuffer::control(256);
        let mut outputs = vec![SignalBuffer::audio(256), SignalBuffer::audio(256)];
        for _ in 0..400 {
            chorus.process(
                &[&input, &input, &rate_cv, &depth_cv],
                &mut outputs,
                &[10.0, 1.0, 30.0, 0.5, 3.0, 1.0, 1.0],
                &ctx,
            );
            assert!(outputs.iter().all(|o| o.samples.iter().all(|s| s.is_finite())));
        }
    }

    #[test]
    fn test_chorus_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Chorus>();
    }

    #[test]
    fn test_chorus_default() {
        let chorus = Chorus::default();
        assert_eq!(chorus.info().id, "fx.chorus");
    }

    #[test]
    fn test_chorus_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Chorus>();

        assert!(registry.contains("fx.chorus"));

        let module = registry.create("fx.chorus");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "fx.chorus");
        assert_eq!(module.info().name, "Chorus");
        assert_eq!(module.ports().len(), 6);
        assert_eq!(module.parameters().len(), 7);
    }

    #[test]
    fn test_readout_shows_the_sweep_and_the_voices() {
        let mut chorus = Chorus::new();
        chorus.prepare(48000.0, 256);
        let ctx = ProcessContext::new(48000.0, 256);
        let input = SignalBuffer::audio(256);
        let mut rate_cv = SignalBuffer::control(256);
        rate_cv.fill(1.0); // +50%
        let depth_cv = SignalBuffer::control(256);
        let mut outputs = vec![SignalBuffer::audio(256), SignalBuffer::audio(256)];
        // Rate 2 Hz, depth 0.4, 10 ms, no feedback, three voices, half mix, triangle
        let params = [2.0, 0.4, 10.0, 0.0, 2.0, 0.5, 1.0];
        for _ in 0..200 {
            chorus.process(&[&input, &input, &rate_cv, &depth_cv], &mut outputs, &params, &ctx);
        }
        let r = chorus.readout(&params).unwrap().values;
        assert!((r[Chorus::READOUT_RATE] - 3.0).abs() < 1e-3);
        assert!((r[Chorus::READOUT_DEPTH] - 0.4).abs() < 1e-3);
        assert!((r[Chorus::READOUT_SHAPE] - 1.0).abs() < 1e-3);
        assert!((0.0..1.0).contains(&r[Chorus::READOUT_PHASE]));
        for slot in 0..Chorus::SLOTS {
            let at = Chorus::READOUT_VOICES + slot * Chorus::READOUT_PER_VOICE;
            if slot < 3 {
                let (left, right) = voice_phases(slot, 3);
                assert!((r[at] - 1.0).abs() < 1e-3, "voice {slot} is on");
                assert!((r[at + 1] - left).abs() < 1e-3 && (r[at + 2] - right).abs() < 1e-3);
            } else {
                assert!(r[at] < 1e-3, "voice {slot} is off");
            }
        }
    }
}
