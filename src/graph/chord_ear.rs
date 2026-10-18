//! The chord popover's ear: the notes held on a MIDI keyboard or the
//! computer's letter keys while it's open, gathered until the hand lifts.
//!
//! A hand doesn't land or lift all at once, so every note held since the
//! first went down counts, and the chord is heard when the last comes up.
//! How hard a MIDI keyboard's notes were struck is heard too, so a chord's
//! accent goes in with it; the letter keys have no touch to hear.
//!
//! While the popover is open the letter keys are its, the way a text field
//! takes them: the Note In modules don't hear them (see [`letter_keys_taken`]),
//! so writing a chord doesn't also play the patch's lead. They play the
//! popover's piano, Z on its lowest C, so its ‹ › move them an octave too,
//! and the chord sounds through the sequencer's own voices while it's held.
//! A MIDI keyboard plays through to the patch as always.

use eframe::egui::{self, Id, Key};

use crate::app::qwerty_keys::key_to_note;

/// What the popover hears.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ChordEar {
    /// Letter keys held, with the note each plays.
    keys: Vec<(Key, u8)>,
    /// Every note held since the first went down, lowest first.
    heard: Vec<u8>,
    /// Each MIDI note among them, with how hard it was struck when it was
    /// first heard.
    struck: Vec<(u8, u8)>,
    /// Whether a letter key is among them.
    keyed: bool,
}

/// A chord played: every note held, lowest first, and how hard the MIDI
/// keyboard's were struck on average, if any were.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Hand {
    pub notes: Vec<u8>,
    pub velocity: Option<u8>,
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

    /// The notes held now, on the letter keys and the MIDI keyboard (`midi`,
    /// each note with its velocity).
    pub fn held(&self, midi: &[(u8, u8)]) -> Vec<u8> {
        let mut held: Vec<u8> = self.keys.iter().map(|&(_, note)| note).chain(midi.iter().map(|&(note, _)| note)).collect();
        held.sort_unstable();
        held.dedup();
        held
    }

    /// Takes in what's held now. Once the last note comes up, gives back
    /// every note held since the first went down.
    pub fn listen(&mut self, midi: &[(u8, u8)]) -> Option<Hand> {
        let held = self.held(midi);
        if held.is_empty() {
            let velocity = self.velocity();
            self.keyed = false;
            self.struck.clear();
            return (!self.heard.is_empty()).then(|| Hand { notes: std::mem::take(&mut self.heard), velocity });
        }
        self.keyed |= !self.keys.is_empty();
        for &(note, velocity) in midi {
            if !self.struck.iter().any(|&(n, _)| n == note) {
                self.struck.push((note, velocity));
            }
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

    /// How hard the chord being gathered was struck: the mean velocity of
    /// its MIDI notes, or `None` if it's all on the letter keys.
    pub fn velocity(&self) -> Option<u8> {
        let count = self.struck.len() as u32;
        let sum: u32 = self.struck.iter().map(|&(_, velocity)| velocity as u32).sum();
        (count > 0).then(|| ((sum + count / 2) / count).clamp(1, 127) as u8)
    }

    /// Whether the chord being gathered was played, in part, on the letter
    /// keys, which nothing else in the patch hears while the popover's open.
    pub fn keyed(&self) -> bool {
        self.keyed
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
/// the Note In modules shouldn't hear them. Asked once a frame, before the
/// graph is drawn: the popover marks them again while it's still open.
pub fn letter_keys_taken(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(taken_id())).unwrap_or(false)
}

/// Whether a chord popover has the letter keys, as [`letter_keys_taken`]
/// asks, but leaving the answer for it.
pub fn letter_keys_held(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(taken_id())).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// MIDI notes held, all struck at velocity 100.
    fn midi(notes: &[u8]) -> Vec<(u8, u8)> {
        notes.iter().map(|&note| (note, 100)).collect()
    }

    fn hand(notes: &[u8], velocity: Option<u8>) -> Option<Hand> {
        Some(Hand { notes: notes.to_vec(), velocity })
    }

    #[test]
    fn a_chord_is_heard_when_the_last_note_lifts() {
        let mut ear = ChordEar::default();
        // A2 C3 E3 on the MIDI keyboard, landing one after another
        assert_eq!(ear.listen(&midi(&[45])), None);
        assert_eq!(ear.listen(&midi(&[45, 48, 52])), None);
        // ...and lifting one after another
        assert_eq!(ear.listen(&midi(&[48])), None);
        assert_eq!(ear.listen(&[]), hand(&[45, 48, 52], Some(100)));
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
        assert_eq!(ear.listen(&[]), hand(&[48, 52, 55], None), "the letter keys have no touch");
    }

    #[test]
    fn a_chord_is_as_loud_as_its_notes_on_average() {
        let mut ear = ChordEar::default();
        // A soft bass, then the chord over it struck hard
        ear.listen(&[(36, 40)]);
        assert_eq!(ear.velocity(), Some(40));
        ear.listen(&[(36, 40), (60, 110), (64, 120)]);
        // The bass lifts and is struck again, harder: it was heard at 40
        ear.listen(&[(60, 110), (64, 120)]);
        ear.listen(&[(36, 127), (60, 110), (64, 120)]);
        assert_eq!(ear.velocity(), Some(90));
        assert_eq!(ear.listen(&[]), hand(&[36, 60, 64], Some(90)));
        // The next chord starts afresh
        ear.listen(&[(62, 64)]);
        assert_eq!(ear.velocity(), Some(64));
    }

    #[test]
    fn letter_keys_and_midi_make_one_chord() {
        let mut ear = ChordEar::default();
        ear.follow_keys(&[key(Key::Z, true)], 36);
        assert_eq!(ear.listen(&[(64, 80), (67, 90)]), None);
        ear.follow_keys(&[key(Key::Z, false)], 36);
        // Only the MIDI notes were struck with a touch
        assert_eq!(ear.listen(&[]), hand(&[36, 64, 67], Some(85)));
    }

    #[test]
    fn the_ear_knows_a_chord_played_on_the_letter_keys() {
        let mut ear = ChordEar::default();
        ear.listen(&midi(&[60, 64]));
        assert!(!ear.keyed(), "MIDI alone");
        // A letter key joins, and lifts before the MIDI notes do
        ear.follow_keys(&[key(Key::B, true)], 60);
        ear.listen(&midi(&[60, 64]));
        ear.follow_keys(&[key(Key::B, false)], 60);
        ear.listen(&midi(&[60, 64]));
        assert!(ear.keyed(), "still part of the chord being gathered");
        assert_eq!(ear.listen(&[]), hand(&[60, 64, 67], Some(100)));
        assert!(!ear.keyed(), "the next chord starts afresh");
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
