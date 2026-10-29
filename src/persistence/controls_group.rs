//! Controls became a group of Controls (#483).
//!
//! Controls (#427) was four slots in one node, `input.controls`, A to D,
//! each hearing one fader, knob or wheel. Each slot is a Control module
//! now, and four of them in a group are what Controls was: a patch that has
//! one opens with a group named Controls in its place, holding four
//! Controls in a row, each hearing what its slot heard, the Slew they
//! shared copied to each, hearing its Device. The group keeps the module's
//! ID, so the cables outside it stay where they were: **A** to **D** leave
//! from the group's jacks of those names, and a **MIDI** cable from a MIDI
//! In goes into the group's MIDI jack, and on to every Control.
//!
//! No version marks this: only an old patch holds `input.controls`. An
//! older app opens the group, but not the Controls in it, which it doesn't
//! know.
//!
//! [`group_of_controls`] builds the group, for the Library's MiniLab 3
//! Faders and Controls ×4 too.

use std::collections::BTreeMap;

use crate::modules::control::{self, Control, Source};

use super::patch::{ConnectionData, GroupData, JackData, NamedParameter, NodeData, ParameterValue, Patch};

/// The module ID patches knew Controls by.
pub const OLD_ID: &str = "input.controls";

/// The old Controls' slots, by letter.
const LETTERS: [&str; 4] = ["A", "B", "C", "D"];

/// Where the Controls sit inside the group: in a row, this far apart.
const COLUMN: f32 = 210.0;

/// What one Control of a group hears, and the name of its jack.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlJack {
    pub jack: String,
    pub source: Source,
    pub cc: u8,
    /// 0 is Omni, 1-16 a channel.
    pub channel: u8,
    /// The Encoding choice: 0 Absolute, or an endless encoder's.
    pub encoding: usize,
    /// The Range choice: 0 Auto, 1 0 to 1, 2 -1 to 1.
    pub range: usize,
}

impl ControlJack {
    /// A Control on `source` (and `cc`, for a CC), Omni, Absolute, Auto,
    /// its jack named `jack`.
    pub fn new(jack: impl Into<String>, source: Source, cc: u8) -> Self {
        Self { jack: jack.into(), source, cc, channel: 0, encoding: 0, range: 0 }
    }
}

/// A group named `name` holding a Control for each of `controls`, in a
/// row, each slewed by `slew` seconds, with `labels` (a Device), its fader
/// pinned to the group's face under its jack's name. Its jacks
/// are a MIDI input into every Control, and each Control's Out by its
/// jack's name. The Controls take IDs from `next` up.
pub fn group_of_controls(id: u64, name: &str, position: (f32, f32), controls: &[ControlJack], slew: f32, labels: &BTreeMap<String, String>, next: &mut u64) -> GroupData {
    let midi = crate::modules::midi_in::PORT_NAME;
    let mut outputs = Vec::new();
    let mut nodes = Vec::new();
    let mut connections = Vec::new();
    for (index, jack) in controls.iter().enumerate() {
        *next += 1;
        let control_id = *next;
        let mut node = NodeData::new(control_id, control::ID, (index as f32 * COLUMN, 0.0));
        node.parameters = vec![
            NamedParameter::new("Source", ParameterValue::Select(jack.source as usize)),
            NamedParameter::new("CC", ParameterValue::Number(jack.cc as f32)),
            NamedParameter::new("Channel", ParameterValue::Select(jack.channel as usize)),
            NamedParameter::new("Encoding", ParameterValue::Select(jack.encoding)),
            NamedParameter::new("Range", ParameterValue::Select(jack.range)),
            NamedParameter::new("Slew", ParameterValue::Number(slew)),
        ];
        node.labels = labels.clone();
        // Its fader on the group's face, by its jack's name (#482's pads' way)
        let fader = crate::graph::groups::CONTROL_FADER.to_string();
        node.pinned.insert(fader.clone(), 1);
        node.face_labels.insert(fader, jack.jack.clone());
        nodes.push(node);
        connections.push(ConnectionData::new(id, midi, control_id, midi));
        connections.push(ConnectionData::new(control_id, "Out", id, jack.jack.clone()));
        outputs.push(JackData { name: jack.jack.clone(), signal: "Control".to_string() });
    }
    GroupData {
        id,
        name: name.to_string(),
        position,
        inputs: vec![JackData { name: midi.to_string(), signal: "MIDI".to_string() }],
        outputs,
        inputs_position: (-280.0, 0.0),
        outputs_position: (controls.len() as f32 * COLUMN + 40.0, 0.0),
        nodes,
        connections,
        groups: Vec::new(),
    }
}

/// What each of an old Controls' slots heard, from its parameters by name,
/// and the Slew they shared. A slot not written hears what it heard when
/// the module was added.
fn old_slots(node: &NodeData) -> ([ControlJack; 4], f32) {
    let value = |name: &str| node.parameters.iter().find(|p| p.name == name).map(|p| p.value.as_f32());
    let added = [(Source::ModWheel, 1), (Source::Bend, 0), (Source::Pressure, 0), (Source::Cc, 74)];
    let slots = std::array::from_fn(|slot| {
        let letter = LETTERS[slot];
        let (source, cc) = added[slot];
        let choice = |name: &str, most: f32| value(&format!("{name} {letter}")).map_or(0, |v| v.round().clamp(0.0, most) as usize);
        ControlJack {
            jack: letter.to_string(),
            source: value(&format!("Source {letter}")).map_or(source, Source::from_param),
            cc: value(&format!("CC {letter}")).map_or(cc, |v| v.round().clamp(0.0, 127.0) as u8),
            channel: choice("Channel", 16.0) as u8,
            encoding: choice("Encoding", 2.0),
            range: choice("Range", 2.0),
        }
    });
    (slots, value("Slew").unwrap_or(control::DEFAULT_SLEW))
}

/// Every Controls in `patch` becomes a group of four Controls (see the
/// module's notes). A MIDI mapping to its Slew follows it to Control A's;
/// the rest, to what its slots hear, go.
pub(super) fn controls_to_groups(patch: &mut Patch) {
    fn last_id(nodes: &[NodeData], groups: &[GroupData]) -> u64 {
        let here = nodes.iter().map(|n| n.id).chain(groups.iter().map(|g| g.id)).max().unwrap_or(0);
        groups.iter().map(|g| last_id(&g.nodes, &g.groups)).fold(here, u64::max)
    }

    fn migrate(nodes: &mut Vec<NodeData>, groups: &mut Vec<GroupData>, next: &mut u64, moved: &mut Vec<(u64, u64)>) {
        for group in groups.iter_mut() {
            migrate(&mut group.nodes, &mut group.groups, next, moved);
        }
        while let Some(at) = nodes.iter().position(|n| n.module_id == OLD_ID) {
            let old = nodes.remove(at);
            let (slots, slew) = old_slots(&old);
            let group = group_of_controls(old.id, "Controls", old.position, &slots, slew, &old.labels, next);
            moved.push((old.id, group.nodes[0].id));
            groups.push(group);
        }
    }

    let mut next = last_id(&patch.nodes, &patch.groups);
    let mut moved = Vec::new();
    migrate(&mut patch.nodes, &mut patch.groups, &mut next, &mut moved);
    if moved.is_empty() {
        return;
    }
    patch.midi_mappings.retain_mut(|mapping| {
        let Some(&(_, first)) = moved.iter().find(|(old, _)| *old == mapping.node_id) else { return true };
        if mapping.scene.is_some() {
            return true;
        }
        if mapping.param_name != "Slew" {
            return false;
        }
        mapping.node_id = first;
        mapping.param_index = Control::PARAM_SLEW;
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::patch_from_json;

    /// An old patch: a Controls hearing the mod wheel, CC 82 on channel 2
    /// as an encoder, the pitch bend from -1 to 1 and slot D left as it was
    /// added, from a MIDI In, its A into a filter and D into a VCA, and a
    /// mapping on its Slew and one on a slot's CC.
    fn old_patch() -> String {
        r#"{"name": "Old controls", "version": 6,
            "nodes": [
                {"id": 2, "module_id": "input.controls", "position": [40.0, 665.0], "labels": {"Device": "MiniLab3"},
                 "parameters": [{"name": "Slew", "type": "Number", "value": 0.05},
                                {"name": "Source A", "type": "Select", "value": 2},
                                {"name": "CC A", "type": "Number", "value": 1.0},
                                {"name": "Source B", "type": "Select", "value": 0},
                                {"name": "CC B", "type": "Number", "value": 82.0},
                                {"name": "Channel B", "type": "Select", "value": 2},
                                {"name": "Encoding B", "type": "Select", "value": 1},
                                {"name": "Source C", "type": "Select", "value": 1},
                                {"name": "Range C", "type": "Select", "value": 2}]},
                {"id": 5, "module_id": "filter.svf", "position": [400.0, 0.0], "parameters": []},
                {"id": 6, "module_id": "util.vca", "position": [400.0, 300.0], "parameters": []},
                {"id": 7, "module_id": "input.midi", "position": [0.0, 0.0], "parameters": []}
            ],
            "connections": [
                {"from_node": 7, "from_port": "MIDI", "to_node": 2, "to_port": "MIDI"},
                {"from_node": 2, "from_port": "A", "to_node": 5, "to_port": "Cutoff"},
                {"from_node": 2, "from_port": "D", "to_node": 6, "to_port": "CV"}
            ],
            "midi_mappings": [
                {"cc_number": 40, "channel": 0, "node_id": 2, "param_index": 0, "param_name": "Slew", "min_value": 0.0, "max_value": 2.0},
                {"cc_number": 41, "channel": 0, "node_id": 2, "param_index": 2, "param_name": "CC A", "min_value": 0.0, "max_value": 127.0}
            ]}"#
        .to_string()
    }

    fn param(node: &NodeData, name: &str) -> f32 {
        node.parameters.iter().find(|p| p.name == name).unwrap().value.as_f32()
    }

    #[test]
    fn an_old_controls_opens_as_a_group_of_four_controls_where_it_was() {
        let patch = patch_from_json(&old_patch()).unwrap();
        assert!(patch.all_nodes().iter().all(|n| n.module_id != OLD_ID));
        let [group] = patch.groups.as_slice() else { panic!("one group") };
        assert_eq!((group.id, group.name.as_str(), group.position), (2, "Controls", (40.0, 665.0)), "its ID, so the cables outside stay");
        assert_eq!(group.nodes.len(), 4);
        assert!(group.nodes.iter().all(|n| n.module_id == control::ID && n.labels.get("Device").map(String::as_str) == Some("MiniLab3")));
        assert!(group.nodes.iter().all(|n| param(n, "Slew") == 0.05), "the Slew they shared, each");
        let faces: Vec<(Option<&u8>, Option<&str>)> = group.nodes.iter().map(|n| (n.pinned.get("Source"), n.face_labels.get("Source").map(String::as_str))).collect();
        assert_eq!(faces, LETTERS.map(|letter| (Some(&1), Some(letter))), "each fader on the group's face, by its jack's letter");
        let heard: Vec<[f32; 5]> = group.nodes.iter().map(|n| ["Source", "CC", "Channel", "Encoding", "Range"].map(|name| param(n, name))).collect();
        assert_eq!(
            heard,
            [[2.0, 1.0, 0.0, 0.0, 0.0], [0.0, 82.0, 2.0, 1.0, 0.0], [1.0, 0.0, 0.0, 0.0, 2.0], [0.0, 74.0, 0.0, 0.0, 0.0]],
            "slot D as it was added: CC 74"
        );

        // Jacks A to D, each from its Control, and the MIDI jack into all four
        let jacks: Vec<&str> = group.outputs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(jacks, LETTERS);
        assert!(group.outputs.iter().all(|j| j.signal == "Control"));
        assert_eq!(group.inputs[0].name, "MIDI");
        assert_eq!(group.connections.iter().filter(|c| c.from_node == 2 && c.from_port == "MIDI").count(), 4);
        let from = |to: u64, port: &str| patch.connections.iter().find(|c| c.to_node == to && c.to_port == port).map(|c| (c.from_node, c.from_port.as_str()));
        assert_eq!(from(5, "Cutoff"), Some((2, "A")));
        assert_eq!(from(6, "CV"), Some((2, "D")));
        assert_eq!(from(2, "MIDI"), Some((7, "MIDI")), "the MIDI In plugs into the group");

        // Slew's mapping follows Control A; one to a slot's CC goes
        let [mapping] = patch.midi_mappings.as_slice() else { panic!("one mapping") };
        assert_eq!((mapping.node_id, mapping.param_name.as_str(), mapping.param_index), (group.nodes[0].id, "Slew", Control::PARAM_SLEW));
        let ids: std::collections::BTreeSet<u64> = patch.all_nodes().iter().map(|n| n.id).chain([group.id]).collect();
        assert_eq!(ids.len(), 3 + 4 + 1, "every ID its own");
    }

    #[test]
    fn a_controls_inside_a_group_becomes_a_group_inside_it() {
        let json = r#"{"name": "Nested", "version": 6, "nodes": [], "connections": [],
            "groups": [{"id": 10, "name": "Voice", "position": [0.0, 0.0], "inputs": [], "outputs": [],
                "inputs_position": [0.0, 0.0], "outputs_position": [0.0, 0.0],
                "nodes": [{"id": 11, "module_id": "input.controls", "position": [5.0, 5.0], "parameters": []},
                          {"id": 12, "module_id": "filter.svf", "position": [300.0, 0.0], "parameters": []}],
                "connections": [{"from_node": 11, "from_port": "B", "to_node": 12, "to_port": "Cutoff"}]}]}"#;
        let patch = patch_from_json(json).unwrap();
        let voice = &patch.groups[0];
        assert_eq!(voice.nodes.len(), 1);
        let [controls] = voice.groups.as_slice() else { panic!("the Controls, inside the Voice") };
        assert_eq!(controls.id, 11);
        assert!(controls.nodes.iter().all(|n| n.id > 12));
        let sources: Vec<f32> = controls.nodes.iter().map(|n| param(n, "Source")).collect();
        assert_eq!(sources, [2.0, 1.0, 3.0, 0.0], "the mod wheel, the bend, the pressure and CC 74, as Controls was added");
        assert!(controls.nodes.iter().all(|n| param(n, "Slew") == control::DEFAULT_SLEW));
        assert!(voice.connections.iter().any(|c| c.from_node == 11 && c.from_port == "B" && c.to_port == "Cutoff"));
    }
}
