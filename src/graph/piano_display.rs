//! Note In's display: a piano of the keys held, from the octave the Octave
//! knob picks.

use eframe::egui;
use egui_node_graph2::NodeId;

use crate::widgets::{piano_keys, PianoConfig, PianoData};

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Draws Note In's piano, lit by the notes held from the keys or MIDI, and
/// playable like the computer keys with the mouse or a finger.
pub fn note_piano_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &mut SynthGraphState, zoom: f32) {
    let data = held_keys(user_state.midi_active_notes(), node_id, graph);
    let config = PianoConfig::midi().with_size(140.0 * zoom, 45.0 * zoom);
    playable_piano(ui, &data, config, user_state, zoom);
}

/// The keys held, on a piano from C4 moved by the node's Octave.
fn held_keys(active_notes: &[u8], node_id: NodeId, graph: &SynthGraph) -> PianoData {
    // Get octave parameter from node
    let octave_shift = if let Some(node) = graph.nodes.get(node_id) {
        let mut octave = 0i32;
        for (name, input_id) in &node.inputs {
            if name == "Octave" {
                let input = graph.get_input(*input_id);
                if let SynthValueType::Number { value, .. } = &input.value {
                    octave = *value as i32;
                }
            }
        }
        octave
    } else {
        0
    };

    // Base note is C4 (60) plus octave shift
    let base_note = (60 + octave_shift * 12).clamp(0, 127) as u8;

    PianoData {
        active_notes: active_notes.to_vec(),
        base_note,
        octave_shift,
        ..Default::default()
    }
}

/// Playable with the mouse, or a finger on a touch screen: the key held
/// plays like a computer key, and sliding across glides.
fn playable_piano(ui: &mut egui::Ui, data: &PianoData, config: PianoConfig, user_state: &mut SynthGraphState, zoom: f32) {
    let config = config.clickable();
    let (response, key) = ui
        .horizontal(|ui| {
            ui.add_space((ui.available_width() - 140.0 * zoom) / 2.0); // Center the display
            piano_keys(ui, data, &config)
        })
        .inner;
    if let (true, Some(key)) = (response.is_pointer_button_down_on(), key) {
        user_state.piano_pointer = Some(key as i32);
    }
}
