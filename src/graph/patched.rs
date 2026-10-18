//! Whether a cable is plugged into a jack, without a walk over every cable
//! in the patch (#364): a face asks this of each knob and port, each frame.

use egui_node_graph2::{InputId, OutputId};
use std::collections::HashMap;

use super::SynthGraph;

/// Whether a cable runs into `input`.
pub fn input(graph: &SynthGraph, input: InputId) -> bool {
    graph.connections.get(input).is_some_and(|cables| !cables.is_empty())
}

/// Where each output's cables go, by output. The graph keeps its cables by
/// input, so this is built once a frame, before the faces are drawn.
#[derive(Default)]
pub struct CablesFrom(HashMap<OutputId, InputId>);

impl CablesFrom {
    pub fn rebuild(&mut self, graph: &SynthGraph) {
        self.0.clear();
        for (input, outputs) in graph.connections.iter() {
            for &output in outputs {
                self.0.entry(output).or_insert(input);
            }
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Whether a cable runs out of `output`.
    pub fn patched(&self, output: OutputId) -> bool {
        self.0.contains_key(&output)
    }

    /// The input `output`'s first cable runs into, in the graph's order.
    pub fn first(&self, output: OutputId) -> Option<InputId> {
        self.0.get(&output).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{stage_patch, ConnectionData, NodeData, Patch};

    #[test]
    fn inputs_and_outputs_know_their_cables() {
        let mut patch = Patch::new("patched");
        patch.nodes.push(NodeData::new(1, "osc.sine", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, "filter.svf", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 2, "In"));
        let staged = stage_patch(&patch).unwrap();
        let [osc, filter] = [1, 2].map(|id| staged.graph_id(id).unwrap());
        let graph = staged.graph;
        let out = graph[osc].get_output("Out").unwrap();
        let into = graph[filter].get_input("In").unwrap();
        let cutoff = graph[filter].get_input("Cutoff").unwrap();
        let mut from = CablesFrom::default();
        from.rebuild(&graph);
        assert!(input(&graph, into));
        assert!(!input(&graph, cutoff));
        assert!(from.patched(out));
        assert_eq!(from.first(out), Some(into));
    }
}
