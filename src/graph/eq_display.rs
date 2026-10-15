//! The 3-Band EQ's display: the curve its three bands add up to, and the
//! handles that move them.
//!
//! On a log-frequency axis from 20 Hz to 20 kHz, each band's own response
//! is a faint tinted fill from the 0 dB line (the low shelf warm, the mid
//! band violet, the high shelf cool), and their sum with the Output gain is
//! the bright curve over them. Every curve is the band's own biquad, so it
//! bends exactly as the sound does.
//!
//! Each band has a dot at its frequency and gain: drag it to move both, or
//! double-click it to flatten the band. The mid band has a wing either side
//! where its width ends; drag one to set Q. The knobs follow.
//!
//! Beneath the curves lies the sound itself (see [`super::filter_spectrum`]):
//! what comes in, a faint blue landscape, and what goes out over it, so a
//! boost or cut shows in the sound as well as in the curve.

use std::f32::consts::LN_2;

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::modules::ParametricEq;

use super::display_frame::{self as face, columns, curve, guide, header, hertz, value_of, with_alpha};
use super::drum_display::fill_under;
use super::filter_spectrum::{FilterSpectrum, BANDS, HIGHEST_HZ, LOWEST_HZ};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// The gain axis, in dB: a little past the knobs' ±15, so a dot at the
/// limit sits inside the panel.
const TOP_DB: f32 = 18.0;
const BOTTOM_DB: f32 = -18.0;

/// How tall the face is, before zoom: room for the dots to be dragged.
const HEIGHT: f32 = 72.0;

/// The sample rate the curves are drawn at. The engine's is 44.1 or 48 kHz;
/// the two differ only in the last few hundred hertz below 20 kHz.
const SAMPLE_RATE: f32 = 48_000.0;

/// How much of the sound's range the landscape spans, in dB, and how much
/// of the plot's height it rises to.
const LANDSCAPE_SPAN_DB: f32 = 48.0;
const LANDSCAPE_HEIGHT: f32 = 0.4;

/// How close the pointer must come to a dot or wing to take it, before zoom.
const GRAB: f32 = 7.0;

/// The knobs' ranges, which a drag stays inside.
const GAIN_LIMIT: f32 = 15.0;
const Q_RANGE: (f32, f32) = (0.1, 10.0);

/// One of the three bands.
struct Band {
    name: &'static str,
    freq: &'static str,
    gain: &'static str,
    /// The Freq knob's range, in Hz.
    range: (f32, f32),
    color: Color32,
}

const BANDS_OF_EQ: [Band; 3] = [
    Band { name: "Low", freq: "Low Freq", gain: "Low Gain", range: (20.0, 500.0), color: Color32::from_rgb(255, 170, 96) },
    Band { name: "Mid", freq: "Mid Freq", gain: "Mid Gain", range: (100.0, 10_000.0), color: Color32::from_rgb(196, 156, 255) },
    Band { name: "High", freq: "High Freq", gain: "High Gain", range: (2000.0, 20_000.0), color: Color32::from_rgb(110, 196, 255) },
];

/// The EQ's knobs, as the curves need them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    freq: [f32; 3],
    gain: [f32; 3],
    q: f32,
    output: f32,
}

impl Settings {
    fn of(graph: &SynthGraph, node_id: NodeId) -> Self {
        let defaults = [(100.0, 0.0), (1000.0, 0.0), (8000.0, 0.0)];
        let mut settings = Settings { freq: [0.0; 3], gain: [0.0; 3], q: 1.0, output: 0.0 };
        for (i, band) in BANDS_OF_EQ.iter().enumerate() {
            settings.freq[i] = value_of(graph, node_id, band.freq).unwrap_or(defaults[i].0);
            settings.gain[i] = value_of(graph, node_id, band.gain).unwrap_or(defaults[i].1);
        }
        settings.q = value_of(graph, node_id, "Mid Q").unwrap_or(1.0);
        settings.output = value_of(graph, node_id, "Output").unwrap_or(0.0);
        settings
    }

    /// Each band's response, in dB at a frequency.
    fn bands(&self) -> [Box<dyn Fn(f32) -> f32>; 3] {
        [
            Box::new(ParametricEq::low_shelf_response(SAMPLE_RATE, self.freq[0], self.gain[0])),
            Box::new(ParametricEq::peaking_response(SAMPLE_RATE, self.freq[1], self.gain[1], self.q)),
            Box::new(ParametricEq::high_shelf_response(SAMPLE_RATE, self.freq[2], self.gain[2])),
        ]
    }
}

/// How many octaves wide a peak of quality `q` is, between its half-gain points.
fn octaves_of(q: f32) -> f32 {
    2.0 / LN_2 * (1.0 / (2.0 * q.max(0.01))).asinh()
}

/// The quality of a peak `octaves` wide.
fn q_of(octaves: f32) -> f32 {
    1.0 / (2.0 * (LN_2 / 2.0 * octaves.max(0.01)).sinh())
}

/// A frequency rounded as a knob shows it: whole hertz under 1 kHz, then 10s.
fn round_hz(hz: f32) -> f32 {
    if hz < 1000.0 {
        hz.round()
    } else {
        (hz / 10.0).round() * 10.0
    }
}

/// "+4.0 dB", "-12.5 dB", "0 dB".
fn decibels(db: f32) -> String {
    if db.abs() < 0.05 {
        "0 dB".to_string()
    } else {
        format!("{db:+.1} dB")
    }
}

/// What a band is set to, in words.
fn describe(band: usize, settings: &Settings) -> String {
    let mut text = format!("{} {} {}", BANDS_OF_EQ[band].name, hertz(settings.freq[band]), decibels(settings.gain[band]));
    if band == 1 {
        text.push_str(&format!(" Q {:.1}", settings.q));
    }
    text
}

/// What a gesture on the face changes.
enum Grip {
    /// A band's dot: its frequency and gain.
    Dot(usize),
    /// One of the mid band's wings: its width.
    Wing,
}

/// Draws the display, as wide as the knob row below it, and returns the
/// knobs a drag on it has moved.
pub fn eq_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let settings = Settings::of(graph, node_id);
    let accent = ModuleCategory::Effect.color();

    let width = face::knob_row_width(ui, 8, z);
    let Some((rect, response, painter)) = face::frame_of_height(ui, width, HEIGHT * z, accent, z) else { return Vec::new() };
    let plot = face::plot(rect, z);
    let span = (HIGHEST_HZ / LOWEST_HZ).ln();
    let x_at = |hz: f32| plot.left() + (hz.max(1.0) / LOWEST_HZ).ln() / span * plot.width();
    let hz_at = |x: f32| LOWEST_HZ * ((x - plot.left()) / plot.width() * span).exp();
    let y_at = |db: f32| plot.top() + (TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB) * plot.height();
    let db_at = |y: f32| TOP_DB - (y - plot.top()) / plot.height() * (TOP_DB - BOTTOM_DB);

    for decade in [100.0, 1000.0, 10_000.0] {
        guide(&painter, Pos2::new(x_at(decade), plot.top()), Pos2::new(x_at(decade), plot.bottom()), 12, z);
    }
    for db in [-12.0, -6.0, 6.0, 12.0] {
        guide(&painter, Pos2::new(plot.left(), y_at(db)), Pos2::new(plot.right(), y_at(db)), 7, z);
    }
    let zero = y_at(0.0);
    guide(&painter, Pos2::new(plot.left(), zero), Pos2::new(plot.right(), zero), 30, z);

    // The sound coming in, and what goes out, behind it all
    let spectrum = user_state.get_engine_node_id(node_id).and_then(|id| user_state.spectra.get(id));
    if let Some(spectrum) = spectrum {
        spectrum.mark_drawn();
    }
    if let Some(spectrum) = spectrum.filter(|spectrum| spectrum.presence > 0.0) {
        let landscape = |bands: &[f32; BANDS]| -> Vec<Pos2> {
            let level = |band: usize| {
                let along = ((bands[band] - spectrum.reference + LANDSCAPE_SPAN_DB) / LANDSCAPE_SPAN_DB).clamp(0.0, 1.0);
                plot.bottom() - along * LANDSCAPE_HEIGHT * plot.height()
            };
            let mut points = Vec::with_capacity(BANDS + 2);
            points.push(Pos2::new(plot.left(), level(0)));
            points.extend((0..BANDS).map(|band| Pos2::new(x_at(FilterSpectrum::band_hz(band)), level(band))));
            points.push(Pos2::new(plot.right(), level(BANDS - 1)));
            points
        };
        let fade = |alpha: f32| (alpha * spectrum.presence).round() as u8;
        let blue = theme::signal::AUDIO;
        let coming_in = landscape(&spectrum.input);
        fill_under(&painter, &coming_in, plot.bottom(), with_alpha(blue, fade(40.0)), with_alpha(blue, fade(5.0)));
        curve(&painter, coming_in, with_alpha(blue, fade(70.0)), 0.7 * z, false);
        let going_out = landscape(&spectrum.output);
        curve(&painter, going_out, Color32::from_white_alpha(fade(70.0)), 0.7 * z, false);
        ui.ctx().request_repaint();
    }

    // Each band on its own, filled from the 0 dB line
    let bands = settings.bands();
    let columns = columns(plot, z);
    let along = |i: usize| LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(i as f32 / columns as f32);
    for (band, respond) in bands.iter().enumerate() {
        if settings.gain[band].abs() < 0.05 {
            continue;
        }
        let points: Vec<Pos2> = (0..=columns).map(|i| Pos2::new(x_at(along(i)), y_at(respond(along(i))))).collect();
        let color = BANDS_OF_EQ[band].color;
        fill_under(&painter, &points, zero, with_alpha(color, 64), with_alpha(color, 10));
        curve(&painter, points, with_alpha(color, 70), 0.6 * z, false);
    }

    // What they add up to, with the Output gain
    let total = |hz: f32| bands.iter().map(|respond| respond(hz)).sum::<f32>() + settings.output;
    let sum: Vec<Pos2> = (0..=columns).map(|i| Pos2::new(x_at(along(i)), y_at(total(along(i))))).collect();
    curve(&painter, sum, accent, 1.5 * z, true);

    // --- The handles ---
    let dot_at = |band: usize| Pos2::new(x_at(settings.freq[band]), y_at(settings.gain[band]));
    let half_width = octaves_of(settings.q) / 2.0;
    let wing_hz = [settings.freq[1] * 2f32.powf(-half_width), settings.freq[1] * 2f32.powf(half_width)];
    let wing_y = y_at(settings.gain[1] / 2.0);

    let mut grips: Vec<(Grip, egui::Response)> = Vec::new();
    for (side, &hz) in wing_hz.iter().enumerate() {
        let x = x_at(hz);
        if x > plot.left() && x < plot.right() {
            let hit = Rect::from_center_size(Pos2::new(x, wing_y), Vec2::new(GRAB * z, 2.0 * GRAB * z));
            grips.push((Grip::Wing, ui.interact(hit, ui.id().with(("eq_wing", node_id, side)), Sense::drag())));
        }
    }
    for band in 0..3 {
        let hit = Rect::from_center_size(dot_at(band), Vec2::splat(2.0 * GRAB * z));
        grips.push((Grip::Dot(band), ui.interact(hit, ui.id().with(("eq_dot", node_id, band)), Sense::click_and_drag())));
    }
    let active = grips.iter().find(|(_, r)| r.dragged()).or_else(|| grips.iter().find(|(_, r)| r.hovered()));
    let active_band = active.map(|(grip, _)| match grip {
        Grip::Dot(band) => *band,
        Grip::Wing => 1,
    });

    // The mid band's wings, lit while it's the one in hand
    let mid_lit = active_band == Some(1);
    for &hz in &wing_hz {
        let x = x_at(hz);
        if x > plot.left() && x < plot.right() {
            let color = with_alpha(BANDS_OF_EQ[1].color, if mid_lit { 220 } else { 90 });
            painter.line_segment([Pos2::new(x, wing_y - 4.0 * z), Pos2::new(x, wing_y + 4.0 * z)], Stroke::new(1.4 * z, color));
        }
    }
    for (band, info) in BANDS_OF_EQ.iter().enumerate() {
        let at = dot_at(band);
        let lit = active_band == Some(band);
        if lit {
            painter.circle_filled(at, 7.0 * z, with_alpha(info.color, 60));
        }
        painter.circle_filled(at, 3.6 * z, if lit { info.color } else { info.color.gamma_multiply(0.8) });
        painter.circle_stroke(at, 3.6 * z, Stroke::new(0.8 * z, with_alpha(Color32::BLACK, 120)));
    }

    // --- Gestures ---
    let mut changes: Vec<(&'static str, f32)> = Vec::new();
    for (grip, grip_response) in &grips {
        if grip_response.hovered() || grip_response.dragged() {
            ui.ctx().set_cursor_icon(match grip {
                Grip::Dot(_) if grip_response.dragged() => CursorIcon::Grabbing,
                Grip::Dot(_) => CursorIcon::Grab,
                Grip::Wing => CursorIcon::ResizeHorizontal,
            });
        }
        match grip {
            Grip::Dot(band) if grip_response.double_clicked() => changes.push((BANDS_OF_EQ[*band].gain, 0.0)),
            Grip::Dot(band) if grip_response.dragged() => {
                if let Some(pointer) = grip_response.interact_pointer_pos() {
                    let info = &BANDS_OF_EQ[*band];
                    let hz = round_hz(hz_at(pointer.x).clamp(info.range.0, info.range.1));
                    let gain = (db_at(pointer.y).clamp(-GAIN_LIMIT, GAIN_LIMIT) * 10.0).round() / 10.0;
                    changes.push((info.freq, hz));
                    changes.push((info.gain, gain));
                }
            }
            Grip::Wing if grip_response.dragged() => {
                if let Some(pointer) = grip_response.interact_pointer_pos() {
                    let octaves = 2.0 * (hz_at(pointer.x) / settings.freq[1]).log2().abs();
                    let q = (q_of(octaves).clamp(Q_RANGE.0, Q_RANGE.1) * 100.0).round() / 100.0;
                    changes.push(("Mid Q", q));
                }
            }
            _ => {}
        }
    }

    // --- Words ---
    let left = active_band.map(|band| (describe(band, &settings), BANDS_OF_EQ[band].color));
    let right = (settings.output.abs() >= 0.05).then(|| (format!("Out {}", decibels(settings.output)), theme::text::SECONDARY));
    header(&painter, rect, z, left, right);

    if active.is_none() && response.hovered() {
        let mut text = (0..3).map(|band| describe(band, &settings)).collect::<Vec<_>>().join("\n");
        if settings.output.abs() >= 0.05 {
            text.push_str(&format!("\nOutput {}", decibels(settings.output)));
        }
        text.push_str("\n\nDrag a dot to move its band, or double-click it to flatten it. Drag the mid band's wings to set its width (Q)");
        if spectrum.is_some_and(|spectrum| spectrum.presence > 0.0) {
            text.push_str("\nBeneath: the sound coming in (blue) and going out (line)");
        }
        response.on_hover_text(text);
    }

    changes
        .into_iter()
        .filter(|&(name, value)| value_of(graph, node_id, name).is_none_or(|now| (now - value).abs() > 1e-4))
        .map(|(name, value)| SynthResponse::ParameterChanged { node_id, param_name: name.to_string(), value })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> Settings {
        Settings { freq: [100.0, 1000.0, 8000.0], gain: [0.0; 3], q: 1.0, output: 0.0 }
    }

    fn total(settings: &Settings, hz: f32) -> f32 {
        settings.bands().iter().map(|respond| respond(hz)).sum::<f32>() + settings.output
    }

    #[test]
    fn a_flat_eq_draws_a_flat_line() {
        let settings = flat();
        for hz in [20.0, 200.0, 2000.0, 19_000.0] {
            assert!(total(&settings, hz).abs() < 1e-3);
        }
    }

    #[test]
    fn the_mid_dot_sits_on_the_curve() {
        let mut settings = flat();
        settings.gain[1] = 6.0;
        settings.freq[1] = 1500.0;
        assert!((total(&settings, 1500.0) - 6.0).abs() < 0.05);
        settings.output = -3.0;
        assert!((total(&settings, 1500.0) - 3.0).abs() < 0.05);
    }

    #[test]
    fn width_and_q_turn_into_each_other() {
        for q in [0.1, 0.7, 1.0, 1.41, 4.0, 10.0] {
            assert!((q_of(octaves_of(q)) - q).abs() < 1e-3 * q, "{q}");
        }
        // Q 1.41 is about an octave wide
        assert!((octaves_of(1.414) - 1.0).abs() < 0.01);
    }

    #[test]
    fn the_wings_sit_at_half_gain() {
        // Where the drawn width ends, the peak has fallen to half its gain in dB
        let mut settings = flat();
        settings.gain[1] = 12.0;
        settings.q = 2.0;
        let half = octaves_of(settings.q) / 2.0;
        let at = total(&settings, 1000.0 * 2f32.powf(half));
        assert!((at - 6.0).abs() < 1.0, "{at}");
    }

    #[test]
    fn frequencies_round_as_the_knobs_show_them() {
        assert_eq!(round_hz(123.4), 123.0);
        assert_eq!(round_hz(1234.0), 1230.0);
        assert_eq!(decibels(4.0), "+4.0 dB");
        assert_eq!(decibels(0.01), "0 dB");
    }
}
