//! The faces of the single senders (#484): Strike and CC Out.
//!
//! - **A Strike**: a lamp that flares as it strikes and stays lit while it
//!   holds its note; the part it strikes, large (Kick, Track 3, Pad 13), or
//!   its own Note; and underneath, the port and channel it plays and the
//!   device they come from. Click the part to choose another of the
//!   device's parts.
//! - **A CC Out**: a bar of the value it last sent; the parameter it sends,
//!   large (Cutoff, Decay), or its CC; and underneath, the value as the
//!   device reads it, the port and channel, and the device. Click the name
//!   to choose another of the device's parameters.
//!
//! Patched from a device, its port and channel are the device's (see
//! `device_follow`), and the menu says so; unpatched, the menu chooses them.

use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::hardware::{self, instrument, Part};
use crate::link::midi::PORT_LABELS;
use crate::modules::{cc_out::CcOut, strike::Strike};

use super::display_frame;
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// How long a strike's flare takes to fade, in seconds.
const FLARE: f32 = 0.2;

/// The face's height, before zoom.
const HEIGHT: f32 = 38.0;

/// What a sender plays: its port, its channel (1-16), and the device's
/// node it's patched from, named by its profile or its title.
fn where_it_plays(graph: &SynthGraph, node_id: NodeId) -> (usize, u8, Option<String>) {
    let port = display_frame::value_of(graph, node_id, "Port").unwrap_or(0.0).round().max(0.0) as usize;
    let channel = display_frame::value_of(graph, node_id, "Channel").unwrap_or(0.0).round().clamp(0.0, 15.0) as u8 + 1;
    let device = super::device_follow::device_of(graph, node_id).map(|device| {
        let node = &graph[device];
        instrument::profile_name(&node.user_data.labels).map_or_else(|| node.label.clone(), str::to_string)
    });
    (port.min(PORT_LABELS.len() - 1), channel, device)
}

/// The line under the name: "A · ch 10 · Digitakt".
fn place_line(port: usize, channel: u8, device: &Option<String>) -> String {
    let from = device.as_ref().map(|name| format!(" · {name}")).unwrap_or_default();
    format!("{} · ch {channel}{from}", PORT_LABELS[port])
}

/// The name, large, as a control to click: returns its response.
fn name_control(ui: &mut egui::Ui, painter: &egui::Painter, rect: Rect, id: Id, text: &str, accent: Color32, z: f32) -> egui::Response {
    let name_rect = Rect::from_min_max(rect.left_top() + Vec2::new(16.0 * z, 3.0 * z), rect.right_top() + Vec2::new(-4.0 * z, 21.0 * z));
    let picker = ui.interact(name_rect, id, Sense::click());
    let ink = if picker.hovered() { theme::text::PRIMARY } else { accent.lerp_to_gamma(theme::text::PRIMARY, 0.6) };
    let font = egui::FontId::new(11.0 * z, egui::FontFamily::Name(theme::TITLE_FAMILY.into()));
    let galley = painter.layout_no_wrap(text.to_string(), font, ink);
    painter
        .with_clip_rect(name_rect.intersect(painter.clip_rect()))
        .galley(Pos2::new(name_rect.left(), name_rect.top() + 9.0 * z - galley.size().y / 2.0), galley, ink);
    painter.rect_stroke(rect, 2.0 * z, egui::Stroke::new(0.5 * z, display_frame::with_alpha(accent, if picker.hovered() { 90 } else { 30 })), egui::StrokeKind::Inside);
    picker
}

/// The small line along the bottom, from `left` (before zoom) in.
fn small_line(painter: &egui::Painter, rect: Rect, left: f32, text: String, z: f32) {
    let small = egui::FontId::proportional(7.5 * z);
    painter.text(rect.left_bottom() + Vec2::new(left * z, -4.0 * z), egui::Align2::LEFT_BOTTOM, text, small, theme::text::DISABLED);
}

/// A popup under `picker`, open while it's toggled by a click.
fn popup(ui: &egui::Ui, picker: &egui::Response, contents: impl FnOnce(&mut egui::Ui)) {
    let id = picker.id.with("menu");
    if picker.clicked() {
        egui::Popup::toggle_id(ui.ctx(), id);
    }
    egui::Popup::from_response(picker)
        .id(id)
        .open_memory(None)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(170.0);
            contents(ui);
        });
}

/// The Port and Channel menus while unpatched, or what sets them.
fn place_items(ui: &mut egui::Ui, node_id: NodeId, port: usize, channel: u8, device: &Option<String>, asked: &mut Vec<SynthResponse>) {
    ui.separator();
    if let Some(device) = device {
        ui.label(egui::RichText::new(format!("Port {} and channel {channel} are {device}'s: unplug its MIDI cable to choose here", PORT_LABELS[port])).small().weak());
        return;
    }
    let mut set = |param_name: &str, value: usize| asked.push(SynthResponse::ParameterChanged { node_id, param_name: param_name.to_string(), value: value as f32 });
    crate::widgets::menu::menu_button(ui, format!("Port {}", PORT_LABELS[port]), |ui| {
        for (index, label) in PORT_LABELS.iter().enumerate() {
            if ui.selectable_label(index == port, *label).clicked() {
                set("Port", index);
                ui.close();
            }
        }
    });
    crate::widgets::menu::menu_button(ui, format!("Channel {channel}"), |ui| {
        for index in 0..16 {
            if ui.selectable_label(index + 1 == channel as usize, format!("{}", index + 1)).clicked() {
                set("Channel", index);
                ui.close();
            }
        }
    });
}

/// A Strike's face, and what was chosen on it.
pub fn strike_face(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut asked = Vec::new();
    let accent = theme::signal::MIDI;
    let width = display_frame::knob_row_width(ui, 3, z);
    let Some((rect, _, painter)) = display_frame::frame_of_height(ui, width, HEIGHT * z, accent, z) else { return asked };
    let data = &graph[node_id].user_data;
    let profiles = hardware::known();
    let part = instrument::strike_part(&data.labels, &profiles);
    let note = display_frame::value_of(graph, node_id, "Note").unwrap_or(36.0).round().clamp(0.0, 127.0) as u8;
    let (port, channel, device) = where_it_plays(graph, node_id);

    // The lamp: a flare as it strikes, lit while it holds its note
    let (struck, held, quiet) = display_frame::readout(user_state, node_id).map_or((0.0, false, 60.0), |readout| Strike::struck_in(&readout));
    let flare = (1.0 - quiet / FLARE).clamp(0.0, 1.0) * (0.4 + 0.6 * struck);
    if flare > 0.0 {
        crate::app::pace::animate(ui.ctx());
    }
    let lamp_at = rect.left_top() + Vec2::new(9.0 * z, 12.0 * z);
    if flare > 0.0 {
        painter.circle_filled(lamp_at, (4.0 + 3.0 * flare) * z, accent.gamma_multiply(0.15 + 0.35 * flare));
    }
    let lamp = if held || flare > 0.0 { accent.lerp_to_gamma(Color32::WHITE, 0.5 * flare) } else { theme::text::DISABLED };
    painter.circle_filled(lamp_at, 3.0 * z, lamp);

    let name = part.as_ref().map_or_else(|| format!("Note {note}"), |part| part.name.clone());
    let picker = name_control(ui, &painter, rect, Id::unique(("strike_part", node_id)), &name, accent, z);
    let what = match &part {
        Some(Part { note: Some(n), .. }) => format!("note {n} · "),
        Some(_) => "any note · ".to_string(),
        None => String::new(),
    };
    let on = part.as_ref().and_then(|part| part.channel).unwrap_or(channel);
    small_line(&painter, rect, 5.0, format!("{what}{}", place_line(port, on, &device)), z);

    popup(ui, &picker, |ui| {
        let known = instrument::profile_name(&data.labels).and_then(|name| profiles.find(name));
        let parts = known.map(|known| known.profile.parts.as_slice()).unwrap_or_default();
        let mut give = |part: Option<&Part>| {
            let (label, text) = match part {
                Some(part) => (format!("Strike {}", part.name), instrument::written_part(part)),
                None => ("Strike its own note".to_string(), String::new()),
            };
            asked.push(SynthResponse::SetLabels { node_id, label, labels: vec![(instrument::PART.to_string(), text)] });
        };
        if parts.is_empty() {
            let hint = match known {
                Some(known) => format!("{} has no parts to strike: it plays its own note", known.profile.name),
                None => "Patch from an Instrument or Kit playing a drum machine or sampler to strike its parts by name".to_string(),
            };
            ui.label(egui::RichText::new(hint).small().weak());
        } else {
            ui.label(egui::RichText::new("Strikes").small().weak());
            let current = part.as_ref().map(|part| part.name.as_str());
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for part in parts {
                    let note = part.note.map(|n| format!("note {n}")).unwrap_or_else(|| "any note".to_string());
                    let channel = part.channel.map(|c| format!(" ch {c}")).unwrap_or_default();
                    if ui.selectable_label(current == Some(part.name.as_str()), &part.name).on_hover_text(format!("{note}{channel}")).clicked() {
                        give(Some(part));
                    }
                }
            });
        }
        ui.separator();
        if ui.selectable_label(part.is_none(), format!("Its own note: {note}")).clicked() {
            give(None);
        }
        let mut own = note as i32;
        if ui.add(egui::DragValue::new(&mut own).range(0..=127).prefix("Note ")).changed() {
            asked.push(SynthResponse::ParameterChanged { node_id, param_name: "Note".to_string(), value: own as f32 });
        }
        place_items(ui, node_id, port, channel, &device, &mut asked);
    });
    picker.on_hover_text("What it strikes: click to choose one of its device's parts");
    asked
}


/// A CC Out's face, and what was chosen on it.
pub fn cc_out_face(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut asked = Vec::new();
    let accent = theme::signal::MIDI;
    let width = display_frame::knob_row_width(ui, 3, z);
    let Some((rect, _, painter)) = display_frame::frame_of_height(ui, width, HEIGHT * z, accent, z) else { return asked };
    let data = &graph[node_id].user_data;
    let profiles = &hardware::known();
    let held = instrument::cc_out_held(&data.labels, profiles);
    let cc = display_frame::value_of(graph, node_id, "CC").unwrap_or(74.0).round().clamp(0.0, 127.0) as u8;
    let (port, channel, device) = where_it_plays(graph, node_id);

    // The bar: what it last sent, across its travel
    let travel = display_frame::readout(user_state, node_id).map_or(-1.0, |readout| readout.values[CcOut::READOUT_TRAVEL]);
    let bar = Rect::from_min_max(rect.left_top() + Vec2::new(4.0 * z, 4.0 * z), rect.left_top() + Vec2::new(7.0 * z, (HEIGHT - 4.0) * z));
    painter.rect_filled(bar, 1.0 * z, display_frame::with_alpha(accent, 40));
    if travel >= 0.0 {
        let top = bar.bottom() - bar.height() * travel.clamp(0.0, 1.0);
        painter.rect_filled(Rect::from_min_max(Pos2::new(bar.left(), top), bar.max), 1.0 * z, accent);
    }

    let name = held.as_ref().map_or_else(|| format!("CC {cc}"), |held| held.parameter.name.clone());
    let picker = name_control(ui, &painter, rect, Id::unique(("cc_out_parameter", node_id)), &name, accent, z);
    let value = match (&held, travel >= 0.0) {
        (Some(held), true) => format!("{} · ", held.parameter.text(hardware::SlotMap::of(&held.parameter).value(travel))),
        (None, true) => format!("{} · ", (travel.clamp(0.0, 1.0) * 127.0).round() as u8),
        _ => String::new(),
    };
    let on = held.as_ref().and_then(|held| held.parameter.channel).unwrap_or(channel);
    small_line(&painter, rect, 16.0, format!("{value}{}", place_line(port, on, &device)), z);

    popup(ui, &picker, |ui| {
        let known = instrument::profile_name(&data.labels).and_then(|name| profiles.find(name));
        let mut give = |parameter: Option<&hardware::Parameter>| {
            let (label, text) = match parameter {
                Some(parameter) => (format!("CC Out to {}", parameter.name), instrument::written(parameter)),
                None => ("CC Out to a plain CC".to_string(), String::new()),
            };
            asked.push(SynthResponse::SetLabels { node_id, label, labels: vec![(instrument::PARAMETER.to_string(), text)] });
            if let Some(parameter) = parameter {
                asked.push(SynthResponse::ParameterChanged { node_id, param_name: "Value".to_string(), value: instrument::start(parameter) });
            }
        };
        match known {
            Some(known) if !known.profile.parameters.is_empty() => {
                ui.label(egui::RichText::new(format!("Sends one of {}'s", known.profile.name)).small().weak());
                let current = held.as_ref().map(|held| held.parameter.name.as_str());
                // By group, in the order the profile first names each
                let mut groups: Vec<&str> = Vec::new();
                for parameter in &known.profile.parameters {
                    if !groups.contains(&parameter.group.as_str()) {
                        groups.push(&parameter.group);
                    }
                }
                for group in groups {
                    let title = if group.is_empty() { "Other" } else { group };
                    crate::widgets::menu::menu_button(ui, egui::RichText::new(title).color(hardware::group_color(group)), |ui| {
                        for parameter in known.profile.parameters.iter().filter(|p| p.group == group) {
                            if ui.selectable_label(current == Some(parameter.name.as_str()), &parameter.name).on_hover_text(parameter.address.text()).clicked() {
                                give(Some(parameter));
                                ui.close();
                            }
                        }
                    });
                }
            }
            _ => {
                ui.label(egui::RichText::new("Patch from an Instrument or Kit to send one of its device's controls by name").small().weak());
            }
        }
        ui.separator();
        if ui.selectable_label(held.is_none(), format!("A plain CC: {cc}")).clicked() {
            give(None);
        }
        let mut number = cc as i32;
        if ui.add(egui::DragValue::new(&mut number).range(0..=127).prefix("CC ")).changed() {
            asked.push(SynthResponse::ParameterChanged { node_id, param_name: "CC".to_string(), value: number as f32 });
        }
        place_items(ui, node_id, port, channel, &device, &mut asked);
    });
    picker.on_hover_text("What it sends: click to choose one of its device's controls");
    asked
}
