//! Cutting a recording into slices, to play a part of it by note.
//!
//! A sliced recording is cut between two points (a Sampler's Start and
//! End), into equal parts or at markers (where its hits begin, see
//! [`crate::dsp::onsets`], or wherever they've been dragged), and a note
//! picks a part: the first slice's key plays slice 1, the key a semitone up
//! slice 2, and so on, the way samplers map a chopped break across the
//! keyboard. A control can pick one too, 0 to 1 across the slices, for a
//! lane or a random source.
//!
//! Places here are whatever the caller measures in: frames on the audio
//! thread, fractions of the recording in the display. Markers are always
//! fractions of the whole recording, so they stay put when Start moves.

/// The most slices a recording is cut into.
pub const MAX_SLICES: usize = 64;

/// The most markers that cut it: one fewer than the slices.
pub const MAX_MARKERS: usize = MAX_SLICES - 1;

/// The Slices setting past 64: cut at markers.
pub const ONSETS: f32 = MAX_SLICES as f32 + 1.0;

/// Where a recording is cut, as fractions of it, in order: a fixed list,
/// so it can be handed to the audio thread without allocating.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Markers {
    len: usize,
    places: [f32; MAX_MARKERS],
}

impl Default for Markers {
    fn default() -> Self {
        Self { len: 0, places: [0.0; MAX_MARKERS] }
    }
}

impl Markers {
    /// The first [`MAX_MARKERS`] of `places` that lie strictly between
    /// `start` and `end` (fractions of the recording), in order, each
    /// once: the ones that cut what plays.
    pub fn between(places: &[f32], start: f32, end: f32) -> Self {
        let mut markers = Self::default();
        let mut inside: Vec<f32> = places.iter().copied().filter(|&place| place > start && place < end).collect();
        inside.sort_by(f32::total_cmp);
        inside.dedup();
        for place in inside.into_iter().take(MAX_MARKERS) {
            markers.places[markers.len] = place;
            markers.len += 1;
        }
        markers
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.places[..self.len]
    }
}

/// How a recording is cut.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cut {
    Equal,
    /// Where slices 2 onwards begin, as fractions of the way from the
    /// start to the end: the first `count - 1` of these.
    Marked([f32; MAX_MARKERS]),
}

/// A recording cut into slices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slices {
    count: usize,
    cut: Cut,
}

impl Slices {
    /// `count` equal slices, at least 2 and at most [`MAX_SLICES`].
    pub fn equal(count: usize) -> Self {
        Self { count: count.clamp(2, MAX_SLICES), cut: Cut::Equal }
    }

    /// Cut at `markers` (fractions of the recording, in order) between
    /// `start` and `end` (fractions too): a slice before the first marker
    /// inside, and one from each. With none inside, one slice.
    pub fn marked(markers: &[f32], start: f32, end: f32) -> Self {
        let mut cuts = [0.0; MAX_MARKERS];
        let mut count = 1;
        let width = end - start;
        for &place in markers.iter().filter(|&&place| place > start && place < end) {
            let along = (place - start) / width;
            // In order, each once, and no more than there's room for
            if count <= MAX_MARKERS && (count == 1 || along > cuts[count - 2]) {
                cuts[count - 1] = along;
                count += 1;
            }
        }
        Self { count, cut: Cut::Marked(cuts) }
    }

    /// From a Slices setting: below 2 is off, 2 to 64 equal slices, and
    /// [`ONSETS`] cut at `markers` between `start` and `end` (all fractions
    /// of the recording, as [`marked`](Self::marked) takes them).
    pub fn from_param(value: f32, markers: &[f32], start: f32, end: f32) -> Option<Self> {
        let count = value.round();
        if count >= ONSETS {
            Some(Self::marked(markers, start.min(end), start.max(end)))
        } else {
            (count >= 2.0).then(|| Self::equal(count.min(MAX_SLICES as f32) as usize))
        }
    }

    /// Whether a Slices setting cuts at markers.
    pub fn is_marked_param(value: f32) -> bool {
        value.round() >= ONSETS
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Whether it's cut at markers, rather than equally.
    pub fn is_marked(&self) -> bool {
        matches!(self.cut, Cut::Marked(_))
    }

    /// Where slice `index` (from 0) lies between `start` and `end`.
    pub fn span(&self, index: usize, start: f64, end: f64) -> (f64, f64) {
        let index = index.min(self.count - 1);
        // The last slice ends exactly at `end`, whatever the rounding
        let last = index + 1 == self.count;
        match &self.cut {
            Cut::Equal => {
                let width = (end - start) / self.count as f64;
                let to = if last { end } else { start + width * (index + 1) as f64 };
                (start + width * index as f64, to)
            }
            Cut::Marked(cuts) => {
                let at = |cut: usize| start + (end - start) * cuts[cut] as f64;
                (if index == 0 { start } else { at(index - 1) }, if last { end } else { at(index) })
            }
        }
    }

    /// The slice `place` lies in between `start` and `end`, if it's there.
    pub fn at(&self, place: f64, start: f64, end: f64) -> Option<usize> {
        if end <= start || place < start || place > end {
            return None;
        }
        Some(match &self.cut {
            Cut::Equal => {
                let index = (((place - start) / (end - start) * self.count as f64) as usize).min(self.count - 1);
                // Rounding can put a slice's own start just before it
                let next = index + 1;
                if next < self.count && self.span(next, start, end).0 <= place {
                    next
                } else {
                    index
                }
            }
            // The last slice that starts at or before it, measured as span does
            Cut::Marked(_) => (1..self.count).take_while(|&index| self.span(index, start, end).0 <= place).last().unwrap_or(0),
        })
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
        let equal = |value| Slices::from_param(value, &[], 0.0, 1.0);
        assert_eq!(equal(0.0), None);
        assert_eq!(equal(1.0), None);
        assert_eq!(equal(1.6).map(|s| s.count()), Some(2));
        assert_eq!(equal(12.0).map(|s| s.count()), Some(12));
        assert_eq!(equal(64.0).map(|s| s.count()), Some(MAX_SLICES));
        assert!(!equal(64.0).unwrap().is_marked());
        // Past 64, the markers cut it
        assert!(equal(ONSETS).unwrap().is_marked());
        assert!(Slices::is_marked_param(ONSETS) && !Slices::is_marked_param(64.0));
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
    fn test_markers_cut_between_start_and_end() {
        // Markers at 10%, 30%, 50% and 90%; Start at 20%, End at 80%
        let slices = Slices::from_param(ONSETS, &[0.1, 0.3, 0.5, 0.9], 0.2, 0.8).unwrap();
        assert_eq!(slices.count(), 3);
        // In frames of a 1000-frame recording
        let (start, end) = (200.0, 800.0);
        let spans: Vec<(f64, f64)> = (0..3).map(|i| slices.span(i, start, end)).collect();
        assert_eq!(spans[0].0, 200.0);
        assert!((spans[0].1 - 300.0).abs() < 1e-3 && (spans[1].0 - 300.0).abs() < 1e-3);
        assert!((spans[1].1 - 500.0).abs() < 1e-3 && (spans[2].0 - 500.0).abs() < 1e-3);
        assert_eq!(spans[2].1, 800.0);
        for (index, &(from, to)) in spans.iter().enumerate() {
            assert_eq!(slices.at(from, start, end), Some(index), "a slice's own start is in it");
            assert_eq!(slices.at((from + to) / 2.0, start, end), Some(index));
        }
        assert_eq!(slices.at(end, start, end), Some(2));
        assert_eq!(slices.at(150.0, start, end), None);
        // Notes and the Slice CV spread over however many there are
        assert_eq!(slices.by_note(2.0), Some(2));
        assert_eq!(slices.by_note(3.0), None);
        assert_eq!(slices.by_control(1.0), 2);
    }

    #[test]
    fn test_markers_out_of_order_or_too_many_are_tidied() {
        // None inside: the whole of Start to End is one slice
        let one = Slices::marked(&[0.0, 1.0], 0.0, 1.0);
        assert_eq!(one.count(), 1);
        assert_eq!(one.span(0, 0.0, 10.0), (0.0, 10.0));
        // Unsorted and repeated, as a list fixed by hand might arrive
        let markers = Markers::between(&[0.5, 0.25, 0.5, 0.75, 1.5, -0.1], 0.0, 1.0);
        assert_eq!(markers.as_slice(), &[0.25, 0.5, 0.75]);
        assert_eq!(Slices::marked(markers.as_slice(), 0.0, 1.0).count(), 4);
        // At most 64 slices: the rest of the recording is the last one
        let many: Vec<f32> = (1..200).map(|i| i as f32 / 200.0).collect();
        let markers = Markers::between(&many, 0.0, 1.0);
        assert_eq!(markers.as_slice().len(), MAX_MARKERS);
        let slices = Slices::marked(&many, 0.0, 1.0);
        assert_eq!(slices.count(), MAX_SLICES);
        assert_eq!(slices.span(MAX_SLICES - 1, 0.0, 1.0).1, 1.0);
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
