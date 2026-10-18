//! The LFO's display: its cycle, and where it is in it.
//!
//! The cycle is drawn from the LFO's own shape ([`Lfo::generate_sample`])
//! on an honest axis: bipolar swings about a centre line, 0-1 sits on the
//! floor. A dot rides the curve at the phase the engine reports, trailing a
//! brighter stretch of the curve behind it, so the motion reads even when
//! it's slow. A tick on the floor marks where the Phase knob starts the
//! cycle, and where Sync restarts it.

use eframe::egui::{Pos2, Rect, Stroke};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::lfo::{Lfo, LfoWaveform, SYNC_DIVISIONS};

use super::display_frame::{self as face, columns, curve, guide, header, hertz, playhead, readout, value_of, with_alpha};
use super::drum_display::fill_under;
use super::{SynthGraph, SynthGraphState};

/// How much of the cycle the bright trail behind the dot covers.
const TRAIL: f32 = 0.3;

/// How many steps the trail fades in.
const TRAIL_STEPS: usize = 6;

/// The Waveform dropdown's names, in order.
const WAVES: [&str; 4] = ["Sine", "Triangle", "Square", "Saw"];

/// Draws the display.
pub fn lfo_display(ui: &mut eframe::egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let value = |name: &str, default: f32| value_of(graph, node_id, name).unwrap_or(default);
    let wave_index = value("Waveform", 0.0);
    let wave = LfoWaveform::from_param(wave_index);
    let bipolar = value("Bipolar", 1.0) > 0.5;
    let start = (value("Phase", 0.0) / 360.0).rem_euclid(1.0);
    let division = SYNC_DIVISIONS.get(value("Tempo Sync", 0.0).max(0.0) as usize).filter(|d| d.beats.is_some()).map(|d| d.label);

    // The rate it's running at, once the engine has said; the knob before
    let rate = readout(user_state, node_id)
        .map(|r| r.values[Lfo::READOUT_RUNNING_RATE])
        .filter(|&hz| hz > 0.0)
        .unwrap_or_else(|| value("Rate", 1.0));
    // Where it is: the Phase output, or a guess from the clock until it's
    // heard. Stopped, it waits where it was, or where Phase starts it
    let playing = user_state.is_playing;
    let phase = user_state
        .get_engine_node_id(node_id)
        .and_then(|id| user_state.get_output_value(id, 1))
        .unwrap_or_else(|| if playing { ((ui.ctx().input(|i| i.time) * rate as f64 + start as f64) % 1.0) as f32 } else { start })
        .rem_euclid(1.0);

    let orange = theme::signal::CONTROL;
    let width = 140.0 * z;
    let Some((rect, response, painter)) = face::frame(ui, width, crate::dsp::ModuleCategory::Modulation.color(), z) else { return };
    let plot = face::plot(rect, z);

    // Bipolar swings about the middle; 0-1 stands on the floor
    let level = |p: f32| {
        let v = Lfo::generate_sample(p, wave);
        if bipolar {
            v
        } else {
            Lfo::to_unipolar(v)
        }
    };
    let (low, high) = if bipolar { (-1.0, 1.0) } else { (0.0, 1.0) };
    let y_at = |v: f32| plot.bottom() - (v - low) / (high - low) * plot.height();
    let baseline = if bipolar { y_at(0.0) } else { plot.bottom() };
    guide(&painter, Pos2::new(plot.left(), baseline), Pos2::new(plot.right(), baseline), 18, z);
    for quarter in [0.25, 0.5, 0.75] {
        let x = plot.left() + quarter * plot.width();
        guide(&painter, Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom()), 9, z);
    }

    let points = cycle_points(wave, plot, z, &y_at, &level, 0.0, 1.0);
    fill_under(&painter, &points, baseline, with_alpha(orange, 34), with_alpha(orange, 4));
    curve(&painter, points, orange.gamma_multiply(0.5), 1.2 * z, false);

    // The trail behind the dot, brightening towards it
    for step in 0..TRAIL_STEPS {
        let from = phase - TRAIL * (TRAIL_STEPS - step) as f32 / TRAIL_STEPS as f32;
        let to = phase - TRAIL * (TRAIL_STEPS - step - 1) as f32 / TRAIL_STEPS as f32;
        let bright = (step + 1) as f32 / TRAIL_STEPS as f32;
        // A stretch that runs back past the start wraps round to the end
        for (a, b) in wrapped(from, to) {
            let stretch = cycle_points(wave, plot, z, &y_at, &level, a, b);
            curve(&painter, stretch, orange.gamma_multiply(0.5 + 0.5 * bright), (1.2 + 0.8 * bright) * z, step + 1 == TRAIL_STEPS);
        }
    }

    // Where the Phase knob starts the cycle
    if start > 0.001 {
        let x = plot.left() + start * plot.width();
        painter.line_segment([Pos2::new(x, plot.bottom() - 4.0 * z), Pos2::new(x, plot.bottom())], Stroke::new(1.0 * z, with_alpha(orange, 160)));
    }

    let now = level(phase);
    playhead(&painter, Pos2::new(plot.left() + phase * plot.width(), y_at(now)), with_alpha(orange, 70), z);

    let speed = match division {
        Some(label) => format!("{label} · {}", hertz(rate)),
        None => hertz(rate),
    };
    let range = if bipolar { "±1" } else { "0–1" };
    header(&painter, rect, z, Some((speed, orange.gamma_multiply(0.9))), Some((range.to_string(), theme::text::SECONDARY)));
    if playing {
        crate::app::pace::animate(ui.ctx());
    }

    if response.hovered() {
        let mut text = format!("{} at {}", WAVES[wave_index.clamp(0.0, 3.0) as usize], hertz(rate));
        if let Some(label) = division {
            text.push_str(&format!(", one cycle every {label}"));
        }
        text.push_str(if bipolar { ", swinging from -1 to 1" } else { ", between 0 and 1" });
        if start > 0.001 {
            text.push_str(&format!("\nStarts {:.0}° into the cycle", start * 360.0));
        }
        text.push_str(&format!("\nNow {:.0}% through the cycle, Out {now:.2}", phase * 100.0));
        response.on_hover_text(text);
    }
}

/// The points of the cycle from phase `from` to `to` (0 to 1), with the
/// square's drop drawn straight down.
fn cycle_points(wave: LfoWaveform, plot: Rect, z: f32, y_at: &impl Fn(f32) -> f32, level: &impl Fn(f32) -> f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = ((columns(plot, z) as f32 * (to - from)).ceil() as usize).max(2);
    let x_at = |p: f32| plot.left() + p * plot.width();
    let mut points = Vec::with_capacity(steps + 3);
    for step in 0..=steps {
        let p = from + (to - from) * step as f32 / steps as f32;
        // The square drops halfway
        if wave == LfoWaveform::Square && points.last().is_some_and(|last: &Pos2| last.x < x_at(0.5)) && p >= 0.5 {
            points.push(Pos2::new(x_at(0.5), y_at(level(0.499_99))));
            points.push(Pos2::new(x_at(0.5), y_at(level(0.5))));
        }
        // The end of the cycle is the moment before it wraps
        let at = if p >= 1.0 { 0.999_99 } else { p };
        points.push(Pos2::new(x_at(p), y_at(level(at))));
    }
    points
}

/// A stretch of the cycle from `from` to `to`, which may start before 0,
/// as one or two stretches inside it.
fn wrapped(from: f32, to: f32) -> Vec<(f32, f32)> {
    if from >= 0.0 {
        vec![(from, to)]
    } else if to <= 0.0 {
        vec![(from + 1.0, to + 1.0)]
    } else {
        vec![(from + 1.0, 1.0), (0.0, to)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    #[test]
    fn the_curve_is_the_lfos_own() {
        let plot = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 40.0));
        let y_at = |v: f32| 20.0 - v * 20.0;
        for wave in [LfoWaveform::Sine, LfoWaveform::Triangle, LfoWaveform::Saw] {
            let level = |p: f32| Lfo::generate_sample(p, wave);
            for point in cycle_points(wave, plot, 1.0, &y_at, &level, 0.0, 1.0) {
                let p = (point.x / 100.0).min(0.999_99);
                assert!((point.y - y_at(Lfo::generate_sample(p, wave))).abs() < 1e-3, "{wave:?} at {p}");
            }
        }
    }

    #[test]
    fn the_square_drops_straight_down_halfway() {
        let plot = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 40.0));
        let y_at = |v: f32| 20.0 - v * 20.0;
        let level = |p: f32| Lfo::generate_sample(p, LfoWaveform::Square);
        let points = cycle_points(LfoWaveform::Square, plot, 1.0, &y_at, &level, 0.0, 1.0);
        let drop = points.windows(2).find(|w| (w[0].y - w[1].y).abs() > 39.0).unwrap();
        assert_eq!(drop[0].x, 50.0);
        assert_eq!(drop[1].x, 50.0);
    }

    #[test]
    fn a_trail_past_the_start_wraps_to_the_end() {
        assert_eq!(wrapped(0.2, 0.3), vec![(0.2, 0.3)]);
        assert_eq!(wrapped(-0.1, 0.05), vec![(0.9, 1.0), (0.0, 0.05)]);
        let back = wrapped(-0.3, -0.25);
        assert!((back[0].0 - 0.7).abs() < 1e-6 && (back[0].1 - 0.75).abs() < 1e-6);
    }
}
