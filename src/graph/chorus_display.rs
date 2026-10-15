//! The Chorus's display: its voices braiding as they sweep.
//!
//! Time runs to the left from now, at the right edge, over the last two
//! seconds. Up and down is how far each tap reads behind the base Delay:
//! the centre line is the Delay itself, and Depth sets how far either side
//! the taps swing. Each voice is a strand following the module's own LFO
//! from its place on the cycle: its left tap solid, its right tap fainter,
//! half a voice-spacing behind. So one voice is a single wave, and four
//! weave a braid. Rate sets how many cycles cross the panel, and Shape
//! bends them from sine to triangle.
//!
//! Where sound came in, the strands light up, so the braid glows with the
//! music passing through it. Positive Feedback thickens the strands; turned
//! negative, the hollow flanger sound, they go hollow and violet.

use std::sync::Arc;

use eframe::egui::{self, epaint::PathStroke, Color32, Pos2, Shape, Stroke};
use egui_node_graph2::{NodeId, SignalTrace};

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::modules::chorus::{lfo, voice_phases};
use crate::modules::Chorus;

use super::display_frame::{self as face, columns, header, incoming_trace, input_patched, readout, value_of, with_alpha, PANEL};
use super::{SynthGraph, SynthGraphState};

/// How far back the braid reaches, in seconds.
const WINDOW: f32 = 2.0;

/// Each voice's colour, slot by slot.
const VOICE_COLORS: [Color32; 4] = [
    Color32::from_rgb(77, 208, 225),
    Color32::from_rgb(170, 140, 255),
    Color32::from_rgb(90, 224, 170),
    Color32::from_rgb(255, 146, 196),
];

/// The colour negative feedback turns the strands toward.
const HOLLOW: Color32 = Color32::from_rgb(196, 120, 255);

/// Below this, the sound coming in is drawn as quiet as it is, not scaled up.
const LOUDEST_FLOOR: f32 = 0.02;

/// A voice as the braid draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Voice {
    gain: f32,
    left: f32,
    right: f32,
}

/// The sweep: where the LFO is, how fast and how far it goes, its shape,
/// and the voices riding it.
#[derive(Clone, Copy, Debug)]
struct Sweep {
    phase: f32,
    rate: f32,
    depth: f32,
    shape: f32,
    voices: [Voice; 4],
}

impl Sweep {
    /// Where a tap at `offset` on the cycle reads, `age` seconds ago: -1 to 1
    /// of the swing either side of the base delay.
    fn tap(&self, offset: f32, age: f32) -> f32 {
        lfo((self.phase - self.rate * age + offset).rem_euclid(1.0), self.shape) * self.depth
    }

    /// The sweep the knobs set, with the LFO at the start of its cycle.
    fn of_knobs(rate: f32, depth: f32, shape: f32, voices: usize) -> Self {
        let mut slots = [Voice { gain: 0.0, left: 0.0, right: 0.0 }; 4];
        for (slot, voice) in slots.iter_mut().enumerate() {
            let (left, right) = voice_phases(slot, voices.max(slot + 1));
            *voice = Voice { gain: if slot < voices { 1.0 } else { 0.0 }, left, right };
        }
        Sweep { phase: 0.0, rate, depth, shape, voices: slots }
    }

    /// The sweep the engine reports.
    fn of_readout(values: &[f32]) -> Self {
        let mut voices = [Voice { gain: 0.0, left: 0.0, right: 0.0 }; 4];
        for (slot, voice) in voices.iter_mut().enumerate() {
            let at = Chorus::READOUT_VOICES + slot * Chorus::READOUT_PER_VOICE;
            *voice = Voice { gain: values[at], left: values[at + 1], right: values[at + 2] };
        }
        Sweep {
            phase: values[Chorus::READOUT_PHASE],
            rate: values[Chorus::READOUT_RATE],
            depth: values[Chorus::READOUT_DEPTH],
            shape: values[Chorus::READOUT_SHAPE],
            voices,
        }
    }
}

/// Mixes `a` toward `b` by `t`.
fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// "1.0 Hz", "0.25 Hz".
fn rate_text(hz: f32) -> String {
    if hz < 1.0 {
        format!("{hz:.2} Hz")
    } else {
        format!("{hz:.1} Hz")
    }
}

/// Draws the display, as wide as the knob row below it.
pub fn chorus_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let knob_rate = value_of(graph, node_id, "Rate").unwrap_or(1.0);
    let knob_depth = value_of(graph, node_id, "Depth").unwrap_or(0.5);
    let delay_ms = value_of(graph, node_id, "Delay").unwrap_or(10.0);
    let feedback = value_of(graph, node_id, "Feedback").unwrap_or(0.0);
    let voices = value_of(graph, node_id, "Voices").unwrap_or(1.0).max(0.0) as usize + 1;
    let shape = value_of(graph, node_id, "Shape").unwrap_or(0.0);

    let live = readout(user_state, node_id).filter(|r| user_state.is_playing && r.values[Chorus::READOUT_RATE] > 0.0);
    let sweep = live.map_or_else(|| Sweep::of_knobs(knob_rate, knob_depth, shape, voices.min(4)), |r| Sweep::of_readout(&r.values));
    let rate_moved = input_patched(graph, node_id, "Rate") && (sweep.rate / knob_rate - 1.0).abs() > 0.02;

    let accent = ModuleCategory::Effect.color();
    let width = face::knob_row_width(ui, 5, z);
    let Some((rect, response, painter)) = face::frame(ui, width, accent, z) else { return };
    let plot = face::plot(rect, z);
    let middle = plot.center().y;
    let half = plot.height() / 2.0 - 1.5 * z;

    // The base delay, and how far the knobs let the taps swing
    painter.hline(plot.x_range(), middle, Stroke::new(0.5 * z, Color32::from_white_alpha(26)));
    for sign in [-1.0, 1.0] {
        let y = middle + sign * sweep.depth * half;
        painter.hline(plot.x_range(), y, Stroke::new(0.5 * z, Color32::from_white_alpha(9)));
    }

    // The sound coming in, to light the strands where it passed
    let heard: Option<SignalTrace> = incoming_trace(graph, node_id, user_state, "In L").filter(|_| user_state.is_playing);
    let columns = columns(plot, z);
    let age_of = |i: usize| i as f32 / columns as f32 * WINDOW;
    let x_of = |i: usize| plot.right() - i as f32 / columns as f32 * plot.width();
    let loudest = heard.as_ref().map_or(0.0, |trace| (0..=columns).filter_map(|i| trace.at(age_of(i))).fold(0.0, |m: f32, v| m.max(v.abs())));
    let light: Arc<[f32]> = (0..=columns)
        .map(|i| heard.as_ref().and_then(|trace| trace.at(age_of(i))).map_or(0.0, |v| (v.abs() / loudest.max(LOUDEST_FLOOR)).min(1.0)))
        .collect();

    // Positive feedback thickens the strands; negative hollows them out
    let thick = 1.1 + 2.4 * feedback.max(0.0);
    let hollow = (-feedback * 2.0).clamp(0.0, 1.0);

    // The right taps behind, the left taps over them, the first voice on top
    for right in [true, false] {
        for (slot, voice) in sweep.voices.iter().enumerate().rev() {
            if voice.gain < 0.01 {
                continue;
            }
            let color = lerp(VOICE_COLORS[slot], HOLLOW, hollow);
            let offset = if right { voice.right } else { voice.left };
            let points: Vec<Pos2> = (0..=columns).map(|i| Pos2::new(x_of(i), middle - sweep.tap(offset, age_of(i)) * half)).collect();
            let (base, lit, width) = if right { (26.0, 70.0, thick * 0.6) } else { (60.0, 195.0, thick) };
            let width = width * z;
            // One path per strand, its brightness following the light along it,
            // so its joints never bead
            let now = points[0];
            let (right_edge, span, gain, light) = (plot.right(), plot.width().max(1.0), voice.gain, Arc::clone(&light));
            let shade = move |_: egui::Rect, at: Pos2| {
                let along = ((right_edge - at.x) / span * (light.len() - 1) as f32).clamp(0.0, (light.len() - 1) as f32);
                let (i, frac) = (along as usize, along.fract());
                let glow = light[i] + (light[(i + 1).min(light.len() - 1)] - light[i]) * frac;
                with_alpha(color, ((base + lit * glow) * gain).min(255.0) as u8)
            };
            let hollow_core = (!right && hollow > 0.2).then(|| points.clone());
            painter.add(Shape::line(points, PathStroke::new_uv(width, shade)));
            if let Some(core) = hollow_core {
                painter.add(Shape::line(core, Stroke::new(width * 0.45, with_alpha(PANEL, (200.0 * hollow) as u8))));
            }
            // Where the left tap is now, at the right edge
            if !right && live.is_some() {
                painter.circle_filled(now, 3.0 * z, with_alpha(color, (60.0 * voice.gain) as u8));
                painter.circle_filled(now, 1.6 * z, with_alpha(Color32::WHITE, (220.0 * voice.gain) as u8));
            }
        }
    }

    // --- Words ---
    let left = format!("{} · {:.0} ms", rate_text(sweep.rate), delay_ms);
    let mut right = vec![if voices == 1 { "1 voice".to_string() } else { format!("{voices} voices") }];
    if feedback.abs() >= 0.05 {
        right.push(format!("FB {:+.0}%", feedback * 100.0));
    }
    header(
        &painter,
        rect,
        z,
        Some((left, if rate_moved { theme::signal::CONTROL } else { accent.gamma_multiply(0.9) })),
        Some((right.join(" · "), if hollow > 0.0 { HOLLOW } else { theme::text::SECONDARY })),
    );

    if live.is_some() {
        ui.ctx().request_repaint();
    }

    if response.hovered() {
        let mut text = format!(
            "{} sweeping {} at {}, {:.0}% either side of {:.0} ms",
            if voices == 1 { "One voice".to_string() } else { format!("{voices} voices") },
            if shape >= 0.5 { "triangles" } else { "sines" },
            rate_text(sweep.rate),
            sweep.depth * 100.0,
            delay_ms
        );
        if rate_moved {
            text.push_str(&format!("\nCV has moved the rate from {}", rate_text(knob_rate)));
        }
        text.push_str(&format!("\nThe last {WINDOW:.0} seconds, newest at the right: each strand is a voice's left tap, the faint one beside it its right tap"));
        if feedback >= 0.05 {
            text.push_str("\nFeedback thickens the strands");
        } else if feedback <= -0.05 {
            text.push_str("\nNegative feedback hollows them: the flanger's hollow sound");
        }
        text.push_str("\nThe strands light up where sound came in");
        response.on_hover_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_stays_inside_the_depth() {
        let sweep = Sweep::of_knobs(3.0, 0.4, 0.0, 4);
        for voice in sweep.voices {
            for i in 0..200 {
                let age = i as f32 * 0.01;
                assert!(sweep.tap(voice.left, age).abs() <= 0.4 + 1e-5);
                assert!(sweep.tap(voice.right, age).abs() <= 0.4 + 1e-5);
            }
        }
    }

    #[test]
    fn a_tap_runs_back_through_its_cycle() {
        // One cycle per second: a second ago the tap was where it is now
        let sweep = Sweep::of_knobs(1.0, 1.0, 0.0, 1);
        assert!((sweep.tap(0.0, 0.0) - sweep.tap(0.0, 1.0)).abs() < 1e-4);
        // A quarter cycle ago, the sine was at its bottom
        assert!((sweep.tap(0.0, 0.25) + 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_knobs_place_the_voices_as_the_module_does() {
        let sweep = Sweep::of_knobs(1.0, 0.5, 0.0, 2);
        assert_eq!(sweep.voices[0], Voice { gain: 1.0, left: 0.0, right: 0.25 });
        assert_eq!(sweep.voices[1], Voice { gain: 1.0, left: 0.5, right: 0.75 });
        assert_eq!(sweep.voices[2].gain, 0.0);
    }

    #[test]
    fn rates_read_plainly() {
        assert_eq!(rate_text(1.0), "1.0 Hz");
        assert_eq!(rate_text(0.25), "0.25 Hz");
    }
}
