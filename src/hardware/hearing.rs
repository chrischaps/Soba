//! An Instrument listening back (#444): the hardware's own knobs, turned,
//! turn the node's. The MicroFreak sends its Cutoff as CC 23 as it turns;
//! an Instrument playing it hears CC 23 on its channel and moves the knob
//! that holds Cutoff, so the screen and the hardware stop disagreeing and
//! the patch keeps what the hand did.
//!
//! An [`Ear`] is one node's: it pieces the CCs a knob sends back together
//! (a 14-bit pair's two halves, an NRPN's four CCs), and tells the device's
//! own moves from its echoes. Some devices send back every value they're
//! sent, and a knob that heard those would fight the mouse turning it, so a
//! value Soba sent that knob within [`ECHO`] seconds is taken as one coming
//! back and passes it by.

use super::profile::Address;
use super::SlotMap;
use crate::link::midi::cc;
use crate::modules::instrument::SLOTS;

/// How long, in seconds, a value Soba sent a knob counts as Soba's own if
/// the device sends it back.
pub const ECHO: f64 = 0.15;

/// One channel's NRPN as heard so far: which parameter, and its value's
/// top half.
#[derive(Clone, Copy, Debug, Default)]
struct Nrpn {
    /// The parameter's halves: CC 99 and CC 98.
    parameter: (u8, u8),
    /// An RPN was chosen since: the data isn't an NRPN's.
    rpn: bool,
    /// Data Entry's top half.
    msb: u8,
}

/// One Instrument's ear: what its knobs sent lately, and the CCs heard
/// from its device this frame.
#[derive(Clone, Debug, Default)]
pub struct Ear {
    /// Each knob's value as last seen on the node, as it sends it: `None`
    /// while patched, when the cable sends instead.
    knobs: [Option<u16>; SLOTS],
    /// Values Soba sent each knob lately, and when.
    sent: [Vec<(u16, f64)>; SLOTS],
    /// Each 14-bit knob's halves as last heard.
    halves: [(u8, u8); SLOTS],
    nrpn: [Nrpn; 16],
    /// What each knob has heard since it was last taken.
    heard: [Option<u16>; SLOTS],
}

/// A knob the device turned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turned {
    pub slot: usize,
    /// Where the knob goes, 0 to 1.
    pub travel: f32,
    /// What the knob sends from there, which the device has already: a
    /// stepped knob's nearest step.
    pub value: u16,
}

impl Ear {
    /// Notes what knob `slot` sends now (`None` while patched), at `now`
    /// seconds. A value it didn't send before is one Soba sent, which the
    /// device may echo.
    pub fn knob(&mut self, slot: usize, value: Option<u16>, now: f64) {
        let Some(was) = self.knobs.get_mut(slot) else { return };
        if let Some(value) = value.filter(|&value| *was != Some(value)) {
            self.sent[slot].push((value, now));
        }
        *was = value;
        self.sent[slot].retain(|&(_, at)| now - at <= ECHO);
    }

    /// Hears a CC from the device: `controller` at `value` on `channel`
    /// (from 0). Each knob listens on its own channel or else `knobs` (from
    /// 0, the node's), for what it sends. One that nudges (relative) has no
    /// value to show, and isn't listening.
    pub fn hear(&mut self, maps: &[SlotMap; SLOTS], knobs: u8, channel: u8, controller: u8, value: u8) {
        let channel = channel & 0x0F;
        let value = value & 0x7F;
        // The NRPN this CC is a part of, if it is
        let nrpn = &mut self.nrpn[channel as usize];
        let data = match controller {
            cc::NRPN_MSB | cc::NRPN_LSB => {
                if controller == cc::NRPN_MSB {
                    nrpn.parameter.0 = value;
                } else {
                    nrpn.parameter.1 = value;
                }
                nrpn.rpn = false;
                None
            }
            cc::RPN_MSB | cc::RPN_LSB => {
                nrpn.rpn = true;
                None
            }
            cc::DATA_ENTRY if !nrpn.rpn => {
                nrpn.msb = value;
                Some(((value as u16) << 7, nrpn.parameter))
            }
            cc::DATA_ENTRY_LSB if !nrpn.rpn => Some((((nrpn.msb as u16) << 7) | value as u16, nrpn.parameter)),
            _ => None,
        };
        for (slot, map) in maps.iter().enumerate() {
            let Some(address) = map.address.filter(|_| !map.relative) else { continue };
            if map.channel.map_or(knobs, |own| own.clamp(1, 16) - 1) != channel {
                continue;
            }
            let halves = &mut self.halves[slot];
            let heard = match address {
                Address::Cc(number) if number == controller => Some(value as u16),
                Address::Cc14 { msb, lsb } if msb == controller || lsb == controller => {
                    // The top half first, as a device sends them: the bottom refines it
                    if msb == controller {
                        *halves = (value, 0);
                    } else {
                        halves.1 = value;
                    }
                    Some(((halves.0 as u16) << 7) | halves.1 as u16)
                }
                Address::Nrpn(parameter) => data.filter(|&(_, (msb, lsb))| ((msb as u16) << 7 | lsb as u16) == parameter).map(|(value, _)| value),
                _ => None,
            };
            if heard.is_some() {
                self.heard[slot] = heard;
            }
        }
    }

    /// The knobs the device turned since this was last asked, at `now`
    /// seconds: each to where it sends what was heard last. A value Soba
    /// just sent that knob is its echo, and one the knob sends already is
    /// no move, so neither is here.
    pub fn take(&mut self, maps: &[SlotMap; SLOTS], now: f64) -> Vec<Turned> {
        let mut turned = Vec::new();
        for slot in 0..SLOTS {
            let Some(heard) = self.heard[slot].take() else { continue };
            let map = &maps[slot];
            if map.is_empty() || self.sent[slot].iter().any(|&(value, at)| value == heard && now - at <= ECHO) {
                continue;
            }
            let travel = map.travel(heard);
            let value = map.value(travel);
            if self.knobs[slot] == Some(value) {
                continue;
            }
            turned.push(Turned { slot, travel, value });
        }
        turned
    }

    /// Notes knob `slot` moved to send `value` because the device turned
    /// it: the device has it already, so it's no echo to listen out for.
    pub fn turned(&mut self, slot: usize, value: u16) {
        if let Some(knob) = self.knobs.get_mut(slot) {
            *knob = Some(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{Parameter, Step};

    fn maps(parameters: &[(usize, Parameter)]) -> [SlotMap; SLOTS] {
        let mut maps = [SlotMap::EMPTY; SLOTS];
        for (slot, parameter) in parameters {
            maps[*slot] = SlotMap::of(parameter);
        }
        maps
    }

    fn slots(turned: &[Turned]) -> Vec<(usize, u16)> {
        turned.iter().map(|t| (t.slot, t.value)).collect()
    }

    #[test]
    fn a_cc_on_the_knobs_channel_turns_the_knob_that_holds_it() {
        let maps = maps(&[(0, Parameter::cc("Cutoff", 23)), (3, Parameter::cc("Resonance", 83))]);
        let mut ear = Ear::default();
        ear.hear(&maps, 2, 2, 23, 100);
        // Another channel, and a CC no knob holds, pass by
        ear.hear(&maps, 2, 5, 83, 10);
        ear.hear(&maps, 2, 2, 24, 10);
        let turned = ear.take(&maps, 0.0);
        assert_eq!(slots(&turned), [(0, 100)]);
        assert!((turned[0].travel - 100.0 / 127.0).abs() < 1e-6);
        assert!(ear.take(&maps, 0.0).is_empty(), "taken once");
        // Several in a frame: the last is where it goes
        ear.hear(&maps, 2, 2, 83, 1);
        ear.hear(&maps, 2, 2, 83, 2);
        assert_eq!(slots(&ear.take(&maps, 0.0)), [(3, 2)]);
    }

    #[test]
    fn a_knob_on_its_own_channel_listens_there() {
        let mut delay = Parameter::cc("Delay Time", 85);
        delay.channel = Some(9);
        let maps = maps(&[(0, delay)]);
        let mut ear = Ear::default();
        ear.hear(&maps, 0, 0, 85, 40);
        assert!(ear.take(&maps, 0.0).is_empty());
        ear.hear(&maps, 0, 8, 85, 40);
        assert_eq!(slots(&ear.take(&maps, 0.0)), [(0, 40)]);
    }

    #[test]
    fn an_echo_of_what_soba_sent_passes_by() {
        let maps = maps(&[(0, Parameter::cc("Cutoff", 23))]);
        let mut ear = Ear::default();
        // The mouse turns the knob: 60, 61, 62
        for (i, value) in [60, 61, 62].into_iter().enumerate() {
            ear.knob(0, Some(value), i as f64 * 0.016);
        }
        // The device sends 61 back while the knob is at 62
        ear.hear(&maps, 0, 0, 23, 61);
        assert!(ear.take(&maps, 0.05).is_empty(), "an echo");
        // Where the knob is already, no move either
        ear.hear(&maps, 0, 0, 23, 62);
        assert!(ear.take(&maps, 0.5).is_empty());
        // Long after, 61 is the hand on the device
        ear.knob(0, Some(62), 0.5);
        ear.hear(&maps, 0, 0, 23, 61);
        assert_eq!(slots(&ear.take(&maps, 0.5)), [(0, 61)]);
    }

    #[test]
    fn the_devices_own_moves_arent_taken_for_echoes() {
        // Turned back and forth on the device: each move counts
        let maps = maps(&[(0, Parameter::cc("Cutoff", 23))]);
        let mut ear = Ear::default();
        ear.knob(0, Some(64), 0.0);
        for value in [65u8, 66, 65] {
            ear.hear(&maps, 0, 0, 23, value);
            let turned = ear.take(&maps, 1.0);
            assert_eq!(slots(&turned), [(0, value as u16)]);
            ear.turned(0, value as u16);
            ear.knob(0, Some(value as u16), 1.0);
        }
    }

    #[test]
    fn a_stepped_knob_goes_to_the_nearest_step() {
        let mut arp = Parameter::cc("Arp Type", 49);
        arp.values = [("Off", 6), ("Rise 1", 19), ("Fall 2", 65)].iter().map(|&(l, v)| Step::At { label: l.into(), value: v }).collect();
        let maps = maps(&[(0, arp)]);
        let mut ear = Ear::default();
        ear.hear(&maps, 0, 0, 49, 21);
        let turned = ear.take(&maps, 0.0);
        assert_eq!(slots(&turned), [(0, 19)]);
        assert_eq!(maps[0].value(turned[0].travel), 19);
    }

    #[test]
    fn a_relative_knob_isnt_listening() {
        let mut nudge = Parameter::cc("Cutoff", 74);
        nudge.relative = true;
        let maps = maps(&[(0, nudge)]);
        let mut ear = Ear::default();
        ear.hear(&maps, 0, 0, 74, 65);
        assert!(ear.take(&maps, 0.0).is_empty());
    }

    #[test]
    fn a_pair_and_an_nrpn_are_pieced_together() {
        let mut depth = Parameter::cc("LFO1 Depth", 109);
        depth.address = Address::Cc14 { msb: 109, lsb: 61 };
        depth.range = (0, crate::hardware::profile::MAX_14);
        let mut level = Parameter::cc("Level", 0);
        level.address = Address::Nrpn(148);
        level.range = (0, crate::hardware::profile::MAX_14);
        let maps = maps(&[(1, depth), (2, level)]);
        let mut ear = Ear::default();
        ear.hear(&maps, 0, 0, 109, 70);
        ear.hear(&maps, 0, 0, 61, 40);
        // NRPN 148 is 1 and 20: then 9000's halves
        for (controller, value) in [(99, 1), (98, 20), (6, 70), (38, 40)] {
            ear.hear(&maps, 0, 0, controller, value);
        }
        assert_eq!(slots(&ear.take(&maps, 0.0)), [(1, 70 * 128 + 40), (2, 70 * 128 + 40)]);
        // Another NRPN's data, or an RPN's, passes it by
        for (controller, value) in [(99, 0), (98, 7), (6, 1), (101, 0), (100, 0), (99, 1), (101, 0), (6, 9)] {
            ear.hear(&maps, 0, 0, controller, value);
        }
        assert!(ear.take(&maps, 0.0).is_empty());
    }

    #[test]
    fn an_empty_knob_hears_nothing() {
        let maps = [SlotMap::EMPTY; SLOTS];
        let mut ear = Ear::default();
        ear.hear(&maps, 0, 0, 23, 1);
        assert!(ear.take(&maps, 0.0).is_empty());
    }
}
