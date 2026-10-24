//! The patch's MIDI clock: one stream from the engine, for every port (#437).
//!
//! The Clock that sets the patch's tempo and beat (the
//! [`TransportState`]) goes out as MIDI clock, 24 ticks a beat, with Start,
//! Stop, Continue and Song Position. The engine runs a [`MidiClock`] beside
//! the patch every block, stamping each message with its sample as a MIDI
//! Out does, and the sender passes them to each port whose clock setting
//! wants them (see [`ClockMode`](super::midi::ClockMode)). Clock used to be
//! a MIDI Out's, and two on one port sent it twice.
//!
//! It runs only while some port wants it, so a rig without clock costs the
//! sender nothing.

use rtrb::Producer;
use web_time::Instant;

use crate::dsp::context::TransportState;
use super::midi::{status, Message, MidiBus, Outgoing, MIDI};

/// MIDI clock ticks in a beat.
const PPQN: f64 = 24.0;

/// Sends the patch's clock to the sender. Lives on the audio thread.
pub struct MidiClock {
    bus: &'static MidiBus,
    ring: Option<Producer<Outgoing>>,
    /// Samples run so far: the clock the ring's messages are stamped on.
    sample: u64,
    /// Whether clock is going out (Start or Continue sent, Stop not yet).
    running: bool,
    /// The next tick to send, counting 24 a beat from beat 0.
    next_tick: i64,
}

impl MidiClock {
    /// The clock for the app's bus. Made on the UI thread: it opens a ring.
    pub fn new() -> Self {
        Self::on_bus(&MIDI)
    }

    /// A clock writing to a ring from `bus`: a test's own, say.
    pub fn on_bus(bus: &'static MidiBus) -> Self {
        Self { bus, ring: bus.open_clock_stream(), sample: 0, running: false, next_tick: 0 }
    }

    /// Sends a block's clock: `frames` at `rate`, leaving for the output
    /// device at `sent_at` (now, for a block not played as it's made), with
    /// the patch's transport at its start, if it has a Clock. `transport`
    /// is asked only while a port wants the clock.
    ///
    /// REAL-TIME SAFE: atomics and a ring.
    pub fn run(&mut self, transport: impl FnOnce() -> Option<TransportState>, frames: usize, rate: f32, sent_at: Option<Instant>) {
        if self.ring.is_none() {
            return;
        }
        let wanted = self.bus.wants_clock();
        if wanted || self.running {
            let at = sent_at.unwrap_or_else(Instant::now);
            self.push(Outgoing::Block { sample: self.sample, at, rate, port: 0 });
        }
        match wanted.then(transport).flatten() {
            Some(transport) => self.ticks(&transport, frames, rate),
            None => self.stop(),
        }
        self.sample += frames as u64;
    }

    /// Stops the clock, if it's going: the audio stopped.
    pub fn stop(&mut self) {
        self.stop_at(0);
    }

    fn push(&mut self, event: Outgoing) {
        if let Some(ring) = &mut self.ring {
            // A full ring drops it: the sender reads every millisecond
            let _ = ring.push(event);
        }
    }

    /// Puts `message` in the ring at `offset` samples into this block.
    fn emit(&mut self, offset: usize, message: Message) {
        self.push(Outgoing::Message { sample: self.sample + offset as u64, message });
    }

    fn stop_at(&mut self, offset: usize) {
        if std::mem::take(&mut self.running) {
            self.emit(offset, Message::real_time(status::STOP));
        }
    }

    /// This block's clock, as `transport` moves through it.
    fn ticks(&mut self, transport: &TransportState, frames: usize, rate: f32) {
        let Some(start) = transport.beat_position.filter(|_| transport.playing) else {
            self.stop_at(0);
            return;
        };
        let beats_per_sample = transport.tempo_bpm.unwrap_or(120.0) as f64 / 60.0 / rate as f64;
        let end = start + frames as f64 * beats_per_sample;
        // The first sample at or past `beat` (a hair's leeway for rounding)
        let at = |beat: f64| (((beat - start) / beats_per_sample - 1e-6).ceil().max(0.0) as usize).min(frames.saturating_sub(1));
        let tick = 1.0 / PPQN;

        // Moved more than a tick from where it was going (a jump in the
        // song): stop, and start again from the new place
        if self.running && (start < (self.next_tick - 1) as f64 * tick - tick || start > self.next_tick as f64 * tick + tick) {
            self.stop_at(0);
        }
        if !self.running {
            if end <= 0.0 {
                return; // a count-in
            }
            if start <= 1e-9 {
                // From the top: Start, and the first tick is beat 0
                self.emit(at(0.0), Message::real_time(status::START));
                self.next_tick = 0;
            } else {
                // Partway in: where to carry on from, then Continue. The
                // follower waits for the next sixteenth's tick
                let sixteenth = (start * 4.0).ceil();
                self.emit(0, Message::song_position(sixteenth.min(16383.0) as u16));
                self.emit(0, Message::real_time(status::CONTINUE));
                self.next_tick = sixteenth as i64 * 6;
            }
            self.running = true;
        }
        loop {
            // A tick a rounding error short of the block's end is the next block's first
            let offset = ((self.next_tick as f64 * tick - start) / beats_per_sample - 1e-6).ceil().max(0.0) as usize;
            if offset >= frames {
                break;
            }
            self.emit(offset, Message::real_time(status::CLOCK));
            self.next_tick += 1;
        }
    }
}

impl Default for MidiClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::midi::ClockMode;
    use rtrb::Consumer;

    const BLOCK: usize = 64;
    const RATE: f32 = 48_000.0;

    /// A clock on a bus of its own, port A wanting it, and the far end of its ring.
    fn clock() -> (&'static MidiBus, MidiClock, Consumer<Outgoing>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        bus.set_clock(0, ClockMode::Transport);
        let clock = MidiClock::on_bus(bus);
        let ring = bus.take_clock_stream().unwrap();
        (bus, clock, ring)
    }

    /// Runs `blocks` blocks from `beat` at `bpm`, as the engine would, the
    /// beat advancing with each. Returns what was sent, as (sample, bytes).
    fn run(clock: &mut MidiClock, ring: &mut Consumer<Outgoing>, beat: &mut f64, bpm: f32, blocks: usize, playing: bool) -> Vec<(u64, Vec<u8>)> {
        let mut sent = Vec::new();
        for _ in 0..blocks {
            let transport = TransportState { playing, beat_position: Some(*beat), ..TransportState::playing_at(bpm) };
            clock.run(|| Some(transport), BLOCK, RATE, None);
            while let Ok(event) = ring.pop() {
                if let Outgoing::Message { sample, message } = event {
                    sent.push((sample, message.bytes().to_vec()));
                }
            }
            if playing {
                *beat += BLOCK as f64 * bpm as f64 / 60.0 / RATE as f64;
            }
        }
        sent
    }

    #[test]
    fn the_clock_ticks_24_a_beat_from_start() {
        let (_, mut clock, mut ring) = clock();
        let mut beat = 0.0;
        // Just short of four beats at 120 BPM (2 s is 1500 blocks)
        let sent = run(&mut clock, &mut ring, &mut beat, 120.0, 1499, true);
        assert_eq!(sent[0], (0, vec![status::START]));
        let ticks: Vec<u64> = sent.iter().filter(|(_, m)| m == &[status::CLOCK]).map(|(s, _)| *s).collect();
        assert_eq!(ticks.len(), 96);
        // Every 1000 samples: 24 a beat at 2 beats a second, at 48 kHz
        assert!(ticks.iter().enumerate().all(|(k, &s)| s == k as u64 * 1000), "{:?}", &ticks[..4]);
        // Stopped, then played on from beat 4: Stop, then the position and Continue
        let stopped = run(&mut clock, &mut ring, &mut beat, 120.0, 2, false);
        assert_eq!(stopped.iter().map(|(_, m)| m[0]).collect::<Vec<_>>(), [status::STOP]);
        beat = 4.1;
        let resumed = run(&mut clock, &mut ring, &mut beat, 120.0, 1, true);
        assert_eq!(resumed[0].1, Message::song_position(17).bytes(), "carries on from the next sixteenth");
        assert_eq!(resumed[1].1, vec![status::CONTINUE]);
    }

    #[test]
    fn a_count_in_starts_the_clock_on_the_downbeat() {
        let (_, mut clock, mut ring) = clock();
        // Half a beat before the top
        let mut beat = -0.5;
        let sent = run(&mut clock, &mut ring, &mut beat, 120.0, 200, true);
        let start = sent.iter().position(|(_, m)| m == &[status::START]).unwrap();
        assert_eq!(sent[start].0, 12_000, "half a beat at 120 BPM");
        assert_eq!(sent[start + 1], (12_000, vec![status::CLOCK]));
    }

    #[test]
    fn a_jump_in_the_song_restarts_the_clock() {
        let (_, mut clock, mut ring) = clock();
        let mut beat = 0.0;
        run(&mut clock, &mut ring, &mut beat, 120.0, 100, true);
        beat = 0.0;
        let sent: Vec<u8> = run(&mut clock, &mut ring, &mut beat, 120.0, 1, true).iter().map(|(_, m)| m[0]).collect();
        assert_eq!(&sent[..3], [status::STOP, status::START, status::CLOCK]);
    }

    #[test]
    fn no_port_wanting_it_sends_nothing_and_stops_a_running_clock() {
        let (bus, mut clock, mut ring) = clock();
        let mut beat = 0.0;
        run(&mut clock, &mut ring, &mut beat, 120.0, 10, true);
        bus.set_clock(0, ClockMode::Off);
        assert_eq!(run(&mut clock, &mut ring, &mut beat, 120.0, 1, true), [(10 * BLOCK as u64, vec![status::STOP])]);
        clock.run(|| None, BLOCK, RATE, None);
        assert!(ring.pop().is_err(), "not even a block: the sender can idle");
    }

    #[test]
    fn a_patch_without_a_clock_sends_none() {
        let (_, mut clock, mut ring) = clock();
        clock.run(|| None, BLOCK, RATE, None);
        assert!(matches!(ring.pop(), Ok(Outgoing::Block { sample: 0, .. })));
        assert!(ring.pop().is_err());
    }

    #[test]
    fn stopping_the_audio_stops_the_clock() {
        let (_, mut clock, mut ring) = clock();
        let mut beat = 0.0;
        run(&mut clock, &mut ring, &mut beat, 120.0, 4, true);
        clock.stop();
        assert!(matches!(ring.pop(), Ok(Outgoing::Message { message, .. }) if message.bytes() == [status::STOP]));
        clock.stop();
        assert!(ring.pop().is_err(), "once");
    }

    #[test]
    fn midi_clock_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MidiClock>();
    }
}
