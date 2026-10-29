//! Learning into a module (#427): MIDI Learn that teaches a module what to
//! hear, rather than mapping a CC onto one of its knobs.
//!
//! A Controls slot hears one fader, knob or wheel, and a Pad (#480) one
//! pad. Which one lives in the module's own hidden parameters (a kind,
//! a number, a channel), so the module can read MIDI on the audio thread,
//! sample-accurately, and the patch keeps what it learned. Learning one is
//! MIDI Learn as every control has it: right-click > Learn, a blinking
//! badge, the status bar asking for a move, Escape to give up. But when
//! the next event comes, nothing is mapped. What was heard is written into
//! the parameters a [`ModuleLearn`] names, through `SetParameter`, as an
//! edit that undo takes back.
//!
//! # Using it from a module's face
//!
//! Start the learn with [`SynthResponse::MidiLearnStart`], `into` set:
//!
//! ```ignore
//! SynthResponse::MidiLearnStart {
//!     engine_node_id,
//!     // The parameter the face lights while it waits: by convention the
//!     // first one the learn writes. `is_midi_learn_target` checks it
//!     param_index: Pad::PARAM_NOTE,
//!     param_name: "Pad on C2".into(), // the status bar asks a move "for" it
//!     min_value: 0.0,
//!     max_value: 1.0,
//!     control: MidiControl::Button,
//!     scene: None,
//!     into: Some(Box::new(ModuleLearn {
//!         kinds: LearnKinds { note: Some(0.0), ..LearnKinds::NONE },
//!         kind: None,
//!         number: Some(Pad::PARAM_NOTE),
//!         channel: Some(Pad::PARAM_CHANNEL),
//!         encoding: None,
//!     })),
//! }
//! ```
//!
//! The face blinks while `user_state.is_midi_learn_target(engine_node_id,
//! param_index)`, offers "Cancel MIDI Learn" then (the response
//! `MidiLearnCancel`), and reads its parameters back to show what it hears.
//! [`MidiSession::hear`](super::midi_session::MidiSession::hear) does the
//! rest: events of a kind the learn doesn't take pass by as they would
//! anyway (a note still plays while a fader is learned), and the first one
//! it does take ends the learn.
//!
//! [`SynthResponse::MidiLearnStart`]: crate::graph::SynthResponse::MidiLearnStart

use crate::dsp::KEYS_CHANNEL;
use crate::engine::MidiEvent;
use crate::persistence::MidiEncoding;

/// The value a learn writes to a module's encoding parameter for what a CC
/// turned out to be: its place in a choice of Absolute, Relative64 and
/// TwosComplement, in that order.
pub fn encoding_value(encoding: MidiEncoding) -> f32 {
    match encoding {
        MidiEncoding::Absolute => 0.0,
        MidiEncoding::Relative64 => 1.0,
        MidiEncoding::TwosComplement => 2.0,
    }
}

/// The value a learn writes to the module's kind parameter for each kind of
/// message it hears, or `None` for a kind it doesn't learn.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LearnKinds {
    /// A control change: a fader, knob, encoder or pad sending CCs.
    pub cc: Option<f32>,
    /// The pitch bend wheel or strip.
    pub bend: Option<f32>,
    /// Channel pressure (aftertouch).
    pub pressure: Option<f32>,
    /// A note struck: a key or a pad.
    pub note: Option<f32>,
}

impl LearnKinds {
    /// Learns nothing: the start of a `..LearnKinds::NONE`.
    pub const NONE: LearnKinds = LearnKinds { cc: None, bend: None, pressure: None, note: None };
}

/// Where a learn into a module writes what it hears: each field names one
/// of the module's parameters by index, or `None` to leave it be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleLearn {
    /// The kinds of message it learns, and what each writes to `kind`.
    pub kinds: LearnKinds,
    /// The parameter told what kind of message was heard.
    pub kind: Option<usize>,
    /// The parameter given the CC's number or the note's (0-127). A bend
    /// or pressure has none, and leaves it be.
    pub number: Option<usize>,
    /// The parameter given the channel it came on, 1-16 (a Channel choice
    /// whose 0 is Omni).
    pub channel: Option<usize>,
    /// The parameter given what a CC turned out to be, as
    /// [`encoding_value`] has it: set to Absolute when the CC is heard, and
    /// to an encoder's once the next few values say it's one.
    pub encoding: Option<usize>,
}

impl ModuleLearn {
    /// The parameters `event` writes, and their values, if it's one this
    /// learns. The computer keys and the piano aren't learned: they're not
    /// on a MIDI channel for a module to listen to.
    pub fn hear(&self, event: &MidiEvent) -> Option<Vec<(usize, f32)>> {
        let (kind, number, channel) = match *event {
            MidiEvent::ControlChange { channel, controller, .. } => (self.kinds.cc?, Some(controller), channel),
            MidiEvent::PitchBend { channel, .. } => (self.kinds.bend?, None, channel),
            MidiEvent::ChannelPressure { channel, .. } => (self.kinds.pressure?, None, channel),
            MidiEvent::NoteOn { channel, note, velocity } if velocity > 0 => (self.kinds.note?, Some(note), channel),
            _ => return None,
        };
        if channel == KEYS_CHANNEL || channel > 15 {
            return None;
        }
        let mut writes = Vec::with_capacity(4);
        writes.extend(self.kind.map(|param| (param, kind)));
        if let (Some(param), Some(number)) = (self.number, number) {
            writes.push((param, number as f32));
        }
        writes.extend(self.channel.map(|param| (param, (channel + 1) as f32)));
        if matches!(event, MidiEvent::ControlChange { .. }) {
            writes.extend(self.encoding.map(|param| (param, 0.0)));
        }
        Some(writes)
    }

    /// What the status bar asks for while it waits: "Move a knob or fader,
    /// or bend", say.
    pub fn prompt(&self) -> String {
        let asks: Vec<&str> = [
            (self.kinds.cc, "move a knob or fader"),
            (self.kinds.note, "hit a pad or key"),
            (self.kinds.bend, "bend"),
            (self.kinds.pressure, "press a key down"),
        ]
        .into_iter()
        .filter_map(|(kind, ask)| kind.map(|_| ask))
        .collect();
        let text = match asks.as_slice() {
            [] => String::new(),
            [one] => one.to_string(),
            [rest @ .., last] => format!("{}, or {}", rest.join(", "), last),
        };
        let mut chars = text.chars();
        chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Controls slot's learn: CCs, bend and pressure, each a kind.
    fn controls() -> ModuleLearn {
        ModuleLearn {
            kinds: LearnKinds { cc: Some(0.0), bend: Some(1.0), pressure: Some(3.0), ..LearnKinds::NONE },
            kind: Some(10),
            number: Some(11),
            channel: Some(12),
            encoding: Some(13),
        }
    }

    #[test]
    fn a_cc_writes_its_kind_number_channel_and_a_fresh_encoding() {
        let heard = controls().hear(&MidiEvent::ControlChange { channel: 2, controller: 74, value: 9 });
        assert_eq!(heard, Some(vec![(10, 0.0), (11, 74.0), (12, 3.0), (13, 0.0)]));
    }

    #[test]
    fn a_bend_leaves_the_number_alone() {
        let heard = controls().hear(&MidiEvent::PitchBend { channel: 0, value: 100 });
        assert_eq!(heard, Some(vec![(10, 1.0), (12, 1.0)]));
    }

    #[test]
    fn kinds_it_doesnt_learn_and_the_keys_pass_by() {
        let learn = controls();
        assert_eq!(learn.hear(&MidiEvent::NoteOn { channel: 0, note: 60, velocity: 100 }), None);
        assert_eq!(learn.hear(&MidiEvent::ControlChange { channel: KEYS_CHANNEL, controller: 1, value: 0 }), None);

        // A pad's learn takes a note, never its release
        let pad = ModuleLearn { kinds: LearnKinds { note: Some(0.0), ..LearnKinds::NONE }, kind: None, number: Some(3), channel: Some(4), encoding: None };
        assert_eq!(pad.hear(&MidiEvent::NoteOn { channel: 9, note: 36, velocity: 0 }), None);
        assert_eq!(pad.hear(&MidiEvent::NoteOff { channel: 9, note: 36, velocity: 0 }), None);
        assert_eq!(pad.hear(&MidiEvent::NoteOn { channel: 9, note: 36, velocity: 90 }), Some(vec![(3, 36.0), (4, 10.0)]));
        assert_eq!(pad.hear(&MidiEvent::ControlChange { channel: 9, controller: 36, value: 9 }), None);
    }

    #[test]
    fn encodings_are_written_as_controls_reads_them() {
        use crate::modules::controls::encoding_from_param;
        for encoding in [MidiEncoding::Absolute, MidiEncoding::Relative64, MidiEncoding::TwosComplement] {
            assert_eq!(encoding_from_param(encoding_value(encoding)), encoding);
        }
    }

    #[test]
    fn the_prompt_asks_for_what_it_learns() {
        assert_eq!(controls().prompt(), "Move a knob or fader, bend, or press a key down");
        let pad = ModuleLearn { kinds: LearnKinds { note: Some(0.0), ..LearnKinds::NONE }, kind: None, number: None, channel: None, encoding: None };
        assert_eq!(pad.prompt(), "Hit a pad or key");
    }
}
