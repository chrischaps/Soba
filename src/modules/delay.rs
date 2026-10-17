//! Stereo Delay effect module.
//!
//! A stereo delay with feedback filtering, ping-pong and tempo sync, plus a
//! Tape mode that behaves like a worn tape echo: the read head wanders with
//! wow and flutter, the record head saturates, and every repeat comes back a
//! little darker than the one before.

use crate::dsp::{
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    context::ProcessContext,
    denormal::flush,
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{FracDelay, RecordHead, TapeTransport},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    connected_input, ParameterDisplay, SignalType,
};

/// Maximum delay time in seconds.
const MAX_DELAY_SECONDS: f32 = 2.0;

/// Line kept beyond the longest delay so wow can swing past it.
const WOW_HEADROOM_SECONDS: f32 = 0.005;

/// Glide on delay-time changes. Plain mode follows the knob quickly; Tape mode
/// glides like a motor changing speed, bending the pitch of the repeats.
const TIME_GLIDE_MS: f32 = 50.0;
const TAPE_TIME_GLIDE_MS: f32 = 250.0;

/// Loop gain at full Feedback in Tape mode. Past unity, so the top of the
/// knob runs away into the saturator the way a tape echo does.
const TAPE_MAX_FEEDBACK: f32 = 1.1;

/// Tape loss cutoff at a 250 ms delay. Longer delays mean slower tape and
/// darker repeats: the cutoff falls with the square root of the time.
const TAPE_LOSS_HZ: f32 = 7000.0;
const TAPE_LOSS_MIN_HZ: f32 = 2500.0;
const TAPE_LOSS_MAX_HZ: f32 = 12000.0;

/// Tape can't record DC; this removes what the lopsided record head adds.
const TAPE_DC_HZ: f32 = 10.0;

/// One channel of the tape path after the record head: a DC blocker, then
/// the high-frequency loss of the tape itself.
#[derive(Clone, Copy, Debug, Default)]
struct TapeChannel {
    dc_x: f32,
    dc_y: f32,
    loss: f32,
}

impl TapeChannel {
    #[inline]
    fn process(&mut self, x: f32, dc_pole: f32, loss_coeff: f32) -> f32 {
        self.dc_y = flush(x - self.dc_x + dc_pole * self.dc_y);
        self.dc_x = x;
        self.loss = flush(self.loss + loss_coeff * (self.dc_y - self.loss));
        self.loss
    }
}

/// Stereo delay effect with feedback, filtering, ping-pong and Tape mode.
///
/// # Ports
///
/// - **In L** (Audio, Input): Left channel input.
/// - **In R** (Audio, Input): Right channel input (normalled from L).
/// - **Time CV** (Control, Input): Modulates delay time.
/// - **Feedback CV** (Control, Input): Modulates feedback amount.
/// - **Out L** (Audio, Output): Processed left channel.
/// - **Out R** (Audio, Output): Processed right channel.
///
/// # Parameters
///
/// - **Time** (1-2000 ms): Delay time.
/// - **Feedback** (0-100%): Amount of output fed back to input.
/// - **Mix** (0-100%): Wet/dry balance.
/// - **High Cut** (100-20000 Hz): Lowpass filter in feedback path.
/// - **Low Cut** (20-2000 Hz): Highpass filter in feedback path.
/// - **Ping-Pong** (toggle): Alternates repeats between channels.
/// - **Sync** (choice): Tempo sync division, following the Clock's tempo.
/// - **Tape** (toggle): Wow and flutter, record-head saturation and tape
///   loss; Feedback can pass unity.
pub struct StereoDelay {
    /// Sample rate.
    sample_rate: f32,
    /// Left channel delay line.
    line_left: FracDelay,
    /// Right channel delay line.
    line_right: FracDelay,
    /// Smoothed delay time in milliseconds.
    time_smooth: SmoothedValue,
    /// Smoothed feedback amount.
    feedback_smooth: SmoothedValue,
    /// Smoothed wet/dry mix.
    mix_smooth: SmoothedValue,
    /// Smoothed high cut frequency.
    high_cut_smooth: SmoothedValue,
    /// Smoothed low cut frequency.
    low_cut_smooth: SmoothedValue,
    /// High cut filter state (left).
    high_cut_state_l: f32,
    /// High cut filter state (right).
    high_cut_state_r: f32,
    /// Low cut filter state (left).
    low_cut_state_l: f32,
    /// Low cut filter state (right).
    low_cut_state_r: f32,
    /// How far into Tape mode we are (0 plain, 1 tape), so the toggle crossfades.
    tape_smooth: SmoothedValue,
    /// Whether the time glide is currently set for Tape mode.
    tape_glide: bool,
    /// Wow and flutter.
    transport: TapeTransport,
    /// Record-head saturation.
    record_head: RecordHead,
    /// Tape path state, per channel.
    tape_left: TapeChannel,
    tape_right: TapeChannel,
    /// DC blocker pole for the tape path.
    dc_pole: f32,
    /// Where the last sample of the last block stood, for the display: the
    /// delay time after Sync and CV (ms), the gain each repeat passes the
    /// loop with, and how far the tape's wow had pulled the read head (ms).
    time_now_ms: f32,
    loop_gain_now: f32,
    wow_now_ms: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl StereoDelay {
    /// Creates a new stereo delay.
    pub fn new() -> Self {
        let sample_rate = 44100.0;

        Self {
            sample_rate,
            line_left: FracDelay::new(Self::line_length(sample_rate)),
            line_right: FracDelay::new(Self::line_length(sample_rate)),
            time_smooth: SmoothedValue::new(500.0, TIME_GLIDE_MS, sample_rate),
            feedback_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            mix_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            high_cut_smooth: SmoothedValue::with_default_smoothing(10000.0, sample_rate),
            low_cut_smooth: SmoothedValue::with_default_smoothing(20.0, sample_rate),
            high_cut_state_l: 0.0,
            high_cut_state_r: 0.0,
            low_cut_state_l: 0.0,
            low_cut_state_r: 0.0,
            tape_smooth: SmoothedValue::new(0.0, 15.0, sample_rate),
            tape_glide: false,
            transport: TapeTransport::new(sample_rate),
            record_head: RecordHead::new(),
            tape_left: TapeChannel::default(),
            tape_right: TapeChannel::default(),
            dc_pole: Self::dc_pole(sample_rate),
            time_now_ms: 500.0,
            loop_gain_now: 0.5,
            wow_now_ms: 0.0,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("in_l", "In L", SignalType::Audio, 0.0).describe("Left audio to echo"),
                PortDefinition::input_with_default("in_r", "In R", SignalType::Audio, 0.0).describe("Right audio to echo; copies left when unpatched"),
                PortDefinition::input_with_default("time_cv", "Time CV", SignalType::Control, 0.0).describe("CV that stretches the delay time by up to 50%"),
                PortDefinition::input_with_default("feedback_cv", "Feedback CV", SignalType::Control, 0.0).describe("CV that adds to the feedback amount"),
                // Output ports
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("Left echoes, mixed with the dry signal"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("Right echoes, mixed with the dry signal"),
            ],
            parameters: vec![
                ParameterDefinition::new(
                    "time",
                    "Time",
                    1.0,
                    2000.0,
                    500.0,
                    ParameterDisplay::Logarithmic { unit: "ms" },
                ).describe("Gap between echoes, 1 to 2000 ms"),
                ParameterDefinition::normalized("feedback", "Feedback", 0.5).describe("How much of each echo is fed back; higher gives more repeats"),
                ParameterDefinition::normalized("mix", "Mix", 0.5).describe("Blend from dry (0) to echoes only (1)"),
                ParameterDefinition::frequency("high_cut", "High Cut", 100.0, 20000.0, 10000.0).describe("Darkens each repeat by rolling off highs above this, in Hz"),
                ParameterDefinition::frequency("low_cut", "Low Cut", 20.0, 2000.0, 20.0).describe("Thins each repeat by rolling off lows below this, in Hz"),
                ParameterDefinition::toggle("ping_pong", "Ping-Pong", false).describe("Bounces repeats between left and right"),
                // Patches save the index, so new divisions go on the end
                ParameterDefinition::choice(
                    "sync",
                    "Sync",
                    &Self::SYNC_LABELS,
                    0,
                ).describe("Locks the delay time to a note length of the tempo; Off uses Time"),
                ParameterDefinition::toggle("tape", "Tape", false).describe("Adds tape-style wobble, saturation and dulling to repeats"),
            ],
        }
    }

    /// The Sync choices, as the dropdown names them.
    pub const SYNC_LABELS: [&'static str; 9] = ["Off", "1/4", "1/8", "1/8T", "1/16", "1/16T", "1/32", "1/4D", "1/8D"];

    /// Readout slots: the delay time after Sync and CV (ms), the gain a
    /// repeat keeps each time round the loop, how far into Tape mode it is
    /// (0 to 1), and the wow's pull on the read head (ms).
    pub const READOUT_TIME_MS: usize = 0;
    pub const READOUT_LOOP_GAIN: usize = 1;
    pub const READOUT_TAPE: usize = 2;
    pub const READOUT_WOW_MS: usize = 3;

    /// Port index constants.
    const PORT_IN_L: usize = 0;
    const PORT_IN_R: usize = 1;
    const PORT_TIME_CV: usize = 2;
    const PORT_FEEDBACK_CV: usize = 3;
    const PORT_OUT_L: usize = 0;
    const PORT_OUT_R: usize = 1;

    /// Parameter index constants.
    const PARAM_TIME: usize = 0;
    const PARAM_FEEDBACK: usize = 1;
    const PARAM_MIX: usize = 2;
    const PARAM_HIGH_CUT: usize = 3;
    const PARAM_LOW_CUT: usize = 4;
    const PARAM_PING_PONG: usize = 5;
    const PARAM_SYNC: usize = 6;
    const PARAM_TAPE: usize = 7;

    /// Delay-line length for a sample rate: the longest delay plus wow headroom.
    fn line_length(sample_rate: f32) -> usize {
        ((MAX_DELAY_SECONDS + WOW_HEADROOM_SECONDS) * sample_rate) as usize + 2
    }

    /// Pole of the tape path's DC blocker.
    fn dc_pole(sample_rate: f32) -> f32 {
        1.0 - std::f32::consts::TAU * TAPE_DC_HZ / sample_rate
    }

    /// Simple one-pole lowpass filter coefficient.
    #[inline]
    fn lowpass_coeff(cutoff: f32, sample_rate: f32) -> f32 {
        let cutoff_clamped = cutoff.clamp(20.0, sample_rate * 0.45);
        let tan = (std::f32::consts::PI * cutoff_clamped / sample_rate).tan();
        tan / (1.0 + tan)
    }

    /// Simple one-pole highpass filter coefficient.
    #[inline]
    fn highpass_coeff(cutoff: f32, sample_rate: f32) -> f32 {
        let cutoff_clamped = cutoff.clamp(20.0, sample_rate * 0.45);
        let tan = (std::f32::consts::PI * cutoff_clamped / sample_rate).tan();
        1.0 / (1.0 + tan)
    }

    /// Tape loss cutoff for a delay time: slower tape for longer delays.
    pub fn tape_loss_hz(time_ms: f32) -> f32 {
        (TAPE_LOSS_HZ * (250.0 / time_ms.max(1.0)).sqrt()).clamp(TAPE_LOSS_MIN_HZ, TAPE_LOSS_MAX_HZ)
    }

    /// Soft clip to prevent runaway feedback.
    #[inline]
    fn soft_clip(x: f32) -> f32 {
        x.tanh()
    }

    /// Get sync division multiplier in beats (quarter notes).
    fn sync_to_beats(sync_index: usize) -> Option<f32> {
        match sync_index {
            0 => None,           // Off
            1 => Some(1.0),      // 1/4 = 1 beat
            2 => Some(0.5),      // 1/8 = 0.5 beats
            3 => Some(1.0 / 3.0), // 1/8T = triplet
            4 => Some(0.25),     // 1/16 = 0.25 beats
            5 => Some(1.0 / 6.0), // 1/16T = triplet
            6 => Some(0.125),    // 1/32 = 0.125 beats
            7 => Some(1.5),      // 1/4D = dotted quarter
            8 => Some(0.75),     // 1/8D = dotted eighth
            _ => None,
        }
    }
}

impl Default for StereoDelay {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for StereoDelay {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "fx.delay",
            name: "Stereo Delay",
            category: ModuleCategory::Effect,
            description: "Stereo delay with feedback filtering, ping-pong, tempo sync and tape mode",
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
        if sample_rate != self.sample_rate {
            self.line_left.allocate(Self::line_length(sample_rate));
            self.line_right.allocate(Self::line_length(sample_rate));
        }
        self.sample_rate = sample_rate;

        // Update sample rate for smoothed values
        self.time_smooth.set_sample_rate(sample_rate);
        self.feedback_smooth.set_sample_rate(sample_rate);
        self.mix_smooth.set_sample_rate(sample_rate);
        self.high_cut_smooth.set_sample_rate(sample_rate);
        self.low_cut_smooth.set_sample_rate(sample_rate);
        self.tape_smooth.set_sample_rate(sample_rate);

        self.transport.set_sample_rate(sample_rate);
        self.dc_pole = Self::dc_pole(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Get parameter values
        let time_ms = params[Self::PARAM_TIME];
        let feedback = params[Self::PARAM_FEEDBACK];
        let mix = params[Self::PARAM_MIX];
        let high_cut = params[Self::PARAM_HIGH_CUT];
        let low_cut = params[Self::PARAM_LOW_CUT];
        let ping_pong = params[Self::PARAM_PING_PONG] > 0.5;
        let sync_index = params[Self::PARAM_SYNC] as usize;
        let tape = params[Self::PARAM_TAPE] > 0.5;

        // Calculate delay time (either from sync or direct)
        let base_time_ms = if let Some(beats) = Self::sync_to_beats(sync_index) {
            // Follow the patch tempo (set by a Clock), or 120 BPM without one
            let bpm = context.transport.tempo_bpm.unwrap_or(120.0);
            let ms_per_beat = 60000.0 / bpm;
            (beats * ms_per_beat).clamp(1.0, 2000.0)
        } else {
            time_ms
        };

        // Tape glides between times like a motor finding its new speed
        if tape != self.tape_glide {
            self.tape_glide = tape;
            // Tape coming on from rest starts its wobble from rest too, so the
            // read head eases in instead of jumping to wherever the reels were
            if tape && self.tape_smooth.current() == 0.0 {
                self.transport.reset();
            }
            self.time_smooth
                .set_time_constant(if tape { TAPE_TIME_GLIDE_MS } else { TIME_GLIDE_MS });
        }

        // Set smoothing targets
        self.time_smooth.set_target(base_time_ms);
        self.feedback_smooth.set_target(feedback);
        self.mix_smooth.set_target(mix);
        self.high_cut_smooth.set_target(high_cut);
        self.low_cut_smooth.set_target(low_cut);
        self.tape_smooth.set_target(if tape { 1.0 } else { 0.0 });

        // Tape loss follows the tape speed; it changes slowly, so once a block is enough
        let loss_hz = Self::tape_loss_hz(self.time_smooth.current());
        let loss_coeff = 1.0 - (-std::f32::consts::TAU * loss_hz / self.sample_rate).exp();
        self.transport.renormalize();

        // Get input buffers
        let in_left = inputs.get(Self::PORT_IN_L);
        // Right is normalled from left when nothing is plugged into it
        let in_right = connected_input(inputs, Self::PORT_IN_R);
        let time_cv = inputs.get(Self::PORT_TIME_CV);
        let feedback_cv = inputs.get(Self::PORT_FEEDBACK_CV);

        // Split outputs
        let (out_left_slice, out_right_slice) = outputs.split_at_mut(Self::PORT_OUT_R);
        let out_left = &mut out_left_slice[Self::PORT_OUT_L];
        let out_right = &mut out_right_slice[0];

        let max_delay_samples = MAX_DELAY_SECONDS * self.sample_rate;
        let (mut time_now_ms, mut loop_gain_now, mut wow_now) = (self.time_now_ms, self.loop_gain_now, 0.0);

        // Process each sample
        for i in 0..context.block_size {
            // Get smoothed values
            let time_ms_smoothed = self.time_smooth.next();
            let feedback_smoothed = self.feedback_smooth.next();
            let mix_smoothed = self.mix_smooth.next();
            let high_cut_smoothed = self.high_cut_smooth.next();
            let low_cut_smoothed = self.low_cut_smooth.next();
            let tape_amount = self.tape_smooth.next();

            // Apply time CV modulation (bipolar, +/- 50% range)
            let time_mod = time_cv
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);
            let modulated_time_ms = (time_ms_smoothed * (1.0 + time_mod * 0.5)).clamp(1.0, 2000.0);

            // Apply feedback CV modulation. Plain mode stops short of unity;
            // Tape mode goes past it and leans on the record head instead.
            let fb_mod = feedback_cv
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);
            let feedback_amount = feedback_smoothed + fb_mod * 0.5;
            let plain_feedback = feedback_amount.clamp(0.0, 0.95);
            let tape_feedback = feedback_amount.clamp(0.0, 1.0) * TAPE_MAX_FEEDBACK;

            // Convert time to samples, and let the tape transport wobble the read head
            let delay_samples = (modulated_time_ms * 0.001 * self.sample_rate)
                .clamp(1.0, max_delay_samples);
            let wow = self.transport.next_offset() * tape_amount;
            let read_at = delay_samples - 1.0 + wow;
            time_now_ms = modulated_time_ms;
            loop_gain_now = plain_feedback + tape_amount * (tape_feedback - plain_feedback);
            wow_now = wow;

            // Get dry input samples
            let dry_left = in_left
                .map(|buf| buf.at(i))
                .unwrap_or(0.0);

            // Right channel is normalled from left if not connected
            let dry_right = match in_right {
                Some(buf) => buf.at(i),
                None => dry_left,
            };

            // Read delayed samples (written delay_samples ago; the lines haven't been pushed yet)
            let wet_left = self.line_left.read(read_at);
            let wet_right = self.line_right.read(read_at);

            // Calculate filter coefficients
            let lp_coeff = Self::lowpass_coeff(high_cut_smoothed, self.sample_rate);
            let hp_coeff = Self::highpass_coeff(low_cut_smoothed, self.sample_rate);

            // Apply lowpass to feedback (high cut)
            self.high_cut_state_l = flush(self.high_cut_state_l + lp_coeff * (wet_left - self.high_cut_state_l));
            self.high_cut_state_r = flush(self.high_cut_state_r + lp_coeff * (wet_right - self.high_cut_state_r));

            let filtered_left = self.high_cut_state_l;
            let filtered_right = self.high_cut_state_r;

            // Apply highpass to feedback (low cut)
            let hp_filtered_left = hp_coeff * (filtered_left - self.low_cut_state_l);
            self.low_cut_state_l = flush(filtered_left - hp_filtered_left);

            let hp_filtered_right = hp_coeff * (filtered_right - self.low_cut_state_r);
            self.low_cut_state_r = flush(filtered_right - hp_filtered_right);

            // Ping-pong cross-feeds the channels; otherwise each feeds itself
            let (loop_left, loop_right) = if ping_pong {
                (hp_filtered_right, hp_filtered_left)
            } else {
                (hp_filtered_left, hp_filtered_right)
            };

            // Plain: the input plus soft-clipped feedback
            let plain_left = dry_left + Self::soft_clip(loop_left * plain_feedback);
            let plain_right = dry_right + Self::soft_clip(loop_right * plain_feedback);

            // Tape: input and feedback recorded together through the record
            // head, then the tape's loss, which every repeat passes once more
            let record = &self.record_head;
            let tape_left = self.tape_left.process(
                record.record(dry_left + loop_left * tape_feedback),
                self.dc_pole,
                loss_coeff,
            );
            let tape_right = self.tape_right.process(
                record.record(dry_right + loop_right * tape_feedback),
                self.dc_pole,
                loss_coeff,
            );

            // Write to the delay lines
            self.line_left.push(plain_left + tape_amount * (tape_left - plain_left));
            self.line_right.push(plain_right + tape_amount * (tape_right - plain_right));

            // Mix dry and wet signals
            let out_l = dry_left * (1.0 - mix_smoothed) + wet_left * mix_smoothed;
            let out_r = dry_right * (1.0 - mix_smoothed) + wet_right * mix_smoothed;

            // Write outputs
            out_left.samples[i] = out_l;
            out_right.samples[i] = out_r;
        }

        self.time_now_ms = time_now_ms;
        self.loop_gain_now = loop_gain_now;
        self.wow_now_ms = wow_now / self.sample_rate * 1000.0;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_TIME_MS] = self.time_now_ms;
        readout.values[Self::READOUT_LOOP_GAIN] = self.loop_gain_now;
        readout.values[Self::READOUT_TAPE] = self.tape_smooth.current();
        readout.values[Self::READOUT_WOW_MS] = self.wow_now_ms;
        Some(readout)
    }

    fn reset(&mut self) {
        // Clear delay lines
        self.line_left.clear();
        self.line_right.clear();

        // Clear filter states
        self.high_cut_state_l = 0.0;
        self.high_cut_state_r = 0.0;
        self.low_cut_state_l = 0.0;
        self.low_cut_state_r = 0.0;
        self.tape_left = TapeChannel::default();
        self.tape_right = TapeChannel::default();
        self.transport.reset();

        // Reset smoothed values
        self.time_smooth.reset(self.time_smooth.target());
        self.feedback_smooth.reset(self.feedback_smooth.target());
        self.mix_smooth.reset(self.mix_smooth.target());
        self.high_cut_smooth.reset(self.high_cut_smooth.target());
        self.low_cut_smooth.reset(self.low_cut_smooth.target());
        self.tape_smooth.reset(self.tape_smooth.target());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{analysis::Spectrum, context::TransportState};

    /// Parameters: time, feedback, mix, high cut, low cut, ping-pong, sync, tape.
    fn params(time_ms: f32, feedback: f32, mix: f32) -> [f32; 8] {
        [time_ms, feedback, mix, 20000.0, 20.0, 0.0, 0.0, 0.0]
    }

    /// Runs `input` (mono, normalled to both sides) through the delay in
    /// blocks, returning the left output.
    fn run(delay: &mut StereoDelay, input: &[f32], params: &[f32], ctx: &ProcessContext) -> Vec<f32> {
        let block = ctx.block_size;
        let right = SignalBuffer::unconnected(block, SignalType::Audio);
        let cv = SignalBuffer::control(block);
        let mut outputs = vec![SignalBuffer::audio(block), SignalBuffer::audio(block)];
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(block) {
            let mut left = SignalBuffer::audio(block);
            left.samples[..chunk.len()].copy_from_slice(chunk);
            delay.process(&[&left, &right, &cv, &cv], &mut outputs, params, ctx);
            out.extend_from_slice(&outputs[0].samples[..chunk.len()]);
        }
        out
    }

    #[test]
    fn test_delay_info() {
        let delay = StereoDelay::new();
        assert_eq!(delay.info().id, "fx.delay");
        assert_eq!(delay.info().name, "Stereo Delay");
        assert_eq!(delay.info().category, ModuleCategory::Effect);
    }

    #[test]
    fn test_delay_ports() {
        let delay = StereoDelay::new();
        let ports = delay.ports();

        assert_eq!(ports.len(), 6);

        // Input ports
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "in_l");
        assert_eq!(ports[0].signal_type, SignalType::Audio);

        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "in_r");
        assert_eq!(ports[1].signal_type, SignalType::Audio);

        assert!(ports[2].is_input());
        assert_eq!(ports[2].id, "time_cv");
        assert_eq!(ports[2].signal_type, SignalType::Control);

        assert!(ports[3].is_input());
        assert_eq!(ports[3].id, "feedback_cv");
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
    fn test_delay_parameters() {
        let delay = StereoDelay::new();
        let params = delay.parameters();

        assert_eq!(params.len(), 8);
        assert_eq!(params[0].id, "time");
        assert_eq!(params[1].id, "feedback");
        assert_eq!(params[2].id, "mix");
        assert_eq!(params[3].id, "high_cut");
        assert_eq!(params[4].id, "low_cut");
        assert_eq!(params[5].id, "ping_pong");
        assert_eq!(params[6].id, "sync");
        assert_eq!(params[7].id, "tape");
    }

    #[test]
    fn test_delay_produces_output() {
        let mut delay = StereoDelay::new();
        delay.prepare(44100.0, 256);

        // Create a constant input signal
        let mut input = SignalBuffer::audio(256);
        input.fill(0.5);

        let empty_cv = SignalBuffer::control(256);
        let mut outputs = vec![
            SignalBuffer::audio(256), // Out L
            SignalBuffer::audio(256), // Out R
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        // Process with 10ms delay, 0% feedback, 50% wet (dry/wet mix)
        delay.process(
            &[&input, &input, &empty_cv, &empty_cv],
            &mut outputs,
            &[10.0, 0.0, 0.5, 10000.0, 20.0, 0.0, 0.0, 0.0],
            &ctx,
        );

        // Output should have signal (at least 50% dry pass-through)
        let has_output = outputs[0].samples.iter().any(|&s| s.abs() > 0.1);
        assert!(has_output, "Expected output signal");
    }

    #[test]
    fn test_delay_feedback() {
        // An impulse comes back exactly one delay later, then again at half level
        let sr = 44100.0;
        let mut delay = StereoDelay::new();
        delay.prepare(sr, 441);
        let ctx = ProcessContext::new(sr, 441);

        let delay_samples = 22050; // 500 ms, the smoother's starting point
        let mut input = vec![0.0; delay_samples * 3];
        input[0] = 1.0;
        let out = run(&mut delay, &input, &params(500.0, 0.5, 1.0), &ctx);

        assert!((out[delay_samples] - 1.0).abs() < 1e-3, "first echo {}", out[delay_samples]);
        // The high cut (a one-pole near Nyquist) softens the click on its way round
        let second = out[2 * delay_samples];
        assert!(second > 0.35 && second < 0.5, "second echo {second}");
        let elsewhere = out
            .iter()
            .enumerate()
            .filter(|(i, _)| i % delay_samples > 4 && i % delay_samples < delay_samples - 4)
            .map(|(_, s)| s.abs())
            .fold(0.0, f32::max);
        assert!(elsewhere < 0.01, "stray output {elsewhere}");
    }

    #[test]
    fn test_delay_reset() {
        let mut delay = StereoDelay::new();
        delay.prepare(44100.0, 256);

        // Fill buffer with some signal
        let mut input = SignalBuffer::audio(256);
        input.fill(1.0);
        let empty_cv = SignalBuffer::control(256);
        let mut outputs = vec![
            SignalBuffer::audio(256),
            SignalBuffer::audio(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        delay.process(
            &[&input, &input, &empty_cv, &empty_cv],
            &mut outputs,
            &[100.0, 0.5, 0.5, 10000.0, 20.0, 0.0, 0.0, 1.0],
            &ctx,
        );

        // Reset
        delay.reset();

        // Process silence
        let silence = SignalBuffer::audio(256);
        let mut outputs2 = vec![
            SignalBuffer::audio(256),
            SignalBuffer::audio(256),
        ];

        delay.process(
            &[&silence, &silence, &empty_cv, &empty_cv],
            &mut outputs2,
            &[100.0, 0.5, 1.0, 10000.0, 20.0, 0.0, 0.0, 1.0],
            &ctx,
        );

        // Output should be silent (lines and tape path cleared)
        assert!(
            outputs2[0].samples.iter().all(|s| s.abs() < 1e-6),
            "Expected silence after reset"
        );
    }

    #[test]
    fn test_delay_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<StereoDelay>();
    }

    #[test]
    fn test_delay_default() {
        let delay = StereoDelay::default();
        assert_eq!(delay.info().id, "fx.delay");
    }

    #[test]
    fn test_sync_to_beats() {
        assert!(StereoDelay::sync_to_beats(0).is_none()); // Off
        assert_eq!(StereoDelay::sync_to_beats(1), Some(1.0)); // 1/4
        assert_eq!(StereoDelay::sync_to_beats(2), Some(0.5)); // 1/8
        assert_eq!(StereoDelay::sync_to_beats(4), Some(0.25)); // 1/16
        assert_eq!(StereoDelay::sync_to_beats(6), Some(0.125)); // 1/32
        assert_eq!(StereoDelay::sync_to_beats(7), Some(1.5)); // 1/4D
        assert_eq!(StereoDelay::sync_to_beats(8), Some(0.75)); // 1/8D

        // Every division in the dropdown has a length
        let delay = StereoDelay::new();
        let ParameterDisplay::Discrete { labels: choices } = delay.parameters()[StereoDelay::PARAM_SYNC].display else {
            panic!("Sync should be a choice");
        };
        for i in 1..choices.len() {
            assert!(StereoDelay::sync_to_beats(i).is_some(), "{} has no length", choices[i]);
        }
    }

    #[test]
    fn test_every_sync_division_lands_on_the_tempo() {
        // At 93 BPM an impulse's echo arrives beats × 60/93 s later, to the sample
        let sr = 48000.0;
        let bpm = 93.0;
        for sync in 1..=8 {
            let beats = StereoDelay::sync_to_beats(sync).unwrap();
            let expected = beats * 60.0 / bpm * sr;
            let ctx = ProcessContext::with_transport(sr, 480, TransportState::playing_at(bpm));
            let mut p = params(500.0, 0.0, 1.0);
            p[StereoDelay::PARAM_SYNC] = sync as f32;

            // Let the time glide from its 500 ms start onto the synced time
            let mut delay = StereoDelay::new();
            delay.prepare(sr, 480);
            run(&mut delay, &vec![0.0; 48000], &p, &ctx);

            let mut input = vec![0.0; 120000];
            input[0] = 1.0;
            let out = run(&mut delay, &input, &p, &ctx);
            let arrival = (0..out.len()).max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs())).unwrap();
            assert!(
                (arrival as f32 - expected).abs() <= 1.0,
                "sync {sync}: echo at {arrival}, expected {expected}"
            );
        }
    }

    #[test]
    fn test_tape_max_feedback_stays_bounded() {
        // Full feedback plus feedback CV past the top, a loud noise burst,
        // then a long wait: the loop runs away into the record head and holds
        let sr = 48000.0;
        for ping_pong in [0.0, 1.0] {
            let mut delay = StereoDelay::new();
            delay.prepare(sr, 512);
            let ctx = ProcessContext::new(sr, 512);
            let right = SignalBuffer::unconnected(512, SignalType::Audio);
            let mut time_cv = SignalBuffer::control(512);
            time_cv.fill(-0.8); // short repeats, many passes
            let mut fb_cv = SignalBuffer::control(512);
            fb_cv.fill(1.0);
            let mut outputs = vec![SignalBuffer::audio(512), SignalBuffer::audio(512)];
            let p = [100.0, 1.0, 1.0, 20000.0, 20.0, ping_pong, 0.0, 1.0];

            let mut seed = 1u32;
            let mut peak = 0.0f32;
            let mut late = Vec::new();
            for block in 0..(sr as usize * 20 / 512) {
                let mut left = SignalBuffer::audio(512);
                if block < 50 {
                    for s in left.samples.iter_mut() {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        *s = (seed as i32 as f32) / i32::MAX as f32;
                    }
                }
                delay.process(&[&left, &right, &time_cv, &fb_cv], &mut outputs, &p, &ctx);
                for out in &outputs {
                    assert!(out.samples.iter().all(|s| s.is_finite()));
                    peak = out.samples.iter().fold(peak, |m, s| m.max(s.abs()));
                }
                if block * 512 > sr as usize * 19 {
                    late.extend_from_slice(&outputs[0].samples);
                }
            }
            assert!(peak < 1.5, "ping-pong {ping_pong}: peak {peak}");
            // Past unity it sustains rather than dying away
            let late_rms = crate::dsp::analysis::rms(&late);
            assert!(late_rms > 0.05, "ping-pong {ping_pong}: tail died ({late_rms})");
        }
    }

    #[test]
    fn test_tape_wow_and_flutter_bend_the_pitch() {
        // A steady 1 kHz sine read through the tape wanders in pitch by a
        // fraction of a percent; through the plain delay it doesn't move
        let sr = 48000.0;
        let input: Vec<f32> = (0..sr as usize * 5)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr).sin() * 0.1)
            .collect();
        let deviation = |tape: f32| {
            let mut delay = StereoDelay::new();
            delay.prepare(sr, 480);
            let ctx = ProcessContext::new(sr, 480);
            // 500 ms is where the time smoother starts, so nothing glides
            let mut p = params(500.0, 0.0, 1.0);
            p[StereoDelay::PARAM_TAPE] = tape;
            let out = run(&mut delay, &input, &p, &ctx);

            // Instantaneous frequency from upward zero crossings, over 20 ms windows
            let crossings: Vec<f32> = out[sr as usize * 6 / 10..]
                .windows(2)
                .enumerate()
                .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
                .map(|(i, w)| i as f32 + w[0] / (w[0] - w[1]))
                .collect();
            let freqs: Vec<f32> = crossings
                .windows(21)
                .step_by(20)
                .map(|w| 20.0 * sr / (w[20] - w[0]))
                .collect();
            let (lo, hi) = freqs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &f| (lo.min(f), hi.max(f)));
            (hi - lo) / 1000.0
        };

        let plain = deviation(0.0);
        let tape = deviation(1.0);
        assert!(plain < 1e-4, "plain delay wobbles: {plain}");
        assert!(tape > 0.002 && tape < 0.012, "tape pitch swing {tape}");
    }

    #[test]
    fn test_tape_repeats_darken_progressively() {
        // An impulse recirculating through the tape: each echo has less top
        // end than the one before it
        let sr = 48000.0;
        let mut delay = StereoDelay::new();
        delay.prepare(sr, 480);
        let ctx = ProcessContext::new(sr, 480);
        let mut p = params(500.0, 0.8, 1.0);
        p[StereoDelay::PARAM_TAPE] = 1.0;
        run(&mut delay, &vec![0.0; 4800], &p, &ctx); // crossfade into tape

        let echo = 24000;
        let mut input = vec![0.0; echo * 5];
        input[0] = 0.5;
        let out = run(&mut delay, &input, &p, &ctx);

        let brightness = |n: usize| {
            let window = &out[n * echo - 1024..n * echo + 1024];
            let spectrum = Spectrum::of(window, sr);
            let energy = |lo: f64, hi: f64| -> f64 {
                spectrum
                    .magnitudes
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| (lo..hi).contains(&(*k as f64 * spectrum.bin_hz)))
                    .map(|(_, m)| m * m)
                    .sum()
            };
            energy(4000.0, 12000.0) / energy(100.0, 1000.0)
        };
        let ratios: Vec<f64> = (1..=4).map(brightness).collect();
        for pair in ratios.windows(2) {
            assert!(pair[1] < pair[0] * 0.8, "repeats not darkening: {ratios:?}");
        }
    }

    #[test]
    fn test_tape_toggle_does_not_click() {
        // Switching Tape on mid-note crossfades rather than jumping
        let sr = 48000.0;
        let mut delay = StereoDelay::new();
        delay.prepare(sr, 480);
        let ctx = ProcessContext::new(sr, 480);
        let input: Vec<f32> = (0..48000)
            .map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / sr).sin() * 0.3)
            .collect();
        let mut p = params(300.0, 0.4, 1.0);
        let mut out = run(&mut delay, &input, &p, &ctx);
        p[StereoDelay::PARAM_TAPE] = 1.0;
        out.extend(run(&mut delay, &input, &p, &ctx));

        // A 220 Hz sine at 0.3 (plus echoes) moves at most ~0.02 per sample
        let worst_step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst_step < 0.05, "step of {worst_step} at the toggle");
    }

    #[test]
    fn test_delay_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<StereoDelay>();

        assert!(registry.contains("fx.delay"));

        let module = registry.create("fx.delay");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "fx.delay");
        assert_eq!(module.info().name, "Stereo Delay");
        assert_eq!(module.ports().len(), 6);
        assert_eq!(module.parameters().len(), 8);
    }

    #[test]
    fn test_mono_input_normals_to_right() {
        // With nothing plugged into In R, a mono source on In L must
        // reach both outputs.
        let mut delay = StereoDelay::new();
        delay.prepare(44100.0, 4410);
        let ctx = ProcessContext::new(44100.0, 4410);

        let mut left = SignalBuffer::audio(4410);
        for i in 0..4410 {
            left.samples[i] = (i as f32 * 0.05).sin();
        }
        let right = SignalBuffer::unconnected(4410, SignalType::Audio);
        let mut outputs = vec![SignalBuffer::audio(4410), SignalBuffer::audio(4410)];
        delay.process(
            &[&left, &right],
            &mut outputs,
            &[10.0, 0.5, 0.5, 10000.0, 20.0, 0.0, 0.0, 1.0],
            &ctx,
        );

        assert!(outputs[1].samples.iter().any(|s| s.abs() > 0.1), "Out R is silent");
        assert_eq!(outputs[0].samples, outputs[1].samples, "Mono input should be centred");
    }

    #[test]
    fn test_connected_silent_right_stays_silent() {
        // A cable carrying silence into In R is a real (silent) right channel,
        // not a reason to copy the left one.
        let mut delay = StereoDelay::new();
        delay.prepare(44100.0, 4410);
        let ctx = ProcessContext::new(44100.0, 4410);

        let mut left = SignalBuffer::audio(4410);
        left.fill(0.5);
        let right = SignalBuffer::audio(4410); // connected, silent
        let mut outputs = vec![SignalBuffer::audio(4410), SignalBuffer::audio(4410)];
        delay.process(
            &[&left, &right],
            &mut outputs,
            &[10.0, 0.0, 0.0, 10000.0, 20.0, 0.0, 0.0, 0.0],
            &ctx,
        );

        assert!(outputs[1].samples.iter().all(|&s| s == 0.0), "Out R should be silent");
    }

    #[test]
    fn test_readout_shows_the_synced_time_and_the_loop_gain() {
        let sr = 48000.0;
        let ctx = ProcessContext::with_transport(sr, 480, TransportState::playing_at(100.0));
        let mut p = params(500.0, 0.6, 0.5);
        p[StereoDelay::PARAM_SYNC] = 8.0; // 1/8D: 0.75 beats, 450 ms at 100 BPM
        let mut delay = StereoDelay::new();
        delay.prepare(sr, 480);
        run(&mut delay, &vec![0.0; 48000], &p, &ctx);
        let readout = delay.readout(&p).unwrap();
        assert!((readout.values[StereoDelay::READOUT_TIME_MS] - 450.0).abs() < 0.5);
        assert!((readout.values[StereoDelay::READOUT_LOOP_GAIN] - 0.6).abs() < 1e-3);
        assert_eq!(readout.values[StereoDelay::READOUT_TAPE], 0.0);

        // Tape lets the loop pass unity at the top of the knob
        p[StereoDelay::PARAM_FEEDBACK] = 1.0;
        p[StereoDelay::PARAM_TAPE] = 1.0;
        run(&mut delay, &vec![0.0; 48000], &p, &ctx);
        let readout = delay.readout(&p).unwrap();
        assert!(readout.values[StereoDelay::READOUT_LOOP_GAIN] > 1.0);
        assert!(readout.values[StereoDelay::READOUT_TAPE] > 0.99);
    }
}
