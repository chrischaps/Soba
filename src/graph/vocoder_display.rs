//! The Vocoder's display: the voice's spectrum, a bar per band.
//!
//! Bars stand on a log-frequency axis from the lowest band to the top of the
//! sibilance range, each as tall as the voice is loud in that band. Odd bands
//! go left and even bands right, so as Width opens their two colours part.
//! When Formant moves the carrier's bands, each bar leaves a ghost cap where
//! the carrier will play it, joined to it at the foot: the vowel, carried up
//! or down. The highs Sibilance lets through are a shaded strip on the right,
//! and while the voice hisses, noise glitters over the bank in proportion to
//! how much Unvoiced lets it in.
//!
//! The bars' feet sit on the carrier's slope as the bank plays it, so a dull
//! carrier's feet fall to the right; as Flatten evens it out they level, and a
//! faint line stays behind on the slope the carrier came in with. Frozen,
//! the bars turn solid and icy, and a snowflake sits by the band count.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::vocoder::{band_centre, Vocoder, BANK_OCTAVES, LOWEST_BAND, MAX_BANDS};

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// The display's frequency axis, in octaves above the lowest band: a little
/// below it, and up past the sibilance high-pass.
const AXIS_FROM: f32 = -0.4;
const AXIS_TO: f32 = BANK_OCTAVES + 1.4;

/// The bars' scale, in dBFS.
const FLOOR_DB: f32 = -60.0;
const TOP_DB: f32 = -6.0;

/// Where the right-hand bands' colour turns, from the cyan of the left.
const RIGHT_TINT: Color32 = Color32::from_rgb(150, 160, 255);

/// Draws the display, as wide as the knob row below it.
pub fn vocoder_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let Some(node) = graph.nodes.get(node_id) else { return };
    let value_of = |name: &str| {
        node.inputs.iter().find(|(input, _)| input == name).and_then(|(_, id)| match graph.get_input(*id).value {
            SynthValueType::Number { value, .. } => Some(value),
            SynthValueType::Select { value, .. } => Some(value as f32),
            _ => None,
        })
    };

    let readout = user_state.get_engine_node_id(node_id).and_then(|id| user_state.readouts.get(&id)).copied();
    let bands = match readout {
        Some(r) if r.values[Vocoder::READOUT_COUNT] >= 1.0 => r.values[Vocoder::READOUT_COUNT] as usize,
        _ => [8, 16, 24][(value_of("Bands").unwrap_or(1.0).round().max(0.0) as usize).min(2)],
    }
    .min(MAX_BANDS);
    let formant = match readout {
        Some(r) => r.values[Vocoder::READOUT_FORMANT],
        None => value_of("Formant").unwrap_or(0.0),
    };
    let unvoiced = readout.map_or(0.0, |r| r.values[Vocoder::READOUT_UNVOICED]) * value_of("Unvoiced").unwrap_or(0.25);
    let width_knob = value_of("Width").unwrap_or(0.5).clamp(0.0, 1.0);
    let sibilance = value_of("Sibilance").unwrap_or(0.25).clamp(0.0, 1.0);
    let flatten = value_of("Flatten").unwrap_or(0.0).clamp(0.0, 1.0);
    let frozen = readout.is_some_and(|r| r.values[Vocoder::READOUT_FROZEN] > 0.5);
    // The carrier's slope, in dB an octave: its own, and as the bank plays it
    let (own_tilt, played_tilt) = readout.map_or((0.0, 0.0), |r| (r.values[Vocoder::READOUT_TILT], r.values[Vocoder::READOUT_EVENED]));

    // As wide as the three knobs below
    let gap = ui.spacing().item_spacing.x;
    let width = 3.0 * 44.0 * z + 2.0 * gap - 8.0 * z;
    let height = 62.0 * z;

    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let cyan = crate::dsp::ModuleCategory::Effect.color();
    ui.painter().hline(
        rect.x_range(),
        rect.top() - 4.0 * z,
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(cyan.r(), cyan.g(), cyan.b(), 64)),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0 * z, Color32::from_rgb(20, 22, 30));

    let pad = 4.0 * z;
    let plot = Rect::from_min_max(rect.min + Vec2::new(pad, pad + 9.0 * z), rect.max - Vec2::new(pad, pad + 7.0 * z));
    let x_at = |hz: f32| {
        let octave = (hz / LOWEST_BAND).log2();
        plot.left() + (octave - AXIS_FROM) / (AXIS_TO - AXIS_FROM) * plot.width()
    };
    let y_at = |amplitude: f32| {
        let db = 20.0 * amplitude.max(1e-6).log10();
        let t = ((db - FLOOR_DB) / (TOP_DB - FLOOR_DB)).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };

    // Octave gridlines, and where the sibilance strip begins
    for hz in [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0] {
        let x = x_at(hz);
        painter.vline(x, plot.y_range(), Stroke::new(0.5 * z, Color32::from_rgba_unmultiplied(255, 255, 255, 8)));
    }
    // The highs Sibilance passes: a faint zone, and the voice's own level
    // there, lit as far as the knob lets it through
    let sibilance_x = x_at(6000.0);
    let strip = Rect::from_min_max(Pos2::new(sibilance_x, plot.top()), plot.right_bottom());
    painter.rect_filled(strip, 0.0, Color32::from_rgba_premultiplied(5, 7, 9, 6));
    painter.vline(sibilance_x, plot.y_range(), Stroke::new(0.6 * z, Color32::from_rgba_premultiplied(20, 26, 30, 30)));
    // Frozen, the voice's own highs aren't heard at all
    let highs = if frozen { 0.0 } else { readout.map_or(0.0, |r| r.values[Vocoder::READOUT_HIGHS]) };
    let highs_top = y_at(highs);
    if highs_top < plot.bottom() - 0.5 {
        let column = Rect::from_min_max(Pos2::new(sibilance_x + 3.0 * z, highs_top), Pos2::new(plot.right() - 3.0 * z, plot.bottom()));
        let lit = 0.12 + 0.6 * sibilance;
        painter.rect_filled(column, 1.0 * z, Color32::from_rgba_unmultiplied(230, 240, 255, (60.0 * lit) as u8));
        painter.hline(column.x_range(), highs_top, Stroke::new(1.2 * z, Color32::from_rgba_unmultiplied(230, 240, 255, (230.0 * lit) as u8)));
    }

    // Each bar's colour: cyan to the left, violet to the right, parting as
    // Width opens
    let tint = |band: usize, alpha: f32| {
        let lean = if band.is_multiple_of(2) { 0.0 } else { width_knob };
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * lean) as u8;
        let c = Color32::from_rgb(mix(cyan.r(), RIGHT_TINT.r()), mix(cyan.g(), RIGHT_TINT.g()), mix(cyan.b(), RIGHT_TINT.b()));
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha * 255.0).clamp(0.0, 255.0) as u8)
    };

    let spacing = (x_at(band_centre(1, bands)) - x_at(band_centre(0, bands))).max(1.0);
    let bar = (spacing * 0.62).clamp(1.5 * z, 7.0 * z);
    let shift = 2f32.powf(formant);
    let shifted = formant.abs() > 0.02;

    // The feet lie along the carrier's slope, centred on the middle of the
    // bank: a slope of -2.5 dB an octave tips them 2.5 points at either end
    let foot_y = |centre: f32, slope: f32| {
        let octaves = (centre / LOWEST_BAND).log2() - BANK_OCTAVES / 2.0;
        plot.bottom() + 3.5 * z - (slope * octaves * z / 3.0).clamp(-2.5 * z, 2.5 * z)
    };
    // Where Flatten has evened the carrier, the slope it came in with stays
    // behind as a faint line
    if flatten > 0.02 && (own_tilt - played_tilt).abs() > 0.3 {
        let (from, to) = (band_centre(0, bands), band_centre(bands - 1, bands));
        painter.line_segment(
            [Pos2::new(x_at(from), foot_y(from, own_tilt)), Pos2::new(x_at(to), foot_y(to, own_tilt))],
            Stroke::new(0.6 * z, Color32::from_rgba_unmultiplied(cyan.r(), cyan.g(), cyan.b(), 50)),
        );
    }

    let mut lively = false;
    for band in 0..bands {
        let centre = band_centre(band, bands);
        let x = x_at(centre);
        let level = readout.map_or(0.0, |r| r.values[Vocoder::READOUT_BANDS + band]);
        let top = y_at(level);
        lively |= top < plot.bottom() - 0.5 || highs_top < plot.bottom() - 0.5;

        // The band's foot, always there, so a quiet vocoder still shows its bank
        let foot = Pos2::new(x, foot_y(centre, played_tilt));
        painter.circle_filled(foot, 1.0 * z, tint(band, 0.45));

        if top < plot.bottom() - 0.5 {
            let column = Rect::from_min_max(Pos2::new(x - bar / 2.0, top), Pos2::new(x + bar / 2.0, plot.bottom()));
            if frozen {
                // Held: solid, and touched with frost along the top
                painter.rect_filled(column, 1.0 * z, tint(band, 0.6));
                painter.line_segment([Pos2::new(x - bar / 2.0, top), Pos2::new(x + bar / 2.0, top)], Stroke::new(1.6 * z, FROST));
            } else {
                painter.rect_filled(column, 1.0 * z, tint(band, 0.28));
                painter.line_segment([Pos2::new(x - bar / 2.0, top), Pos2::new(x + bar / 2.0, top)], Stroke::new(1.6 * z, tint(band, 0.95)));
            }
        }

        // Where the carrier plays it, once Formant has moved it
        if shifted {
            let carried = x_at(centre * shift);
            if carried <= plot.right() {
                painter.line_segment(
                    [foot, Pos2::new(carried, plot.bottom())],
                    Stroke::new(0.6 * z, tint(band, 0.25)),
                );
                painter.line_segment(
                    [Pos2::new(carried - bar / 2.0, top), Pos2::new(carried + bar / 2.0, top)],
                    Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(255, 255, 255, if top < plot.bottom() - 0.5 { 110 } else { 30 })),
                );
            }
        }
    }

    // Noise standing in for the carrier while the voice hisses
    if unvoiced > 0.02 {
        let t = ui.input(|i| i.time) as f32;
        let frame = (t * 30.0) as u32;
        let mut seed = frame.wrapping_mul(2_654_435_761) ^ 0x9e37_79b9;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 10_000) as f32 / 10_000.0
        };
        let count = (40.0 * unvoiced) as usize;
        for _ in 0..count {
            let p = Pos2::new(plot.left() + next() * (sibilance_x - plot.left()), plot.top() + next() * plot.height());
            painter.circle_filled(p, 0.7 * z, Color32::from_rgba_unmultiplied(230, 240, 255, (60.0 + 120.0 * next() * unvoiced) as u8));
        }
        lively = true;
    }

    // The band count and the throat
    let small = egui::FontId::proportional(8.0 * z);
    let ink = theme::text::SECONDARY;
    let label = painter.text(rect.left_top() + Vec2::new(4.0 * z, 2.0 * z), egui::Align2::LEFT_TOP, format!("{bands} bands"), small.clone(), ink);
    if frozen {
        snowflake(&painter, Pos2::new(label.right() + 6.0 * z, label.center().y), 3.2 * z);
    }
    if shifted {
        painter.text(
            rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z),
            egui::Align2::RIGHT_TOP,
            format!("{:+.2} oct", formant),
            small.clone(),
            cyan,
        );
    }
    painter.text(
        Pos2::new(sibilance_x + 2.0 * z, rect.bottom() - 1.0 * z),
        egui::Align2::LEFT_BOTTOM,
        "s",
        egui::FontId::proportional(7.0 * z),
        ink.gamma_multiply(0.5 + 0.5 * sibilance),
    );

    if response.hovered() {
        let mut text = format!("{bands} bands, {:.0} Hz to {:.1} kHz", band_centre(0, bands), band_centre(bands - 1, bands) / 1000.0);
        if shifted {
            text.push_str(&format!("\nThe carrier's bands {} {:.2} octave", if formant > 0.0 { "up" } else { "down" }, formant.abs()));
        }
        text.push_str("\nBars: the voice's level in each band");
        if frozen {
            text.push_str(", frozen where they were");
        }
        if readout.is_some() {
            text.push_str(&format!("\nFeet: the carrier's slope, {played_tilt:+.1} dB an octave"));
            if flatten > 0.02 {
                text.push_str(&format!(" (evened from {own_tilt:+.1})"));
            }
        }
        response.on_hover_text(text);
    }

    if lively {
        crate::app::pace::animate(ui.ctx());
    }
}

/// Frozen bars' caps and the snowflake: an icy white.
const FROST: Color32 = Color32::from_rgb(214, 238, 255);

/// A six-armed snowflake of radius `r`, each arm with a pair of twigs.
fn snowflake(painter: &egui::Painter, centre: Pos2, r: f32) {
    let stroke = Stroke::new(0.11 * r, FROST);
    for arm in 0..6 {
        let angle = std::f32::consts::FRAC_PI_3 * arm as f32 + std::f32::consts::FRAC_PI_2;
        let along = Vec2::angled(angle);
        let tip = centre + along * r;
        painter.line_segment([centre, tip], stroke);
        let fork = centre + along * (0.55 * r);
        for side in [-1.0f32, 1.0] {
            let twig = Vec2::angled(angle + side * std::f32::consts::FRAC_PI_4) * (0.35 * r);
            painter.line_segment([fork, fork + twig], stroke);
        }
    }
}
