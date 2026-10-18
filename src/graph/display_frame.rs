//! The frame the Oscillator, LFO, ADSR and filter faces are drawn in: the
//! same one the Drum and Slope draw for themselves.
//!
//! A separator in the module's colour, a dark panel as wide as the knob row,
//! and a painter clipped to it, so a glow never spills past the edge. Inside,
//! a plot inset from the edges with a strip along the top for a word or two.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::{ConnectionSignalTrait, NodeId, SignalTrace};

use super::{SynthGraph, SynthGraphState, SynthValueType};
use crate::dsp::Readout;

/// The panel behind every face.
pub const PANEL: Color32 = Color32::from_rgb(20, 22, 30);

/// How tall a face is, before zoom.
pub const HEIGHT: f32 = 56.0;

/// Room along the top for the header text, before zoom.
pub const HEADER: f32 = 8.0;

/// As wide as a row of `knobs` knobs below it.
pub fn knob_row_width(ui: &egui::Ui, knobs: usize, z: f32) -> f32 {
    let gap = ui.spacing().item_spacing.x;
    let n = knobs.max(1) as f32;
    n * 44.0 * z + (n - 1.0) * gap - 8.0 * z
}

/// A face: the separator above it, its panel, and a painter clipped to it.
/// `None` when it's scrolled out of view.
pub fn frame(ui: &mut egui::Ui, width: f32, accent: Color32, z: f32) -> Option<(Rect, Response, Painter)> {
    frame_of_height(ui, width, HEIGHT * z, accent, z)
}

/// A face [`frame`] `height` tall (zoomed), for one that needs more room.
pub fn frame_of_height(ui: &mut egui::Ui, width: f32, height: f32, accent: Color32, z: f32) -> Option<(Rect, Response, Painter)> {
    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    ui.painter().hline(rect.x_range(), rect.top() - 4.0 * z, Stroke::new(1.0 * z, with_alpha(accent, 64)));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0 * z, PANEL);
    Some((rect, response, painter))
}

/// The plot inside a face: inset from the edges, below the header.
pub fn plot(rect: Rect, z: f32) -> Rect {
    let pad = 4.0 * z;
    Rect::from_min_max(rect.min + Vec2::new(pad, pad + HEADER * z), rect.max - Vec2::new(pad, pad))
}

/// How many points to draw across `plot`: about one every 1.5 px, so a
/// curve stays smooth however far in it's zoomed.
pub fn columns(plot: Rect, z: f32) -> usize {
    (plot.width() / (1.5 * z.max(0.1))).max(48.0) as usize
}

/// Words in the header strip: `left` at the left edge, `right` at the right.
pub fn header(painter: &Painter, rect: Rect, z: f32, left: Option<(String, Color32)>, right: Option<(String, Color32)>) {
    let font = egui::FontId::proportional(8.0 * z);
    if let Some((text, color)) = left {
        painter.text(rect.left_top() + Vec2::new(4.0 * z, 2.0 * z), egui::Align2::LEFT_TOP, text, font.clone(), color);
    }
    if let Some((text, color)) = right {
        painter.text(rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z), egui::Align2::RIGHT_TOP, text, font, color);
    }
}

/// A faint guide line across or down the plot.
pub fn guide(painter: &Painter, from: Pos2, to: Pos2, alpha: u8, z: f32) {
    painter.line_segment([from, to], Stroke::new(0.5 * z, Color32::from_rgba_unmultiplied(255, 255, 255, alpha)));
}

/// A curve drawn as one path, with a soft glow under it when `glow` is set.
pub fn curve(painter: &Painter, points: Vec<Pos2>, color: Color32, width: f32, glow: bool) {
    if points.len() < 2 {
        return;
    }
    if glow {
        painter.add(Shape::line(points.clone(), Stroke::new(width * 3.0, color.gamma_multiply(0.18))));
    }
    painter.add(Shape::line(points, Stroke::new(width, color)));
}

/// The dot that rides a curve where the module is now.
pub fn playhead(painter: &Painter, at: Pos2, glow: Color32, z: f32) {
    painter.circle_filled(at, 7.0 * z, glow);
    painter.circle_filled(at, 3.6 * z, Color32::from_rgb(255, 220, 180));
    painter.circle_filled(at, 1.8 * z, Color32::WHITE);
}

/// A knob, toggle or dropdown's value on a node, by parameter name.
pub fn value_of(graph: &SynthGraph, node_id: NodeId, name: &str) -> Option<f32> {
    let node = graph.nodes.get(node_id)?;
    let (_, id) = node.inputs.iter().find(|(input, _)| input == name)?;
    match graph.get_input(*id).value {
        SynthValueType::Number { value, .. } => Some(value),
        SynthValueType::Select { value, .. } => Some(value as f32),
        SynthValueType::Toggle { value, .. } => Some(if value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Whether a cable runs into the node's input `name`.
pub fn input_patched(graph: &SynthGraph, node_id: NodeId, name: &str) -> bool {
    let Some(node) = graph.nodes.get(node_id) else { return false };
    node.inputs
        .iter()
        .find(|(input, _)| input == name)
        .is_some_and(|(_, id)| super::patched::input(graph, *id))
}

/// Whether a cable runs out of the node's output `name`.
pub fn output_patched(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId, name: &str) -> bool {
    let Some(node) = graph.nodes.get(node_id) else { return false };
    node.outputs
        .iter()
        .find(|(output, _)| output == name)
        .is_some_and(|(_, id)| user_state.cables_from.patched(*id))
}

/// The recent past of whatever is patched into the node's input `name`
/// (its first channel), if anything is.
pub fn incoming_trace<'a>(graph: &SynthGraph, node_id: NodeId, user_state: &'a SynthGraphState, name: &str) -> Option<SignalTrace<'a>> {
    let node = graph.nodes.get(node_id)?;
    let (_, input) = node.inputs.iter().find(|(input, _)| input == name)?;
    let output = graph.connection(*input)?;
    let index = graph.get_output_index(output)?;
    user_state.output_trace(graph.get_output(output).node, index, 0)
}

/// The node's latest readout from the engine, if it has sent one.
pub fn readout(user_state: &SynthGraphState, node_id: NodeId) -> Option<Readout> {
    user_state.get_engine_node_id(node_id).and_then(|id| user_state.readouts.get(&id)).copied()
}

/// `color` at `alpha` (0-255).
pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// A frequency to read at a glance: "82 Hz", "1.2 kHz", "12 kHz".
pub fn hertz(hz: f32) -> String {
    if hz >= 10_000.0 {
        format!("{:.0} kHz", hz / 1000.0)
    } else if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else if hz >= 10.0 {
        format!("{hz:.0} Hz")
    } else if hz >= 1.0 {
        format!("{hz:.1} Hz")
    } else {
        format!("{hz:.2} Hz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hertz_reads_at_a_glance() {
        assert_eq!(hertz(82.4), "82 Hz");
        assert_eq!(hertz(1234.0), "1.2 kHz");
        assert_eq!(hertz(12_000.0), "12 kHz");
        assert_eq!(hertz(2.0), "2.0 Hz");
        assert_eq!(hertz(0.25), "0.25 Hz");
    }

    #[test]
    fn columns_follow_the_zoom() {
        let small = Rect::from_min_size(Pos2::ZERO, Vec2::new(140.0, 40.0));
        let big = Rect::from_min_size(Pos2::ZERO, Vec2::new(280.0, 80.0));
        assert!(columns(big, 1.0) > columns(small, 1.0));
        assert_eq!(columns(big, 2.0), columns(small, 1.0));
    }
}
