//! The Noise module's display: the three slopes, with the patched ones lit.

use eframe::egui;
use egui_node_graph2::NodeId;

use crate::widgets::NoiseDisplayConfig;

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Draws the slopes, centred.
pub fn noise_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let (level, patched) = if let Some(node) = graph.nodes.get(node_id) {
        // With Level patched, the CV sets the level (often an envelope
        // over a knob at 0), so draw the slopes at full scale
        let level_input = node.inputs.iter().find(|(name, _)| name == "Level").map(|(_, id)| *id);
        let level = match level_input {
            Some(id) if super::patched::input(graph, id) => 1.0,
            Some(id) => match graph.get_input(id).value {
                SynthValueType::Number { value, .. } => value,
                _ => 0.5,
            },
            None => 0.5,
        };
        let is_patched = |port: &str| {
            node.outputs
                .iter()
                .find(|(name, _)| name == port)
                .is_some_and(|(_, id)| user_state.cables_from.patched(*id))
        };
        (level, [is_patched("White"), is_patched("Pink"), is_patched("Brown")])
    } else {
        (0.5, [false; 3])
    };

    let config = NoiseDisplayConfig {
        size: egui::vec2(140.0 * zoom, 50.0 * zoom),
        level,
        patched,
        playing: user_state.is_playing,
    };
    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - 140.0 * zoom) / 2.0); // Center the display
        crate::widgets::noise_display(ui, &config);
    });
}
