//! The Quantizer's display: its scale on a one-octave piano, the notes it
//! plays glowing, and their names under the keys. Clicking a key adds it to
//! or takes it out of the scale.

use eframe::egui;
use egui_node_graph2::NodeId;

use crate::modules::quantizer::{self, NOTE_NAMES};
use crate::widgets::{piano_keys, PianoConfig, PianoData};

use super::{SynthGraph, SynthGraphState, SynthResponse, SynthValueType};

/// Draws the scale, and returns the Mask and Scale a click on a key sets.
pub fn scale_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let mut responses = Vec::new();
    let engine_node_id = user_state.get_engine_node_id(node_id);

    let node = graph.nodes.get(node_id);
    let input_of = |name: &str| node.and_then(|n| n.inputs.iter().find(|(n, _)| n == name).map(|(_, id)| *id));
    let value_of = |name: &str| {
        input_of(name).map_or(0.0, |id| match &graph.get_input(id).value {
            SynthValueType::Number { value, .. } => *value,
            SynthValueType::Select { value, .. } => *value as f32,
            _ => 0.0,
        })
    };
    let scale = value_of("Scale") as usize;
    let relative = quantizer::scale_mask(scale, value_of("Mask").round() as u16);

    // The key Out is in: the root moved by Transpose, CV included
    let transpose_patched = input_of("Transpose")
        .is_some_and(|id| graph.iter_connections().any(|(input, _)| input == id));
    let transpose_cv = if transpose_patched {
        engine_node_id.and_then(|eid| user_state.get_input_value(eid, 1)).unwrap_or(0.0)
    } else {
        0.0
    };
    let tonic = value_of("Root") as i32 + (value_of("Transpose") + transpose_cv * 12.0).round() as i32;

    // The notes Out is playing, one per voice
    let playing: Vec<u8> = engine_node_id
        .and_then(|eid| user_state.output_channels.get(&(eid, 0)))
        .map(|peaks| {
            (0..peaks.count())
                .map(|voice| (60.0 + peaks.peak(voice) * 12.0).round().clamp(0.0, 127.0) as u8)
                .collect()
        })
        .unwrap_or_default();

    let data = PianoData {
        active_notes: playing.clone(),
        base_note: 60,
        octave_shift: 0,
        scale: Some(quantizer::rotate_to_key(relative, tonic)),
        root: Some(tonic.rem_euclid(12) as u8),
    };
    let accent = crate::dsp::SignalType::Control.color();
    let config = PianoConfig::scale(accent).with_size(140.0 * zoom, 45.0 * zoom);

    let (response, hovered) = ui
        .horizontal(|ui| {
            ui.add_space((ui.available_width() - 140.0 * zoom) / 2.0); // Center the display
            piano_keys(ui, &data, &config)
        })
        .inner;

    if let Some(key) = hovered {
        let bit = 1 << (key as i32 - tonic).rem_euclid(12);
        let name = NOTE_NAMES[key as usize];
        let tip = if relative & bit != 0 {
            format!("{name}: click to take it out of the scale")
        } else {
            format!("{name}: click to add it to the scale")
        };
        if response.clicked() {
            responses.push(SynthResponse::ParameterChanged {
                node_id,
                param_name: "Mask".to_string(),
                value: (relative ^ bit) as f32,
            });
            responses.push(SynthResponse::ParameterChanged {
                node_id,
                param_name: "Scale".to_string(),
                value: quantizer::CUSTOM_SCALE as f32,
            });
        }
        response.on_hover_text(tip);
    }

    // The notes by name, under the keys
    let mut names: Vec<String> = playing.iter().map(|&note| crate::modules::sequencer::note_to_name(note)).collect();
    names.dedup();
    let caption = if names.is_empty() { "–".to_string() } else { names.join("  ") };
    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - 140.0 * zoom) / 2.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(140.0 * zoom, 14.0 * zoom), egui::Sense::hover());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            caption,
            egui::FontId::proportional(10.0 * zoom),
            accent,
        );
    });

    responses
}
