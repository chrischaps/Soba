//! The chord popover's chimes: a chord chosen with a click sounds for a
//! moment, the way one played sounds while the hand is down.
//!
//! A click has no hand to lift, so a chime rings for a fixed time instead
//! (see [`ring_for`]). Resting the pointer on a chip sounds its chord without
//! writing it, so Gm7 and Gm9 can be compared before one's chosen. The rest
//! has to be a real one ([`REST`]), so a pointer crossing the grid stays
//! quiet; once a chip has sounded, its neighbours sound sooner ([`FOLLOW`]),
//! as tooltips do, so a row can be played along by moving across it.

use crate::dsp::Audition;

/// How long a chord chosen sounds, in seconds, once its strum has struck
/// its last string: a short strum's worth of a held chord.
const RING: f64 = 0.6;
/// How long the pointer rests on a chip before its chord sounds.
pub(super) const REST: f64 = 0.35;
/// ...and on the next chip, after one has sounded.
pub(super) const FOLLOW: f64 = 0.12;
/// How long after a chip sounds its neighbours follow sooner.
const WARM: f64 = 1.0;

/// A chip in the popover, by what it chooses rather than where it sits, so
/// the chip under the pointer is still the same one after the popover moves
/// along to the next step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Chip {
    /// A key on the piano: the root.
    Root(u8),
    /// A Bass choice: 0 for the root, or a pitch class plus one.
    Bass(usize),
    /// A Top choice: `None` for Auto.
    Top(Option<i32>),
    /// A chord type, by its index in the type list.
    Type(usize),
}

/// How long a chime of `audition` rings, for a strum of `strum` seconds
/// between strings.
pub(super) fn ring_for(audition: &Audition, strum: f32) -> f64 {
    RING + strum.max(0.0) as f64 * audition.notes().len().saturating_sub(1) as f64
}

/// The chip under the pointer: which, since when, and whether it's sounded.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rest {
    chip: Chip,
    since: f64,
    heard: bool,
}

/// What the popover's chips are sounding.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Chimes {
    /// The chord sounding, and when it stops.
    ringing: Option<(Audition, f64)>,
    rest: Option<Rest>,
    /// When a chip last sounded.
    last: Option<f64>,
}

impl Chimes {
    /// Sounds `audition` from now, for `ring` seconds, chosen on `chip`. The
    /// chip it was chosen on counts as heard, so resting on it doesn't
    /// sound it again. True if it's struck afresh: a chord already ringing,
    /// as the chip clicked was sounded by the pointer resting on it a moment
    /// before, rings on instead, so the click doesn't strike it twice.
    pub fn strike(&mut self, chip: Chip, audition: Audition, ring: f64, now: f64) -> bool {
        let afresh = self.ringing(now) != Some(audition);
        self.ringing = Some((audition, now + ring));
        self.rest = Some(Rest { chip, since: now, heard: true });
        self.last = Some(now);
        afresh
    }

    /// Follows the chip under the pointer, if any. True when it has rested
    /// there long enough to be heard, once per rest.
    pub fn rest_on(&mut self, chip: Option<Chip>, now: f64) -> bool {
        let Some(chip) = chip else {
            self.rest = None;
            return false;
        };
        let rest = match self.rest {
            Some(rest) if rest.chip == chip => rest,
            _ => Rest { chip, since: now, heard: false },
        };
        let warm = self.last.is_some_and(|last| rest.since - last < WARM);
        let ready = !rest.heard && now - rest.since >= if warm { FOLLOW } else { REST };
        self.rest = Some(Rest { heard: rest.heard || ready, ..rest });
        if ready {
            self.last = Some(now);
        }
        ready
    }

    /// The chord sounding now, until its time is up.
    pub fn ringing(&mut self, now: f64) -> Option<Audition> {
        if self.ringing.is_some_and(|(_, until)| now >= until) {
            self.ringing = None;
        }
        self.ringing.map(|(audition, _)| audition)
    }

    /// Stops the chord sounding: the hand has come down on the keys.
    pub fn hush(&mut self) {
        self.ringing = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A moment past `wait` after `t`, clear of rounding.
    fn past(t: f64, wait: f64) -> f64 {
        t + wait + 1e-6
    }

    fn chord(notes: &[u8]) -> Audition {
        let mut audition = Audition { len: notes.len() as u8, velocity: 0.75, ..Audition::default() };
        audition.notes[..notes.len()].copy_from_slice(notes);
        audition
    }

    #[test]
    fn a_chord_chosen_rings_for_a_moment() {
        let mut chimes = Chimes::default();
        let gm7 = chord(&[55, 58, 62, 65]);
        chimes.strike(Chip::Type(3), gm7, RING, 10.0);
        assert_eq!(chimes.ringing(10.0), Some(gm7));
        assert_eq!(chimes.ringing(10.0 + RING - 0.01), Some(gm7));
        assert_eq!(chimes.ringing(10.0 + RING), None);
        assert_eq!(chimes.ringing(10.0), None, "once it's stopped it stays stopped");
    }

    #[test]
    fn a_strum_rings_on_past_its_last_string() {
        let gm7 = chord(&[55, 58, 62, 65]);
        assert_eq!(ring_for(&gm7, 0.0), RING);
        // Three gaps between four strings
        assert!((ring_for(&gm7, 0.05) - (RING + 0.15)).abs() < 1e-6);
        assert_eq!(ring_for(&chord(&[55]), 0.05), RING, "one note has no strum");
    }

    #[test]
    fn a_chip_rested_on_sounds_once() {
        let mut chimes = Chimes::default();
        assert!(!chimes.rest_on(Some(Chip::Type(3)), 0.0));
        assert!(!chimes.rest_on(Some(Chip::Type(3)), REST - 0.01));
        assert!(chimes.rest_on(Some(Chip::Type(3)), past(0.0, REST)));
        assert!(!chimes.rest_on(Some(Chip::Type(3)), REST + 1.0), "once per rest");
        // Away and back, a while later: it's a new rest
        assert!(!chimes.rest_on(None, 5.0));
        assert!(!chimes.rest_on(Some(Chip::Type(3)), 5.0));
        assert!(chimes.rest_on(Some(Chip::Type(3)), past(5.0, REST)));
    }

    #[test]
    fn a_pointer_crossing_the_chips_stays_quiet() {
        let mut chimes = Chimes::default();
        for (i, chip) in [Chip::Type(0), Chip::Type(1), Chip::Type(2), Chip::Bass(0)].into_iter().enumerate() {
            assert!(!chimes.rest_on(Some(chip), i as f64 * 0.1));
        }
    }

    #[test]
    fn once_a_chip_has_sounded_its_neighbours_follow_sooner() {
        let mut chimes = Chimes::default();
        chimes.rest_on(Some(Chip::Type(3)), 0.0);
        assert!(chimes.rest_on(Some(Chip::Type(3)), past(0.0, REST)));
        // Gm7 to Gm9: a short pause is enough
        let next = REST + 0.2;
        assert!(!chimes.rest_on(Some(Chip::Type(4)), next));
        assert!(chimes.rest_on(Some(Chip::Type(4)), past(next, FOLLOW)));
        // Left for a while, the next takes a full rest again
        let later = next + 3.0;
        chimes.rest_on(None, later);
        assert!(!chimes.rest_on(Some(Chip::Type(5)), later));
        assert!(!chimes.rest_on(Some(Chip::Type(5)), later + FOLLOW));
        assert!(chimes.rest_on(Some(Chip::Type(5)), past(later, REST)));
    }

    #[test]
    fn a_chip_clicked_isnt_sounded_again_by_the_rest() {
        let mut chimes = Chimes::default();
        // Clicking a type moves the popover along, but the pointer's still on
        // the same chip: it was heard as it was chosen
        chimes.rest_on(Some(Chip::Type(3)), 0.0);
        chimes.strike(Chip::Type(3), chord(&[55, 58, 62, 65]), RING, 0.1);
        assert!(!chimes.rest_on(Some(Chip::Type(3)), 0.1 + REST));
        // ...while its neighbour, a moment later, follows on
        assert!(!chimes.rest_on(Some(Chip::Type(4)), 0.8));
        assert!(chimes.rest_on(Some(Chip::Type(4)), past(0.8, FOLLOW)));
    }

    #[test]
    fn a_chord_clicked_as_it_rings_rings_on() {
        let mut chimes = Chimes::default();
        let f7 = chord(&[53, 57, 60, 63]);
        // Rested on, then clicked a moment later: one strike, held longer
        assert!(chimes.strike(Chip::Type(2), f7, RING, 0.0));
        assert!(!chimes.strike(Chip::Type(2), f7, RING, 0.1));
        assert_eq!(chimes.ringing(RING + 0.05), Some(f7));
        // Another chord, or the same once it's stopped, strikes afresh
        assert!(chimes.strike(Chip::Type(3), chord(&[53, 56, 60, 63]), RING, 0.2));
        assert!(chimes.strike(Chip::Type(3), chord(&[53, 56, 60, 63]), RING, 2.0));
    }

    #[test]
    fn the_hand_hushes_a_chime() {
        let mut chimes = Chimes::default();
        chimes.strike(Chip::Bass(2), chord(&[50, 57, 60, 65]), RING, 0.0);
        chimes.hush();
        assert_eq!(chimes.ringing(0.1), None);
    }
}
