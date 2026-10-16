//! The Audio Input node's display: what it's listening to, and how loud.
//!
//! Two slim meters show the input's left and right peaks after Gain. Beside
//! them, the last few seconds of Follow scroll past on the same dB scale as
//! the Threshold, which lies across them as a line. Wherever the gate was
//! open the trace turns the gate's green, so you can see which hits got
//! through. Drag the line to move the Threshold.

use eframe::egui::{self, Color32, CursorIcon, Mesh, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::audio_input::{follow_from_db, FOLLOW_FLOOR_DB};
use crate::widgets::{column_meter, PeakBallistics};

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Seconds of Follow the trace shows.
const TRACE_SECONDS: f32 = 3.0;

/// Engine output ports of Follow and Gate.
const FOLLOW_PORT: usize = 2;
const GATE_PORT: usize = 3;

/// Draws the display and returns the Threshold to set, while its line is
/// being dragged: `("Threshold", dB)`.
pub fn input_display(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    zoom: f32,
) -> Option<(String, f32)> {
    let z = zoom;
    let engine_node_id = user_state.get_engine_node_id(node_id);
    let node = graph.nodes.get(node_id)?;
    let threshold_db = node
        .inputs
        .iter()
        .find(|(name, _)| name == "Threshold")
        .and_then(|(_, id)| match graph.get_input(*id).value {
            SynthValueType::Number { value, .. } => Some(value),
            _ => None,
        })
        .unwrap_or(-30.0);

    // As wide as the knob row below: four 44-pt columns
    let gap = ui.spacing().item_spacing.x;
    let width = 4.0 * 44.0 * z + 3.0 * gap - 8.0 * z;
    let height = 54.0 * z;

    // Separator, as the other displays have
    ui.add_space(4.0 * z);
    let accent = crate::dsp::ModuleCategory::Source.color();
    let full = ui.available_rect_before_wrap();
    ui.painter().hline(
        (full.left() + 4.0 * z)..=(full.right() - 4.0 * z),
        ui.cursor().top(),
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64)),
    );
    ui.add_space(6.0 * z);

    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let painter = ui.painter_at(rect.expand(2.0 * z));
    let small = egui::FontId::proportional(8.0 * z);

    // --- The meters: L and R, labelled underneath ---
    let silent = PeakBallistics::default();
    let meters = engine_node_id.and_then(|eid| user_state.module_meters.get(&eid));
    let meter = |index: usize| meters.map_or(&silent, |m| m.get(index));
    let label_height = 10.0 * z;
    let meter_bottom = rect.bottom() - label_height;
    for (side, label) in ["L", "R"].into_iter().enumerate() {
        let x = rect.left() + 4.0 * z + side as f32 * 8.0 * z;
        let bar = Rect::from_x_y_ranges(x - 2.5 * z..=x + 2.5 * z, rect.top()..=meter_bottom);
        column_meter(&painter, bar, meter(side), false);
        painter.text(
            Pos2::new(x, meter_bottom + label_height / 2.0 + 1.0 * z),
            egui::Align2::CENTER_CENTER,
            label,
            small.clone(),
            theme::text::QUIET,
        );
    }
    let meter_area = Rect::from_x_y_ranges(rect.left()..=rect.left() + 14.0 * z, rect.top()..=meter_bottom);
    ui.interact(meter_area, ui.id().with(("input_meters", node_id)), Sense::hover()).on_hover_text(format!(
        "Input after Gain: L {} / R {}",
        format_db(meter(0).level_db()),
        format_db(meter(1).level_db()),
    ));

    // --- The trace ---
    let panel = Rect::from_x_y_ranges(rect.left() + 20.0 * z..=rect.right(), rect.top()..=rect.bottom());
    painter.rect(panel, 4.0 * z, theme::background::MAIN, Stroke::new(1.0 * z, theme::background::GRID_MAJOR));
    let inner = panel.shrink2(Vec2::new(3.0 * z, 4.0 * z));
    let y_at = |follow: f32| inner.bottom() - inner.height() * follow.clamp(0.0, 1.0);

    // Faint lines every 12 dB
    for db in [-12.0, -24.0, -36.0, -48.0] {
        let y = y_at(follow_from_db(db));
        painter.hline(inner.x_range(), y, Stroke::new(1.0 * z, theme::background::GRID));
    }

    let listening = user_state.audio_input_name.as_deref();
    let follow = engine_node_id.and_then(|eid| user_state.signal_history.get(&(eid, FOLLOW_PORT))).and_then(|h| h.trace(0));
    let gate = engine_node_id.and_then(|eid| user_state.signal_history.get(&(eid, GATE_PORT))).and_then(|h| h.trace(0));
    let gate_now = gate.as_ref().and_then(|g| g.at(0.0)).is_some_and(|g| g > 0.5);

    if let Some(follow) = follow.as_ref() {
        // Newest at the right edge, one point every 1.5 pt going back
        let steps = (inner.width() / (1.5 * z)).max(2.0) as usize;
        let mut points: Vec<Option<(Pos2, bool)>> = Vec::with_capacity(steps + 1);
        for step in 0..=steps {
            let along = step as f32 / steps as f32;
            let age = along * TRACE_SECONDS;
            let x = inner.right() - along * inner.width();
            let open = gate.as_ref().and_then(|g| g.at(age)).is_some_and(|g| g > 0.5);
            points.push(follow.at(age).map(|level| (Pos2::new(x, y_at(level)), open)));
        }
        let control = theme::signal::CONTROL;
        let green = theme::signal::GATE;
        // The area under the trace, as one mesh so neighbouring slices don't
        // show seams, fading toward the floor
        let mut area = Mesh::default();
        for pair in points.windows(2) {
            let (Some((a, open_a)), Some((b, open_b))) = (pair[0], pair[1]) else { continue };
            let hue = if open_a && open_b { green } else { control };
            let top = hue.gamma_multiply(if open_a && open_b { 0.38 } else { 0.22 });
            let bottom = hue.gamma_multiply(0.04);
            let base = area.vertices.len() as u32;
            area.colored_vertex(a, top);
            area.colored_vertex(b, top);
            area.colored_vertex(Pos2::new(b.x, inner.bottom()), bottom);
            area.colored_vertex(Pos2::new(a.x, inner.bottom()), bottom);
            area.add_triangle(base, base + 1, base + 2);
            area.add_triangle(base, base + 2, base + 3);
        }
        painter.add(Shape::mesh(area));
        // Then its edge
        for pair in points.windows(2) {
            let (Some((a, open_a)), Some((b, open_b))) = (pair[0], pair[1]) else { continue };
            let hue = if open_a && open_b { green } else { control };
            painter.line_segment([a, b], Stroke::new(1.3 * z, hue));
        }
    }

    // What it's listening to, or how to start it
    match listening {
        Some(name) => {
            painter.text(
                Pos2::new(inner.left() + 2.0 * z, inner.top() + 1.0 * z),
                egui::Align2::LEFT_TOP,
                truncate(name, 26),
                small.clone(),
                theme::text::QUIET,
            );
        }
        None => {
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "No input: choose one under Input",
                small.clone(),
                theme::text::QUIET,
            );
        }
    }

    // --- The Threshold line, draggable ---
    let line_y = y_at(follow_from_db(threshold_db));
    let grab = Rect::from_x_y_ranges(inner.x_range(), line_y - 5.0 * z..=line_y + 5.0 * z);
    let response = ui.interact(grab, ui.id().with(("input_threshold", node_id)), Sense::drag());
    let hot = response.hovered() || response.dragged();
    let green = theme::signal::GATE;
    let line_color = if hot { green } else { green.gamma_multiply(0.7) };
    let dash = 4.0 * z;
    let mut x = inner.left();
    while x < inner.right() {
        let end = (x + dash).min(inner.right());
        painter.line_segment([Pos2::new(x, line_y), Pos2::new(end, line_y)], Stroke::new(if hot { 1.6 * z } else { 1.0 * z }, line_color));
        x += dash * 1.8;
    }
    let label = format!("{:.0} dB", threshold_db);
    let label_above = line_y - inner.top() > 12.0 * z;
    let label_pos = if label_above {
        Pos2::new(inner.right() - 10.0 * z, line_y - 1.5 * z)
    } else {
        Pos2::new(inner.right() - 10.0 * z, line_y + 1.5 * z)
    };
    let anchor = if label_above { egui::Align2::RIGHT_BOTTOM } else { egui::Align2::RIGHT_TOP };
    painter.text(label_pos, anchor, label, small.clone(), line_color);

    // The gate's lamp, at the end of the line
    let lamp = Pos2::new(inner.right() - 3.5 * z, line_y);
    if gate_now {
        painter.circle_filled(lamp, 5.0 * z, green.gamma_multiply(0.25));
        painter.circle_filled(lamp, 3.0 * z, green);
    } else {
        painter.circle(lamp, 2.5 * z, theme::background::MAIN, Stroke::new(1.0 * z, green.gamma_multiply(0.6)));
    }

    let mut changed = None;
    if response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let follow = ((inner.bottom() - pointer.y) / inner.height()).clamp(0.0, 1.0);
            let db = FOLLOW_FLOOR_DB * (1.0 - follow);
            changed = Some(("Threshold".to_string(), (db * 2.0).round() / 2.0));
        }
    }
    if hot {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    response.on_hover_text("Threshold: the gate opens when Follow rises above this line. Drag to move it");

    ui.add_space(2.0 * z);
    changed
}

/// A meter level for a tooltip.
fn format_db(db: f32) -> String {
    if db <= -48.0 {
        "-inf dBFS".to_string()
    } else {
        format!("{db:+.1} dBFS")
    }
}

/// `text`, cut to `chars` characters with an ellipsis if longer.
fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(chars - 3).collect::<String>())
    }
}
