//! The Distortion's display: the bend, and the stretch of it the sound
//! reaches.
//!
//! On the left, the transfer curve: what goes in across, from -1 to 1, and
//! what comes out up. It is the module's own shaper for the chosen Type,
//! after Drive (and for Fold, the Symmetry offset), blended by Mix and
//! scaled by Output: a gentle S for Soft, a flat-topped clip for Hard, a
//! zigzag for Fold, a leaning S for Tube and a staircase for Bit. The part
//! of the curve the input reaches right now is lit, hotter the harder it's
//! driven, with a dot at each end, so turning up a quiet input visibly
//! walks it into the bend.
//!
//! On the right, one cycle of a sine at the input's level, faint, and what
//! the curve makes of it over it: the shape the harmonics come from.

use eframe::egui::{self, Color32, Pos2, Rect};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::modules::distortion::DistortionType;
use crate::modules::Distortion;

use super::display_frame::{self as face, columns, curve, guide, header, input_patched, readout, value_of, with_alpha};
use super::{SynthGraph, SynthGraphState};

/// How tall the face is, before zoom: room for the curve's square.
const HEIGHT: f32 = 72.0;

/// The gap between the curve and the waveform, before zoom.
const GAP: f32 = 6.0;

/// The output axis reaches this far either side, so a curve lifted by
/// Output shows its top before it leaves the panel.
const OUT_RANGE: f32 = 1.25;

/// The level of the sine the waveform shows when nothing is coming in.
const RESTING_LEVEL: f32 = 0.7;

/// The colour the lit stretch heats toward as Drive rises.
const HOT: Color32 = Color32::from_rgb(255, 150, 80);

/// The type names, as the Type dropdown gives them.
const TYPE_NAMES: [&str; 5] = ["Soft", "Hard", "Fold", "Bit", "Tube"];

/// The knobs, as the curve needs them.
#[derive(Clone, Copy, Debug)]
struct Shape {
    kind: DistortionType,
    drive: f32,
    symmetry: f32,
    mix: f32,
    output: f32,
}

impl Shape {
    fn at(&self, x: f32) -> f32 {
        Distortion::transfer(self.kind, self.drive, self.symmetry, self.mix, self.output, x)
    }
}

/// Mixes `a` toward `b` by `t`.
fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// "4 kHz", "800 Hz".
fn rate_text(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.0} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

/// Draws the display, as wide as the knob row below it.
pub fn distortion_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let type_index = (value_of(graph, node_id, "Type").unwrap_or(0.0).max(0.0) as usize).min(TYPE_NAMES.len() - 1);
    let knob_drive = value_of(graph, node_id, "Drive").unwrap_or(0.5);
    let rate = value_of(graph, node_id, "Rate").unwrap_or(48_000.0);

    let live = readout(user_state, node_id).filter(|_| user_state.is_playing);
    let drive = live.filter(|_| input_patched(graph, node_id, "Drive")).map_or(knob_drive, |r| r.values[Distortion::READOUT_DRIVE]);
    let level = live.map_or(0.0, |r| r.values[Distortion::READOUT_INPUT_PEAK]);
    let shape = Shape {
        kind: DistortionType::from_index(type_index),
        drive,
        symmetry: value_of(graph, node_id, "Symmetry").unwrap_or(0.0),
        mix: value_of(graph, node_id, "Mix").unwrap_or(1.0),
        output: value_of(graph, node_id, "Output").unwrap_or(0.0),
    };
    let swept = (drive - knob_drive).abs() > 0.01;

    let accent = ModuleCategory::Effect.color();
    let hot = lerp(accent, HOT, drive);
    let width = face::knob_row_width(ui, 3, z);
    let Some((rect, response, painter)) = face::frame_of_height(ui, width, HEIGHT * z, accent, z) else { return };
    let plot = face::plot(rect, z);
    let square = Rect::from_min_size(plot.min, egui::Vec2::splat(plot.height()));
    let wave = Rect::from_min_max(Pos2::new(square.right() + GAP * z, plot.top()), plot.max);

    // --- The curve ---
    let x_at = |x: f32| square.center().x + x.clamp(-1.0, 1.0) * square.width() / 2.0;
    let y_at = |y: f32| square.center().y - y.clamp(-OUT_RANGE, OUT_RANGE) / OUT_RANGE * square.height() / 2.0;
    guide(&painter, Pos2::new(square.left(), square.center().y), Pos2::new(square.right(), square.center().y), 20, z);
    guide(&painter, Pos2::new(square.center().x, square.top()), Pos2::new(square.center().x, square.bottom()), 20, z);
    guide(&painter, Pos2::new(x_at(-1.0), y_at(-1.0)), Pos2::new(x_at(1.0), y_at(1.0)), 14, z);

    // A staircase needs more points than a curve to keep its risers upright
    let steps = columns(square, z) * if shape.kind == DistortionType::Bit { 6 } else { 2 };
    let along = |i: usize| -1.0 + 2.0 * i as f32 / steps as f32;
    let points: Vec<Pos2> = (0..=steps).map(|i| Pos2::new(x_at(along(i)), y_at(shape.at(along(i))))).collect();
    curve(&painter, points.clone(), with_alpha(accent, 150), 1.2 * z, false);

    // The stretch the input reaches, lit, with a dot at each end
    let reach = level.min(1.0);
    if reach > 0.01 {
        let lit: Vec<Pos2> = points.iter().copied().zip((0..=steps).map(along)).filter(|&(_, x)| x.abs() <= reach).map(|(p, _)| p).collect();
        curve(&painter, lit, hot, 2.0 * z, true);
        for x in [-reach, reach] {
            let at = Pos2::new(x_at(x), y_at(shape.at(x)));
            painter.circle_filled(at, 3.4 * z, with_alpha(hot, 70));
            painter.circle_filled(at, 1.7 * z, Color32::WHITE);
        }
    }

    // --- One cycle of a sine at the input's level, and what it becomes ---
    painter.rect_filled(wave, 1.0 * z, with_alpha(Color32::BLACK, 60));
    let amplitude = if reach > 0.01 { reach } else { RESTING_LEVEL };
    let wave_y = |y: f32| wave.center().y - y.clamp(-OUT_RANGE, OUT_RANGE) / OUT_RANGE * (wave.height() / 2.0 - 1.0 * z);
    let samples = columns(wave, z) * 3;
    let phase = |i: usize| i as f32 / samples as f32 * std::f32::consts::TAU;
    let x_of = |i: usize| wave.left() + i as f32 / samples as f32 * wave.width();
    let sine: Vec<Pos2> = (0..=samples).map(|i| Pos2::new(x_of(i), wave_y(amplitude * phase(i).sin()))).collect();
    curve(&painter, sine, Color32::from_white_alpha(40), 0.7 * z, false);
    let shaped: Vec<Pos2> = (0..=samples).map(|i| Pos2::new(x_of(i), wave_y(shape.at(amplitude * phase(i).sin())))).collect();
    let (color, alpha) = if reach > 0.01 { (hot, 230) } else { (accent, 110) };
    curve(&painter, shaped, with_alpha(color, alpha), 1.2 * z, reach > 0.01);

    // --- Words ---
    let right = match shape.kind {
        DistortionType::Bit if rate < 48_000.0 => format!("{:.0} bit · {}", Distortion::bits(drive), rate_text(rate)),
        DistortionType::Bit => format!("{:.0} bit", Distortion::bits(drive)),
        _ => format!("Drive {:.0}%", drive * 100.0),
    };
    header(
        &painter,
        rect,
        z,
        Some((TYPE_NAMES[type_index].to_string(), accent.gamma_multiply(0.9))),
        Some((right, if swept { theme::signal::CONTROL } else { theme::text::SECONDARY })),
    );

    if live.is_some() && reach > 0.01 {
        ui.ctx().request_repaint();
    }

    if response.hovered() {
        let mut text = format!(
            "{}: in across, out up, from -1 to 1",
            match shape.kind {
                DistortionType::Soft => "Soft clipping, a gentle S",
                DistortionType::Hard => "Hard clipping, flat at the top",
                DistortionType::Fold => "Wavefolding: past the top, the wave folds back down",
                DistortionType::Bit => "Bitcrushing: a staircase of levels",
                DistortionType::Tube => "Tube: an S that leans, for even harmonics",
            }
        );
        if reach > 0.01 {
            text.push_str(&format!("\nLit: the stretch the input reaches now (peaks at {:.0}%)", level * 100.0));
        }
        if swept {
            text.push_str(&format!("\nCV has moved the drive from {:.0}%", knob_drive * 100.0));
        }
        text.push_str("\nRight: one cycle of a sine at the input's level, and what the curve makes of it");
        response.on_hover_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_type_names_follow_the_dropdown() {
        for (index, name) in TYPE_NAMES.iter().enumerate() {
            let kind = DistortionType::from_index(index);
            assert_eq!(format!("{kind:?}"), *name);
        }
    }

    #[test]
    fn hard_clipping_flattens_at_full_drive() {
        let shape = Shape { kind: DistortionType::Hard, drive: 1.0, symmetry: 0.0, mix: 1.0, output: 0.0 };
        assert!((shape.at(0.5) - 1.0).abs() < 1e-5);
        assert!((shape.at(-0.5) + 1.0).abs() < 1e-5);
    }

    #[test]
    fn rates_read_plainly() {
        assert_eq!(rate_text(4000.0), "4 kHz");
        assert_eq!(rate_text(800.0), "800 Hz");
    }
}
