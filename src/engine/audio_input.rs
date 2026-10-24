//! Live audio input: carries a microphone, guitar or line source from the
//! input device's callback to the output callback.
//!
//! The two callbacks run on their own clocks, in their own buffer sizes, and
//! sometimes at their own sample rates. The input side's [`InputSender`]
//! pushes every frame it gets into an `rtrb` ring, converting it to the
//! output's rate first if the input device runs at another one. A frame
//! keeps every channel of the device, up to [`MAX_INPUT_CHANNELS`], so a
//! mixer's worth of instruments can each come back on their own (#450); a
//! mono device's one channel is carried as two, heard on both sides. The
//! output side's [`InputFeed`] is a small jitter buffer over that ring:
//!
//! - It waits until the ring holds a little more than the largest input
//!   buffer and the largest output buffer seen (the *target*) before it
//!   starts reading, so the reads ride out the callbacks' uneven timing.
//! - **Underrun:** when a read finds too little, the missing frames are
//!   silence, and the feed waits to refill to the target before reading
//!   again. Each underrun adds a little cushion to the target, so a device
//!   pair that keeps running dry settles at a latency that works.
//! - **Overflow:** when the ring holds well over the target (the input clock
//!   runs a little fast, or the output stalled), the oldest frames are
//!   dropped back down to it. Frames the ring had no room for are dropped
//!   on the input side.
//! - **Settling:** on separate clocks, the ring starts out holding at
//!   least the target, often up to a buffer more, depending on where the
//!   reads fell as it filled. The sender stamps each push, so before each
//!   callback the feed knows the *fill*: the level, plus the input built up
//!   since the last buffer arrived. Unlike the level, the fill doesn't jump
//!   with each buffer, so the margin it leaves just before the next buffer
//!   is due holds whatever the phase between the callbacks. Over a couple
//!   of seconds, the feed trims away the margin never needed beyond the
//!   cushion; when it dips toward empty instead (an input clock running a
//!   little slow, or timing rougher than before), it stretches back up to
//!   the cushion before it runs dry. Both go a frame at a time, a block
//!   playing 0.4% fast or slow so nothing clicks, or a trim goes all at
//!   once while the input is quiet. Clocks that drift apart are followed
//!   this way rather than running dry or ahead. After a long run without
//!   an underrun the cushion steps back down, so one glitch at start-up
//!   doesn't cost latency for the rest of the session; if that runs dry
//!   again, it waits twice as long before the next try.
//!
//! All of these are counted, in frames, for the status bar.
//!
//! **Same clock.** When one driver serves both sides (ASIO), the input and
//! output callbacks run in lockstep, one after the other at each buffer
//! switch, at one rate. Their timing never wanders, so the feed holds just
//! one input buffer and no cushion: see [`input_channel_same_clock`]. If
//! the input callback runs first, each output reads the input that arrived
//! moments before it, and the buffer adds nothing to the round trip.
//!
//! The feed reaches the
//! audio thread inside an [`AudioMessage`] and goes back to the UI thread to
//! be dropped, like a recording's tap, so the ring is never freed there.
//!
//! [`AudioMessage`]: super::commands::AudioMessage

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};
use web_time::Instant;

use super::latency::LatencyGauge;
use crate::dsp::finite::finite;
use crate::dsp::input_channels::MAX_INPUT_CHANNELS;
use crate::dsp::primitives::hermite;

/// The channels a ring carries for a device of `channels`: all of them, up
/// to [`MAX_INPUT_CHANNELS`], and never fewer than two.
pub fn ring_channels(channels: usize) -> usize {
    channels.clamp(2, MAX_INPUT_CHANNELS)
}

/// How much audio the ring can hold, in seconds. Far more than the jitter
/// buffer ever keeps: room for the output to stall without losing input.
pub const RING_SECONDS: f32 = 0.5;

/// Counters shared by both ends and the UI.
#[derive(Debug, Default)]
struct InputStats {
    /// Frames of silence read in place of input that hadn't arrived.
    underrun_frames: AtomicU64,
    /// Frames dropped: input with no room in the ring, or trimmed because
    /// the ring ran too far ahead.
    overflow_frames: AtomicU64,
    /// Frames trimmed as the feed settled to the latency it needs.
    settled_frames: AtomicU64,
    /// Frames stretched as the ring ran low, to keep it from running dry.
    stretched_frames: AtomicU64,
    /// When the input last pushed, in nanoseconds from the channel's epoch.
    pushed_at: AtomicU64,
    /// The largest buffer the input device has delivered, in frames at the
    /// output's rate.
    largest_input: AtomicUsize,
    /// The level the feed refills to, in frames.
    target_frames: AtomicUsize,
    /// The latency the feed adds, in frames: the average over the last
    /// window, or the target until a window has passed. On separate
    /// clocks, how long input waits from its buffer's arrival to its turn
    /// to play; on a shared one, the level less the input buffer that
    /// arrives just before each read when the input's callback runs first.
    added_frames: AtomicUsize,
    /// Set by the input stream's error callback (unplugged, say).
    failed: AtomicBool,
    /// Glitches the input device itself reported (it dropped audio before
    /// Soba saw it), which the stream recovers from.
    device_xruns: AtomicU64,
    /// The input device's own delay, from its buffers' timestamps.
    device_latency: LatencyGauge,
}

/// Converts a stream of frames from one sample rate to another, a frame at
/// a time, by 4-point cubic Hermite interpolation.
///
/// Clean for voice and guitar. Content right at the top of the band (above
/// the lower rate's Nyquist, when converting down) folds back faintly, as
/// there's no steep anti-aliasing filter.
#[derive(Clone, Debug)]
struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Where the next output frame falls: 0 at the second-oldest frame of
    /// `history`, 1 at the second-newest.
    position: f64,
    /// The last four input frames, in a ring: the newest at `newest`.
    history: [[f32; MAX_INPUT_CHANNELS]; 4],
    newest: usize,
    /// Channels in a frame.
    width: usize,
}

impl Resampler {
    fn new(input_rate: u32, output_rate: u32, width: usize) -> Self {
        Self {
            step: input_rate as f64 / output_rate.max(1) as f64,
            position: 1.0,
            history: [[0.0; MAX_INPUT_CHANNELS]; 4],
            newest: 3,
            width,
        }
    }

    /// The most output frames one input frame can complete.
    fn max_burst(&self) -> usize {
        (1.0 / self.step).ceil() as usize + 1
    }

    /// Takes in one input frame (its first `width` channels), handing each
    /// output frame it completes to `emit`.
    #[inline]
    fn push(&mut self, frame: &[f32; MAX_INPUT_CHANNELS], mut emit: impl FnMut(&[f32])) {
        let width = self.width;
        self.newest = (self.newest + 1) % 4;
        self.history[self.newest][..width].copy_from_slice(&frame[..width]);
        self.position -= 1.0;
        // Oldest first
        let [x0, x1, x2, x3] = [1, 2, 3, 0].map(|back| &self.history[(self.newest + back) % 4]);
        let mut out = [0.0; MAX_INPUT_CHANNELS];
        while self.position < 1.0 {
            let t = self.position.max(0.0) as f32;
            for (c, out) in out[..width].iter_mut().enumerate() {
                *out = hermite(x0[c], x1[c], x2[c], x3[c], t);
            }
            emit(&out[..width]);
            self.position += self.step;
        }
    }
}

/// Writes interleaved frames of `width` channels into the ring, as many as
/// fit, counting the rest as dropped. Returns the frames written.
fn write_frames(producer: &mut Producer<f32>, stats: &InputStats, samples: &[f32], width: usize) -> usize {
    let frames = samples.len() / width;
    let fit = (producer.slots() / width).min(frames);
    if fit > 0 {
        if let Ok(chunk) = producer.write_chunk_uninit(fit * width) {
            chunk.fill_from_iter(samples[..fit * width].iter().copied());
        }
    }
    if fit < frames {
        stats.overflow_frames.fetch_add((frames - fit) as u64, Ordering::Relaxed);
    }
    fit
}

/// The input callback's end: pushes what the device records.
pub struct InputSender {
    producer: Producer<f32>,
    stats: Arc<InputStats>,
    /// Converts the device's rate to the output's, when they differ.
    resampler: Option<Resampler>,
    /// When the channel opened, shared with the feed.
    epoch: Instant,
    /// Channels in each frame of the ring.
    width: usize,
}

impl InputSender {
    /// Pushes one callback's interleaved input of `channels` channels: each
    /// channel the ring carries (a mono device's one on both of its two,
    /// and no more than [`MAX_INPUT_CHANNELS`] of a bigger interface).
    /// Frames that don't fit are counted.
    ///
    /// REAL-TIME SAFE: no allocation, locking or waiting.
    pub fn push<T: Copy>(&mut self, data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) {
        if channels == 0 {
            return;
        }
        self.write(data, channels, to_f32);
        self.stamp(self.epoch.elapsed());
    }

    /// Notes that the input pushed at `at` from the epoch, for the feed to
    /// tell how long ago the last buffer arrived.
    fn stamp(&self, at: Duration) {
        self.stats.pushed_at.store(at.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Writes one callback's input into the ring: see [`push`](Self::push).
    fn write<T: Copy>(&mut self, data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) {
        let width = self.width;
        // Channel `c` of the ring's frame: a mono device's one on both
        // sides, and silence for one a device leaves out. A driver's NaN or
        // wild value goes no further than here
        let sample = |frame: &[T], c: usize| {
            let c = if channels == 1 { 0 } else { c };
            frame.get(c).map_or(0.0, |&s| finite(to_f32(s)))
        };
        let Self { producer, stats, resampler, .. } = self;

        let Some(resampler) = resampler.as_mut() else {
            let frames = data.len() / channels;
            stats.largest_input.fetch_max(frames, Ordering::Relaxed);
            let fit = (producer.slots() / width).min(frames);
            if fit > 0 {
                if let Ok(chunk) = producer.write_chunk_uninit(fit * width) {
                    let sample = &sample;
                    let frames = data.chunks_exact(channels).take(fit);
                    chunk.fill_from_iter(frames.flat_map(|frame| (0..width).map(move |c| sample(frame, c))));
                }
            }
            if fit < frames {
                stats.overflow_frames.fetch_add((frames - fit) as u64, Ordering::Relaxed);
            }
            return;
        };

        // Converted a few hundred frames at a time through the stack
        let mut converted = [0.0f32; 2048];
        let mut len = 0;
        let mut produced = 0;
        let headroom = width * resampler.max_burst();
        let mut taken = [0.0; MAX_INPUT_CHANNELS];
        for frame in data.chunks_exact(channels) {
            for (c, slot) in taken[..width].iter_mut().enumerate() {
                *slot = sample(frame, c);
            }
            resampler.push(&taken, |out| {
                converted[len..len + width].copy_from_slice(out);
                len += width;
            });
            if len + headroom > converted.len() {
                write_frames(producer, stats, &converted[..len], width);
                produced += len / width;
                len = 0;
            }
        }
        write_frames(producer, stats, &converted[..len], width);
        produced += len / width;
        stats.largest_input.fetch_max(produced, Ordering::Relaxed);
    }

    /// Pushes interleaved `f32` input.
    pub fn push_f32(&mut self, data: &[f32], channels: usize) {
        self.push(data, channels, |s| s);
    }

    /// Takes in one callback's input device delay: from its first frame's
    /// capture to the callback, or `None` if the timestamps ran backwards.
    ///
    /// REAL-TIME SAFE: see [`LatencyGauge::record`].
    #[inline]
    pub fn record_latency(&self, delay: Option<Duration>) {
        self.stats.device_latency.record(delay);
    }
}

/// The output callback's end: a jitter buffer that hands out input a block
/// at a time.
pub struct InputFeed {
    consumer: Consumer<f32>,
    stats: Arc<InputStats>,
    /// Whether the ring has filled to the target since it last ran dry.
    primed: bool,
    /// Extra frames kept on top of one input and one output buffer, grown
    /// by each underrun and relaxed after a long calm.
    cushion: usize,
    /// The largest buffer the output has asked for, in frames. Some drivers
    /// vary their buffer size from one callback to the next.
    largest_output: usize,
    /// How the two callbacks are clocked.
    clock: Clock,
    /// Output frames a second.
    rate: usize,
    /// When the channel opened, shared with the sender.
    epoch: Instant,
    /// Frames the current callback has still to read.
    owed: usize,
    /// How low the ring has run in the current window.
    watch: Watch,
    /// The latency the feed added on average over the last whole window,
    /// once one has passed since the feed primed.
    measured: Option<usize>,
    /// Frames still to take in beyond those handed out (a trim, positive)
    /// or to hold back (a stretch, negative).
    plan: isize,
    /// Frames read toward the next frame trimmed or stretched.
    slew: usize,
    /// The last frame taken from the ring, which a squeezed or stretched
    /// read curves on from.
    last: [f32; MAX_INPUT_CHANNELS],
    /// Channels in each frame of the ring.
    width: usize,
    /// Whether the last frame heard was quiet enough to cut after.
    quiet: bool,
    /// Frames read since the last underrun or relaxed cushion.
    calm: usize,
    /// How many calm frames relax the cushion a step.
    relax_after: usize,
    /// Whether the cushion was relaxed since the last underrun, so running
    /// dry now means it was relaxed too soon.
    relaxed: bool,
}

/// The ring's low-water mark over a window of callbacks.
#[derive(Clone, Copy, Debug)]
struct Watch {
    /// Output frames into the window.
    frames: usize,
    /// The latency the feed added, summed over the callbacks.
    added: usize,
    /// Callbacks into the window.
    callbacks: usize,
    /// The fewest frames a callback would have left, had the next input
    /// buffer been just about to arrive: the margin that keeps it from
    /// running dry whatever the phase between the callbacks.
    margin: usize,
    /// Whether a trim or stretch was under way, which leaves the margin out
    /// of date.
    settling: bool,
}

impl Watch {
    const NEW: Self = Self { frames: 0, added: 0, callbacks: 0, margin: usize::MAX, settling: false };
}

/// How an input's callbacks are clocked against the output's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clock {
    /// Separate devices, each on its own clock.
    Separate,
    /// One driver, both callbacks at each buffer switch; `input_first` when
    /// the input's runs before the output's.
    Shared { input_first: bool },
}

impl InputFeed {
    /// The fewest frames of cushion, for a little timing jitter.
    const MIN_CUSHION: usize = 32;
    /// The cushion never grows past this, about 20 ms at 48 kHz.
    const MAX_CUSHION: usize = 1024;
    /// How much each underrun adds to the cushion, and each calm stretch
    /// takes away.
    const CUSHION_STEP: usize = 64;
    /// How long the feed watches the ring's low-water mark before trimming.
    const WINDOW_SECONDS: usize = 2;
    /// The fewest unneeded frames worth trimming, so trims and stretches
    /// don't chase each other a few frames at a time.
    const TRIM_MIN: usize = 16;
    /// Frames read for each frame trimmed or stretched: the input plays
    /// about 0.4% fast or slow, 7 cents, while the feed settles.
    const SLEW: usize = 256;
    /// Below this (-60 dBFS) a cut is inaudible, so a trim goes all at once.
    const QUIET: f32 = 0.001;
    /// How long without an underrun before the cushion relaxes a step.
    const RELAX_SECONDS: usize = 30;
    /// The longest the relaxing backs off to, after relaxed cushions ran dry.
    const MAX_RELAX_SECONDS: usize = 240;

    /// The level reads start from: the largest input buffer, the largest
    /// output buffer, and the cushion. On a shared clock, the input buffer
    /// and the cushion: each read finds exactly one buffer waiting.
    fn target(&self) -> usize {
        let input = self.stats.largest_input.load(Ordering::Relaxed);
        match self.clock {
            Clock::Separate => input + self.largest_output + self.cushion,
            Clock::Shared { .. } => input + self.cushion,
        }
    }

    /// The latency the feed adds when the ring holds `level` before a
    /// callback on a shared clock, or at the target on any: all of it,
    /// unless the input arrives just before each read.
    fn added(&self, level: usize) -> usize {
        match self.clock {
            Clock::Shared { input_first: true } => level.saturating_sub(self.stats.largest_input.load(Ordering::Relaxed)),
            _ => level,
        }
    }

    /// Channels in each frame it reads: every channel of the device, up to
    /// [`MAX_INPUT_CHANNELS`], and at least two.
    pub fn channels(&self) -> usize {
        self.width
    }

    /// Frames waiting in the ring.
    fn level(&self) -> usize {
        self.consumer.slots() / self.width
    }

    /// Drops the oldest `frames` frames.
    fn drop_frames(&mut self, frames: usize) {
        if let Ok(chunk) = self.consumer.read_chunk(frames * self.width) {
            chunk.commit_all();
        }
    }

    /// Starts watching afresh, for a level that has just been set: by
    /// priming, or by a drop back to the target.
    fn reset_watch(&mut self) {
        self.watch = Watch::NEW;
        self.measured = None;
        self.plan = 0;
        self.slew = 0;
    }

    /// Call at the start of an output callback that will read
    /// `output_frames` frames: waits until the ring has filled to the
    /// target, trims it back to the target if it has run well ahead, and
    /// watches how low it runs.
    ///
    /// REAL-TIME SAFE.
    pub fn begin(&mut self, output_frames: usize) {
        self.begin_at(output_frames, self.epoch.elapsed());
    }

    /// [`begin`](Self::begin), `now` from the epoch.
    fn begin_at(&mut self, output_frames: usize, now: Duration) {
        self.largest_output = self.largest_output.max(output_frames);
        self.owed = output_frames;
        let target = self.target();
        self.stats.target_frames.store(target, Ordering::Relaxed);
        let mut level = self.level();
        if !self.primed {
            self.stats.added_frames.store(self.added(target), Ordering::Relaxed);
            // Nothing to start from yet: on a shared clock, before the first
            // input buffer, the target is still zero
            if level < target || level == 0 {
                return;
            }
            self.primed = true;
            self.reset_watch();
        }
        // The level naturally swings between about target - output and
        // target + input. Past that, with a step to spare so a relaxed
        // cushion doesn't tip it over, the input is running ahead
        let input = self.stats.largest_input.load(Ordering::Relaxed);
        let ceiling = target + input + self.largest_output + Self::CUSHION_STEP;
        if level > ceiling {
            let excess = level - target;
            self.drop_frames(excess);
            self.stats.overflow_frames.fetch_add(excess as u64, Ordering::Relaxed);
            self.reset_watch();
            level = target;
        }
        match self.clock {
            Clock::Separate => {
                // The input that has built up since the last buffer arrived,
                // still on its way: with it, the fill grows smoothly between
                // buffers rather than in steps, whatever the phase
                let since = now.saturating_sub(Duration::from_nanos(self.stats.pushed_at.load(Ordering::Relaxed)));
                let coming = ((since.as_secs_f64() * self.rate as f64) as usize).min(input);
                self.settle_toward(level + coming, input, output_frames);
            }
            Clock::Shared { .. } => {
                self.measure(self.added(level), level.saturating_sub(output_frames));
            }
        }
        self.stats.added_frames.store(self.measured.unwrap_or(self.added(target)), Ordering::Relaxed);
    }

    /// Takes in the fill before a separate-clock callback that reads
    /// `output_frames`: the level, and the input on its way. Just before an
    /// `input`-frame buffer arrives, the level is the fill less the buffer,
    /// so that, less the read, is the margin whatever the phase between the
    /// callbacks. When it dips toward empty, plans a stretch back up to the
    /// cushion; at the end of a window in which it never fell to the
    /// cushion, plans a trim down to it. After a long calm, relaxes the
    /// cushion a step.
    ///
    /// A shared clock doesn't settle: it already holds the least it can,
    /// and its cushion only helps in whole buffers, for a driver that swaps
    /// the callbacks' order.
    fn settle_toward(&mut self, fill: usize, input: usize, output_frames: usize) {
        let added = fill.saturating_sub(input);
        let margin = added.saturating_sub(output_frames);
        if margin < self.cushion / 2 && self.plan >= 0 {
            self.plan = -((self.cushion - margin) as isize);
        }
        if let Some(Watch { margin, settling, .. }) = self.measure(added, margin) {
            if !settling && margin >= self.cushion + Self::TRIM_MIN {
                self.plan = (margin - self.cushion) as isize;
            }
        }

        self.calm += output_frames;
        if self.calm >= self.relax_after && self.cushion > Self::MIN_CUSHION {
            self.cushion = (self.cushion - Self::CUSHION_STEP).max(Self::MIN_CUSHION);
            self.calm = 0;
            self.relaxed = true;
        }
    }

    /// Adds a callback's latency and margin to the window, returning the
    /// window if that ended it.
    fn measure(&mut self, added: usize, margin: usize) -> Option<Watch> {
        let watch = &mut self.watch;
        watch.frames += self.owed;
        watch.added += added;
        watch.callbacks += 1;
        watch.margin = watch.margin.min(margin);
        watch.settling |= self.plan != 0;
        if watch.frames < Self::WINDOW_SECONDS * self.rate {
            return None;
        }
        let ended = *watch;
        self.watch = Watch::NEW;
        self.measured = Some(ended.added / ended.callbacks);
        Some(ended)
    }

    /// Reads the next frames into `out`, interleaved, [`channels`] to a
    /// frame. Until the buffer has filled, or past the input it holds, the
    /// frames are silence; running dry counts as an underrun and waits for
    /// the buffer to refill. While a trim or stretch is planned, a read
    /// takes in a frame more or less than it hands out, or a trim cuts at
    /// once if the input is quiet.
    ///
    /// REAL-TIME SAFE.
    ///
    /// [`channels`]: Self::channels
    pub fn read(&mut self, out: &mut [f32]) {
        let width = self.width;
        let wanted = out.len() / width;
        let mut got = 0;
        if self.primed {
            let nudge = self.settle(wanted);
            if nudge != 0 {
                // Taken a little faster or slower, looking two frames on
                got = wanted;
                let taken = wanted.saturating_add_signed(nudge);
                if let Ok(chunk) = self.consumer.read_chunk((taken + 2) * width) {
                    let (first, second) = chunk.as_slices();
                    resample(first, second, &self.last[..width], &mut out[..wanted * width], width, taken);
                    let at = |i: usize| if i < first.len() { first[i] } else { second[i - first.len()] };
                    for (c, last) in self.last[..width].iter_mut().enumerate() {
                        *last = at((taken - 1) * width + c);
                    }
                    chunk.commit(taken * width);
                }
            } else {
                got = self.level().min(wanted);
                if let Ok(chunk) = self.consumer.read_chunk(got * width) {
                    let (first, second) = chunk.as_slices();
                    out[..first.len()].copy_from_slice(first);
                    out[first.len()..first.len() + second.len()].copy_from_slice(second);
                    chunk.commit_all();
                }
                if got > 0 {
                    self.last[..width].copy_from_slice(&out[(got - 1) * width..got * width]);
                }
            }
            if got > 0 {
                self.quiet = out[(got - 1) * width..got * width].iter().all(|s| s.abs() < Self::QUIET);
            }
            self.owed = self.owed.saturating_sub(wanted);
            if got < wanted {
                self.underrun(wanted - got);
            }
        }
        out[got * width..wanted * width].fill(0.0);
    }

    /// Runs dry: counts the silence, grows the cushion, and waits to refill.
    fn underrun(&mut self, frames: usize) {
        self.stats.underrun_frames.fetch_add(frames as u64, Ordering::Relaxed);
        self.primed = false;
        self.last = [0.0; MAX_INPUT_CHANNELS];
        self.quiet = true;
        self.cushion = (self.cushion + Self::CUSHION_STEP).min(Self::MAX_CUSHION);
        self.calm = 0;
        if self.relaxed {
            // The relaxed cushion wasn't enough: wait longer next time
            self.relax_after = (self.relax_after * 2).min(Self::MAX_RELAX_SECONDS * self.rate);
            self.relaxed = false;
        }
    }

    /// Carries out the plan for a read of `wanted` frames. Returns how many
    /// frames more (a trim) or fewer (a stretch) than `wanted` this read
    /// should take in, at most one in 64 and averaging one in [`SLEW`]; or
    /// cuts the whole trim at once if the input is quiet either side.
    ///
    /// [`SLEW`]: Self::SLEW
    fn settle(&mut self, wanted: usize) -> isize {
        if self.plan == 0 || wanted < 64 {
            return 0;
        }
        let level = self.level();
        self.slew += wanted;
        let most = (self.slew / Self::SLEW).min(wanted / 64);
        let nudge = if self.plan > 0 {
            // Never into the cushion the rest of the callback leaves, and
            // with two frames to look ahead to
            let needed = self.owed.max(wanted) + self.cushion + 2;
            let spare = level.saturating_sub(needed).min(self.plan as usize);
            if spare > 0 && self.quiet && self.quiet_through(spare + 1) {
                self.drop_frames(spare);
                self.plan -= spare as isize;
                self.stats.settled_frames.fetch_add(spare as u64, Ordering::Relaxed);
                0
            } else {
                let skip = most.min(spare);
                self.stats.settled_frames.fetch_add(skip as u64, Ordering::Relaxed);
                skip as isize
            }
        } else {
            let short = most.min(self.plan.unsigned_abs());
            // The frames it takes, and two to look ahead to
            if level + short < wanted + 2 {
                0
            } else {
                self.stats.stretched_frames.fetch_add(short as u64, Ordering::Relaxed);
                -(short as isize)
            }
        };
        self.plan -= nudge;
        self.slew = (self.slew - nudge.unsigned_abs() * Self::SLEW).min(Self::SLEW);
        nudge
    }

    /// Whether the oldest `frames` frames waiting are all quiet.
    fn quiet_through(&mut self, frames: usize) -> bool {
        let Ok(chunk) = self.consumer.read_chunk(frames * self.width) else {
            return false;
        };
        let (first, second) = chunk.as_slices();
        first.iter().chain(second).all(|s| s.abs() < Self::QUIET)
    }

    /// Drops everything waiting, for a callback that won't read (the
    /// transport is stopped), and waits to refill before reading again.
    ///
    /// REAL-TIME SAFE.
    pub fn discard(&mut self) {
        let level = self.level();
        self.drop_frames(level);
        self.primed = false;
        self.last = [0.0; MAX_INPUT_CHANNELS];
        self.quiet = true;
    }
}

/// Plays `taken` of the interleaved frames of `width` channels in `first`
/// then `second` into `out`, in the time of its frames: a little fast or
/// slow, read between frames along the Hermite curve. `before` is the frame
/// before the first. The first frame lands exactly, and the curve looks up
/// to two frames past the last one taken, which the next read starts from.
fn resample(first: &[f32], second: &[f32], before: &[f32], out: &mut [f32], width: usize, taken: usize) {
    let sample = |i: usize| if i < first.len() { first[i] } else { second[i - first.len()] };
    let frames = out.len() / width;
    for i in 0..frames {
        // Frame i lands at i * taken / frames: frame k, and t of the way on
        // to the next
        let at = i * taken;
        let (k, t) = (at / frames, (at % frames) as f32 / frames as f32);
        let x = |n: usize, c: usize| if n == 0 { before[c] } else { sample(width * (n - 1) + c) };
        // x(n) is frame n - 1, so x(k + 1) is frame k
        for (c, out) in out[i * width..(i + 1) * width].iter_mut().enumerate() {
            *out = if t == 0.0 { x(k + 1, c) } else { hermite(x(k, c), x(k + 1, c), x(k + 2, c), x(k + 3, c), t) };
        }
    }
}

/// The UI's view of an open input: its counters and health.
#[derive(Clone)]
pub struct InputMonitor {
    stats: Arc<InputStats>,
}

impl InputMonitor {
    /// Frames of silence played in place of late input, so far.
    pub fn underrun_frames(&self) -> u64 {
        self.stats.underrun_frames.load(Ordering::Relaxed)
    }

    /// Frames of input dropped, so far.
    pub fn overflow_frames(&self) -> u64 {
        self.stats.overflow_frames.load(Ordering::Relaxed)
    }

    /// Frames trimmed as the jitter buffer settled to the latency it
    /// needs, so far: unneeded margin, not dropouts.
    pub fn settled_frames(&self) -> u64 {
        self.stats.settled_frames.load(Ordering::Relaxed)
    }

    /// Frames the jitter buffer has stretched (played a little slow) as it
    /// ran low, so far: a dropout avoided, or a slow input clock kept up
    /// with.
    pub fn stretched_frames(&self) -> u64 {
        self.stats.stretched_frames.load(Ordering::Relaxed)
    }

    /// The largest packet the input device has delivered, in frames at the
    /// output's rate.
    pub fn packet_frames(&self) -> usize {
        self.stats.largest_input.load(Ordering::Relaxed)
    }

    /// The latency the jitter buffer adds on top of the devices' own, in
    /// frames: the lowest it has run over the last couple of seconds, or
    /// the level it refills to before then.
    pub fn buffered_frames(&self) -> usize {
        self.stats.added_frames.load(Ordering::Relaxed)
    }

    /// The level the jitter buffer refills to, in frames. More than
    /// [`buffered_frames`](Self::buffered_frames) when one driver delivers
    /// each input buffer just before the output reads it.
    pub fn target_frames(&self) -> usize {
        self.stats.target_frames.load(Ordering::Relaxed)
    }

    /// The input device's own delay, from capture to callback, or `None` if
    /// it doesn't report timestamps.
    pub fn device_latency(&self) -> Option<Duration> {
        self.stats.device_latency.get()
    }

    /// Whether the input stream has reported an error.
    pub fn failed(&self) -> bool {
        self.stats.failed.load(Ordering::Relaxed)
    }

    /// Marks the input as failed, from the stream's error callback.
    pub fn mark_failed(&self) {
        self.stats.failed.store(true, Ordering::Relaxed);
    }

    /// Glitches the input device has reported, so far.
    pub fn device_xruns(&self) -> u64 {
        self.stats.device_xruns.load(Ordering::Relaxed)
    }

    /// Counts a glitch the device reported, from the stream's error
    /// callback. The stream carries on.
    ///
    /// REAL-TIME SAFE: an atomic add.
    pub fn mark_xrun(&self) {
        self.stats.device_xruns.fetch_add(1, Ordering::Relaxed);
    }
}

/// Creates the three ends of an input whose stereo device runs at the
/// output's `sample_rate`.
pub fn input_channel(sample_rate: u32) -> (InputSender, InputFeed, InputMonitor) {
    input_channel_converting(sample_rate, sample_rate, 2)
}

/// Creates the three ends of an input whose device of `channels` channels
/// runs at `input_rate`, converted to the output's `output_rate` if they
/// differ.
pub fn input_channel_converting(input_rate: u32, output_rate: u32, channels: usize) -> (InputSender, InputFeed, InputMonitor) {
    channel(input_rate, output_rate, Clock::Separate, channels)
}

/// Creates the three ends of an input of `channels` channels served by the
/// output's own driver, on its clock and at its `sample_rate`: the feed
/// holds one input buffer and no cushion. `input_first` when the driver
/// runs the input's callback before the output's at each buffer switch, so
/// each read takes the input that arrived moments before.
pub fn input_channel_same_clock(sample_rate: u32, input_first: bool, channels: usize) -> (InputSender, InputFeed, InputMonitor) {
    channel(sample_rate, sample_rate, Clock::Shared { input_first }, channels)
}

fn channel(input_rate: u32, output_rate: u32, clock: Clock, channels: usize) -> (InputSender, InputFeed, InputMonitor) {
    let width = ring_channels(channels);
    let frames = ((output_rate as f32 * RING_SECONDS) as usize).max(1024);
    let (producer, consumer) = RingBuffer::new(frames * width);
    let stats = Arc::new(InputStats::default());
    let epoch = Instant::now();
    let resampler = (input_rate != output_rate && input_rate > 0).then(|| Resampler::new(input_rate, output_rate, width));
    (
        InputSender { producer, stats: Arc::clone(&stats), resampler, epoch, width },
        InputFeed {
            consumer,
            stats: Arc::clone(&stats),
            primed: false,
            cushion: match clock {
                Clock::Separate => InputFeed::MIN_CUSHION,
                Clock::Shared { .. } => 0,
            },
            largest_output: 0,
            clock,
            rate: output_rate.max(1) as usize,
            epoch,
            owed: 0,
            watch: Watch::NEW,
            measured: None,
            plan: 0,
            slew: 0,
            last: [0.0; MAX_INPUT_CHANNELS],
            width,
            quiet: true,
            calm: 0,
            relax_after: InputFeed::RELAX_SECONDS * output_rate.max(1) as usize,
            relaxed: false,
        },
        InputMonitor { stats },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Interleaved stereo frames counting up from `start`: left n, right -n.
    fn ramp(start: usize, frames: usize) -> Vec<f32> {
        (start..start + frames).flat_map(|n| [n as f32, -(n as f32)]).collect()
    }

    /// Reads a stereo feed's next `left.len()` frames into `left` and `right`.
    fn read_stereo(feed: &mut InputFeed, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(feed.channels(), 2);
        let mut out = vec![0.0; left.len() * 2];
        feed.read(&mut out);
        for (i, frame) in out.as_chunks::<2>().0.iter().enumerate() {
            left[i] = frame[0];
            right[i] = frame[1];
        }
    }

    /// Interleaved frames of `channels` channels, counting up from `start`:
    /// channel c of frame n is `n + 1000 c`, so each channel is told apart.
    fn wide_ramp(start: usize, frames: usize, channels: usize) -> Vec<f32> {
        (start..start + frames).flat_map(|n| (0..channels).map(move |c| (n + 1000 * c) as f32)).collect()
    }

    #[test]
    fn test_a_twelve_channel_device_keeps_every_channel_apart() {
        assert_eq!(input_channel(48000).1.channels(), 2, "a stereo device's ring is as it was");
        let (mut sender, mut feed, _) = input_channel_converting(48000, 48000, 12);
        assert_eq!(feed.channels(), 12);
        for chunk in 0..4 {
            sender.push_f32(&wide_ramp(chunk * 128, 128, 12), 12);
        }
        feed.begin(128);
        let mut out = vec![0.0; 128 * 12];
        feed.read(&mut out);
        for (n, frame) in out.as_chunks::<12>().0.iter().enumerate() {
            // Channel 7 (from 1) is c = 6, all the way through
            assert_eq!(frame[6], (n + 6000) as f32);
            assert_eq!(frame, &wide_ramp(n, 1, 12)[..]);
        }
    }

    #[test]
    fn test_an_interface_bigger_than_the_most_keeps_the_first_sixteen() {
        let (mut sender, mut feed, _) = input_channel_converting(48000, 48000, 24);
        assert_eq!(feed.channels(), MAX_INPUT_CHANNELS);
        for chunk in 0..4 {
            sender.push_f32(&wide_ramp(chunk * 128, 128, 24), 24);
        }
        feed.begin(128);
        let mut out = vec![0.0; 128 * MAX_INPUT_CHANNELS];
        feed.read(&mut out);
        assert_eq!(&out[..MAX_INPUT_CHANNELS], &wide_ramp(0, 1, MAX_INPUT_CHANNELS)[..]);
    }

    #[test]
    fn test_wide_input_converts_between_rates_channel_by_channel() {
        // Each of eight channels a 1 kHz sine at its own level, 48 -> 44.1 kHz
        let tone = sine(48000, 0.5);
        let wide: Vec<f32> = tone.iter().flat_map(|&s| (1..=8).map(move |c| s * c as f32 / 8.0)).collect();
        let (mut sender, mut feed, _) = input_channel_converting(48000, 44100, 8);
        let stereo = convert(&tone, 48000, 44100, 480);
        let mut heard = Vec::new();
        for piece in wide.chunks(480 * 8) {
            sender.push_f32(piece, 8);
            let mut out = vec![0.0; feed.level() * 8];
            feed.primed = true;
            feed.read(&mut out);
            heard.extend_from_slice(&out);
        }
        assert_eq!(heard.len() / 8, stereo.len());
        for (frame, &expected) in heard.as_chunks::<8>().0.iter().zip(&stereo) {
            for (c, &s) in frame.iter().enumerate() {
                assert!((s - expected * (c + 1) as f32 / 8.0).abs() < 1e-6, "channel {c}: {s} against {expected}");
            }
        }
    }

    #[test]
    fn test_wide_input_settles_without_mixing_its_channels() {
        // Six channels, jittery 480-frame packets against 256-frame reads
        // with a fast input clock: the feed trims and squeezes, and each
        // channel's ramp runs on in step with the first
        let (mut sender, mut feed, monitor) = input_channel_converting(48000, 48000, 6);
        let mut out = vec![0.0; 256 * 6];
        let mut pushed = 0;
        let mut heard = Vec::new();
        for read in 0..(20 * 48000 / 256) {
            let now = read * 256;
            while (pushed + 480) as f64 / 1.002 <= now as f64 {
                sender.push_f32(&wide_ramp(pushed + 1, 480, 6), 6);
                pushed += 480;
                sender.stamp(Duration::from_secs_f64(pushed as f64 / 1.002 / 48000.0));
            }
            feed.begin_at(256, Duration::from_secs_f64(now as f64 / 48000.0));
            feed.read(&mut out);
            heard.extend_from_slice(&out);
        }
        assert!(monitor.settled_frames() > 0, "it followed the fast clock");
        let start = heard.as_chunks::<6>().0.iter().position(|f| f[0] != 0.0).expect("input was heard");
        assert!(heard.as_chunks::<6>().0.iter().skip(start).map(|f| f[0]).collect::<Vec<_>>().windows(2).all(|w| w[1] > w[0]), "never backwards");
        for frame in heard.as_chunks::<6>().0.iter().skip(start) {
            for (c, &s) in frame.iter().enumerate() {
                // Within f32's rounding of a million or so
                assert!((s - frame[0] - 1000.0 * c as f32).abs() < 0.5, "channel {c} drifted from the first: {frame:?}");
            }
        }
    }

    #[test]
    fn test_a_mono_device_fills_both_sides_of_a_wide_ring() {
        // A mono device pushed into a ring opened as one channel is two
        let (mut sender, mut feed, _) = input_channel_converting(48000, 48000, 1);
        assert_eq!(feed.channels(), 2);
        for _ in 0..4 {
            sender.push_f32(&[0.5; 128], 1);
        }
        feed.begin(128);
        let mut out = vec![0.0; 256];
        feed.read(&mut out);
        assert!(out.iter().all(|&s| s == 0.5));
    }

    /// Runs `seconds` of a 48 kHz input pushing `input` frames at a time
    /// against an output reading `output` frames at a time. The input clock
    /// runs `drift` times as fast as the output's. Returns everything read
    /// (left side), and the monitor.
    fn run(input: usize, output: usize, drift: f64, seconds: f64) -> (Vec<f32>, InputMonitor) {
        let ran = run_jittered(input, output, drift, seconds, 0.0);
        (ran.heard, ran.monitor)
    }

    /// What a simulated run heard.
    struct Ran {
        /// Everything read, left side. Frame `i` of it plays at the output's
        /// frame `i`, and the input's frame `n` was captured at frame `n`.
        heard: Vec<f32>,
        monitor: InputMonitor,
        /// Frames of silence in place of input over the second half.
        late_underruns: u64,
    }

    /// [`run`], with each callback running up to `jitter` seconds early or
    /// late, as a busy system's do, though never ahead of the one before.
    /// Each input buffer is pushed as its first frame is due, so frame `n`
    /// is heard `i - n` frames after its buffer arrived.
    fn run_jittered(input: usize, output: usize, drift: f64, seconds: f64, jitter: f64) -> Ran {
        // Time is in output frames, exact while nothing wobbles
        let jitter = jitter * 48000.0;
        let (mut sender, mut feed, monitor) = input_channel(48000);
        // A small xorshift: the same jitter every run
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64 ^ (input * 7919 + output) as u64;
        let mut wobble = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            jitter * ((seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0)
        };
        let mut pushed = 0;
        let mut heard = Vec::new();
        let (mut left, mut right) = (vec![0.0; output], vec![0.0; output]);
        let total_reads = (seconds * 48000.0 / output as f64) as usize;
        let input_period = input as f64 / drift;
        let mut next_input = wobble();
        let mut now = 0.0f64;
        let mut late_underruns = 0;
        for read in 0..total_reads {
            if read == total_reads / 2 {
                late_underruns = monitor.underrun_frames();
            }
            now = now.max((read * output) as f64 + wobble());
            let at = |frames: f64| Duration::from_secs_f64(frames.max(0.0) / 48000.0);
            while next_input <= now {
                sender.push_f32(&ramp(pushed, input), 2);
                sender.stamp(at(next_input));
                pushed += input;
                let due = (pushed / input) as f64 * input_period;
                next_input = next_input.max(due + wobble());
            }
            feed.begin_at(output, at(now));
            read_stereo(&mut feed, &mut left, &mut right);
            heard.extend_from_slice(&left);
        }
        let late_underruns = monitor.underrun_frames() - late_underruns;
        Ran { heard, monitor, late_underruns }
    }

    /// The first frame that isn't silence, and that the ramp runs on from
    /// there to the end: see [`assert_smooth`]. Returns the start, and how
    /// many steps were squeezed.
    fn assert_unbroken(heard: &[f32]) -> (usize, usize) {
        let start = heard.iter().position(|&s| s != 0.0).expect("input was heard");
        // The first frame pushed is 0, which reads as silence
        assert_eq!(heard[start], 1.0, "reads start at the first frame");
        (start, assert_smooth(&heard[start..]))
    }

    /// That the ramp runs on, never backwards, at most a frame and a little
    /// at a time: a frame now and then squeezed into a block as the feed
    /// settles, but nothing skipped whole. Returns how many steps were
    /// squeezed.
    fn assert_smooth(heard: &[f32]) -> usize {
        let mut squeezed = 0;
        for (i, pair) in heard.windows(2).enumerate() {
            let step = pair[1] - pair[0];
            // Far enough inside f32's integers for a tenth of a frame
            assert!((0.9..1.1).contains(&step), "the ramp stepped {step} at frame {i}");
            squeezed += (step != 1.0) as usize;
        }
        squeezed
    }

    /// The latency the feed added at the end of a run of [`run_jittered`]:
    /// how long the last frame heard waited from its input buffer's arrival
    /// to its turn to play.
    fn added_latency(heard: &[f32]) -> f64 {
        let last = heard.len() - 1;
        last as f64 - heard[last] as f64
    }

    #[test]
    fn test_steady_state_passes_every_frame_in_order() {
        // WASAPI-like input buffers against odd output buffers
        for (input, output) in [(480, 256), (441, 512), (128, 1024), (1024, 64)] {
            let (heard, monitor) = run(input, output, 1.0, 10.0);
            let (start, _) = assert_unbroken(&heard);
            assert_eq!(monitor.underrun_frames(), 0, "{input}/{output} ran dry");
            assert_eq!(monitor.overflow_frames(), 0, "{input}/{output} dropped input");
            // It waited for about one of each buffer before starting
            assert!(start <= input + output + 2 * output, "{input}/{output} started after {start} frames");
            assert!(monitor.buffered_frames() <= input + output + InputFeed::MIN_CUSHION);
        }
    }

    #[test]
    fn test_jittery_callbacks_settle_lower_without_running_dry() {
        // Each callback up to 1 ms early or late, on both sides
        for (input, output) in [(480, 256), (441, 512), (128, 1024), (1024, 64)] {
            let ran = run_jittered(input, output, 1.0, 20.0, 0.001);
            let squeezed = assert_smooth(&ran.heard[ran.heard.len() / 2..]);
            assert_eq!(ran.late_underruns, 0, "{input}/{output} still ran dry once settled");
            assert_eq!(ran.monitor.overflow_frames(), 0, "{input}/{output} dropped input");

            // Lower than the bound it used to hold, measured and reported
            let bound = (input + output + InputFeed::MIN_CUSHION) as f64;
            let added = added_latency(&ran.heard);
            let reported = ran.monitor.buffered_frames() as f64;
            println!(
                "{input}/{output}: added {added:.0} frames ({:.1} ms), reported {reported:.0}, bound {bound:.0}; \
                 settled {} frames, {squeezed} squeezed steps in the second half, {} frames of underrun first",
                added / 48.0,
                ran.monitor.settled_frames(),
                ran.monitor.underrun_frames(),
            );
            assert!(added < bound, "{input}/{output}: added {added} frames");
            assert!(reported < bound, "{input}/{output}: reported {reported} frames");
            // The status bar tells it within a millisecond
            assert!((reported - added).abs() <= 48.0, "{input}/{output}: reported {reported}, added {added}");
            assert!(ran.monitor.settled_frames() > 0, "{input}/{output} never trimmed");
        }
    }

    #[test]
    fn test_drifting_clocks_are_kept_in_step() {
        // Two devices' crystals a little apart, as separate devices' are,
        // with rough timing besides: the feed stretches or trims to follow,
        // and never runs dry or drops input once settled
        for (input, output) in [(480, 256), (441, 512), (128, 1024), (1024, 64), (480, 480)] {
            for drift in [0.9999, 1.0001] {
                let ran = run_jittered(input, output, drift, 60.0, 0.001);
                assert_eq!(ran.late_underruns, 0, "{input}/{output} at {drift} ran dry");
                assert_eq!(ran.monitor.overflow_frames(), 0, "{input}/{output} at {drift} dropped input");
                // This far into the ramp, f32 holds it to half a frame
                let steps = ran.heard[ran.heard.len() / 2..].windows(2).map(|w| w[1] - w[0]);
                assert!(steps.clone().all(|step| step > 0.0), "{input}/{output} at {drift} stepped backwards");
                assert!(steps.clone().all(|step| step <= 2.0), "{input}/{output} at {drift} skipped");
                let followed = if drift < 1.0 { ran.monitor.stretched_frames() } else { ran.monitor.settled_frames() };
                // 100 ppm of a minute is 288 frames, give or take settling
                assert!(followed >= 200, "{input}/{output} at {drift} followed {followed} frames");
            }
        }
    }

    #[test]
    fn test_cushion_relaxes_after_a_calm_and_waits_longer_if_that_fails() {
        // 480-frame packets against 256-frame reads, losing 20 ms of input
        // at 1 s and again at 45 s
        let (mut sender, mut feed, monitor) = input_channel(48000);
        let (mut left, mut right) = (vec![0.0; 256], vec![0.0; 256]);
        let seconds = |s: usize| s * 48000;
        let gap = |packet: usize| [seconds(1), seconds(45)].iter().any(|&at| (at..at + 960).contains(&(packet * 480)));
        let mut packets = 0;
        let mut target_at = Vec::new();
        for read in 0..seconds(120) / 256 {
            let now = read * 256;
            while (packets + 1) * 480 <= now {
                if !gap(packets) {
                    sender.push_f32(&[0.25; 480], 1);
                    sender.stamp(Duration::from_secs_f64(((packets + 1) * 480) as f64 / 48000.0));
                }
                packets += 1;
            }
            feed.begin_at(256, Duration::from_secs_f64(now as f64 / 48000.0));
            read_stereo(&mut feed, &mut left, &mut right);
            if now % seconds(5) < 256 {
                target_at.push((now / 48000, monitor.target_frames(), monitor.underrun_frames()));
            }
        }
        let at = |s: usize| target_at.iter().find(|&&(t, ..)| t == s).copied().unwrap();
        let (step, base) = (InputFeed::CUSHION_STEP, 480 + 256 + InputFeed::MIN_CUSHION);
        // The first gap ran dry and grew the cushion; 30 s on, it relaxed
        let (_, target, dry) = at(5);
        assert!(dry > 0, "the gap ran dry");
        assert_eq!(target, base + step);
        assert_eq!(at(30).1, base + step, "not yet");
        assert_eq!(at(35).1, base, "relaxed after 30 s calm");
        assert_eq!(at(40).2, dry, "and held");
        // The second gap ran dry soon after relaxing, so the next try
        // waits twice as long
        let (_, target, again) = at(50);
        assert!(again > dry);
        assert_eq!(target, base + step);
        assert_eq!(at(100).1, base + step, "not after 30 s");
        assert_eq!(at(110).1, base, "after 60 s");
        assert_eq!(monitor.overflow_frames(), 0);
    }

    #[test]
    fn test_quiet_input_is_trimmed_at_once() {
        // Silence, then a tone: the trim planned after the first window
        // lands in one cut while it's quiet. A loud input takes it a frame
        // in 256 at a time
        for quiet in [true, false] {
            let (mut sender, mut feed, monitor) = input_channel(48000);
            let (mut left, mut right) = (vec![0.0; 256], vec![0.0; 256]);
            let level = if quiet { 0.0 } else { 0.5 };
            let mut pushed = 0;
            let mut settled_soon_after = 0;
            for read in 0..1000 {
                // 480-frame packets every 10 ms
                let now = read * 256 + 100;
                while pushed + 480 <= now {
                    sender.push_f32(&[level; 480], 1);
                    pushed += 480;
                    sender.stamp(Duration::from_secs_f64(pushed as f64 / 48000.0));
                }
                feed.begin_at(256, Duration::from_secs_f64(now as f64 / 48000.0));
                read_stereo(&mut feed, &mut left, &mut right);
                // The first window ends two seconds after priming
                if read == 2 * 48000 / 256 + 10 {
                    settled_soon_after = monitor.settled_frames();
                }
            }
            let settled = monitor.settled_frames();
            assert!(settled >= 100, "quiet {quiet}: settled {settled} frames");
            if quiet {
                assert_eq!(settled_soon_after, settled, "all at once");
            } else {
                assert!(settled_soon_after <= 12, "a frame at a time: {settled_soon_after} frames");
                assert!(left.iter().all(|&s| (s - 0.5).abs() < 1e-6), "a squeezed tone holds its level");
            }
            assert_eq!(monitor.underrun_frames(), 0);
        }
    }

    #[test]
    fn test_mono_input_is_heard_on_both_sides() {
        let (mut sender, mut feed, _) = input_channel(48000);
        // Several small buffers fill it past the target: one buffer of each
        // side, and the cushion
        for _ in 0..4 {
            sender.push_f32(&[0.5; 128], 1);
        }
        feed.begin(256);
        let (mut left, mut right) = (vec![0.0; 256], vec![0.0; 256]);
        read_stereo(&mut feed, &mut left, &mut right);
        assert!(left.iter().all(|&s| s == 0.5));
        assert_eq!(left, right);
    }

    #[test]
    fn test_takes_the_first_two_channels_of_an_interface() {
        let (mut sender, mut feed, _) = input_channel(48000);
        let frames: Vec<f32> = (0..128).flat_map(|_| [0.25, -0.25, 9.0, 9.0]).collect();
        for _ in 0..4 {
            sender.push_f32(&frames, 4);
        }
        feed.begin(128);
        let (mut left, mut right) = (vec![0.0; 128], vec![0.0; 128]);
        read_stereo(&mut feed, &mut left, &mut right);
        assert!(left.iter().all(|&s| s == 0.25));
        assert!(right.iter().all(|&s| s == -0.25));
    }

    #[test]
    fn test_converts_integer_samples() {
        let (mut sender, mut feed, _) = input_channel(48000);
        for _ in 0..4 {
            sender.push(&[i16::MIN; 128], 1, |s| s as f32 / 32768.0);
        }
        feed.begin(128);
        let (mut left, mut right) = (vec![0.0; 128], vec![0.0; 128]);
        read_stereo(&mut feed, &mut left, &mut right);
        assert!(left.iter().all(|&s| s == -1.0));
    }

    #[test]
    fn test_underrun_fills_silence_counts_it_and_refills() {
        let (mut sender, mut feed, monitor) = input_channel(48000);
        let (mut left, mut right) = (vec![0.0; 256], vec![0.0; 256]);

        // 2 x 480 frames: enough to start, then the input goes quiet
        sender.push_f32(&ramp(1, 480), 2);
        sender.push_f32(&ramp(481, 480), 2);
        // Each read a second on: the next buffer is long overdue
        let late = Duration::from_secs(1);
        let mut heard = Vec::new();
        for _ in 0..5 {
            feed.begin_at(256, late);
            read_stereo(&mut feed, &mut left, &mut right);
            heard.extend_from_slice(&left);
        }
        // All 960 frames, in order, then silence for the rest
        assert_eq!(&heard[..960], &ramp(1, 960).iter().step_by(2).copied().collect::<Vec<_>>()[..]);
        assert!(heard[960..].iter().all(|&s| s == 0.0));
        // Only the read that ran dry counts; the one after was waiting
        assert_eq!(monitor.underrun_frames(), (4 * 256 - 960) as u64);

        // Running dry raised the target, and nothing is read until the
        // ring is back up to it
        let target = 480 + 256 + InputFeed::MIN_CUSHION + InputFeed::CUSHION_STEP;
        sender.push_f32(&ramp(2000, 480), 2);
        feed.begin_at(256, late);
        assert_eq!(monitor.buffered_frames(), target);
        read_stereo(&mut feed, &mut left, &mut right);
        assert!(left.iter().all(|&s| s == 0.0), "still refilling");
        assert_eq!(monitor.underrun_frames(), (4 * 256 - 960) as u64, "refilling isn't an underrun");

        sender.push_f32(&ramp(2480, 480), 2);
        feed.begin_at(256, late);
        read_stereo(&mut feed, &mut left, &mut right);
        assert_eq!(left[0], 2000.0, "reading resumes where the input did");
    }

    #[test]
    fn test_a_slow_input_clock_is_stretched_to_keep_up() {
        // Input 0.2% slow: about 96 frames short a second. As the ring runs
        // low, each block takes in a frame less than it plays now and then
        let (heard, monitor) = run(480, 256, 0.998, 20.0);
        assert_unbroken(&heard);
        assert_eq!(monitor.underrun_frames(), 0, "it never runs dry");
        assert_eq!(monitor.overflow_frames(), 0);
        // About the frames it fell short, after the trim it settled by
        let stretched = monitor.stretched_frames();
        assert!((1500..2500).contains(&stretched), "stretched {stretched} frames");
    }

    #[test]
    fn test_a_fast_input_clock_is_trimmed_back() {
        // Input 0.5% fast: 240 frames a second too many
        let (heard, monitor) = run(480, 256, 1.005, 20.0);
        let dropped = monitor.overflow_frames();
        assert!(dropped > 0, "the excess was trimmed");
        // Roughly the excess, not more
        assert!(dropped <= 20 * 240 + 2000, "dropped {dropped}");
        assert_eq!(monitor.underrun_frames(), 0);
        let jumps = heard.windows(2).filter(|w| w[1] != w[0] + 1.0).count();
        assert!(jumps > 0, "the trims show as jumps forward");
        assert!(heard.windows(2).all(|w| w[1] >= w[0]), "never backwards");
    }

    #[test]
    fn test_a_full_ring_drops_new_input_and_counts_it() {
        let (mut sender, _feed, monitor) = input_channel(48000);
        let capacity = 24000;
        sender.push_f32(&ramp(0, capacity), 2);
        assert_eq!(monitor.overflow_frames(), 0);
        sender.push_f32(&ramp(capacity, 500), 2);
        assert_eq!(monitor.overflow_frames(), 500);
    }

    #[test]
    fn test_an_output_stall_is_trimmed_to_the_newest_input() {
        let (mut sender, mut feed, monitor) = input_channel(48000);
        let (mut left, mut right) = (vec![0.0; 256], vec![0.0; 256]);
        sender.push_f32(&ramp(1, 480), 2);
        sender.push_f32(&ramp(481, 480), 2);
        feed.begin(256);
        read_stereo(&mut feed, &mut left, &mut right);
        // The output stalls for 100 ms while input keeps coming
        for chunk in 0..10 {
            sender.push_f32(&ramp(961 + chunk * 480, 480), 2);
        }
        feed.begin(256);
        read_stereo(&mut feed, &mut left, &mut right);
        let target = monitor.target_frames();
        assert_eq!(monitor.overflow_frames() as usize, 960 + 4800 - 256 - target);
        // What's read is the newest input, `target` frames behind
        assert_eq!(left[0] as usize, 960 + 4800 - target + 1);
    }

    /// Runs `buffers` ASIO-style buffer switches of `frames` frames, each
    /// running both callbacks, the input's first if `input_first`. Returns
    /// the frames heard (left side), and the monitor.
    fn run_lockstep(frames: usize, buffers: usize, input_first: bool) -> (Vec<f32>, InputMonitor) {
        let (mut sender, mut feed, monitor) = input_channel_same_clock(48000, input_first, 2);
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        let mut heard = Vec::new();
        for switch in 0..buffers {
            let input = ramp(switch * frames, frames);
            if input_first {
                sender.push_f32(&input, 2);
            }
            feed.begin(frames);
            read_stereo(&mut feed, &mut left, &mut right);
            heard.extend_from_slice(&left);
            if !input_first {
                sender.push_f32(&input, 2);
            }
        }
        (heard, monitor)
    }

    #[test]
    fn test_same_clock_holds_one_buffer_whichever_callback_runs_first() {
        for frames in [32, 64, 128, 256] {
            for input_first in [true, false] {
                let (heard, monitor) = run_lockstep(frames, 2000, input_first);
                let (start, squeezed) = assert_unbroken(&heard);
                assert_eq!(squeezed, 0, "nothing to trim");
                assert_eq!(monitor.underrun_frames(), 0, "{frames}, input first {input_first}");
                assert_eq!(monitor.overflow_frames(), 0, "{frames}, input first {input_first}");
                assert_eq!(monitor.target_frames(), frames, "one buffer, no cushion");
                if input_first {
                    // Each read takes the buffer that arrived just before it
                    assert_eq!(start, 1, "the first switch is heard at once");
                    assert_eq!(monitor.buffered_frames(), 0, "adds nothing to the round trip");
                } else {
                    assert_eq!(start, frames + 1, "heard one switch later");
                    assert_eq!(monitor.buffered_frames(), frames);
                }
            }
        }
    }

    #[test]
    fn test_same_clock_recovers_from_a_late_input() {
        // The driver runs the output's callback first once, where it had
        // been running the input's: that read runs dry, and the cushion
        // grows by a step, so the order no longer matters
        let frames = 128;
        let (mut sender, mut feed, monitor) = input_channel_same_clock(48000, true, 2);
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        let mut pushed = 0;
        let mut push = |sender: &mut InputSender| {
            sender.push_f32(&ramp(pushed, frames), 2);
            pushed += frames;
        };
        for _ in 0..10 {
            push(&mut sender);
            feed.begin(frames);
            read_stereo(&mut feed, &mut left, &mut right);
        }
        feed.begin(frames);
        read_stereo(&mut feed, &mut left, &mut right);
        assert_eq!(monitor.underrun_frames(), frames as u64);
        for i in 0..200 {
            if i == 1 {
                assert_eq!(monitor.target_frames(), frames + InputFeed::CUSHION_STEP);
            }
            push(&mut sender);
            feed.begin(frames);
            read_stereo(&mut feed, &mut left, &mut right);
        }
        assert_eq!(monitor.underrun_frames(), frames as u64, "it settled");
        assert_eq!(monitor.overflow_frames(), 0);
    }

    /// A 1 kHz sine at `rate`, `seconds` long, at amplitude 0.5.
    fn sine(rate: u32, seconds: f64) -> Vec<f32> {
        (0..(seconds * rate as f64) as usize)
            .map(|n| 0.5 * (2.0 * std::f64::consts::PI * 1000.0 * n as f64 / rate as f64).sin() as f32)
            .collect()
    }

    /// Converts mono `input` (pushed `chunk` frames at a time) and returns
    /// the left side of everything that came out.
    fn convert(input: &[f32], input_rate: u32, output_rate: u32, chunk: usize) -> Vec<f32> {
        let (mut sender, mut feed, _) = input_channel_converting(input_rate, output_rate, 2);
        let mut out = Vec::new();
        for piece in input.chunks(chunk) {
            sender.push_f32(piece, 1);
            let level = feed.level();
            let (mut left, mut right) = (vec![0.0; level], vec![0.0; level]);
            feed.primed = true;
            read_stereo(&mut feed, &mut left, &mut right);
            out.extend_from_slice(&left);
        }
        out
    }

    #[test]
    fn test_converts_between_rates_keeping_pitch_and_level() {
        for (from, to) in [(48000, 44100), (44100, 48000), (96000, 48000), (16000, 48000)] {
            let out = convert(&sine(from, 1.0), from, to, 480);
            // One second in, one second out (the curve runs two frames behind)
            let expected = to as usize;
            assert!(out.len().abs_diff(expected) <= 3, "{from} -> {to}: {} frames", out.len());

            // Still 1 kHz: count upward zero crossings past the start-up
            let settled = &out[100..];
            let crossings = settled.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64;
            let hz = crossings / (settled.len() as f64 / to as f64);
            assert!((hz - 1000.0).abs() < 2.0, "{from} -> {to}: {hz} Hz");

            // Still at 0.5, within a whisker
            let peak = settled.iter().fold(0.0f32, |p, s| p.max(s.abs()));
            assert!((peak - 0.5).abs() < 0.01, "{from} -> {to}: peak {peak}");
        }
    }

    #[test]
    fn test_conversion_is_clean() {
        // Compare 48 -> 44.1 kHz against the ideal sine at 44.1 kHz, lined
        // up for the converter's two-frame delay
        let out = convert(&sine(48000, 0.5), 48000, 44100, 480);
        let delay = 2.0 * 44100.0 / 48000.0;
        let error: f64 = out[200..out.len() - 10]
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let t = (i as f64 + 200.0 - delay) / 44100.0;
                let ideal = 0.5 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
                (s as f64 - ideal).powi(2)
            })
            .sum::<f64>()
            / (out.len() - 210) as f64;
        let snr_db = 10.0 * (0.125 / error).log10();
        assert!(snr_db > 60.0, "a 1 kHz tone converts {snr_db:.1} dB clean");
    }

    #[test]
    fn test_conversion_is_seamless_across_buffers() {
        let input = sine(48000, 0.2);
        let whole = convert(&input, 48000, 44100, input.len());
        for chunk in [1, 37, 480, 512] {
            assert_eq!(convert(&input, 48000, 44100, chunk), whole, "pushed {chunk} at a time");
        }
    }

    #[test]
    fn test_converted_input_runs_steady_against_the_output() {
        // A 48 kHz microphone feeding a 44.1 kHz output: 480-frame input
        // buffers every 10 ms against 441-frame output reads every 10 ms
        let (mut sender, mut feed, monitor) = input_channel_converting(48000, 44100, 2);
        let (mut left, mut right) = (vec![0.0; 441], vec![0.0; 441]);
        let input = vec![0.25; 480];
        for _ in 0..1000 {
            sender.push_f32(&input, 1);
            feed.begin(441);
            read_stereo(&mut feed, &mut left, &mut right);
        }
        assert_eq!(monitor.underrun_frames(), 0);
        assert_eq!(monitor.overflow_frames(), 0);
        assert!(left.iter().all(|&s| (s - 0.25).abs() < 1e-6));
        assert!(monitor.buffered_frames() < 441 * 3, "{} frames buffered", monitor.buffered_frames());
    }

    #[test]
    fn test_discard_empties_the_ring_and_waits_to_refill() {
        let (mut sender, mut feed, monitor) = input_channel(48000);
        sender.push_f32(&ramp(1, 2000), 2);
        feed.discard();
        sender.push_f32(&ramp(5000, 100), 2);
        feed.begin(256);
        let (mut left, mut right) = (vec![1.0; 256], vec![1.0; 256]);
        read_stereo(&mut feed, &mut left, &mut right);
        assert!(left.iter().all(|&s| s == 0.0));
        assert_eq!(monitor.underrun_frames(), 0);
    }
}
