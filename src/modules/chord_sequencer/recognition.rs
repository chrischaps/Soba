//! Naming a chord from the notes of a hand: which root, which of the 19
//! types, and which note is under it.
//!
//! Every root and type is tried against the notes heard, and each is charged
//! for what doesn't fit:
//!
//! - a note heard that the chord doesn't have, most of all;
//! - a tone of the chord that wasn't heard. Players leave out the 5th of a
//!   chord with more to it, and a 13's 9th, so those still name the chord
//!   exactly, but the 5th costs as much as any tone: E♭ G C is C minor over
//!   its 3rd, not an E♭6 without its 5th;
//! - a bass that isn't the root. An inversion (the 3rd, 5th or 7th under
//!   it) costs less than a bass from outside the chord (Am/G#), which costs
//!   less than an upper tone (a 9th) under it.
//!
//! The cheapest wins. Of those, the chord built on the bass wins, then the
//! earliest type in the list. So where one set of notes is two chords (C6
//! is Am7 over C) the bass decides. A set that every chord charges for is
//! still named after the nearest one, and [`Heard`] says what didn't fit.

use super::{ChordSpec, CHORD_TYPES};

/// A note heard that the chord doesn't have.
const EXTRA: u32 = 5;
/// A tone of the chord that wasn't heard.
const MISSING: u32 = 3;
/// A 13's 9th that wasn't heard: the 13 is often played as 3, 7 and 13.
const NO_NINTH: u32 = 1;
/// The 3rd, 5th or 7th in the bass.
const INVERSION: u32 = 2;
/// A note from outside the chord in the bass.
const SLASH: u32 = 3;
/// A 9th, 11th or 13th in the bass.
const UPPER_BASS: u32 = 4;

/// A chord named from the notes heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heard {
    /// The chord, with its root in the octave just above the lowest note
    /// heard, so its bass note is the lowest note heard.
    pub spec: ChordSpec,
    /// Pitch classes heard that the chord doesn't have (bit 0 = C).
    pub extra: u16,
    /// Tones of the chord that weren't heard, bar the ones players leave
    /// out (bit 0 = C).
    pub missing: u16,
}

impl Heard {
    /// Whether the chord accounts for every note, and every note it needs
    /// was heard.
    pub fn exact(&self) -> bool {
        self.extra == 0 && self.missing == 0
    }
}

/// A chord type's tones as pitch classes above its root (bit 0 = the root).
struct Shape {
    tones: u16,
    /// Tones players leave out: the chord is still exactly itself without.
    optional: u16,
    /// The 13's 9th, which costs little to leave out.
    ninth: u16,
}

fn shape(kind: usize) -> Shape {
    let tones = CHORD_TYPES[kind].tones;
    let mut shape = Shape { tones: 0, optional: 0, ninth: 0 };
    for &tone in tones {
        let bit = 1 << tone.rem_euclid(12);
        shape.tones |= bit;
        if tone == 7 && tones.len() >= 4 {
            shape.optional |= bit;
        }
        if tone == 14 && tones.contains(&21) {
            shape.optional |= bit;
            shape.ninth |= bit;
        }
    }
    shape
}

/// What a bass `interval` semitones above the root costs a chord of `kind`.
fn bass_cost(kind: usize, interval: i32) -> u32 {
    match CHORD_TYPES[kind].tones.iter().find(|&&t| t.rem_euclid(12) == interval) {
        _ if interval == 0 => 0,
        Some(&t) if t < 12 => INVERSION,
        Some(_) => UPPER_BASS,
        None => SLASH,
    }
}

/// Rotates a 12-bit pitch-class set down by `by` semitones, so the pitch
/// class `by` becomes 0.
fn rotate(set: u16, by: i32) -> u16 {
    let by = by.rem_euclid(12) as u32;
    ((set >> by) | (set << (12 - by))) & 0xFFF
}

/// Names the chord in `notes` (MIDI note numbers, in any order, repeats
/// allowed). `None` when there are none.
pub fn recognise(notes: &[i32]) -> Option<Heard> {
    let lowest = *notes.iter().min()?;
    let bass = lowest.rem_euclid(12);
    let heard = notes.iter().fold(0u16, |set, &n| set | 1 << n.rem_euclid(12));

    // The cost, then whether the bass is someone else's root
    let mut best: Option<((u32, bool), Heard)> = None;
    for kind in 0..CHORD_TYPES.len() {
        let shape = shape(kind);
        for root in 0..12 {
            let relative = rotate(heard, root);
            let under = (bass - root).rem_euclid(12);
            // A slash bass is charged as a bass, not as a note the chord lacks
            let extra = relative & !shape.tones & !(1 << under);
            let unheard = shape.tones & !relative;
            let cost = EXTRA * extra.count_ones()
                + MISSING * (unheard & !shape.ninth).count_ones()
                + NO_NINTH * (unheard & shape.ninth).count_ones()
                + bass_cost(kind, under);
            let rank = (cost, under != 0);
            if best.is_some_and(|(r, _)| r <= rank) {
                continue;
            }
            let spec = ChordSpec {
                root: lowest + (root - bass).rem_euclid(12),
                kind,
                bass: (under != 0).then_some(bass),
                top: None,
            };
            let missing = unheard & !shape.optional;
            best = Some((rank, Heard { spec, extra: rotate(extra, -root), missing: rotate(missing, -root) }));
        }
    }
    best.map(|(_, heard)| heard)
}

/// The pitch classes in a set, from C up.
pub fn pitch_classes(set: u16) -> impl Iterator<Item = i32> {
    (0..12).filter(move |pc| set & (1 << pc) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chord's notes in close position from `root`, its tones in the
    /// order of the octave and those above it, with `bass` (a tone's index
    /// in that order) moved to the bottom.
    fn played(root: i32, kind: usize, bass: usize) -> Vec<i32> {
        let mut tones: Vec<i32> = CHORD_TYPES[kind].tones.to_vec();
        tones.sort();
        let low = tones[bass];
        tones.iter().map(|&t| root + t + if t < low { 12 } else { 0 }).collect()
    }

    fn named(notes: &[i32]) -> String {
        recognise(notes).unwrap().spec.name()
    }

    fn set_of(notes: &[i32]) -> u16 {
        notes.iter().fold(0, |s, &n| s | 1 << n.rem_euclid(12))
    }

    fn spec_set(spec: &ChordSpec) -> u16 {
        rotate(shape(spec.kind).tones, -spec.root) | spec.bass.map_or(0, |b| 1 << b)
    }

    #[test]
    fn every_type_in_every_key_is_named_in_root_position() {
        for kind in 0..CHORD_TYPES.len() {
            for root in 48..60 {
                let notes = played(root, kind, 0);
                let heard = recognise(&notes).unwrap();
                assert_eq!(heard.spec, ChordSpec { root, kind, bass: None, top: None }, "{notes:?}");
                assert!(heard.exact());
            }
        }
    }

    #[test]
    fn every_inversion_is_named_over_its_bass() {
        let mut renamed = Vec::new();
        for (kind, chord) in CHORD_TYPES.iter().enumerate() {
            let tones = chord.tones.iter().filter(|&&t| t < 12).count();
            for root in 48..60 {
                for bass in 1..tones {
                    let notes = played(root, kind, bass);
                    let heard = recognise(&notes).unwrap();
                    assert!(heard.exact(), "{notes:?} → {}", heard.spec.name());
                    // The lowest note heard is the bass that sounds
                    assert_eq!(heard.spec.bass_note(), *notes.iter().min().unwrap(), "{notes:?}");
                    assert_eq!(spec_set(&heard.spec), set_of(&notes), "{notes:?} → {}", heard.spec.name());
                    if heard.spec.kind != kind {
                        renamed.push((CHORD_TYPES[kind].label, CHORD_TYPES[heard.spec.kind].label, heard.spec.name()));
                    }
                }
            }
        }
        // Where the same notes make two chords, the one built on the bass is
        // named (A C E G over C is C6), or the earlier type when neither is
        // (C6 over E is Am7/E). Only these types have a twin
        let twins = [("6", "m7"), ("m7", "6"), ("m6", "m7b5"), ("m7b5", "m6"), ("sus2", "sus4"), ("sus4", "sus2")];
        for (meant, kind, name) in renamed {
            assert!(twins.contains(&(meant, kind)), "a {meant} was named {name}");
        }
    }

    #[test]
    fn a_fifth_left_out_is_still_the_chord() {
        // Bar the 6ths: C E A is A minor over C, and C E♭ A is A dim over C
        let sixths = ["6", "m6"];
        for kind in (0..CHORD_TYPES.len()).filter(|&k| CHORD_TYPES[k].tones.len() >= 4 && CHORD_TYPES[k].tones.contains(&7) && !sixths.contains(&CHORD_TYPES[k].label)) {
            for root in 48..60 {
                let notes: Vec<i32> = CHORD_TYPES[kind].tones.iter().filter(|&&t| t != 7).map(|&t| root + t).collect();
                let heard = recognise(&notes).unwrap();
                assert_eq!(heard.spec, ChordSpec { root, kind, bass: None, top: None }, "{notes:?} → {}", heard.spec.name());
                assert!(heard.exact());
            }
        }
    }

    #[test]
    fn chords_as_hands_play_them() {
        // A line cliché: the bass walks down under A minor
        assert_eq!(named(&[45, 57, 60, 64]), "Am");
        assert_eq!(named(&[44, 57, 60, 64]), "Am/G#");
        assert_eq!(named(&[43, 57, 60, 64]), "Am7/G");
        // Am6 over its 6th is F#m7b5, as the cliché is often written
        assert_eq!(named(&[42, 57, 60, 64]), "F#m7b5");
        // A first inversion, and the 5th in the bass
        assert_eq!(named(&[52, 55, 60]), "C/E");
        assert_eq!(named(&[50, 54, 57, 60, 62]), "D7");
        assert_eq!(named(&[43, 48, 52]), "C/G");
        // A 13 as 3, 7 and 13 over the root, and an 11, which is a B♭
        // triad over C
        assert_eq!(named(&[48, 52, 57, 58]), "C13");
        assert_eq!(named(&[36, 58, 62, 65]), "C11");
        // A D7 over its 7th, without its 5th
        let heard = recognise(&[48, 50, 54]).unwrap();
        assert_eq!((heard.spec.name(), heard.exact()), ("D7/C".to_string(), true));
        // A Cmaj7#5, which isn't a type, is E over C, and with a D, E7
        assert_eq!(named(&[48, 52, 56, 59]), "E/C");
        assert_eq!(named(&[48, 52, 56, 59, 62]), "E7/C");
        // E♭ G C is C minor over its 3rd
        assert_eq!(named(&[51, 55, 60]), "Cm/Eb");
        // The same notes over a different bass: C6 and Am7
        assert_eq!(named(&[48, 52, 55, 57]), "C6");
        assert_eq!(named(&[45, 48, 52, 55]), "Am7");
        // E, A and C read as A minor over its 3rd, not a C6 without its 5th
        assert_eq!(named(&[48, 52, 57]), "Am/C");
    }

    #[test]
    fn the_root_sits_just_above_the_bass() {
        let heard = recognise(&[40, 55, 60]).unwrap(); // E2 G3 C4
        assert_eq!((heard.spec.root, heard.spec.bass_note()), (48, 40));
        let heard = recognise(&[57, 60, 64]).unwrap(); // A3 C4 E4
        assert_eq!(heard.spec.root, 57);
        assert_eq!(heard.spec.bass, None);
    }

    #[test]
    fn notes_that_fit_no_chord_are_named_after_the_nearest() {
        // A cluster
        let heard = recognise(&[48, 49, 50]).unwrap();
        assert!(!heard.exact(), "{}", heard.spec.name());
        // C E F: an Fmaj7 over C without its 3rd
        let heard = recognise(&[48, 52, 53]).unwrap();
        assert_eq!(heard.spec.name(), "Fmaj7/C");
        assert_eq!(pitch_classes(heard.missing).collect::<Vec<_>>(), [9]);
        // Two notes are a chord with the rest missing
        let heard = recognise(&[48, 55]).unwrap();
        assert_eq!(heard.spec, ChordSpec { root: 48, kind: 0, bass: None, top: None });
        assert_eq!(pitch_classes(heard.missing).collect::<Vec<_>>(), [4]);
    }

    #[test]
    fn doubled_notes_and_order_dont_matter() {
        assert_eq!(named(&[67, 48, 64, 60, 55, 72]), "C");
        assert_eq!(recognise(&[]), None);
    }
}
