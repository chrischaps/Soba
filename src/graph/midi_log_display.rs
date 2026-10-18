//! The MIDI Monitor's display: the latest events, newest first, after its
//! channel and type filters.

use eframe::egui::{self, Color32};
use egui_node_graph2::NodeId;

use crate::engine::midi_engine::MidiEvent;

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// MIDI event colors for the MIDI Monitor display.
mod midi_colors {
    use super::Color32;

    /// Note On/Off events (green)
    pub const NOTE: Color32 = Color32::from_rgb(100, 200, 100);
    /// Control Change events (orange)
    pub const CC: Color32 = Color32::from_rgb(255, 165, 0);
    /// Pitch Bend events (purple)
    pub const PITCH_BEND: Color32 = Color32::from_rgb(180, 100, 200);
    /// Transport events from a MIDI clock master: Start, Stop (pale blue)
    pub const TRANSPORT: Color32 = Color32::from_rgb(130, 180, 230);
    /// Other events (gray)
    pub const OTHER: Color32 = Color32::from_rgb(150, 150, 150);
}

/// Convert a MIDI note number to a note name (e.g., 60 -> "C4").
fn note_to_name(note: u8) -> String {
    const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = (note / 12) as i32 - 1; // MIDI note 60 = C4
    let name = NOTES[(note % 12) as usize];
    format!("{}{}", name, octave)
}

/// Rows in the MIDI Monitor's log.
const MIDI_LOG_ROWS: usize = 8;
/// The longest line the log usually shows, which sets its width.
const MIDI_LOG_WIDEST: &str = "NoteOn Ch16 C#-1 vel=127";

/// Where a note came from: its MIDI channel, or the computer keys.
fn note_channel(channel: u8) -> String {
    if channel == crate::dsp::KEYS_CHANNEL { "Keys".to_string() } else { format!("Ch{}", channel + 1) }
}

/// Format a MIDI event for display.
fn format_midi_event(event: &MidiEvent) -> (String, Color32) {
    match event {
        MidiEvent::NoteOn { channel, note, velocity } => (
            format!("NoteOn {} {} vel={}", note_channel(*channel), note_to_name(*note), velocity),
            midi_colors::NOTE,
        ),
        MidiEvent::NoteOff { channel, note, .. } => (
            format!("NoteOff {} {}", note_channel(*channel), note_to_name(*note)),
            midi_colors::NOTE,
        ),
        MidiEvent::ControlChange { channel, controller, value } => (
            format!("CC Ch{} #{} val={}", channel + 1, controller, value),
            midi_colors::CC,
        ),
        MidiEvent::PitchBend { channel, value } => (
            format!("PitchBend Ch{} {}", channel + 1, value),
            midi_colors::PITCH_BEND,
        ),
        MidiEvent::ChannelPressure { channel, pressure } => (
            format!("Pressure Ch{} {}", channel + 1, pressure),
            midi_colors::OTHER,
        ),
        MidiEvent::PolyPressure { channel, note, pressure } => (
            format!("PolyPres Ch{} {} {}", channel + 1, note_to_name(*note), pressure),
            midi_colors::OTHER,
        ),
        MidiEvent::ProgramChange { channel, program } => (
            format!("Program Ch{} #{}", channel + 1, program),
            midi_colors::OTHER,
        ),
        // Ticks never reach the UI, but name them in case one does
        MidiEvent::Clock => ("Clock".to_string(), midi_colors::TRANSPORT),
        MidiEvent::Start => ("Start".to_string(), midi_colors::TRANSPORT),
        MidiEvent::Continue => ("Continue".to_string(), midi_colors::TRANSPORT),
        MidiEvent::Stop => ("Stop".to_string(), midi_colors::TRANSPORT),
        MidiEvent::SongPosition { sixteenths } => (
            format!("SongPos bar {} 16th {}", sixteenths / 16 + 1, sixteenths % 16 + 1),
            midi_colors::TRANSPORT,
        ),
    }
}

/// Draws the event log.
pub fn midi_log_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    // Get filter settings from the node's input parameters
    let (channel_filter, show_notes, show_cc, show_pitch_bend) = if let Some(node) = graph.nodes.get(node_id) {
        let mut channel = 0usize; // 0 = all channels
        let mut notes = true;
        let mut cc = true;
        let mut pb = true;

        for (name, input_id) in &node.inputs {
            let input = graph.get_input(*input_id);
            match name.as_str() {
                "Channel" => {
                    if let SynthValueType::Select { value, .. } = &input.value {
                        channel = *value;
                    }
                }
                "Notes" => {
                    if let SynthValueType::Toggle { value, .. } = &input.value {
                        notes = *value;
                    }
                }
                "CC" => {
                    if let SynthValueType::Toggle { value, .. } = &input.value {
                        cc = *value;
                    }
                }
                "Pitch Bend" => {
                    if let SynthValueType::Toggle { value, .. } = &input.value {
                        pb = *value;
                    }
                }
                _ => {}
            }
        }
        (channel, notes, cc, pb)
    } else {
        (0, true, true, true)
    };

    // Render MIDI event log
    let midi_events = user_state.midi_events();

    // Newest first, after the channel and type filters
    let visible: Vec<_> = midi_events
        .iter()
        .rev()
        .filter(|event| {
            // Channel filter: 0 = all, 1-16 = that channel
            let channel_ok = channel_filter == 0
                || event.event.channel() as usize + 1 == channel_filter;
            let type_ok = match &event.event {
                MidiEvent::NoteOn { .. } | MidiEvent::NoteOff { .. } => show_notes,
                MidiEvent::ControlChange { .. } => show_cc,
                MidiEvent::PitchBend { .. } => show_pitch_bend,
                _ => true, // Always show other events
            };
            channel_ok && type_ok
        })
        .take(MIDI_LOG_ROWS)
        .collect();

    // A fixed panel holding exactly the rows it shows, painted at a set
    // pitch: rows can't crowd into each other, and the node keeps its
    // size as events arrive
    let font = egui::FontId::monospace(egui::TextStyle::Small.resolve(ui.style()).size);
    let row_height = ui.fonts(|f| f.row_height(&font)) + 2.0 * zoom;
    let gap = 6.0 * zoom;
    let (stamp_width, width) = ui.fonts(|f| {
        let measure = |text: &str| f.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x;
        let stamp = measure("000.0s");
        (stamp, (stamp + gap + measure(MIDI_LOG_WIDEST)).max(180.0 * zoom))
    });
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, MIDI_LOG_ROWS as f32 * row_height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let weak = ui.visuals().weak_text_color();

    if visible.is_empty() {
        let note = if midi_events.is_empty() { "No MIDI events" } else { "No matching events" };
        painter.text(
            egui::pos2(rect.left(), rect.top() + row_height / 2.0),
            egui::Align2::LEFT_CENTER,
            note,
            font.clone(),
            weak,
        );
    }
    for (row, event) in visible.iter().enumerate() {
        let y = rect.top() + (row as f32 + 0.5) * row_height;
        let (text, color) = format_midi_event(&event.event);
        painter.text(
            egui::pos2(rect.left() + stamp_width, y),
            egui::Align2::RIGHT_CENTER,
            format!("{:.1}s", event.timestamp),
            font.clone(),
            weak,
        );
        painter.text(egui::pos2(rect.left() + stamp_width + gap, y), egui::Align2::LEFT_CENTER, text, font.clone(), color);
    }
}
