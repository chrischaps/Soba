//! The ADSR's display: the envelope's shape, and where a note is in it.
//!
//! Each stage is drawn with the envelope's own curve ([`AdsrParams::level_at`]),
//! its width growing with the log of its time so a 5 ms attack still shows
//! beside a 5 s release. A, D, S and R sit over their stages. A stage lights
//! while a note is in it; a stage CV is moving is drawn as CV has moved it,
//! its letter tinted. While a note plays, the curve lights up to a dot where
//! the envelope is, as loud as the note's velocity made it, and the glow
//! under the curve swells with the level. The stage the note is in is lit
//! from where it really began: a note let go mid-decay releases from above
//! the sustain, so its release is drawn from there, off the shape.

use eframe::egui::{self, Color32, Pos2, Rect};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::envelope::{curve_steepness, stage_shape, stage_time, AdsrEnvelope};
use crate::widgets::{get_adsr_segment_boundaries, AdsrParams, AdsrSegment};

use super::display_frame::{self as face, columns, curve, guide, playhead, readout, value_of, with_alpha};
use super::drum_display::{duration, fill_under};
use super::{SynthGraph, SynthGraphState};

/// The stages in order, with their letters.
const STAGES: [(AdsrSegment, &str); 4] = [
    (AdsrSegment::Attack, "A"),
    (AdsrSegment::Decay, "D"),
    (AdsrSegment::Sustain, "S"),
    (AdsrSegment::Release, "R"),
];

/// Where a note is in the envelope, from the engine.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Now {
    /// The stage, as an index into [`STAGES`].
    stage: usize,
    /// How far through the stage, in normalized time.
    through: f32,
    level: f32,
    peak: f32,
    /// The level the stage began at, and the one it's heading for.
    start: f32,
    target: f32,
}

/// Draws the display, as wide as the knob row below it.
pub fn envelope_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let params = params(graph, node_id, user_state);
    let now = readout(user_state, node_id).and_then(|r| now(&params, &r.values));

    let orange = theme::signal::CONTROL;
    let width = face::knob_row_width(ui, 4, z);
    let Some((rect, response, painter)) = face::frame(ui, width, crate::dsp::ModuleCategory::Modulation.color(), z) else { return };
    let plot = face::plot(rect, z);
    let x_at = |share: f32| plot.left() + share * plot.width();
    let y_at = |level: f32| plot.bottom() - level.clamp(0.0, 1.0) * plot.height();

    // Where each stage starts and ends, as a share of the width
    let (attack_end, decay_end, sustain_end) = get_adsr_segment_boundaries(&params);
    let spans = [(0.0, attack_end), (attack_end, decay_end), (decay_end, sustain_end), (sustain_end, 1.0)];

    for &(_, end) in &spans[..3] {
        guide(&painter, Pos2::new(x_at(end), plot.top()), Pos2::new(x_at(end), plot.bottom()), 12, z);
    }
    if params.sustain > 0.01 {
        let y = y_at(params.sustain);
        guide(&painter, Pos2::new(plot.left(), y), Pos2::new(plot.right(), y), 16, z);
    }

    let points = envelope_points(&params, plot, z, 1.0, 1.0);
    let swell = now.map_or(0.0, |now| now.level.clamp(0.0, 1.0));
    fill_under(&painter, &points, plot.bottom(), with_alpha(orange, (40.0 + 90.0 * swell) as u8), with_alpha(orange, (5.0 + 16.0 * swell) as u8));

    // The softest note: the same shape at its lower peak, faint, so the
    // span between the two lines is the dynamics
    if params.softest_peak < 0.999 {
        curve(&painter, envelope_points(&params, plot, z, params.softest_peak, 1.0), with_alpha(orange, 90), 0.9 * z, false);
    }

    match now {
        Some(now) => {
            // The shape dims, and lights up to where the note is
            curve(&painter, points, orange.gamma_multiply(0.55), 1.2 * z, false);
            let span = spans[now.stage];
            let at = span.0 + (span.1 - span.0) * now.through;
            curve(&painter, lit_points(&params, plot, z, &now, span, at), orange, 1.8 * z, true);
            playhead(&painter, Pos2::new(x_at(at), y_at(now.level)), with_alpha(orange, (50.0 + 60.0 * swell) as u8), z);
            ui.ctx().request_repaint();
        }
        None => curve(&painter, points, orange, 1.5 * z, true),
    }

    // The letters, over their stages: lit while a note is in the stage,
    // tinted while CV is moving it
    let font = egui::FontId::proportional(8.0 * z);
    for (index, ((_, letter), &(start, end))) in STAGES.iter().zip(&spans).enumerate() {
        let running = now.is_some_and(|now| now.stage == index);
        let color = if running {
            orange
        } else if params.moved[index] {
            orange.gamma_multiply(0.6)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 110)
        };
        let x = x_at(0.5 * (start + end)).clamp(rect.left() + 5.0 * z, rect.right() - 5.0 * z);
        painter.text(Pos2::new(x, rect.top() + 2.0 * z), egui::Align2::CENTER_TOP, letter, font.clone(), color);
    }

    if response.hovered() {
        let mut text = format!(
            "Attack {}, decay {} to {:.0}%, release {}",
            duration(params.attack),
            duration(params.decay),
            params.sustain * 100.0,
            duration(params.release)
        );
        if params.softest_peak < 0.999 {
            text.push_str(&format!("\nThe softest note peaks at {:.0}%", params.softest_peak * 100.0));
        }
        if let Some(now) = now {
            let stage = ["Attacking", "Decaying", "Sustaining", "Releasing"][now.stage];
            text.push_str(&format!("\n{stage}, at {:.0}%", now.level * 100.0));
        }
        response.on_hover_text(text);
    }
}

/// The envelope's settings on the node, with CV's moves where it's patched.
fn params(graph: &SynthGraph, node_id: NodeId, user_state: &SynthGraphState) -> AdsrParams {
    let defaults = AdsrParams::default();
    let number = |name: &str, default: f32| value_of(graph, node_id, name).unwrap_or(default);
    // The softest note only differs when velocity is patched in
    let softest_peak = if face::input_patched(graph, node_id, "Velocity") {
        1.0 - number("Velocity Amount", 0.0)
    } else {
        1.0
    };
    AdsrParams::new(
        number("Attack", defaults.attack),
        number("Decay", defaults.decay),
        number("Sustain", defaults.sustain),
        number("Release", defaults.release),
    )
    .with_curves(
        number("Attack Curve", defaults.attack_curve),
        number("Decay Curve", defaults.decay_curve),
        number("Release Curve", defaults.release_curve),
    )
    .with_softest_peak(softest_peak)
    .with_cv(super::envelope_cv::patched_stages(graph, node_id), super::envelope_cv::readout(user_state, node_id))
}

/// Where the engine says a note is, or `None` when the envelope is idle.
fn now(params: &AdsrParams, values: &[f32]) -> Option<Now> {
    let stage = match values[AdsrEnvelope::READOUT_STAGE].round() as i32 {
        1 => 0,
        2 => 1,
        3 => 2,
        4 => 3,
        _ => return None,
    };
    let progress = values[AdsrEnvelope::READOUT_PROGRESS].clamp(0.0, 1.0) as f64;
    let through = |curve: f32| stage_time(curve_steepness(curve), progress) as f32;
    let through = match stage {
        0 => through(params.attack_curve),
        1 => through(params.decay_curve),
        // Sustain lasts as long as the note: the dot rests mid-plateau
        2 => 0.5,
        _ => through(params.release_curve),
    };
    Some(Now {
        stage,
        through,
        level: values[AdsrEnvelope::READOUT_LEVEL].clamp(0.0, 1.0),
        peak: values[AdsrEnvelope::READOUT_PEAK].clamp(0.0, 1.0),
        start: values[AdsrEnvelope::READOUT_STAGE_START].clamp(0.0, 1.0),
        target: values[AdsrEnvelope::READOUT_STAGE_TARGET].clamp(0.0, 1.0),
    })
}

/// The envelope across `plot`, scaled to `peak`, from the start to `until`
/// (a share of the width). Each stage's ends are points of their own, so
/// the corners stay sharp however few columns there are.
fn envelope_points(params: &AdsrParams, plot: Rect, z: f32, peak: f32, until: f32) -> Vec<Pos2> {
    let (attack_end, decay_end, sustain_end) = get_adsr_segment_boundaries(params);
    let spans = [(0.0, attack_end), (attack_end, decay_end), (decay_end, sustain_end), (sustain_end, 1.0)];
    let columns = columns(plot, z) as f32;
    let mut points = Vec::with_capacity(columns as usize + 8);
    for ((segment, _), (start, end)) in STAGES.iter().zip(spans) {
        if start >= until {
            break;
        }
        let stop = end.min(until);
        let steps = ((columns * (stop - start)).ceil() as usize).max(1);
        for step in 0..=steps {
            let share = start + (stop - start) * step as f32 / steps as f32;
            let u = if end > start { (share - start) / (end - start) } else { 1.0 };
            let level = params.level_at(*segment, u) * peak;
            points.push(Pos2::new(plot.left() + share * plot.width(), plot.bottom() - level.clamp(0.0, 1.0) * plot.height()));
        }
    }
    points
}

/// The lit line, from the start to `until` inside the note's stage, which
/// spans `span`. The stage is drawn as the envelope is running it, from the
/// level it began at, so it ends on the dot. When that's where the shape
/// says the stage begins, the stages before it light up too; when the note
/// left the shape early, only its own path lights.
fn lit_points(params: &AdsrParams, plot: Rect, z: f32, now: &Now, span: (f32, f32), until: f32) -> Vec<Pos2> {
    let (start, end) = span;
    let (segment, _) = STAGES[now.stage];
    let steepness = match segment {
        AdsrSegment::Attack => Some(curve_steepness(params.attack_curve)),
        AdsrSegment::Decay => Some(curve_steepness(params.decay_curve)),
        AdsrSegment::Sustain => None,
        AdsrSegment::Release => Some(curve_steepness(params.release_curve)),
    };
    let steps = ((columns(plot, z) as f32 * (until - start)).ceil() as usize).max(1);
    let stage = (0..=steps).map(|step| {
        let share = start + (until - start) * step as f32 / steps as f32;
        let u = if end > start { (share - start) / (end - start) } else { 1.0 };
        let level = match steepness {
            Some(steepness) => now.start + (now.target - now.start) * stage_shape(steepness, u as f64) as f32,
            None => now.level,
        };
        Pos2::new(plot.left() + share * plot.width(), plot.bottom() - level.clamp(0.0, 1.0) * plot.height())
    });

    let on_shape = (now.start - params.level_at(segment, 0.0) * now.peak).abs() < 0.01;
    let mut points = if on_shape && start > 0.0 {
        let mut before = envelope_points(params, plot, z, now.peak, start);
        before.pop();
        before
    } else {
        Vec::new()
    };
    points.extend(stage);
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    fn plot() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 40.0))
    }

    #[test]
    fn the_corners_are_points_of_their_own() {
        let params = AdsrParams::new(0.01, 0.3, 0.4, 0.5);
        let points = envelope_points(&params, plot(), 1.0, 1.0, 1.0);
        // The peak, where the attack ends, is reached exactly
        assert!(points.iter().any(|p| p.y.abs() < 1e-4), "no point at the peak");
        let (_, decay_end, _) = get_adsr_segment_boundaries(&params);
        let corner = points.iter().find(|p| (p.x - decay_end * 200.0).abs() < 1e-3).unwrap();
        assert!((corner.y - 40.0 * 0.6).abs() < 1e-3, "sustain corner at {}", corner.y);
        assert!((points.last().unwrap().y - 40.0).abs() < 1e-3);
    }

    #[test]
    fn the_lit_part_stops_where_the_note_is() {
        let params = AdsrParams::default();
        let points = envelope_points(&params, plot(), 1.0, 0.5, 0.3);
        assert!((points.last().unwrap().x - 60.0).abs() < 1e-3);
        // Scaled to the note's peak
        assert!(points.iter().all(|p| p.y >= 20.0 - 1e-3));
    }

    #[test]
    fn the_engine_says_where_the_note_is() {
        let params = AdsrParams::default();
        let mut values = [0.0; crate::dsp::MAX_READOUT];
        assert_eq!(now(&params, &values), None, "idle");
        values[AdsrEnvelope::READOUT_STAGE] = 2.0;
        values[AdsrEnvelope::READOUT_PROGRESS] = crate::modules::envelope::stage_shape(curve_steepness(params.decay_curve), 0.25) as f32;
        values[AdsrEnvelope::READOUT_LEVEL] = 0.9;
        values[AdsrEnvelope::READOUT_PEAK] = 1.0;
        values[AdsrEnvelope::READOUT_STAGE_START] = 1.0;
        values[AdsrEnvelope::READOUT_STAGE_TARGET] = params.sustain;
        let note = now(&params, &values).unwrap();
        assert_eq!(note.stage, 1);
        assert!((note.through - 0.25).abs() < 1e-4, "{}", note.through);
    }

    /// Renders a note through the real envelope, gated for `gate` seconds,
    /// and reads where it is after `at` seconds
    fn note_at(module_params: [f32; 8], gate: f32, at: f32) -> Now {
        use crate::dsp::{DspModule, ProcessContext, SignalBuffer};
        let sr = 48000.0;
        let total = (at * sr) as usize;
        let mut env = AdsrEnvelope::new();
        env.prepare(sr, total);
        let mut gate_in = SignalBuffer::control(total);
        gate_in.samples[..((gate * sr) as usize).min(total)].fill(1.0);
        let mut out = vec![SignalBuffer::control(total)];
        env.process(&[&gate_in], &mut out, &module_params, &ProcessContext::new(sr, total));
        let [a, d, s, r, ac, dc, rc, _] = module_params;
        let params = AdsrParams::new(a, d, s, r).with_curves(ac, dc, rc);
        now(&params, &env.readout(&[]).unwrap().values).unwrap()
    }

    #[test]
    fn the_dot_sits_on_the_lit_line() {
        let module_params = [0.05, 0.3, 0.4, 0.5, 0.2, 0.5, 0.5, 0.0];
        let [a, d, s, r, ac, dc, rc, _] = module_params;
        let params = AdsrParams::new(a, d, s, r).with_curves(ac, dc, rc);
        let (attack_end, decay_end, sustain_end) = get_adsr_segment_boundaries(&params);
        let spans = [(0.0, attack_end), (attack_end, decay_end), (decay_end, sustain_end), (sustain_end, 1.0)];
        // Let go after reaching the sustain, mid-decay and mid-attack; then a
        // held note in each of its first three stages
        for (gate, at, stage) in [(0.5, 0.6, 3), (0.1, 0.2, 3), (0.02, 0.1, 3), (0.5, 0.03, 0), (0.5, 0.15, 1), (0.5, 0.45, 2)] {
            let note = note_at(module_params, gate, at);
            assert_eq!(note.stage, stage, "gate {gate}, at {at}");
            let span = spans[note.stage];
            let x = span.0 + (span.1 - span.0) * note.through;
            let points = lit_points(&params, plot(), 1.0, &note, span, x);
            let end = *points.last().unwrap();
            let dot = Pos2::new(x * 200.0, 40.0 - note.level * 40.0);
            assert!((end - dot).length() < 0.05, "gate {gate}, at {at}: line ends at {end:?}, dot at {dot:?}");
            // Lit from the start only when the note followed the shape that far
            let from_start = points[0].x.abs() < 1e-4;
            assert_eq!(from_start, gate >= a + d || stage < 3, "gate {gate}, at {at}");
        }
    }
}
