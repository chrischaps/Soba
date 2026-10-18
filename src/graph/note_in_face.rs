//! Note In's face: the controls that only matter with one voice, or only
//! with several, fade while they don't.

use std::collections::HashSet;

use egui_node_graph2::NodeId;

use crate::modules::note_in::{self, NoteIn, Source};

use super::{port_mapping, SynthGraph, SynthGraphState};

/// The controls that only one voice uses: which held key sounds, and
/// whether moving between them restarts the gate.
const MONO_ONLY: [&str; 2] = ["Priority", "Retrigger"];

/// The control only several voices use: which voice a new note takes.
const POLY_ONLY: [&str; 1] = ["Allocation"];

/// The Note In modules set to more than one voice. Kept on the state each
/// frame (see [`SynthGraphState::poly_note_ins`]) for [`fades`] to ask.
pub fn poly_note_ins(graph: &SynthGraph) -> HashSet<NodeId> {
    graph
        .nodes
        .iter()
        .filter(|(_, node)| node.user_data.module_id == note_in::ID)
        .filter(|(node_id, _)| {
            let inputs = port_mapping::parameter_inputs(graph, *node_id);
            inputs.get(NoteIn::PARAM_VOICES).is_some_and(|&input| graph.get_input(input).value.actual_value() > 1.5)
        })
        .map(|(node_id, _)| node_id)
        .collect()
}

/// Whether a Note In in `graph` plays a MIDI keyboard's notes itself (its
/// Source isn't Keys), so a sequencer listening to one needn't play them too.
pub fn plays_midi(graph: &SynthGraph) -> bool {
    graph.nodes.iter().filter(|(_, node)| node.user_data.module_id == note_in::ID).any(|(node_id, _)| {
        let inputs = port_mapping::parameter_inputs(graph, node_id);
        let source = inputs.get(NoteIn::PARAM_SOURCE).map_or(0.0, |&input| graph.get_input(input).value.actual_value());
        Source::from_param(source) != Source::Keys
    })
}

/// Whether a control on a node's face does nothing as the node is set, so
/// it's drawn faded: Priority and Retrigger with several voices, Allocation
/// with one. It can still be set, for when the voices change.
pub fn fades(module_id: &str, param: &str, node: NodeId, user_state: &SynthGraphState) -> bool {
    if module_id != note_in::ID {
        return false;
    }
    let poly = user_state.poly_note_ins.contains(&node);
    (poly && MONO_ONLY.contains(&param)) || (!poly && POLY_ONLY.contains(&param))
}
