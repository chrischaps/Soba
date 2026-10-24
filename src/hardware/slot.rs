//! One of an Instrument's knobs (#442) as the audio thread sends it: a
//! profile's [`Parameter`] boiled down to what it takes to turn a knob's
//! travel, 0 to 1, into a message. Where it goes, across which values, and
//! for a stepped one, exactly which values.
//!
//! It's `Copy` and fixed in size, so it reaches a running module by value
//! (`EngineCommand::MapSlot`) and nothing allocates.

use super::profile::{Address, Curve, Parameter};
use crate::link::midi::Control;

/// The most named values a slot keeps. A parameter with more is sent
/// across its first this many.
pub const MAX_STEPS: usize = 32;

/// What a slot sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotMap {
    /// Where its value goes: `None` for a slot that holds nothing.
    pub address: Option<Address>,
    /// Its own channel, 1-16, whatever the Instrument's.
    pub channel: Option<u8>,
    /// The lowest and highest values it sends.
    pub range: (u16, u16),
    /// Fine at the bottom of the knob's travel.
    pub log: bool,
    /// It nudges the device's value: the middle of its range is no change.
    pub relative: bool,
    /// For a stepped one, the value each step sends, in order.
    steps: [u16; MAX_STEPS],
    step_count: u8,
}

impl Default for SlotMap {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl SlotMap {
    /// A slot that holds nothing, and sends nothing.
    pub const EMPTY: Self = Self { address: None, channel: None, range: (0, 127), log: false, relative: false, steps: [0; MAX_STEPS], step_count: 0 };

    /// What a knob holding `parameter` sends.
    pub fn of(parameter: &Parameter) -> Self {
        let mut map = Self {
            address: Some(parameter.address),
            channel: parameter.channel,
            range: parameter.range,
            log: parameter.curve == Curve::Log,
            relative: parameter.relative,
            ..Self::EMPTY
        };
        for (place, (_, value)) in map.steps.iter_mut().zip(parameter.steps()) {
            *place = value;
            map.step_count += 1;
        }
        map
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.address.is_none()
    }

    /// The values its steps send, for a stepped one.
    pub fn steps(&self) -> &[u16] {
        &self.steps[..self.step_count as usize]
    }

    /// What a knob at `travel` (0 to 1) sends: a step's value, or a place
    /// across the range, as its curve spreads it. REAL-TIME SAFE.
    pub fn value(&self, travel: f32) -> u16 {
        let travel = if travel.is_finite() { travel.clamp(0.0, 1.0) } else { 0.0 };
        let steps = self.steps();
        if !steps.is_empty() {
            let index = ((travel * steps.len() as f32) as usize).min(steps.len() - 1);
            return steps[index];
        }
        let (lo, hi) = self.range;
        let shaped = if self.log { travel * travel } else { travel };
        (lo as f32 + shaped * hi.saturating_sub(lo) as f32).round() as u16
    }

    /// Where a knob sits to send `value`: the middle of a step's share of
    /// the travel, or its place across the range. [`value`](Self::value)
    /// sends it back.
    pub fn travel(&self, value: u16) -> f32 {
        let steps = self.steps();
        if let Some(index) = (0..steps.len()).min_by_key(|&i| steps[i].abs_diff(value)) {
            return (index as f32 + 0.5) / steps.len() as f32;
        }
        let (lo, hi) = self.range;
        let span = hi.saturating_sub(lo).max(1) as f32;
        let place = ((value.clamp(lo, hi) - lo) as f32 / span).clamp(0.0, 1.0);
        if self.log { place.sqrt() } else { place }
    }

    /// `value` as a message to `address`, on the slot's own channel or else
    /// `channel` (from 0, the Instrument's). A 14-bit pair on CC n and
    /// n + 32 goes as MIDI Out's 14-bit lanes do. REAL-TIME SAFE.
    pub fn control(&self, address: Address, channel: u8, value: u16) -> Control {
        let channel = self.channel.map_or(channel, |own| own.clamp(1, 16) - 1);
        match address {
            Address::Cc(number) => Control::Cc { channel, number: number & 0x7F, value: value.min(127) as u8 },
            Address::Cc14 { msb, lsb } if msb < 32 && lsb == msb + 32 => Control::Cc14 { channel, number: msb, value: value.min(super::profile::MAX_14) },
            Address::Cc14 { msb, lsb } => Control::Pair { channel, msb: msb & 0x7F, lsb: lsb & 0x7F, value: value.min(super::profile::MAX_14) },
            Address::Nrpn(parameter) => Control::Nrpn { channel, parameter: parameter & super::profile::MAX_14, value: value.min(super::profile::MAX_14) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::Step;

    fn stepped(values: &[(&str, u16)]) -> Parameter {
        let mut p = Parameter::cc("Arp", 49);
        p.values = values.iter().map(|&(label, value)| Step::At { label: label.to_string(), value }).collect();
        p
    }

    #[test]
    fn a_sweep_spreads_the_travel_over_its_range() {
        let mut p = Parameter::cc("Cutoff", 23);
        p.range = (10, 110);
        let map = SlotMap::of(&p);
        assert_eq!([map.value(0.0), map.value(0.5), map.value(1.0), map.value(2.0)], [10, 60, 110, 110]);
        assert_eq!(map.value(f32::NAN), 10, "nonsense rests at the bottom");
        assert!((map.travel(60) - 0.5).abs() < 1e-6);
        // Log is fine at the bottom, and travel undoes it
        p.curve = Curve::Log;
        let map = SlotMap::of(&p);
        assert_eq!(map.value(0.5), 35);
        assert_eq!(map.value(map.travel(35)), 35);
    }

    #[test]
    fn steps_send_their_own_values_each_across_its_share() {
        // The volca fm's Arp Type: values Korg chose, not an even spread
        let map = SlotMap::of(&stepped(&[("Off", 6), ("Rise 1", 19), ("Rise 2", 32), ("Fall 2", 65)]));
        assert_eq!(map.steps(), [6, 19, 32, 65]);
        assert_eq!([map.value(0.0), map.value(0.24), map.value(0.26), map.value(1.0)], [6, 6, 19, 65]);
        assert_eq!(map.travel(65), 0.875, "the middle of its quarter");
        assert_eq!(map.value(map.travel(32)), 32);
        // Labels alone share the range, each sent from its middle
        let mut p = Parameter::cc("Mode", 9);
        p.values = vec![Step::Label("A".into()), Step::Label("B".into())];
        assert_eq!(SlotMap::of(&p).steps(), [32, 96]);
    }

    #[test]
    fn controls_go_to_the_address_on_the_right_channel() {
        let cc = SlotMap::of(&Parameter::cc("Cutoff", 23));
        let Some(address) = cc.address else { panic!("holds a CC") };
        assert_eq!(cc.control(address, 2, 64), Control::Cc { channel: 2, number: 23, value: 64 });
        // Its own channel, 1-16, beats the Instrument's
        let mut delay = Parameter::cc("Delay Time", 85);
        delay.channel = Some(9);
        let own = SlotMap::of(&delay);
        assert_eq!(own.control(own.address.unwrap(), 0, 1), Control::Cc { channel: 8, number: 85, value: 1 });
        // A pair of a device's own, a standard pair, and an NRPN
        let address = Address::Cc14 { msb: 109, lsb: 61 };
        assert_eq!(cc.control(address, 0, 9000), Control::Pair { channel: 0, msb: 109, lsb: 61, value: 9000 });
        assert_eq!(cc.control(Address::Cc14 { msb: 1, lsb: 33 }, 0, 9000), Control::Cc14 { channel: 0, number: 1, value: 9000 });
        assert_eq!(cc.control(Address::Nrpn(148), 0, 300), Control::Nrpn { channel: 0, parameter: 148, value: 300 });
        assert!(SlotMap::EMPTY.is_empty() && !cc.is_empty());
    }

    #[test]
    fn a_slot_is_small_enough_to_send_by_value() {
        assert!(std::mem::size_of::<SlotMap>() <= 96, "{}", std::mem::size_of::<SlotMap>());
    }
}
