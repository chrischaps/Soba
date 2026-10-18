//! The UI's side of MIDI in (#343): the inputs to choose from and the one
//! chosen, the notes held on it, which CC turns which knob, and MIDI Learn.
//! Kept apart from the frame loop, with no egui in it, so what a note or a
//! CC does can be tested.
//!
//! Notes reach Note In modules on the audio thread, sample-accurately;
//! nothing here changes what they play. The UI's copy of each event lights
//! the pianos and the MIDI Monitor, and turns mapped knobs.
//!
//! Notes played on the UI itself (the computer keys, the piano) go to the
//! audio thread on a ring of their own ([`UiNotes`]), with or without a
//! MIDI device, and are heard here as MIDI in is.

use rtrb::Consumer;

use crate::engine::{MidiDeviceInfo, MidiEngine, MidiEvent, TimestampedMidiEvent, UiNotes};
use crate::persistence::MidiMapping;
use super::midi_mappings::MidiMappings;

/// The knob MIDI Learn is waiting for a CC for.
#[derive(Debug, Clone)]
pub struct MidiLearnTarget {
    /// Engine node ID of the target parameter.
    pub node_id: u64,
    /// Parameter index within the node.
    pub param_index: usize,
    /// Parameter name for display.
    pub param_name: String,
    /// Minimum value of the parameter range.
    pub min_value: f32,
    /// Maximum value of the parameter range.
    pub max_value: f32,
}

/// What the events heard in one go asked of the UI.
#[derive(Debug, Default, PartialEq)]
pub struct Heard {
    /// A note started or stopped: the pianos show [`MidiSession::held`].
    pub notes_changed: bool,
    /// MIDI Learn took a CC, and has stopped listening.
    pub learned: bool,
    /// Knobs turned by mapped CCs: engine node, parameter index and value.
    pub turns: Vec<(u64, usize, f32)>,
}

/// MIDI in, as the UI holds it.
#[derive(Default)]
pub struct MidiSession {
    /// The MIDI inputs, or `None` where there are none to have (a DAW
    /// hands the plugin its MIDI, and a failed start leaves none).
    pub engine: Option<MidiEngine>,
    /// The UI's copy of the incoming events.
    pub events: Option<Consumer<TimestampedMidiEvent>>,
    /// The inputs to choose from, as last listed.
    pub devices: Vec<MidiDeviceInfo>,
    /// The input chosen, by its index in `devices`.
    pub selected: Option<usize>,
    /// Why MIDI can't be had, or the chosen input wouldn't open.
    pub error: Option<String>,
    /// The notes held, in order of press, each with its velocity.
    held: Vec<(u8, u8)>,
    /// Which CC or macro turns which knob.
    pub mappings: MidiMappings,
    /// The mappings as last opened or saved. Undo history doesn't hold
    /// them, so they say for themselves whether the patch has changed.
    saved_mappings: MidiMappings,
    learning: Option<MidiLearnTarget>,
    /// The ring the UI's own notes take to the audio thread.
    notes: Option<UiNotes>,
    /// Notes played on the UI since the last [`drain`](Self::drain), for
    /// it to hear with the MIDI that came in.
    played: Vec<MidiEvent>,
}

impl MidiSession {
    /// The session of `engine`, its inputs listed, or of none (`error`
    /// says why, when MIDI wouldn't start).
    pub fn new(engine: Option<MidiEngine>, events: Option<Consumer<TimestampedMidiEvent>>, error: Option<String>) -> Self {
        let mut session = Self { engine, events, error, ..Self::default() };
        session.refresh_devices();
        session
    }

    /// Lists the inputs again.
    pub fn refresh_devices(&mut self) {
        if let Some(engine) = self.engine.as_mut() {
            self.devices = engine.enumerate_devices();
        }
    }

    /// The chosen input, if one is.
    pub fn selected_device(&self) -> Option<&MidiDeviceInfo> {
        self.selected.and_then(|i| self.devices.get(i))
    }

    /// Is the chosen input open?
    pub fn is_connected(&self) -> bool {
        self.selected.is_some() && self.engine.as_ref().is_some_and(|e| e.is_connected())
    }

    /// Opens input `index` in place of the one open. The notes held on the
    /// last were let go (All Notes Off), so none are held after.
    pub fn connect(&mut self, index: usize) {
        let Some(engine) = self.engine.as_mut() else { return };
        match engine.connect(index) {
            Ok(()) => {
                self.selected = Some(index);
                self.error = None;
            }
            Err(e) => {
                // connect() drops the previous connection before trying
                self.selected = None;
                self.error = Some(e.to_string());
            }
        }
        self.held.clear();
    }

    /// Closes the input open, letting go of its notes.
    pub fn disconnect(&mut self) {
        let Some(engine) = self.engine.as_mut() else { return };
        engine.disconnect();
        self.selected = None;
        self.error = None;
        self.held.clear();
    }

    /// The notes held, in order of press, each with its velocity.
    pub fn held(&self) -> &[(u8, u8)] {
        &self.held
    }

    /// Has the UI's own notes ([`play_note`](Self::play_note)) sent on
    /// `notes`, whose other end the audio processor has.
    pub fn set_ui_notes(&mut self, notes: UiNotes) {
        self.notes = Some(notes);
    }

    /// Plays `note` (0-127), or lets it go, from the UI: a computer key or
    /// the piano. It goes to the audio thread on the UI's ring, and is
    /// heard here with the next [`drain`](Self::drain), as MIDI in is.
    pub fn play_note(&mut self, note: u8, on: bool) {
        if let Some(notes) = self.notes.as_mut() {
            self.played.push(notes.note(note, on));
        }
    }

    /// Takes every event waiting, and the notes the UI played, handing
    /// each to `each` (the MIDI Monitor's log) as it's heard.
    pub fn drain(&mut self, mut each: impl FnMut(MidiEvent)) -> Heard {
        let mut heard = Heard::default();
        if let Some(mut events) = self.events.take() {
            while let Ok(timestamped) = events.pop() {
                each(timestamped.event);
                self.hear(timestamped.event, &mut heard);
            }
            self.events = Some(events);
        }
        for event in std::mem::take(&mut self.played) {
            each(event);
            self.hear(event, &mut heard);
        }
        heard
    }

    /// What one event does: a note is held or let go, and a CC is learned,
    /// or turns the knobs mapped to it.
    pub fn hear(&mut self, event: MidiEvent, heard: &mut Heard) {
        match event {
            MidiEvent::NoteOn { note, velocity, .. } => {
                if !self.held.iter().any(|&(n, _)| n == note) {
                    self.held.push((note, velocity));
                    heard.notes_changed = true;
                }
            }
            MidiEvent::NoteOff { note, .. } => {
                if let Some(at) = self.held.iter().position(|&(n, _)| n == note) {
                    self.held.remove(at);
                    heard.notes_changed = true;
                }
            }
            MidiEvent::ControlChange { channel, controller, value } => {
                if let Some(target) = self.learning.take() {
                    // Omni: a learned CC turns the knob from any channel.
                    // The CC leaves whatever knob it turned before
                    self.mappings.learn(MidiMapping::new(
                        controller,
                        0,
                        target.node_id,
                        target.param_index,
                        target.param_name,
                        target.min_value,
                        target.max_value,
                    ));
                    heard.learned = true;
                } else {
                    for mapping in &self.mappings {
                        if mapping.matches(controller, channel) {
                            heard.turns.push((mapping.node_id, mapping.param_index, mapping.cc_to_value(value)));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Waits for a CC for `target`, in place of any knob it waited for.
    pub fn start_learn(&mut self, target: MidiLearnTarget) {
        self.learning = Some(target);
    }

    /// Stops waiting for a CC.
    pub fn cancel_learn(&mut self) {
        self.learning = None;
    }

    /// The knob MIDI Learn waits for a CC for.
    pub fn learning(&self) -> Option<&MidiLearnTarget> {
        self.learning.as_ref()
    }

    /// The patch's mappings, opened in place of these.
    pub fn open(&mut self, mappings: Vec<MidiMapping>) {
        self.mappings.load(mappings);
        self.learning = None;
    }

    /// No mappings, and nothing being learned: a new patch's.
    pub fn clear(&mut self) {
        self.mappings.clear();
        self.learning = None;
    }

    /// The mappings as they stand are the patch's file's.
    pub fn mark_saved(&mut self) {
        self.saved_mappings = self.mappings.clone();
    }

    /// Have the mappings changed since the patch was opened or saved?
    pub fn unsaved(&self) -> bool {
        self.mappings != self.saved_mappings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(node_id: u64) -> MidiLearnTarget {
        MidiLearnTarget { node_id, param_index: 0, param_name: "Cutoff".into(), min_value: 0.0, max_value: 100.0 }
    }

    fn cc(channel: u8, controller: u8, value: u8) -> MidiEvent {
        MidiEvent::ControlChange { channel, controller, value }
    }

    #[test]
    fn notes_played_here_reach_the_audio_ring_and_are_heard_with_no_device() {
        let mut session = MidiSession::default();
        let (notes, mut audio) = crate::engine::ui_notes();
        session.set_ui_notes(notes);
        session.play_note(60, true);
        session.play_note(64, true);
        session.play_note(60, false);

        let mut logged = Vec::new();
        let heard = session.drain(|event| logged.push(event));
        assert!(heard.notes_changed);
        assert_eq!(logged.len(), 3, "the MIDI Monitor logs them");
        assert_eq!(session.held(), [(64, crate::engine::UI_VELOCITY)]);
        assert_eq!(session.drain(|_| {}), Heard::default(), "heard once");

        let sent: Vec<_> = std::iter::from_fn(|| audio.pop().ok()).map(|e| e.event).collect();
        assert!(matches!(sent[..], [
            MidiEvent::NoteOn { channel: crate::dsp::KEYS_CHANNEL, note: 60, .. },
            MidiEvent::NoteOn { note: 64, .. },
            MidiEvent::NoteOff { note: 60, .. },
        ]));
    }

    fn hear(session: &mut MidiSession, events: &[MidiEvent]) -> Heard {
        let mut heard = Heard::default();
        for &event in events {
            session.hear(event, &mut heard);
        }
        heard
    }

    #[test]
    fn notes_are_held_in_order_once_each() {
        let mut session = MidiSession::default();
        let on = |note, velocity| MidiEvent::NoteOn { channel: 0, note, velocity };
        let heard = hear(&mut session, &[on(60, 100), on(64, 90), on(60, 50)]);
        assert!(heard.notes_changed);
        assert_eq!(session.held(), [(60, 100), (64, 90)]);
        let heard = hear(&mut session, &[MidiEvent::NoteOff { channel: 0, note: 60, velocity: 0 }]);
        assert!(heard.notes_changed);
        assert_eq!(session.held(), [(64, 90)]);
        // Letting go of a note not held changes nothing
        assert!(!hear(&mut session, &[MidiEvent::NoteOff { channel: 0, note: 1, velocity: 0 }]).notes_changed);
    }

    #[test]
    fn learning_takes_the_next_cc_and_then_it_turns_the_knob() {
        let mut session = MidiSession::default();
        session.start_learn(target(7));
        let heard = hear(&mut session, &[cc(3, 74, 0)]);
        assert!(heard.learned);
        assert!(heard.turns.is_empty());
        assert!(session.learning().is_none());
        assert!(session.unsaved());

        // Learned omni: any channel turns it, across the parameter's range
        let heard = hear(&mut session, &[cc(9, 74, 127), cc(0, 75, 64)]);
        assert_eq!(heard.turns, [(7, 0, 100.0)]);

        session.mark_saved();
        assert!(!session.unsaved());
    }

    #[test]
    fn a_cancelled_learn_learns_nothing_and_a_new_patch_forgets() {
        let mut session = MidiSession::default();
        session.start_learn(target(1));
        session.cancel_learn();
        assert_eq!(hear(&mut session, &[cc(0, 10, 64)]), Heard::default());

        session.start_learn(target(1));
        hear(&mut session, &[cc(0, 10, 64)]);
        session.start_learn(target(2));
        session.clear();
        assert!(session.learning().is_none());
        assert!(hear(&mut session, &[cc(0, 10, 64)]).turns.is_empty());
    }

    #[test]
    fn drain_hands_every_event_to_the_monitor() {
        let (mut producer, consumer) = rtrb::RingBuffer::new(8);
        let mut session = MidiSession::new(None, Some(consumer), None);
        for event in [MidiEvent::NoteOn { channel: 0, note: 60, velocity: 1 }, cc(0, 1, 2)] {
            producer.push(TimestampedMidiEvent { event, received: web_time::Instant::now() }).unwrap();
        }
        let mut logged = Vec::new();
        let heard = session.drain(|event| logged.push(event));
        assert_eq!(logged.len(), 2);
        assert!(heard.notes_changed);
        assert!(session.events.is_some());
    }
}
