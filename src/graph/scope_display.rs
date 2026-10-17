//! The Oscilloscope's display: its two live traces, and the trigger level.

use eframe::egui;
use egui_node_graph2::NodeId;

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Draws the traces.
pub fn scope_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    // Get scope data from user state
    let scope_data = user_state
        .get_engine_node_id(node_id)
        .and_then(|eid| user_state.get_scope_data(eid));

    // Get trigger level from the node's input parameters
    let trigger_level = if let Some(node) = graph.nodes.get(node_id) {
        let mut level = 0.0f32;
        for (name, input_id) in &node.inputs {
            if name == "Trigger Level" {
                let input = graph.get_input(*input_id);
                if let SynthValueType::Number { value, .. } = &input.value {
                    level = *value;
                }
            }
        }
        level
    } else {
        0.0
    };

    // Render the oscilloscope display
    let (channel1, channel2) = if let Some(data) = scope_data {
        (data.channel1.as_slice(), data.channel2.as_slice())
    } else {
        (&[] as &[f32], &[] as &[f32])
    };

    let config = crate::widgets::OscilloscopeConfig::new(200.0 * zoom, 120.0 * zoom)
        .with_trigger_level(trigger_level)
        .with_trigger_indicator(true);

    crate::widgets::oscilloscope_display(ui, channel1, channel2, &config);
}
