//! The MIDI In's face (#479): the device the modules patched from it hear.
//!
//! - **A lamp**, purple while the device is open and grey while it isn't,
//!   flaring with each message the device sends on the MIDI In's channel, so
//!   a pad struck on the controller shows on the node it comes in through.
//! - **The device's name**, large: click it to choose another, from the
//!   inputs open under the toolbar's MIDI In. "Any device" hears them all.
//! - **Underneath**, the channel, and how many modules hear it, through
//!   any groups between.

use eframe::egui::{self, Color32, Id, Sense, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::engine::device_names::same;

use super::display_frame;
use super::midi_device::{device, device_choices, hearing};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// How long the lamp's flare takes to fade, in seconds.
const FLARE: f64 = 0.2;

/// Draws the face, and the device chosen on it, if one is.
pub fn midi_in_face(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Option<SynthResponse> {
    let z = zoom;
    let purple = theme::signal::MIDI;
    let width = display_frame::knob_row_width(ui, 3, z);
    let (rect, _, painter) = display_frame::frame_of_height(ui, width, 38.0 * z, purple, z)?;
    let current = device(&graph[node_id].user_data);
    let channel = display_frame::value_of(graph, node_id, "Channel").and_then(crate::modules::midi_in::channel_of);
    let open = current.is_none_or(|name| user_state.midi_inputs.iter().any(|input| same(input, name)));

    // The newest message it passes on, as a count that moves with each one
    let latest = user_state
        .midi_events()
        .iter()
        .rev()
        .find(|event| {
            let from_it = match (current, event.device.as_deref()) {
                (Some(name), Some(from)) => same(name, from),
                (Some(_), None) => false,
                (None, _) => !event.thru_only,
            };
            from_it && channel.is_none_or(|only| event.event.channel() == only)
        })
        .map_or(0, |event| event.timestamp.to_bits());
    let now = ui.input(|i| i.time);
    let flare = super::midi_out_display::flash_of(ui, Id::unique(("midi_in_flare", node_id)), latest, now, FLARE);
    if flare > 0.0 {
        crate::app::pace::animate(ui.ctx());
    }

    let lamp = if open { purple } else { theme::text::DISABLED };
    let lamp_at = rect.left_center() + Vec2::new(9.0 * z, -5.0 * z);
    if flare > 0.0 {
        painter.circle_filled(lamp_at, (4.0 + 3.0 * flare) * z, lamp.gamma_multiply(0.15 + 0.35 * flare));
    }
    painter.circle_filled(lamp_at, 3.0 * z, lamp.lerp_to_gamma(Color32::WHITE, 0.5 * flare));

    // The device's name, the face's one control
    let name_rect = egui::Rect::from_min_max(rect.left_top() + Vec2::new(16.0 * z, 3.0 * z), rect.right_top() + Vec2::new(-4.0 * z, 21.0 * z));
    let picker = ui.interact(name_rect, Id::unique(("midi_in_device", node_id)), Sense::click());
    let name = match current {
        None => "Any device".to_string(),
        Some(name) if open => name.to_string(),
        Some(name) => format!("{name} (not open)"),
    };
    let ink = match (picker.hovered(), current.is_some() && open) {
        (true, _) => theme::text::PRIMARY,
        (false, true) => purple.lerp_to_gamma(theme::text::PRIMARY, 0.6),
        (false, false) => theme::text::SECONDARY,
    };
    let font = egui::FontId::proportional(11.0 * z);
    let galley = painter.layout_no_wrap(name, font, ink);
    let clip = painter.with_clip_rect(name_rect.intersect(painter.clip_rect()));
    clip.galley(egui::pos2(name_rect.left(), lamp_at.y - galley.size().y / 2.0), galley, ink);

    // The channel, and who hears it
    let channel = channel.map_or("every channel".to_string(), |channel| format!("channel {}", channel + 1));
    let listeners = match hearing(graph, node_id).len() {
        0 => "patch its MIDI into a module".to_string(),
        1 => "1 module hears it".to_string(),
        n => format!("{n} modules hear it"),
    };
    let small = egui::FontId::proportional(7.5 * z);
    let below = rect.left_bottom() + Vec2::new(5.0 * z, -4.0 * z);
    painter.text(below, egui::Align2::LEFT_BOTTOM, format!("{channel} · {listeners}"), small, theme::text::DISABLED);
    painter.rect_stroke(rect, 2.0 * z, egui::Stroke::new(0.5 * z, display_frame::with_alpha(purple, if picker.hovered() { 90 } else { 30 })), egui::StrokeKind::Inside);


    let popup = picker.id.with("menu");
    if picker.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    let mut chosen = None;
    egui::Popup::from_response(&picker)
        .id(popup)
        .open_memory(None)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            ui.set_min_width(160.0);
            ui.label(egui::RichText::new("The modules patched from it hear").small().weak());
            chosen = device_choices(ui, node_id, current, &user_state.midi_inputs);
        });
    picker.on_hover_text("Which MIDI device the modules patched from it hear: click to choose");
    chosen
}
