//! Cutting a recording into slices, to play a part of it by note.
//!
//! A sliced recording is cut into equal parts between two points (a
//! Sampler's Start and End), and a note picks a part: the first slice's key
//! plays slice 1, the key a semitone up slice 2, and so on, the way samplers
//! map a chopped break across the keyboard. A control can pick one too,
//! 0 to 1 across the slices, for a lane or a random source.
//!
//! Places here are whatever the caller measures in: frames on the audio
//! thread, fractions of the recording in the display.

/// The most slices a recording is cut into.
pub const MAX_SLICES: usize = 64;

/// A recording cut into equal slices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slices {
    count: usize,
}

impl Slices {
    /// `count` equal slices, at least 2 and at most [`MAX_SLICES`].
    pub fn equal(count: usize) -> Self {
        Self { count: count.clamp(2, MAX_SLICES) }
    }

    /// From a Slices setting: below 2 is off.
    pub fn from_param(value: f32) -> Option<Self> {
        let count = value.round();
        (count >= 2.0).then(|| Self::equal(count.min(MAX_SLICES as f32) as usize))
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Where slice `index` (from 0) lies between `start` and `end`.
    pub fn span(&self, index: usize, start: f64, end: f64) -> (f64, f64) {
        let width = (end - start) / self.count as f64;
        let index = index.min(self.count - 1);
        // The last slice ends exactly at `end`, whatever the rounding
        let to = if index + 1 == self.count { end } else { start + width * (index + 1) as f64 };
        (start + width * index as f64, to)
    }

    /// The slice `place` lies in between `start` and `end`, if it's there.
    pub fn at(&self, place: f64, start: f64, end: f64) -> Option<usize> {
        if end <= start || place < start || place > end {
            return None;
        }
        let index = (((place - start) / (end - start) * self.count as f64) as usize).min(self.count - 1);
        // Rounding can put a slice's own start just before it
        let next = index + 1;
        Some(if next < self.count && self.span(next, start, end).0 <= place { next } else { index })
    }

    /// The slice a note `semitones` above the first slice's key plays, if
    /// there is one: notes below it, or past the last slice, play nothing.
    pub fn by_note(&self, semitones: f32) -> Option<usize> {
        let index = semitones.round();
        (index >= 0.0 && index < self.count as f32).then_some(index as usize)
    }

    /// The slice a control from 0 to 1 picks, evenly across them: 0 is the
    /// first, and 1 is the last rather than one past it.
    pub fn by_control(&self, value: f32) -> usize {
        ((value.clamp(0.0, 1.0) * self.count as f32) as usize).min(self.count - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_below_two_is_off_and_the_count_is_bounded() {
        assert_eq!(Slices::from_param(0.0), None);
        assert_eq!(Slices::from_param(1.0), None);
        assert_eq!(Slices::from_param(1.6).map(|s| s.count()), Some(2));
        assert_eq!(Slices::from_param(12.0).map(|s| s.count()), Some(12));
        assert_eq!(Slices::from_param(500.0).map(|s| s.count()), Some(MAX_SLICES));
    }

    #[test]
    fn test_spans_tile_the_region_exactly() {
        let slices = Slices::equal(12);
        let (start, end) = (1000.0, 781_000.0);
        let mut at = start;
        for index in 0..12 {
            let (from, to) = slices.span(index, start, end);
            assert_eq!(from, at, "slice {index} starts where the last ended");
            assert!(to > from);
            at = to;
        }
        assert_eq!(at, end);
    }

    #[test]
    fn test_a_place_finds_its_slice() {
        let slices = Slices::equal(4);
        assert_eq!(slices.at(0.0, 0.0, 1.0), Some(0));
        assert_eq!(slices.at(0.26, 0.0, 1.0), Some(1));
        assert_eq!(slices.at(1.0, 0.0, 1.0), Some(3));
        assert_eq!(slices.at(0.1, 0.2, 0.6), None);
        assert_eq!(slices.at(0.7, 0.2, 0.6), None);
        // Each slice's own start is in it
        for index in 0..4 {
            let (from, _) = slices.span(index, 0.2, 0.6);
            assert_eq!(slices.at(from, 0.2, 0.6), Some(index));
        }
    }

    #[test]
    fn test_notes_pick_slices_from_the_first_key() {
        let slices = Slices::equal(8);
        assert_eq!(slices.by_note(0.0), Some(0));
        // A semitone in volts, times twelve, isn't quite whole
        assert_eq!(slices.by_note(12.0 * (1.0 / 12.0)), Some(1));
        assert_eq!(slices.by_note(7.0), Some(7));
        assert_eq!(slices.by_note(8.0), None);
        assert_eq!(slices.by_note(-1.0), None);
    }

    #[test]
    fn test_a_control_spreads_evenly_across_the_slices() {
        let slices = Slices::equal(4);
        assert_eq!(slices.by_control(0.0), 0);
        assert_eq!(slices.by_control(0.24), 0);
        assert_eq!(slices.by_control(0.25), 1);
        assert_eq!(slices.by_control(0.99), 3);
        assert_eq!(slices.by_control(1.0), 3);
        assert_eq!(slices.by_control(-3.0), 0);
        assert_eq!(slices.by_control(7.0), 3);
    }
}
