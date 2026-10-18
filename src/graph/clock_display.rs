//! Where the patch is in the bar, and what's keeping time.
//!
//! The Clock's display is a bar of four beat lamps. The lamp of the beat
//! playing flashes as the beat lands and fades through it, the downbeat's a
//! little brighter, and a thin line sweeps under them across the bar. Dots
//! on that line mark where the pulses fall; with Swing, the late ones are
//! drawn in orange, pushed along towards the next pulse. Beside
//! them a badge says where the time comes from: INT for the Clock's own
//! Tempo, MIDI for a clock master, lit while its ticks are coming in, and
//! LINK for an Ableton Link session, lit with a count of its peers while
//! anyone else is in it.
//!
//! Knobs whose job another control has taken over are drawn still and
//! dimmed, reading what's actually in charge: an LFO's Rate shows its
//! division while it's synced, and the Clock's Tempo shows the master's
//! tempo while it follows MIDI. A Clock's Tempo with a cable in its jack
//! stays in hand, and shows the tempo the cable bends it to (see
//! [`super::knob_cv`]). An Attenuverter's Amount at Range ×4 stays in
//! hand too, and reads the gain it gives.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::Readout;
use crate::modules::attenuverter::Attenuverter;
use crate::dsp::tempo::Division;
use crate::modules::clock::{self, Clock};
use crate::modules::lfo::{Lfo, SYNC_DIVISIONS};

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// Beats in the bar the lamps show.
const BEATS_PER_BAR: usize = 4;

/// The LINK badge's colour: a teal of its own, apart from the signal
/// colours, as the session is no cable.
const LINK_TEAL: Color32 = Color32::from_rgb(72, 196, 184);

/// The value of the Select parameter `name` on a node, if it has one.
fn selected(graph: &SynthGraph, node_id: NodeId, name: &str) -> Option<usize> {
    let node = graph.nodes.get(node_id)?;
    let (_, input) = node.inputs.iter().find(|(input, _)| input == name)?;
    match graph.get_input(*input).value {
        SynthValueType::Select { value, .. } => Some(value),
        _ => None,
    }
}

/// The value of the Number parameter `name` on a node, if it has one.
pub(super) fn number(graph: &SynthGraph, node_id: NodeId, name: &str) -> Option<f32> {
    let node = graph.nodes.get(node_id)?;
    let (_, input) = node.inputs.iter().find(|(input, _)| input == name)?;
    match graph.get_input(*input).value {
        SynthValueType::Number { value, .. } => Some(value),
        _ => None,
    }
}

/// Where a clock's pulses fall in the bar, in beats, and whether each is
/// swung late: pulses `division` beats apart, every second one pushed to
/// `swing` (0.5-0.75) of its pair.
fn pulses_in_bar(division: f32, swing: f32) -> impl Iterator<Item = (f32, bool)> {
    let delay = (2.0 * swing - 1.0) * division;
    (0..)
        .map(move |n| {
            let late = n % 2 == 1;
            (n as f32 * division + if late { delay } else { 0.0 }, late && delay > 1e-4)
        })
        .take_while(|&(beat, _)| beat < BEATS_PER_BAR as f32)
}

/// Whether a Clock node follows MIDI.
fn follows_midi(graph: &SynthGraph, node_id: NodeId) -> bool {
    selected(graph, node_id, "Source") == Some(1)
}

/// Whether a Clock node plays in a Link session. In the plugin, the same
/// choice follows the DAW instead (see [`follows_host`]).
pub fn follows_link(graph: &SynthGraph, node_id: NodeId) -> bool {
    selected(graph, node_id, "Source") == Some(2) && !crate::hosted::is_hosted()
}

/// Whether a Clock node plays in time with the DAW, in the plugin.
pub fn follows_host(graph: &SynthGraph, node_id: NodeId) -> bool {
    selected(graph, node_id, "Source") == Some(2) && crate::hosted::is_hosted()
}

/// The latest readout from a node's module.
fn readout(user_state: &SynthGraphState, node_id: NodeId) -> Option<&Readout> {
    user_state.get_engine_node_id(node_id).and_then(|id| user_state.readouts.get(&id))
}

/// A knob another control has taken over, or whose reading something else
/// is moving.
pub struct KnobTakeover {
    /// Where the knob points, or `None` to leave it at its own value.
    pub value: Option<f32>,
    /// What it reads instead of its own value.
    pub text: String,
    /// Whether the knob is out of the player's hands: dimmed and still. A
    /// knob that's only reading something else can still be turned.
    pub locked: bool,
}

/// The takeover of a node's knob for `param`, if something else is in charge
/// of it right now.
pub fn knob_takeover(
    module_id: &str,
    param: &str,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
) -> Option<KnobTakeover> {
    let readout = readout(user_state, node_id);
    match (module_id, param) {
        ("mod.lfo", "Rate") => {
            let &Division { label, beats } = SYNC_DIVISIONS.get(selected(graph, node_id, "Tempo Sync")?)?;
            beats?;
            Some(KnobTakeover {
                value: readout.map(|r| r.values[Lfo::READOUT_RATE]).filter(|&hz| hz > 0.0),
                text: label.to_string(),
                locked: true,
            })
        }
        (clock::ID, "Tempo") if follows_midi(graph, node_id) => {
            let receiving = readout.is_some_and(|r| r.values[Clock::READOUT_RECEIVING] > 0.5);
            let tempo = readout.map(|r| r.values[Clock::READOUT_TEMPO]);
            Some(KnobTakeover {
                value: tempo.filter(|_| receiving),
                text: match tempo {
                    Some(bpm) if receiving => format!("{bpm:.1} BPM"),
                    _ => "No clock".to_string(),
                },
                locked: true,
            })
        }
        // The DAW's tempo, which only the DAW sets
        (clock::ID, "Tempo") if follows_host(graph, node_id) => {
            let tempo = readout.filter(|r| r.values[Clock::READOUT_RECEIVING] > 0.5).map(|r| r.values[Clock::READOUT_TEMPO]);
            Some(KnobTakeover {
                value: tempo,
                text: tempo.map_or("No host tempo".to_string(), |bpm| format!("{bpm:.1} BPM")),
                locked: true,
            })
        }
        // The session's tempo, which a peer may have set; turning the knob
        // proposes a new one
        (clock::ID, "Tempo") if follows_link(graph, node_id) => {
            let tempo = readout.map(|r| r.values[Clock::READOUT_TEMPO]).filter(|&bpm| bpm > 0.0)?;
            Some(KnobTakeover { value: None, text: format!("{tempo:.1} BPM"), locked: false })
        }
        ("seq.arranger", "Steps") => super::arranger_follow::steps_takeover(graph, user_state, node_id),
        // At ×4 the Amount knob reads the gain it gives, not its position
        ("util.attenuverter", "Amount") => {
            let scale = Attenuverter::range_scale(selected(graph, node_id, "Range")? as f32);
            (scale != 1.0).then(|| KnobTakeover {
                value: None,
                text: format!("{:.1}", number(graph, node_id, "Amount").unwrap_or(1.0) * scale),
                locked: false,
            })
        }
        _ => None,
    }
}

/// Draws the Clock's beat lamps and source badge.
pub fn clock_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let midi = follows_midi(graph, node_id);
    let link = follows_link(graph, node_id);
    let host = follows_host(graph, node_id);
    let readout = readout(user_state, node_id).copied().unwrap_or_default();
    let beat = readout.values[Clock::READOUT_BEAT].max(0.0);
    let running = readout.values[Clock::READOUT_RUNNING] > 0.5 && user_state.is_playing;
    let receiving = readout.values[Clock::READOUT_RECEIVING] > 0.5 && user_state.is_playing;
    let peers = readout.values[Clock::READOUT_PEERS].round() as usize;

    // Separator, as the other displays have, as wide as the display: the
    // node is no wider than its rows, which may be narrower than the space
    // on offer
    let width = 132.0 * z;
    ui.add_space(4.0 * z);
    let accent = crate::dsp::ModuleCategory::Utility.color();
    let left = ui.cursor().left();
    ui.painter().hline(
        left..=(left + width),
        ui.cursor().top(),
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64)),
    );
    ui.add_space(6.0 * z);

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 22.0 * z), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect.expand(2.0 * z));

    // --- The source badge ---
    let badge = Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 3.0 * z), Vec2::new(34.0 * z, 16.0 * z));
    let together = link && peers > 0 && user_state.is_playing;
    let (label, ink, fill, stroke) = if link {
        // Lit while anyone else is in the session
        let teal = LINK_TEAL;
        if together {
            ("LINK", Color32::WHITE, teal.gamma_multiply(0.85), teal)
        } else {
            ("LINK", teal, Color32::TRANSPARENT, teal.gamma_multiply(0.6))
        }
    } else if host {
        // Lit while the DAW plays
        if running {
            ("HOST", Color32::WHITE, accent.gamma_multiply(0.85), accent)
        } else {
            ("HOST", accent, Color32::TRANSPARENT, accent.gamma_multiply(0.6))
        }
    } else if midi {
        let purple = theme::signal::MIDI;
        if receiving {
            ("MIDI", Color32::WHITE, purple.gamma_multiply(0.85), purple)
        } else {
            ("MIDI", purple, Color32::TRANSPARENT, purple.gamma_multiply(0.6))
        }
    } else {
        ("INT", theme::text::SECONDARY, theme::background::WIDGET, theme::background::GRID_MAJOR)
    };
    painter.rect(badge, 8.0 * z, fill, Stroke::new(1.0 * z, stroke), egui::StrokeKind::Middle);
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::new(9.0 * z, egui::FontFamily::Name(theme::TITLE_FAMILY.into())),
        ink,
    );

    // How many others are in the session, in a bubble on the badge's corner
    if together {
        let centre = Pos2::new(badge.right() - 1.0 * z, badge.top() + 1.5 * z);
        painter.circle(centre, 5.5 * z, theme::background::MAIN, Stroke::new(1.0 * z, LINK_TEAL));
        painter.text(
            centre,
            egui::Align2::CENTER_CENTER,
            if peers > 9 { "9+".to_string() } else { peers.to_string() },
            egui::FontId::new(7.0 * z, egui::FontFamily::Name(theme::TITLE_FAMILY.into())),
            LINK_TEAL,
        );
    }

    // --- The bar: four lamps and the sweep under them ---
    let lamps = Rect::from_min_max(Pos2::new(badge.right() + 8.0 * z, rect.top() + 2.0 * z), Pos2::new(rect.right(), rect.bottom() - 6.0 * z));
    let gap = 4.0 * z;
    let lamp_width = (lamps.width() - gap * (BEATS_PER_BAR - 1) as f32) / BEATS_PER_BAR as f32;
    let current = (beat as usize).min(BEATS_PER_BAR - 1);
    let through = beat.fract();
    let green = theme::signal::GATE;
    for n in 0..BEATS_PER_BAR {
        let lamp = Rect::from_min_size(
            Pos2::new(lamps.left() + n as f32 * (lamp_width + gap), lamps.top()),
            Vec2::new(lamp_width, lamps.height()),
        );
        painter.rect_filled(lamp, 3.0 * z, theme::background::MAIN);
        if n == current && running {
            // Flash on the beat, fade through it; the downbeat flashes hardest
            let peak = if n == 0 { 1.0 } else { 0.75 };
            let glow = peak * (1.0 - 0.75 * through);
            painter.rect_filled(lamp, 3.0 * z, green.gamma_multiply(glow));
            if n == 0 {
                painter.rect_stroke(lamp.expand(1.0 * z), 4.0 * z, Stroke::new(1.0 * z, green.gamma_multiply(0.5 * glow)), egui::StrokeKind::Middle);
            }
        } else if n == current && readout != Readout::default() {
            // Stopped: where it'll carry on from, faintly
            painter.rect_stroke(lamp, 3.0 * z, Stroke::new(1.0 * z, green.gamma_multiply(0.35)), egui::StrokeKind::Middle);
        } else {
            let rim = if n == 0 { theme::background::GRID_MAJOR.gamma_multiply(1.6) } else { theme::background::GRID_MAJOR };
            painter.rect_stroke(lamp, 3.0 * z, Stroke::new(1.0 * z, rim), egui::StrokeKind::Middle);
        }
    }
    let sweep_y = rect.bottom() - 2.0 * z;
    painter.hline(lamps.x_range(), sweep_y, Stroke::new(1.0 * z, theme::background::GRID));
    if running || readout != Readout::default() {
        let x = lamps.left() + lamps.width() * (beat / BEATS_PER_BAR as f32).min(1.0);
        let ink = if running { green } else { green.gamma_multiply(0.35) };
        painter.hline(lamps.left()..=x, sweep_y, Stroke::new(1.5 * z, ink));
    }

    // --- Where the pulses fall: on the grid in grey, swung late in orange ---
    let division = Clock::division_beats(selected(graph, node_id, "Division").unwrap_or(2) as f32) as f32;
    let swing = number(graph, node_id, "Swing").unwrap_or(50.0) / 100.0;
    for (at, swung) in pulses_in_bar(division, swing) {
        let x = lamps.left() + lamps.width() * at / BEATS_PER_BAR as f32;
        let (ink, radius) = if swung { (theme::signal::CONTROL, 1.8) } else { (theme::text::SECONDARY.gamma_multiply(0.7), 1.2) };
        painter.circle_filled(Pos2::new(x, sweep_y), radius * z, ink);
    }

    let peer_text = match peers {
        0 => "no one else yet".to_string(),
        1 => "1 peer".to_string(),
        n => format!("{n} peers"),
    };
    let state = match (midi, running, receiving) {
        _ if link && running => format!("Playing in a Link session at {:.1} BPM, with {peer_text}", readout.values[Clock::READOUT_TEMPO]),
        _ if link => format!("In a Link session, stopped, with {peer_text}. Run starts every peer"),
        _ if host && running => format!("Playing in time with the DAW at {:.1} BPM", readout.values[Clock::READOUT_TEMPO]),
        _ if host => "Waiting for the DAW to play".to_string(),
        (true, true, _) => "Following MIDI clock".to_string(),
        (true, false, true) => "The MIDI clock master is stopped".to_string(),
        (true, false, false) => "Waiting for MIDI clock on the MIDI input".to_string(),
        (false, true, _) => format!("Keeping its own time at {:.1} BPM", readout.values[Clock::READOUT_TEMPO]),
        (false, false, _) => "Stopped".to_string(),
    };
    let swing_text = if swing > 0.5005 { format!("\nSwing {:.0}%", swing * 100.0) } else { String::new() };
    response.on_hover_text(format!("{state}\nBeat {} of the bar{swing_text}", current + 1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pulses_in_bar_mark_the_swung_ones() {
        let straight: Vec<_> = pulses_in_bar(0.25, 0.5).collect();
        assert_eq!(straight.len(), 16);
        assert!(straight.iter().all(|&(_, swung)| !swung));

        // Eighths at 75%: the off-beats land three quarters through each beat
        let swung: Vec<_> = pulses_in_bar(0.5, 0.75).collect();
        assert_eq!(swung, [(0.0, false), (0.75, true), (1.0, false), (1.75, true), (2.0, false), (2.75, true), (3.0, false), (3.75, true)]);

        // Whole notes: one pulse in the bar; its pair's other is in the next
        assert_eq!(pulses_in_bar(4.0, 0.66).collect::<Vec<_>>(), [(0.0, false)]);
    }
}
