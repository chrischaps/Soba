//! The computer keys held as notes (#316), kept apart from the frame loop so
//! the rules for when a key counts can be tested.
//!
//! A press counts unless something else has the keys: a modifier (it's a
//! shortcut), a text field or a chord popover. A release always counts, so a
//! key let go while any of those took over doesn't stick, and losing the
//! window lets go of every key, since its releases go to whichever window
//! took the focus.
//!
//! Also the layout: which key plays which note ([`key_to_note`]).

/// Maps a computer keyboard key to a MIDI note number relative to C4.
///
/// Uses a tracker-style layout: two pianos, an octave apart.
/// - Bottom row (Z, X, C, V, B, N, M, comma, period, slash) = white keys from C4
/// - Middle row (S, D, G, H, J, L, semicolon) = their black keys
/// - Top row (Q, W, E, R, T, Y, U, I, O, P) = white keys from C5
/// - Number row (2, 3, 5, 6, 7, 9, 0) = their black keys
///
/// The two pianos overlap: `,` `.` `/` play the same C D E as Q W E.
///
/// Returns None if the key doesn't map to a note.
pub fn key_to_note(key: egui::Key) -> Option<i32> {
    use egui::Key;

    // Bottom row: white keys starting from C
    // Z=C, X=D, C=E, V=F, B=G, N=A, M=B, ,=C+, .=D+, /=E+
    match key {
        // Lower octave - white keys (Z X C V B N M , . /)
        Key::Z => Some(0),   // C
        Key::X => Some(2),   // D
        Key::C => Some(4),   // E
        Key::V => Some(5),   // F
        Key::B => Some(7),   // G
        Key::N => Some(9),   // A
        Key::M => Some(11),  // B
        Key::Comma => Some(12),  // C (next octave)
        Key::Period => Some(14), // D (next octave)
        Key::Slash => Some(16),  // E (next octave)

        // Lower octave - black keys (S D F G H J K L ; ')
        Key::S => Some(1),   // C#
        Key::D => Some(3),   // D#
        // F is skipped (no black key between E and F)
        Key::G => Some(6),   // F#
        Key::H => Some(8),   // G#
        Key::J => Some(10),  // A#
        // K is skipped (no black key between B and C)
        Key::L => Some(13),  // C# (next octave)
        Key::Semicolon => Some(15), // D# (next octave)

        // Upper octave - white keys (Q W E R T Y U I O P), from C5
        Key::Q => Some(12),  // C
        Key::W => Some(14),  // D
        Key::E => Some(16),  // E
        Key::R => Some(17),  // F
        Key::T => Some(19),  // G
        Key::Y => Some(21),  // A
        Key::U => Some(23),  // B
        Key::I => Some(24),  // C (next octave)
        Key::O => Some(26),  // D (next octave)
        Key::P => Some(28),  // E (next octave)

        // Upper octave - black keys (2 3 5 6 7 9 0), skipping 4 and 8 as
        // the middle row skips F and K
        Key::Num2 => Some(13), // C#
        Key::Num3 => Some(15), // D#
        Key::Num5 => Some(18), // F#
        Key::Num6 => Some(20), // G#
        Key::Num7 => Some(22), // A#
        Key::Num9 => Some(25), // C# (next octave)
        Key::Num0 => Some(27), // D# (next octave)

        _ => None,
    }
}

/// Converts a relative note (from key_to_note) to an absolute MIDI note number.
///
/// Base octave 0 means the keyboard starts at C4 (MIDI 60).
pub fn relative_to_midi(relative_note: i32, octave_shift: i32) -> u8 {
    let base_midi = 60; // C4
    let midi_note = base_midi + relative_note + (octave_shift * 12);
    midi_note.clamp(0, 127) as u8
}

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
    fn test_key_mapping() {
        use egui::Key;

        // White keys
        assert_eq!(key_to_note(Key::Z), Some(0));  // C
        assert_eq!(key_to_note(Key::X), Some(2));  // D
        assert_eq!(key_to_note(Key::C), Some(4));  // E
        assert_eq!(key_to_note(Key::V), Some(5));  // F
        assert_eq!(key_to_note(Key::B), Some(7));  // G
        assert_eq!(key_to_note(Key::N), Some(9));  // A
        assert_eq!(key_to_note(Key::M), Some(11)); // B

        // Black keys
        assert_eq!(key_to_note(Key::S), Some(1));  // C#
        assert_eq!(key_to_note(Key::D), Some(3));  // D#
        assert_eq!(key_to_note(Key::G), Some(6));  // F#
        assert_eq!(key_to_note(Key::H), Some(8));  // G#
        assert_eq!(key_to_note(Key::J), Some(10)); // A#

        // Non-note keys
        // Top row and numbers: the same piano, an octave up
        assert_eq!(key_to_note(Key::Q), Some(12)); // C5
        assert_eq!(key_to_note(Key::R), Some(17)); // F5
        assert_eq!(key_to_note(Key::P), Some(28)); // E6
        assert_eq!(key_to_note(Key::Num2), Some(13)); // C#5
        assert_eq!(key_to_note(Key::Num0), Some(27)); // D#6
        assert_eq!(key_to_note(Key::Num4), None);
        assert_eq!(key_to_note(Key::Num8), None);

        assert_eq!(key_to_note(Key::Space), None);
        assert_eq!(key_to_note(Key::Escape), None);
    }

    #[test]
    fn test_relative_to_midi() {
        // C at base octave = C4 = 60
        assert_eq!(relative_to_midi(0, 0), 60);

        // D at base octave = D4 = 62
        assert_eq!(relative_to_midi(2, 0), 62);

        // C one octave up = C5 = 72
        assert_eq!(relative_to_midi(0, 1), 72);

        // C one octave down = C3 = 48
        assert_eq!(relative_to_midi(0, -1), 48);

        // Clamping
        assert_eq!(relative_to_midi(0, -10), 0);  // Can't go below 0
        assert_eq!(relative_to_midi(12, 10), 127); // Can't go above 127
    }

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
