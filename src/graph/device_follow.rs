//! Device follow (#484): a Strike or CC Out patched from a device plays it.
//!
//! An Instrument, a Kit and MIDI Out each have a purple MIDI output. Patch
//! it into a Strike's or CC Out's MIDI input, or a group's that leads on to
//! them, and they play that device: each frame, before the engine hears
//! about the frame's edits, [`sync`] copies the device's Port and Channel
//! onto every sender patched from it, and its profile, so a Strike chooses
//! its part from the device's parts and a CC Out its parameter from the
//! device's knobs. An edit to the device and the copies of it are one undo
//! step, as an Arranger's followers' are (see `arranger_follow`).
//!
//! It runs this way round, the device's cable out to its senders, as a MIDI
//! In's runs out to the modules that hear it (#479): a purple cable always
//! leaves a device and says who uses it, and a device can have as many
//! senders as it has parts. The cable is a routing, not a stream: each
//! sender writes to the device's port through its own ring, stamped with
//! its sample, and the sender thread sends each when it's heard.
//!
//! Unpatched, a sender keeps what it last had, and its Port and Channel are
//! its own to change again.

use egui_node_graph2::NodeId;

use crate::hardware::instrument::PROFILE;
use crate::modules::{cc_out, instrument, kit, midi_out, strike};

use super::groups::{leaf_source, GroupIndex};
use super::SynthGraph;

/// The parameters a sender takes from its device, by name.
pub const FOLLOWED: [&str; 2] = ["Port", "Channel"];

/// Whether a module is a device a sender can be patched from: it has a
/// purple MIDI output for them.
pub fn is_device(module_id: &str) -> bool {
    [instrument::ID, kit::ID, midi_out::ID].contains(&module_id)
}

/// Whether a module sends to a device it's patched from: a Strike or a CC Out.
pub fn sends_to_device(module_id: &str) -> bool {
    module_id == strike::ID || module_id == cc_out::ID
}

/// The device the sender `node_id`'s MIDI input is patched from, following
/// the cable through any groups, or `None` if it isn't (or a group's jack
/// it comes through has nothing plugged in, or a MIDI In is).
pub fn device_of(graph: &SynthGraph, node_id: NodeId) -> Option<NodeId> {
    let node = graph.nodes.get(node_id)?;
    if !sends_to_device(node.user_data.module_id) {
        return None;
    }
    let input = node.get_input(strike::PORT_NAME).ok()?;
    let output = graph.connections.get(input)?.first().copied()?;
    let source = leaf_source(graph, &GroupIndex::of(graph), output)?;
    let from = graph.outputs.get(source)?.node;
    is_device(graph.nodes.get(from)?.user_data.module_id).then_some(from)
}

/// The senders patched from the device `from`, through any groups.
pub fn senders(graph: &SynthGraph, from: NodeId) -> Vec<NodeId> {
    graph.nodes.iter().filter(|(node_id, _)| device_of(graph, *node_id) == Some(from)).map(|(node_id, _)| node_id).collect()
}

/// Has every sender patched from a device play it: its Port and Channel,
/// and its profile (or none, from a MIDI Out). Returns whether anything
/// changed.
pub fn sync(graph: &mut SynthGraph) -> bool {
    let routes: Vec<(NodeId, NodeId)> = graph
        .nodes
        .iter()
        .filter(|(_, node)| sends_to_device(node.user_data.module_id))
        .filter_map(|(node_id, _)| Some((node_id, device_of(graph, node_id)?)))
        .collect();
    let mut changed = false;
    for (sender, device) in routes {
        for name in FOLLOWED {
            let Some(value) = super::display_frame::value_of(graph, device, name) else { continue };
            let Ok(input) = graph[sender].get_input(name) else { continue };
            let held = &mut graph.inputs[input].value;
            if held.actual_value() != value {
                held.set_actual_value(value);
                changed = true;
            }
        }
        let profile = graph[device].user_data.labels.get(PROFILE).cloned();
        let labels = &mut graph[sender].user_data.labels;
        if labels.get(PROFILE) != profile.as_ref() {
            match profile {
                Some(profile) => labels.insert(PROFILE.to_string(), profile),
                None => labels.remove(PROFILE),
            };
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};
    use egui_node_graph2::NodeTemplateTrait;

    use crate::graph::groups::{group, GroupId};
    use crate::graph::{create_editor_state, SynthGraphEditorState, SynthGraphState, SynthNodeTemplate};
    use crate::modules::midi_in;

    fn add(editor: &mut SynthGraphEditorState, module_id: &str) -> NodeId {
        let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
        let mut user_state = SynthGraphState::new();
        let node_id = editor.graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        );
        editor.node_positions.insert(node_id, pos2(0.0, 0.0));
        editor.node_order.push(node_id);
        node_id
    }

    fn patch(editor: &mut SynthGraphEditorState, from: NodeId, to: NodeId) {
        let graph = &mut editor.graph;
        let (output, input) = (graph[from].get_output(strike::PORT_NAME).unwrap(), graph[to].get_input(strike::PORT_NAME).unwrap());
        graph.add_connection(output, input, 0);
    }

    fn set(editor: &mut SynthGraphEditorState, node_id: NodeId, name: &str, value: f32) {
        let input = editor.graph[node_id].get_input(name).unwrap();
        editor.graph.inputs[input].value.set_actual_value(value);
    }

    fn value(editor: &SynthGraphEditorState, node_id: NodeId, name: &str) -> f32 {
        super::super::display_frame::value_of(&editor.graph, node_id, name).unwrap()
    }

    #[test]
    fn a_strike_patched_from_an_instrument_plays_its_port_channel_and_profile() {
        let mut editor = create_editor_state();
        let sp = add(&mut editor, instrument::ID);
        let strike = add(&mut editor, strike::ID);
        let knob = add(&mut editor, cc_out::ID);
        editor.graph[sp].user_data.labels.insert(PROFILE.into(), "SP-404MKII".into());
        set(&mut editor, sp, "Port", 3.0);
        set(&mut editor, sp, "Channel", 9.0);
        assert!(!sync(&mut editor.graph), "unpatched, nothing to follow");
        assert_eq!(device_of(&editor.graph, strike), None);

        patch(&mut editor, sp, strike);
        patch(&mut editor, sp, knob);
        assert_eq!(device_of(&editor.graph, strike), Some(sp));
        assert!(sync(&mut editor.graph));
        for sender in [strike, knob] {
            assert_eq!((value(&editor, sender, "Port"), value(&editor, sender, "Channel")), (3.0, 9.0));
            assert_eq!(editor.graph[sender].user_data.labels.get(PROFILE).map(String::as_str), Some("SP-404MKII"));
        }
        let mut found = senders(&editor.graph, sp);
        found.sort();
        let mut expected = vec![strike, knob];
        expected.sort();
        assert_eq!(found, expected);
        assert!(!sync(&mut editor.graph), "settled");

        // A MIDI Out has no profile: the senders lose theirs
        let out = add(&mut editor, midi_out::ID);
        let input = editor.graph[strike].get_input(strike::PORT_NAME).unwrap();
        editor.graph.remove_connection(input, editor.graph[sp].get_output(strike::PORT_NAME).unwrap());
        patch(&mut editor, out, strike);
        assert!(sync(&mut editor.graph));
        assert_eq!((value(&editor, strike, "Port"), value(&editor, strike, "Channel")), (0.0, 0.0));
        assert_eq!(editor.graph[strike].user_data.labels.get(PROFILE), None);
    }

    #[test]
    fn a_midi_in_isnt_a_device_to_play() {
        let mut editor = create_editor_state();
        let keys = add(&mut editor, midi_in::ID);
        let strike = add(&mut editor, strike::ID);
        patch(&mut editor, keys, strike);
        assert_eq!(device_of(&editor.graph, strike), None);
        assert!(!sync(&mut editor.graph));
    }

    #[test]
    fn the_device_follows_the_cable_into_a_group_of_strikes() {
        let mut editor = create_editor_state();
        let kit = add(&mut editor, kit::ID);
        let a = add(&mut editor, strike::ID);
        let b = add(&mut editor, strike::ID);
        patch(&mut editor, kit, a);
        patch(&mut editor, kit, b);
        set(&mut editor, kit, "Channel", 4.0);
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 300.0));
        group(&mut editor, &[a, b], GroupId(7), "Strikes", bounds).expect("grouped");
        assert_eq!(device_of(&editor.graph, a), Some(kit), "through the group's one MIDI jack");
        assert_eq!(device_of(&editor.graph, b), Some(kit));
        sync(&mut editor.graph);
        assert_eq!(value(&editor, b, "Channel"), 4.0);
    }
}
