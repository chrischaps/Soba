//! One of a Kit's lanes (#443) as the audio thread plays it: a profile's
//! [`Part`] boiled down to the note it strikes and the channel it goes out
//! on. The Digitakt's Track 3 is note 2 on channel 3, the SP-404MKII's
//! Pad 13 note 36 on the Kit's channel, a volca drum's Part 4 any note on
//! channel 4 (middle C, then).
//!
//! Like a [`SlotMap`](super::SlotMap), it's `Copy` and small, so it reaches
//! a running module by value (`EngineCommand::MapLane`) and nothing
//! allocates.

use super::profile::Part;

/// The note a part with only a channel is struck with: middle C, which a
/// sampler's track plays at its own pitch.
pub const MIDDLE_C: u8 = 60;

/// What a lane plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaneMap {
    /// The note it strikes, if its part names one.
    pub note: Option<u8>,
    /// Its own channel, 1-16, if its part names one: else the Kit's.
    pub channel: Option<u8>,
    /// The knobs, turning this lane's track, go out on its channel: the
    /// device takes the same CCs on each track's channel (the Digitakt).
    pub track: bool,
}

impl LaneMap {
    /// A lane that plays nothing.
    pub const EMPTY: Self = Self { note: None, channel: None, track: false };

    /// What a lane playing `part` sends: on a device whose parts take the
    /// same CCs each on its own channel (`cc_per_part`), its knobs follow it.
    pub fn of(part: &Part, cc_per_part: bool) -> Self {
        Self { note: part.note.map(|n| n.min(127)), channel: part.channel, track: cc_per_part && part.channel.is_some() }
    }

    /// Whether it plays nothing.
    pub fn is_empty(&self) -> bool {
        self.note.is_none() && self.channel.is_none()
    }

    /// The note it strikes. REAL-TIME SAFE.
    pub fn note(&self) -> u8 {
        self.note.unwrap_or(MIDDLE_C)
    }

    /// The channel it plays on, from 0: its own, or else `kit` (from 0, the
    /// Kit's). REAL-TIME SAFE.
    pub fn channel(&self, kit: u8) -> u8 {
        self.channel.map_or(kit, |own| own.clamp(1, 16) - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(note: Option<u8>, channel: Option<u8>) -> Part {
        Part { name: "Test".into(), note, channel }
    }

    #[test]
    fn a_lane_plays_its_parts_note_on_its_parts_channel_or_the_kits() {
        let both = LaneMap::of(&part(Some(2), Some(3)), true);
        assert_eq!((both.note(), both.channel(0)), (2, 2));
        assert!(both.track, "a part on its own channel takes the knobs where the device asks");
        let note = LaneMap::of(&part(Some(36), None), true);
        assert_eq!((note.note(), note.channel(5)), (36, 5));
        assert!(!note.track, "no channel of its own: the knobs stay on the Kit's");
        let channel = LaneMap::of(&part(None, Some(4)), false);
        assert_eq!((channel.note(), channel.channel(0)), (MIDDLE_C, 3));
        assert!(!channel.track, "a device that doesn't take CCs a track a channel");
        assert!(LaneMap::EMPTY.is_empty() && !channel.is_empty());
    }
}
