//! Putting back the cable a refused one replaced.
//!
//! Every input takes one cable, and a cable dropped on a full input replaces
//! the one there as the editor makes it, before Soba has looked at it. If
//! Soba then refuses it (the wrong signal, a loop, an Arranger following
//! something it can't), taking it out would leave the input empty, the old
//! cable lost with no sign of why (#309). So while a cable is being dragged
//! the inputs' cables are kept from the start of each frame, and a refused
//! one gives its input back what it held.

use std::collections::HashMap;

use egui_node_graph2::{InputId, OutputId};

use super::SynthGraph;

/// Every input's cables as a frame began.
pub struct Before(HashMap<InputId, Vec<OutputId>>);

/// The cables to put back from, if a cable might be dropped this frame:
/// only while one is being dragged.
pub fn before_drop(graph: &SynthGraph, dragging: bool) -> Option<Before> {
    dragging.then(|| Before(graph.iter_connection_groups().collect()))
}

/// Takes out the refused cable from `output` into `input`, and puts back
/// whatever it replaced.
pub fn refuse(graph: &mut SynthGraph, output: OutputId, input: InputId, before: Option<&Before>) {
    graph.remove_connection(input, output);
    let Some(held) = before.and_then(|before| before.0.get(&input)) else { return };
    // Only cables whose module is still there (it can't have gone mid-drag, but a cable to nothing would panic)
    let held: Vec<OutputId> = held.iter().copied().filter(|&o| graph.outputs.contains_key(o)).collect();
    if !held.is_empty() {
        graph.connections.insert(input, held);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{stage_patch, ConnectionData, NodeData, Patch};

    /// Two oscillators and a filter, the first patched into its input.
    fn patched() -> (SynthGraph, OutputId, OutputId, InputId) {
        let mut patch = Patch::new("refused");
        patch.nodes.push(NodeData::new(1, "osc.sine", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, "osc.sine", (0.0, 200.0)));
        patch.nodes.push(NodeData::new(3, "filter.svf", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 3, "In"));
        let staged = stage_patch(&patch).unwrap();
        assert!(staged.warnings.is_empty(), "{:?}", staged.warnings);
        let [a, b, filter] = [1, 2, 3].map(|id| staged.graph_id(id).unwrap());
        let graph = staged.graph;
        let (a, b) = (graph[a].get_output("Out").unwrap(), graph[b].get_output("Out").unwrap());
        let input = graph[filter].get_input("In").unwrap();
        (graph, a, b, input)
    }

    #[test]
    fn a_refused_cable_puts_back_the_one_it_replaced() {
        let (mut graph, a, b, input) = patched();
        let before = before_drop(&graph, true);
        // The editor makes the cable as it's dropped, replacing the one there
        graph.add_connection(b, input, 0);
        assert_eq!(graph.connections(input), vec![b]);
        refuse(&mut graph, b, input, before.as_ref());
        assert_eq!(graph.connections(input), vec![a]);
    }

    #[test]
    fn a_refused_cable_into_an_empty_input_leaves_it_empty() {
        let (mut graph, a, b, input) = patched();
        graph.remove_connection(input, a);
        let before = before_drop(&graph, true);
        graph.add_connection(b, input, 0);
        refuse(&mut graph, b, input, before.as_ref());
        assert!(graph.connections(input).is_empty());
    }

    #[test]
    fn nothing_is_kept_unless_a_cable_is_being_dragged() {
        let (graph, ..) = patched();
        assert!(before_drop(&graph, false).is_none());
    }
}
