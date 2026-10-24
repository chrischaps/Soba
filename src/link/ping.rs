//! Measuring a device's delay (#449): ping a note, hear it come back, and
//! line the port up.
//!
//! Every device takes its own time to turn a note into sound. With its
//! audio coming back in, Soba can time that itself:
//!
//! 1. **Ping.** The engine plays a note a few times on the port
//!    ([`PingPlan`]), from a ring of its own on the [`MidiBus`], as a MIDI
//!    Out does: each block says when it leaves for the output device, and
//!    the sender sends each note as its sample is heard, the port's offset
//!    later (or earlier).
//! 2. **Listen.** Beside it, the engine copies what the audio input hears
//!    into a capture ring the UI set aside, sample for sample on the same
//!    count as the notes, from a little before the first to the end of the
//!    last. All of this is [`Pinger`]'s, on the audio thread; it runs
//!    whether the transport plays or not.
//! 3. **Find each one.** On the UI side, each ping's window is searched for
//!    where the sound begins ([`arrival`]): a number of samples after the
//!    note's own.
//! 4. **Take away the rest of the trip.** That time holds more than the
//!    device: the output's delay and the port's offset before the note
//!    left, and the input's delay and jitter buffer after the sound came
//!    back. [`Trip`] takes them away (the same [`RoundTrip`] the status bar
//!    shows), leaving the device's own delay: from the note leaving Soba to
//!    its sound at the device's jack.
//! 5. **Agree.** The pings' median and spread ([`agree`]), or a reason to
//!    give up: too few heard over the noise, or too scattered to trust.
//!
//! **The offset to set.** Soba's own sound reaches the speakers the
//! output's delay after its block is made, and the limiter's look-ahead
//! after that; a port's note leaves the output's delay after, plus its
//! offset. So a device that takes `d` to sound lands with Soba with an
//! offset of the look-ahead less `d` ([`Trip::offset_for`]), as far
//! earlier as the output's delay allows.
//!
//! [`arrival`]: crate::dsp::arrival::arrival
//! [`agree`]: crate::dsp::arrival::agree

use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};
use web_time::Instant;

use super::midi::{Message, MidiBus, Outgoing, OFFSET_RANGE};
use crate::dsp::arrival::arrival;
use crate::dsp::input_channels::InputChannels;
use crate::dsp::InputAudio;
use crate::engine::RoundTrip;

/// How a measurement plays its pings, in frames at the output's rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingPlan {
    /// The port, 0 to 7.
    pub port: u8,
    /// The MIDI channel, 0 to 15.
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    /// How many pings.
    pub pings: usize,
    /// From one ping to the next.
    pub every: usize,
    /// How long each note is held.
    pub hold: usize,
    /// From the start to the first ping: time for the input to settle.
    pub lead: usize,
    /// The quiet listened to before each ping, to know the noise from it.
    pub quiet: usize,
    /// The input's channels the device comes back on: a pair is heard as
    /// its middle.
    pub listen: InputChannels,
}

impl PingPlan {
    /// How many pings a measurement plays.
    pub const PINGS: usize = 8;
    /// From one ping to the next: time for a kick to die away.
    pub const EVERY: f32 = 0.6;
    /// How long each note is held.
    pub const HOLD: f32 = 0.08;
    /// The quiet before each ping.
    pub const QUIET: f32 = 0.1;
    /// Before the first ping while the transport plays, and the input is
    /// already running...
    pub const LEAD: f32 = 0.3;
    /// ...and while it's stopped: a stopped transport doesn't read the
    /// input, so its jitter buffer fills afresh and takes a couple of
    /// seconds to settle to its latency.
    pub const LEAD_STOPPED: f32 = 2.6;
    /// How hard each ping is played.
    pub const VELOCITY: u8 = 112;

    /// The pings for `note` on `channel` (0 to 15) of `port`, at `rate`,
    /// starting `lead` seconds in, heard on the input's first two channels
    /// (see [`listening_on`](Self::listening_on)).
    pub fn new(port: usize, channel: u8, note: u8, rate: f32, lead: f32) -> Self {
        let frames = |seconds: f32| (seconds * rate.max(1.0)).round() as usize;
        Self {
            port: port.min(u8::MAX as usize) as u8,
            channel: channel.min(15),
            note: note.min(127),
            velocity: Self::VELOCITY,
            pings: Self::PINGS,
            every: frames(Self::EVERY),
            hold: frames(Self::HOLD),
            lead: frames(lead.max(Self::QUIET)),
            quiet: frames(Self::QUIET),
            listen: InputChannels::default(),
        }
    }

    /// The same pings, heard on `listen`.
    pub fn listening_on(self, listen: InputChannels) -> Self {
        Self { listen, ..self }
    }

    /// What the plan hears of frame `frame` of `input`: its channel, or
    /// the middle of its pair.
    #[inline]
    fn hear(&self, input: &InputAudio<'_>, frame: usize) -> f32 {
        let (left, right) = self.listen.sides();
        let (l, r) = self.listen.frame(input, frame);
        // A pair on a mono device is its one channel, not half of it
        if right != left && right < input.channels() {
            0.5 * (l + r)
        } else {
            l
        }
    }

    /// How many frames the measurement listens to.
    pub fn frames(&self) -> usize {
        self.lead + self.pings * self.every
    }

    /// The frame ping `k` is played on.
    pub fn note_at(&self, k: usize) -> usize {
        self.lead + k * self.every
    }

    /// How far after its note each ping's sound is looked for.
    pub fn window(&self) -> usize {
        self.every.saturating_sub(self.quiet)
    }

    /// The `index`th message, in time order (each ping's Note On, then its
    /// Note Off), and the frame it plays on.
    fn message(&self, index: usize) -> Option<(usize, Message)> {
        if index >= 2 * self.pings {
            return None;
        }
        let at = self.note_at(index / 2);
        Some(if index.is_multiple_of(2) {
            (at, Message::note_on(self.channel, self.note, self.velocity))
        } else {
            (at + self.hold.min(self.window()), Message::note_off(self.channel, self.note))
        })
    }

    /// Where each ping's sound begins in `capture`, in frames after its
    /// note, or `None` for one not heard clear of the quiet before it.
    pub fn arrivals(&self, capture: &[f32]) -> Vec<Option<usize>> {
        (0..self.pings)
            .map(|k| {
                let at = self.note_at(k);
                let quiet = capture.get(at.saturating_sub(self.quiet)..at)?;
                let window = &capture.get(at..)?[..self.window().min(capture.len() - at)];
                arrival(quiet, window)
            })
            .collect()
    }
}

/// A measurement on its way to the audio thread: the plan, the ring its
/// notes go out on, and the ring what the input hears goes into.
///
/// Dropped on the audio thread when it's done, which frees nothing there:
/// the sender keeps the other end of the notes' ring until it finds it
/// abandoned, and the UI keeps the capture's until it does (see
/// [`Listening`]).
pub struct Ping {
    plan: PingPlan,
    midi: Producer<Outgoing>,
    capture: Producer<f32>,
    /// Frames run so far: the count the notes are stamped on, and where
    /// the next frame heard goes.
    sample: u64,
    /// The next of the plan's messages to send.
    next: usize,
    /// Frames put in the capture so far.
    heard: usize,
}

impl Ping {
    /// A measurement playing `plan` into `midi` (a ring from
    /// [`MidiBus::open_stream`]), with the capture ring for it, made here:
    /// the UI's end. Allocates; call on the UI thread.
    pub fn new(plan: PingPlan, midi: Producer<Outgoing>) -> (Self, Listening) {
        let (capture, heard) = RingBuffer::new(plan.frames().max(1));
        let ping = Self { plan, midi, capture, sample: 0, next: 0, heard: 0 };
        (ping, Listening { plan, heard, captured: Vec::with_capacity(plan.frames()) })
    }

    /// A measurement playing `plan` on `bus`, or `None` while no sender
    /// drains it.
    pub fn on_bus(bus: &MidiBus, plan: PingPlan) -> Option<(Self, Listening)> {
        Some(Self::new(plan, bus.open_stream()?))
    }
}

/// The audio thread's measurement, if one is running.
#[derive(Default)]
pub struct Pinger {
    ping: Option<Ping>,
}

impl Pinger {
    /// Starts `ping`, or with `None` stops the one running. One that was
    /// running stops where it is.
    ///
    /// REAL-TIME SAFE: see [`Ping`].
    pub fn start(&mut self, ping: Option<Ping>) {
        self.ping = ping;
    }

    /// Whether a measurement is running.
    pub fn is_pinging(&self) -> bool {
        self.ping.is_some()
    }

    /// Runs one block of `frames` at `rate`: its notes, and what `input`
    /// heard during it. The block leaves for the output device at `at`;
    /// `None` (a block not played as it's made), or no input, stops the
    /// measurement, which the UI finds short.
    ///
    /// REAL-TIME SAFE: two rings, written into room made beforehand.
    pub fn run(&mut self, input: &InputAudio<'_>, frames: usize, rate: f32, at: Option<Instant>) {
        let Some(ping) = self.ping.as_mut() else { return };
        let heard = input.frames() >= frames && input.channels() > 0;
        let Some(at) = at.filter(|_| heard && frames > 0) else {
            self.ping = None;
            return;
        };
        let start = ping.sample as usize;
        // A full ring loses it: the sender reads every millisecond
        let _ = ping.midi.push(Outgoing::Block { sample: ping.sample, at, rate, port: ping.plan.port });
        while let Some((sample, message)) = ping.plan.message(ping.next).filter(|&(sample, _)| sample < start + frames) {
            let _ = ping.midi.push(Outgoing::Message { sample: sample as u64, message });
            ping.next += 1;
        }
        // The ring holds the whole measurement, so it never runs out of room
        let fit = ping.capture.slots().min(frames).min(ping.plan.frames() - ping.heard);
        if let Ok(chunk) = ping.capture.write_chunk_uninit(fit) {
            let plan = ping.plan;
            chunk.fill_from_iter((0..fit).map(|frame| plan.hear(input, frame)));
            ping.heard += fit;
        }
        ping.sample += frames as u64;
        if ping.heard >= ping.plan.frames() {
            // Every frame heard: done
            self.ping = None;
        }
    }
}

/// The UI's end of a measurement: what the input heard, as it comes in.
pub struct Listening {
    plan: PingPlan,
    heard: Consumer<f32>,
    captured: Vec<f32>,
}

/// Where a measurement is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Still listening: this many frames heard of the plan's.
    Listening(usize),
    /// Every frame heard.
    Heard,
    /// The engine stopped it short: the input closed, or the audio stopped.
    Cut(usize),
}

impl Listening {
    pub fn plan(&self) -> &PingPlan {
        &self.plan
    }

    /// Takes in what has been heard since last time.
    pub fn poll(&mut self) -> Progress {
        let ready = self.heard.slots();
        if let Ok(chunk) = self.heard.read_chunk(ready) {
            let (first, second) = chunk.as_slices();
            self.captured.extend_from_slice(first);
            self.captured.extend_from_slice(second);
            chunk.commit_all();
        }
        let heard = self.captured.len();
        if heard >= self.plan.frames() {
            Progress::Heard
        } else if self.heard.is_abandoned() && self.heard.is_empty() {
            Progress::Cut(heard)
        } else {
            Progress::Listening(heard)
        }
    }

    /// Whether the engine has let go of its end, so this one can be
    /// dropped without freeing the ring on the audio thread.
    pub fn released(&self) -> bool {
        self.heard.is_abandoned()
    }

    /// What has been heard.
    pub fn captured(&self) -> &[f32] {
        &self.captured
    }
}

/// The rest of a ping's trip besides the device: what Soba and the audio
/// interface add, from the note's sample to the sound's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trip {
    /// Live input's round trip, part by part.
    pub round_trip: RoundTrip,
    /// What the port's offset moved the note by, in milliseconds: the
    /// offset, as far earlier as the output's delay allows.
    pub sent_offset: f32,
}

impl Trip {
    /// The trip for a port offset by `offset` ms, through `round_trip`.
    pub fn new(round_trip: RoundTrip, offset: f32) -> Self {
        let earliest = -(round_trip.output_device.unwrap_or_default().as_secs_f32() * 1000.0);
        Self { round_trip, sent_offset: offset.max(earliest) }
    }

    /// From a note's sample to its sound's, all but the device's share, in
    /// milliseconds: the output's delay and the offset before the note
    /// leaves, and the input's delay and jitter buffer once it's back. Not
    /// the limiter's look-ahead: that delays what Soba plays, not what it
    /// hears.
    pub fn besides_device(&self) -> f32 {
        let trip = &self.round_trip;
        (trip.total() - trip.limiter).as_secs_f32() * 1000.0 + self.sent_offset
    }

    /// Each ping's device delay in milliseconds, from where it was heard
    /// (`arrivals`, in frames at `rate` after its note).
    pub fn device_delays(&self, arrivals: &[Option<usize>], rate: f32) -> Vec<Option<f32>> {
        let besides = self.besides_device();
        arrivals.iter().map(|at| at.map(|frames| frames as f32 / rate.max(1.0) * 1000.0 - besides)).collect()
    }

    /// The offset that lands a device taking `delay` ms with Soba's sound,
    /// in milliseconds, and whether it's out of reach: earlier than the
    /// output's delay allows, or later than the offset goes.
    pub fn offset_for(&self, delay: f32) -> (f32, bool) {
        let wanted = self.round_trip.limiter.as_secs_f32() * 1000.0 - delay;
        let earliest = -(self.round_trip.output_device.unwrap_or_default().as_secs_f32() * 1000.0);
        let offset = wanted.clamp(earliest.max(-OFFSET_RANGE), OFFSET_RANGE);
        (offset, (offset - wanted).abs() > 0.05)
    }

    /// How late (or, negative, early) a device taking `delay` ms lands
    /// now, against Soba's sound, in milliseconds.
    pub fn lands(&self, delay: f32) -> f32 {
        self.sent_offset + delay - self.round_trip.limiter.as_secs_f32() * 1000.0
    }
}

/// How long the output limiter looks ahead, at `rate`.
pub fn look_ahead(rate: u32) -> Duration {
    RoundTrip::new(None, 0, 0, None, rate).limiter
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn ring() -> (Producer<Outgoing>, Consumer<Outgoing>) {
        RingBuffer::new(1024)
    }

    fn plan() -> PingPlan {
        PingPlan::new(2, 9, 36, RATE, PingPlan::LEAD)
    }

    #[test]
    fn test_the_plan_is_in_frames() {
        let plan = plan();
        assert_eq!((plan.port, plan.channel, plan.note), (2, 9, 36));
        assert_eq!(plan.every, 28_800);
        assert_eq!(plan.lead, 14_400);
        assert_eq!(plan.window(), 24_000);
        assert_eq!(plan.frames(), 14_400 + 8 * 28_800);
        assert_eq!(plan.note_at(3), 14_400 + 3 * 28_800);
        assert_eq!(PingPlan::new(0, 99, 200, RATE, 0.0).lead, plan.quiet, "always a quiet before the first");
        assert_eq!(PingPlan::new(0, 99, 200, RATE, 0.0).channel, 15);
    }

    /// Runs `pinger` over `input` in blocks of `block`, from `start`.
    fn run_all(pinger: &mut Pinger, input: &[f32], block: usize, start: Instant) {
        for (index, chunk) in input.chunks(block).enumerate() {
            let at = start + Duration::from_secs_f64((index * block) as f64 / RATE as f64);
            pinger.run(&InputAudio::interleaved(chunk, 1), chunk.len(), RATE, Some(at));
        }
    }

    #[test]
    fn test_the_pinger_plays_each_note_on_its_sample_and_hears_every_frame() {
        let plan = plan();
        let (midi, mut sent) = ring();
        let (ping, mut listening) = Ping::new(plan, midi);
        let mut pinger = Pinger::default();
        pinger.start(Some(ping));
        assert!(pinger.is_pinging());
        let input: Vec<f32> = (0..plan.frames() + 1000).map(|i| (i % 100) as f32 / 100.0).collect();
        let start = Instant::now();
        run_all(&mut pinger, &input, 256, start);
        assert!(!pinger.is_pinging(), "done once every frame is heard");

        let mut blocks = 0;
        let mut messages = Vec::new();
        while let Ok(event) = sent.pop() {
            match event {
                Outgoing::Block { sample, at, port, .. } => {
                    assert_eq!(port, 2);
                    assert_eq!(at, start + Duration::from_secs_f64(sample as f64 / RATE as f64));
                    blocks += 1;
                }
                Outgoing::Message { sample, message } => messages.push((sample, message)),
                Outgoing::Control { .. } => panic!("a ping sends notes only"),
            }
        }
        assert_eq!(blocks, plan.frames().div_ceil(256));
        assert_eq!(messages.len(), 16);
        for k in 0..8 {
            assert_eq!(messages[2 * k], ((plan.note_at(k)) as u64, Message::note_on(9, 36, PingPlan::VELOCITY)));
            assert_eq!(messages[2 * k + 1], ((plan.note_at(k) + plan.hold) as u64, Message::note_off(9, 36)));
        }
        assert!(sent.is_abandoned(), "the notes' ring is let go");

        assert_eq!(listening.poll(), Progress::Heard);
        assert_eq!(listening.captured(), &input[..plan.frames()]);
        assert!(listening.released());
    }

    #[test]
    fn test_a_pair_is_heard_as_its_middle_and_a_channel_alone() {
        // Four channels: 1.0, 0.0, 0.2, 0.6 in every frame
        let samples: Vec<f32> = (0..64).flat_map(|_| [1.0, 0.0, 0.2, 0.6]).collect();
        let input = InputAudio::interleaved(&samples, 4);
        for (listen, heard) in [(InputChannels::Pair(0), 0.5), (InputChannels::Pair(2), 0.4), (InputChannels::Mono(3), 0.6), (InputChannels::Mono(6), 0.0)] {
            let (ping, mut listening) = Ping::new(plan().listening_on(listen), ring().0);
            let mut pinger = Pinger::default();
            pinger.start(Some(ping));
            pinger.run(&input, 64, RATE, Some(Instant::now()));
            assert_eq!(listening.poll(), Progress::Listening(64));
            assert!(listening.captured().iter().all(|&s| (s - heard).abs() < 1e-6), "{listen:?}");
        }
        // A mono device's one channel, whole
        let (ping, mut listening) = Ping::new(plan(), ring().0);
        let mut pinger = Pinger::default();
        pinger.start(Some(ping));
        pinger.run(&InputAudio::interleaved(&[0.3; 64], 1), 64, RATE, Some(Instant::now()));
        listening.poll();
        assert!(listening.captured().iter().all(|&s| (s - 0.3).abs() < 1e-6));
    }

    #[test]
    fn test_no_input_or_no_clock_cuts_it_short() {
        for case in 0..3 {
            let (ping, mut listening) = Ping::new(plan(), ring().0);
            let mut pinger = Pinger::default();
            pinger.start(Some(ping));
            pinger.run(&InputAudio::interleaved(&[0.1; 512], 2), 256, RATE, Some(Instant::now()));
            match case {
                0 => pinger.run(&InputAudio::default(), 256, RATE, Some(Instant::now())),
                1 => pinger.run(&InputAudio::interleaved(&[0.0; 512], 2), 256, RATE, None),
                _ => pinger.start(None),
            }
            assert!(!pinger.is_pinging());
            assert_eq!(listening.poll(), Progress::Cut(256), "case {case}");
            assert!(listening.released());
        }
    }

    #[test]
    fn test_arrivals_are_found_after_each_note() {
        let plan = plan();
        let mut capture = vec![0.0f32; plan.frames()];
        // Each ping sounds 30 ms and a little more after its note; the sixth is lost
        for k in (0..8).filter(|&k| k != 5) {
            let at = plan.note_at(k) + 1440 + k * 10;
            for (n, s) in capture[at..].iter_mut().take(4800).enumerate() {
                *s = 0.5 * (n as f32 * 0.05).sin() * (-(n as f32) / 2000.0).exp();
            }
        }
        let arrivals = plan.arrivals(&capture);
        for (k, at) in arrivals.iter().enumerate() {
            if k == 5 {
                assert_eq!(*at, None);
            } else {
                let at = at.expect("heard");
                assert!(at.abs_diff(1440 + k * 10) <= 2, "ping {k} at {at}");
            }
        }
        assert_eq!(plan.arrivals(&capture[..plan.note_at(2) + 10]).len(), 8, "a short capture still answers for each");
    }

    fn round_trip(input_ms: u64, buffer_frames: usize, output_ms: u64) -> RoundTrip {
        RoundTrip::new(Some(Duration::from_millis(input_ms)), 480, buffer_frames, Some(Duration::from_millis(output_ms)), 48_000)
    }

    #[test]
    fn test_the_trip_besides_the_device_is_taken_away() {
        // 5 ms in, 10 ms of buffer, 12 ms out, and the port 3 ms late
        let trip = Trip::new(round_trip(5, 480, 12), 3.0);
        assert!((trip.besides_device() - 30.0).abs() < 1e-3, "{}", trip.besides_device());
        // Heard 44 ms after the note: the device took 14
        let delays = trip.device_delays(&[Some(2112), None], RATE);
        assert!((delays[0].unwrap() - 14.0).abs() < 1e-3);
        assert_eq!(delays[1], None);
    }

    #[test]
    fn test_an_early_offset_counts_only_as_far_as_it_reaches() {
        let trip = Trip::new(round_trip(5, 480, 12), -40.0);
        assert_eq!(trip.sent_offset, -12.0);
    }

    #[test]
    fn test_the_offset_to_set_lands_the_device_with_the_sound() {
        let trip = Trip::new(round_trip(5, 480, 12), 0.0);
        let limiter = trip.round_trip.limiter.as_secs_f32() * 1000.0;
        let (offset, out_of_reach) = trip.offset_for(8.0);
        assert!((offset - (limiter - 8.0)).abs() < 1e-4);
        assert!(!out_of_reach);
        assert!((trip.lands(8.0) - (8.0 - limiter)).abs() < 1e-4, "late by its delay, less the look-ahead");
        assert!(Trip::new(round_trip(5, 480, 12), offset).lands(8.0).abs() < 1e-4, "on time once set");

        // Slower than the output's delay can make up for
        let (offset, out_of_reach) = trip.offset_for(30.0);
        assert_eq!(offset, -12.0);
        assert!(out_of_reach);
    }

    #[test]
    fn test_look_ahead_is_the_limiters() {
        assert_eq!(look_ahead(48_000), Duration::from_secs_f64(47.0 / 48_000.0));
    }
}
