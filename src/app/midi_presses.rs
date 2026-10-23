//! Footswitches pressed from MIDI (#423): a pad tapped quicker than a UI
//! frame sends its press and its release together, and the audio thread
//! only looks for a press at the start of a block, so it would never see
//! it. A release that comes too soon after its press waits here until the
//! press has been down for [`MIN_PRESS_SECONDS`], as a click on the
//! node's footswitch does.

use std::collections::HashMap;

pub use crate::graph::sampler_take::MIN_PRESS_SECONDS;

/// The footswitches MIDI holds down, and the releases waiting their turn.
#[derive(Default)]
pub struct Presses {
    /// When each footswitch held went down: (engine node, parameter).
    down: HashMap<(u64, usize), f64>,
    /// Releases to send once due: when, and the footswitch with its value.
    waiting: Vec<(f64, (u64, usize), f32)>,
}

impl Presses {
    /// A footswitch set to `value` at `now`, `pressed` or let go: the value
    /// to send now, or `None` for a release held back until [`due`](Self::due).
    pub fn set(&mut self, now: f64, node_id: u64, param_index: usize, value: f32, pressed: bool) -> Option<f32> {
        let key = (node_id, param_index);
        if pressed {
            // A press supersedes a release still waiting
            self.waiting.retain(|(_, k, _)| *k != key);
            self.down.insert(key, now);
            return Some(value);
        }
        match self.down.remove(&key) {
            Some(at) if now - at < MIN_PRESS_SECONDS => {
                self.waiting.push((at + MIN_PRESS_SECONDS, key, value));
                None
            }
            _ => Some(value),
        }
    }

    /// The releases due by `now`, taken from the wait.
    pub fn due(&mut self, now: f64) -> Vec<(u64, usize, f32)> {
        let mut due = Vec::new();
        self.waiting.retain(|&(at, (node_id, param_index), value)| {
            let ready = at <= now;
            if ready {
                due.push((node_id, param_index, value));
            }
            !ready
        });
        due
    }

    /// Is a release waiting? The frame loop keeps drawing until it's sent.
    pub fn waiting(&self) -> bool {
        !self.waiting.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_quicker_than_a_frame_is_held_down_long_enough_to_be_seen() {
        let mut presses = Presses::default();
        assert_eq!(presses.set(1.0, 7, 2, 1.0, true), Some(1.0));
        assert_eq!(presses.set(1.01, 7, 2, 0.0, false), None);
        assert!(presses.waiting());
        assert!(presses.due(1.03).is_empty());
        assert_eq!(presses.due(1.0 + MIN_PRESS_SECONDS), [(7, 2, 0.0)]);
        assert!(!presses.waiting());
    }

    #[test]
    fn a_held_press_lets_go_at_once() {
        let mut presses = Presses::default();
        presses.set(1.0, 7, 2, 1.0, true);
        assert_eq!(presses.set(1.5, 7, 2, 0.0, false), Some(0.0));
        // A release with no press seen (a CC at 0) goes straight through
        assert_eq!(presses.set(2.0, 7, 3, 0.0, false), Some(0.0));
        assert!(!presses.waiting());
    }

    #[test]
    fn a_second_press_takes_the_place_of_a_release_still_waiting() {
        let mut presses = Presses::default();
        presses.set(1.0, 7, 2, 1.0, true);
        presses.set(1.01, 7, 2, 0.0, false);
        presses.set(1.02, 7, 2, 1.0, true);
        assert!(presses.due(5.0).is_empty(), "held down again");
    }
}
