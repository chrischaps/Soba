//! The computer keys held as notes (#316), kept apart from the frame loop so
//! the rules for when a key counts can be tested.
//!
//! A press counts unless something else has the keys: a modifier (it's a
//! shortcut), a text field or a chord popover. A release always counts, so a
//! key let go while any of those took over doesn't stick, and losing the
//! window lets go of every key, since its releases go to whichever window
//! took the focus.

use crate::modules::keyboard::key_to_note;

/// One held note: its semitone above the keyboard's C, and the computer key
/// holding it, or `None` for a piano key held with the pointer.
pub type HeldKey = (i32, Option<egui::Key>);

/// Apply one raw key event to `held`, in press order. Returns the note that
/// started or stopped, with `true` for a press, or `None` if nothing changed.
/// `taken` says something else has the keys, which turns presses away.
pub fn key_event(held: &mut Vec<HeldKey>, key: egui::Key, pressed: bool, taken: bool) -> Option<(i32, bool)> {
    let note = key_to_note(key)?;
    let at = held.iter().position(|(_, k)| *k == Some(key));
    match (pressed, at) {
        (true, None) if !taken => {
            held.push((note, Some(key)));
            Some((note, true))
        }
        (false, Some(at)) => Some((held.remove(at).0, false)),
        _ => None,
    }
}

/// Let go of every computer key (the pointer's piano key isn't one), as when
/// the window loses the focus. Returns the notes that stopped.
pub fn release_all(held: &mut Vec<HeldKey>) -> Vec<(i32, bool)> {
    let mut released = Vec::new();
    held.retain(|&(note, key)| {
        let keep = key.is_none();
        if !keep {
            released.push((note, false));
        }
        keep
    });
    released
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    #[test]
    fn release_counts_while_a_modifier_is_held() {
        let mut held = Vec::new();
        assert_eq!(key_event(&mut held, Key::Z, true, false), Some((0, true)));
        // Alt goes down, then Z comes up: the release still lands
        assert_eq!(key_event(&mut held, Key::Z, false, true), Some((0, false)));
        assert!(held.is_empty());
    }

    #[test]
    fn press_is_turned_away_while_taken() {
        let mut held = Vec::new();
        assert_eq!(key_event(&mut held, Key::Z, true, true), None);
        assert!(held.is_empty());
        // ...and its release, with nothing held, changes nothing
        assert_eq!(key_event(&mut held, Key::Z, false, false), None);
    }

    #[test]
    fn a_second_press_of_a_held_key_is_ignored() {
        let mut held = Vec::new();
        key_event(&mut held, Key::Z, true, false);
        assert_eq!(key_event(&mut held, Key::Z, true, false), None);
        assert_eq!(held.len(), 1);
    }

    #[test]
    fn keys_that_are_not_notes_are_ignored() {
        let mut held = Vec::new();
        assert_eq!(key_event(&mut held, Key::Space, true, false), None);
        assert!(held.is_empty());
    }

    #[test]
    fn losing_focus_releases_computer_keys_but_not_the_pointer() {
        let mut held = Vec::new();
        key_event(&mut held, Key::Z, true, false);
        held.push((7, None));
        key_event(&mut held, Key::X, true, false);
        let x = held[2].0;
        assert_eq!(release_all(&mut held), vec![(0, false), (x, false)]);
        assert_eq!(held, vec![(7, None)]);
        assert!(release_all(&mut held).is_empty());
    }
}
