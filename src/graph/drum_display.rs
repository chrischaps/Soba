//! The Drum's display: the hit it will play, and the hit it just played.
//!
//! The envelope is drawn from the voice's own [`Voicing`], so the curve is
//! the decay you hear. Time runs on a warped axis, wide at the start, so a
//! kick's 10 ms drop and a cymbal's five-second wash both fit. Over it, in
//! the Control orange, is the pitch: a skin's sweep falling to rest, or for
//! the metal, its square partials as lines that fade with the ring.
//!
//! Each hit lights the curve up to a playhead that travels along it, as
//! loud as the hit's Accent, and flashes the fill as it lands. A choked hit
//! drops to nothing where the Choke caught it.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::drum::{Drum, DrumType, Strike, Voicing, CLAP_BURSTS};

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Where the warped time axis bends from linear to logarithmic, in seconds.
const BEND: f32 = 0.025;

/// How long a hit's flash lasts, in seconds.
const FLASH: f32 = 0.12;

/// The pitch axis, in Hz, bottom to top.
const LOWEST_HZ: f32 = 30.0;
const HIGHEST_HZ: f32 = 6000.0;

/// How fast a choked voice falls, matching the voice's own damping.
const CHOKE_TAU: f32 = 0.0007;

/// Draws the display, as wide as the knob row below it.
pub fn drum_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let Some(node) = graph.nodes.get(node_id) else { return };
    let value_of = |name: &str| {
        node.inputs.iter().find(|(input, _)| input == name).and_then(|(_, id)| match graph.get_input(*id).value {
            SynthValueType::Number { value, .. } => Some(value),
            SynthValueType::Select { value, .. } => Some(value as f32),
            _ => None,
        })
    };
    // The hit the knobs will play next, at full accent
    let defaults = Strike::default();
    let strike = Strike {
        tune: value_of("Tune").unwrap_or(defaults.tune),
        decay: value_of("Decay").unwrap_or(defaults.decay),
        tone: value_of("Tone").unwrap_or(defaults.tone),
        snap: value_of("Snap").unwrap_or(defaults.snap),
        ..defaults
    };
    let kind = DrumType::from_param(value_of("Type").unwrap_or(0.0));
    let voicing = Voicing::new(kind, strike);

    let readout = user_state
        .get_engine_node_id(node_id)
        .and_then(|id| user_state.readouts.get(&id))
        .copied()
        .unwrap_or_default();
    let since = readout.values[Drum::READOUT_SINCE];
    let accent = readout.values[Drum::READOUT_ACCENT].clamp(0.0, 1.0);
    let choked = readout.values[Drum::READOUT_CHOKED];

    // As wide as the five knobs below
    let gap = ui.spacing().item_spacing.x;
    let width = 5.0 * 44.0 * z + 4.0 * gap - 8.0 * z;
    let height = 56.0 * z;

    // Separator, as the other displays have, across the display's width
    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let accent_color = crate::dsp::ModuleCategory::Source.color();
    ui.painter().hline(
        rect.x_range(),
        rect.top() - 4.0 * z,
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(accent_color.r(), accent_color.g(), accent_color.b(), 64)),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0 * z, Color32::from_rgb(20, 22, 30));

    // The window is the whole hit; time bends so its start has room
    let window = voicing.length();
    let warp = |t: f32| (1.0 + t.max(0.0) / BEND).ln() / (1.0 + window / BEND).ln();
    let unwarp = |x: f32| BEND * (((1.0 + window / BEND).ln() * x).exp() - 1.0);

    let pad = 3.0 * z;
    let plot = Rect::from_min_max(rect.min + Vec2::new(pad, pad + 8.0 * z), rect.max - Vec2::new(pad, pad));
    let x_at = |t: f32| plot.left() + warp(t) * plot.width();
    let y_level = |level: f32| plot.bottom() - level.clamp(0.0, 1.05) * plot.height();
    let y_pitch = |hz: f32| {
        let u = (hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln();
        plot.bottom() - u.clamp(0.0, 1.0) * plot.height()
    };

    // Faint marks at 10 ms, 100 ms and every second, so the bend reads
    let grid = Color32::from_rgba_unmultiplied(255, 255, 255, 14);
    for mark in [0.01, 0.1, 1.0, 2.0, 3.0, 4.0, 5.0] {
        if mark < window {
            let x = x_at(mark);
            painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(0.5 * z, grid));
        }
    }

    // Sample the envelope along the axis
    let columns = (plot.width() / (1.5 * z)).max(48.0) as usize;
    let times: Vec<f32> = (0..=columns).map(|i| unwarp(i as f32 / columns as f32)).collect();
    let mut levels: Vec<f32> = times.iter().map(|&t| voicing.amplitude_at(t)).collect();
    // The clap's bursts are narrower than a column: catch each one's peak
    if kind == DrumType::Clap {
        for &start in &CLAP_BURSTS {
            let column = (warp(start) * columns as f32).round() as usize;
            if let Some(level) = levels.get_mut(column) {
                *level = level.max(voicing.amplitude_at(start));
            }
        }
    }
    let tallest = levels.iter().copied().fold(1e-6, f32::max);
    let points: Vec<Pos2> = times.iter().zip(&levels).map(|(&t, &level)| Pos2::new(x_at(t), y_level(level / tallest))).collect();

    // The flash of a fresh hit
    let flash = if since >= 0.0 { (-since / FLASH).exp() * accent } else { 0.0 };
    let blue = accent_color;
    let glow = |alpha: f32| Color32::from_rgba_unmultiplied(blue.r(), blue.g(), blue.b(), alpha.clamp(0.0, 255.0) as u8);
    fill_under(&painter, &points, plot.bottom(), glow(60.0 + 110.0 * flash), glow(6.0 + 20.0 * flash));
    painter.add(Shape::line(points.clone(), Stroke::new(1.2 * z, blue.gamma_multiply(0.55 + 0.45 * flash))));

    // The pitch, over the envelope
    let orange = theme::signal::CONTROL;
    if let Some(start) = voicing.pitch_at(0.0) {
        let trace: Vec<Pos2> = times.iter().map(|&t| Pos2::new(x_at(t), y_pitch(voicing.pitch_at(t).unwrap_or(start)))).collect();
        painter.add(Shape::line(trace, Stroke::new(1.0 * z, orange.gamma_multiply(0.8))));
    } else {
        let partials: Vec<f32> = match kind {
            // The band the hands or the beads ring in
            _ if kind.is_band() => vec![voicing.pitch_hz],
            _ => kind.partials().iter().map(|ratio| voicing.pitch_hz * ratio).collect(),
        };
        for hz in partials {
            let y = y_pitch(hz);
            // Each partial fades with the ring, column by column
            for (pair, level) in points.windows(2).zip(&levels) {
                let alpha = (24.0 + 190.0 * level / tallest) as u8;
                painter.line_segment(
                    [Pos2::new(pair[0].x, y), Pos2::new(pair[1].x, y)],
                    Stroke::new(0.9 * z, Color32::from_rgba_unmultiplied(orange.r(), orange.g(), orange.b(), alpha)),
                );
            }
        }
    }

    // The hit just played, lit up to the playhead
    if since >= 0.0 && since < window {
        let choke_at = (choked >= 0.0).then_some(choked);
        let played = |t: f32| {
            let damping = choke_at.map_or(1.0, |at| if t > at { (-(t - at) / CHOKE_TAU).exp() } else { 1.0 });
            voicing.amplitude_at(t) / tallest * accent * damping
        };
        let lit: Vec<Pos2> = times
            .iter()
            .take_while(|&&t| t <= since)
            .chain(std::iter::once(&since))
            .map(|&t| Pos2::new(x_at(t), y_level(played(t))))
            .collect();
        if lit.len() >= 2 {
            painter.add(Shape::line(lit, Stroke::new(1.8 * z, blue.gamma_multiply(1.2))));
        }
        let head = Pos2::new(x_at(since), y_level(played(since)));
        painter.line_segment([Pos2::new(head.x, plot.top()), Pos2::new(head.x, plot.bottom())], Stroke::new(0.6 * z, blue.gamma_multiply(0.35)));
        painter.circle_filled(head, 3.5 * z, blue.gamma_multiply(0.25));
        painter.circle_filled(head, 1.8 * z, Color32::WHITE.gamma_multiply(0.6 + 0.4 * flash));
    }

    // What it is, in a word or two
    let small = egui::FontId::proportional(8.0 * z);
    let label = match voicing.pitch_at(f32::INFINITY) {
        Some(hz) => format!("{} Hz", hz.round()),
        None if kind.is_band() => format!("{} Hz band", voicing.pitch_hz.round()),
        None => format!("{} squares", kind.partials().len()),
    };
    painter.text(rect.left_top() + Vec2::new(4.0 * z, 2.0 * z), egui::Align2::LEFT_TOP, label, small.clone(), orange.gamma_multiply(0.8));
    painter.text(rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z), egui::Align2::RIGHT_TOP, duration(window), small, theme::text::SECONDARY);

    if response.hovered() {
        let pitch = match (voicing.pitch_at(0.0), voicing.pitch_at(f32::INFINITY)) {
            (Some(start), Some(rest)) => format!("Pitch falls from {} to {} Hz", start.round(), rest.round()),
            _ if kind == DrumType::Clap => format!("Four bursts of noise in a band at {} Hz, then a tail", voicing.pitch_hz.round()),
            _ if kind == DrumType::Shaker => {
                format!("Beads in a band at {} Hz, swelling in over {}", voicing.pitch_hz.round(), duration(3.0 * voicing.swell))
            }
            _ => {
                let hz: Vec<String> = kind.partials().iter().map(|r| format!("{}", (voicing.pitch_hz * r).round())).collect();
                format!("Squares at {} Hz", hz.join(", "))
            }
        };
        let last = if since < 0.0 {
            "Not hit yet".to_string()
        } else if choked >= 0.0 && choked < since {
            format!("Last hit at {:.0}% accent, choked after {}", accent * 100.0, duration(choked))
        } else {
            format!("Last hit at {:.0}% accent", accent * 100.0)
        };
        response.on_hover_text(format!("{}: rings for {}\n{pitch}\n{last}", kind.name(), duration(window)));
    }
}

/// A time to read at a glance: "45 ms", "1.8 s".
pub(super) fn duration(seconds: f32) -> String {
    if seconds < 1.0 {
        format!("{} ms", (seconds * 1000.0).round())
    } else {
        format!("{seconds:.1} s")
    }
}

/// Fills between a curve and the baseline, fading from `top` at the curve
/// to `bottom` at the baseline.
pub(super) fn fill_under(painter: &egui::Painter, points: &[Pos2], baseline: f32, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    for point in points {
        mesh.colored_vertex(*point, top);
        mesh.colored_vertex(Pos2::new(point.x, baseline), bottom);
    }
    for i in 0..points.len().saturating_sub(1) as u32 {
        let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(c, b, d);
    }
    painter.add(Shape::mesh(mesh));
}
