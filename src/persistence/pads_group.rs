//! Pads became a group of Pads (#480).
//!
//! Pads (#428) was eight pads in one node, `input.pads`. Each pad is a Pad
//! module now, and eight of them in a group are what Pads was: a patch that
//! has one opens with a group named Pads in its place, holding eight Pads
//! laid out 2×4, each on the note and channel its pad heard, in its Mode,
//! hearing its Device. The group keeps the module's ID, so the cables
//! outside it stay where they were:
//!
//! - **Gate 1** to **Gate 8** leave from the group's jacks of those names.
//! - **Hit** went: it gave five drums each its own velocity, which each
//!   Pad's Velocity now does. A cable from Hit (or from the polyphonic
//!   Velocity) leaves from the **Vel** jack of the pad whose Gate feeds the
//!   same module, so a drum's Accent hears its own pad.
//! - **Pressure**, the hardest of the held pads, leaves from the
//!   **Pressure** jack of the pad that drum followed.
//! - A **MIDI** cable from a MIDI In goes into the group's MIDI jack, and on
//!   to every Pad.
//!
//! No version marks this: only an old patch holds `input.pads`. An older
//! app opens the group, but not the Pads in it, which it doesn't know.
//!
//! [`group_of_pads`] builds the group, for the Library's Pads ×8 too.

use std::collections::BTreeMap;

use crate::modules::pad;

use super::patch::{ConnectionData, GroupData, JackData, NamedParameter, NodeData, ParameterValue, Patch};

/// The module ID patches knew Pads by.
pub const OLD_ID: &str = "input.pads";

/// How many pads Pads had.
const PADS: usize = 8;

/// Where the Pads sit inside the group: four to a row, this far apart.
const COLUMNS: usize = 4;
const COLUMN: f32 = 250.0;
const ROW: f32 = 290.0;

/// What one pad of a group of Pads listens to, and which jacks of its own
/// the group gives it besides Gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadJacks {
    pub note: u8,
    /// 0 is Omni, 1-16 a channel.
    pub channel: u8,
    /// A **Vel** jack, for its Velocity.
    pub velocity: bool,
    /// A **Pressure** jack.
    pub pressure: bool,
}

/// A group named `name` holding a Pad for each of `pads`, laid out four to
/// a row, each in `mode` (Momentary 0, Latch 1, Trigger 2), with `labels`
/// (a Device). Its jacks
/// are a MIDI input into every Pad, and per pad its Gate, then any Vel and
/// Pressure. The Pads take IDs from `next` up.
pub fn group_of_pads(id: u64, name: &str, position: (f32, f32), pads: &[PadJacks], mode: usize, labels: &BTreeMap<String, String>, next: &mut u64) -> GroupData {
    let midi = crate::modules::midi_in::PORT_NAME;
    let mut outputs = Vec::new();
    let mut nodes = Vec::new();
    let mut connections = Vec::new();
    for (index, jacks) in pads.iter().enumerate() {
        *next += 1;
        let pad_id = *next;
        let at = ((index % COLUMNS) as f32 * COLUMN, (index / COLUMNS) as f32 * ROW);
        let mut node = NodeData::new(pad_id, pad::ID, at);
        node.parameters = vec![
            NamedParameter::new("Mode", ParameterValue::Select(mode)),
            NamedParameter::new("Note", ParameterValue::Number(jacks.note as f32)),
            NamedParameter::new("Channel", ParameterValue::Select(jacks.channel as usize)),
            NamedParameter::new("Press", ParameterValue::Number(0.0)),
        ];
        node.labels = labels.clone();
        nodes.push(node);
        connections.push(ConnectionData::new(id, midi, pad_id, midi));
        let n = index + 1;
        let mut jack = |port: &str, jack: String, signal: &str| {
            connections.push(ConnectionData::new(pad_id, port, id, jack.clone()));
            outputs.push(JackData { name: jack, signal: signal.to_string() });
        };
        jack("Gate", format!("Gate {n}"), "Gate");
        if jacks.velocity {
            jack("Velocity", format!("Vel {n}"), "Control");
        }
        if jacks.pressure {
            jack("Pressure", format!("Pressure {n}"), "Control");
        }
    }
    let rows = pads.len().div_ceil(COLUMNS).max(1) as f32;
    GroupData {
        id,
        name: name.to_string(),
        position,
        inputs: vec![JackData { name: midi.to_string(), signal: "MIDI".to_string() }],
        outputs,
        inputs_position: (-280.0, (rows - 1.0) * ROW / 2.0),
        outputs_position: (COLUMNS as f32 * COLUMN + 40.0, (rows - 1.0) * ROW / 2.0),
        nodes,
        connections,
        groups: Vec::new(),
    }
}

/// What each of an old Pads' pads listened to, from its parameters by
/// name: pad 1 on Base and Channel, the rest on their own Note and Channel
/// or following pad 1 (-1).
fn old_listens(node: &NodeData) -> [(u8, u8); PADS] {
    let value = |name: &str| node.parameters.iter().find(|p| p.name == name).map(|p| p.value.as_f32());
    let base = value("Base").unwrap_or(pad::DEFAULT_NOTE as f32).round().clamp(0.0, 127.0) as u8;
    let channel = value("Channel").unwrap_or(pad::DEFAULT_CHANNEL as f32).round().clamp(0.0, 16.0) as u8;
    std::array::from_fn(|index| {
        if index == 0 {
            return (base, channel);
        }
        let n = index + 1;
        let own_note = value(&format!("Note {n}")).unwrap_or(-1.0).round();
        let own_channel = value(&format!("Channel {n}")).unwrap_or(-1.0).round();
        // Past the top of MIDI's notes it heard none; 127 is as near
        let note = if own_note >= 0.0 { own_note.min(127.0) as u8 } else { (base as usize + index).min(127) as u8 };
        (note, if own_channel >= 0.0 { own_channel.min(16.0) as u8 } else { channel })
    })
}

/// Every Pads in `patch` becomes a group of eight Pads (see the module's
/// notes). A MIDI mapping to a pad's Press follows it to that Pad's, and
/// one to Mode to pad 1's; the rest, to its hidden notes, go.
pub(super) fn pads_to_groups(patch: &mut Patch) {
    fn last_id(nodes: &[NodeData], groups: &[GroupData]) -> u64 {
        let here = nodes.iter().map(|n| n.id).chain(groups.iter().map(|g| g.id)).max().unwrap_or(0);
        groups.iter().map(|g| last_id(&g.nodes, &g.groups)).fold(here, u64::max)
    }

    fn migrate(nodes: &mut Vec<NodeData>, connections: &mut [ConnectionData], groups: &mut Vec<GroupData>, next: &mut u64, moved: &mut Vec<(u64, Vec<u64>)>) {
        for group in groups.iter_mut() {
            migrate(&mut group.nodes, &mut group.connections, &mut group.groups, next, moved);
        }
        while let Some(at) = nodes.iter().position(|n| n.module_id == OLD_ID) {
            let old = nodes.remove(at);
            // Each cable from Hit, Velocity or Pressure leaves from the jack
            // of the pad whose Gate feeds the same module, or pad 1's
            let gate_into = |to: u64, connections: &[ConnectionData]| {
                connections
                    .iter()
                    .filter(|c| c.from_node == old.id && c.to_node == to)
                    .find_map(|c| c.from_port.strip_prefix("Gate ")?.parse::<usize>().ok().filter(|n| (1..=PADS).contains(n)))
                    .unwrap_or(1)
            };
            let mut jacks = old_listens(&old).map(|(note, channel)| PadJacks { note, channel, velocity: false, pressure: false });
            let renamed: Vec<(usize, String)> = connections
                .iter()
                .enumerate()
                .filter(|(_, c)| c.from_node == old.id)
                .filter_map(|(i, c)| {
                    let n = gate_into(c.to_node, connections);
                    match c.from_port.as_str() {
                        "Hit" | "Velocity" => {
                            jacks[n - 1].velocity = true;
                            Some((i, format!("Vel {n}")))
                        }
                        "Pressure" => {
                            jacks[n - 1].pressure = true;
                            Some((i, format!("Pressure {n}")))
                        }
                        _ => None,
                    }
                })
                .collect();
            for (i, port) in renamed {
                connections[i].from_port = port;
            }
            let mode = old.parameters.iter().find(|p| p.name == "Mode").map_or(0, |p| p.value.as_f32().round().clamp(0.0, 2.0) as usize);
            let group = group_of_pads(old.id, "Pads", old.position, &jacks, mode, &old.labels, next);
            moved.push((old.id, group.nodes.iter().map(|n| n.id).collect()));
            groups.push(group);
        }
    }

    let mut next = last_id(&patch.nodes, &patch.groups);
    let mut moved = Vec::new();
    migrate(&mut patch.nodes, &mut patch.connections, &mut patch.groups, &mut next, &mut moved);
    if moved.is_empty() {
        return;
    }
    patch.midi_mappings.retain_mut(|mapping| {
        let Some((_, pads)) = moved.iter().find(|(old, _)| *old == mapping.node_id) else { return true };
        if mapping.scene.is_some() {
            return true;
        }
        let (pad, param) = match mapping.param_name.as_str() {
            "Mode" => (0, pad::Pad::PARAM_MODE),
            name => match name.strip_prefix("Press ").and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=PADS).contains(n)) {
                Some(n) => (n - 1, pad::Pad::PARAM_PRESS),
                None => return false,
            },
        };
        mapping.node_id = pads[pad];
        mapping.param_index = param;
        mapping.param_name = if param == pad::Pad::PARAM_MODE { "Mode" } else { "Press" }.to_string();
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{patch_from_json, MidiMapping};

    /// An old patch: a Pads striking two drums, Hit into both Accents and
    /// Pressure into the second's Decay, pad 4 learned on its own.
    fn old_patch() -> String {
        r#"{"name": "Old pads", "version": 5,
            "nodes": [
                {"id": 1, "module_id": "input.pads", "position": [40.0, 420.0], "labels": {"Device": "MPD218"},
                 "parameters": [{"name": "Mode", "type": "Select", "value": 1},
                                {"name": "Base", "type": "Number", "value": 48.0},
                                {"name": "Channel", "type": "Select", "value": 2},
                                {"name": "Note 4", "type": "Number", "value": 70.0},
                                {"name": "Channel 4", "type": "Number", "value": 0.0}]},
                {"id": 2, "module_id": "source.drum", "position": [470.0, 40.0], "parameters": []},
                {"id": 3, "module_id": "source.drum", "position": [470.0, 440.0], "parameters": []},
                {"id": 4, "module_id": "input.midi", "position": [0.0, 0.0], "parameters": []}
            ],
            "connections": [
                {"from_node": 4, "from_port": "MIDI", "to_node": 1, "to_port": "MIDI"},
                {"from_node": 1, "from_port": "Gate 1", "to_node": 2, "to_port": "Trig"},
                {"from_node": 1, "from_port": "Hit", "to_node": 2, "to_port": "Accent"},
                {"from_node": 1, "from_port": "Gate 3", "to_node": 3, "to_port": "Trig"},
                {"from_node": 1, "from_port": "Hit", "to_node": 3, "to_port": "Accent"},
                {"from_node": 1, "from_port": "Pressure", "to_node": 3, "to_port": "Decay"}
            ],
            "midi_mappings": [
                {"cc_number": 40, "channel": 0, "node_id": 1, "param_index": 20, "param_name": "Press 3", "min_value": 0.0, "max_value": 1.0},
                {"cc_number": 41, "channel": 0, "node_id": 1, "param_index": 1, "param_name": "Base", "min_value": 0.0, "max_value": 127.0}
            ]}"#
        .to_string()
    }

    fn param(node: &NodeData, name: &str) -> f32 {
        node.parameters.iter().find(|p| p.name == name).unwrap().value.as_f32()
    }

    #[test]
    fn an_old_pads_opens_as_a_group_of_eight_pads_where_it_was() {
        let patch = patch_from_json(&old_patch()).unwrap();
        assert!(patch.all_nodes().iter().all(|n| n.module_id != OLD_ID));
        let [group] = patch.groups.as_slice() else { panic!("one group") };
        assert_eq!((group.id, group.name.as_str(), group.position), (1, "Pads", (40.0, 420.0)), "its ID, so the cables outside stay");
        assert_eq!(group.nodes.len(), 8);
        assert!(group.nodes.iter().all(|n| n.module_id == pad::ID && n.labels.get("Device").map(String::as_str) == Some("MPD218")));
        assert!(group.nodes.iter().all(|n| param(n, "Mode") == 1.0), "each in the Mode Pads was in");
        let heard: Vec<(f32, f32)> = group.nodes.iter().map(|n| (param(n, "Note"), param(n, "Channel"))).collect();
        assert_eq!(heard, [(48.0, 2.0), (49.0, 2.0), (50.0, 2.0), (70.0, 0.0), (52.0, 2.0), (53.0, 2.0), (54.0, 2.0), (55.0, 2.0)]);
        // Laid out 2×4
        let rows: std::collections::BTreeSet<_> = group.nodes.iter().map(|n| n.position.1 as i32).collect();
        assert_eq!(rows.len(), 2);

        // Gate 1-8, and the Vel and Pressure jacks the cables need, in pad order
        let jacks: Vec<&str> = group.outputs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(jacks, ["Gate 1", "Vel 1", "Gate 2", "Gate 3", "Vel 3", "Pressure 3", "Gate 4", "Gate 5", "Gate 6", "Gate 7", "Gate 8"]);
        assert_eq!(group.inputs[0].name, "MIDI");
        assert_eq!(group.connections.iter().filter(|c| c.from_node == 1 && c.from_port == "MIDI").count(), 8, "the MIDI jack into every Pad");

        // Each Accent hears its own pad, and Decay the pad it followed
        let from = |to: u64, port: &str| patch.connections.iter().find(|c| c.to_node == to && c.to_port == port).map(|c| (c.from_node, c.from_port.as_str()));
        assert_eq!(from(2, "Accent"), Some((1, "Vel 1")));
        assert_eq!(from(3, "Accent"), Some((1, "Vel 3")));
        assert_eq!(from(3, "Decay"), Some((1, "Pressure 3")));
        assert_eq!(from(3, "Trig"), Some((1, "Gate 3")));
        assert_eq!(from(1, "MIDI"), Some((4, "MIDI")), "the MIDI In plugs into the group");

        // Press 3's mapping follows pad 3; one to a hidden note goes
        let mappings: &[MidiMapping] = &patch.midi_mappings;
        assert_eq!(mappings.len(), 1);
        assert_eq!((mappings[0].node_id, mappings[0].param_name.as_str(), mappings[0].param_index), (group.nodes[2].id, "Press", pad::Pad::PARAM_PRESS));
        let ids: std::collections::BTreeSet<u64> = patch.all_nodes().iter().map(|n| n.id).chain([group.id]).collect();
        assert_eq!(ids.len(), 4 + 8, "every ID its own");
    }

    #[test]
    fn a_pads_inside_a_group_becomes_a_group_inside_it() {
        let json = r#"{"name": "Nested", "version": 6, "nodes": [], "connections": [],
            "groups": [{"id": 10, "name": "Kit", "position": [0.0, 0.0], "inputs": [], "outputs": [],
                "inputs_position": [0.0, 0.0], "outputs_position": [0.0, 0.0],
                "nodes": [{"id": 11, "module_id": "input.pads", "position": [5.0, 5.0], "parameters": []},
                          {"id": 12, "module_id": "source.drum", "position": [300.0, 0.0], "parameters": []}],
                "connections": [{"from_node": 11, "from_port": "Gate 2", "to_node": 12, "to_port": "Trig"},
                                {"from_node": 11, "from_port": "Velocity", "to_node": 12, "to_port": "Accent"}]}]}"#;
        let patch = patch_from_json(json).unwrap();
        let kit = &patch.groups[0];
        assert_eq!(kit.nodes.len(), 1);
        let [pads] = kit.groups.as_slice() else { panic!("the Pads, inside the Kit") };
        assert_eq!(pads.id, 11);
        assert!(pads.nodes.iter().all(|n| n.id > 12));
        let heard: Vec<f32> = pads.nodes.iter().map(|n| param(n, "Note")).collect();
        assert_eq!(heard, (36..44).map(|n| n as f32).collect::<Vec<_>>(), "the GM drum notes, as Pads was added");
        assert!(kit.connections.iter().any(|c| c.from_node == 11 && c.from_port == "Vel 2" && c.to_port == "Accent"));
    }
}
