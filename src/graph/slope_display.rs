//! The Slope's display: its rise and fall, drawn to proportion.
//!
//! The rise takes the left of the display and the fall the right, each as
//! wide as its share of Rise + Fall, both bent by Shape. A dot rides the
//! curve where the slope is. The rise lights while it's rising and the fall
//! while it's falling, and the glow under the curve swells with Out, so a
//! cycling Slope breathes.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::MAX_READOUT;
use crate::modules::slope::{Curve, Slope, MAX_TIME, MIN_TIME};

use super::drum_display::{duration, fill_under};
use super::{SynthGraph, SynthGraphState, SynthValueType};

/// The narrowest a stage is drawn, as a share of the width, so a 1 ms rise
/// before a 20 s fall is still a line you can see.
const MIN_SHARE: f32 = 0.06;

/// Draws the display, as wide as the knob row below it.
pub fn slope_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let Some(node) = graph.nodes.get(node_id) else { return };
    let value_of = |name: &str| {
        node.inputs.iter().find(|(input, _)| input == name).and_then(|(_, id)| match graph.get_input(*id).value {
            SynthValueType::Number { value, .. } => Some(value),
            SynthValueType::Toggle { value, .. } => Some(if value { 1.0 } else { 0.0 }),
            _ => None,
        })
    };
    let curve = Curve::from_shape(value_of("Shape").unwrap_or(0.0));

    // What the engine says it's doing; before it has said, the knobs
    let readout = user_state.get_engine_node_id(node_id).and_then(|id| user_state.readouts.get(&id)).copied();
    let live = readout.filter(|r| r.values[Slope::READOUT_RISE] > 0.0);
    let (rise, fall) = match live {
        Some(r) => (r.values[Slope::READOUT_RISE], r.values[Slope::READOUT_FALL]),
        None => (
            value_of("Rise").unwrap_or(0.1).clamp(MIN_TIME, MAX_TIME),
            value_of("Fall").unwrap_or(0.3).clamp(MIN_TIME, MAX_TIME),
        ),
    };
    let values = live.map_or([0.0; MAX_READOUT], |r| r.values);
    let charge = values[Slope::READOUT_CHARGE];
    let motion = values[Slope::READOUT_MOTION];
    let eor = values[Slope::READOUT_EOR] > 0.5;
    let level = values[Slope::READOUT_LEVEL];
    let cycling = match live {
        Some(r) => r.values[Slope::READOUT_CYCLING] > 0.5,
        None => value_of("Cycle").unwrap_or(0.0) > 0.5,
    };
    let rising = motion > 0.5;
    let falling = motion < -0.5;

    // As wide as the three knobs below
    let gap = ui.spacing().item_spacing.x;
    let width = 3.0 * 44.0 * z + 2.0 * gap - 8.0 * z;
    let height = 58.0 * z;

    // Separator, as the other displays have, across the display's width
    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let orange = crate::dsp::ModuleCategory::Modulation.color();
    ui.painter().hline(
        rect.x_range(),
        rect.top() - 4.0 * z,
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(orange.r(), orange.g(), orange.b(), 64)),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0 * z, Color32::from_rgb(20, 22, 30));

    let pad = 4.0 * z;
    let plot = Rect::from_min_max(rect.min + Vec2::new(pad, pad + 9.0 * z), rect.max - Vec2::new(pad, pad + 1.0 * z));
    let share = (rise / (rise + fall)).clamp(MIN_SHARE, 1.0 - MIN_SHARE);
    let peak_x = plot.left() + share * plot.width();
    let y_at = |level: f32| plot.bottom() - level.clamp(0.0, 1.0) * plot.height();
    // A point on the rise, or on the fall, at a charge from 0 to 1
    let on_rise = |c: f32| Pos2::new(plot.left() + c * (peak_x - plot.left()), y_at(curve.level(c.into())));
    let on_fall = |c: f32| Pos2::new(peak_x + (1.0 - c) * (plot.right() - peak_x), y_at(curve.level(c.into())));

    let steps = 48;
    let rise_points: Vec<Pos2> = (0..=steps).map(|i| on_rise(i as f32 / steps as f32)).collect();
    let fall_points: Vec<Pos2> = (0..=steps).map(|i| on_fall(1.0 - i as f32 / steps as f32)).collect();

    // The glow swells with Out
    let swell = level.clamp(0.0, 1.0);
    let glow = |alpha: f32| Color32::from_rgba_unmultiplied(orange.r(), orange.g(), orange.b(), alpha.clamp(0.0, 255.0) as u8);
    let (top, bottom) = (glow(34.0 + 90.0 * swell), glow(4.0 + 14.0 * swell));
    fill_under(&painter, &rise_points, plot.bottom(), top, bottom);
    fill_under(&painter, &fall_points, plot.bottom(), top, bottom);

    // Each stage lights while it's running
    let stroke = |lit: bool| if lit { Stroke::new(2.0 * z, orange) } else { Stroke::new(1.2 * z, orange.gamma_multiply(0.5)) };
    painter.add(Shape::line(rise_points, stroke(rising)));
    painter.add(Shape::line(fall_points, stroke(falling)));
    painter.line_segment(
        [Pos2::new(peak_x, plot.top()), Pos2::new(peak_x, plot.bottom())],
        Stroke::new(0.5 * z, Color32::from_rgba_unmultiplied(255, 255, 255, if eor { 40 } else { 14 })),
    );

    // The dot: on the fall while falling, or held at the top; else on the rise
    if live.is_some() {
        let c = charge.clamp(0.0, 1.0);
        let dot = if falling || (!rising && eor) { on_fall(c) } else { on_rise(c) };
        painter.circle_filled(dot, 7.0 * z, glow(40.0 + 50.0 * swell));
        painter.circle_filled(dot, 3.6 * z, Color32::from_rgb(255, 220, 180));
        painter.circle_filled(dot, 1.8 * z, Color32::WHITE);
    }

    // The times, over their stages, and the rate when it cycles
    let small = egui::FontId::proportional(8.0 * z);
    let ink = theme::text::SECONDARY;
    painter.text(rect.left_top() + Vec2::new(4.0 * z, 2.0 * z), egui::Align2::LEFT_TOP, duration(rise), small.clone(), if rising { orange } else { ink });
    painter.text(rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z), egui::Align2::RIGHT_TOP, duration(fall), small.clone(), if falling { orange } else { ink });
    if cycling {
        painter.text(
            Pos2::new(rect.center().x, rect.top() + 2.0 * z),
            egui::Align2::CENTER_TOP,
            hertz(1.0 / (rise + fall)),
            small,
            orange.gamma_multiply(0.85),
        );
    }

    if response.hovered() {
        let doing = if rising {
            "Rising"
        } else if falling {
            "Falling"
        } else if eor {
            "Holding at the top"
        } else {
            "At rest"
        };
        let mut text = format!("Rise {}, fall {}", duration(rise), duration(fall));
        if cycling {
            text.push_str(&format!(", cycling at {}", hertz(1.0 / (rise + fall))));
        }
        if live.is_some() {
            text.push_str(&format!("\n{doing}, Out {level:.3}"));
        }
        response.on_hover_text(text);
    }

    // Moving, it animates; at rest, the next readout repaints it
    if rising || falling || cycling {
        crate::app::pace::animate(ui.ctx());
    }
}

/// A rate, to three figures.
fn hertz(hz: f32) -> String {
    if hz >= 10.0 {
        format!("{hz:.0} Hz")
    } else if hz >= 1.0 {
        format!("{hz:.1} Hz")
    } else {
        format!("{hz:.2} Hz")
    }
}
