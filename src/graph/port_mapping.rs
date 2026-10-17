//! Mapping between editor graph ports and audio engine indices.
//!
//! The editor graph and the `DspModule` definitions describe the same ports in
//! different ways. These helpers translate one into the other, and are shared by
//! the live editor and headless patch compilation so both always agree.
//!
//! - **Input ports**: inputs that accept connections (`ConnectionOnly` or
//!   `ConnectionOrConstant`), counted in editor order, map to `DspModule` input
//!   port indices.
//! - **Output ports**: follow all input ports, so an output's port index is
//!   `num_input_ports + output_position`.
//! - **Parameters**: inputs that carry a value (`ConstantOnly` or
//!   `ConnectionOrConstant`), counted in editor order, map to parameter indices.

use egui_node_graph2::{Graph, InputId, InputParamKind, NodeId, OutputId};

use super::{SynthDataType, SynthNodeData, SynthValueType};

/// The concrete editor graph type used by the synth.
pub type SynthGraph = Graph<SynthNodeData, SynthDataType, SynthValueType>;

/// Returns true if an input of this kind maps to a `DspModule` input port.
#[inline]
pub fn is_connectable(kind: InputParamKind) -> bool {
    matches!(
        kind,
        InputParamKind::ConnectionOnly | InputParamKind::ConnectionOrConstant
    )
}

/// Returns true if an input of this kind maps to a `DspModule` parameter.
#[inline]
pub fn is_parameter(kind: InputParamKind) -> bool {
    matches!(
        kind,
        InputParamKind::ConstantOnly | InputParamKind::ConnectionOrConstant
    )
}

/// Gets the `DspModule` port index for an editor input, or `None` if the input
/// is parameter-only (`ConstantOnly`) or not on this node.
pub fn input_port_index(graph: &SynthGraph, node_id: NodeId, input_id: InputId) -> Option<usize> {
    let node = graph.nodes.get(node_id)?;

    let mut port_index = 0;
    for (_, id) in &node.inputs {
        let connectable = is_connectable(graph.get_input(*id).kind);
        if *id == input_id {
            return connectable.then_some(port_index);
        }
        if connectable {
            port_index += 1;
        }
    }
    None
}

/// Gets the `DspModule` parameter index for an editor input, or `None` if the
/// input is connection-only (`ConnectionOnly`) or not on this node.
pub fn parameter_index(graph: &SynthGraph, node_id: NodeId, input_id: InputId) -> Option<usize> {
    let node = graph.nodes.get(node_id)?;
    node.inputs
        .iter()
        .filter(|(_, id)| is_parameter(graph.get_input(*id).kind))
        .position(|(_, id)| *id == input_id)
}

/// Gets the `DspModule` port index for an editor output.
pub fn output_port_index(graph: &SynthGraph, node_id: NodeId, output_id: OutputId) -> Option<usize> {
    let node = graph.nodes.get(node_id)?;

    let num_input_ports = node
        .inputs
        .iter()
        .filter(|(_, id)| is_connectable(graph.get_input(*id).kind))
        .count();

    let output_position = node.outputs.iter().position(|(_, id)| *id == output_id)?;

    Some(num_input_ports + output_position)
}

/// Lists a node's parameter inputs in parameter-index order.
pub fn parameter_inputs(graph: &SynthGraph, node_id: NodeId) -> Vec<InputId> {
    graph
        .nodes
        .get(node_id)
        .map(|node| {
            node.inputs
                .iter()
                .map(|(_, id)| *id)
                .filter(|id| is_parameter(graph.get_input(*id).kind))
                .collect()
        })
        .unwrap_or_default()
}

/// Number of leading parameters the editor drives from live input rather than
/// from the graph (Keyboard: Note, Gate; MIDI Note: Note, Gate, Velocity,
/// Aftertouch). Parameter sync skips these.
pub fn live_input_parameter_count(module_id: &str) -> usize {
    super::SynthNodeTemplate::from_module_id(module_id)
        .map(|template| template.live_parameter_count())
        .unwrap_or(0)
}
