//! One of an Instrument's buttons (#474) as the audio thread presses it: a
//! profile's [`Button`] boiled down to the CC it sends, on which channel,
//! with which values, and whether a press holds it or flips it.
//!
//! It's `Copy` and fixed in size, so it reaches a running module by value
//! (`EngineCommand::MapButton`) and nothing allocates.

use super::profile::{Button, ButtonMode};
use crate::link::midi::Message;

/// What a button sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonMap {
    /// The CC it presses: `None` for a button that holds nothing.
    pub cc: Option<u8>,
    /// Its own channel, 1-16, whatever the Instrument's.
    pub channel: Option<u8>,
    /// Each press flips it, rather than holding it while it's down.
    pub toggle: bool,
    /// The values it sends on and off.
    pub on: u8,
    pub off: u8,
}

impl Default for ButtonMap {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl ButtonMap {
    /// A button that holds nothing, and sends nothing.
    pub const EMPTY: Self = Self { cc: None, channel: None, toggle: false, on: 127, off: 0 };

    /// What a button pressing `button` sends.
    pub fn of(button: &Button) -> Self {
        Self { cc: Some(button.cc), channel: button.channel, toggle: button.mode == ButtonMode::Toggle, on: button.on, off: button.off }
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.cc.is_none()
    }

    /// The message that turns it on or off, on its own channel or else
    /// `channel` (from 0). REAL-TIME SAFE.
    pub fn message(&self, channel: u8, on: bool) -> Option<Message> {
        let cc = self.cc?;
        let channel = self.channel.map_or(channel, |c| c.saturating_sub(1));
        Some(Message::control(channel, cc, if on { self.on } else { self.off }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_sends_its_own_values_on_its_own_channel() {
        let rec = Button { name: "Rec".into(), cc: 14, mode: ButtonMode::Momentary, on: 127, off: 0, channel: None };
        let map = ButtonMap::of(&rec);
        assert!(!map.toggle && !map.is_empty());
        assert_eq!(map.message(2, true).unwrap().bytes(), [0xB2, 14, 127]);
        assert_eq!(map.message(2, false).unwrap().bytes(), [0xB2, 14, 0]);
        let hold = Button { name: "Hold".into(), cc: 64, mode: ButtonMode::Toggle, on: 100, off: 10, channel: Some(9) };
        let map = ButtonMap::of(&hold);
        assert!(map.toggle);
        assert_eq!(map.message(2, true).unwrap().bytes(), [0xB8, 64, 100]);
        assert!(ButtonMap::EMPTY.message(0, true).is_none());
    }
}
