//! The Compressor's display: the curve it presses the sound against, and
//! the last two seconds of it pressing.
//!
//! On the left, the transfer curve: the level the detector hears across,
//! how loud the output comes out up, both from -60 to 0 dB. It's the
//! module's own gain computer (and its makeup and dry blend), so the knee
//! bends where the knob says. A dot rides it at the level the detector
//! hears now, placed by the reduction actually applied: while the attack
//! is still clamping down or the release letting go, the dot sits off the
//! curve, and a short afterglow shows the path it took.
//!
//! On the right, time runs to the left from now. The sound coming in is a
//! faint blue landscape, what goes out a line over it, and the gain
//! reduction hangs from the top in orange, so pumping shows as a rhythm.

use std::collections::VecDeque;

use eframe::egui::{self, Color32, Id, Pos2, Rect};
use egui_node_graph2::{ConnectionSignalTrait, NodeId, SignalTrace};

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::modules::Compressor;

use super::display_frame::{self as face, columns, curve, guide, header, incoming_trace, input_patched, playhead, readout, value_of, with_alpha};
use super::drum_display::fill_under;
use super::{SynthGraph, SynthGraphState};

/// The level axis of both halves, in dB.
const FLOOR_DB: f32 = -60.0;
const CEILING_DB: f32 = 0.0;

/// How tall the face is, before zoom: taller than the standard face, so
/// the curve's square has room for its knee.
const HEIGHT: f32 = 72.0;

/// The gap between the curve and the strip, before zoom.
const GAP: f32 = 8.0;

/// How far back the strip looks, in seconds.
const STRIP_SECONDS: f32 = 2.0;

/// The strip's level axis reaches down this far, in dB: the top of the
/// range, where compression happens, gets the room.
const STRIP_FLOOR_DB: f32 = -36.0;

/// Gain reduction that reaches the strip's floor, in dB.
const STRIP_GR_DB: f32 = 24.0;

/// How long the dot's afterglow lasts, in seconds.
const AFTERGLOW: f64 = 0.35;

/// The GR output carries the reduction as `dB / 60`.
const GR_PER_DB: f32 = 1.0 / 60.0;

/// The output ports, in order.
const OUT_PORT: usize = 0;
const GR_PORT: usize = 1;

/// Where the dot has been lately: (when, level heard, level out), in dB.
#[derive(Clone, Default)]
struct Trail(VecDeque<(f64, f32, f32)>);

/// The knobs' settings, as the curve needs them.
#[derive(Clone, Copy, Debug)]
struct Settings {
    threshold: f32,
    ratio: f32,
    knee: f32,
    makeup: f32,
    mix: f32,
}

impl Settings {
    /// How loud the output is for a steady `level_db` heard, in dB.
    fn output_db(&self, level_db: f32) -> f32 {
        let reduction = Compressor::compute_gain_reduction(level_db, self.threshold, self.ratio.max(1.0), self.knee.max(0.0));
        level_db + Compressor::output_gain_db(reduction, self.makeup, self.mix)
    }

    /// Where the output lands for `level_db` heard while `reduction_db` is applied.
    fn output_with(&self, level_db: f32, reduction_db: f32) -> f32 {
        level_db + Compressor::output_gain_db(reduction_db, self.makeup, self.mix)
    }
}

/// "4:1", "2.5:1", "20:1".
fn ratio_text(ratio: f32) -> String {
    if (ratio - ratio.round()).abs() < 0.05 {
        format!("{:.0}:1", ratio)
    } else {
        format!("{:.1}:1", ratio)
    }
}

/// A linear level, in dB, held at the floor below it.
fn to_db(level: f32) -> f32 {
    if level > 0.0 {
        (20.0 * level.log10()).max(FLOOR_DB)
    } else {
        FLOOR_DB
    }
}

/// Draws the display, as wide as the knob row below it.
pub fn compressor_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let settings = Settings {
        threshold: value_of(graph, node_id, "Threshold").unwrap_or(-20.0),
        ratio: value_of(graph, node_id, "Ratio").unwrap_or(4.0),
        knee: value_of(graph, node_id, "Knee").unwrap_or(6.0),
        makeup: value_of(graph, node_id, "Makeup").unwrap_or(0.0),
        mix: value_of(graph, node_id, "Mix").unwrap_or(1.0),
    };
    let rms = value_of(graph, node_id, "Detector").unwrap_or(0.0) == Compressor::DETECTOR_RMS;
    let keyed = input_patched(graph, node_id, "Sidechain");

    let accent = ModuleCategory::Effect.color();
    let orange = theme::signal::CONTROL;
    let blue = theme::signal::AUDIO;

    let width = face::knob_row_width(ui, 7, z);
    let Some((rect, response, painter)) = face::frame_of_height(ui, width, HEIGHT * z, accent, z) else { return };
    let plot = face::plot(rect, z);
    let square = Rect::from_min_size(plot.min, egui::Vec2::splat(plot.height()));
    let strip = Rect::from_min_max(Pos2::new(square.right() + GAP * z, plot.top()), plot.max);

    // --- The curve ---
    let along = |db: f32| (db.clamp(FLOOR_DB, CEILING_DB) - FLOOR_DB) / (CEILING_DB - FLOOR_DB);
    let x_at = |db: f32| square.left() + along(db) * square.width();
    let y_at = |db: f32| square.bottom() - along(db) * square.height();

    for db in [-12.0, -24.0, -36.0, -48.0] {
        guide(&painter, Pos2::new(x_at(db), square.top()), Pos2::new(x_at(db), square.bottom()), 9, z);
        guide(&painter, Pos2::new(square.left(), y_at(db)), Pos2::new(square.right(), y_at(db)), 9, z);
    }
    // Unity: where the sound would sit untouched
    guide(&painter, Pos2::new(x_at(FLOOR_DB), y_at(FLOOR_DB)), Pos2::new(x_at(CEILING_DB), y_at(CEILING_DB)), 26, z);

    // The knee, a soft band either side of the threshold, and the threshold itself
    if settings.knee > 0.0 {
        let half = settings.knee / 2.0;
        let band = Rect::from_x_y_ranges(x_at(settings.threshold - half)..=x_at(settings.threshold + half), square.y_range());
        painter.rect_filled(band, 0.0, with_alpha(accent, 12));
    }
    let threshold_x = x_at(settings.threshold);
    painter.line_segment(
        [Pos2::new(threshold_x, square.top()), Pos2::new(threshold_x, square.bottom())],
        egui::Stroke::new(0.6 * z, with_alpha(accent, 56)),
    );

    let columns = columns(square, z);
    let points: Vec<Pos2> = (0..=columns)
        .map(|i| {
            let level = FLOOR_DB + (CEILING_DB - FLOOR_DB) * i as f32 / columns as f32;
            Pos2::new(x_at(level), y_at(settings.output_db(level)))
        })
        .collect();
    fill_under(&painter, &points, square.bottom(), with_alpha(accent, 44), with_alpha(accent, 4));
    curve(&painter, points, accent, 1.4 * z, true);

    // The dot: what the detector hears now, and where the applied reduction puts it
    let live = readout(user_state, node_id).filter(|_| user_state.is_playing);
    let heard = live.map_or(FLOOR_DB, |r| r.values[Compressor::READOUT_DETECTOR_DB]);
    let reduction = live.map_or(0.0, |r| r.values[Compressor::READOUT_GR_DB].max(0.0));
    let now = ui.input(|input| input.time);
    let trail_id = Id::new(("compressor_dot", node_id));
    let mut trail = ui.ctx().data_mut(|data| data.get_temp::<Trail>(trail_id)).unwrap_or_default();
    while trail.0.front().is_some_and(|&(when, _, _)| now - when > AFTERGLOW) {
        trail.0.pop_front();
    }
    let sounding = heard > FLOOR_DB + 0.5;
    if sounding {
        trail.0.push_back((now, heard, settings.output_with(heard, reduction)));
    }
    let squeezing = reduction >= 0.1;
    let dot_glow = if squeezing { orange } else { accent };
    for pair in trail.0.iter().collect::<Vec<_>>().windows(2) {
        let (&(when, a_in, a_out), &(_, b_in, b_out)) = (pair[0], pair[1]);
        let fade = 1.0 - ((now - when) / AFTERGLOW) as f32;
        painter.line_segment(
            [Pos2::new(x_at(a_in), y_at(a_out)), Pos2::new(x_at(b_in), y_at(b_out))],
            egui::Stroke::new(1.6 * z * fade.max(0.3), with_alpha(dot_glow, (fade.clamp(0.0, 1.0) * 120.0) as u8)),
        );
    }
    if sounding {
        let at = Pos2::new(x_at(heard), y_at(settings.output_with(heard, reduction)));
        playhead(&painter, at, with_alpha(dot_glow, 70), z);
    }
    let glowing = !trail.0.is_empty();
    ui.ctx().data_mut(|data| data.insert_temp(trail_id, trail));

    // --- The last two seconds ---
    painter.rect_filled(strip, 1.0 * z, with_alpha(Color32::BLACK, 60));
    let level_y = |db: f32| {
        let along = (db.clamp(STRIP_FLOOR_DB, CEILING_DB) - STRIP_FLOOR_DB) / (CEILING_DB - STRIP_FLOOR_DB);
        strip.bottom() - along * strip.height()
    };
    let threshold_y = level_y(settings.threshold);
    guide(&painter, Pos2::new(strip.left(), threshold_y), Pos2::new(strip.right(), threshold_y), 34, z);

    let steps = (strip.width() / (1.5 * z)).max(2.0) as usize;
    let x_ago = |step: usize| strip.right() - step as f32 / steps as f32 * strip.width();
    let age = |step: usize| step as f32 / steps as f32 * STRIP_SECONDS;
    // The runs of a trace where there was signal, as points on the strip
    let runs = |trace: &SignalTrace, y: &dyn Fn(f32) -> f32| -> Vec<Vec<Pos2>> {
        let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
        for step in 0..=steps {
            match trace.at(age(step)) {
                Some(value) => runs.last_mut().unwrap().push(Pos2::new(x_ago(step), y(value))),
                None if !runs.last().unwrap().is_empty() => runs.push(Vec::new()),
                None => {}
            }
        }
        runs.retain(|run| run.len() > 1);
        runs
    };

    // The reduction, hanging from the top: its veil first, so the sound
    // coming in shows through it, and its edge last, over everything
    let gr_y = |value: f32| strip.top() + (value.abs() / GR_PER_DB / STRIP_GR_DB).min(1.0) * strip.height();
    let reduction_runs: Vec<Vec<Pos2>> = user_state
        .output_trace(node_id, GR_PORT, 0)
        .map(|trace| runs(&trace, &gr_y))
        .unwrap_or_default()
        .into_iter()
        .filter(|run| run.iter().any(|point| point.y > strip.top() + 0.5 * z))
        .collect();
    for run in &reduction_runs {
        fill_under(&painter, run, strip.top(), with_alpha(orange, 46), with_alpha(orange, 10));
    }

    let mut heard_any = false;
    if let Some(trace) = incoming_trace(graph, node_id, user_state, "In") {
        for run in runs(&trace, &|level| level_y(to_db(level.abs()))) {
            heard_any = true;
            fill_under(&painter, &run, strip.bottom(), with_alpha(blue, 46), with_alpha(blue, 6));
            curve(&painter, run, with_alpha(blue, 90), 0.7 * z, false);
        }
    }
    if let Some(trace) = user_state.output_trace(node_id, OUT_PORT, 0) {
        for run in runs(&trace, &|level| level_y(to_db(level.abs()))) {
            curve(&painter, run, with_alpha(accent, 235), 1.2 * z, false);
        }
    }
    let pressed = !reduction_runs.is_empty();
    for run in reduction_runs {
        curve(&painter, run, with_alpha(orange, 220), 1.1 * z, false);
    }

    // --- Words ---
    let mut setting = format!("{} · {:.0} dB", ratio_text(settings.ratio), settings.threshold);
    if rms {
        setting.push_str(" · RMS");
    }
    if keyed {
        setting.push_str(" · SC");
    }
    let right = if squeezing {
        (format!("GR -{:.1} dB", reduction), orange)
    } else {
        ("GR 0 dB".to_string(), theme::text::SECONDARY)
    };
    header(&painter, rect, z, Some((setting, accent.gamma_multiply(0.9))), Some(right));

    if user_state.is_playing && (glowing || heard_any || pressed) {
        ui.ctx().request_repaint();
    }

    if response.hovered() {
        let mut text = format!(
            "{} above {:.0} dB, a {:.0} dB knee",
            ratio_text(settings.ratio),
            settings.threshold,
            settings.knee
        );
        text.push_str(&format!(
            "\nLeft: the level the {} detector hears{} across, the output up; the dot is now",
            if rms { "RMS" } else { "peak" },
            if keyed { " (from the sidechain)" } else { "" }
        ));
        if squeezing {
            text.push_str(&format!("\nTaking {:.1} dB off now", reduction));
        }
        text.push_str(&format!(
            "\nRight: the last {:.0} seconds, newest at the right: in (blue), out (line), and the reduction hanging from the top (orange, {:.0} dB at the floor)",
            STRIP_SECONDS, STRIP_GR_DB
        ));
        response.on_hover_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(threshold: f32, ratio: f32, knee: f32) -> Settings {
        Settings { threshold, ratio, knee, makeup: 0.0, mix: 1.0 }
    }

    #[test]
    fn the_curve_is_unity_below_the_threshold() {
        let s = settings(-20.0, 4.0, 0.0);
        for level in [-60.0, -40.0, -21.0] {
            assert!((s.output_db(level) - level).abs() < 1e-3);
        }
    }

    #[test]
    fn the_curve_follows_the_modules_gain_computer() {
        let s = settings(-24.0, 4.0, 6.0);
        for level in [-30.0, -24.0, -22.0, -12.0, 0.0] {
            let reduction = Compressor::compute_gain_reduction(level, -24.0, 4.0, 6.0);
            assert!((s.output_db(level) - (level - reduction)).abs() < 1e-3, "at {level} dB");
        }
        // Over the knee, the ratio: 24 dB over comes out 6 dB over
        assert!((s.output_db(0.0) - -18.0).abs() < 1e-3);
    }

    #[test]
    fn makeup_lifts_the_whole_curve() {
        let mut s = settings(-20.0, 2.0, 0.0);
        s.makeup = 6.0;
        assert!((s.output_db(-40.0) - -34.0).abs() < 1e-3);
        assert!((s.output_db(0.0) - -4.0).abs() < 1e-3);
    }

    #[test]
    fn ratios_read_plainly() {
        assert_eq!(ratio_text(4.0), "4:1");
        assert_eq!(ratio_text(2.5), "2.5:1");
        assert_eq!(ratio_text(19.98), "20:1");
    }
}
