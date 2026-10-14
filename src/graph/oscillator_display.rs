//! The Oscillator's display: one cycle of the wave it plays, and its voices.
//!
//! The wave is drawn from the oscillator's own shape ([`OscWaveform::value`])
//! at the Pulse Width, with its jumps drawn straight up and down. With more
//! than one unison voice, the others are drawn faintly over it, sliding
//! along the cycle at the rate each one beats against the pitch: a narrow
//! Detune drifts slowly, a wide one shimmers. With Sub patched, two cycles
//! show, with the square an octave below under them.
//!
//! While it plays, what Out really carries is drawn over the shape, lit,
//! and the shape steps back to a faint guide (see [`super::oscillator_trace`]).
//! It's drawn to the same time scale, the cycle the knobs set, so a note
//! V/Oct takes an octave up fits two cycles in and one an octave down half a
//! cycle: a melody squeezes and stretches it. PWM moves the edge, FM
//! wrinkles the wave, sync cuts it short, and unison voices beat in it.

use eframe::egui::{self, Color32, Pos2, Rect};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::oscillator::{OscWaveform, Oscillator};
use crate::modules::quantizer::NOTE_NAMES;

use super::display_frame::{self as face, columns, curve, guide, header, hertz, input_patched, output_patched, readout, value_of, with_alpha};
use super::drum_display::fill_under;
use super::oscillator_trace::{live, trace_points};
use super::{SynthGraph, SynthGraphState};

/// The fastest a voice is drawn sliding, in cycles per second. Wider beats
/// are slowed to this, all in proportion, so a wide detune still reads.
const MAX_DRIFT: f32 = 1.5;

/// Where each unison voice starts along its cycle, as the oscillator starts them.
const GOLDEN: f32 = 0.618_034;

/// The Waveform dropdown's names, in order.
const WAVES: [&str; 4] = ["Sine", "Saw", "Square", "Triangle"];

/// Draws the display, as wide as the knob row below it.
pub fn oscillator_display(ui: &mut eframe::egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let value = |name: &str, default: f32| value_of(graph, node_id, name).unwrap_or(default);
    let wave_index = value("Waveform", 0.0);
    let wave = OscWaveform::from_param(wave_index);
    let pulse_width = value("Pulse Width", 0.5).clamp(0.1, 0.9);
    let voices = (value("Voices", 1.0).round() as usize).clamp(1, crate::modules::oscillator::MAX_VOICES);
    let detune = value("Detune", 0.4);
    let hz = Oscillator::C4_HZ * Oscillator::tune_octaves(value("Octave", 0.0), value("Semitone", 0.0), value("Fine", 0.0)).exp2();
    let sub = output_patched(graph, node_id, "Sub");
    let tracked = input_patched(graph, node_id, "V/Oct");
    let bent = tracked || input_patched(graph, node_id, "Exp FM");

    let blue = theme::signal::AUDIO;
    let width = face::knob_row_width(ui, 3, z);
    let Some((rect, response, painter)) = face::frame(ui, width, crate::dsp::ModuleCategory::Source.color(), z) else { return };
    let plot = face::plot(rect, z);
    let cycles = if sub { 2.0 } else { 1.0 };
    let y_at = |v: f32| plot.center().y - v.clamp(-1.0, 1.0) * plot.height() * 0.5;
    // A live trace may ring a little past the top and bottom at its edges
    let y_live = |v: f32| plot.center().y - v.clamp(-1.15, 1.15) * plot.height() * 0.5;

    let scope = user_state.get_engine_node_id(node_id).and_then(|id| user_state.get_scope_data(id));
    let now = live(ui.ctx(), egui::Id::new(("oscillator trace", node_id)), scope);
    let presence = now.presence;
    let playing = readout(user_state, node_id).filter(|_| presence > 0.0);
    // While it plays, the shape is drawn at the pulse width PWM has it at
    let pulse_width = playing.map_or(pulse_width, |r| r.values[Oscillator::READOUT_PULSE_WIDTH].clamp(0.1, 0.9));
    // The shape steps back while the trace is lit
    let shade = |alpha: f32| (alpha * (1.0 - 0.7 * presence)) as u8;

    guide(&painter, Pos2::new(plot.left(), plot.center().y), Pos2::new(plot.right(), plot.center().y), 18, z);
    if sub {
        guide(&painter, Pos2::new(plot.center().x, plot.top()), Pos2::new(plot.center().x, plot.bottom()), 12, z);
    }

    // The sub: high for the first cycle, low for the second, at half height
    if sub {
        let deep = Color32::from_rgb(46, 104, 196);
        let half = 0.5;
        let points = vec![
            Pos2::new(plot.left(), y_at(0.0)),
            Pos2::new(plot.left(), y_at(half)),
            Pos2::new(plot.center().x, y_at(half)),
            Pos2::new(plot.center().x, y_at(-half)),
            Pos2::new(plot.right(), y_at(-half)),
            Pos2::new(plot.right(), y_at(0.0)),
        ];
        curve(&painter, points, deep.gamma_multiply(0.8), 1.0 * z, false);
    }

    // The other voices, sliding at their beat rates
    if voices > 1 {
        let time = ui.ctx().input(|i| i.time) as f32;
        let beats: Vec<f32> = (0..voices)
            .map(|i| hz * ((Oscillator::unison_cents(i, voices, detune) / 1200.0).exp2() - 1.0))
            .collect();
        let fastest = beats.iter().fold(0.0f32, |most, b| most.max(b.abs()));
        let slow = if fastest > MAX_DRIFT { MAX_DRIFT / fastest } else { 1.0 };
        for (i, beat) in beats.iter().enumerate() {
            let offset = (i as f32 * GOLDEN + time * beat * slow).rem_euclid(1.0);
            let points = wave_points(wave, pulse_width, plot, cycles, offset, z, y_at);
            // The live trace shows them beating, so these all but go
            curve(&painter, points, with_alpha(blue, (44.0 * (1.0 - 0.85 * presence)) as u8), 0.9 * z, false);
        }
        if fastest > 0.0 {
            ui.ctx().request_repaint();
        }
    }

    // The wave itself
    let points = wave_points(wave, pulse_width, plot, cycles, 0.0, z, y_at);
    fill_under(&painter, &points, plot.center().y, with_alpha(blue, shade(46.0)), with_alpha(blue, shade(6.0)));
    curve(&painter, points, with_alpha(blue, shade(255.0)), (1.4 - 0.4 * presence) * z, presence < 0.5);

    // What Out is playing: the last few captures fading behind the newest
    if presence > 0.0 {
        let lit = blue.lerp_to_gamma(Color32::WHITE, 0.3);
        let newest = now.traces.len() - 1;
        for (i, (brightness, trace)) in now.traces.iter().enumerate() {
            let points = trace_points(trace, plot, cycles, y_live);
            if i == newest {
                fill_under(&painter, &points, plot.center().y, with_alpha(blue, (40.0 * presence) as u8), with_alpha(blue, (5.0 * presence) as u8));
                curve(&painter, points, with_alpha(lit, (255.0 * presence) as u8), 1.4 * z, true);
            } else {
                curve(&painter, points, with_alpha(blue, (110.0 * brightness * presence) as u8), 1.0 * z, false);
            }
        }
        ui.ctx().request_repaint();
    }

    // Moved by V/Oct or Exp FM, the note it's playing
    let played = playing.map(|r| r.values[Oscillator::READOUT_HZ]).filter(|hz| bent && *hz > 0.0);
    let pitch = |hz: f32| format!("{} · {}", note_name(hz), hertz(hz));
    let ink = theme::text::SECONDARY;
    let left = match played {
        Some(hz) => pitch(hz),
        None if tracked => format!("{} + V/Oct", pitch(hz)),
        None => pitch(hz),
    };
    let right = if voices > 1 { format!("{voices} voices") } else { WAVES[wave as usize].to_string() };
    header(&painter, rect, z, Some((left, blue.gamma_multiply(0.9))), Some((right, ink)));

    if response.hovered() {
        let shape = match wave {
            OscWaveform::Square if (pulse_width - 0.5).abs() > 0.01 => format!("Pulse, high for {:.0}% of the cycle", pulse_width * 100.0),
            _ => WAVES[wave as usize].to_string(),
        };
        let mut text = format!("{shape} at {} ({})", note_name(hz), hertz(hz));
        if let Some(played) = played {
            text.push_str(&format!(", playing {} ({})", note_name(played), hertz(played)));
        } else if tracked {
            text.push_str(", moved by V/Oct");
        }
        if voices > 1 {
            let outer = Oscillator::unison_cents(voices - 1, voices, detune);
            text.push_str(&format!("\n{voices} voices, detuned up to ±{outer:.0} cents"));
        }
        if sub {
            text.push_str("\nSub: a square an octave below");
        }
        if presence > 0.0 {
            text.push_str("\nLit: what Out is playing now");
        }
        response.on_hover_text(text);
    }
}

/// The points of `cycles` cycles of a wave across `plot`, starting `offset`
/// of a cycle in, with each jump drawn as a straight edge.
fn wave_points(wave: OscWaveform, pulse_width: f32, plot: Rect, cycles: f32, offset: f32, z: f32, y_at: impl Fn(f32) -> f32) -> Vec<Pos2> {
    let columns = columns(plot, z);
    let x_at = |u: f32| plot.left() + u * plot.width();
    // Where the wave jumps, as a phase through the cycle
    let jumps: &[f32] = match wave {
        OscWaveform::Saw => &[0.0],
        OscWaveform::Square => &[0.0, pulse_width],
        _ => &[],
    };
    // Each jump inside the window, as a share of the way across
    let mut edges: Vec<(f32, f32)> = Vec::new();
    for cycle in offset.floor() as i32..=(offset + cycles).ceil() as i32 {
        for &jump in jumps {
            let u = (cycle as f32 + jump - offset) / cycles;
            if u > 1e-4 && u < 1.0 - 1e-4 {
                edges.push((u, jump));
            }
        }
    }
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut points = Vec::with_capacity(columns + 2 * edges.len() + 1);
    let mut edges = edges.into_iter().peekable();
    let mut last_edge = f32::NEG_INFINITY;
    for column in 0..=columns {
        let u = column as f32 / columns as f32;
        // The jumps before this column, straight up or down
        while let Some(&(at, jump)) = edges.peek().filter(|(at, _)| *at <= u + 1e-4) {
            let before = if jump == 0.0 { 1.0 } else { jump - 1e-4 };
            points.push(Pos2::new(x_at(at), y_at(wave.value(before, pulse_width))));
            points.push(Pos2::new(x_at(at), y_at(wave.value(jump, pulse_width))));
            last_edge = at;
            edges.next();
        }
        // A column on a jump is the jump's foot, already drawn
        if (u - last_edge).abs() < 1e-4 {
            continue;
        }
        let phase = (offset + u * cycles).rem_euclid(1.0);
        points.push(Pos2::new(x_at(u), y_at(wave.value(phase, pulse_width))));
    }
    points
}

/// The nearest note to a pitch: "C4", "F#2".
fn note_name(hz: f32) -> String {
    let semis = (12.0 * (hz / Oscillator::C4_HZ).log2()).round() as i32;
    format!("{}{}", NOTE_NAMES[semis.rem_euclid(12) as usize], 4 + semis.div_euclid(12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    fn plot() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(150.0, 40.0))
    }

    fn y_at(v: f32) -> f32 {
        20.0 - v * 20.0
    }

    #[test]
    fn the_triangle_starts_where_the_oscillator_does() {
        // The oscillator's triangle starts at the bottom and peaks halfway
        let points = wave_points(OscWaveform::Triangle, 0.5, plot(), 1.0, 0.0, 1.0, y_at);
        assert!((points[0].y - y_at(-1.0)).abs() < 0.01);
        let top = points.iter().min_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
        assert!((top.x - 75.0).abs() < 2.0, "peak at {}", top.x);
    }

    #[test]
    fn a_square_jumps_straight_up_and_down() {
        let points = wave_points(OscWaveform::Square, 0.3, plot(), 1.0, 0.0, 1.0, y_at);
        // Two points at the same x a full swing apart, at the pulse width
        let edge = points.windows(2).find(|w| (w[0].x - w[1].x).abs() < 1e-3 && (w[0].y - w[1].y).abs() > 39.0).unwrap();
        assert!((edge[0].x - 0.3 * 150.0).abs() < 0.01, "edge at {}", edge[0].x);
        // Every point lies on the wave
        assert!(points.iter().all(|p| (p.y - y_at(1.0)).abs() < 0.01 || (p.y - y_at(-1.0)).abs() < 0.01));
    }

    #[test]
    fn a_saw_jumps_at_each_wrap() {
        let points = wave_points(OscWaveform::Saw, 0.5, plot(), 2.0, 0.25, 1.0, y_at);
        let edges: Vec<f32> = points
            .windows(2)
            .filter(|w| (w[0].x - w[1].x).abs() < 1e-3 && (w[0].y - w[1].y).abs() > 30.0)
            .map(|w| w[0].x)
            .collect();
        // Starting a quarter in, two cycles wrap at 3/8 and 7/8 of the way
        assert_eq!(edges.len(), 2, "{edges:?}");
        assert!((edges[0] - 150.0 * 0.375).abs() < 0.01 && (edges[1] - 150.0 * 0.875).abs() < 0.01, "{edges:?}");
    }

    #[test]
    fn notes_are_named() {
        assert_eq!(note_name(Oscillator::C4_HZ), "C4");
        assert_eq!(note_name(440.0), "A4");
        assert_eq!(note_name(Oscillator::C4_HZ / 4.0 * 2f32.powf(6.0 / 12.0)), "F#2");
    }
}
