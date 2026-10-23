//! The patch's MIDI CC, note and host-macro mappings, kept in one place
//!
//! The knobs' badges are worked out from these mappings
//! ([`MidiMappings::badges`]) rather than kept as a second copy beside them,
//! so taking a CC from one knob can't leave a badge on the knob it came
//! from (#321).

use std::collections::HashMap;

use crate::engine::NodeId as EngineNodeId;
use crate::graph::MidiMappingInfo;
use crate::persistence::{MidiMapping, MidiSource};

/// Which CC or macro turns which parameter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MidiMappings(Vec<MidiMapping>);

impl MidiMappings {
    /// The mappings loaded with a patch, in place of these.
    pub fn load(&mut self, mappings: Vec<MidiMapping>) {
        self.0 = mappings;
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn as_slice(&self) -> &[MidiMapping] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, MidiMapping> {
        self.0.iter()
    }

    /// A CC or note learned for a parameter or a Scenes pad: it takes the
    /// place of whatever turned that, and leaves anything else it turned.
    pub fn learn(&mut self, mapping: MidiMapping) {
        self.unmap(mapping.node_id, mapping.param_index, mapping.scene);
        self.0.retain(|m| {
            m.macro_slot.is_some()
                || !(m.source == mapping.source
                    && m.cc_number == mapping.cc_number
                    && (m.channel == 0 || mapping.channel == 0 || m.channel == mapping.channel))
        });
        self.0.push(mapping);
    }

    /// A host macro mapped to a parameter, in place of whatever CC or
    /// macro turned it. Other parameters may follow the same macro.
    pub fn map_macro(&mut self, mapping: MidiMapping) {
        self.unmap(mapping.node_id, mapping.param_index, None);
        self.0.push(mapping);
    }

    /// Lets a parameter, or with `scene` a Scenes pad, go of whatever
    /// turned it.
    pub fn unmap(&mut self, node_id: EngineNodeId, param_index: usize, scene: Option<u8>) {
        self.0.retain(|m| {
            let same = match scene {
                Some(_) => m.scene == scene,
                None => m.scene.is_none() && m.param_index == param_index,
            };
            !(m.node_id == node_id && same)
        });
    }

    /// The badge each mapped knob, toggle, dropdown or footswitch shows.
    pub fn badges(&self) -> HashMap<(EngineNodeId, usize), MidiMappingInfo> {
        self.0.iter().filter(|m| m.scene.is_none()).map(|m| ((m.node_id, m.param_index), info(m))).collect()
    }

    /// The badge each mapped Scenes pad shows, keyed by its slot.
    pub fn scene_badges(&self) -> HashMap<(EngineNodeId, usize), MidiMappingInfo> {
        self.0.iter().filter_map(|m| Some(((m.node_id, m.scene? as usize), info(m)))).collect()
    }
}

/// What a mapping's badge says.
fn info(m: &MidiMapping) -> MidiMappingInfo {
    match m.macro_slot {
        Some(slot) => MidiMappingInfo { cc_number: 0, channel: 0, macro_slot: Some(slot), source: MidiSource::Cc },
        None => MidiMappingInfo { cc_number: m.cc_number, channel: m.channel, macro_slot: None, source: m.source },
    }
}

impl<'a> IntoIterator for &'a MidiMappings {
    type Item = &'a MidiMapping;
    type IntoIter = std::slice::Iter<'a, MidiMapping>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
