//! The Logic node's display: its inputs, its answers, and its comparator.
//!
//! - **A** and **B** are two small lamps at the left, lit while each is
//!   high. With nothing patched into B it reads Above, and its lamp says so.
//! - **The answers** are four lamps, lit while AND, OR, XOR and NOT A are
//!   high, so the truth table plays out in front of you.
//! - **The comparator** is a column the CV rises in, with Threshold drawn
//!   across it as a line you can drag. The CV turns green when it's above.

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::logic::Logic;

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Engine output ports of the four logic outputs, with their lamp labels.
const LAMPS: [(usize, &str); 4] = [(0, "AND"), (1, "OR"), (2, "XOR"), (3, "NOT A")];

/// Draws the display and returns the Threshold to set while its line is
/// being dragged: `("Threshold", value)`.
pub fn logic_display(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    zoom: f32,
) -> Option<(String, f32)> {
    let z = zoom;
    let node = graph.nodes.get(node_id)?;
    let input_of = |name: &str| node.inputs.iter().find(|(input, _)| input == name).map(|(_, id)| *id);
    let threshold = input_of("Threshold")
        .and_then(|id| match graph.get_input(id).value {
            SynthValueType::Number { value, .. } => Some(value),
            _ => None,
        })
        .unwrap_or(0.5);
    let b_patched = input_of("B").is_some_and(|id| super::patched::input(graph, id));

    let engine_node_id = user_state.get_engine_node_id(node_id);
    let readout = engine_node_id.and_then(|id| user_state.readouts.get(&id)).copied().unwrap_or_default();
    let cv = readout.values[Logic::READOUT_CV];
    let above = readout.values[Logic::READOUT_ABOVE] > 0.5;
    let a = readout.values[Logic::READOUT_A] > 0.5;
    let b = readout.values[Logic::READOUT_B] > 0.5;

    let width = 136.0 * z;
    let height = 52.0 * z;

    // Separator, as the other displays have
    ui.add_space(4.0 * z);
    let accent = crate::dsp::ModuleCategory::Utility.color();
    let left = ui.cursor().left();
    ui.painter().hline(
        left..=(left + width),
        ui.cursor().top(),
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64)),
    );
    ui.add_space(6.0 * z);

    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let painter = ui.painter_at(rect.expand(2.0 * z));
    let green = theme::signal::GATE;
    let orange = theme::signal::CONTROL;
    let small = egui::FontId::proportional(8.0 * z);

    // --- A and B, the inputs ---
    let inputs_x = rect.left() + 6.0 * z;
    for (row, (label, lit)) in [("A", a), ("B", b)].into_iter().enumerate() {
        let y = rect.top() + height * (0.28 + 0.44 * row as f32);
        let lamp = Pos2::new(inputs_x, y);
        if lit {
            painter.circle_filled(lamp, 5.5 * z, green.gamma_multiply(0.25));
            painter.circle_filled(lamp, 3.5 * z, green);
        } else {
            painter.circle(lamp, 3.5 * z, theme::background::MAIN, Stroke::new(1.0 * z, theme::background::GRID_MAJOR));
        }
        painter.text(lamp + Vec2::new(7.0 * z, 0.0), egui::Align2::LEFT_CENTER, label, small.clone(), theme::text::SECONDARY);
    }
    // An empty B reads Above, and says so under its lamp
    let b_lamp = Pos2::new(inputs_x, rect.top() + height * 0.72);
    let b_rect = Rect::from_center_size(b_lamp + Vec2::new(4.0 * z, 0.0), Vec2::new(18.0 * z, 12.0 * z));
    let b_hover = ui.interact(b_rect, ui.id().with(("logic_b", node_id)), Sense::hover());
    if !b_patched {
        painter.text(
            b_lamp + Vec2::new(0.0, 9.0 * z),
            egui::Align2::LEFT_TOP,
            "= Above",
            egui::FontId::proportional(7.0 * z),
            theme::text::QUIET,
        );
        b_hover.on_hover_text("Nothing is patched into B, so B is Above: AND passes A only while CV is over the Threshold");
    }

    // --- The column, at the right: CV against Threshold ---
    let column = Rect::from_x_y_ranges(rect.right() - 12.0 * z..=rect.right() - 2.0 * z, rect.top() + 1.0 * z..=rect.bottom() - 10.0 * z);
    let y_at = |value: f32| column.bottom() - column.height() * ((value + 1.0) / 2.0).clamp(0.0, 1.0);
    painter.rect(column, 3.0 * z, theme::background::MAIN, Stroke::new(1.0 * z, theme::background::GRID_MAJOR));
    let level = Rect::from_x_y_ranges(column.shrink(2.0 * z).x_range(), y_at(cv)..=column.bottom() - 2.0 * z);
    if level.height() > 0.0 {
        painter.rect_filled(level, 2.0 * z, if above { green } else { orange.gamma_multiply(0.6) });
    }
    painter.text(Pos2::new(column.center().x, rect.bottom() + 1.0 * z), egui::Align2::CENTER_BOTTOM, "CV", small.clone(), theme::text::QUIET);

    // The Threshold, a line across the column, draggable
    let line_y = y_at(threshold);
    let grab = Rect::from_x_y_ranges(column.left() - 4.0 * z..=column.right() + 2.0 * z, line_y - 4.0 * z..=line_y + 4.0 * z);
    let response = ui
        .interact(grab, ui.id().with(("logic_threshold", node_id)), Sense::drag())
        .on_hover_cursor(CursorIcon::ResizeVertical);
    let hot = response.hovered() || response.dragged();
    let line_ink = if hot { green } else { green.gamma_multiply(0.75) };
    painter.hline(column.left() - 3.0 * z..=column.right() + 1.0 * z, line_y, Stroke::new(if hot { 1.8 * z } else { 1.2 * z }, line_ink));
    let dragged = response
        .dragged()
        .then(|| response.interact_pointer_pos())
        .flatten()
        .map(|pointer| ((column.bottom() - pointer.y) / column.height() * 2.0 - 1.0).clamp(-1.0, 1.0))
        .map(|value| ("Threshold".to_string(), (value * 100.0).round() / 100.0));
    response.on_hover_text(format!(
        "CV {cv:.2}, Threshold {threshold:.2}: Above is {}\nDrag the line to move the Threshold",
        if above { "high" } else { "low" }
    ));

    // --- The answers, between: AND, OR, XOR, NOT A ---
    let lamps = Rect::from_x_y_ranges(rect.left() + 34.0 * z..=column.left() - 10.0 * z, rect.top() + 2.0 * z..=rect.bottom() - 2.0 * z);
    let lamp_gap = 4.0 * z;
    let lamp_size = Vec2::new((lamps.width() - lamp_gap) / 2.0, (lamps.height() - lamp_gap) / 2.0);
    for (i, (port, label)) in LAMPS.into_iter().enumerate() {
        let min = lamps.left_top() + Vec2::new((i % 2) as f32 * (lamp_size.x + lamp_gap), (i / 2) as f32 * (lamp_size.y + lamp_gap));
        let lamp = Rect::from_min_size(min, lamp_size);
        let lit = engine_node_id.and_then(|id| user_state.get_output_value(id, port)).is_some_and(|v| v > 0.5);
        if lit {
            painter.rect(lamp, 4.0 * z, green.gamma_multiply(0.8), Stroke::new(1.0 * z, green));
            painter.text(lamp.center(), egui::Align2::CENTER_CENTER, label, small.clone(), Color32::WHITE);
        } else {
            painter.rect(lamp, 4.0 * z, theme::background::MAIN, Stroke::new(1.0 * z, theme::background::GRID_MAJOR));
            painter.text(lamp.center(), egui::Align2::CENTER_CENTER, label, small.clone(), theme::text::DISABLED);
        }
    }

    dragged
}
