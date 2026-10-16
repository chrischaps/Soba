//! The Resonator's display: what's ringing, drawn as the thing itself.
//!
//! A **String** is drawn between its bridge and its nut, swinging. The
//! shape is one period of the loop, read from the voice as it plays, so the
//! line moves the way the sound does: ragged just after a pluck, smoothing
//! to a single arc as the top dies away. A haze either side shows how far
//! it swings, and an orange mark shows where it's plucked.
//!
//! A **Body** is drawn as its modes, a ring for each across a log-frequency
//! axis. At rest they're the dots a strike will ring, each as large as it
//! will sound. Struck, each blooms into a ring that shrinks and fades as
//! that mode dies, so a bell's high partials vanish first and its hum
//! lingers.

use std::f32::consts::PI;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::resonator::{self, Material, Mode, Resonator, MODE_COUNT, STRING_POINTS};

use super::drum_display::duration;
use super::oscillator_display::note_name;
use super::{SynthGraph, SynthGraphState, SynthValueType};

/// How long a pluck's flash lasts, in seconds.
const FLASH: f32 = 0.15;

/// The highest a mode is drawn, as a ratio of the note.
const WIDEST_RATIO: f32 = 24.0;

/// Draws the display, as wide as the four knobs a row below it.
pub fn resonator_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let Some(node) = graph.nodes.get(node_id) else { return };
    let value_of = |name: &str| {
        node.inputs.iter().find(|(input, _)| input == name).and_then(|(_, id)| match graph.get_input(*id).value {
            SynthValueType::Number { value, .. } => Some(value),
            SynthValueType::Select { value, .. } => Some(value as f32),
            _ => None,
        })
    };
    let mode = Mode::from_param(value_of("Mode").unwrap_or(0.0));
    let material = Material::from_param(value_of("Material").unwrap_or(0.0));
    let tune = value_of("Tune").unwrap_or(0.0);
    let decay = value_of("Decay").unwrap_or(0.5);
    let brightness = value_of("Brightness").unwrap_or(0.6);
    let position = value_of("Position").unwrap_or(0.3);
    let inharm = value_of("Inharm").unwrap_or(0.5);

    let readout = user_state
        .get_engine_node_id(node_id)
        .and_then(|id| user_state.readouts.get(&id))
        .copied()
        .unwrap_or_default();
    let values = &readout.values;
    let live = readout_matches(values[Resonator::READOUT_MODE], mode);
    let level = if live { values[Resonator::READOUT_LEVEL].clamp(0.0, 1.0) } else { 0.0 };
    let since = values[Resonator::READOUT_SINCE];
    let hz = if live && values[Resonator::READOUT_HZ] > 0.0 {
        values[Resonator::READOUT_HZ]
    } else {
        resonator::C4_HZ * (tune / 12.0).exp2()
    };

    let gap = ui.spacing().item_spacing.x;
    let width = 4.0 * 44.0 * z + 3.0 * gap - 8.0 * z;
    let height = 56.0 * z;

    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let blue = crate::dsp::ModuleCategory::Source.color();
    ui.painter().hline(rect.x_range(), rect.top() - 4.0 * z, Stroke::new(1.0 * z, with_alpha(blue, 64.0)));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0 * z, Color32::from_rgb(20, 22, 30));

    let pad = 3.0 * z;
    let plot = Rect::from_min_max(rect.min + Vec2::new(pad + 4.0 * z, pad + 10.0 * z), rect.max - Vec2::new(pad + 4.0 * z, pad + 2.0 * z));
    let flash = if since >= 0.0 && live { (-since / FLASH).exp() } else { 0.0 };
    let orange = theme::signal::CONTROL;

    match mode {
        Mode::String => {
            let shape: &[f32] = &values[Resonator::READOUT_SHAPE..][..STRING_POINTS];
            draw_string(&painter, plot, if live { shape } else { &[] }, level, flash, position, blue, orange, z);
        }
        Mode::Body => {
            let amps: &[f32] = &values[Resonator::READOUT_SHAPE..][..MODE_COUNT];
            draw_modes(&painter, plot, if live { amps } else { &[] }, hz, material, brightness, position, inharm, flash, blue, orange, z);
        }
    }
    if level > 1e-4 {
        ui.ctx().request_repaint();
    }

    // The note, and what rings it how long
    let small = egui::FontId::proportional(8.0 * z);
    let ring = resonator::decay_seconds(decay) * if mode == Mode::Body { material.ring() } else { 1.0 };
    let what = match mode {
        Mode::String => "String".to_string(),
        Mode::Body => material.name().to_string(),
    };
    painter.text(rect.left_top() + Vec2::new(4.0 * z, 2.0 * z), egui::Align2::LEFT_TOP, format!("{} · {} Hz", note_name(hz), hz.round()), small.clone(), orange.gamma_multiply(0.8));
    painter.text(rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z), egui::Align2::RIGHT_TOP, format!("{what} · {}", duration(ring)), small, theme::text::SECONDARY);

    if response.hovered() {
        let how = match mode {
            Mode::String => format!("A plucked string at {} ({:.1} Hz), ringing for {}", note_name(hz), hz, duration(ring)),
            Mode::Body => {
                let ratios = resonator::mode_ratios(material, inharm);
                let shown: Vec<String> = ratios.iter().take(5).map(|r| format!("{r:.2}")).collect();
                format!("A struck {} at {} ({:.1} Hz), ringing for {}\nModes at {} … times the note", material.name().to_lowercase(), note_name(hz), hz, duration(ring), shown.join(", "))
            }
        };
        let last = if since < 0.0 || !live { "Not played yet".to_string() } else { format!("Last played {} ago", duration(since)) };
        response.on_hover_text(format!("{how}\n{last}"));
    }
}

/// Whether the readout comes from the model the knobs show (a Mode just
/// changed hasn't reached the engine yet).
fn readout_matches(readout_mode: f32, mode: Mode) -> bool {
    (readout_mode > 0.5) == (mode == Mode::Body)
}

#[allow(clippy::too_many_arguments)]
fn draw_string(painter: &egui::Painter, plot: Rect, shape: &[f32], level: f32, flash: f32, position: f32, blue: Color32, orange: Color32, z: f32) {
    let mid = plot.center().y;
    let half = 0.46 * plot.height();
    let x_at = |u: f32| plot.left() + u * plot.width();
    // A string is pinned at both ends, so its swing goes to nothing there
    let pin = |u: f32| (PI * u).sin().max(0.0).powf(0.7);
    let swing = |v: f32| half * (3.0 * v).tanh();

    // The haze of how far it swings, widest in the middle
    let reach = swing(level.min(0.5));
    if reach > 0.3 * z {
        let columns = 40;
        let mut mesh = egui::Mesh::default();
        for i in 0..=columns {
            let u = i as f32 / columns as f32;
            let r = reach * pin(u);
            mesh.colored_vertex(Pos2::new(x_at(u), mid - r), with_alpha(blue, 10.0 + 24.0 * flash));
            mesh.colored_vertex(Pos2::new(x_at(u), mid), with_alpha(blue, 30.0 + 60.0 * flash));
            mesh.colored_vertex(Pos2::new(x_at(u), mid + r), with_alpha(blue, 10.0 + 24.0 * flash));
        }
        for i in 0..columns as u32 {
            let (a, b) = (3 * i, 3 * (i + 1));
            for k in 0..2 {
                mesh.add_triangle(a + k, a + k + 1, b + k);
                mesh.add_triangle(a + k + 1, b + k + 1, b + k);
            }
        }
        painter.add(Shape::mesh(mesh));
    }

    // The string, as it is now
    let points: Vec<Pos2> = if shape.is_empty() {
        vec![Pos2::new(x_at(0.0), mid), Pos2::new(x_at(1.0), mid)]
    } else {
        let n = shape.len();
        (0..=n + 1)
            .map(|i| {
                let u = i as f32 / (n + 1) as f32;
                let v = if i == 0 || i == n + 1 { 0.0 } else { shape[i - 1] };
                Pos2::new(x_at(u), mid - swing(v) * pin(u))
            })
            .collect()
    };
    let lit = blue.lerp_to_gamma(Color32::WHITE, 0.25 + 0.5 * flash);
    painter.add(Shape::line(points.clone(), Stroke::new(3.0 * z, with_alpha(blue, 40.0 + 120.0 * level.min(1.0) * 2.0))));
    painter.add(Shape::line(points, Stroke::new(1.1 * z, lit.gamma_multiply(0.55 + 0.45 * (4.0 * level).min(1.0)))));

    // The bridge and the nut
    for u in [0.0, 1.0] {
        let x = x_at(u);
        painter.line_segment([Pos2::new(x, mid - 6.0 * z), Pos2::new(x, mid + 6.0 * z)], Stroke::new(2.0 * z, Color32::from_gray(150)));
    }
    // Where it's plucked
    let place = resonator::strike_place(position);
    let x = x_at(place);
    painter.line_segment([Pos2::new(x, plot.bottom() - 4.0 * z), Pos2::new(x, plot.bottom())], Stroke::new(1.2 * z, orange.gamma_multiply(0.7 + 0.3 * flash)));
}

#[allow(clippy::too_many_arguments)]
fn draw_modes(
    painter: &egui::Painter,
    plot: Rect,
    amps: &[f32],
    hz: f32,
    material: Material,
    brightness: f32,
    position: f32,
    inharm: f32,
    flash: f32,
    blue: Color32,
    orange: Color32,
    z: f32,
) {
    let ratios = resonator::mode_ratios(material, inharm);
    let strikes = resonator::mode_amplitudes(&ratios, position, brightness);
    let loudest = strikes.iter().copied().fold(1e-6, f32::max);
    let mid = plot.center().y;
    let biggest = 0.5 * plot.height();
    // Room either side for the outermost rings to bloom whole
    let axis = plot.shrink2(Vec2::new(0.6 * biggest, 0.0));
    let x_of = |ratio: f32| axis.left() + (ratio.ln() / WIDEST_RATIO.ln()).clamp(0.0, 1.0) * axis.width();

    // The axis the modes sit on
    painter.hline(axis.x_range(), mid, Stroke::new(0.5 * z, Color32::from_rgba_unmultiplied(255, 255, 255, 16)));

    for (i, (&ratio, &strike)) in ratios.iter().zip(&strikes).enumerate() {
        // Modes past the top of hearing (or the drawing) aren't rung
        if ratio > WIDEST_RATIO || hz * ratio > 20_000.0 {
            continue;
        }
        let center = Pos2::new(x_of(ratio), mid);
        let share = strike / loudest;
        // The dot a strike will ring, as large as it'll sound
        let dot = (1.0 + 2.2 * share.sqrt()) * z;
        painter.circle_filled(center, dot, with_alpha(orange, 70.0 + 120.0 * share));

        // Ringing: a ring as wide as the mode is loud, fading with it
        let amp = amps.get(i).copied().unwrap_or(0.0);
        let bloom = (amp * 6.0).sqrt().min(1.0);
        if bloom > 0.02 {
            let radius = dot + bloom * biggest;
            painter.circle_stroke(center, radius, Stroke::new((0.8 + 1.2 * bloom) * z, with_alpha(blue, 40.0 + 190.0 * bloom)));
            painter.circle_filled(center, radius, with_alpha(blue, (14.0 + 30.0 * flash) * bloom));
        }
    }
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha.clamp(0.0, 255.0) as u8)
}
