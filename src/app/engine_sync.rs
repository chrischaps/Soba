//! Engine commands for edits to the editor graph.
//!
//! The editor graph and the audio engine each keep their own copy of the
//! patch. Every edit to the graph, whether the user made it or undo replayed
//! it, has to be mirrored to the engine. [`add_module`] builds the commands
//! for a new module, and [`sync_cables`] brings the engine's cables in line
//! with the graph's, whatever changed them.
//!
//! The engine only knows modules. A cable that runs through a group's jacks
//! reaches it as one cable from module to module (see
//! [`crate::graph::groups`]), so cables are synced by comparing those with
//! the ones the engine was last sent, rather than edit by edit.

use std::collections::{BTreeMap, HashMap};

use egui_node_graph2::{InputId, NodeId};

use crate::engine::{EngineCommand, NodeId as EngineNodeId};
use crate::graph::{groups, knob_cv, port_mapping, SynthGraph, SynthGraphState};

/// Commands that create a node's module and start monitoring the outputs it
/// shows (LEDs, lit ports, displays). Parameter values follow with the next
/// parameter sync.
pub fn add_module(graph: &SynthGraph, node_id: NodeId, engine_node_id: EngineNodeId) -> Vec<EngineCommand> {
    let Some(node) = graph.nodes.get(node_id) else {
        return Vec::new();
    };
    let mut commands = vec![EngineCommand::AddModule {
        node_id: engine_node_id,
        module_id: node.user_data.module_id,
    }];
    if node.user_data.bypassed {
        commands.push(EngineCommand::SetBypass { node_id: engine_node_id, bypassed: true });
    }
    let monitored = node.user_data.led_indicators.iter()
        .map(|led| led.output_index)
        .chain(node.user_data.monitored_outputs.iter().copied());
    commands.extend(monitored.map(|output_index| EngineCommand::MonitorOutput {
        node_id: engine_node_id,
        output_index,
    }));
    commands
}

/// A module input, as the engine knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeafInput {
    pub node: EngineNodeId,
    /// The engine's input port index.
    pub port: usize,
}

/// What feeds a module input, as the engine knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeafCable {
    pub from_node: EngineNodeId,
    /// The engine's port index for the output (inputs count first).
    pub from_port: usize,
    /// The output's position among the node's outputs, which monitoring uses.
    pub output_index: usize,
    /// The input is an exposed parameter: its knob follows the cable.
    pub exposed: bool,
}

/// Every module input that has a cable, and where the cable comes from.
pub type LeafCables = BTreeMap<LeafInput, LeafCable>;

/// The graph's cables as the engine should have them: module to module,
/// through any groups between.
pub fn leaf_cables(graph: &SynthGraph, user_state: &SynthGraphState) -> LeafCables {
    groups::leaf_cables(graph)
        .into_iter()
        .filter_map(|(output, input)| {
            let (from, to) = (graph.get_output(output).node, graph.get_input(input).node);
            let key = LeafInput {
                node: user_state.get_engine_node_id(to)?,
                port: port_mapping::input_port_index(graph, to, input)?,
            };
            let cable = LeafCable {
                from_node: user_state.get_engine_node_id(from)?,
                from_port: port_mapping::output_port_index(graph, from, output)?,
                output_index: graph.get_output_index(output)?,
                exposed: is_exposed(graph, input),
            };
            Some((key, cable))
        })
        .collect()
}

/// Whether a knob follows an input: an exposed parameter (both a jack and a
/// knob), or a jack another knob takes its CV through (a VCA's CV moves Level).
fn is_exposed(graph: &SynthGraph, input: InputId) -> bool {
    let Some(node) = graph.inputs.get(input).and_then(|i| graph.nodes.get(i.node)) else {
        return false;
    };
    let Some((name, _)) = node.inputs.iter().find(|(_, id)| *id == input) else {
        return false;
    };
    let data = &node.user_data;
    data.knob_params.iter().any(|kp| {
        (kp.param_name == *name && kp.has_input_port())
            || knob_cv::jack(data.module_id, &kp.param_name) == Some(name.as_str())
    })
}

/// Brings the engine's cables in line with the graph's, and returns the
/// commands that do it: what [`SynthGraphState::engine_cables`] says the
/// engine has, against what the graph now adds up to.
///
/// Inputs of modules that are gone are left alone (removing the module took
/// its cables). An input whose cable comes or goes starts or stops its knob
/// following it if it's exposed, and an output starts being monitored, for
/// the cable's animation, with its first cable, and stops with its last,
/// unless the module always shows it.
pub fn sync_cables(graph: &SynthGraph, user_state: &mut SynthGraphState) -> Vec<EngineCommand> {
    let now = leaf_cables(graph, user_state);
    let before = std::mem::replace(&mut user_state.engine_cables, now);
    let now = &user_state.engine_cables;
    if before == *now {
        return Vec::new();
    }
    let live: HashMap<EngineNodeId, NodeId> =
        user_state.node_id_map.iter().map(|(&graph_id, &engine_id)| (engine_id, graph_id)).collect();

    let mut commands = Vec::new();
    let inputs: std::collections::BTreeSet<LeafInput> = before.keys().chain(now.keys()).copied().collect();
    for input in inputs {
        let (was, is) = (before.get(&input), now.get(&input));
        let source = |c: Option<&LeafCable>| c.map(|c| (c.from_node, c.from_port));
        if source(was) == source(is) || !live.contains_key(&input.node) {
            continue;
        }
        if was.is_some() {
            commands.push(EngineCommand::Disconnect { node_id: input.node, port: input.port, is_input: true });
        }
        if let Some(cable) = is {
            commands.push(EngineCommand::Connect {
                from_node: cable.from_node,
                from_port: cable.from_port,
                to_node: input.node,
                to_port: input.port,
            });
        }
        let (node_id, input_index) = (input.node, input.port);
        match (was, is) {
            (None, Some(cable)) if cable.exposed => commands.push(EngineCommand::MonitorInput { node_id, input_index }),
            (Some(cable), None) if cable.exposed => commands.push(EngineCommand::UnmonitorInput { node_id, input_index }),
            _ => {}
        }
    }

    // Outputs whose first cable came or last cable went
    let fed = |cables: &LeafCables| {
        let mut counts: BTreeMap<(EngineNodeId, usize), usize> = BTreeMap::new();
        for cable in cables.values() {
            *counts.entry((cable.from_node, cable.output_index)).or_default() += 1;
        }
        counts
    };
    let (was_fed, is_fed) = (fed(&before), fed(now));
    let outputs: std::collections::BTreeSet<(EngineNodeId, usize)> = was_fed.keys().chain(is_fed.keys()).copied().collect();
    for (node_id, output_index) in outputs {
        let Some(&graph_id) = live.get(&node_id) else { continue };
        match (was_fed.contains_key(&(node_id, output_index)), is_fed.contains_key(&(node_id, output_index))) {
            (false, true) => commands.push(EngineCommand::MonitorOutput { node_id, output_index }),
            (true, false) => {
                let always = graph.nodes.get(graph_id).is_some_and(|node| {
                    node.user_data.monitored_outputs.contains(&output_index)
                        || node.user_data.led_indicators.iter().any(|led| led.output_index == output_index)
                });
                if !always {
                    commands.push(EngineCommand::UnmonitorOutput { node_id, output_index });
                }
            }
            _ => {}
        }
    }
    commands
}
