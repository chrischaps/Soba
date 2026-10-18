//! Notes played on the UI thread, on their own way to the audio thread.
//!
//! The computer keys and the playable piano make MIDI of their own. It
//! doesn't go through the [`MidiEngine`](super::MidiEngine), which the
//! plugin hasn't got and the browser may not have yet: it has a ring of its
//! own, which the [`MidiScheduler`](super::MidiScheduler) merges with device
//! MIDI (or, in the plugin, the host's), placing each note by when it was
//! played, as it places device input.

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::KEYS_CHANNEL;

use super::midi_engine::{MidiEvent, TimestampedMidiEvent};

/// How hard UI-made notes are sent. Note In plays them at its own Key
/// Velocity instead; this is what the MIDI Monitor shows.
pub const UI_VELOCITY: u8 = 100;

/// Room for a burst of keys, a chord or two, with plenty to spare.
const UI_NOTES_CAPACITY: usize = 256;

/// The UI's end of the ring.
pub struct UiNotes {
    producer: Producer<TimestampedMidiEvent>,
}

/// A ring for UI-made notes: the app's end, and the end for the
/// [`AudioProcessor`](super::AudioProcessor)'s
/// [`set_ui_midi_input`](super::AudioProcessor::set_ui_midi_input).
pub fn ui_notes() -> (UiNotes, Consumer<TimestampedMidiEvent>) {
    let (producer, consumer) = RingBuffer::new(UI_NOTES_CAPACITY);
    (UiNotes { producer }, consumer)
}

impl UiNotes {
    /// Plays `note` (0-127), or lets it go, on [`KEYS_CHANNEL`] at
    /// [`UI_VELOCITY`]. Returns the event sent.
    pub fn note(&mut self, note: u8, on: bool) -> MidiEvent {
        let event = if on {
            MidiEvent::NoteOn { channel: KEYS_CHANNEL, note, velocity: UI_VELOCITY }
        } else {
            MidiEvent::NoteOff { channel: KEYS_CHANNEL, note, velocity: 0 }
        };
        self.send(event);
        event
    }

    /// Sends `event`, stamped now. Lossy, like device input: a full ring
    /// drops it.
    pub fn send(&mut self, event: MidiEvent) {
        let _ = self.producer.push(TimestampedMidiEvent::now(event));
    }
}
