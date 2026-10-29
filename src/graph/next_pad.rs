//! A new Pad takes the next note (#481). Added, pasted or duplicated beside
//! other Pads on the same level, a Pad hears the note after the highest one
//! there, on that Pad's channel, so a group of Pads built by hand mostly
//! needs no learn at all. Several at once take the next few, in the order of
//! the notes they came with: duplicate a row on 36-39 and the copies are on
//! 40-43.

use egui_node_graph2::NodeId;

use crate::modules::{pad, Pad};

use super::pads_face::{self, parameters};
use super::SynthGraph;

/// Moves each Pad among `new` onto the notes after the highest Pad already
/// on its level, if there is one, and onto that Pad's channel.
pub fn follow_on(graph: &mut SynthGraph, new: &[NodeId]) {
    let is_pad = |graph: &SynthGraph, n: NodeId| graph.nodes.get(n).is_some_and(|node| node.user_data.module_id == pad::ID);
    let new_pads: Vec<NodeId> = new.iter().copied().filter(|&n| is_pad(graph, n)).collect();
    let mut levels: Vec<_> = new_pads.iter().map(|&n| graph[n].user_data.parent).collect();
    levels.dedup();
    for level in levels {
        let highest = graph
            .nodes
            .iter()
            .filter(|(n, node)| node.user_data.module_id == pad::ID && node.user_data.parent == level && !new.contains(n))
            .map(|(n, _)| pads_face::listen_of(graph, n))
            .max_by_key(|listen| listen.note);
        let Some(highest) = highest else { continue };
        let mut here: Vec<NodeId> = new_pads.iter().copied().filter(|&n| graph[n].user_data.parent == level).collect();
        here.sort_by_key(|&n| pads_face::listen_of(graph, n).note);
        for (n, node_id) in here.into_iter().enumerate() {
            let note = highest.note as usize + 1 + n;
            if note > 127 {
                break;
            }
            set(graph, node_id, Pad::PARAM_NOTE, note as f32);
            set(graph, node_id, Pad::PARAM_CHANNEL, highest.channel as f32);
        }
    }
}

/// Sets one of a Pad node's parameters, by its index.
fn set(graph: &mut SynthGraph, node_id: NodeId, param: usize, value: f32) {
    let name = parameters()[param].0;
    let Some(&(_, input)) = graph[node_id].inputs.iter().find(|(input, _)| input == name) else { return };
    graph.inputs[input].value.set_actual_value(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_node_graph2::NodeTemplateTrait;
    use crate::graph::{create_editor_state, GroupId, SynthGraphState, SynthNodeTemplate};

    fn add_pad(graph: &mut SynthGraph, parent: Option<GroupId>, note: u8, channel: u8) -> NodeId {
        let template = SynthNodeTemplate::from_module_id(pad::ID).unwrap();
        let mut user_state = SynthGraphState::new();
        let node_id = graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        );
        graph[node_id].user_data.parent = parent;
        set(graph, node_id, Pad::PARAM_NOTE, note as f32);
        set(graph, node_id, Pad::PARAM_CHANNEL, channel as f32);
        node_id
    }

    fn listen(graph: &SynthGraph, node_id: NodeId) -> (u8, u8) {
        let listen = pads_face::listen_of(graph, node_id);
        (listen.note, listen.channel)
    }

    #[test]
    fn a_duplicate_of_the_pad_on_40_is_on_41() {
        let mut graph = create_editor_state().graph;
        let group = Some(GroupId(3));
        add_pad(&mut graph, group, 36, 10);
        add_pad(&mut graph, group, 40, 10);
        let copy = add_pad(&mut graph, group, 40, 10);
        follow_on(&mut graph, &[copy]);
        assert_eq!(listen(&graph, copy), (41, 10));
    }

    #[test]
    fn several_take_the_next_few_in_their_notes_order_on_the_highest_ones_channel() {
        let mut graph = create_editor_state().graph;
        add_pad(&mut graph, None, 39, 2);
        let fresh = [add_pad(&mut graph, None, 37, 0), add_pad(&mut graph, None, 36, 0)];
        follow_on(&mut graph, &fresh);
        assert_eq!(listen(&graph, fresh[1]), (40, 2));
        assert_eq!(listen(&graph, fresh[0]), (41, 2));
    }

    #[test]
    fn a_pad_alone_on_its_level_or_past_the_top_keeps_its_note() {
        let mut graph = create_editor_state().graph;
        // Only Pads on another level
        add_pad(&mut graph, Some(GroupId(1)), 50, 1);
        let alone = add_pad(&mut graph, None, 36, 0);
        follow_on(&mut graph, &[alone]);
        assert_eq!(listen(&graph, alone), (36, 0));

        add_pad(&mut graph, Some(GroupId(2)), 127, 1);
        let over = add_pad(&mut graph, Some(GroupId(2)), 60, 1);
        follow_on(&mut graph, &[over]);
        assert_eq!(listen(&graph, over), (60, 1));
    }
}
