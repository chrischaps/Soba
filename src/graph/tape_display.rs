//! The Tape's display: two reels and the tape running between them.
//!
//! The reels turn at the tape speed, and lurch with the wow, many times
//! exaggerated so the eye can follow what the ear hears as a slow sway. The
//! tape between the guides trembles with the flutter as it crosses the
//! head, and the head glows as Saturation drives it. Age fades the oxide
//! from a fresh dark brown to a dusty tan, and a dropout thins the tape
//! where it crosses the head. When the Stop gate brakes the transport, the
//! reels slow to a halt and the speed between them counts down to nothing.

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{self, Color32, Pos2, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::tape::{Tape, REEL_TURNS_PER_SECOND, SPEEDS};

use super::{SynthGraph, SynthGraphState, SynthValueType};

/// How many times over the reels show the read head's lag.
const LAG_EXAGGERATION: f32 = 25.0;

/// How far, in points, the tape between the guides trembles per cent of pitch.
const TREMBLE_PER_CENT: f32 = 0.12;

/// Draws the reels, as wide as the knob row below.
pub fn tape_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let Some(node) = graph.nodes.get(node_id) else { return };
    let value_of = |name: &str, default: f32| {
        node.inputs
            .iter()
            .find(|(input, _)| input == name)
            .and_then(|(_, id)| match graph.get_input(*id).value {
                SynthValueType::Number { value, .. } => Some(value),
                SynthValueType::Select { value, .. } => Some(value as f32),
                _ => None,
            })
            .unwrap_or(default)
    };
    let age = value_of("Age", 0.15).clamp(0.0, 1.0);
    let speed_index = (value_of("Speed", 1.0).max(0.0) as usize).min(SPEEDS.len() - 1);
    let speed = SPEEDS[speed_index];

    let readout = user_state
        .get_engine_node_id(node_id)
        .and_then(|id| user_state.readouts.get(&id))
        .copied()
        .unwrap_or_default();
    let values = readout.values;
    let lag = values[Tape::READOUT_LAG];
    let pitch = values[Tape::READOUT_PITCH];
    let drive = values[Tape::READOUT_DRIVE];
    let running = 1.0 - values[Tape::READOUT_BRAKE].clamp(0.0, 1.25);
    // Before the engine has said anything, the tape is whole
    let dropout = if values[Tape::READOUT_DROPOUT] > 0.0 { values[Tape::READOUT_DROPOUT] } else { 1.0 };
    let turns = values[Tape::READOUT_REEL] - lag * REEL_TURNS_PER_SECOND * speed.rate * LAG_EXAGGERATION;

    // As wide as the four knobs below
    let gap = ui.spacing().item_spacing.x;
    let width = 4.0 * 44.0 * z + 3.0 * gap - 8.0 * z;
    let height = 70.0 * z;

    // Separator, as the other displays have
    ui.add_space(4.0 * z);
    let accent = crate::dsp::ModuleCategory::Effect.color();
    let full = ui.available_rect_before_wrap();
    ui.painter().hline(
        (full.left() + 4.0 * z)..=(full.right() - 4.0 * z),
        ui.cursor().top(),
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64)),
    );
    ui.add_space(6.0 * z);

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0 * z, Color32::from_rgb(20, 22, 30));

    // Oxide: a glossy near-black brown when new, a dusty tan from the attic
    let oxide = lerp_color(Color32::from_rgb(58, 36, 24), Color32::from_rgb(132, 104, 72), age);
    let metal = Color32::from_rgb(150, 156, 170);

    let radius = height * 0.36;
    let reel_y = rect.top() + radius + 4.0 * z;
    let left = Pos2::new(rect.left() + width * 0.24, reel_y);
    let right = Pos2::new(rect.right() - width * 0.24, reel_y);

    // The tape path: off the bottom of each pack, round a guide, across the head
    let head = Pos2::new(rect.center().x, rect.bottom() - 9.0 * z);
    let guide_left = Pos2::new(left.x + radius * 0.55, head.y - 1.5 * z);
    let guide_right = Pos2::new(right.x - radius * 0.55, head.y - 1.5 * z);
    let tape_ink = Stroke::new(1.6 * z, oxide.gamma_multiply(1.4));
    let pack = radius * 0.78;
    painter.line_segment([left + Vec2::new(-pack * 0.2, pack * 0.98), guide_left], tape_ink);
    painter.line_segment([guide_right, right + Vec2::new(pack * 0.2, pack * 0.98)], tape_ink);

    // Across the head the tape trembles with the flutter, and thins in a dropout
    let tremble = (pitch.abs() * TREMBLE_PER_CENT * z).min(2.5 * z) * running.min(1.0);
    let segments = 24;
    let across: Vec<Pos2> = (0..=segments)
        .map(|s| {
            let t = s as f32 / segments as f32;
            let x = guide_left.x + (guide_right.x - guide_left.x) * t;
            let envelope = (t * std::f32::consts::PI).sin();
            let y = guide_left.y - 1.0 * z * envelope
                + tremble * envelope * (TAU * (3.0 * t + turns * 7.0)).sin();
            Pos2::new(x, y)
        })
        .collect();
    let thinned = tape_ink.color.gamma_multiply(0.35 + 0.65 * dropout);
    painter.add(Shape::line(across, Stroke::new(1.6 * z, thinned)));

    // The head, glowing as it's driven
    let heat = ((drive - 0.25) / 1.75).clamp(0.0, 1.0);
    let ember = Color32::from_rgb(255, 160, 64);
    if heat > 0.0 {
        painter.circle_filled(head, 9.0 * z, ember.gamma_multiply(0.18 * heat));
        painter.circle_filled(head, 5.0 * z, ember.gamma_multiply(0.30 * heat));
    }
    let head_rect = egui::Rect::from_center_size(head + Vec2::new(0.0, 2.5 * z), Vec2::new(8.0 * z, 5.0 * z));
    painter.rect_filled(head_rect, 1.5 * z, lerp_color(Color32::from_rgb(90, 94, 106), ember, heat * 0.8));
    for guide in [guide_left, guide_right] {
        painter.circle_filled(guide, 2.2 * z, metal.gamma_multiply(0.8));
    }

    // The reels: the pack of tape, then the flange with its three windows
    for (center, phase) in [(left, 0.0), (right, 0.37)] {
        painter.circle_filled(center, pack, oxide);
        // A faint line where the winding changed, as on a real pack
        painter.circle_stroke(center, pack * 0.82, Stroke::new(0.6 * z, oxide.gamma_multiply(0.75)));
        painter.circle_stroke(center, pack, Stroke::new(0.8 * z, oxide.gamma_multiply(1.3)));
        painter.circle_stroke(center, radius, Stroke::new(1.2 * z, metal.gamma_multiply(0.7)));
        let angle = TAU * (turns + phase);
        for w in 0..3 {
            let a = angle + TAU * w as f32 / 3.0 - FRAC_PI_2;
            let window = center + Vec2::angled(a) * radius * 0.5;
            painter.circle_filled(window, radius * 0.22, Color32::from_rgb(20, 22, 30));
            painter.circle_stroke(window, radius * 0.22, Stroke::new(0.8 * z, metal.gamma_multiply(0.5)));
        }
        painter.circle_filled(center, radius * 0.14, metal);
        painter.circle_filled(center, radius * 0.06, Color32::from_rgb(20, 22, 30));
    }

    // The speed, between the reels: while the Stop brakes it, the speed the
    // tape is really running at, reddening as it slows
    let braking = (running - 1.0).abs() > 0.005;
    let label = if braking {
        format!("{:.1}", 15.0 * speed.rate * running.max(0.0))
    } else {
        ["7½", "15", "30"][speed_index].to_string()
    };
    let ink = if braking {
        lerp_color(theme::text::SECONDARY, Color32::from_rgb(232, 88, 72), (1.0 - running).clamp(0.0, 1.0))
    } else {
        theme::text::SECONDARY
    };
    painter.text(
        Pos2::new(rect.center().x, reel_y - 4.0 * z),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::new(12.0 * z, egui::FontFamily::Name(theme::TITLE_FAMILY.into())),
        ink,
    );
    painter.text(
        Pos2::new(rect.center().x, reel_y + 8.0 * z),
        egui::Align2::CENTER_CENTER,
        "ips",
        egui::FontId::proportional(8.0 * z),
        theme::text::DISABLED,
    );

    let stopping = if !braking {
        String::new()
    } else if running <= 0.0 {
        " · stopped".to_string()
    } else {
        format!(" · running at {:.0}% speed", running * 100.0)
    };
    response.on_hover_text(format!(
        "{} ips · playing {:+.1} cents · head driven to {:.0}% of its level{stopping}",
        ["7½", "15", "30"][speed_index],
        pitch,
        drive * 100.0
    ));
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
