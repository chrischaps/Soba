//! Cables that would close a loop.
//!
//! Every module needs its inputs before it runs, so the engine turns away a
//! cable that would close a loop, unless it ends at a late input (see
//! [`PortDefinition::late`](crate::dsp::PortDefinition::late)). These find
//! such cables in the editor's graph first, so it never shows a cable that
//! isn't heard: a patch's are skipped as it loads, with a warning, and one
//! just patched is taken out again.

use std::collections::{HashMap, HashSet, VecDeque};

use egui_node_graph2::{InputId, NodeId, OutputId};

use super::groups::leaf_cables;
use super::{catalog, port_mapping, SynthGraph};

/// A cable the engine would turn away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closing {
    /// Its place in the cables it was found among.
    pub index: usize,
    /// The modules round the loop it would close, from the one it feeds to
    /// the one it comes from.
    pub route: Vec<NodeId>,
}

/// The cables among `cables` (module to module, see [`leaf_cables`]) that
/// the engine would turn away, taking them in order as it does: a cable is
/// kept unless the ones kept before it already lead from the module it feeds
/// back to the module it comes from. A cable into a late input is always kept.
pub fn closing(graph: &SynthGraph, cables: &[(OutputId, InputId)]) -> Vec<Closing> {
    let mut feeds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut found = Vec::new();
    for (index, &(output, input)) in cables.iter().enumerate() {
        if is_late(graph, input) {
            continue;
        }
        let (from, to) = (graph.get_output(output).node, graph.get_input(input).node);
        match route(&feeds, to, from) {
            Some(route) => found.push(Closing { index, route }),
            None => feeds.entry(from).or_default().push(to),
        }
    }
    found
}

/// Why the cable just patched from `output` to `input` can't stay, if it
/// would close a loop, naming the modules round it.
pub fn refusal(graph: &SynthGraph, output: OutputId, input: InputId) -> Option<String> {
    // The module-to-module cables it makes, through any groups, go last
    let mut without = graph.clone();
    without.remove_connection(input, output);
    let before: HashSet<(OutputId, InputId)> = leaf_cables(&without).into_iter().collect();
    let (mut cables, new): (Vec<_>, Vec<_>) = leaf_cables(graph).into_iter().partition(|cable| before.contains(cable));
    let old = cables.len();
    cables.extend(new);
    let first = closing(graph, &cables).into_iter().find(|found| found.index >= old)?;
    Some(format!("A cable can't close a loop: {}", describe(graph, &first.route)))
}

/// The loop round `route`, by module name: "LFO → Logic → Quantizer → LFO".
pub fn describe(graph: &SynthGraph, route: &[NodeId]) -> String {
    route
        .iter()
        .chain(route.first())
        .map(|&node| graph.nodes.get(node).map_or("?", |n| n.label.as_str()))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Whether a module's input may close a loop, hearing its cable a block late.
fn is_late(graph: &SynthGraph, input: InputId) -> bool {
    let node = graph.get_input(input).node;
    let Some(port) = port_mapping::input_port_index(graph, node, input) else {
        return false;
    };
    catalog::module(graph[node].user_data.module_id)
        .and_then(|spec| spec.ports.get(port))
        .is_some_and(|port| port.late)
}

/// The shortest way from `start` to `end` along `feeds`, both ends included.
fn route(feeds: &HashMap<NodeId, Vec<NodeId>>, start: NodeId, end: NodeId) -> Option<Vec<NodeId>> {
    let mut came_from: HashMap<NodeId, NodeId> = HashMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        if node == end {
            let mut route = vec![end];
            while let Some(&previous) = came_from.get(route.last()?) {
                route.push(previous);
            }
            route.reverse();
            return Some(route);
        }
        for &next in feeds.get(&node).into_iter().flatten() {
            if next != start && !came_from.contains_key(&next) {
                came_from.insert(next, node);
                queue.push_back(next);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{stage_patch, ConnectionData, NodeData, Patch};

    /// An oscillator through a filter into a mixer, and the filter's LowPass
    /// and the mixer's Send L, to patch back.
    fn chain() -> (SynthGraph, [NodeId; 3]) {
        let mut patch = Patch::new("chain");
        patch.nodes.push(NodeData::new(1, "osc.sine", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, "filter.svf", (200.0, 0.0)));
        patch.nodes.push(NodeData::new(3, "util.mixer", (400.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 2, "In"));
        patch.connections.push(ConnectionData::new(2, "LowPass", 3, "Ch 1"));
        let staged = stage_patch(&patch).unwrap();
        assert!(staged.warnings.is_empty(), "{:?}", staged.warnings);
        let ids = [1, 2, 3].map(|id| staged.graph_id(id).unwrap());
        (staged.graph, ids)
    }

    fn patch(graph: &mut SynthGraph, from: NodeId, output: &str, to: NodeId, input: &str) -> Option<String> {
        let (output, input) = (graph[from].get_output(output).unwrap(), graph[to].get_input(input).unwrap());
        graph.add_connection(output, input, 0);
        let refused = refusal(graph, output, input);
        if refused.is_some() {
            graph.remove_connection(input, output);
        }
        refused
    }

    #[test]
    fn a_cable_closing_a_loop_is_refused_naming_it() {
        let (mut graph, [osc, filter, mixer]) = chain();
        let refused = patch(&mut graph, mixer, "Send L", osc, "FM");
        assert_eq!(refused.as_deref(), Some("A cable can't close a loop: Oscillator → SVF Filter → Mixer → Oscillator"));
        // Into itself, too
        let refused = patch(&mut graph, filter, "HighPass", filter, "Cutoff");
        assert_eq!(refused.as_deref(), Some("A cable can't close a loop: SVF Filter → SVF Filter"));
    }

    #[test]
    fn a_late_input_or_a_cable_closing_nothing_is_let_be() {
        let (mut graph, [osc, filter, mixer]) = chain();
        assert_eq!(patch(&mut graph, mixer, "Send L", mixer, "Return L"), None);
        assert_eq!(patch(&mut graph, osc, "Sub", mixer, "Ch 2"), None);
        assert_eq!(patch(&mut graph, osc, "Out L", filter, "Cutoff"), None);
    }
}
