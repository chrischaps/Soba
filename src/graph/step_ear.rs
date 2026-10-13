//! The Step Sequencer piano's ear: notes played on a MIDI keyboard or the
//! computer's letter keys while it's open, each written into its step the
//! moment it's struck, the way a hardware sequencer's step-record takes a
//! melody. Nothing is gathered, as the chord popover gathers a hand: every
//! note-on is one step.
//!
//! A MIDI keyboard's touch goes into the step's Velocity. The letter keys
//! have none, so they leave it as it was. A note still held when the next
//! one is struck is played legato, and ties into it; one let go first
//! doesn't. So the tie is written with the phrasing, as hardware's legato
//! step-record writes it.
//!
//! While the piano's open the letter keys are its, as they are the chord
//! popover's (see [`super::chord_ear`]), Z on its lowest C. The Keyboard
//! modules don't hear them, so the sequencer sounds the note under the hand
//! itself (see [`StepEar::sounding`]). A MIDI keyboard plays through to the
//! patch as always, so a note struck there is heard once, by the patch, and
//! written; only with no MIDI module in the patch does the sequencer sound it.

use eframe::egui::{self, Key};

use crate::modules::keyboard::key_to_note;

/// A note struck on the piano's keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Strike {
    pub note: u8,
    /// How hard a MIDI keyboard struck it; the letter keys have no touch.
    pub velocity: Option<u8>,
    /// Whether the note struck before it was still held.
    pub legato: bool,
}

/// What the piano hears.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct StepEar {
    /// Letter keys held, with the note each plays.
    keys: Vec<(Key, u8)>,
    /// MIDI notes struck since the piano opened, and still held.
    midi: Vec<u8>,
    /// How many MIDI notes had been struck or let go when last looked.
    seen: u64,
    /// The note struck last.
    last: Option<u8>,
    /// The notes struck and still held, in the order they were, each with
    /// its touch (`None` for a letter key).
    down: Vec<(u8, Option<u8>)>,
}

impl StepEar {
    /// An ear that hears the MIDI notes after the `seen`th, so a note held
    /// as the piano opened isn't written.
    pub fn new(seen: u64) -> Self {
        Self { seen, ..Self::default() }
    }

    /// How many MIDI notes the ear has looked at, to ask for the ones after.
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// Hears a frame's letter keys (Z plays `base`) and the MIDI notes struck
    /// and let go since it last looked (`midi`, each with its velocity, 0 for
    /// a release; the count is now `count`). Gives back the notes struck, in
    /// the order they were.
    pub fn hear(&mut self, events: &[egui::Event], base: u8, midi: &[(u8, u8)], count: u64) -> Vec<Strike> {
        let mut struck = Vec::new();
        for event in events {
            let egui::Event::Key { key, pressed, repeat: false, modifiers, .. } = event else { continue };
            let Some(relative) = key_to_note(*key) else { continue };
            let shortcut = modifiers.ctrl || modifiers.alt || modifiers.command;
            let down = self.keys.iter().any(|(k, _)| k == key);
            if !*pressed {
                self.keys.retain(|(k, _)| k != key);
                self.lifted();
            } else if !(shortcut || down) {
                let note = (base as i32 + relative).clamp(0, 127) as u8;
                struck.push(self.strike(note, None));
                self.keys.push((*key, note));
            }
        }
        for &(note, velocity) in midi {
            if velocity == 0 {
                self.midi.retain(|&n| n != note);
                self.lifted();
            } else if !self.midi.contains(&note) {
                struck.push(self.strike(note, Some(velocity)));
                self.midi.push(note);
            }
        }
        self.seen = count;
        struck
    }

    /// The note under the hand: the one struck last of those still held, on
    /// the letter keys, or on a MIDI keyboard too if `midi` (when nothing
    /// else in the patch plays it). With its touch, if it has one.
    pub fn sounding(&self, midi: bool) -> Option<(u8, Option<u8>)> {
        self.down.iter().rev().find(|(_, velocity)| velocity.is_none() || midi).copied()
    }

    fn strike(&mut self, note: u8, velocity: Option<u8>) -> Strike {
        let legato = self.last.is_some_and(|last| last != note && self.held(last));
        self.last = Some(note);
        self.down.push((note, velocity));
        Strike { note, velocity, legato }
    }

    /// Forgets the notes no longer held.
    fn lifted(&mut self) {
        let (keys, midi) = (&self.keys, &self.midi);
        self.down.retain(|&(note, velocity)| match velocity {
            None => keys.iter().any(|&(_, n)| n == note),
            Some(_) => midi.contains(&note),
        });
    }

    fn held(&self, note: u8) -> bool {
        self.midi.contains(&note) || self.keys.iter().any(|&(_, n)| n == note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    fn strike(note: u8, velocity: Option<u8>, legato: bool) -> Strike {
        Strike { note, velocity, legato }
    }

    #[test]
    fn every_note_on_is_struck_with_its_touch() {
        let mut ear = StepEar::new(0);
        // C4 soft, let go, then E4 hard, both landing between two frames
        let struck = ear.hear(&[], 48, &[(60, 40), (60, 0), (64, 110)], 3);
        assert_eq!(struck, vec![strike(60, Some(40), false), strike(64, Some(110), false)]);
        assert_eq!(ear.seen(), 3);
        // A release is no note
        assert_eq!(ear.hear(&[], 48, &[(64, 0)], 4), vec![]);
    }

    #[test]
    fn a_note_held_into_the_next_is_legato() {
        let mut ear = StepEar::new(0);
        ear.hear(&[], 48, &[(60, 90)], 1);
        // E4 lands before C4 lifts
        assert_eq!(ear.hear(&[], 48, &[(64, 90), (60, 0)], 3), vec![strike(64, Some(90), true)]);
        // G4 after E4 has gone
        assert_eq!(ear.hear(&[], 48, &[(64, 0), (67, 90)], 5), vec![strike(67, Some(90), false)]);
    }

    #[test]
    fn the_letter_keys_play_from_the_pianos_lowest_c() {
        let mut ear = StepEar::new(0);
        // Z then B held over it: C3, then G3 legato, with no touch to write
        let struck = ear.hear(&[key(Key::Z, true), key(Key::B, true)], 48, &[], 0);
        assert_eq!(struck, vec![strike(48, None, false), strike(55, None, true)]);
        // Both lifted, then Z again: struck afresh, and not legato
        ear.hear(&[key(Key::Z, false), key(Key::B, false)], 48, &[], 0);
        assert_eq!(ear.hear(&[key(Key::Z, true)], 48, &[], 0), vec![strike(48, None, false)]);
    }

    #[test]
    fn shortcuts_repeats_and_held_keys_arent_notes() {
        let mut ear = StepEar::new(0);
        let ctrl_z = egui::Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::CTRL };
        let repeat = egui::Event::Key { key: Key::X, physical_key: None, pressed: true, repeat: true, modifiers: egui::Modifiers::NONE };
        assert_eq!(ear.hear(&[ctrl_z, repeat, key(Key::ArrowLeft, true)], 48, &[], 0), vec![]);
        // A key's second press without a release, as some keyboards send
        ear.hear(&[key(Key::M, true)], 48, &[], 0);
        assert_eq!(ear.hear(&[key(Key::M, true)], 48, &[], 0), vec![]);
    }

    #[test]
    fn the_note_sounding_is_the_last_one_struck_still_held() {
        let mut ear = StepEar::new(0);
        assert_eq!(ear.sounding(true), None);
        ear.hear(&[key(Key::Z, true)], 48, &[], 0);
        assert_eq!(ear.sounding(false), Some((48, None)));
        // B over it, then lifted: back to the Z still held
        ear.hear(&[key(Key::B, true)], 48, &[], 0);
        assert_eq!(ear.sounding(false), Some((55, None)));
        ear.hear(&[key(Key::B, false)], 48, &[], 0);
        assert_eq!(ear.sounding(false), Some((48, None)));
        ear.hear(&[key(Key::Z, false)], 48, &[], 0);
        assert_eq!(ear.sounding(false), None);
    }

    #[test]
    fn a_midi_note_sounds_only_when_nothing_else_plays_it() {
        let mut ear = StepEar::new(0);
        ear.hear(&[], 48, &[(62, 90)], 1);
        assert_eq!(ear.sounding(false), None, "the patch's MIDI module plays it");
        assert_eq!(ear.sounding(true), Some((62, Some(90))));
        // A letter key over it is the sequencer's either way
        ear.hear(&[key(Key::Z, true)], 48, &[], 1);
        assert_eq!(ear.sounding(false), Some((48, None)));
        ear.hear(&[key(Key::Z, false)], 48, &[(62, 0)], 2);
        assert_eq!(ear.sounding(true), None);
    }

    #[test]
    fn a_note_held_as_the_piano_opened_isnt_heard_until_struck_again() {
        // Two notes went by before it opened
        let mut ear = StepEar::new(2);
        // Its release arrives first: it was never struck here, so nothing ties from it
        ear.hear(&[], 48, &[(50, 0)], 3);
        assert_eq!(ear.hear(&[], 48, &[(62, 80)], 4), vec![strike(62, Some(80), false)]);
    }
}
