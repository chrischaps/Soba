//! A kit as wide as the hardware (#484): an Instrument playing a drum
//! machine's or sampler's profile, and a Strike for each of its parts,
//! patched from it, in a group.
//!
//! [`group_of_strikes`] builds it, for the Library's Digitakt Kit, SP-404
//! Bank A and volca drum Kit. Inside, the Instrument's purple MIDI output
//! goes into every Strike, so each plays the Instrument's port and channel
//! and strikes one of its parts by name. Outside, the group has a **Gate**
//! and a **Vel** jack for each part, numbered as a Trigger Sequencer's lanes
//! are, so a sequencer's eight lanes patch straight across, and two of them
//! play a whole SP bank.

use crate::hardware::instrument::{self, PART, PROFILE};
use crate::hardware::{Part, Profile};
use crate::modules::{instrument as instrument_module, strike};

use super::patch::{ConnectionData, GroupData, JackData, NamedParameter, NodeData, ParameterValue};

/// Where the Strikes sit inside the group: this many to a row, this far
/// apart, to the right of the Instrument.
const PER_ROW: usize = 4;
const COLUMN: f32 = 230.0;
const ROW: f32 = 230.0;
const LEFT: f32 = 340.0;

/// A group named `name` holding an Instrument playing `profile` and a
/// Strike for each of `parts`, as the profile names them. Its jacks are
/// `Gate n` and `Vel n` into the nth Strike. The nodes take IDs from `next`
/// up.
pub fn group_of_strikes(id: u64, name: &str, position: (f32, f32), profile: &Profile, parts: &[Part], next: &mut u64) -> GroupData {
    let midi = strike::PORT_NAME;
    let channel = ParameterValue::Select((profile.channel() as usize).clamp(1, 16) - 1);
    let mut nodes = Vec::new();
    let mut connections = Vec::new();
    let mut inputs = Vec::new();

    *next += 1;
    let device = *next;
    let mut node = NodeData::new(device, instrument_module::ID, (0.0, 0.0));
    node.parameters = vec![NamedParameter::new("Port", ParameterValue::Select(0)), NamedParameter::new("Channel", channel.clone())];
    node.parameters.extend(
        instrument_module::SLOT_NAMES
            .iter()
            .enumerate()
            .map(|(slot, name)| NamedParameter::new(*name, ParameterValue::Number(profile.parameters.get(slot).map_or(0.5, instrument::start)))),
    );
    node.labels = instrument::choose(profile).into_iter().filter(|(_, text)| !text.is_empty()).collect();
    nodes.push(node);

    for (index, part) in parts.iter().enumerate() {
        *next += 1;
        let strike_id = *next;
        let at = (LEFT + (index % PER_ROW) as f32 * COLUMN, (index / PER_ROW) as f32 * ROW);
        let mut node = NodeData::new(strike_id, strike::ID, at);
        node.parameters = vec![
            NamedParameter::new("Port", ParameterValue::Select(0)),
            NamedParameter::new("Channel", channel.clone()),
            NamedParameter::new("Note", ParameterValue::Number(strike::DEFAULT_NOTE)),
            NamedParameter::new("Velocity", ParameterValue::Number(0.8)),
        ];
        node.labels.insert(PROFILE.to_string(), profile.name.clone());
        node.labels.insert(PART.to_string(), instrument::written_part(part));
        nodes.push(node);
        let (gate, vel) = (format!("Gate {}", index + 1), format!("Vel {}", index + 1));
        connections.push(ConnectionData::new(device, midi, strike_id, midi));
        connections.push(ConnectionData::new(id, gate.clone(), strike_id, "Gate"));
        connections.push(ConnectionData::new(id, vel.clone(), strike_id, "Vel"));
        inputs.push(JackData { name: gate, signal: "Gate".to_string() });
        inputs.push(JackData { name: vel, signal: "Control".to_string() });
    }
    GroupData {
        id,
        name: name.to_string(),
        position,
        inputs,
        outputs: Vec::new(),
        inputs_position: (-300.0, 0.0),
        outputs_position: (LEFT + PER_ROW as f32 * COLUMN + 40.0, 0.0),
        nodes,
        connections,
        groups: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::Patch;

    #[test]
    fn a_strike_for_each_part_patched_from_the_instrument() {
        let profiles = crate::hardware::Profiles::load_from(std::path::Path::new("no such folder"));
        let sp = &profiles.find("SP-404MKII").unwrap().profile;
        let mut next = 900;
        let group = group_of_strikes(900, "SP-404 Bank A", (0.0, 0.0), sp, &sp.parts, &mut next);
        assert_eq!(group.nodes.len(), 17, "the Instrument and sixteen Strikes");
        assert_eq!(group.inputs.len(), 32);
        assert_eq!(group.inputs[30].name, "Gate 16");
        let instrument = &group.nodes[0];
        assert_eq!(instrument.labels.get(PROFILE).map(String::as_str), Some("SP-404MKII"));
        for strike in &group.nodes[1..] {
            assert!(group.connections.iter().any(|c| c.from_node == instrument.id && c.to_node == strike.id && c.to_port == "MIDI"));
        }
        let mut patch = Patch::new("SP-404 Bank A");
        patch.groups.push(group);
        let compiled = crate::persistence::compile_patch(&patch).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
    }
}

