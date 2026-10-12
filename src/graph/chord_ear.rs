//! The chord popover's ear: the notes held on a MIDI keyboard or the
//! computer's letter keys while it's open, gathered until the hand lifts.
//!
//! A hand doesn't land or lift all at once, so every note held since the
//! first went down counts, and the chord is heard when the last comes up.
//!
//! While the popover is open the letter keys are its, the way a text field
//! takes them: the Keyboard modules don't hear them (see [`letter_keys_taken`]),
//! so writing a chord doesn't also play the patch's lead. They play the
//! popover's piano, Z on its lowest C, so its ‹ › move them an octave too.
//! A MIDI keyboard plays through to the patch as always.

use eframe::egui::{self, Id, Key};

use crate::modules::keyboard::key_to_note;

/// What the popover hears.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ChordEar {
    /// Letter keys held, with the note each plays.
    keys: Vec<(Key, u8)>,
    /// Every note held since the first went down, lowest first.
    heard: Vec<u8>,
}

impl ChordEar {
    /// Follows the letter keys in a frame's raw input. Z plays `base`. A key
    /// already down when the popover opened isn't heard, nor are keys held
    /// with Ctrl, Alt or Cmd, which are shortcuts.
    pub fn follow_keys(&mut self, events: &[egui::Event], base: u8) {
        for event in events {
            let egui::Event::Key { key, pressed, repeat: false, modifiers, .. } = event else { continue };
            let Some(relative) = key_to_note(*key) else { continue };
            let shortcut = modifiers.ctrl || modifiers.alt || modifiers.command;
            let down = self.keys.iter().any(|(k, _)| k == key);
            if !*pressed {
                self.keys.retain(|(k, _)| k != key);
            } else if !(shortcut || down) {
                self.keys.push((*key, (base as i32 + relative).clamp(0, 127) as u8));
            }
        }
    }

    /// The notes held now, on the letter keys and the MIDI keyboard (`midi`).
    pub fn held(&self, midi: &[u8]) -> Vec<u8> {
        let mut held: Vec<u8> = self.keys.iter().map(|&(_, note)| note).chain(midi.iter().copied()).collect();
        held.sort_unstable();
        held.dedup();
        held
    }

    /// Takes in what's held now. Once the last note comes up, gives back
    /// every note held since the first went down.
    pub fn listen(&mut self, midi: &[u8]) -> Option<Vec<u8>> {
        let held = self.held(midi);
        if held.is_empty() {
            return (!self.heard.is_empty()).then(|| std::mem::take(&mut self.heard));
        }
        self.heard.extend(held);
        self.heard.sort_unstable();
        self.heard.dedup();
        None
    }

    /// The notes gathered so far, while a chord is being played.
    pub fn gathering(&self) -> &[u8] {
        &self.heard
    }
}

fn taken_id() -> Id {
    Id::new("chord_popover_takes_letter_keys")
}

/// Marks the letter keys as the open popover's for the next frame.
pub(super) fn take_letter_keys(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(taken_id(), true));
}

/// Whether a chord popover was open last frame and has the letter keys, so
/// the Keyboard modules shouldn't hear them. Asked once a frame, before the
/// graph is drawn: the popover marks them again while it's still open.
pub fn letter_keys_taken(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(taken_id())).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn a_chord_is_heard_when_the_last_note_lifts() {
        let mut ear = ChordEar::default();
        // A2 C3 E3 on the MIDI keyboard, landing one after another
        assert_eq!(ear.listen(&[45]), None);
        assert_eq!(ear.listen(&[45, 48, 52]), None);
        // ...and lifting one after another
        assert_eq!(ear.listen(&[48]), None);
        assert_eq!(ear.listen(&[]), Some(vec![45, 48, 52]));
        // Nothing more until the next chord
        assert_eq!(ear.listen(&[]), None);
    }

    #[test]
    fn the_letter_keys_play_the_popover_piano() {
        let mut ear = ChordEar::default();
        // Z C B: C E G from the piano's lowest C, C3
        ear.follow_keys(&[key(Key::Z, true), key(Key::C, true), key(Key::B, true)], 48);
        assert_eq!(ear.held(&[]), vec![48, 52, 55]);
        assert_eq!(ear.listen(&[]), None);
        ear.follow_keys(&[key(Key::Z, false), key(Key::C, false)], 48);
        assert_eq!(ear.listen(&[]), None);
        ear.follow_keys(&[key(Key::B, false)], 48);
        assert_eq!(ear.listen(&[]), Some(vec![48, 52, 55]));
    }

    #[test]
    fn letter_keys_and_midi_make_one_chord() {
        let mut ear = ChordEar::default();
        ear.follow_keys(&[key(Key::Z, true)], 36);
        assert_eq!(ear.listen(&[64, 67]), None);
        ear.follow_keys(&[key(Key::Z, false)], 36);
        assert_eq!(ear.listen(&[]), Some(vec![36, 64, 67]));
    }

    #[test]
    fn shortcuts_and_repeats_arent_notes() {
        let mut ear = ChordEar::default();
        let ctrl_z = egui::Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::CTRL };
        let repeat = egui::Event::Key { key: Key::X, physical_key: None, pressed: true, repeat: true, modifiers: egui::Modifiers::NONE };
        ear.follow_keys(&[ctrl_z, repeat, key(Key::ArrowLeft, true)], 48);
        assert_eq!(ear.held(&[]), Vec::<u8>::new());
        // A release of a key it never saw go down changes nothing
        ear.follow_keys(&[key(Key::M, false)], 48);
        assert_eq!(ear.listen(&[]), None);
    }
}
