//! Slice markers in the editor: where a Sampler sliced by Onsets is cut.
//!
//! The markers are found on the file (see [`crate::dsp::onsets`]), when the
//! node first wants them, and kept on the shelf with it; Sensitivity only
//! chooses among them. Moved by hand, they're kept on the node instead, at
//! the Sensitivity they were moved at: turning it finds them afresh, which
//! undo takes back like any edit. A new file or a new take starts afresh
//! too, and a trim moves them with the sound.
//!
//! Once a frame, [`SynthApp::sync_markers`] gives each Sampler sliced by
//! Onsets the markers its node shows, wherever they aren't what it has.

use egui_node_graph2::NodeId;

use crate::dsp::onsets::{HandMarkers, DEFAULT_SENSITIVITY};
use crate::dsp::primitives::{Markers, Slices};
use crate::engine::EngineCommand;
use crate::graph::SynthGraph;
use crate::modules::sampler::SAMPLER_ID;
use super::SynthApp;

/// A parameter's value on a node, by name.
fn value_of(graph: &SynthGraph, node_id: NodeId, name: &str) -> Option<f32> {
    let node = graph.nodes.get(node_id)?;
    let (_, input) = node.inputs.iter().find(|(input, _)| input == name)?;
    Some(graph.get_input(*input).value.actual_value())
}

/// Start and End, in order, as fractions of the recording.
fn region(graph: &SynthGraph, node_id: NodeId) -> (f32, f32) {
    let start = value_of(graph, node_id, "Start").unwrap_or(0.0).clamp(0.0, 1.0);
    let end = value_of(graph, node_id, "End").unwrap_or(1.0).clamp(0.0, 1.0);
    (start.min(end), start.max(end))
}

impl SynthApp {
    /// Keeps markers placed by hand on a Sampler's node, as an undo step
    /// named `label`.
    pub(super) fn set_slice_markers(&mut self, node_id: NodeId, label: String, markers: HandMarkers) {
        if let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) {
            node.user_data.slice_markers = Some(markers);
            self.history.name_next(label);
        }
    }

    /// Lets go of a Sampler's markers placed by hand: its file is another.
    pub(super) fn forget_slice_markers(&mut self, node_id: NodeId) {
        if let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) {
            node.user_data.slice_markers = None;
        }
    }

    /// Moves a Sampler's markers placed by hand with the sound, when a trim
    /// keeps `kept` of a recording `frames` long. Those in what was cut go.
    pub(super) fn move_slice_markers_with(&mut self, node_id: NodeId, frames: usize, kept: std::ops::Range<usize>) {
        let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) else { return };
        let Some(hand) = node.user_data.slice_markers.as_mut() else { return };
        let moved = hand.places.iter().map(|&place| ((place as f64 * frames as f64 - kept.start as f64) / kept.len() as f64) as f32);
        hand.places = moved.filter(|place| *place > 0.0 && *place < 1.0).collect();
    }

    /// Gives every Sampler sliced by Onsets the markers its node shows,
    /// wherever they aren't what it has; and lets go of markers placed by
    /// hand once Sensitivity has been turned from where they were placed.
    /// Call once a frame, after the samples are synced.
    pub(super) fn sync_markers(&mut self) {
        let samplers: Vec<NodeId> =
            self.graph_state.graph.nodes.iter().filter(|(_, node)| node.user_data.module_id == SAMPLER_ID).map(|(node_id, _)| node_id).collect();
        let mut sends = Vec::new();
        for node_id in samplers {
            let graph = &mut self.graph_state.graph;
            let sensitivity = value_of(graph, node_id, "Sensitivity").unwrap_or(DEFAULT_SENSITIVITY);
            let data = &mut graph[node_id].user_data;
            // Turning Sensitivity finds them afresh
            if data.slice_markers.as_ref().is_some_and(|hand| !hand.stand_at(sensitivity)) {
                data.slice_markers = None;
            }
            let (hand, file) = (data.slice_markers.clone(), data.file.clone());
            if !value_of(graph, node_id, "Slices").is_some_and(Slices::is_marked_param) {
                continue;
            }
            let (Some(file), Some(engine_id)) = (file, self.user_state.get_engine_node_id(node_id)) else { continue };
            let Some(Ok(shelved)) = self.user_state.samples.get(&file) else { continue };
            let (start, end) = region(graph, node_id);
            let markers = Markers::between(&shelved.onsets().markers(hand.as_ref(), sensitivity), start, end);
            let Some(handle) = self.ui_handle.as_ref() else { continue };
            if handle.graph().contains_module(engine_id) && handle.graph().markers(engine_id) != Some(&markers) {
                sends.push(EngineCommand::SetMarkers { node_id: engine_id, markers });
            }
        }
        for command in sends {
            self.send_command(command);
        }
    }
}
