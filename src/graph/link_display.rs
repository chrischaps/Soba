//! The Link Out's face: what it sends Seoul, and whether Seoul hears.
//!
//! - **Along the top**, a lamp and the port: green while bundles go out and
//!   something listens there, amber while the port is closed (Seoul isn't
//!   running, or listens elsewhere). At the right, the Clock's tempo.
//! - **Eight columns**, one per CV lane, filling up from the middle line
//!   for a positive value and down for a negative one, as Seoul gets them.
//!   An empty lane is a faint slot.
//! - **Four lamps** for the gates, lit while high and flashing on every
//!   rising edge, so a trigger too short to see still shows. Beside them,
//!   the beat in the bar, as the Clock's face draws it.

use eframe::egui::{self, Color32, Id, Pos2, Rect, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::link::{LinkStatus, BUS, CV_LANES, GATE_LANES, PORTS};
use crate::modules::LinkOut;

use super::display_frame::{self, PANEL};
use super::{SynthGraph, SynthGraphState};

/// How long a gate's flash takes to fade, in seconds.
const FLASH: f64 = 0.25;

/// Draws the face.
pub fn link_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let port = display_frame::value_of(graph, node_id, "Port").unwrap_or(0.0).round().max(0.0) as usize;
    let port = port.min(PORTS.len() - 1);
    let status = if cfg!(target_arch = "wasm32") { None } else { Some(BUS.slot(port).status()) };
    let readout = display_frame::readout(user_state, node_id).unwrap_or_default();
    let values = &readout.values;
    let patched = values[LinkOut::READOUT_PATCHED] as u32;

    let width = display_frame::knob_row_width(ui, 3, z);
    let accent = theme::module::OUTPUT;
    let Some((rect, response, painter)) = display_frame::frame_of_height(ui, width, 74.0 * z, accent, z) else { return };
    let small = egui::FontId::proportional(8.0 * z);
    let tiny = egui::FontId::proportional(6.5 * z);
    let orange = theme::signal::CONTROL;
    let green = theme::signal::GATE;
    let amber = Color32::from_rgb(240, 190, 80);

    // --- The header: a lamp, the port, and the tempo ---
    let (lamp, words) = match status {
        None => (theme::text::DISABLED, "desktop app only"),
        Some(LinkStatus::Idle) => (theme::text::DISABLED, "starting"),
        Some(LinkStatus::Sending) => (green, "listening"),
        Some(LinkStatus::NoListener) => (amber, "no Seoul"),
        Some(LinkStatus::Failed) => (Color32::from_rgb(240, 90, 90), "can't send"),
    };
    let lamp_at = rect.left_top() + Vec2::new(7.0 * z, 6.0 * z);
    if status == Some(LinkStatus::Sending) {
        // A slow breath while the link is up
        let breath = 0.5 + 0.5 * (ui.input(|i| i.time) * 2.0).sin() as f32;
        painter.circle_filled(lamp_at, 4.5 * z, lamp.gamma_multiply(0.15 + 0.2 * breath));
    }
    painter.circle_filled(lamp_at, 2.5 * z, lamp);
    painter.text(
        lamp_at + Vec2::new(6.0 * z, 0.0),
        egui::Align2::LEFT_CENTER,
        format!(":{} {words}", PORTS[port]),
        small.clone(),
        if status == Some(LinkStatus::Sending) { theme::text::SECONDARY } else { theme::text::DISABLED },
    );
    let has_clock = values[LinkOut::READOUT_HAS_CLOCK] > 0.5;
    let playing = values[LinkOut::READOUT_PLAYING] > 0.5;
    let tempo = if has_clock { format!("{:.0} BPM", values[LinkOut::READOUT_BPM]) } else { "no Clock".to_string() };
    painter.text(
        rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z),
        egui::Align2::RIGHT_TOP,
        tempo,
        small.clone(),
        if has_clock && playing { theme::text::SECONDARY } else { theme::text::DISABLED },
    );

    // --- The CV lanes: a column each, zero across the middle ---
    let lanes = Rect::from_min_max(rect.left_top() + Vec2::new(4.0 * z, 14.0 * z), rect.right_top() + Vec2::new(-4.0 * z, 46.0 * z));
    let pitch = lanes.width() / CV_LANES as f32;
    let middle = lanes.center().y;
    for lane in 0..CV_LANES {
        let column = Rect::from_center_size(
            Pos2::new(lanes.left() + pitch * (lane as f32 + 0.5), middle),
            Vec2::new((pitch - 4.0 * z).max(2.0 * z), lanes.height()),
        );
        let live = patched & (1 << lane) != 0;
        painter.rect(column, 2.0 * z, theme::background::MAIN, Stroke::new(0.5 * z, theme::background::GRID_MAJOR));
        painter.hline(column.x_range(), middle, Stroke::new(0.5 * z, theme::background::GRID_MAJOR));
        if live {
            let value = values[LinkOut::READOUT_CV + lane];
            let reach = value.clamp(-1.0, 1.0) * (column.height() / 2.0 - 1.0 * z);
            let fill = Rect::from_x_y_ranges(column.shrink(1.5 * z).x_range(), (middle - reach).min(middle)..=(middle - reach).max(middle));
            if fill.height() > 0.2 * z {
                painter.rect_filled(fill.expand(1.0 * z), 2.0 * z, orange.gamma_multiply(0.18));
                painter.rect_filled(fill, 1.5 * z, orange);
            }
        }
        painter.text(
            Pos2::new(column.center().x, column.bottom() + 1.0 * z),
            egui::Align2::CENTER_TOP,
            (lane + 1).to_string(),
            tiny.clone(),
            if live { orange } else { theme::text::DISABLED },
        );
    }

    // --- The gates: a lamp each, flashing on every rising edge ---
    let now = ui.input(|i| i.time);
    let row_y = rect.bottom() - 9.0 * z;
    for lane in 0..GATE_LANES {
        let at = Pos2::new(rect.left() + 10.0 * z + lane as f32 * 15.0 * z, row_y);
        let live = patched & (1 << (CV_LANES + lane)) != 0;
        let high = values[LinkOut::READOUT_GATE + lane] > 0.5;
        let edges = values[LinkOut::READOUT_EDGES + lane] as u32;
        let flash = flash_of(ui, node_id, lane, edges, now);
        let glow = if high { 1.0 } else { flash };
        if glow > 0.0 {
            painter.circle_filled(at, (4.0 + 3.0 * glow) * z, green.gamma_multiply(0.25 * glow));
            painter.circle_filled(at, 4.0 * z, green.gamma_multiply(0.35 + 0.65 * glow));
        } else {
            painter.circle(at, 4.0 * z, theme::background::MAIN, Stroke::new(0.8 * z, theme::background::GRID_MAJOR));
        }
        painter.text(
            at,
            egui::Align2::CENTER_CENTER,
            (lane + 1).to_string(),
            tiny.clone(),
            if glow > 0.5 { PANEL } else if live { green } else { theme::text::DISABLED },
        );
    }

    // --- The beat in the bar, at the right of the gates ---
    if has_clock {
        let beats = (values[LinkOut::READOUT_BEATS_PER_BAR] as usize).clamp(1, 8);
        let beat = values[LinkOut::READOUT_BEAT_IN_BAR].max(0.0) as usize;
        let spacing = 7.0 * z;
        let right = rect.right() - 7.0 * z;
        for i in 0..beats {
            let at = Pos2::new(right - (beats - 1 - i) as f32 * spacing, row_y);
            let on = playing && i == beat.min(beats - 1);
            let color = if on {
                if i == 0 { accent } else { theme::text::SECONDARY }
            } else {
                theme::background::GRID_MAJOR
            };
            painter.circle_filled(at, if on { 2.6 * z } else { 1.8 * z }, color);
        }
    }

    response.on_hover_text(match status {
        None => "Link Out sends from the desktop app; in the browser it stays quiet".to_string(),
        Some(LinkStatus::NoListener) => format!(
            "Nothing is listening on port {}. Start Seoul, or pick the port its seoul.toml names",
            PORTS[port]
        ),
        _ => format!(
            "Sending to Seoul on port {}: the CV lanes as cv1 to cv8, the gates as gate1 to gate4, and the Clock",
            PORTS[port]
        ),
    });
}

/// How bright gate `lane`'s flash is, 0 to 1: full when its edge count has
/// moved since the last frame, fading over [`FLASH`].
fn flash_of(ui: &egui::Ui, node_id: NodeId, lane: usize, edges: u32, now: f64) -> f32 {
    let id = Id::new(("link_out_flash", node_id, lane));
    let (seen, at) = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || (edges, f64::NEG_INFINITY)));
    let at = if seen != edges {
        ui.ctx().data_mut(|d| d.insert_temp(id, (edges, now)));
        now
    } else {
        at
    };
    (1.0 - (now - at) / FLASH).clamp(0.0, 1.0) as f32
}
