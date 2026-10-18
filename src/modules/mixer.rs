//! Stereo Mixer module.
//!
//! Four channel strips, each with a level, a pan, a width, a send, a mute
//! and a solo, summed into a stereo pair under one master level. A
//! polyphonic cable into a channel can be fanned across the stereo field
//! with its Width, so a chord opens up instead of sitting dead centre.
//!
//! Each channel's Send feeds Send L/R after its fader and pan; the effect
//! comes back on Return L/R, which may close the loop through this same
//! mixer (it's then heard a block late). Send and Return take CV as Level
//! and Pan do, added to their knobs, so a score can ride the room: one part
//! stepping into the reverb alone, or the whole room opening for a verse.
//!
//! Mixers cascade, as console sidecars do, over one cable. Chain Out carries
//! the whole mix on a Bus cable, four strands: the stereo pair and the send
//! bus. Into the next Mixer's Chain In, the pair joins its mix after the
//! faders and the sends join its send bus, so a kit spread over several
//! mixers stays stereo and shares one reverb.
//!
//! For a plain mono sum, of CVs or of audio, there's the Mix module.

use std::f32::consts::FRAC_PI_4;

use crate::dsp::{
    db_to_gain,
    primitives::knee_clip,
    connected_input,
    context::ProcessContext,
    module_trait::{DspModule, MeterLevels, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{SignalBuffer, MAX_CHANNELS},
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// The number of channel strips.
pub const STRIPS: usize = 4;

/// The strands of a Bus cable, in order: the mix's left and right, then the
/// send bus's left and right.
pub const BUS_STRANDS: usize = 4;

/// Samples between updates of the pan gains. In between, each gain glides
/// in a straight line, so a pan swept by an LFO moves smoothly without a
/// sine and cosine per voice per sample.
const PAN_STEP: usize = 32;

/// Where a channel's voice sits, from -1 (left) to 1 (right): the channel's
/// pan, moved by the voice's place in the fan its Width opens.
pub fn voice_pan(pan: f32, width: f32, voice: usize, voices: usize) -> f32 {
    (pan + width * spread_offset(voice, voices)).clamp(-1.0, 1.0)
}

/// A voice's place in a full fan, from -1 to 1.
///
/// The places are evenly spaced, and the voices take them alternately left
/// and right from the outside in: the first voice hard left, the second hard
/// right, the third just inside the first. Note In hands notes out in
/// turn, and neighbouring voices sit on opposite sides, so even two notes
/// held on an eight-voice cable land either side of the pan.
pub fn spread_offset(voice: usize, voices: usize) -> f32 {
    if voices < 2 {
        return 0.0;
    }
    let place = if voice.is_multiple_of(2) { voice / 2 } else { voices - 1 - voice / 2 };
    -1.0 + 2.0 * place as f32 / (voices - 1) as f32
}

/// Equal-power gains (left, right) for a pan from -1 (left) to 1 (right).
///
/// The power of the two sides adds up to the same at every position, so a
/// sound swept across keeps its loudness. At centre each side is at -3 dB.
#[inline]
pub fn pan_gains(pan: f32) -> [f32; 2] {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
    let (sin, cos) = angle.sin_cos();
    [cos, sin]
}

/// Whether a channel is heard: not muted, and either nothing is soloed or
/// it is. Mute wins over solo.
pub fn heard(muted: bool, soloed: bool, any_solo: bool) -> bool {
    !muted && (soloed || !any_solo)
}

/// One channel strip's running state.
#[derive(Clone)]
struct Strip {
    /// The Level knob, smoothed per sample. Level CV adds on unsmoothed.
    level: SmoothedValue,
    /// The Pan knob, smoothed once per [`PAN_STEP`].
    pan: SmoothedValue,
    /// The Width knob, smoothed once per [`PAN_STEP`].
    width: SmoothedValue,
    /// 1 while heard, 0 while muted or soloed out, smoothed so neither clicks.
    heard: SmoothedValue,
    /// The Send knob, smoothed per sample: how much goes to the send bus.
    /// Send CV adds on unsmoothed.
    send: SmoothedValue,
    /// Each voice's (left, right) pan gains, as they stand now.
    gains: [[f32; 2]; MAX_CHANNELS],
    /// The voices the strip played in the last block, 0 when unpatched.
    voices: usize,
    /// The loudest sample since the meters were last read, after the fader.
    peak: f32,
}

impl Strip {
    fn new(sample_rate: f32) -> Self {
        Self {
            level: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            pan: SmoothedValue::with_default_smoothing(0.0, sample_rate / PAN_STEP as f32),
            width: SmoothedValue::with_default_smoothing(0.0, sample_rate / PAN_STEP as f32),
            heard: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            send: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            gains: [pan_gains(0.0); MAX_CHANNELS],
            voices: 0,
            peak: 0.0,
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.level.set_sample_rate(sample_rate);
        self.pan.set_sample_rate(sample_rate / PAN_STEP as f32);
        self.width.set_sample_rate(sample_rate / PAN_STEP as f32);
        self.heard.set_sample_rate(sample_rate);
        self.send.set_sample_rate(sample_rate);
    }

    /// Jumps every smoothed value to where it's heading.
    fn settle(&mut self) {
        for value in [&mut self.level, &mut self.pan, &mut self.width, &mut self.heard, &mut self.send] {
            value.reset(value.target());
        }
        self.voices = 0;
    }
}

/// The module's ID, as patches name it.
pub const ID: &str = "util.mixer";

/// A four-channel stereo mixer.
///
/// # Ports
///
/// **Inputs:**
/// - **Ch 1** to **Ch 4** (Audio): The channels. A polyphonic cable keeps
///   its voices apart, so Width can place them.
/// - **Level 1** to **Level 4** (Control): CV added to each channel's level.
/// - **Pan 1** to **Pan 4** (Control): CV added to each channel's pan.
/// - **Send 1** to **Send 4** (Control): CV added to each channel's send.
/// - **Return** (Control): CV added to the Return knob.
/// - **Chain In** (Bus): Another Mixer's Chain Out. Its mix joins this one
///   after the faders, and its sends join this one's send bus.
/// - **Return L**, **Return R** (Audio, late): An effect's output, added at
///   the Return knob. A cable here may close a loop through this mixer.
///   R copies L when only L is patched, so a mono source comes in centred.
///
/// **Outputs:**
/// - **Out L**, **Out R** (Audio): The stereo mix.
/// - **Send L**, **Send R** (Audio): The send bus, for one effect to serve
///   every channel.
/// - **Chain Out** (Bus): Out L/R and Send L/R on one cable, for the next
///   Mixer's Chain In.
///
/// # Parameters
///
/// - **Level 1** to **Level 4** (0 to 1): Each channel's volume. Default 1.
/// - **Pan 1** to **Pan 4** (-1 to 1): Each channel's place, equal power.
/// - **Width 1** to **Width 4** (0 to 1): How far a polyphonic channel's
///   voices fan out around its pan. Default 0, where they all sit at it.
/// - **Send 1** to **Send 4** (0 to 1): How much of each channel, after its
///   fader and pan, goes to Send L/R. Default 0.
/// - **Mute 1** to **Mute 4**: Silences a channel.
/// - **Solo 1** to **Solo 4**: Hears only the soloed channels.
/// - **Return** (0 to 1): Level of Return L/R in the mix. Default 1.
/// - **Master** (-60 to +6 dB): Volume of the mix. Default 0 dB.
pub struct Mixer {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    strips: [Strip; STRIPS],
    /// 1 while Chain In is heard, 0 while a channel here is soloed.
    chain_heard: SmoothedValue,
    /// The Return knob, smoothed per sample. Return CV adds on unsmoothed.
    return_level: SmoothedValue,
    /// The Master knob, smoothed per sample.
    master: SmoothedValue,
    /// The loudest samples of Out L and Out R since the meters were last read.
    master_peaks: [f32; 2],
    /// The loudest sample the return and the chain brought in.
    return_peak: f32,
    chain_peak: f32,
}

impl Mixer {
    /// Creates a new Mixer.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let channel = |id, name, description| PortDefinition::input_with_default(id, name, SignalType::Audio, 0.0).describe(description);
        let cv = |id, name, description| PortDefinition::input_with_default(id, name, SignalType::Control, 0.0).describe(description);
        let unit = |id, name, default, description| {
            ParameterDefinition::new(id, name, 0.0, 1.0, default, ParameterDisplay::linear("")).describe(description)
        };
        let pan = |id, name, description| {
            ParameterDefinition::new(id, name, -1.0, 1.0, 0.0, ParameterDisplay::linear("pan")).describe(description)
        };
        let toggle = |id, name, description| ParameterDefinition::toggle(id, name, false).describe(description);
        Self {
            ports: vec![
                channel("ch1", "Ch 1", "Channel 1. A polyphonic cable keeps its voices apart, for Width"),
                channel("ch2", "Ch 2", "Channel 2. A polyphonic cable keeps its voices apart, for Width"),
                channel("ch3", "Ch 3", "Channel 3. A polyphonic cable keeps its voices apart, for Width"),
                channel("ch4", "Ch 4", "Channel 4. A polyphonic cable keeps its voices apart, for Width"),
                cv("level1_cv", "Level 1", "CV added to channel 1's Level knob"),
                cv("level2_cv", "Level 2", "CV added to channel 2's Level knob"),
                cv("level3_cv", "Level 3", "CV added to channel 3's Level knob"),
                cv("level4_cv", "Level 4", "CV added to channel 4's Level knob"),
                cv("pan1_cv", "Pan 1", "CV added to channel 1's Pan knob. An LFO here pans it to and fro"),
                cv("pan2_cv", "Pan 2", "CV added to channel 2's Pan knob. An LFO here pans it to and fro"),
                cv("pan3_cv", "Pan 3", "CV added to channel 3's Pan knob. An LFO here pans it to and fro"),
                cv("pan4_cv", "Pan 4", "CV added to channel 4's Pan knob. An LFO here pans it to and fro"),
                cv("send1_cv", "Send 1", "CV added to channel 1's Send knob, so a score can step it into the room alone"),
                cv("send2_cv", "Send 2", "CV added to channel 2's Send knob, so a score can step it into the room alone"),
                cv("send3_cv", "Send 3", "CV added to channel 3's Send knob, so a score can step it into the room alone"),
                cv("send4_cv", "Send 4", "CV added to channel 4's Send knob, so a score can step it into the room alone"),
                cv("return_cv", "Return", "CV added to the Return knob: an Arranger lane here is the whole room's fader"),
                PortDefinition::input_with_default("chain_in", "Chain In", SignalType::Bus, 0.0)
                    .describe("Another Mixer's Chain Out: its mix joins this one after the faders, and its sends join this one's send bus"),
                PortDefinition::input_with_default("return_l", "Return L", SignalType::Audio, 0.0).late().describe(
                    "Left of an effect fed from Send L/R, mixed in at the Return knob. It may come from this same mixer: the loop is heard a block late",
                ),
                PortDefinition::input_with_default("return_r", "Return R", SignalType::Audio, 0.0)
                    .late()
                    .describe("Right of the effect return; copies Return L when unpatched, so a mono source comes in centred"),
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("Left side of the stereo mix"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("Right side of the stereo mix"),
                PortDefinition::output("send_l", "Send L", SignalType::Audio)
                    .describe("Left of the send bus: each channel at its Send knob, after its fader and pan. Patch it to a reverb"),
                PortDefinition::output("send_r", "Send R", SignalType::Audio).describe("Right of the send bus"),
                PortDefinition::output("chain_out", "Chain Out", SignalType::Bus)
                    .describe("The whole mix and its sends on one cable, for the next Mixer's Chain In"),
            ],
            parameters: vec![
                unit("level1", "Level 1", 1.0, "Volume of channel 1"),
                unit("level2", "Level 2", 1.0, "Volume of channel 2"),
                unit("level3", "Level 3", 1.0, "Volume of channel 3"),
                unit("level4", "Level 4", 1.0, "Volume of channel 4"),
                pan("pan1", "Pan 1", "Where channel 1 sits, from hard left to hard right. Each side is at -3 dB in the centre"),
                pan("pan2", "Pan 2", "Where channel 2 sits, from hard left to hard right. Each side is at -3 dB in the centre"),
                pan("pan3", "Pan 3", "Where channel 3 sits, from hard left to hard right. Each side is at -3 dB in the centre"),
                pan("pan4", "Pan 4", "Where channel 4 sits, from hard left to hard right. Each side is at -3 dB in the centre"),
                // The knobs that take CV come first, in the order of their jacks
                unit("send1", "Send 1", 0.0, "How much of channel 1 goes to Send L/R, after its fader and pan"),
                unit("send2", "Send 2", 0.0, "How much of channel 2 goes to Send L/R, after its fader and pan"),
                unit("send3", "Send 3", 0.0, "How much of channel 3 goes to Send L/R, after its fader and pan"),
                unit("send4", "Send 4", 0.0, "How much of channel 4 goes to Send L/R, after its fader and pan"),
                unit("return", "Return", 1.0, "Level of Return L/R in the mix, before the Master"),
                unit("width1", "Width 1", 0.0, "Fans a polyphonic channel 1's voices out across the field, around its pan"),
                unit("width2", "Width 2", 0.0, "Fans a polyphonic channel 2's voices out across the field, around its pan"),
                unit("width3", "Width 3", 0.0, "Fans a polyphonic channel 3's voices out across the field, around its pan"),
                unit("width4", "Width 4", 0.0, "Fans a polyphonic channel 4's voices out across the field, around its pan"),
                toggle("mute1", "Mute 1", "Silences channel 1"),
                toggle("mute2", "Mute 2", "Silences channel 2"),
                toggle("mute3", "Mute 3", "Silences channel 3"),
                toggle("mute4", "Mute 4", "Silences channel 4"),
                toggle("solo1", "Solo 1", "Hears channel 1 alone, with any other soloed channels and the return"),
                toggle("solo2", "Solo 2", "Hears channel 2 alone, with any other soloed channels and the return"),
                toggle("solo3", "Solo 3", "Hears channel 3 alone, with any other soloed channels and the return"),
                toggle("solo4", "Solo 4", "Hears channel 4 alone, with any other soloed channels and the return"),
                ParameterDefinition::new("master", "Master", MASTER_FLOOR_DB, 6.0, 0.0, ParameterDisplay::linear("dB"))
                    .describe("Volume of the mix. Up to +6 dB, to win back the 3 dB a centred channel gives up on each side"),
            ],
            strips: std::array::from_fn(|_| Strip::new(sample_rate)),
            chain_heard: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            return_level: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            master: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            master_peaks: [0.0; 2],
            return_peak: 0.0,
            chain_peak: 0.0,
        }
    }

    /// Input port indices: the channels, the Level, Pan and Send CVs, the
    /// Return CV, then the chain and the return.
    pub const PORT_CH: usize = 0;
    pub const PORT_LEVEL_CV: usize = 4;
    pub const PORT_PAN_CV: usize = 8;
    pub const PORT_SEND_CV: usize = 12;
    pub const PORT_RETURN_CV: usize = 16;
    pub const PORT_CHAIN_IN: usize = 17;
    pub const PORT_RETURN: usize = 18;
    /// The number of inputs; the outputs follow them.
    pub const INPUTS: usize = 20;

    /// Output indices, counted among the outputs.
    pub const OUT_L: usize = 0;
    pub const OUT_R: usize = 1;
    pub const SEND_L: usize = 2;
    pub const SEND_R: usize = 3;
    pub const CHAIN_OUT: usize = 4;

    /// Parameter indices: those that take CV, in the order of their jacks
    /// (four of each per-channel kind, then the Return), then the rest.
    pub const PARAM_LEVEL: usize = 0;
    pub const PARAM_PAN: usize = 4;
    pub const PARAM_SEND: usize = 8;
    pub const PARAM_RETURN: usize = 12;
    pub const PARAM_WIDTH: usize = 13;
    pub const PARAM_MUTE: usize = 17;
    pub const PARAM_SOLO: usize = 21;
    pub const PARAM_MASTER: usize = 25;
    pub const PARAMS: usize = 26;

    /// Meter indices: one per strip, then these.
    pub const METER_OUT_L: usize = STRIPS;
    pub const METER_OUT_R: usize = STRIPS + 1;
    pub const METER_RETURN: usize = STRIPS + 2;
    pub const METER_CHAIN: usize = STRIPS + 3;
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Mixer {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Mixer",
            category: ModuleCategory::Utility,
            description: "4-channel stereo console: pan, width, sends, mute and solo, chained over one cable",
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
        for strip in &mut self.strips {
            strip.set_sample_rate(sample_rate);
        }
        self.chain_heard.set_sample_rate(sample_rate);
        self.return_level.set_sample_rate(sample_rate);
        self.master.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [out_l, out_r, send_l, send_r, chain_out] = outputs else {
            return;
        };
        let n = [&*out_l, &*out_r, &*send_l, &*send_r, &*chain_out]
            .iter()
            .fold(context.block_size, |n, buf| n.min(buf.samples.len()));
        let (out_l, out_r) = (&mut out_l.samples[..n], &mut out_r.samples[..n]);
        let (send_l, send_r) = (&mut send_l.samples[..n], &mut send_r.samples[..n]);
        for bus in [&mut *out_l, &mut *out_r, &mut *send_l, &mut *send_r] {
            bus.fill(0.0);
        }

        let on = |index: usize| params[index] >= 0.5;
        let any_solo = (0..STRIPS).any(|s| on(Self::PARAM_SOLO + s));
        self.chain_heard.set_target(if any_solo { 0.0 } else { 1.0 });
        self.return_level.set_target(params[Self::PARAM_RETURN]);
        self.master.set_target(master_gain(params[Self::PARAM_MASTER]));

        for (s, strip) in self.strips.iter_mut().enumerate() {
            strip.level.set_target(params[Self::PARAM_LEVEL + s]);
            strip.pan.set_target(params[Self::PARAM_PAN + s]);
            strip.width.set_target(params[Self::PARAM_WIDTH + s]);
            strip.send.set_target(params[Self::PARAM_SEND + s]);
            let is_heard = heard(on(Self::PARAM_MUTE + s), on(Self::PARAM_SOLO + s), any_solo);
            strip.heard.set_target(if is_heard { 1.0 } else { 0.0 });

            let Some(input) = connected_input(inputs, Self::PORT_CH + s).filter(|buf| buf.samples.len() >= n) else {
                strip.settle();
                continue;
            };
            let cv = |port: usize| connected_input(inputs, port).map(|buf| &buf.samples[..]).filter(|cv| cv.len() >= n);
            let level_cv = cv(Self::PORT_LEVEL_CV + s);
            let pan_cv = cv(Self::PORT_PAN_CV + s);
            let send_cv = cv(Self::PORT_SEND_CV + s);

            let voices = input.channels().min(MAX_CHANNELS);
            let mut channels: [&[f32]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
            for (v, channel) in channels[..voices].iter_mut().enumerate() {
                *channel = &input.voice(v).samples[..n];
            }
            let channels = &channels[..voices];

            let mut start = 0;
            while start < n {
                let end = (start + PAN_STEP).min(n);
                let len = (end - start) as f32;

                // Where each voice is heading by the end of this stretch.
                // A voice that has just joined starts there, as it was silent
                let pan = strip.pan.next() + pan_cv.map_or(0.0, |cv| cv[end - 1]);
                let width = strip.width.next();
                let mut targets = [[0.0; 2]; MAX_CHANNELS];
                let mut steps = [[0.0; 2]; MAX_CHANNELS];
                for v in 0..voices {
                    let target = pan_gains(voice_pan(pan, width, v, voices));
                    if v >= strip.voices {
                        strip.gains[v] = target;
                    }
                    targets[v] = target;
                    steps[v] = [(target[0] - strip.gains[v][0]) / len, (target[1] - strip.gains[v][1]) / len];
                }
                strip.voices = voices;

                for i in start..end {
                    let level = (strip.level.next() + level_cv.map_or(0.0, |cv| cv[i])).clamp(0.0, 1.0);
                    let gain = level * strip.heard.next();
                    let (mut mono, mut left, mut right) = (0.0, 0.0, 0.0);
                    for (v, channel) in channels.iter().enumerate() {
                        let x = channel[i];
                        let g = &mut strip.gains[v];
                        g[0] += steps[v][0];
                        g[1] += steps[v][1];
                        mono += x;
                        left += x * g[0];
                        right += x * g[1];
                    }
                    let (left, right) = (left * gain, right * gain);
                    out_l[i] += left;
                    out_r[i] += right;
                    let send = (strip.send.next() + send_cv.map_or(0.0, |cv| cv[i])).clamp(0.0, 1.0);
                    send_l[i] += left * send;
                    send_r[i] += right * send;
                    strip.peak = strip.peak.max((mono * gain).abs());
                }

                // Land exactly, so rounding never builds up
                strip.gains[..voices].copy_from_slice(&targets[..voices]);
                start = end;
            }
        }

        // Another mixer's whole mix and its sends join after the faders.
        // Solo here silences them, as it does the channels
        match connected_input(inputs, Self::PORT_CHAIN_IN).filter(|buf| buf.samples.len() >= n) {
            Some(chain) => {
                let strand = |k: usize| (k < chain.channels()).then(|| &chain.voice(k).samples[..n]);
                let strands = [strand(0), strand(1), strand(2), strand(3)];
                for i in 0..n {
                    let heard = self.chain_heard.next();
                    let at = |k: usize| strands[k].map_or(0.0, |s| s[i] * heard);
                    let (l, r) = (at(0), at(1));
                    out_l[i] += l;
                    out_r[i] += r;
                    send_l[i] += at(2);
                    send_r[i] += at(3);
                    self.chain_peak = self.chain_peak.max(l.abs()).max(r.abs());
                }
            }
            None => self.chain_heard.reset(self.chain_heard.target()),
        }

        // The effect comes back at the Return knob, whatever is soloed
        if let Some([l, r]) = stereo_in(inputs, Self::PORT_RETURN, n) {
            let return_cv = connected_input(inputs, Self::PORT_RETURN_CV).map(|buf| &buf.samples[..]).filter(|cv| cv.len() >= n);
            for i in 0..n {
                let level = (self.return_level.next() + return_cv.map_or(0.0, |cv| cv[i])).clamp(0.0, 1.0);
                let (l, r) = (summed(l, i) * level, summed(r, i) * level);
                out_l[i] += l;
                out_r[i] += r;
                self.return_peak = self.return_peak.max(l.abs()).max(r.abs());
            }
        } else {
            self.return_level.reset(self.return_level.target());
        }

        // The send bus goes out at its own level, whatever the Master
        for sample in send_l.iter_mut().chain(send_r.iter_mut()) {
            *sample = soft_clip(*sample);
        }

        for i in 0..n {
            let master = self.master.next();
            out_l[i] = soft_clip(out_l[i] * master);
            out_r[i] = soft_clip(out_r[i] * master);
            self.master_peaks[0] = self.master_peaks[0].max(out_l[i].abs());
            self.master_peaks[1] = self.master_peaks[1].max(out_r[i].abs());
        }

        // The chain carries the same four signals on one cable
        chain_out.set_channels(BUS_STRANDS);
        for (k, strand) in [&*out_l, &*out_r, &*send_l, &*send_r].into_iter().enumerate() {
            chain_out.channel_mut(k)[..n].copy_from_slice(strand);
        }
    }

    fn reset(&mut self) {
        for strip in &mut self.strips {
            strip.settle();
            strip.peak = 0.0;
        }
        for value in [&mut self.chain_heard, &mut self.return_level, &mut self.master] {
            value.reset(value.target());
        }
        self.master_peaks = [0.0; 2];
        self.return_peak = 0.0;
        self.chain_peak = 0.0;
    }

    /// Each channel's peak after its fader (Ch 1 to Ch 4), then Out L and
    /// Out R, then what came in on the Return and on Chain In.
    fn take_meter_levels(&mut self) -> Option<MeterLevels> {
        let mut levels = MeterLevels::default();
        for (peak, strip) in levels.peaks.iter_mut().zip(&mut self.strips) {
            *peak = std::mem::take(&mut strip.peak);
        }
        levels.peaks[Self::METER_OUT_L] = std::mem::take(&mut self.master_peaks[0]);
        levels.peaks[Self::METER_OUT_R] = std::mem::take(&mut self.master_peaks[1]);
        levels.peaks[Self::METER_RETURN] = std::mem::take(&mut self.return_peak);
        levels.peaks[Self::METER_CHAIN] = std::mem::take(&mut self.chain_peak);
        Some(levels)
    }

    /// Runs as one module, but hears each voice of a polyphonic cable on its
    /// own, so Width can place them, and each strand of a Bus. Its stereo
    /// outputs carry one channel each, and Chain Out four.
    fn polyphonic(&self) -> bool {
        true
    }
}

/// A stereo pair of inputs at `port` and `port + 1`, or `None` when neither
/// is patched. The right copies the left when only the left is patched, so
/// one cable brings a mono source in centred.
fn stereo_in<'a>(inputs: &[&'a SignalBuffer], port: usize, n: usize) -> Option<[&'a SignalBuffer; 2]> {
    let left = *inputs.get(port)?;
    let right = connected_input(inputs, port + 1);
    if !left.is_connected() && right.is_none() {
        return None;
    }
    let right = right.unwrap_or(left);
    (left.samples.len() >= n && right.samples.len() >= n).then_some([left, right])
}

/// Sample `i` of a cable, its voices summed: a stereo side is one channel.
#[inline]
fn summed(buf: &SignalBuffer, i: usize) -> f32 {
    (0..buf.channels()).map(|c| buf.voice(c).samples[i]).sum()
}

/// The bottom of the Master knob, where it turns the mix off.
const MASTER_FLOOR_DB: f32 = -60.0;

/// The Master knob's gain: its decibels, or silence at the bottom.
fn master_gain(db: f32) -> f32 {
    if db <= MASTER_FLOOR_DB {
        0.0
    } else {
        db_to_gain(db)
    }
}

/// Where soft clipping starts: anything within ±1 passes untouched.
const CLIP_KNEE: f32 = 1.0;
/// The level the clipped sum eases toward but never reaches. Above 1 so two
/// envelopes summed for a wider filter sweep still open it further than one.
const CLIP_CEILING: f32 = 1.5;

/// Soft clipping to keep hot sums from clipping harshly: unity up to the
/// knee, easing toward the ceiling above it.
#[inline]
pub(crate) fn soft_clip(x: f32) -> f32 {
    knee_clip(x, CLIP_KNEE, CLIP_CEILING)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_1_SQRT_2;

    const BLOCK: usize = 256;

    /// Default parameters: every level at 1 and the master at 0 dB,
    /// centred, unmuted, no width, no sends, the return at full.
    fn params() -> [f32; Mixer::PARAMS] {
        let mut p = [0.0; Mixer::PARAMS];
        p[..STRIPS].fill(1.0);
        p[Mixer::PARAM_RETURN] = 1.0;
        p
    }

    /// Out L, Out R, Send L, Send R, Chain Out (with room for its strands).
    fn outputs() -> Vec<SignalBuffer> {
        let mut out: Vec<_> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        out.push(SignalBuffer::polyphonic(BLOCK, SignalType::Bus));
        out
    }

    /// An unpatched input, as the engine passes one.
    fn unpatched() -> SignalBuffer {
        SignalBuffer::unconnected(BLOCK, SignalType::Audio)
    }

    /// Runs `blocks` blocks with `inputs` laid on the ports by index (the
    /// rest unpatched), returning the outputs of the last.
    fn run(mixer: &mut Mixer, patched: &[(usize, &SignalBuffer)], params: &[f32], blocks: usize) -> Vec<SignalBuffer> {
        let empty = unpatched();
        let mut inputs: Vec<&SignalBuffer> = vec![&empty; Mixer::INPUTS];
        for &(port, buf) in patched {
            inputs[port] = buf;
        }
        let mut out = outputs();
        let ctx = ProcessContext::new(44100.0, BLOCK);
        for _ in 0..blocks {
            mixer.process(&inputs, &mut out, params, &ctx);
        }
        out
    }

    fn prepared() -> Mixer {
        let mut mixer = Mixer::new();
        mixer.prepare(44100.0, BLOCK);
        mixer
    }

    fn constant(value: f32) -> SignalBuffer {
        let mut buf = SignalBuffer::audio(BLOCK);
        buf.fill(value);
        buf
    }

    /// A polyphonic buffer with each channel at its own constant.
    fn poly(values: &[f32]) -> SignalBuffer {
        let mut buf = SignalBuffer::polyphonic(BLOCK, SignalType::Audio);
        buf.set_channels(values.len());
        for (c, &v) in values.iter().enumerate() {
            buf.channel_mut(c).fill(v);
        }
        buf
    }

    /// A Bus cable with each strand at its own constant.
    fn bus(strands: [f32; BUS_STRANDS]) -> SignalBuffer {
        let mut buf = SignalBuffer::polyphonic(BLOCK, SignalType::Bus);
        buf.set_channels(BUS_STRANDS);
        for (k, &v) in strands.iter().enumerate() {
            buf.channel_mut(k).fill(v);
        }
        buf
    }

    fn last(buf: &SignalBuffer) -> f32 {
        buf.samples[BLOCK - 1]
    }

    /// Out L and Out R at the end of the block.
    fn lr(out: &[SignalBuffer]) -> [f32; 2] {
        [last(&out[Mixer::OUT_L]), last(&out[Mixer::OUT_R])]
    }

    /// Send L and Send R at the end of the block.
    fn sends(out: &[SignalBuffer]) -> [f32; 2] {
        [last(&out[Mixer::SEND_L]), last(&out[Mixer::SEND_R])]
    }

    /// Centred, each side is at -3 dB.
    const CENTRE: f32 = FRAC_1_SQRT_2;

    #[test]
    fn test_mixer_info() {
        let mixer = Mixer::new();
        assert_eq!(mixer.info().id, "util.mixer");
        assert_eq!(mixer.info().name, "Mixer");
        assert_eq!(mixer.info().category, ModuleCategory::Utility);
        assert!(mixer.polyphonic(), "hears poly voices and bus strands apart");
    }

    #[test]
    fn test_ports_and_parameters_line_up_with_their_indices() {
        let mixer = Mixer::new();
        let inputs: Vec<_> = mixer.ports().iter().filter(|p| p.is_input()).collect();
        let outputs: Vec<_> = mixer.ports().iter().filter(|p| p.is_output()).collect();
        assert_eq!(inputs.len(), Mixer::INPUTS);
        assert_eq!(inputs[Mixer::PORT_CH + 2].name, "Ch 3");
        assert_eq!(inputs[Mixer::PORT_LEVEL_CV + 1].name, "Level 2");
        assert_eq!(inputs[Mixer::PORT_PAN_CV + 3].name, "Pan 4");
        assert_eq!(inputs[Mixer::PORT_SEND_CV + 2].name, "Send 3");
        assert_eq!(inputs[Mixer::PORT_RETURN_CV].name, "Return");
        assert_eq!((inputs[Mixer::PORT_CHAIN_IN].name, inputs[Mixer::PORT_CHAIN_IN].signal_type), ("Chain In", SignalType::Bus));
        assert_eq!(inputs[Mixer::PORT_RETURN].name, "Return L");
        assert!(inputs.iter().all(|p| p.late == p.name.starts_with("Return ")), "only the return closes loops");
        assert_eq!(outputs.iter().map(|p| p.name).collect::<Vec<_>>(), ["Out L", "Out R", "Send L", "Send R", "Chain Out"]);
        assert_eq!(outputs[Mixer::CHAIN_OUT].signal_type, SignalType::Bus);

        let definitions = mixer.parameters();
        assert_eq!(definitions.len(), Mixer::PARAMS);
        for (index, name) in [
            (Mixer::PARAM_LEVEL, "Level 1"),
            (Mixer::PARAM_PAN + 1, "Pan 2"),
            (Mixer::PARAM_WIDTH + 2, "Width 3"),
            (Mixer::PARAM_SEND + 3, "Send 4"),
            (Mixer::PARAM_MUTE, "Mute 1"),
            (Mixer::PARAM_SOLO + 1, "Solo 2"),
            (Mixer::PARAM_RETURN, "Return"),
            (Mixer::PARAM_MASTER, "Master"),
        ] {
            assert_eq!(definitions[index].name, name);
        }
        // Defaults change nothing until turned: full level, centred, no
        // width, no send, the return at full, unity master
        let defaults: Vec<f32> = definitions.iter().map(|p| p.default).collect();
        assert_eq!(defaults, params().to_vec());
    }

    #[test]
    fn test_pan_law() {
        let mut mixer = prepared();
        let x = constant(0.5);

        // Centre: each side at -3 dB
        let out = run(&mut mixer, &[(0, &x)], &params(), 4);
        let centre_db = 20.0 * (lr(&out)[0] / 0.5).log10();
        assert!((centre_db + 3.01).abs() < 0.02, "centre is {centre_db} dB per side");
        assert!((lr(&out)[0] - lr(&out)[1]).abs() < 1e-6);

        // Hard left: silence on the right
        let mut p = params();
        p[Mixer::PARAM_PAN] = -1.0;
        let out = run(&mut mixer, &[(0, &x)], &p, 40);
        assert!((lr(&out)[0] - 0.5).abs() < 1e-4);
        assert!(lr(&out)[1].abs() < 1e-6, "right should be silent, got {}", lr(&out)[1]);

        // Power is the same wherever it sits
        for pan in [-0.7, -0.2, 0.3, 0.9] {
            let [l, r] = pan_gains(pan);
            assert!((l * l + r * r - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn test_pan_cv_moves_the_channel() {
        let mut mixer = prepared();
        let (x, cv) = (constant(0.5), constant(1.0));
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_PAN_CV, &cv)], &params(), 4);
        assert!(lr(&out)[0].abs() < 1e-6, "CV of 1 pans a centred channel hard right");
        assert!((lr(&out)[1] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn test_level_cv_adds_to_the_knob() {
        let mut mixer = prepared();
        let x = constant(0.5);
        let mut p = params();
        p[Mixer::PARAM_PAN] = -1.0;
        p[0] = 0.5;
        let cv = constant(0.25);
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_LEVEL_CV, &cv)], &p, 20);
        assert!((lr(&out)[0] - 0.5 * 0.75).abs() < 1e-4);

        // Never past full level, nor below silence
        let cv = constant(4.0);
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_LEVEL_CV, &cv)], &p, 2);
        assert!((lr(&out)[0] - 0.5).abs() < 1e-4);
        let cv = constant(-4.0);
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_LEVEL_CV, &cv)], &p, 2);
        assert_eq!(lr(&out)[0], 0.0);
    }

    #[test]
    fn test_mute_silences_without_a_click() {
        let mut mixer = prepared();
        let x = constant(0.8);
        run(&mut mixer, &[(0, &x)], &params(), 4);

        let mut p = params();
        p[Mixer::PARAM_MUTE] = 1.0;
        let out = run(&mut mixer, &[(0, &x)], &p, 1);
        // Fades over the first block rather than dropping at once
        assert!(out[Mixer::OUT_L].samples[0] > 0.5);
        let out = run(&mut mixer, &[(0, &x)], &p, 20);
        assert!(lr(&out).iter().all(|s| s.abs() < 1e-4));
    }

    #[test]
    fn test_solo_hears_only_the_soloed() {
        let mut mixer = prepared();
        let (a, b) = (constant(0.5), constant(0.3));
        let mut p = params();
        p[Mixer::PARAM_PAN] = -1.0;
        p[Mixer::PARAM_PAN + 1] = 1.0;
        run(&mut mixer, &[(0, &a), (1, &b)], &p, 4);

        // Soloing channel 2 fades channel 1 out, without a click
        p[Mixer::PARAM_SOLO + 1] = 1.0;
        let out = run(&mut mixer, &[(0, &a), (1, &b)], &p, 1);
        assert!(out[Mixer::OUT_L].samples[0] > 0.4, "channel 1 fades rather than drops");
        let out = run(&mut mixer, &[(0, &a), (1, &b)], &p, 20);
        assert!(lr(&out)[0].abs() < 1e-4, "channel 1 is soloed out");
        assert!((lr(&out)[1] - 0.3).abs() < 1e-4, "channel 2 plays on");

        // Two soloed: both heard. Mute wins over solo
        p[Mixer::PARAM_SOLO] = 1.0;
        let out = run(&mut mixer, &[(0, &a), (1, &b)], &p, 20);
        assert!((lr(&out)[0] - 0.5).abs() < 1e-4);
        p[Mixer::PARAM_MUTE] = 1.0;
        let out = run(&mut mixer, &[(0, &a), (1, &b)], &p, 20);
        assert!(lr(&out)[0].abs() < 1e-4);

        for (muted, soloed, any, expected) in [
            (false, false, false, true),
            (false, false, true, false),
            (false, true, true, true),
            (true, true, true, false),
            (true, false, false, false),
        ] {
            assert_eq!(heard(muted, soloed, any), expected, "muted {muted} soloed {soloed} any {any}");
        }
    }

    #[test]
    fn test_a_soloed_channel_keeps_its_room_and_the_return_plays() {
        let mut mixer = prepared();
        let (a, b, wet) = (constant(0.5), constant(0.3), constant(0.1));
        let mut p = params();
        p[Mixer::PARAM_SEND] = 1.0;
        p[Mixer::PARAM_SEND + 1] = 1.0;
        p[Mixer::PARAM_SOLO] = 1.0;
        let out = run(&mut mixer, &[(0, &a), (1, &b), (Mixer::PORT_RETURN, &wet)], &p, 20);
        assert!((sends(&out)[0] - 0.5 * CENTRE).abs() < 1e-4, "only the soloed channel sends");
        assert!((lr(&out)[0] - (0.5 * CENTRE + 0.1)).abs() < 1e-4, "the return is solo-safe");
    }

    #[test]
    fn test_a_patch_starts_where_it_was_saved() {
        // Loaded muted and panned hard right: not a sample leaks through on
        // the way there
        let mut mixer = prepared();
        let x = constant(0.8);
        let mut p = params();
        p[Mixer::PARAM_PAN] = 1.0;
        p[Mixer::PARAM_MUTE + 1] = 1.0;
        let out = run(&mut mixer, &[(0, &x), (1, &x)], &p, 1);
        assert!(out[Mixer::OUT_L].samples.iter().all(|s| s.abs() < 1e-6), "left should be silent");
        assert!((out[Mixer::OUT_R].samples[0] - 0.8).abs() < 1e-4, "channel 1 is right from the start");
        assert_eq!(mixer.take_meter_levels().unwrap().peaks[1], 0.0, "channel 2 never sounds");
    }

    #[test]
    fn test_master_scales_the_mix() {
        let mut mixer = prepared();
        let x = constant(0.5);
        let mut p = params();
        p[Mixer::PARAM_MASTER] = -6.0206;
        let out = run(&mut mixer, &[(0, &x)], &p, 20);
        assert!((lr(&out)[0] - 0.25 * CENTRE).abs() < 1e-4);

        // +3 dB wins back what a centred channel gives up on each side
        p[Mixer::PARAM_MASTER] = 3.0103;
        let out = run(&mut mixer, &[(0, &x)], &p, 40);
        assert!((lr(&out)[0] - 0.5).abs() < 1e-3);

        // The bottom of the knob is silence
        p[Mixer::PARAM_MASTER] = MASTER_FLOOR_DB;
        let out = run(&mut mixer, &[(0, &x)], &p, 40);
        assert_eq!(lr(&out), [0.0, 0.0]);
    }

    #[test]
    fn test_four_channels_sum() {
        let mut mixer = prepared();
        let bufs = [constant(0.1), constant(0.2), constant(0.3), constant(0.15)];
        let patched: Vec<_> = bufs.iter().enumerate().collect();
        let out = run(&mut mixer, &patched, &params(), 4);
        assert!((lr(&out)[0] - 0.75 * CENTRE).abs() < 1e-4);
    }

    #[test]
    fn test_poly_without_width_sums_at_the_pan() {
        let mut p = params();
        p[Mixer::PARAM_PAN] = 0.3;
        let mut a = prepared();
        let chord = run(&mut a, &[(0, &poly(&[0.1, 0.2, 0.15, 0.05]))], &p, 40);
        let mut b = prepared();
        let summed = run(&mut b, &[(0, &constant(0.5))], &p, 40);
        for (x, y) in lr(&chord).iter().zip(lr(&summed)) {
            assert!((x - y).abs() < 1e-5);
        }
        assert_eq!(chord[Mixer::OUT_L].channels(), 1, "the mix is one channel");
    }

    #[test]
    fn test_width_places_each_voice() {
        // Four voices, each its own level, so each one's place shows in L and R
        let levels = [0.1, 0.2, 0.3, 0.4];
        let mut p = params();
        p[Mixer::PARAM_WIDTH] = 1.0;
        let mut energies = Vec::new();
        for (v, &level) in levels.iter().enumerate() {
            let mut voice = [0.0; 4];
            voice[v] = level;
            let mut mixer = prepared();
            let out = run(&mut mixer, &[(0, &poly(&voice))], &p, 40);
            let [l, r] = lr(&out);
            // Each voice sits where its place says, at full power
            let [gl, gr] = pan_gains(spread_offset(v, 4));
            assert!((l - level * gl).abs() < 1e-4 && (r - level * gr).abs() < 1e-4, "voice {v}: {l} {r}");
            energies.push((l / level, r / level));
        }
        // Alternating sides from the outside in: hard left, hard right, then inside
        assert!(energies[0].1.abs() < 1e-4, "voice 1 hard left");
        assert!(energies[1].0.abs() < 1e-4, "voice 2 hard right");
        assert!(energies[2].0 > energies[2].1 && energies[2].1 > 0.1, "voice 3 left of centre");
        assert!(energies[3].1 > energies[3].0 && energies[3].0 > 0.1, "voice 4 right of centre");

        // With every voice playing, the left/right balance is even
        let mut mixer = prepared();
        let out = run(&mut mixer, &[(0, &poly(&[0.2; 4]))], &p, 40);
        assert!((lr(&out)[0] - lr(&out)[1]).abs() < 1e-4);
    }

    #[test]
    fn test_width_is_per_channel() {
        // The pad on channel 1 fans out; a lead on channel 2 stays put
        let mut p = params();
        p[Mixer::PARAM_WIDTH] = 1.0;
        let lead = poly(&[0.0, 0.4]);
        let mut mixer = prepared();
        let out = run(&mut mixer, &[(1, &lead)], &p, 40);
        assert!((lr(&out)[0] - lr(&out)[1]).abs() < 1e-5, "channel 2 has no width: centred");
        let mut mixer = prepared();
        let out = run(&mut mixer, &[(0, &lead)], &p, 40);
        assert!(lr(&out)[0].abs() < 1e-4, "channel 1's second voice fans hard right");
    }

    #[test]
    fn test_spread_offsets() {
        assert_eq!(spread_offset(0, 1), 0.0);
        assert_eq!([spread_offset(0, 2), spread_offset(1, 2)], [-1.0, 1.0]);
        assert_eq!([spread_offset(0, 3), spread_offset(1, 3), spread_offset(2, 3)], [-1.0, 1.0, 0.0]);
        // Eight voices take eight different places
        let mut places: Vec<f32> = (0..8).map(|v| spread_offset(v, 8)).collect();
        places.sort_by(f32::total_cmp);
        for pair in places.windows(2) {
            assert!((pair[1] - pair[0] - 2.0 / 7.0).abs() < 1e-5);
        }
        // Width fans out around the pan, and stops at the edges
        assert_eq!(voice_pan(0.5, 1.0, 1, 2), 1.0);
        assert_eq!(voice_pan(0.5, 0.25, 0, 2), 0.25);
    }

    #[test]
    fn test_meters_read_each_strip_the_master_the_return_and_the_chain() {
        let mut mixer = prepared();
        let (a, b, wet) = (constant(0.5), constant(-0.2), constant(0.05));
        let chain = bus([0.0, 0.07, 0.0, 0.0]);
        let mut p = params();
        p[1] = 0.5;
        run(&mut mixer, &[(0, &a), (2, &b), (Mixer::PORT_RETURN, &wet), (Mixer::PORT_CHAIN_IN, &chain)], &p, 20);
        let levels = mixer.take_meter_levels().unwrap().peaks;
        assert!((levels[0] - 0.5).abs() < 1e-3);
        assert_eq!(levels[1], 0.0, "channel 2 isn't patched");
        assert!((levels[2] - 0.2).abs() < 1e-3);
        assert!((levels[Mixer::METER_OUT_L] - (0.3 * CENTRE + 0.05)).abs() < 1e-3);
        assert!((levels[Mixer::METER_RETURN] - 0.05).abs() < 1e-6);
        assert!((levels[Mixer::METER_CHAIN] - 0.07).abs() < 1e-6);
        // Reading starts a new measurement
        assert_eq!(mixer.take_meter_levels().unwrap().peaks, [0.0; 8]);
    }

    #[test]
    fn test_chain_out_carries_the_mix_and_the_sends() {
        let mut mixer = prepared();
        let x = constant(0.5);
        let mut p = params();
        p[Mixer::PARAM_PAN] = -0.4;
        p[Mixer::PARAM_SEND] = 0.3;
        p[Mixer::PARAM_MASTER] = -6.0;
        let out = run(&mut mixer, &[(0, &x)], &p, 40);
        let chain = &out[Mixer::CHAIN_OUT];
        assert_eq!(chain.channels(), BUS_STRANDS);
        for (k, from) in [Mixer::OUT_L, Mixer::OUT_R, Mixer::SEND_L, Mixer::SEND_R].into_iter().enumerate() {
            assert_eq!(chain.voice(k).samples, out[from].samples, "strand {k}");
        }
    }

    #[test]
    fn test_two_mixers_chain_over_one_cable() {
        // A tone hard left with a send on the first mixer stays hard left
        // through the second, and its send joins the second's send bus
        let mut p = params();
        p[Mixer::PARAM_PAN] = -1.0;
        p[Mixer::PARAM_SEND] = 0.5;
        let mut first = prepared();
        let tone = constant(0.5);
        let a = run(&mut first, &[(0, &tone)], &p, 40);

        let mut q = params();
        q[Mixer::PARAM_PAN] = 1.0;
        let mut second = prepared();
        let other = constant(0.2);
        let b = run(&mut second, &[(0, &other), (Mixer::PORT_CHAIN_IN, &a[Mixer::CHAIN_OUT])], &q, 4);
        assert!((lr(&b)[0] - 0.5).abs() < 1e-4, "left is the chained tone: {}", lr(&b)[0]);
        assert!((lr(&b)[1] - 0.2).abs() < 1e-4, "right is only the second mixer's own channel: {}", lr(&b)[1]);
        assert!((sends(&b)[0] - 0.25).abs() < 1e-4, "the first mixer's send rides along");
        assert!(sends(&b)[1].abs() < 1e-6);

        // The chain comes in before the Master, which turns it down too
        q[Mixer::PARAM_MASTER] = -6.0206;
        let b = run(&mut second, &[(Mixer::PORT_CHAIN_IN, &a[Mixer::CHAIN_OUT])], &q, 40);
        assert!((lr(&b)[0] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn test_one_hop_passes_the_mix_bit_for_bit() {
        // Strands copied, not resummed: a chained mix arrives exactly
        let mut p = params();
        p[Mixer::PARAM_PAN] = 0.37;
        p[Mixer::PARAM_SEND] = 0.21;
        let mut first = prepared();
        let a = run(&mut first, &[(0, &constant(0.43))], &p, 3);
        let mut second = prepared();
        let b = run(&mut second, &[(Mixer::PORT_CHAIN_IN, &a[Mixer::CHAIN_OUT])], &params(), 1);
        for k in 0..BUS_STRANDS {
            assert_eq!(b[k].samples, a[k].samples, "output {k}");
        }
    }

    #[test]
    fn test_solo_silences_the_chain() {
        let mut mixer = prepared();
        let chain = bus([0.2; BUS_STRANDS]);
        let x = constant(0.5);
        run(&mut mixer, &[(0, &x), (Mixer::PORT_CHAIN_IN, &chain)], &params(), 4);
        let mut p = params();
        p[Mixer::PARAM_SOLO] = 1.0;
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_CHAIN_IN, &chain)], &p, 1);
        assert!(out[Mixer::OUT_L].samples[0] > 0.2 + 0.5 * CENTRE - 0.01, "fades rather than drops");
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_CHAIN_IN, &chain)], &p, 20);
        assert!((lr(&out)[0] - 0.5 * CENTRE).abs() < 1e-4);
        assert!(sends(&out)[0].abs() < 1e-4);
    }

    #[test]
    fn test_one_return_cable_comes_in_centred() {
        let mut mixer = prepared();
        let x = constant(0.4);
        let out = run(&mut mixer, &[(Mixer::PORT_RETURN, &x)], &params(), 2);
        assert_eq!(lr(&out), [0.4, 0.4], "Return R copies Return L");

        // Only the right patched: the left stays silent
        let out = run(&mut mixer, &[(Mixer::PORT_RETURN + 1, &x)], &params(), 2);
        assert_eq!(lr(&out), [0.0, 0.4]);
    }

    #[test]
    fn test_send_follows_fader_and_pan_but_not_master() {
        let mut mixer = prepared();
        let x = constant(0.8);
        let mut p = params();
        p[Mixer::PARAM_PAN] = -1.0;
        p[0] = 0.5;
        p[Mixer::PARAM_SEND] = 0.5;
        p[Mixer::PARAM_MASTER] = MASTER_FLOOR_DB;
        let out = run(&mut mixer, &[(0, &x)], &p, 40);
        assert_eq!(lr(&out)[0], 0.0, "the Master is all the way down");
        assert!((sends(&out)[0] - 0.8 * 0.5 * 0.5).abs() < 1e-4, "Send L after the fader: {}", sends(&out)[0]);
        assert!(sends(&out)[1].abs() < 1e-6, "panned hard left, so nothing on Send R");

        // A muted channel sends nothing either
        p[Mixer::PARAM_MUTE] = 1.0;
        let out = run(&mut mixer, &[(0, &x)], &p, 40);
        assert!(sends(&out)[0].abs() < 1e-4);
    }

    #[test]
    fn test_send_cv_adds_to_the_knob() {
        let mut mixer = prepared();
        let x = constant(0.8);
        let mut p = params();
        p[Mixer::PARAM_PAN] = -1.0;
        p[Mixer::PARAM_SEND] = 0.25;
        let cv = constant(0.5);
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_SEND_CV, &cv)], &p, 40);
        assert!((sends(&out)[0] - 0.8 * 0.75).abs() < 1e-4, "knob and CV add: {}", sends(&out)[0]);
        assert!((lr(&out)[0] - 0.8).abs() < 1e-4, "the dry mix is untouched");

        // A lane at 1 is a full send from a knob at 0, and never more
        p[Mixer::PARAM_SEND] = 0.0;
        let cv = constant(1.0);
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_SEND_CV, &cv)], &p, 40);
        assert!((sends(&out)[0] - 0.8).abs() < 1e-4);
        let cv = constant(3.0);
        let out = run(&mut mixer, &[(0, &x), (Mixer::PORT_SEND_CV, &cv)], &p, 2);
        assert!((sends(&out)[0] - 0.8).abs() < 1e-4);

        // Each channel's CV is its own: channel 2 stays dry
        let out = run(&mut mixer, &[(1, &x), (Mixer::PORT_SEND_CV, &cv)], &p, 40);
        assert!(sends(&out)[0].abs() < 1e-6, "Send 1 CV moved channel 2: {}", sends(&out)[0]);
        let out = run(&mut mixer, &[(1, &x), (Mixer::PORT_SEND_CV + 1, &cv)], &p, 40);
        assert!(sends(&out)[0] > 0.5);
    }

    #[test]
    fn test_return_cv_adds_to_the_knob() {
        let mut mixer = prepared();
        let wet = constant(0.4);
        let mut p = params();
        p[Mixer::PARAM_RETURN] = 0.0;
        let cv = constant(0.5);
        let out = run(&mut mixer, &[(Mixer::PORT_RETURN, &wet), (Mixer::PORT_RETURN_CV, &cv)], &p, 40);
        assert!((lr(&out)[0] - 0.2).abs() < 1e-4, "{}", lr(&out)[0]);

        // Clamped both ways: never past full, never below silence
        p[Mixer::PARAM_RETURN] = 1.0;
        let out = run(&mut mixer, &[(Mixer::PORT_RETURN, &wet), (Mixer::PORT_RETURN_CV, &constant(2.0))], &p, 40);
        assert!((lr(&out)[0] - 0.4).abs() < 1e-4);
        let out = run(&mut mixer, &[(Mixer::PORT_RETURN, &wet), (Mixer::PORT_RETURN_CV, &constant(-2.0))], &p, 40);
        assert_eq!(lr(&out), [0.0, 0.0]);
    }

    #[test]
    fn test_a_lane_into_return_cv_fades_the_room_without_a_click() {
        // A steady room, a knob at 0, and a lane ramping 0 to 1 over a block
        // (as an Arranger's Ramp or Glide hands it over): the return comes in
        // sample by sample, never in a step
        let mut mixer = prepared();
        let wet = constant(0.5);
        let mut p = params();
        p[Mixer::PARAM_RETURN] = 0.0;
        let mut ramp = SignalBuffer::control(BLOCK);
        for (i, sample) in ramp.samples.iter_mut().enumerate() {
            *sample = i as f32 / (BLOCK - 1) as f32;
        }
        let out = run(&mut mixer, &[(Mixer::PORT_RETURN, &wet), (Mixer::PORT_RETURN_CV, &ramp)], &p, 1);
        let left = &out[Mixer::OUT_L].samples;
        assert_eq!(left[0], 0.0);
        assert!((left[BLOCK - 1] - 0.5).abs() < 1e-4);
        let biggest_step = left.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(biggest_step < 0.5 / (BLOCK - 1) as f32 + 1e-5, "steps by {biggest_step}");
    }

    #[test]
    fn test_sends_are_silent_until_turned_up() {
        let mut mixer = prepared();
        let x = constant(0.8);
        let out = run(&mut mixer, &[(0, &x), (1, &x)], &params(), 4);
        assert!(out[Mixer::SEND_L].samples.iter().chain(&out[Mixer::SEND_R].samples).all(|&s| s == 0.0));
    }

    #[test]
    fn test_return_comes_in_at_its_knob() {
        let mut mixer = prepared();
        let (wet_l, wet_r) = (constant(0.2), constant(-0.1));
        let mut p = params();
        let patched = [(Mixer::PORT_RETURN, &wet_l), (Mixer::PORT_RETURN + 1, &wet_r)];
        let out = run(&mut mixer, &patched, &p, 2);
        assert_eq!(lr(&out), [0.2, -0.1]);
        assert!(out[Mixer::SEND_L].samples.iter().all(|&s| s == 0.0), "the return never feeds the send");

        p[Mixer::PARAM_RETURN] = 0.5;
        let out = run(&mut mixer, &patched, &p, 40);
        assert!((lr(&out)[0] - 0.1).abs() < 1e-4);
        assert!((lr(&out)[1] + 0.05).abs() < 1e-4);
    }

    #[test]
    fn test_unpatched_is_silent() {
        let mut mixer = prepared();
        let out = run(&mut mixer, &[], &params(), 2);
        for buf in &out {
            assert!(buf.samples.iter().all(|&s| s == 0.0));
        }
    }

    #[test]
    fn test_soft_clip_is_continuous_and_eases() {
        // Unity inside full scale
        assert_eq!(soft_clip(0.7), 0.7);
        assert_eq!(soft_clip(-1.0), -1.0);

        // No step at the knee: just past it, still just past 1
        assert!((soft_clip(1.001) - 1.001).abs() < 1e-4);
        assert!((soft_clip(-1.001) + 1.001).abs() < 1e-4);

        // Never falls back as the input grows, and stays under the ceiling
        let mut prev = soft_clip(1.0);
        for i in 1..=400 {
            let y = soft_clip(1.0 + i as f32 * 0.01);
            assert!(y >= prev, "falls at {}", 1.0 + i as f32 * 0.01);
            assert!(y <= CLIP_CEILING);
            prev = y;
        }

        // Two full-scale signals land a little under the ceiling
        assert!((soft_clip(2.0) - 1.482).abs() < 0.001);
    }

    #[test]
    fn test_hot_sums_soft_clip() {
        let mut mixer = prepared();
        let x = constant(1.0);
        let mut p = params();
        p[Mixer::PARAM_PAN] = -1.0;
        p[Mixer::PARAM_PAN + 1] = -1.0;
        let out = run(&mut mixer, &[(0, &x), (1, &x)], &p, 40);
        assert!((lr(&out)[0] - soft_clip(2.0)).abs() < 1e-4);
    }

    #[test]
    fn test_mixer_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Mixer>();
    }

    #[test]
    fn test_mixer_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Mixer>();
        let module = registry.create("util.mixer").unwrap();
        assert_eq!(module.info().id, "util.mixer");
        assert_eq!(module.ports().len(), Mixer::INPUTS + 5);
        assert_eq!(module.parameters().len(), Mixer::PARAMS);
    }
}
