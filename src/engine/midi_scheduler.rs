//! Places live MIDI at sample offsets on the audio thread.
//!
//! MIDI arrives between audio callbacks, stamped with the moment it came in
//! ([`TimestampedMidiEvent::received`]). Each callback takes what arrived
//! since the previous callback began and spreads it over its own buffer in
//! proportion to when it arrived: an event that came in a quarter of the way
//! through that interval lands a quarter of the way into the buffer.
//!
//! Every event is therefore heard one callback interval after it arrived,
//! plus the device's output latency. The delay is the same for every event,
//! so a steady sequence stays steady. Applying each event at the start of
//! whichever buffer it happened to fall into would jitter it by up to a
//! whole buffer instead.
//!
//! Notes played on the UI thread (the computer keys, the piano) come on a
//! queue of their own ([`super::ui_notes`]) and are merged with device MIDI
//! in arrival order, placed the same way.

use web_time::Instant;

use rtrb::Consumer;

use crate::dsp::MidiEvent;

use super::midi_engine::TimestampedMidiEvent;

/// The most MIDI events one callback takes. Any more wait for the next
/// callback, which keeps the event list within its preallocated capacity.
pub const MAX_MIDI_EVENTS_PER_CALLBACK: usize = 256;

/// Audio-thread end of the MIDI input queue.
pub struct MidiScheduler {
    /// Events from the MIDI engine, or `None` with no MIDI input.
    input: Option<Consumer<TimestampedMidiEvent>>,
    /// Notes played on the UI thread, or `None` with no UI.
    ui: Option<Consumer<TimestampedMidiEvent>>,
    /// This callback's events, in time order. Allocated once, never grown.
    events: Vec<MidiEvent>,
    /// When the previous callback began.
    previous_callback: Option<Instant>,
}

impl MidiScheduler {
    /// A scheduler with no MIDI input yet.
    pub fn new() -> Self {
        Self {
            input: None,
            ui: None,
            events: Vec::with_capacity(MAX_MIDI_EVENTS_PER_CALLBACK),
            previous_callback: None,
        }
    }

    /// Connects the queue the MIDI engine sends to.
    pub fn set_input(&mut self, input: Consumer<TimestampedMidiEvent>) {
        self.input = Some(input);
    }

    /// Connects the queue the UI sends its notes to.
    pub fn set_ui_input(&mut self, ui: Consumer<TimestampedMidiEvent>) {
        self.ui = Some(ui);
    }

    /// Takes the MIDI that arrived before `now`, for the callback that began
    /// at `now` and renders `frames` samples. Each event's `sample_offset` is
    /// where it falls in those frames; events are in time order.
    ///
    /// REAL-TIME SAFE: no allocation, locking or blocking.
    pub fn collect(&mut self, now: Instant, frames: usize) -> &mut [MidiEvent] {
        self.events.clear();
        let previous = self.previous_callback.replace(now);
        if self.input.is_none() && self.ui.is_none() {
            return &mut self.events;
        }

        let interval = previous.map_or(0.0, |previous| now.saturating_duration_since(previous).as_secs_f64());
        let last_frame = frames.saturating_sub(1);
        let mut earliest = 0;

        while self.events.len() < MAX_MIDI_EVENTS_PER_CALLBACK {
            // Anything that came in after this callback began is the next one's
            let Some(stamped) = pop_earliest(&mut self.input, &mut self.ui, now) else { break };

            let offset = match previous {
                Some(previous) if interval > 0.0 => {
                    let into = stamped.received.saturating_duration_since(previous).as_secs_f64();
                    (into / interval * frames as f64) as usize
                }
                // The first callback has no interval to spread over
                _ => 0,
            };
            // Never earlier than the event before it, whatever the clocks say
            let offset = offset.clamp(earliest, last_frame);
            earliest = offset;

            if let Some(event) = stamped.event.to_dsp(offset as u32) {
                self.events.push(event);
            }
        }
        &mut self.events
    }

    /// Takes the UI's notes played before `now`, at most `room` of them, all
    /// at the start of the buffer, for a callback whose MIDI the caller
    /// placed ([`AudioProcessor::process_offline`]). Device MIDI that came
    /// meanwhile is dropped: the caller's MIDI stands in for it.
    ///
    /// The start, not spread by arrival: a capture renders frame by frame,
    /// not in real time, and the note was played in the frame just drawn.
    ///
    /// REAL-TIME SAFE.
    ///
    /// [`AudioProcessor::process_offline`]: super::AudioProcessor::process_offline
    pub fn collect_ui(&mut self, now: Instant, room: usize) -> &[MidiEvent] {
        self.events.clear();
        self.previous_callback = Some(now);
        if let Some(input) = self.input.as_mut() {
            while due(input, now) {
                let _ = input.pop();
            }
        }
        let room = room.min(MAX_MIDI_EVENTS_PER_CALLBACK);
        if let Some(ui) = self.ui.as_mut() {
            while self.events.len() < room && due(ui, now) {
                let Ok(stamped) = ui.pop() else { break };
                if let Some(event) = stamped.event.to_dsp(0) {
                    self.events.push(event);
                }
            }
        }
        &self.events
    }

    /// Drops the MIDI that arrived before `now`, for a callback that renders
    /// nothing (while stopped), so it isn't played late when audio resumes.
    ///
    /// REAL-TIME SAFE.
    pub fn skip(&mut self, now: Instant) {
        loop {
            let full = self.collect(now, 0).len() == MAX_MIDI_EVENTS_PER_CALLBACK;
            if !full {
                break;
            }
        }
        self.events.clear();
    }
}

/// Whether `queue`'s next event arrived by `now`.
fn due(queue: &Consumer<TimestampedMidiEvent>, now: Instant) -> bool {
    matches!(queue.peek(), Ok(next) if next.received <= now)
}

/// Takes whichever of the two queues' next events arrived first, if it
/// arrived by `now`. Each queue is in arrival order, so the two come out
/// merged in arrival order.
fn pop_earliest(
    a: &mut Option<Consumer<TimestampedMidiEvent>>,
    b: &mut Option<Consumer<TimestampedMidiEvent>>,
    now: Instant,
) -> Option<TimestampedMidiEvent> {
    let next = |queue: &Option<Consumer<TimestampedMidiEvent>>| {
        queue.as_ref().and_then(|q| q.peek().ok()).map(|e| e.received).filter(|&at| at <= now)
    };
    let queue = match (next(a), next(b)) {
        (Some(at_a), Some(at_b)) if at_b < at_a => b,
        (Some(_), _) => a,
        (None, Some(_)) => b,
        (None, None) => return None,
    };
    queue.as_mut()?.pop().ok()
}

impl Default for MidiScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Splits the events for one chunk of a buffer, `start..end`, off the front
/// of `events`, re-basing their offsets to count from `start`.
///
/// A device buffer larger than the graph's block is rendered in several
/// chunks; calling this for each chunk in turn hands every event to the
/// chunk it falls in.
pub fn take_chunk<'a>(events: &mut &'a mut [MidiEvent], start: usize, end: usize) -> &'a [MidiEvent] {
    let count = events.partition_point(|event| (event.sample_offset as usize) < end);
    let (chunk, rest) = std::mem::take(events).split_at_mut(count);
    for event in chunk.iter_mut() {
        event.sample_offset = event.sample_offset.saturating_sub(start as u32);
    }
    *events = rest;
    chunk
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rtrb::{Producer, RingBuffer};

    use super::*;
    use crate::dsp::MidiMessage;
    use crate::engine::MidiEvent as WireEvent;

    const SR: f64 = 48000.0;

    fn scheduler() -> (Producer<TimestampedMidiEvent>, MidiScheduler) {
        let (producer, consumer) = RingBuffer::new(1024);
        let mut scheduler = MidiScheduler::new();
        scheduler.set_input(consumer);
        (producer, scheduler)
    }

    fn note_on_at(received: Instant, note: u8) -> TimestampedMidiEvent {
        TimestampedMidiEvent { event: WireEvent::NoteOn { channel: 0, note, velocity: 100 }, received }
    }

    fn seconds(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    /// Runs callbacks of `frames` samples, evenly spaced in real time, and
    /// returns the absolute sample position of every event.
    fn play(scheduler: &mut MidiScheduler, base: Instant, frames: usize, callbacks: usize) -> Vec<u64> {
        let period = frames as f64 / SR;
        let mut positions = Vec::new();
        for k in 0..callbacks {
            let now = base + seconds(k as f64 * period);
            for event in scheduler.collect(now, frames) {
                positions.push((k * frames) as u64 + event.sample_offset as u64);
            }
        }
        positions
    }

    /// The largest distance from `expected` between consecutive positions.
    fn worst_spacing_error(positions: &[u64], expected: f64) -> f64 {
        positions.windows(2).map(|w| ((w[1] - w[0]) as f64 - expected).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn steady_sixteenths_stay_steady() {
        // 16th notes at 120 BPM: 125 ms, 6000 samples apart, starting at an
        // arbitrary point inside a buffer
        let (mut producer, mut scheduler) = scheduler();
        let base = Instant::now();
        for n in 0..32 {
            let at = base + seconds(0.0371 + n as f64 * 0.125);
            producer.push(note_on_at(at, 60)).unwrap();
        }

        let positions = play(&mut scheduler, base, 256, 800);
        assert_eq!(positions.len(), 32);
        let error = worst_spacing_error(&positions, 6000.0);
        assert!(error <= 1.0, "16ths drift by up to {error} samples");
    }

    #[test]
    fn odd_device_buffers_stay_steady() {
        // 441 frames is a common WASAPI buffer at 44.1/48 kHz
        let (mut producer, mut scheduler) = scheduler();
        let base = Instant::now();
        for n in 0..32 {
            producer.push(note_on_at(base + seconds(0.0123 + n as f64 * 0.125), 60)).unwrap();
        }

        let positions = play(&mut scheduler, base, 441, 500);
        assert_eq!(positions.len(), 32);
        assert!(worst_spacing_error(&positions, 6000.0) <= 1.0);
    }

    #[test]
    fn latency_is_one_callback() {
        let (mut producer, mut scheduler) = scheduler();
        let base = Instant::now();
        let period = 256.0 / SR;
        // Halfway through the interval between the first two callbacks
        producer.push(note_on_at(base + seconds(period * 0.5), 60)).unwrap();

        let positions = play(&mut scheduler, base, 256, 4);
        // Rendered by the second callback (frames 256..512), half way in
        assert_eq!(positions, vec![256 + 128]);
    }

    #[test]
    fn events_after_the_callback_began_wait_for_the_next() {
        let (mut producer, mut scheduler) = scheduler();
        let base = Instant::now();
        producer.push(note_on_at(base + seconds(0.001), 60)).unwrap();

        assert!(scheduler.collect(base, 256).is_empty());
        assert_eq!(scheduler.collect(base + seconds(0.002), 256).len(), 1);
    }

    #[test]
    fn first_callback_plays_at_the_start() {
        let (mut producer, mut scheduler) = scheduler();
        let now = Instant::now();
        producer.push(note_on_at(now - seconds(0.5), 60)).unwrap();

        let events = scheduler.collect(now, 256);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sample_offset, 0);
        assert_eq!(events[0].message, MidiMessage::NoteOn { note: 60, velocity: 100 });
    }

    #[test]
    fn offsets_never_run_backwards() {
        // An event stamped before the previous callback (it raced the
        // drain) goes to the start, still in order
        let (mut producer, mut scheduler) = scheduler();
        let base = Instant::now();
        scheduler.collect(base, 256);
        producer.push(note_on_at(base - seconds(0.001), 60)).unwrap();
        producer.push(note_on_at(base + seconds(0.002), 62)).unwrap();

        let events = scheduler.collect(base + seconds(256.0 / SR), 256);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sample_offset, 0);
        assert!(events[1].sample_offset > 0);
    }

    #[test]
    fn a_flood_is_spread_over_callbacks() {
        let (mut producer, mut scheduler) = scheduler();
        let base = Instant::now();
        for _ in 0..MAX_MIDI_EVENTS_PER_CALLBACK + 10 {
            producer.push(note_on_at(base, 60)).unwrap();
        }
        let later = base + seconds(0.01);
        assert_eq!(scheduler.collect(later, 256).len(), MAX_MIDI_EVENTS_PER_CALLBACK);
        assert_eq!(scheduler.collect(later + seconds(0.01), 256).len(), 10);
    }

    #[test]
    fn skip_drops_everything_that_arrived() {
        let (mut producer, mut scheduler) = scheduler();
        let base = Instant::now();
        for _ in 0..MAX_MIDI_EVENTS_PER_CALLBACK * 2 + 3 {
            producer.push(note_on_at(base, 60)).unwrap();
        }
        scheduler.skip(base + seconds(0.01));
        assert!(scheduler.collect(base + seconds(0.02), 256).is_empty());
    }

    #[test]
    fn ui_notes_and_device_midi_merge_by_arrival() {
        let (mut device, mut scheduler) = scheduler();
        let (mut ui, ui_input) = RingBuffer::new(64);
        scheduler.set_ui_input(ui_input);
        let base = Instant::now();
        let period = 256.0 / SR;
        scheduler.collect(base, 256);

        // A quarter, half and three quarters through the interval
        device.push(note_on_at(base + seconds(period * 0.25), 60)).unwrap();
        ui.push(note_on_at(base + seconds(period * 0.5), 64)).unwrap();
        device.push(note_on_at(base + seconds(period * 0.75), 67)).unwrap();

        let events = scheduler.collect(base + seconds(period), 256);
        let placed: Vec<(u8, u32)> = events
            .iter()
            .map(|e| match e.message {
                MidiMessage::NoteOn { note, .. } => (note, e.sample_offset),
                _ => unreachable!(),
            })
            .collect();
        // A quarter of 256 is 64, give or take the clock's rounding
        assert_eq!(placed.iter().map(|&(note, _)| note).collect::<Vec<_>>(), vec![60, 64, 67]);
        for (&(_, offset), expected) in placed.iter().zip([64, 128, 192]) {
            assert!(offset.abs_diff(expected) <= 1, "placed at {offset}, not {expected}");
        }
    }

    #[test]
    fn ui_notes_play_with_no_device() {
        let mut scheduler = MidiScheduler::new();
        let (mut ui, ui_input) = RingBuffer::new(64);
        scheduler.set_ui_input(ui_input);
        let now = Instant::now();
        ui.push(note_on_at(now - seconds(0.001), 60)).unwrap();
        assert_eq!(scheduler.collect(now, 256).len(), 1);
    }

    #[test]
    fn offline_takes_ui_notes_and_drops_device_midi() {
        let (mut device, mut scheduler) = scheduler();
        let (mut ui, ui_input) = RingBuffer::new(64);
        scheduler.set_ui_input(ui_input);
        let base = Instant::now();
        device.push(note_on_at(base, 60)).unwrap();
        for note in [62, 64, 65] {
            ui.push(note_on_at(base, note)).unwrap();
        }

        // Room for two: the third waits
        let later = base + seconds(0.01);
        let events = scheduler.collect_ui(later, 2);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.sample_offset == 0));
        let events = scheduler.collect_ui(later, 2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, MidiMessage::NoteOn { note: 65, velocity: 100 });
        assert!(scheduler.collect(later + seconds(0.01), 256).is_empty(), "device MIDI dropped");
    }

    #[test]
    fn no_input_means_no_events() {
        let mut scheduler = MidiScheduler::new();
        assert!(scheduler.collect(Instant::now(), 256).is_empty());
    }

    #[test]
    fn chunks_rebase_their_events() {
        let mut events = [
            MidiEvent::note_on(10, 0, 60, 100),
            MidiEvent::note_off(300, 0, 60, 0),
            MidiEvent::note_on(511, 0, 62, 100),
            MidiEvent::note_on(512, 0, 64, 100),
        ];
        let mut rest: &mut [MidiEvent] = &mut events;

        let first = take_chunk(&mut rest, 0, 256);
        assert_eq!(first.iter().map(|e| e.sample_offset).collect::<Vec<_>>(), vec![10]);
        let second = take_chunk(&mut rest, 256, 512);
        assert_eq!(second.iter().map(|e| e.sample_offset).collect::<Vec<_>>(), vec![44, 255]);
        let third = take_chunk(&mut rest, 512, 600);
        assert_eq!(third.iter().map(|e| e.sample_offset).collect::<Vec<_>>(), vec![0]);
        assert!(rest.is_empty());
    }
}
