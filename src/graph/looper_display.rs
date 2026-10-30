//! The Looper's display: the loop as a ring, and its four footswitches.
//!
//! A loop is circular time, so it's drawn as a circle, read clockwise from
//! twelve o'clock like a record. The waveform runs round the ring and a
//! playhead sweeps it. Each overdub pass lays a band inside the ring, newer
//! ones brighter, like the rings of a tree, so you can see the layers
//! you've built and where each one reached.
//!
//! The ring's colour is the Looper's state: red while recording, amber
//! while overdubbing, green while playing, grey dashes when empty. The first
//! take draws its arc as it grows, and closes into a circle when the loop
//! does. The loop's length sits in the middle, in bars when clocked.
//!
//! The footswitches below do what the gates do. They're parameters, so a
//! MIDI footswitch can be learned onto them (right-click), and pressing one
//! plays the Looper rather than editing the patch: it isn't an undo step.

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::looper::{LoopState, Looper, OVERVIEW_SEGMENTS};

use super::{SynthGraph, SynthGraphState, SynthResponse, SynthValueType};
use crate::persistence::MidiControl;

/// The shortest a press of a footswitch on the node is held, in seconds, so
/// the audio thread sees it down however quick the click.
const MIN_PRESS_SECONDS: f64 = 0.06;

/// The most growth rings drawn per segment: past this they merge.
const MAX_RINGS: u16 = 6;

/// Red while recording, amber while overdubbing, green while playing.
fn state_color(state: LoopState) -> Color32 {
    match state {
        LoopState::Recording | LoopState::Armed => theme::accent::ERROR,
        LoopState::Overdubbing => theme::accent::WARNING,
        LoopState::Playing => theme::accent::SUCCESS,
        LoopState::Stopped => theme::accent::SUCCESS.gamma_multiply(0.45),
        LoopState::Empty => theme::text::DISABLED,
    }
}

/// What a footswitch is called now: Rec's changes with what it would do.
fn pedal_label(pedal: usize, state: LoopState, undo: f32) -> &'static str {
    match (pedal, state) {
        (0, LoopState::Recording) => "Play",
        (0, LoopState::Playing) => "Dub",
        (0, LoopState::Overdubbing) => "Play",
        (0, LoopState::Stopped) => "Play",
        (0, _) => "Rec",
        (1, LoopState::Stopped) => "Play",
        (1, _) => "Stop",
        (2, _) if undo >= 1.5 => "Redo",
        (2, _) => "Undo",
        _ => "Clear",
    }
}

/// Draws the ring and the footswitches, and returns what was pressed.
pub fn looper_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut responses = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return responses };
    let value_of = |name: &str| {
        node.inputs.iter().find(|(input, _)| input == name).map_or(0.0, |(_, id)| match graph.get_input(*id).value {
            SynthValueType::Number { value, .. } => value,
            SynthValueType::Toggle { value, .. } => f32::from(u8::from(value)),
            SynthValueType::Select { value, .. } => value as f32,
            _ => 0.0,
        })
    };
    let engine_node_id = user_state.get_engine_node_id(node_id);
    let readout = engine_node_id.and_then(|id| user_state.readouts.get(&id)).copied().unwrap_or_default();
    let v = readout.values;
    let state = LoopState::from_code(v[Looper::READOUT_STATE]);
    let overview = engine_node_id.and_then(|id| user_state.get_scope_data(id));
    if matches!(state, LoopState::Recording | LoopState::Playing | LoopState::Overdubbing | LoopState::Armed) {
        crate::app::pace::animate(ui.ctx());
    }

    // As wide as the knob row below: four 44-pt columns
    let gap = ui.spacing().item_spacing.x;
    let width = 4.0 * 44.0 * z + 3.0 * gap - 8.0 * z;
    let ring_height = 104.0 * z;
    let pedal_height = 34.0 * z;

    // Separator, as the other displays have
    ui.add_space(4.0 * z);
    let accent = crate::dsp::ModuleCategory::Utility.color();
    let full = ui.available_rect_before_wrap();
    ui.painter().hline(
        (full.left() + 4.0 * z)..=(full.right() - 4.0 * z),
        ui.cursor().top(),
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64)),
    );
    ui.add_space(6.0 * z);

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ring_height + pedal_height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return responses;
    }
    let painter = ui.painter_at(rect.expand(2.0 * z));
    let ring_rect = Rect::from_min_size(rect.min, Vec2::new(width, ring_height));
    let center = ring_rect.center();
    let color = state_color(state);

    // Where the ring is, and what a full turn stands for
    let radius = ring_height / 2.0 - 12.0 * z;
    let swing = 9.0 * z; // the waveform's reach either side of the ring
    let at = |turn: f32, r: f32| center + Vec2::angled(-FRAC_PI_2 + TAU * turn) * r;
    let has_loop = matches!(state, LoopState::Playing | LoopState::Overdubbing | LoopState::Stopped);
    let recording = state == LoopState::Recording;
    // While the first take records, the ring fills as it grows
    let drawn = if recording {
        (v[Looper::READOUT_SECONDS] / v[Looper::READOUT_SCALE].max(1e-3)).clamp(0.0, 1.0)
    } else if has_loop {
        1.0
    } else {
        0.0
    };

    // The record's surface: a faint disc the ring sits on
    painter.circle_filled(center, radius + swing + 3.0 * z, Color32::from_rgb(20, 22, 30));

    if drawn <= 0.0 {
        // Empty: grey dashes; armed, red ones that breathe until the pulse
        let dashes = 48;
        let ink = if state == LoopState::Armed {
            let t = ui.input(|i| i.time) as f32;
            color.gamma_multiply(0.45 + 0.35 * (t * TAU * 2.0).sin().abs())
        } else {
            theme::text::DISABLED.gamma_multiply(0.7)
        };
        for d in 0..dashes {
            let a = d as f32 / dashes as f32;
            let b = a + 0.5 / dashes as f32;
            painter.line_segment([at(a, radius), at(b, radius)], Stroke::new(1.2 * z, ink));
        }
    } else {
        // The track: the stretch of time the loop holds
        let steps = (96.0 * drawn).ceil().max(2.0) as usize;
        let track: Vec<Pos2> = (0..=steps).map(|k| at(drawn * k as f32 / steps as f32, radius)).collect();
        painter.add(Shape::line(track, Stroke::new(1.0 * z, color.gamma_multiply(0.5))));

        if let Some(data) = overview.filter(|d| d.channel1.len() == OVERVIEW_SEGMENTS) {
            let peaks = &data.channel1;
            let rings = &data.channel2;
            let tallest = peaks.iter().fold(0.05_f32, |m, &p| m.max(p));
            let shown = ((OVERVIEW_SEGMENTS as f32 * drawn).ceil() as usize).min(OVERVIEW_SEGMENTS);

            // The waveform round the ring, a wedge per segment
            let mut mesh = egui::Mesh::default();
            let wave_ink = color.gamma_multiply(if state == LoopState::Stopped { 0.5 } else { 0.85 });
            for (s, &peak) in peaks.iter().enumerate().take(shown) {
                let level = (peak / tallest).clamp(0.0, 1.0).sqrt();
                let reach = (0.6 * z).max(level * swing);
                let (a0, a1) = (s as f32 / OVERVIEW_SEGMENTS as f32, (s + 1) as f32 / OVERVIEW_SEGMENTS as f32);
                let base = mesh.vertices.len() as u32;
                for p in [at(a0, radius - reach), at(a0, radius + reach), at(a1, radius + reach), at(a1, radius - reach)] {
                    mesh.colored_vertex(p, wave_ink);
                }
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
            painter.add(Shape::mesh(mesh));

            // Growth rings: a band inside the ring for every overdub pass
            // over each stretch, the newest brightest
            let band = 2.4 * z;
            let inner = radius - swing - 3.0 * z;
            let deepest = rings.iter().fold(0.0_f32, |m, &r| m.max(r)).min(MAX_RINGS as f32) as usize;
            for k in 0..deepest {
                let r = inner - k as f32 * band;
                let bright = 0.25 + 0.6 * (k + 1) as f32 / deepest.max(1) as f32;
                let ink = theme::accent::WARNING.gamma_multiply(bright);
                // Runs of segments this many passes deep
                let mut s = 0;
                while s < OVERVIEW_SEGMENTS {
                    if (rings[s] as usize).min(MAX_RINGS as usize) <= k {
                        s += 1;
                        continue;
                    }
                    let start = s;
                    while s < OVERVIEW_SEGMENTS && (rings[s] as usize).min(MAX_RINGS as usize) > k {
                        s += 1;
                    }
                    let (a0, a1) = (start as f32 / OVERVIEW_SEGMENTS as f32, s as f32 / OVERVIEW_SEGMENTS as f32);
                    let points = ((a1 - a0) * 96.0).ceil().max(1.0) as usize;
                    let arc: Vec<Pos2> = (0..=points).map(|p| at(a0 + (a1 - a0) * p as f32 / points as f32, r)).collect();
                    painter.add(Shape::line(arc, Stroke::new(1.3 * z, ink)));
                }
            }
        }

        // The playhead, sweeping clockwise; while recording, the growing
        // edge of the take
        let turn = if recording { drawn } else { v[Looper::READOUT_PHASE] };
        if state != LoopState::Stopped {
            let inner = radius - swing - 2.0 * z;
            let outer = radius + swing + 2.0 * z;
            painter.line_segment([at(turn, inner), at(turn, outer)], Stroke::new(3.0 * z, color.gamma_multiply(0.25)));
            painter.line_segment([at(turn, inner), at(turn, outer)], Stroke::new(1.2 * z, theme::text::PRIMARY));
            painter.circle_filled(at(turn, outer + 1.5 * z), 1.8 * z, color);
        }
    }

    // The middle: how long, and what it's doing
    let seconds = v[Looper::READOUT_SECONDS];
    let bars = v[Looper::READOUT_BARS];
    let length = match state {
        LoopState::Empty => "–".to_string(),
        LoopState::Armed => "…".to_string(),
        _ if bars > 0.0 => {
            let n = bars.round();
            if (bars - n).abs() < 0.05 {
                format!("{} bar{}", n as i32, if n == 1.0 { "" } else { "s" })
            } else {
                format!("{bars:.1} bars")
            }
        }
        _ => format!("{seconds:.1} s"),
    };
    let caption = match state {
        LoopState::Empty => "tap Rec",
        LoopState::Armed => "on the beat",
        LoopState::Recording => "recording",
        LoopState::Playing => "playing",
        LoopState::Overdubbing => "overdub",
        LoopState::Stopped => "stopped",
    };
    let title_font = egui::FontId::new(12.0 * z, egui::FontFamily::Name(theme::TITLE_FAMILY.into()));
    painter.text(center - Vec2::new(0.0, 4.0 * z), egui::Align2::CENTER_CENTER, length, title_font, theme::text::PRIMARY);
    painter.text(center + Vec2::new(0.0, 9.0 * z), egui::Align2::CENTER_CENTER, caption, egui::FontId::proportional(8.5 * z), color);

    let layers = v[Looper::READOUT_LAYERS] as i32;
    if response.hover_pos().is_some_and(|pos| pos.distance(center) < radius + swing + 4.0 * z) {
        let mut tip = match state {
            LoopState::Empty => "Empty: tap Rec to record. With a Clock patched, recording starts on the beat".to_string(),
            LoopState::Armed => "Waiting for the next clock pulse to start recording".to_string(),
            LoopState::Recording => format!("Recording: {seconds:.1} s so far. Tap Rec to close the loop and play it"),
            _ => format!("A {seconds:.2} s loop, with {layers} overdub layer{}", if layers == 1 { "" } else { "s" }),
        };
        let offset = v[Looper::READOUT_OFFSET_MS];
        if offset.abs() >= 0.05 {
            tip.push_str(&format!("\nWrites land {offset:.1} ms earlier, to line up with what was heard"));
        }
        response.on_hover_text(tip);
    }

    // --- The footswitches ---
    let undo = v[Looper::READOUT_UNDO];
    let pedals_top = ring_rect.bottom();
    let pitch = width / 4.0;
    for (n, name) in Looper::PEDALS.iter().enumerate() {
        let param = format!("Pedal {name}");
        let param_index = Looper::PARAM_PEDALS + n;
        let cx = rect.left() + pitch * (n as f32 + 0.5);
        let disc_center = Pos2::new(cx, pedals_top + 10.0 * z);
        let disc = 8.5 * z;
        let hit = Rect::from_center_size(Pos2::new(cx, pedals_top + pedal_height / 2.0), Vec2::new(pitch - 2.0 * z, pedal_height));
        let id = ui.scope_id().with(("looper_pedal", node_id, n));
        let pedal = ui.interact(hit, id, Sense::click_and_drag());
        super::marks::mark(ui, node_id, format_args!("pedal:{name}"), hit);

        // Pressed from the node: down for at least MIN_PRESS_SECONDS
        let now = ui.input(|i| i.time);
        let pressed_at: Option<f64> = ui.ctx().data(|d| d.get_temp(id));
        let holding = pedal.is_pointer_button_down_on() || pedal.clicked();
        match pressed_at {
            None if holding => {
                ui.ctx().data_mut(|d| d.insert_temp(id, now));
                responses.push(SynthResponse::PlayParameter { node_id, param_name: param.clone(), value: 1.0 });
            }
            Some(at) if !pedal.is_pointer_button_down_on() => {
                let left = MIN_PRESS_SECONDS - (now - at);
                if left <= 0.0 {
                    ui.ctx().data_mut(|d| d.remove::<f64>(id));
                    responses.push(SynthResponse::PlayParameter { node_id, param_name: param.clone(), value: 0.0 });
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(left));
                }
            }
            _ => {}
        }
        // Down from the node or from a MIDI footswitch
        let down = pressed_at.is_some() || value_of(&param) >= 0.5;

        let lit = match n {
            0 => matches!(state, LoopState::Recording | LoopState::Overdubbing | LoopState::Armed),
            1 => state == LoopState::Stopped,
            2 => undo > 0.0,
            _ => state != LoopState::Empty,
        };
        let ink = match n {
            0 if state == LoopState::Overdubbing => theme::accent::WARNING,
            0 => theme::accent::ERROR,
            1 => theme::accent::SUCCESS,
            _ => theme::text::SECONDARY,
        };
        draw_pedal(&painter, disc_center, disc, n, PedalLook { ink, lit, down, hovered: pedal.hovered() }, z);
        painter.text(
            Pos2::new(cx, pedals_top + 25.0 * z),
            egui::Align2::CENTER_CENTER,
            pedal_label(n, state, undo),
            egui::FontId::proportional(8.0 * z),
            if lit { ink } else { theme::text::SECONDARY },
        );

        // A learned MIDI footswitch or pad shows a badge
        let mapping = engine_node_id.and_then(|eid| user_state.get_midi_mapping(eid, param_index));
        let learning = engine_node_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, param_index));
        if mapping.is_some() || learning {
            let badge = disc_center + Vec2::new(disc * 0.85, -disc * 0.85);
            let purple = theme::signal::MIDI;
            let alpha = if learning { 0.4 + 0.6 * ((now * 4.0).sin().abs() as f32) } else { 1.0 };
            painter.circle_filled(badge, 2.6 * z, purple.gamma_multiply(alpha));
            if learning {
                crate::app::pace::animate(ui.ctx());
            }
        }

        let tip = match (n, mapping) {
            (_, Some(m)) => format!("{}: MIDI {} presses it. Right-click to change", pedal_tip(n), m.text()),
            _ => format!("{}. Right-click to learn a MIDI footswitch or pad", pedal_tip(n)),
        };
        // In Map Mode a click maps the footswitch instead of pressing it (#426)
        let learn = engine_node_id.map(|eid| SynthResponse::midi_learn_start(eid, param_index, param.clone(), 0.0, 1.0, MidiControl::Button));
        let ring = super::map_mode::Outline::Ring { center: disc_center, radius: disc + 2.5 * z };
        let pedal = match learn.and_then(|learn| super::map_mode::catch(ui, hit, ring, &learn, user_state)) {
            Some(caught) => {
                responses.extend(caught.learn);
                caught.response
            }
            None => pedal,
        };
        let pedal = pedal.on_hover_text(tip);
        crate::widgets::menu::context_menu(&pedal, |ui| {
            let Some(eid) = engine_node_id else { return };
            if learning {
                if ui.button("Cancel MIDI Learn").clicked() {
                    responses.push(SynthResponse::MidiLearnCancel);
                    ui.close();
                }
                return;
            }
            let learn = if mapping.is_some() { "Re-learn MIDI" } else { "Learn MIDI" };
            if ui.button(learn).on_hover_text("Press a footswitch or hit a pad to press it with").clicked() {
                responses.push(SynthResponse::midi_learn_start(eid, param_index, param.clone(), 0.0, 1.0, MidiControl::Button));
                ui.close();
            }
            if mapping.is_some() && ui.button("Clear MIDI mapping").clicked() {
                responses.push(SynthResponse::midi_learn_clear(eid, param_index));
                ui.close();
            }
        });
    }

    ui.add_space(2.0 * z);
    responses
}

/// What each footswitch does, for its tooltip.
fn pedal_tip(pedal: usize) -> &'static str {
    match pedal {
        0 => "Rec: record, close the loop, then overdub and play in turn",
        1 => "Stop: stop the loop; again to play it from the top",
        2 => "Undo: take back the last overdub layer, or bring it back",
        _ => "Clear: empty the loop",
    }
}

/// How a pedal looks this frame: its colour, whether what it does is on,
/// and whether it's pressed or under the mouse.
#[derive(Clone, Copy)]
struct PedalLook {
    ink: Color32,
    lit: bool,
    down: bool,
    hovered: bool,
}

/// A footswitch: a metal disc with its symbol, lit when it's doing its job.
fn draw_pedal(painter: &egui::Painter, center: Pos2, radius: f32, pedal: usize, look: PedalLook, z: f32) {
    let PedalLook { ink, lit, down, hovered } = look;
    // A pressed pedal sinks a little
    let center = if down { center + Vec2::new(0.0, 0.8 * z) } else { center };
    if lit {
        painter.circle_filled(center, radius * 1.45, ink.gamma_multiply(0.14));
    }
    let face = if down {
        theme::background::WIDGET_ACTIVE
    } else if hovered {
        theme::background::WIDGET_HOVERED
    } else {
        theme::background::WIDGET
    };
    painter.circle_filled(center, radius, face);
    painter.circle_stroke(center, radius, Stroke::new(1.0 * z, theme::node::PORT_HIGHLIGHT.gamma_multiply(if hovered { 0.6 } else { 0.35 })));
    let symbol = if lit { ink } else { ink.gamma_multiply(0.75) };
    let s = radius * 0.42;
    match pedal {
        // Rec: a dot
        0 => {
            painter.circle_filled(center, s, symbol);
        }
        // Stop: a square
        1 => {
            painter.rect_filled(Rect::from_center_size(center, Vec2::splat(s * 1.6)), 0.6 * z, symbol);
        }
        // Undo: an arrow curling back
        2 => {
            let arc: Vec<Pos2> = (0..=12)
                .map(|k| center + Vec2::angled(-FRAC_PI_2 * 0.2 + TAU * 0.7 * k as f32 / 12.0) * s)
                .collect();
            let tip = arc[0];
            painter.add(Shape::line(arc, Stroke::new(1.3 * z, symbol)));
            painter.add(Shape::convex_polygon(
                vec![tip + Vec2::new(-s * 0.55, -s * 0.15), tip + Vec2::new(s * 0.45, -s * 0.45), tip + Vec2::new(s * 0.1, s * 0.55)],
                symbol,
                Stroke::NONE,
            ));
        }
        // Clear: a cross
        _ => {
            let d = s * 0.8;
            painter.line_segment([center + Vec2::new(-d, -d), center + Vec2::new(d, d)], Stroke::new(1.4 * z, symbol));
            painter.line_segment([center + Vec2::new(-d, d), center + Vec2::new(d, -d)], Stroke::new(1.4 * z, symbol));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rec_says_what_it_will_do() {
        assert_eq!(pedal_label(0, LoopState::Empty, 0.0), "Rec");
        assert_eq!(pedal_label(0, LoopState::Recording, 0.0), "Play");
        assert_eq!(pedal_label(0, LoopState::Playing, 0.0), "Dub");
        assert_eq!(pedal_label(0, LoopState::Overdubbing, 0.0), "Play");
        assert_eq!(pedal_label(1, LoopState::Stopped, 0.0), "Play");
        assert_eq!(pedal_label(2, LoopState::Playing, 2.0), "Redo");
    }
}
