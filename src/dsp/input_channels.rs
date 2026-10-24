//! Which of the input device's channels a module hears (#450).
//!
//! An interface like a Tascam Model 12 or a Zoom L6 gives the computer
//! every mixer channel as an input of its own, so each instrument plugged
//! into it can come back on its own channels. A module that listens picks
//! them with a Channel choice built by [`parameter`]: a pair (1+2, 3+4…),
//! heard on its L and R, or one channel (1, 2, 3…), heard on both.
//!
//! The choices run three to a pair, `1+2, 1, 2, 3+4, 3, 4, …`, so the first
//! three are Audio Input's old Stereo, 1 and 2, and patches saved before
//! this keep their index and their sound.
//!
//! ```
//! use soba::dsp::input_channels::{self, InputChannels};
//! // A Channel parameter: "1+2" to start with
//! let channel = input_channels::parameter("channel", "Channel");
//! assert_eq!(InputChannels::from_value(channel.default), InputChannels::Pair(0));
//! // Channel 7 alone, as a module reads it from its params
//! let seven = InputChannels::Mono(6);
//! assert_eq!(InputChannels::from_value(seven.index() as f32), seven);
//! assert_eq!(seven.label(), "7");
//! ```

use super::{InputAudio, ParameterDefinition};

/// The most input channels the engine carries from a device: past these, a
/// bigger interface's channels go unheard.
pub const MAX_INPUT_CHANNELS: usize = 16;

/// Every choice, three to a pair: the pair, then each of its channels.
pub const LABELS: &[&str] = &[
    "1+2", "1", "2", "3+4", "3", "4", "5+6", "5", "6", "7+8", "7", "8", //
    "9+10", "9", "10", "11+12", "11", "12", "13+14", "13", "14", "15+16", "15", "16",
];

const _: () = assert!(LABELS.len() == MAX_INPUT_CHANNELS / 2 * 3);

/// The channels a module hears, counted from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputChannels {
    /// Channel `n` on L and `n + 1` on R, `n` even.
    Pair(usize),
    /// Channel `n` alone, on both sides.
    Mono(usize),
}

impl Default for InputChannels {
    /// 1+2: what Audio Input always heard.
    fn default() -> Self {
        Self::Pair(0)
    }
}

impl InputChannels {
    /// The choice at `index` in [`LABELS`], held to the list.
    pub fn from_index(index: usize) -> Self {
        let index = index.min(LABELS.len() - 1);
        let pair = index / 3 * 2;
        match index % 3 {
            0 => Self::Pair(pair),
            1 => Self::Mono(pair),
            _ => Self::Mono(pair + 1),
        }
    }

    /// The choice a parameter's value names, as a module's `params` carry it.
    #[inline]
    pub fn from_value(value: f32) -> Self {
        Self::from_index(value.round().max(0.0) as usize)
    }

    /// Where it falls in [`LABELS`].
    pub fn index(self) -> usize {
        match self {
            Self::Pair(first) => first / 2 * 3,
            Self::Mono(channel) => channel / 2 * 3 + 1 + channel % 2,
        }
    }

    /// Its name in the list: "3+4", or "7".
    pub fn label(self) -> &'static str {
        LABELS[self.index().min(LABELS.len() - 1)]
    }

    /// The channels heard on L and on R.
    #[inline]
    pub fn sides(self) -> (usize, usize) {
        match self {
            Self::Pair(first) => (first, first + 1),
            Self::Mono(channel) => (channel, channel),
        }
    }

    /// Whether a device of `channels` channels has every one it hears. A
    /// mono device counts as two, as the engine hears it on both.
    pub fn fits(self, channels: usize) -> bool {
        self.sides().1 < channels.max(2)
    }

    /// Frame `frame` of what it hears in `input`, as (L, R). A channel the
    /// device doesn't have is silence.
    ///
    /// REAL-TIME SAFE.
    #[inline]
    pub fn frame(self, input: &InputAudio<'_>, frame: usize) -> (f32, f32) {
        let (left, right) = self.sides();
        (input.sample(frame, left), input.sample(frame, right))
    }
}

/// A Channel choice listing every pair and channel, starting at 1+2.
pub fn parameter(id: &'static str, name: &'static str) -> ParameterDefinition {
    ParameterDefinition::choice(id, name, LABELS, 0)
}

/// Whether a dropdown's `options` are these choices, so the editor can
/// offer only the channels the input device has.
pub fn is_choice(options: &[String]) -> bool {
    options.len() == LABELS.len() && options.iter().zip(LABELS).all(|(option, label)| option == label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_three_are_the_old_stereo_one_and_two() {
        assert_eq!(InputChannels::from_index(0), InputChannels::Pair(0));
        assert_eq!(InputChannels::from_index(1), InputChannels::Mono(0));
        assert_eq!(InputChannels::from_index(2), InputChannels::Mono(1));
        assert_eq!(InputChannels::default().index(), 0);
    }

    #[test]
    fn every_choice_round_trips_through_its_index_and_label() {
        for (index, label) in LABELS.iter().enumerate() {
            let choice = InputChannels::from_index(index);
            assert_eq!(choice.index(), index);
            assert_eq!(choice.label(), *label);
            let (left, right) = choice.sides();
            match choice {
                InputChannels::Pair(_) => assert_eq!(*label, format!("{}+{}", left + 1, right + 1)),
                InputChannels::Mono(_) => assert_eq!(*label, (left + 1).to_string()),
            }
        }
        assert_eq!(InputChannels::from_value(10.2), InputChannels::Mono(6), "7");
        assert_eq!(InputChannels::from_value(999.0), InputChannels::Mono(15), "held to the list");
        assert_eq!(InputChannels::from_value(-1.0), InputChannels::Pair(0));
    }

    #[test]
    fn fits_the_channels_a_device_has() {
        assert!(InputChannels::Pair(0).fits(1), "a mono device is heard on both");
        assert!(InputChannels::Mono(1).fits(1));
        assert!(!InputChannels::Mono(2).fits(2));
        assert!(InputChannels::Mono(11).fits(12));
        assert!(!InputChannels::Pair(12).fits(12));
        let offered = (0..LABELS.len()).filter(|&i| InputChannels::from_index(i).fits(12)).count();
        assert_eq!(offered, 18, "1+2 to 11+12, and each of the twelve");
    }

    #[test]
    fn reads_its_channels_from_interleaved_input() {
        // Four channels: sample = channel * 10 + frame
        let samples: Vec<f32> = (0..3).flat_map(|frame| (0..4).map(move |c| (c * 10 + frame) as f32)).collect();
        let input = InputAudio::interleaved(&samples, 4);
        assert_eq!(InputChannels::Pair(2).frame(&input, 1), (21.0, 31.0));
        assert_eq!(InputChannels::Mono(1).frame(&input, 2), (12.0, 12.0));
        assert_eq!(InputChannels::Mono(6).frame(&input, 0), (0.0, 0.0), "a channel it hasn't");
        assert_eq!(InputChannels::Pair(0).frame(&input, 3), (0.0, 0.0), "past the block");
    }

    #[test]
    fn knows_its_own_list() {
        let parameter = parameter("channel", "Channel");
        let crate::dsp::ParameterDisplay::Discrete { labels } = parameter.display else { panic!("a choice") };
        let options: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
        assert!(is_choice(&options));
        assert!(!is_choice(&options[..3]));
        assert!(!is_choice(&["Stereo".to_string(), "1".to_string(), "2".to_string()]));
    }
}
