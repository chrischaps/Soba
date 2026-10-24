//! The patch's MIDI CC, note and host-macro mappings, kept in one place
//!
//! The knobs' badges are worked out from these mappings
//! ([`MidiMappings::badges`]) rather than kept as a second copy beside them,
//! so taking a CC from one knob can't leave a badge on the knob it came
//! from (#321).
//!
//! A CC turns a knob round its travel, as a hand would: a fader sweeps a
//! cutoff by octaves. An endless encoder steps it from where it stands,
//! and a knob or fader with ends picks it up there rather than jumping it
//! to wherever the hardware happens to be (#424).

use std::collections::HashMap;

use crate::engine::NodeId as EngineNodeId;
use crate::graph::MidiMappingInfo;
use crate::persistence::{MidiControl, MidiInput, MidiMapping, MidiSource};
use crate::widgets::knob::KnobConfig;

/// How far round its travel one step of an endless encoder turns a knob.
pub const ENCODER_STEP: f32 = 0.01;

/// How near, round its travel, the hardware has to come to a knob to pick
/// it up: a step and a half of a CC's 128.
const CATCH: f32 = 1.5 / 127.0;

/// A mapped control as it stands, for a CC to turn it from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Standing {
    pub value: f32,
    /// Its knob turns by octaves (a frequency), not evenly.
    pub logarithmic: bool,
    /// Whole numbers only.
    pub stepped: bool,
}

/// A knob or fader with ends, against the knob it turns.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Hand {
    /// It has met the knob, and turns it now.
    caught: bool,
    /// Where the hardware was last, round the knob's travel.
    at: Option<f32>,
    /// The value it last set the knob to: anything else means something
    /// other than the hardware has turned it since.
    set: Option<f32>,
}

/// Which CC or macro turns which parameter.
#[derive(Debug, Clone, Default)]
pub struct MidiMappings {
    mappings: Vec<MidiMapping>,
    /// Each knob a fader or knob with ends turns, by (node, parameter).
    hands: HashMap<(EngineNodeId, usize), Hand>,
}

/// Two sets of mappings are the same when they map the same: where the
/// hardware stands isn't part of the patch.
impl PartialEq for MidiMappings {
    fn eq(&self, other: &Self) -> bool {
        self.mappings == other.mappings
    }
}

impl MidiMappings {
    /// The mappings loaded with a patch, in place of these.
    pub fn load(&mut self, mappings: Vec<MidiMapping>) {
        self.mappings = mappings;
        self.hands.clear();
    }

    pub fn clear(&mut self) {
        self.mappings.clear();
        self.hands.clear();
    }

    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    pub fn as_slice(&self) -> &[MidiMapping] {
        &self.mappings
    }

    pub fn iter(&self) -> std::slice::Iter<'_, MidiMapping> {
        self.mappings.iter()
    }

    /// A CC or note learned for a parameter or a Scenes pad: it takes the
    /// place of whatever turned that, and leaves anything else it turned.
    pub fn learn(&mut self, mapping: MidiMapping) {
        self.unmap(mapping.node_id, mapping.param_index, mapping.scene);
        self.mappings.retain(|m| {
            m.macro_slot.is_some()
                || !(m.source == mapping.source
                    && m.cc_number == mapping.cc_number
                    && (m.channel == 0 || mapping.channel == 0 || m.channel == mapping.channel))
        });
        self.mappings.push(mapping);
    }

    /// A host macro mapped to a parameter, in place of whatever CC or
    /// macro turned it. Other parameters may follow the same macro.
    pub fn map_macro(&mut self, mapping: MidiMapping) {
        self.unmap(mapping.node_id, mapping.param_index, None);
        self.mappings.push(mapping);
    }

    /// Lets a parameter, or with `scene` a Scenes pad, go of whatever
    /// turned it.
    pub fn unmap(&mut self, node_id: EngineNodeId, param_index: usize, scene: Option<u8>) {
        if scene.is_none() {
            self.hands.remove(&(node_id, param_index));
        }
        self.mappings.retain(|m| {
            let same = match scene {
                Some(_) => m.scene == scene,
                None => m.scene.is_none() && m.param_index == param_index,
            };
            !(m.node_id == node_id && same)
        });
    }

    /// Restyles the mapping of a parameter, or a Scenes pad: an endless
    /// encoder or not, picking up or jumping, and which way up.
    pub fn restyle(&mut self, node_id: EngineNodeId, param_index: usize, scene: Option<u8>, style: impl Fn(&mut MidiMapping)) {
        for m in &mut self.mappings {
            let same = match scene {
                Some(_) => m.scene == scene,
                None => m.scene.is_none() && m.param_index == param_index,
            };
            if m.node_id == node_id && same && m.macro_slot.is_none() {
                style(m);
            }
        }
        self.hands.remove(&(node_id, param_index));
    }

    /// Sets the ends a parameter's CC turns it between, kept inside
    /// `limits`, the parameter's own range, and the right way round (Invert
    /// turns it upside down). Returns whether they took: ends that meet
    /// leave nothing to turn, and don't.
    pub fn set_range(&mut self, node_id: EngineNodeId, param_index: usize, (min, max): (f32, f32), limits: (f32, f32)) -> bool {
        let (low, high) = (limits.0.min(limits.1), limits.0.max(limits.1));
        let (a, b) = (min.clamp(low, high), max.clamp(low, high));
        let (min, max) = (a.min(b), a.max(b));
        if max - min <= (high - low).abs() * 1e-6 || !(min.is_finite() && max.is_finite()) {
            return false;
        }
        self.restyle(node_id, param_index, None, |m| {
            m.min_value = min;
            m.max_value = max;
        });
        true
    }

    /// What `input` turns the control of mapping `index` to, from where it
    /// stands, or `None` to leave it be: a fader that hasn't picked it up
    /// yet, or a toggle's note let go.
    pub fn turn(&mut self, index: usize, input: MidiInput, standing: Option<Standing>) -> Option<f32> {
        let mapping = &self.mappings[index];
        let current = standing.map_or(mapping.min_value, |s| s.value);
        let MidiInput::Cc(value) = input else {
            return mapping.respond(input, current);
        };
        let (min, max) = (mapping.min_value, mapping.max_value);
        let knob = Curve::of(mapping, standing);

        // An endless encoder steps from where the control stands
        if let Some(steps) = mapping.encoding.steps(value) {
            let steps = if mapping.invert { -steps } else { steps };
            return match mapping.control {
                MidiControl::Knob if knob.stepped => Some((current + steps as f32).clamp(min, max)),
                MidiControl::Knob => Some(knob.value_at(knob.travel(current) + steps as f32 * ENCODER_STEP)),
                MidiControl::Select => Some((current.round() + steps as f32).clamp(min, max)),
                MidiControl::Toggle | MidiControl::Button => Some(if steps > 0 { max } else { min }),
            };
        }

        let value = if mapping.invert { 127 - value } else { value };
        if mapping.control != MidiControl::Knob {
            return mapping.respond(MidiInput::Cc(value), current);
        }
        let at = value as f32 / 127.0;
        let target = knob.value_at(at);
        if mapping.jump || standing.is_none() {
            return Some(target);
        }

        // A knob or fader with ends picks the knob up where it stands
        let tolerance = 1e-4 * (max - min).abs().max(1e-6);
        let hand = self.hands.entry((mapping.node_id, mapping.param_index)).or_default();
        if hand.set.is_some_and(|set| (set - current).abs() > tolerance) {
            // Something else turned it since: a hand, a scene, undo
            hand.caught = false;
        }
        if !hand.caught {
            let now = knob.travel(current);
            let crossed = hand.at.is_some_and(|before| (before - now) * (at - now) <= 0.0);
            hand.caught = crossed || (at - now).abs() <= CATCH;
        }
        hand.at = Some(at);
        if hand.caught {
            hand.set = Some(target);
            Some(target)
        } else {
            hand.set = None;
            None
        }
    }

    /// Where the hardware stands, round its knob's travel, for each knob
    /// still waiting to be picked up.
    pub fn waiting(&self) -> HashMap<(EngineNodeId, usize), f32> {
        self.hands.iter().filter(|(_, hand)| !hand.caught).filter_map(|(&key, hand)| Some((key, hand.at?))).collect()
    }

    /// The badge each mapped knob, toggle, dropdown or footswitch shows.
    pub fn badges(&self) -> HashMap<(EngineNodeId, usize), MidiMappingInfo> {
        self.mappings.iter().filter(|m| m.scene.is_none()).map(|m| ((m.node_id, m.param_index), info(m))).collect()
    }

    /// The badge each mapped Scenes pad shows, keyed by its slot.
    pub fn scene_badges(&self) -> HashMap<(EngineNodeId, usize), MidiMappingInfo> {
        self.mappings.iter().filter_map(|m| Some(((m.node_id, m.scene? as usize), info(m)))).collect()
    }
}

/// What a mapping's badge says.
pub fn info(m: &MidiMapping) -> MidiMappingInfo {
    let info = MidiMappingInfo {
        cc_number: m.cc_number,
        channel: m.channel,
        macro_slot: None,
        source: m.source,
        control: m.control,
        encoding: m.encoding,
        jump: m.jump,
        invert: m.invert,
    };
    match m.macro_slot {
        Some(slot) => MidiMappingInfo { cc_number: 0, channel: 0, macro_slot: Some(slot), source: MidiSource::Cc, ..info },
        None => info,
    }
}

/// How a mapped control's value runs round its travel: between the
/// mapping's ends, by octaves or evenly, and in whole steps or not.
struct Curve {
    config: KnobConfig,
    stepped: bool,
}

impl Curve {
    fn of(mapping: &MidiMapping, standing: Option<Standing>) -> Self {
        let (min, max) = (mapping.min_value.min(mapping.max_value), mapping.min_value.max(mapping.max_value));
        let logarithmic = standing.is_some_and(|s| s.logarithmic) && min >= 0.0 && max > 0.0;
        let config = KnobConfig { range: min..=max, logarithmic, ..Default::default() };
        Self { config, stepped: standing.is_some_and(|s| s.stepped) }
    }

    fn travel(&self, value: f32) -> f32 {
        let (min, max) = (*self.config.range.start(), *self.config.range.end());
        if max <= min {
            return 0.0;
        }
        self.config.travel(value.clamp(min, max)).clamp(0.0, 1.0)
    }

    fn value_at(&self, travel: f32) -> f32 {
        let value = self.config.value_at_travel(travel.clamp(0.0, 1.0));
        if self.stepped { value.round() } else { value }
    }
}

impl<'a> IntoIterator for &'a MidiMappings {
    type Item = &'a MidiMapping;
    type IntoIter = std::slice::Iter<'a, MidiMapping>;

    fn into_iter(self) -> Self::IntoIter {
        self.mappings.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::MidiEncoding;

    fn cc(cc: u8, node_id: EngineNodeId, param_index: usize) -> MidiMapping {
        MidiMapping::new(cc, 0, node_id, param_index, "Knob", 0.0, 1.0)
    }

    #[test]
    fn learning_a_taken_cc_takes_the_badge_off_the_knob_that_had_it() {
        let mut mappings = MidiMappings::default();
        mappings.learn(cc(1, 10, 0));
        mappings.learn(cc(1, 20, 3));

        let badges = mappings.badges();
        assert!(!badges.contains_key(&(10, 0)), "knob A still shows CC1");
        assert_eq!(badges.get(&(20, 3)).map(|b| b.cc_number), Some(1));
        assert_eq!(mappings.as_slice().len(), 1);
    }

    #[test]
    fn learning_replaces_what_turned_the_knob() {
        let mut mappings = MidiMappings::default();
        mappings.learn(cc(1, 10, 0));
        mappings.learn(cc(7, 10, 0));
        assert_eq!(mappings.badges().get(&(10, 0)).map(|b| b.cc_number), Some(7));
        assert_eq!(mappings.as_slice().len(), 1);
    }

    #[test]
    fn a_learned_cc_leaves_macros_alone() {
        let mut mappings = MidiMappings::default();
        mappings.map_macro(MidiMapping::from_macro(2, 10, 0, "Knob", 0.0, 1.0));
        mappings.learn(cc(0, 20, 0));
        let badges = mappings.badges();
        assert_eq!(badges.get(&(10, 0)).and_then(|b| b.macro_slot), Some(2));
        assert_eq!(badges.len(), 2);
    }

    #[test]
    fn a_macro_takes_a_knob_from_its_cc() {
        let mut mappings = MidiMappings::default();
        mappings.learn(cc(1, 10, 0));
        mappings.map_macro(MidiMapping::from_macro(0, 10, 0, "Knob", 0.0, 1.0));
        assert_eq!(mappings.badges().get(&(10, 0)).and_then(|b| b.macro_slot), Some(0));
        assert_eq!(mappings.as_slice().len(), 1);
    }

    #[test]
    fn a_pad_takes_its_note_from_the_control_that_had_it_on_its_channel_only() {
        let mut mappings = MidiMappings::default();
        let pad = |channel, node_id| MidiMapping::new(36, channel, node_id, 0, "Pedal Rec", 0.0, 1.0).note();
        mappings.learn(pad(10, 1));
        mappings.learn(pad(1, 2));
        // The same number as a CC is another control
        mappings.learn(cc(36, 3, 0));
        assert_eq!(mappings.as_slice().len(), 3);
        // On channel 10 again: it moves
        mappings.learn(pad(10, 4));
        let badges = mappings.badges();
        assert!(!badges.contains_key(&(1, 0)));
        assert_eq!(badges.get(&(4, 0)).map(|b| (b.source, b.text())), Some((MidiSource::Note, "C2 · ch 10".to_string())));
    }

    #[test]
    fn a_scene_pad_is_mapped_apart_from_the_knobs_of_its_module() {
        let mut mappings = MidiMappings::default();
        mappings.learn(cc(1, 10, 0));
        mappings.learn(MidiMapping::new(40, 10, 10, 0, "Scene", 0.0, 1.0).note().recalling(2));
        mappings.learn(MidiMapping::new(41, 10, 10, 0, "Scene", 0.0, 1.0).note().recalling(3));
        assert_eq!(mappings.badges().len(), 1, "the knob keeps its CC");
        let scenes = mappings.scene_badges();
        assert_eq!(scenes.get(&(10, 2)).map(|b| b.cc_number), Some(40));
        assert_eq!(scenes.get(&(10, 3)).map(|b| b.cc_number), Some(41));
        mappings.unmap(10, 0, Some(2));
        assert_eq!(mappings.scene_badges().len(), 1);
        assert_eq!(mappings.badges().len(), 1);
    }

    fn standing(value: f32) -> Option<Standing> {
        Some(Standing { value, logarithmic: false, stepped: false })
    }

    /// The mappings with one fader, CC 1, on a 0-1 knob.
    fn fader(style: impl Fn(&mut MidiMapping)) -> MidiMappings {
        let mut mappings = MidiMappings::default();
        let mut mapping = cc(1, 10, 0);
        style(&mut mapping);
        mappings.learn(mapping);
        mappings
    }

    fn turns(mappings: &mut MidiMappings, knob: &mut f32, values: &[u8]) -> Vec<Option<f32>> {
        values
            .iter()
            .map(|&v| {
                let turned = mappings.turn(0, MidiInput::Cc(v), standing(*knob));
                if let Some(t) = turned {
                    *knob = t;
                }
                turned
            })
            .collect()
    }

    #[test]
    fn a_fader_picks_the_knob_up_from_below() {
        let mut mappings = fader(|_| {});
        let mut knob = 0.5;
        let turned = turns(&mut mappings, &mut knob, &[10, 30, 50]);
        assert_eq!(turned, [None, None, None], "the knob waits");
        assert_eq!(knob, 0.5);
        assert!((mappings.waiting()[&(10, 0)] - 50.0 / 127.0).abs() < 1e-6, "the tick shows where the fader is");
        // Crossing the knob's value picks it up, and it follows from there
        let turned = turns(&mut mappings, &mut knob, &[70, 60]);
        assert!((turned[0].unwrap() - 70.0 / 127.0).abs() < 1e-6);
        assert!((knob - 60.0 / 127.0).abs() < 1e-6);
        assert!(mappings.waiting().is_empty());
    }

    #[test]
    fn a_fader_picks_the_knob_up_from_above_or_on_meeting_it() {
        let mut mappings = fader(|_| {});
        let mut knob = 0.25;
        assert_eq!(turns(&mut mappings, &mut knob, &[120, 90]), [None, None]);
        assert!(turns(&mut mappings, &mut knob, &[20])[0].is_some(), "it passed the knob on the way down");

        // Arriving right at it, with nothing before, picks it up too
        let mut mappings = fader(|_| {});
        let mut knob = 64.0 / 127.0;
        assert!(turns(&mut mappings, &mut knob, &[64])[0].is_some());
    }

    #[test]
    fn a_knob_turned_some_other_way_waits_to_be_picked_up_again() {
        let mut mappings = fader(|_| {});
        let mut knob = 0.0;
        turns(&mut mappings, &mut knob, &[0, 10]);
        // Turned by hand, or a scene
        knob = 0.9;
        assert_eq!(turns(&mut mappings, &mut knob, &[12]), [None]);
        assert!(turns(&mut mappings, &mut knob, &[127])[0].is_some());
    }

    #[test]
    fn a_jumping_fader_and_one_upside_down() {
        let mut mappings = fader(|m| m.jump = true);
        let mut knob = 0.5;
        assert_eq!(turns(&mut mappings, &mut knob, &[0]), [Some(0.0)]);

        let mut mappings = fader(|m| {
            m.jump = true;
            m.invert = true;
        });
        assert_eq!(turns(&mut mappings, &mut knob, &[0, 127]), [Some(1.0), Some(0.0)]);
    }

    #[test]
    fn an_encoder_steps_the_knob_from_where_it_stands() {
        let mut mappings = fader(|m| m.encoding = MidiEncoding::Relative64);
        let mut knob = 0.5;
        let turned = turns(&mut mappings, &mut knob, &[65, 65, 62]);
        assert!(turned.iter().all(Option::is_some), "an encoder never waits to pick up");
        assert!((knob - 0.5).abs() < 1e-6, "two up and two down");
        turns(&mut mappings, &mut knob, &[74]);
        assert!((knob - 0.5 - 10.0 * ENCODER_STEP).abs() < 1e-5, "a fast turn sends many steps at once");

        let mut mappings = fader(|m| {
            m.encoding = MidiEncoding::TwosComplement;
            m.invert = true;
        });
        let mut knob = 0.5;
        turns(&mut mappings, &mut knob, &[1]);
        assert!((knob - (0.5 - ENCODER_STEP)).abs() < 1e-6, "inverted, up turns it down");
        // It stops at the ends
        turns(&mut mappings, &mut knob, &[1; 80]);
        assert_eq!(knob, 0.0);
    }

    #[test]
    fn a_fader_sweeps_a_frequency_by_octaves_and_a_whole_number_knob_by_steps() {
        let mut mappings = MidiMappings::default();
        mappings.learn(MidiMapping { jump: true, ..MidiMapping::new(1, 0, 10, 0, "Cutoff", 20.0, 20000.0) });
        let log = Some(Standing { value: 1000.0, logarithmic: true, stepped: false });
        let middle = mappings.turn(0, MidiInput::Cc(64), log).unwrap();
        assert!((400.0..1000.0).contains(&middle), "halfway is far below 10 kHz: {middle}");

        let mut mappings = MidiMappings::default();
        mappings.learn(MidiMapping { encoding: MidiEncoding::Relative64, ..MidiMapping::new(1, 0, 10, 0, "Divide", 1.0, 16.0) });
        let whole = Some(Standing { value: 4.0, logarithmic: false, stepped: true });
        assert_eq!(mappings.turn(0, MidiInput::Cc(65), whole), Some(5.0), "one detent, one step");
    }

    #[test]
    fn an_encoder_steps_a_dropdown_and_a_restyle_keeps_the_mapping() {
        let mut mappings = MidiMappings::default();
        mappings.learn(MidiMapping::new(1, 0, 10, 0, "Wave", 0.0, 3.0).of(MidiControl::Select));
        mappings.restyle(10, 0, None, |m| m.encoding = MidiEncoding::Relative64);
        assert_eq!(mappings.turn(0, MidiInput::Cc(65), standing(1.0)), Some(2.0));
        assert_eq!(mappings.turn(0, MidiInput::Cc(60), standing(1.0)), Some(0.0));
        assert_eq!(mappings.badges()[&(10, 0)].encoding, MidiEncoding::Relative64);
    }

    #[test]
    fn a_range_stays_inside_the_parameter_and_the_right_way_round() {
        let mut mappings = MidiMappings::default();
        mappings.learn(MidiMapping { jump: true, ..MidiMapping::new(1, 0, 10, 0, "Cutoff", 20.0, 20000.0) });
        assert!(mappings.set_range(10, 0, (8000.0, 200.0), (20.0, 20000.0)));
        assert_eq!((mappings.as_slice()[0].min_value, mappings.as_slice()[0].max_value), (200.0, 8000.0));
        assert!(mappings.set_range(10, 0, (5.0, 90000.0), (20.0, 20000.0)));
        assert_eq!((mappings.as_slice()[0].min_value, mappings.as_slice()[0].max_value), (20.0, 20000.0));
        assert!(!mappings.set_range(10, 0, (300.0, 300.0), (20.0, 20000.0)), "ends that meet");
        assert_eq!(mappings.as_slice()[0].max_value, 20000.0);

        // The fader turns the knob between the new ends
        mappings.set_range(10, 0, (100.0, 1000.0), (20.0, 20000.0));
        assert_eq!(mappings.turn(0, MidiInput::Cc(127), standing(500.0)), Some(1000.0));
        assert_eq!(mappings.turn(0, MidiInput::Cc(0), standing(500.0)), Some(100.0));
        // A macro's range is the DAW's
        mappings.map_macro(MidiMapping::from_macro(0, 20, 0, "Knob", 0.0, 1.0));
        mappings.set_range(20, 0, (0.2, 0.4), (0.0, 1.0));
        assert_eq!(mappings.as_slice()[1].max_value, 1.0);
    }

    #[test]
    fn unmapping_and_clearing_take_the_badges_off() {
        let mut mappings = MidiMappings::default();
        mappings.learn(cc(1, 10, 0));
        mappings.learn(cc(2, 20, 0));
        mappings.unmap(10, 0, None);
        assert!(!mappings.badges().contains_key(&(10, 0)));
        mappings.clear();
        assert!(mappings.badges().is_empty());
    }
}
