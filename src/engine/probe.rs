//! The cable probe: what a cable carries, heard on its own (#296).
//!
//! Hold Alt over a cable and its signal plays beside the patch, like the
//! cue (see [`super::cue`]): after the patch and its limiter, at the cue's
//! level, and not into a recording. It's the patch-cable trick of real
//! modular, touching a lead to a jack to hear what's on it.
//!
//! The probe reads the output buffer the cable leaves from, which the plan
//! keeps for every output anyway, so it needs nothing new on the audio
//! thread. What it plays depends on what the cable carries:
//!
//! - **Audio** plays as it is, its channels summed.
//! - **Control** plays as a quiet sine, pitched by the value: 0 to 1 is two
//!   octaves up from A3, and down to -1 is two octaves below.
//! - **Gate** plays a soft click on each rising edge, in any of its voices,
//!   and a faint tone while any is high, so a gate's length can be heard as
//!   well as its start.
//!
//! Whatever it plays is soft-limited under [`CUE_LEVEL`], so a probe can't
//! come out louder than a cue. The patch is turned down beneath it (heard,
//! not recorded): almost to nothing under an audio cable, which all but
//! solos it, and only by half under a control or gate cable, whose sine or
//! clicks make sense against the music they move. Starting, stopping and moving to another
//! cable fade, so none of them click: a move fades the old cable out before
//! the new one comes in.
//!
//! The probe also copies what it hears into a [`ProbeScope`], a ring the
//! UI shares, for the waveform drawn beside the pointer.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::dsp::{SignalBuffer, SignalType, MAX_CHANNELS};

use super::commands::NodeId;
use super::cue::CUE_LEVEL;
use super::graph_plan::GraphPlan;

/// A cable to hear, by the output it leaves from.
#[derive(Clone)]
pub struct Probe {
    /// The engine node the cable leaves.
    pub node_id: NodeId,
    /// The output's position among the node's outputs.
    pub output_index: usize,
    /// Where to copy what it hears, for the UI to draw. The UI keeps its own
    /// handle for as long as the app runs, so the audio thread letting go
    /// of this one never frees it.
    pub scope: Arc<ProbeScope>,
}

impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probe").field("node_id", &self.node_id).field("output_index", &self.output_index).finish()
    }
}

/// How many samples the scope's ring holds.
pub const SCOPE_RING: usize = 4096;

/// How many recent samples the UI reads at a time: half the ring, so the
/// block being written is never in the half being read.
pub const SCOPE_WINDOW: usize = SCOPE_RING / 2;

/// Every how many samples a slow signal (control or gate) is kept in the
/// scope, so its window spans more than a second rather than 40 ms.
const SLOW_STRIDE: usize = 32;

/// What the probe heard, recently, for the UI to draw. Atomics, as in
/// [`super::SpectrumTap`], so neither side waits.
pub struct ProbeScope {
    ring: Box<[AtomicU32]>,
    /// Samples kept so far, wrapping. Published after the samples.
    written: AtomicUsize,
    /// What the samples are (see [`ProbeScope::kind`]).
    kind: AtomicU32,
}

impl ProbeScope {
    pub fn new() -> Self {
        Self { ring: (0..SCOPE_RING).map(|_| AtomicU32::new(0)).collect(), written: AtomicUsize::new(0), kind: AtomicU32::new(0) }
    }

    /// Keeps one sample.
    ///
    /// REAL-TIME SAFE.
    #[inline]
    fn push(&self, sample: f32) {
        let at = self.written.load(Ordering::Relaxed);
        self.ring[at % SCOPE_RING].store(sample.to_bits(), Ordering::Relaxed);
        self.written.store(at.wrapping_add(1), Ordering::Release);
    }

    /// Copies the newest [`SCOPE_WINDOW`] samples, oldest first, and returns
    /// how many had been kept by then, so a reader can tell whether anything
    /// new has arrived.
    pub fn read(&self, window: &mut [f32; SCOPE_WINDOW]) -> usize {
        let written = self.written.load(Ordering::Acquire);
        let oldest = written.wrapping_sub(SCOPE_WINDOW);
        for (n, sample) in window.iter_mut().enumerate() {
            *sample = f32::from_bits(self.ring[oldest.wrapping_add(n) % SCOPE_RING].load(Ordering::Relaxed));
        }
        written
    }

    /// How many samples have been kept so far, wrapping.
    pub fn written(&self) -> usize {
        self.written.load(Ordering::Acquire)
    }

    /// What kind of signal the samples are, once the probe has heard some:
    /// a slow one (control or gate) is kept every [`SLOW_STRIDE`]th sample.
    pub fn kind(&self) -> Option<SignalType> {
        match self.kind.load(Ordering::Relaxed) {
            1 => Some(SignalType::Audio),
            2 => Some(SignalType::Control),
            3 => Some(SignalType::Gate),
            _ => None,
        }
    }

    fn set_kind(&self, kind: SignalType) {
        let code = match kind {
            SignalType::Audio | SignalType::Bus => 1,
            SignalType::Control => 2,
            SignalType::Gate => 3,
            SignalType::Midi => 0,
        };
        self.kind.store(code, Ordering::Relaxed);
    }
}

impl Default for ProbeScope {
    fn default() -> Self {
        Self::new()
    }
}

/// The most frames one callback's probe can hold; past it, the rest of a
/// callback goes unheard. Far beyond any device buffer Soba asks for.
const MAX_FRAMES: usize = 16384;

/// How long a probe takes to come in.
const ATTACK_SECONDS: f32 = 0.010;

/// How long a probe takes to go, when it's let go of or moved.
const RELEASE_SECONDS: f32 = 0.015;

/// How quickly a control probe's pitch follows its value.
const GLIDE_SECONDS: f32 = 0.005;

/// A control probe's pitch at 0: A3.
const CONTROL_BASE_HZ: f32 = 220.0;

/// How loud a control probe's sine is, before the cue level.
const CONTROL_LEVEL: f32 = 0.45;

/// A gate probe's click: a short ping, this high and this quick to die.
const CLICK_HZ: f32 = 1800.0;
const CLICK_SECONDS: f32 = 0.006;
const CLICK_LEVEL: f32 = 0.9;

/// The faint tone under a gate that's high, an octave under A4.
const HELD_HZ: f32 = 440.0;
const HELD_LEVEL: f32 = 0.08;

/// How far the patch is turned down beneath an audio probe: 20 dB, so the
/// cable is all but soloed.
const AUDIO_DUCK: f32 = 0.9;

/// ...and beneath a control or gate probe: 6 dB, so the music is still
/// there to hear the modulation against.
const SIGNAL_DUCK: f32 = 0.5;

/// How long the patch takes to go down, and to come back up. Slower than
/// the probe's own fades, so moving from cable to cable doesn't let the
/// patch swell up in between.
const DUCK_SECONDS: f32 = 0.030;
const UNDUCK_SECONDS: f32 = 0.120;

/// Where the soft limit starts to bend.
const KNEE: f32 = 0.6;

/// Bends anything past the knee towards 1, never reaching it.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let size = x.abs();
    if size <= KNEE {
        x
    } else {
        (KNEE + (1.0 - KNEE) * ((size - KNEE) / (1.0 - KNEE)).tanh()).copysign(x)
    }
}

/// Plays the probed cable, on the audio thread.
///
/// REAL-TIME SAFE: everything it needs is set aside when it's made.
pub struct ProbePlayer {
    /// The cable heard now.
    probe: Option<Probe>,
    /// Where it's moving to once the one heard has faded: `Some(None)` is a
    /// stop.
    next: Option<Option<Probe>>,
    /// The probe's output buffer in the plan, found each callback.
    source: Option<usize>,
    /// This callback's sound, mono, ready to mix in.
    heard: Box<[f32]>,
    /// How loud the patch plays beneath it, frame by frame, 0 to 1.
    under: Box<[f32]>,
    /// How far the patch is turned down now, and how far for the kind of
    /// cable heard.
    duck: f32,
    depth: f32,
    /// How much of `heard` this callback has filled.
    filled: usize,
    /// The fade, 0 to 1.
    envelope: f32,
    /// The sine's phase, 0 to 1, and its pitch's smoothed value.
    phase: f32,
    value: f32,
    /// The click's phase and level, the held tone's level, and each voice's
    /// last gate, for spotting a rise.
    click_phase: f32,
    click: f32,
    held: f32,
    gate_was: [bool; MAX_CHANNELS],
    /// Samples since the last one kept in the scope.
    stride: usize,
}

impl Default for ProbePlayer {
    fn default() -> Self {
        Self {
            probe: None,
            next: None,
            source: None,
            heard: vec![0.0; MAX_FRAMES].into_boxed_slice(),
            under: vec![1.0; MAX_FRAMES].into_boxed_slice(),
            duck: 0.0,
            depth: AUDIO_DUCK,
            filled: 0,
            envelope: 0.0,
            phase: 0.0,
            value: 0.0,
            click_phase: 0.0,
            click: 0.0,
            held: 0.0,
            gate_was: [false; MAX_CHANNELS],
            stride: 0,
        }
    }
}

impl ProbePlayer {
    /// Hears another cable, or with `None` stops, fading out what's heard
    /// first.
    pub fn probe(&mut self, probe: Option<Probe>) {
        let same = match (&self.probe, &probe) {
            (Some(a), Some(b)) => a.node_id == b.node_id && a.output_index == b.output_index,
            _ => false,
        };
        if same {
            // Back onto the cable still fading: it just comes up again
            self.next = None;
        } else if self.probe.is_none() {
            self.adopt(probe);
        } else {
            self.next = Some(probe);
        }
    }

    /// Whether a cable is being heard, or fading, or the patch is still
    /// coming back up after one.
    pub fn is_probing(&self) -> bool {
        self.probe.is_some() || self.duck > 0.0
    }

    fn adopt(&mut self, probe: Option<Probe>) {
        self.next = None;
        self.envelope = 0.0;
        self.click = 0.0;
        self.held = 0.0;
        self.gate_was = [false; MAX_CHANNELS];
        self.value = f32::NAN;
        self.probe = probe;
    }

    /// Readies a callback: finds the probed output's buffer in the plan,
    /// as it may be a new plan.
    ///
    /// REAL-TIME SAFE: looks along the plan's nodes, allocating nothing.
    pub fn begin(&mut self, plan: &GraphPlan) {
        self.filled = 0;
        self.source = self.probe.as_ref().and_then(|probe| {
            let node = plan.nodes.iter().find(|node| node.node_id == probe.node_id)?;
            node.outputs.clone().nth(probe.output_index)
        });
    }

    /// Listens to the block the plan just ran, and makes this block's
    /// sound from it.
    ///
    /// REAL-TIME SAFE.
    pub fn listen(&mut self, plan: &GraphPlan, frames: usize, sample_rate: f32) {
        if self.probe.is_none() && self.duck == 0.0 {
            return;
        }
        let frames = frames.min(MAX_FRAMES - self.filled);
        let duck_down = 1.0 / (DUCK_SECONDS * sample_rate).max(1.0);
        let duck_up = 1.0 / (UNDUCK_SECONDS * sample_rate).max(1.0);
        let rise = 1.0 / (ATTACK_SECONDS * sample_rate).max(1.0);
        let fall = 1.0 / (RELEASE_SECONDS * sample_rate).max(1.0);
        let glide = 1.0 - (-1.0 / (GLIDE_SECONDS * sample_rate)).exp();
        let click_decay = (-1.0 / (CLICK_SECONDS * sample_rate)).exp();
        let buffer = self.source.and_then(|index| plan.outputs.get(index));
        let kind = buffer.map_or(SignalType::Audio, |buffer| buffer.signal_type);
        if let (Some(probe), Some(_)) = (&self.probe, buffer) {
            probe.scope.set_kind(kind);
            self.depth = match kind {
                SignalType::Audio | SignalType::Bus => AUDIO_DUCK,
                _ => SIGNAL_DUCK,
            };
        }
        // The patch stays down while a cable is heard or moved between, and
        // comes back once it's let go of
        let ducking = self.probe.is_some() && !matches!(self.next, Some(None));
        let duck_to = if ducking { self.depth } else { 0.0 };
        let slow = matches!(kind, SignalType::Control | SignalType::Gate);

        for n in 0..frames {
            self.duck = if self.duck < duck_to {
                (self.duck + duck_down).min(duck_to)
            } else {
                (self.duck - duck_up).max(duck_to)
            };
            self.under[self.filled + n] = 1.0 - self.duck;

            // Fading towards another cable, or out
            let leaving = self.next.is_some() || buffer.is_none();
            if leaving {
                self.envelope -= fall;
                if self.envelope <= 0.0 {
                    self.envelope = 0.0;
                    if let Some(next) = self.next.take() {
                        // The rest of this callback is silent; the new cable
                        // is found from the next one
                        self.adopt(next);
                        self.source = None;
                        self.heard[self.filled + n..self.filled + frames].fill(0.0);
                        self.under[self.filled + n..self.filled + frames].fill(1.0 - self.duck);
                        self.filled += frames;
                        return;
                    }
                }
            } else {
                self.envelope = (self.envelope + rise).min(1.0);
            }

            let signal = buffer.map_or(0.0, |buffer| sum(buffer, n, kind));
            let sound = match kind {
                SignalType::Audio | SignalType::Bus => signal,
                SignalType::Control => {
                    let target = signal.clamp(-1.0, 1.0);
                    self.value = if self.value.is_nan() { target } else { self.value + (target - self.value) * glide };
                    let hz = CONTROL_BASE_HZ * 4f32.powf(self.value);
                    self.phase = (self.phase + hz / sample_rate).fract();
                    (self.phase * std::f32::consts::TAU).sin() * CONTROL_LEVEL
                }
                SignalType::Gate => {
                    let mut high = false;
                    if let Some(buffer) = buffer {
                        for (channel, was) in self.gate_was.iter_mut().enumerate().take(buffer.channels()) {
                            let is = buffer.voice(channel).samples.get(n).is_some_and(|&gate| gate > 0.5);
                            if is && !*was {
                                self.click = 1.0;
                                self.click_phase = 0.0;
                            }
                            *was = is;
                            high |= is;
                        }
                    }
                    self.click *= click_decay;
                    self.click_phase = (self.click_phase + CLICK_HZ / sample_rate).fract();
                    let target = if high { HELD_LEVEL } else { 0.0 };
                    self.held += (target - self.held) * glide;
                    self.phase = (self.phase + HELD_HZ / sample_rate).fract();
                    (self.click_phase * std::f32::consts::TAU).sin() * self.click * CLICK_LEVEL
                        + (self.phase * std::f32::consts::TAU).sin() * self.held
                }
                SignalType::Midi => 0.0,
            };
            self.heard[self.filled + n] = soft_limit(sound) * self.envelope * CUE_LEVEL;

            // Every sample of audio, every so many of something slow
            self.stride += 1;
            if !slow || self.stride >= SLOW_STRIDE {
                self.stride = 0;
                if let (Some(probe), Some(_)) = (&self.probe, buffer) {
                    probe.scope.push(signal);
                }
            }
        }
        self.filled += frames;

        // Faded right out with nothing to move to: stopped
        if self.envelope == 0.0 && self.next.is_none() && buffer.is_none() {
            self.probe = None;
        }
    }

    /// Turns the patch in `output` (interleaved, `channels` wide) down
    /// beneath the probe and adds the probe's sound, on every channel.
    ///
    /// REAL-TIME SAFE.
    pub fn mix(&mut self, output: &mut [f32], channels: usize) {
        if channels == 0 || self.filled == 0 {
            return;
        }
        let sounds = self.heard[..self.filled].iter().zip(&self.under[..self.filled]);
        for (frame, (&sound, &under)) in output.chunks_exact_mut(channels).zip(sounds) {
            for sample in frame.iter_mut() {
                *sample = (*sample * under + sound).clamp(-1.0, 1.0);
            }
        }
        self.filled = 0;
    }

    /// Ends a callback in which the patch didn't run (the transport's
    /// stopped): a cable carries nothing, so a probe that's let go of or
    /// moved goes at once.
    pub fn idle(&mut self) {
        self.filled = 0;
        self.duck = 0.0;
        if let Some(next) = self.next.take() {
            self.adopt(next);
        }
    }
}

/// Sample `n` of a buffer as the probe hears it: every channel summed, but
/// for a mix bus, whose third strand is its send rather than more of the mix.
#[inline]
fn sum(buffer: &SignalBuffer, n: usize, kind: SignalType) -> f32 {
    let channels = match kind {
        SignalType::Bus => buffer.channels().min(2),
        // A poly control or gate is drawn as its average, so it stays in range
        _ => buffer.channels(),
    };
    let total: f32 = (0..channels).map(|channel| buffer.voice(channel).samples.get(n).copied().unwrap_or(0.0)).sum();
    match kind {
        SignalType::Control | SignalType::Gate if channels > 1 => total / channels as f32,
        _ => total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48000.0;
    const BLOCK: usize = 480;

    /// A plan with one node whose one output is `buffer`.
    fn plan_with(buffer: SignalBuffer) -> GraphPlan {
        let mut plan = GraphPlan::empty(BLOCK);
        plan.outputs.push(buffer);
        plan.nodes.push(crate::engine::graph_plan::PlanNode::bare(9, 0..1));
        plan
    }

    fn probe(scope: &Arc<ProbeScope>) -> Option<Probe> {
        Some(Probe { node_id: 9, output_index: 0, scope: scope.clone() })
    }

    /// `blocks` blocks of the player's sound from `plan`, refilled by `fill`
    /// before each block.
    fn hear(player: &mut ProbePlayer, plan: &mut GraphPlan, blocks: usize, mut fill: impl FnMut(usize, &mut [f32])) -> Vec<f32> {
        let mut heard = Vec::new();
        for block in 0..blocks {
            if let Some(buffer) = plan.outputs.first_mut() {
                fill(block, &mut buffer.samples);
            }
            player.begin(plan);
            player.listen(plan, BLOCK, RATE);
            let mut output = vec![0.0; BLOCK * 2];
            player.mix(&mut output, 2);
            heard.extend(output.iter().step_by(2));
        }
        heard
    }

    fn sine(hz: f32) -> impl FnMut(usize, &mut [f32]) {
        move |block, samples| {
            for (n, s) in samples.iter_mut().enumerate() {
                *s = ((block * BLOCK + n) as f32 * hz / RATE * std::f32::consts::TAU).sin();
            }
        }
    }

    #[test]
    fn test_audio_plays_as_it_is_at_the_cue_level() {
        let scope = Arc::new(ProbeScope::new());
        let mut plan = plan_with(SignalBuffer::audio(BLOCK));
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        let heard = hear(&mut player, &mut plan, 10, |_, s| s.fill(0.25));
        // Past the fade in, a quiet signal comes through untouched
        assert!((heard[4000] - 0.25 * CUE_LEVEL).abs() < 1e-5, "{}", heard[4000]);
        assert_eq!(scope.kind(), Some(SignalType::Audio));
        let mut window = [0.0; SCOPE_WINDOW];
        scope.read(&mut window);
        assert_eq!(window[SCOPE_WINDOW - 1], 0.25, "the scope sees the signal itself");
    }

    #[test]
    fn test_nothing_comes_out_louder_than_the_cue_level() {
        let scope = Arc::new(ProbeScope::new());
        let mut buffer = SignalBuffer::polyphonic(BLOCK, SignalType::Audio);
        buffer.set_channels(4);
        // Four voices at full scale, summed
        for channel in 0..4 {
            buffer.channel_mut(channel).fill(1.0);
        }
        let mut plan = plan_with(buffer);
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        let heard = hear(&mut player, &mut plan, 10, |_, _| {});
        let loudest = heard.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(loudest <= CUE_LEVEL && loudest > 0.9 * CUE_LEVEL, "{loudest}");
    }

    #[test]
    fn test_control_plays_a_sine_pitched_by_its_value() {
        let scope = Arc::new(ProbeScope::new());
        let mut plan = plan_with(SignalBuffer::control(BLOCK));
        let mut crossings = |value: f32| {
            let mut player = ProbePlayer::default();
            player.probe(probe(&scope));
            let heard = hear(&mut player, &mut plan, 100, |_, s| s.fill(value));
            // Rising zero crossings over the last half second
            heard[24000..].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
        };
        assert!((crossings(0.0) as i32 - 110).abs() <= 1, "A3 at 0");
        assert!((crossings(0.5) as i32 - 220).abs() <= 1, "an octave up at 0.5");
        assert!((crossings(1.0) as i32 - 440).abs() <= 1, "two octaves up at 1");
        assert_eq!(scope.kind(), Some(SignalType::Control));
    }

    #[test]
    fn test_gate_clicks_on_each_rise() {
        let scope = Arc::new(ProbeScope::new());
        let mut plan = plan_with(SignalBuffer::gate(BLOCK));
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        // High for the first half of every tenth of a second
        let heard = hear(&mut player, &mut plan, 50, |block, s| s.fill(if block % 10 < 5 { 1.0 } else { 0.0 }));
        let loud = |range: std::ops::Range<usize>| heard[range].iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(loud(4800..5100) > 0.3, "a click as it rises");
        assert!(loud(6000..7000) < 0.06, "only the faint tone while it's held");
        assert!(loud(8500..9500) < 1e-3, "silence while it's low");
    }

    #[test]
    fn test_one_voice_of_a_poly_gate_clicks() {
        let scope = Arc::new(ProbeScope::new());
        let mut buffer = SignalBuffer::polyphonic(BLOCK, SignalType::Gate);
        buffer.set_channels(8);
        let mut plan = plan_with(buffer);
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        hear(&mut player, &mut plan, 5, |_, _| {});
        plan.outputs[0].channel_mut(5).fill(1.0);
        let heard = hear(&mut player, &mut plan, 1, |_, _| {});
        assert!(heard.iter().fold(0.0f32, |a, s| a.max(s.abs())) > 0.3, "the sixth voice's note clicks");
    }

    /// The patch's level beneath the probe, `blocks` blocks of a patch that
    /// plays a steady 0.4, as a fraction of it.
    fn under(player: &mut ProbePlayer, plan: &GraphPlan, blocks: usize) -> Vec<f32> {
        let mut level = Vec::new();
        for _ in 0..blocks {
            player.begin(plan);
            player.listen(plan, BLOCK, RATE);
            let mut output = vec![0.4; BLOCK * 2];
            player.mix(&mut output, 2);
            level.extend(output.iter().step_by(2).map(|s| s / 0.4));
        }
        level
    }

    #[test]
    fn test_the_patch_ducks_deep_under_audio_and_less_under_cv() {
        let scope = Arc::new(ProbeScope::new());
        // A silent audio cable, so what's left is the patch
        let plan = plan_with(SignalBuffer::audio(BLOCK));
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        let level = under(&mut player, &plan, 10);
        assert!((level.last().unwrap() - (1.0 - AUDIO_DUCK)).abs() < 1e-5, "{}", level.last().unwrap());
        assert!(level.windows(2).all(|w| (w[1] - w[0]).abs() < 0.001), "it goes down smoothly");

        // Let go: the patch comes back up, smoothly, and then the probe's done
        player.probe(None);
        let level = under(&mut player, &plan, 20);
        assert!(level.windows(2).all(|w| (w[1] - w[0]).abs() < 0.001));
        assert_eq!(*level.last().unwrap(), 1.0);
        assert!(!player.is_probing());

        // A control or gate cable: only halfway down
        for cable in [SignalBuffer::control(BLOCK), SignalBuffer::gate(BLOCK)] {
            let plan = plan_with(cable);
            let mut player = ProbePlayer::default();
            player.probe(probe(&scope));
            under(&mut player, &plan, 10);
            assert!((player.duck - SIGNAL_DUCK).abs() < 1e-5, "{}", player.duck);
        }
    }

    #[test]
    fn test_moving_between_cables_keeps_the_patch_down() {
        let scope = Arc::new(ProbeScope::new());
        let mut plan = plan_with(SignalBuffer::audio(BLOCK));
        plan.outputs.push(SignalBuffer::audio(BLOCK));
        plan.nodes.push(crate::engine::graph_plan::PlanNode::bare(10, 1..2));
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        under(&mut player, &plan, 10);
        player.probe(Some(Probe { node_id: 10, output_index: 0, scope: scope.clone() }));
        let level = under(&mut player, &plan, 10);
        assert!(level.iter().all(|&l| (l - (1.0 - AUDIO_DUCK)).abs() < 1e-5), "no swell between cables");
    }

    #[test]
    fn test_letting_go_and_moving_fade_without_a_click() {
        let scope = Arc::new(ProbeScope::new());
        let mut plan = plan_with(SignalBuffer::audio(BLOCK));
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        let mut heard = hear(&mut player, &mut plan, 5, sine(440.0));
        player.probe(None);
        heard.extend(hear(&mut player, &mut plan, 15, sine(440.0)));
        let largest = heard.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // A 440 Hz sine at the cue level moves at most about 0.03 a sample
        assert!(largest < 0.04, "{largest}");
        assert!(heard[2400 + 1000..].iter().all(|&s| s == 0.0), "gone after the fade");
        assert!(!player.is_probing());

        // Moving to another cable fades the first out, then the next in
        let mut other = SignalBuffer::audio(BLOCK);
        other.samples.fill(-0.5);
        plan.outputs.push(other);
        plan.nodes.push(crate::engine::graph_plan::PlanNode::bare(10, 1..2));
        player.probe(probe(&scope));
        let mut heard = hear(&mut player, &mut plan, 5, |_, s| s.fill(0.5));
        player.probe(Some(Probe { node_id: 10, output_index: 0, scope: scope.clone() }));
        heard.extend(hear(&mut player, &mut plan, 10, |_, s| s.fill(0.5)));
        let largest = heard.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(largest < 0.01, "{largest}");
        assert!((heard.last().unwrap() + 0.5 * CUE_LEVEL).abs() < 1e-5, "the new one plays");
    }

    #[test]
    fn test_a_probe_lets_go_of_a_node_that_goes() {
        let scope = Arc::new(ProbeScope::new());
        let mut plan = plan_with(SignalBuffer::audio(BLOCK));
        let mut player = ProbePlayer::default();
        player.probe(probe(&scope));
        hear(&mut player, &mut plan, 5, |_, s| s.fill(0.5));
        // Its sound went with it; the probe lets go after its fade
        let mut empty = GraphPlan::empty(BLOCK);
        hear(&mut player, &mut empty, 15, |_, _| {});
        assert!(!player.is_probing());
    }
}
