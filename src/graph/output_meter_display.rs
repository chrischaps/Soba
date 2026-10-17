//! The Audio Output's display: the output stage's level meter.

use eframe::egui;
use egui_node_graph2::NodeId;

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Draws the meter, centred.
pub fn output_meter_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    // The meter marks overs differently when the limiter is off
    let limiter_enabled = graph
        .nodes
        .get(node_id)
        .and_then(|node| {
            node.inputs.iter().find(|(name, _)| name == "Limiter").map(|(_, id)| *id)
        })
        .map(|input_id| match graph.get_input(input_id).value {
            SynthValueType::Toggle { value, .. } => value,
            _ => true,
        })
        .unwrap_or(true);

    let config = crate::widgets::LevelMeterConfig {
        limiter_enabled,
        ceiling_db: crate::dsp::dynamics::PeakLimiter::DEFAULT_CEILING_DB,
        ..Default::default()
    }
    .scaled(zoom);

    ui.horizontal(|ui| {
        let meter_width = config.width + config.bar_height * 9.0;
        ui.add_space(((ui.available_width() - meter_width) / 2.0).max(0.0));
        crate::widgets::level_meter(ui, &user_state.output_meter, &config);
    });
    ui.add_space(2.0 * zoom);
}
