//! The SVF and Ladder filters' display: what each output passes.
//!
//! Each curve is the filter's own small-signal response (the analog
//! prototype it's modelled on), on a log-frequency axis from 20 Hz to 20 kHz.
//! Patched outputs are drawn lit, the rest faint; with nothing patched, the
//! lowpass is lit. The curves are drawn at the cutoff and resonance the
//! engine reports, after CV, so an envelope into Cutoff visibly opens and
//! closes the filter; the knobs' own curve stays behind as a ghost. Past the
//! point where the filter rings on its own, its peak glows.
//!
//! Beneath the curves lies the sound itself (see [`super::filter_spectrum`]):
//! what comes in, a faint blue landscape, and what the lit output lets
//! through, green over it. The gap between them is what the filter takes
//! away, and where green rises above blue, the resonance is adding. It's
//! scaled to the loudest band coming in, so that band sits just under the
//! curve's unity line. With no sound, the curve's own glow fills beneath it.

use eframe::egui::{self, Pos2, Stroke};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::{LadderFilter, SvfFilter};

use super::display_frame::{self as face, columns, curve, guide, header, hertz, input_patched, output_patched, readout, value_of, with_alpha};
use super::drum_display::fill_under;
use super::filter_spectrum::{FilterSpectrum, BANDS, HIGHEST_HZ, LOWEST_HZ};
use super::filter_trail;
use super::{SynthGraph, SynthGraphState};

/// The level axis, in dB: room above 0 for a resonant peak.
const TOP_DB: f32 = 15.0;
const BOTTOM_DB: f32 = -36.0;

/// Where the loudest band coming in is drawn, in dB on the response's
/// scale: just under unity, so the passband rests on the sound's peaks.
const LANDSCAPE_TOP_DB: f32 = -4.0;

/// How tall the trail along the floor is, and the gap above it, before zoom.
const TRAIL_HEIGHT: f32 = 12.0;
const TRAIL_GAP: f32 = 1.5;

/// One of a filter's outputs: its port, and its response in dB at
/// (cutoff, resonance, frequency).
type Output = (&'static str, fn(f32, f32, f32) -> f32);

const SVF: [Output; 4] = [
    ("LowPass", SvfFilter::lowpass_response_db),
    ("HighPass", SvfFilter::highpass_response_db),
    ("BandPass", SvfFilter::bandpass_response_db),
    ("Notch", SvfFilter::notch_response_db),
];

const LADDER: [Output; 2] = [("LP24", LadderFilter::lowpass_response_db), ("LP12", LadderFilter::lp12_response_db)];

/// Which filter a display is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Svf,
    Ladder,
}

impl Filter {
    fn outputs(self) -> &'static [Output] {
        match self {
            Filter::Svf => &SVF,
            Filter::Ladder => &LADDER,
        }
    }

    /// Whether a resonance setting rings on its own.
    fn sings(self, resonance: f32) -> bool {
        match self {
            Filter::Svf => SvfFilter::small_signal_damping(resonance) < 0.0,
            Filter::Ladder => resonance >= LadderFilter::SELF_OSCILLATION,
        }
    }

    /// Where the readout keeps the cutoff and resonance after CV.
    fn readout_indices(self) -> (usize, usize) {
        match self {
            Filter::Svf => (SvfFilter::READOUT_CUTOFF, SvfFilter::READOUT_RESONANCE),
            Filter::Ladder => (LadderFilter::READOUT_CUTOFF, LadderFilter::READOUT_RESONANCE),
        }
    }
}

/// Draws the display, as wide as the knob row below it.
pub fn filter_display(ui: &mut egui::Ui, filter: Filter, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let knob_cutoff = value_of(graph, node_id, "Cutoff").unwrap_or(1000.0);
    let knob_resonance = value_of(graph, node_id, "Resonance").unwrap_or(0.5);
    let modulated = input_patched(graph, node_id, "Cutoff") || input_patched(graph, node_id, "Resonance");
    // What it's running at: the engine's word when CV is in play, else the knobs
    let (cutoff_index, resonance_index) = filter.readout_indices();
    let (cutoff, resonance) = readout(user_state, node_id)
        .filter(|_| modulated)
        .map(|r| (r.values[cutoff_index], r.values[resonance_index]))
        .filter(|&(hz, _)| hz > 0.0)
        .unwrap_or((knob_cutoff, knob_resonance));
    let swept = (cutoff / knob_cutoff).log2().abs() > 0.02;
    let moved = swept || (resonance - knob_resonance).abs() > 0.01;

    let outputs = filter.outputs();
    let patched: Vec<bool> = outputs.iter().map(|(port, _)| output_patched(graph, user_state, node_id, port)).collect();
    let any_patched = patched.iter().any(|&p| p);
    let lit = |index: usize| if any_patched { patched[index] } else { index == 0 };

    let green = crate::dsp::ModuleCategory::Filter.color();
    let width = face::knob_row_width(ui, 4, z);
    let Some((rect, response, painter)) = face::frame(ui, width, green, z) else { return };
    let mut plot = face::plot(rect, z);
    // With CV moving the filter, a strip along the floor for the trail it leaves
    let strip = modulated.then(|| {
        let strip = egui::Rect::from_min_max(Pos2::new(plot.left(), plot.bottom() - TRAIL_HEIGHT * z), plot.max);
        plot.max.y = strip.top() - TRAIL_GAP * z;
        strip
    });
    let x_at = |hz: f32| plot.left() + (hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln() * plot.width();
    let y_at = |db: f32| plot.top() + (TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB) * plot.height();

    for decade in [100.0, 1000.0, 10_000.0] {
        guide(&painter, Pos2::new(x_at(decade), plot.top()), Pos2::new(x_at(decade), plot.bottom()), 12, z);
    }
    guide(&painter, Pos2::new(plot.left(), y_at(0.0)), Pos2::new(plot.right(), y_at(0.0)), 26, z);

    // The sound coming in, and what the lit output lets through, behind it all
    let spectrum = user_state.get_engine_node_id(node_id).and_then(|id| user_state.spectra.get(id));
    let presence = spectrum.map_or(0.0, |spectrum| spectrum.presence);
    if let Some(spectrum) = spectrum {
        spectrum.mark_drawn();
    }
    if let Some(spectrum) = spectrum.filter(|_| presence > 0.0) {
        let landscape = |bands: &[f32; BANDS]| -> Vec<Pos2> {
            let level = |band: usize| y_at(bands[band] - spectrum.reference + LANDSCAPE_TOP_DB);
            let mut points = Vec::with_capacity(BANDS + 2);
            points.push(Pos2::new(plot.left(), level(0)));
            points.extend((0..BANDS).map(|band| Pos2::new(x_at(FilterSpectrum::band_hz(band)), level(band))));
            points.push(Pos2::new(plot.right(), level(BANDS - 1)));
            points
        };
        let fade = |alpha: f32| (alpha * presence).round() as u8;
        let blue = theme::signal::AUDIO;
        let coming_in = landscape(&spectrum.input);
        fill_under(&painter, &coming_in, plot.bottom(), with_alpha(blue, fade(46.0)), with_alpha(blue, fade(6.0)));
        curve(&painter, coming_in, with_alpha(blue, fade(80.0)), 0.7 * z, false);
        let going_out = landscape(&spectrum.output);
        fill_under(&painter, &going_out, plot.bottom(), with_alpha(green, fade(64.0)), with_alpha(green, fade(10.0)));
        curve(&painter, going_out, with_alpha(green, fade(110.0)), 0.7 * z, false);
        crate::app::pace::animate(ui.ctx());
    }

    // The last two seconds of it along the floor, falling away from the plot
    let trail = spectrum.map(|spectrum| &spectrum.trail).filter(|trail| !trail.is_dark());
    if let Some(strip) = strip {
        painter.rect_filled(strip, 1.0 * z, with_alpha(egui::Color32::BLACK, 70));
        if let Some(trail) = trail {
            trail.paint(&painter, strip, green, theme::signal::AUDIO);
            crate::app::pace::animate(ui.ctx());
        }
    }

    let columns = columns(plot, z);
    let response_points = |respond: fn(f32, f32, f32) -> f32, cutoff: f32, resonance: f32| -> Vec<Pos2> {
        (0..=columns)
            .map(|i| {
                let hz = LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(i as f32 / columns as f32);
                Pos2::new(x_at(hz), y_at(respond(cutoff, resonance, hz)))
            })
            .collect()
    };

    // Where the knobs alone would put the first lit output, when CV has moved it
    if moved {
        if let Some(&(_, respond)) = outputs.iter().enumerate().find(|&(index, _)| lit(index)).map(|(_, output)| output) {
            curve(&painter, response_points(respond, knob_cutoff, knob_resonance), with_alpha(green, 60), 0.9 * z, false);
        }
    }

    // The outputs nothing hears, faint, behind
    for (index, &(_, respond)) in outputs.iter().enumerate() {
        if !lit(index) {
            curve(&painter, response_points(respond, cutoff, resonance), with_alpha(green, 34), 0.8 * z, false);
        }
    }

    // Past self-oscillation, the peak glows where it rings
    let sings = filter.sings(resonance);
    if sings {
        let at = Pos2::new(x_at(cutoff), plot.top());
        painter.circle_filled(at, 9.0 * z, with_alpha(green, 40));
        painter.circle_filled(at, 4.5 * z, with_alpha(green, 70));
    }

    // The patched ones, lit, the first with its glow beneath
    let mut filled = false;
    for (index, &(_, respond)) in outputs.iter().enumerate() {
        if lit(index) {
            let points = response_points(respond, cutoff, resonance);
            // With no sound beneath it, the curve's glow fills in
            if !filled && presence < 1.0 {
                let fade = |alpha: f32| (alpha * (1.0 - presence)).round() as u8;
                fill_under(&painter, &points, plot.bottom(), with_alpha(green, fade(50.0)), with_alpha(green, fade(6.0)));
            }
            filled = true;
            curve(&painter, points, green, 1.5 * z, true);
        }
    }

    // A tick on the floor at the cutoff, orange while CV is moving it
    let orange = theme::signal::CONTROL;
    let tick = if swept { orange } else { green };
    let x = x_at(cutoff);
    painter.line_segment([Pos2::new(x, plot.bottom() - 4.0 * z), Pos2::new(x, plot.bottom())], Stroke::new(1.2 * z, tick));

    let res_moved = (resonance - knob_resonance).abs() > 0.01;
    let right = if sings {
        ("sings".to_string(), green)
    } else {
        (format!("Res {:.0}%", resonance * 100.0), if res_moved { orange } else { theme::text::SECONDARY })
    };
    header(&painter, rect, z, Some((hertz(cutoff), if swept { orange } else { green.gamma_multiply(0.9) })), Some(right));
    if modulated && user_state.is_playing {
        crate::app::pace::animate(ui.ctx());
    }

    if response.hovered() {
        let mut text = format!("Cutoff {}, resonance {:.0}%", hertz(cutoff), resonance * 100.0);
        if swept {
            text.push_str(&format!("\nCV has moved the cutoff from {}", hertz(knob_cutoff)));
        }
        if res_moved {
            text.push_str(&format!("\nCV has moved the resonance from {:.0}%", knob_resonance * 100.0));
        }
        if sings {
            text.push_str("\nRinging on its own at the cutoff");
        }
        let heard: Vec<&str> = outputs.iter().enumerate().filter(|&(i, _)| lit(i)).map(|(_, (port, _))| *port).collect();
        text.push_str(&format!("\nShowing {}", heard.join(" and ")));
        if !any_patched {
            text.push_str(" (nothing is patched yet)");
        }
        if presence > 0.0 {
            text.push_str(&format!("\nBeneath: the sound coming in (blue) and out of {} (green)", heard[0]));
        }
        if strip.is_some() {
            text.push_str(&format!("\nAlong the floor: the last {:.0} seconds of it, newest at the top", filter_trail::SECONDS));
        }
        response.on_hover_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_filter_lists_its_outputs_in_port_order() {
        let svf: Vec<&str> = Filter::Svf.outputs().iter().map(|(port, _)| *port).collect();
        assert_eq!(svf, ["LowPass", "HighPass", "BandPass", "Notch"]);
        let ladder: Vec<&str> = Filter::Ladder.outputs().iter().map(|(port, _)| *port).collect();
        assert_eq!(ladder, ["LP24", "LP12"]);
    }

    #[test]
    fn the_top_of_the_resonance_knob_sings() {
        for filter in [Filter::Svf, Filter::Ladder] {
            assert!(!filter.sings(0.5), "{filter:?} at half");
            assert!(filter.sings(1.0), "{filter:?} at full");
        }
    }
}
