//! The MIDI Out's face: which device it plays, and what it's playing.
//!
//! - **Along the top**, a lamp, the port and its device: green while
//!   messages go out, amber when the port has no device or it's unplugged,
//!   red when it won't open. Click the device to choose another; every
//!   MIDI Out on the port follows. At the right, the channel.
//! - **A keyboard** of five octaves (C2 to C7), each held note lit, so a
//!   chord shows as its shape. A note beyond either end lights an arrow.
//! - **Four bars** for the CC lanes, each with its controller number, and
//!   at the right the Clock's beat while clock goes out.

use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::link::midi::{PortStatus, MIDI, PORTS, PORT_LABELS};
use crate::modules::midi_out::CC_LANES;
use crate::modules::MidiOut;

use super::display_frame::{self, PANEL};
use super::{SynthGraph, SynthGraphState};

/// The keyboard's lowest note, C2, and how many it shows: five octaves and
/// the top C.
const LOWEST: u8 = 36;
const KEYS: u8 = 61;

/// How long a note's strike glows, in seconds.
const FLASH: f64 = 0.2;

/// Draws the face.
pub fn midi_out_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let port = (display_frame::value_of(graph, node_id, "Port").unwrap_or(0.0).round().max(0.0) as usize).min(PORTS - 1);
    let channel = display_frame::value_of(graph, node_id, "Channel").unwrap_or(0.0).round() as usize + 1;
    let readout = display_frame::readout(user_state, node_id).unwrap_or_default();
    let values = &readout.values;
    let live = !cfg!(target_arch = "wasm32") && values[MidiOut::READOUT_LIVE] > 0.5;
    let status = live.then(|| MIDI.status(port));
    let device = MIDI.chosen(port);

    let width = display_frame::knob_row_width(ui, CC_LANES, z);
    let accent = theme::module::OUTPUT;
    let Some((rect, response, painter)) = display_frame::frame_of_height(ui, width, 76.0 * z, accent, z) else { return };
    let small = egui::FontId::proportional(8.0 * z);
    let tiny = egui::FontId::proportional(6.5 * z);
    let purple = theme::signal::MIDI;
    let orange = theme::signal::CONTROL;
    let green = theme::signal::GATE;
    let amber = Color32::from_rgb(240, 190, 80);
    let red = Color32::from_rgb(240, 90, 90);
    let now = ui.input(|i| i.time);

    // --- The header: a lamp, the port, its device, and the channel ---
    let lamp = match status {
        None | Some(PortStatus::Idle) => theme::text::DISABLED,
        Some(PortStatus::Sending) => green,
        Some(PortStatus::NoDevice | PortStatus::Missing) => amber,
        Some(PortStatus::Failed) => red,
    };
    let lamp_at = rect.left_top() + Vec2::new(7.0 * z, 6.5 * z);
    if status == Some(PortStatus::Sending) {
        // A flash as each note goes out
        let flash = flash_of(ui, node_id, values[MidiOut::READOUT_NOTES_SENT] as u32, now);
        painter.circle_filled(lamp_at, (4.0 + 2.0 * flash) * z, lamp.gamma_multiply(0.12 + 0.3 * flash));
    }
    painter.circle_filled(lamp_at, 2.5 * z, lamp);
    let name = match (&device, status) {
        (_, None) => "desktop app only".to_string(),
        (None, _) => "choose a device".to_string(),
        (Some(name), Some(PortStatus::Missing)) => format!("{name} (unplugged)"),
        (Some(name), Some(PortStatus::Failed)) => format!("{name} (in use)"),
        (Some(name), _) => name.clone(),
    };
    let label_rect = Rect::from_min_max(
        lamp_at + Vec2::new(5.0 * z, -5.0 * z),
        Pos2::new(rect.right() - 26.0 * z, lamp_at.y + 5.0 * z),
    );
    let picker = ui.interact(label_rect, Id::new(("midi_out_device", node_id)), Sense::click());
    let text_color = if picker.hovered() && live {
        theme::text::PRIMARY
    } else if device.is_some() && live {
        theme::text::SECONDARY
    } else {
        theme::text::DISABLED
    };
    let galley = painter.layout_no_wrap(format!("{} · {name}", PORT_LABELS[port]), small.clone(), text_color);
    let clip = painter.with_clip_rect(label_rect.intersect(painter.clip_rect()));
    clip.galley(Pos2::new(label_rect.left(), lamp_at.y - galley.size().y / 2.0), galley, text_color);
    painter.text(
        rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z),
        egui::Align2::RIGHT_TOP,
        format!("Ch {channel}"),
        small.clone(),
        theme::text::SECONDARY,
    );
    if live {
        device_menu(ui, &picker, port, device.as_deref());
    }

    // --- The keyboard: white keys across, black keys over them ---
    let board = Rect::from_min_max(rect.left_top() + Vec2::new(9.0 * z, 15.0 * z), rect.right_top() + Vec2::new(-9.0 * z, 44.0 * z));
    let whites = (0..KEYS).filter(|k| !is_black(LOWEST + k)).count() as f32;
    let key_w = board.width() / whites;
    let lit = |note: u8| MidiOut::note_in(&readout, note);
    let mut white = 0.0;
    let mut blacks = Vec::new();
    for key in 0..KEYS {
        let note = LOWEST + key;
        if is_black(note) {
            blacks.push((note, board.left() + white * key_w));
            continue;
        }
        let r = Rect::from_min_size(Pos2::new(board.left() + white * key_w, board.top()), Vec2::new(key_w, board.height())).shrink2(Vec2::new(0.35 * z, 0.0));
        let on = lit(note);
        painter.rect_filled(r, 1.2 * z, if on { purple } else { Color32::from_rgb(196, 196, 214) });
        if on {
            painter.rect_filled(r.expand(1.2 * z), 2.0 * z, purple.gamma_multiply(0.25));
        }
        if note % 12 == 0 {
            // Each C named, faintly, to read the octave by
            painter.text(
                Pos2::new(r.center().x, r.bottom() - 1.0 * z),
                egui::Align2::CENTER_BOTTOM,
                format!("C{}", note / 12 - 1),
                egui::FontId::proportional(4.6 * z),
                if on { PANEL } else { Color32::from_rgb(120, 120, 150) },
            );
        }
        white += 1.0;
    }
    for (note, x) in blacks {
        let r = Rect::from_center_size(Pos2::new(x, board.top() + board.height() * 0.31), Vec2::new(key_w * 0.62, board.height() * 0.62));
        painter.rect_filled(r, 1.0 * z, if lit(note) { purple } else { PANEL });
    }
    // Notes beyond the keyboard: an arrow at that end
    let below = (0..LOWEST).any(lit);
    let above = (LOWEST + KEYS..128).any(lit);
    for (beyond, x, sign) in [(below, board.left() - 4.5 * z, -1.0), (above, board.right() + 4.5 * z, 1.0)] {
        let y = board.center().y;
        let tip = Pos2::new(x + sign * 2.5 * z, y);
        let points = vec![tip, Pos2::new(x - sign * 2.0 * z, y - 3.5 * z), Pos2::new(x - sign * 2.0 * z, y + 3.5 * z)];
        let color = if beyond { purple } else { theme::background::GRID_MAJOR };
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }

    // --- The CC lanes: a bar each, its number beneath ---
    let row = Rect::from_min_max(Pos2::new(rect.left() + 6.0 * z, rect.bottom() - 26.0 * z), Pos2::new(rect.right() - 40.0 * z, rect.bottom() - 13.0 * z));
    let pitch = row.width() / CC_LANES as f32;
    for lane in 0..CC_LANES {
        let value = values[MidiOut::READOUT_CC + lane];
        let patched = value >= 0.0;
        let number = display_frame::value_of(graph, node_id, ["CC 1 #", "CC 2 #", "CC 3 #", "CC 4 #"][lane]).unwrap_or(0.0).round();
        let slot = Rect::from_min_size(Pos2::new(row.left() + pitch * lane as f32 + 2.0 * z, row.top()), Vec2::new(pitch - 4.0 * z, 5.0 * z));
        painter.rect(slot, 2.0 * z, theme::background::MAIN, Stroke::new(0.5 * z, theme::background::GRID_MAJOR));
        if patched {
            let fill = Rect::from_min_size(slot.min, Vec2::new(slot.width() * value / 127.0, slot.height())).shrink(0.8 * z);
            if fill.width() > 0.2 * z {
                painter.rect_filled(fill, 1.5 * z, orange);
            }
        }
        painter.text(
            Pos2::new(slot.center().x, slot.bottom() + 1.5 * z),
            egui::Align2::CENTER_TOP,
            format!("CC {number:.0}"),
            tiny.clone(),
            if patched { orange } else { theme::text::DISABLED },
        );
    }

    // --- The clock: four beat lamps, lit in turn while it goes out ---
    let clock = values[MidiOut::READOUT_CLOCK];
    let beat = values[MidiOut::READOUT_BEAT].max(0.0) as usize;
    let clock_y = row.top() + 2.5 * z;
    for i in 0..4 {
        let at = Pos2::new(rect.right() - 30.0 * z + i as f32 * 7.0 * z, clock_y);
        let on = clock > 1.5 && beat % 4 == i;
        let color = if on {
            if i == 0 { accent } else { theme::text::SECONDARY }
        } else {
            theme::background::GRID_MAJOR
        };
        painter.circle_filled(at, if on { 2.6 * z } else { 1.8 * z }, color);
    }
    painter.text(
        Pos2::new(rect.right() - 19.5 * z, clock_y + 4.0 * z),
        egui::Align2::CENTER_TOP,
        "clock",
        tiny,
        if clock > 0.5 { theme::text::SECONDARY } else { theme::text::DISABLED },
    );

    let dropped = values[MidiOut::READOUT_DROPPED] > 0.0;
    response.on_hover_text(match status {
        None => "MIDI Out plays devices from the desktop app; in the browser it stays quiet".to_string(),
        Some(PortStatus::NoDevice) | Some(PortStatus::Idle) if device.is_none() => {
            format!("Port {} has no device yet: click its name to choose one", PORT_LABELS[port])
        }
        Some(PortStatus::Missing) => "The device isn't plugged in. Notes go out again once it is".to_string(),
        Some(PortStatus::Failed) => "The device wouldn't open. Another app may be using it: close that, and Soba tries again".to_string(),
        _ if dropped => "Some messages were lost: the patch sent more than the device could take".to_string(),
        _ => format!(
            "Playing port {} on channel {channel}: gates and pitch as notes, the CC lanes as controllers{}",
            PORT_LABELS[port],
            if clock > 0.5 { ", and the Clock as MIDI clock" } else { "" }
        ),
    });
}

/// The menu a click on the device's name opens: every output found, or none.
fn device_menu(ui: &mut egui::Ui, picker: &egui::Response, port: usize, current: Option<&str>) {
    let popup = picker.id.with("menu");
    if picker.clicked() {
        MIDI.rescan();
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    egui::popup::popup_below_widget(ui, popup, picker, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(160.0);
        ui.label(egui::RichText::new(format!("Port {} plays", PORT_LABELS[port])).small().weak());
        let devices = MIDI.devices();
        if devices.is_empty() {
            ui.label(egui::RichText::new("No MIDI outputs found").weak());
        }
        for device in &devices {
            if ui.selectable_label(current == Some(device.as_str()), device).clicked() {
                MIDI.choose(port, Some(device));
            }
        }
        // Chosen before, on another day, and not plugged in now
        if let Some(missing) = current.filter(|c| !devices.iter().any(|d| d == c)) {
            let _ = ui.selectable_label(true, format!("{missing} (unplugged)"));
        }
        ui.separator();
        if ui.selectable_label(current.is_none(), "None").clicked() {
            MIDI.choose(port, None);
        }
    });
}

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// How bright the lamp's flash is, 0 to 1: full when the count of notes
/// sent has moved since the last frame, fading over [`FLASH`].
fn flash_of(ui: &egui::Ui, node_id: NodeId, count: u32, now: f64) -> f32 {
    let id = Id::new(("midi_out_flash", node_id));
    let (seen, at) = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || (count, f64::NEG_INFINITY)));
    let at = if seen != count {
        ui.ctx().data_mut(|d| d.insert_temp(id, (count, now)));
        now
    } else {
        at
    };
    (1.0 - (now - at) / FLASH).clamp(0.0, 1.0) as f32
}
