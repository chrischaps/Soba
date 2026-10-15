//! The Dancer's face: a stick figure dancing on a little stage.
//!
//! The engine says where the dance is (`modules::dancer`) and
//! [`dancer_moves`](super::dancer_moves) poses the figure for it. The face
//! adds the stage: a floor that flashes on each beat as brightly as the
//! music is loud, a shadow that spreads when the figure lands and shrinks
//! when it leaves the ground, and a faint trail of where the limbs just
//! were. While stopped, or with no beat, the figure stands about, and it
//! eases between the two.
//!
//! The stage is as big as its hidden Size parameter says; the grip in its
//! corner drags it bigger. Everything moves on the engine's beat and on
//! egui's clock, which a capture steps, so a film of the Dancer is the same
//! every time.

use eframe::egui::{self, Color32, CursorIcon, Id, Mesh, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use super::dancer_moves::{self, Joints, Moment, Pose, HEAD};
use super::display_frame as face;
use super::{SynthGraph, SynthGraphState};
use crate::app::theme;
use crate::modules::dancer::{Dancer, MAX_SIZE, STYLES};

/// The stage at Size 1, before zoom: as wide as three knobs, and tall
/// enough for a figure you can read.
const HEIGHT: f32 = 116.0;

/// How long the figure takes to ease between dancing and standing about.
const EASE_SECONDS: f64 = 0.35;

/// Trail frames kept, and how far apart in time.
const TRAIL: usize = 6;
const TRAIL_EVERY: f64 = 0.035;

/// Each style's colour: the glow round the figure and the floor's flash.
fn hue(style: usize) -> Color32 {
    match style {
        1 => Color32::from_rgb(236, 72, 190),  // Disco: magenta
        2 => Color32::from_rgb(77, 208, 225),  // Robot: cyan
        3 => Color32::from_rgb(239, 83, 80),   // Headbang: red
        _ => Color32::from_rgb(255, 183, 77),  // Bounce: amber
    }
}

/// The figure's own colour: warm white, like a spotlight on it.
const CHALK: Color32 = Color32::from_rgb(246, 238, 222);

/// What the face remembers from frame to frame.
#[derive(Clone, Default)]
struct Stage {
    /// egui's time last frame, to ease by.
    time: f64,
    /// How far into dancing the figure is: 0 standing about, 1 dancing.
    dancing: f32,
    /// The last moment the engine danced, held while easing out of it.
    last: Option<Moment>,
    /// Where the joints were lately, newest last, and when the last was kept.
    trail: Vec<[Vec2; 12]>,
    trail_time: f64,
}

/// Draws the stage; returns a new Size when its grip is dragged.
pub fn dancer_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Option<(String, f32)> {
    let z = zoom;
    let size = face::value_of(graph, node_id, "Size").unwrap_or(1.0).clamp(1.0, MAX_SIZE);
    let style = (face::value_of(graph, node_id, "Style").unwrap_or(0.0).round().max(0.0) as usize).min(STYLES.len() - 1);
    let base = Vec2::new(face::knob_row_width(ui, 3, z), HEIGHT * z);

    ui.add_space(8.0 * z);
    let (rect, _) = ui.allocate_exact_size(base * size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let accent = crate::dsp::ModuleCategory::Utility.color();
    ui.painter().hline(rect.x_range(), rect.top() - 4.0 * z, Stroke::new(1.0 * z, face::with_alpha(accent, 64)));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0 * z, face::PANEL);

    // Where the dance is
    let time = ui.input(|i| i.time);
    let values = face::readout(user_state, node_id).map(|r| r.values);
    let value = |slot: usize| values.map_or(0.0, |v| v[slot]);
    let playing = user_state.is_playing;
    let live = playing && value(Dancer::READOUT_DANCING) > 0.5;
    let energy = if playing { value(Dancer::READOUT_ENERGY) } else { 0.0 };
    let moment = live.then(|| Moment {
        style,
        beat: value(Dancer::READOUT_MOVE).max(0.0) as u32,
        phase: value(Dancer::READOUT_PHASE),
        energy,
        punch: value(Dancer::READOUT_PUNCH),
        accent: (value(Dancer::READOUT_ACCENTS).max(0.0) as u32, value(Dancer::READOUT_ACCENT_AGE)),
    });

    let id = Id::new((node_id, "dancer-stage"));
    let mut stage: Stage = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    // A capture starting over winds egui's clock back: start the stage afresh
    if time < stage.time {
        stage = Stage::default();
    }
    let dt = (time - stage.time).clamp(0.0, 0.25);
    stage.time = time;
    let step = (dt / EASE_SECONDS) as f32;
    stage.dancing = if live { (stage.dancing + step).min(1.0) } else { (stage.dancing - step).max(0.0) };
    if let Some(m) = moment {
        stage.last = Some(Moment { style, ..m });
    }

    // Standing about, a dance, or part way between
    let idle = dancer_moves::idle(time, energy);
    let pose = match stage.last {
        Some(m) if stage.dancing > 0.0 => idle.lerp(&dancer_moves::dance(&Moment { style, ..m }), smooth(stage.dancing)),
        _ => idle,
    };
    let joints = dancer_moves::joints(&pose).turned(pose.turn);

    // The stage's geometry: the floor near the bottom, the figure as big as
    // fits above it
    let inner = Rect::from_min_max(rect.min + Vec2::new(4.0, 4.0 + face::HEADER) * z, rect.max - Vec2::splat(4.0 * z));
    let floor = inner.bottom() - 0.1 * inner.height();
    let bounds = dancer_moves::BOUNDS;
    let scale = ((floor - inner.top()) / bounds.max.y).min(inner.width() / bounds.width());
    let centre = inner.center().x;
    let at = |p: Vec2| Pos2::new(centre + p.x * scale, floor - p.y * scale);
    let colour = hue(style);

    // The floor, flashing on the beat
    let phase = stage.last.map_or(1.0, |m| m.phase);
    let flash = (1.0 - phase / 0.3).max(0.0).powi(2) * stage.dancing * (0.35 + 0.65 * energy);
    let punch = stage.last.map_or(0.0, |m| m.punch) * stage.dancing;
    let glow_height = 0.35 * (floor - inner.top());
    let mut mesh = Mesh::default();
    let top = face::with_alpha(colour, 0);
    let bottom = face::with_alpha(colour, (110.0 * (flash + 0.5 * punch).min(1.0)) as u8);
    mesh.colored_vertex(Pos2::new(rect.left(), floor - glow_height), top);
    mesh.colored_vertex(Pos2::new(rect.right(), floor - glow_height), top);
    mesh.colored_vertex(Pos2::new(rect.right(), floor), bottom);
    mesh.colored_vertex(Pos2::new(rect.left(), floor), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
    let floor_colour = lerp_colour(Color32::from_rgb(60, 64, 80), colour, (0.25 * energy + 0.6 * flash).min(1.0));
    painter.line_segment([Pos2::new(rect.left(), floor), Pos2::new(rect.right(), floor)], Stroke::new(1.0 * z * size.sqrt(), floor_colour));
    // Boards, faintly, receding from the edge of the stage
    for i in 1..4 {
        let y = floor + (rect.bottom() - floor) * (i as f32 / 4.0).powf(0.8);
        face::guide(&painter, Pos2::new(rect.left(), y), Pos2::new(rect.right(), y), 10, z);
    }

    // The shadow: wider as the hips drop, smaller and fainter in the air
    let air = joints.feet[0].y.min(joints.feet[1].y).max(0.0);
    let crouch = ((Pose::stand().hip.y - pose.hip.y) / 0.1).clamp(0.0, 1.0);
    let shadow_width = scale * (0.3 + 0.08 * crouch) / (1.0 + 4.0 * air);
    let shadow_alpha = (90.0 / (1.0 + 6.0 * air)) as u8;
    ellipse(&painter, Pos2::new(at(joints.hip).x, floor + 1.5 * z), shadow_width, 0.035 * scale, Color32::from_rgba_unmultiplied(0, 0, 0, shadow_alpha));

    // The trail: where the limbs just were, fading
    if time - stage.trail_time >= TRAIL_EVERY || stage.trail.is_empty() {
        stage.trail.push(joints.all());
        if stage.trail.len() > TRAIL {
            stage.trail.remove(0);
        }
        stage.trail_time = time;
    }
    let width = (0.034 * scale).max(1.0);
    let moving = stage.dancing;
    if moving > 0.0 {
        let n = stage.trail.len();
        for (age, past) in stage.trail.iter().enumerate().take(n.saturating_sub(1)) {
            let fade = (age + 1) as f32 / n as f32;
            let alpha = (60.0 * fade * fade * moving) as u8;
            limbs(&painter, &unpack(past), &at, Stroke::new(width * (0.5 + 0.5 * fade), face::with_alpha(colour, alpha)), HEAD * scale, Color32::TRANSPARENT);
        }
    }

    // The figure, glowing in its style's colour
    limbs(&painter, &joints, &at, Stroke::new(width * 3.2, face::with_alpha(colour, 46)), HEAD * scale, Color32::TRANSPARENT);
    limbs(&painter, &joints, &at, Stroke::new(width, CHALK), HEAD * scale, CHALK);

    // Its style, and the tempo it's dancing at: by ear, roughly
    let ear = value(Dancer::READOUT_EAR);
    let tempo = if live {
        let seconds = value(Dancer::READOUT_MOVE_SECONDS);
        let by_ear = if ear > 0.75 { "~" } else { "" };
        (seconds > 0.0).then(|| format!("{by_ear}{:.0} bpm", 60.0 / seconds))
    } else if playing && ear > 0.25 {
        Some("listening".to_string())
    } else if playing {
        Some("no beat".to_string())
    } else {
        Some("waiting".to_string())
    };
    face::header(
        &painter,
        rect,
        z,
        Some((STYLES[style].to_string(), colour.gamma_multiply(0.85))),
        tempo.map(|t| (t, theme::text::SECONDARY)),
    );

    // The grip: drag the corner to resize the stage
    let grip = Rect::from_min_max(rect.max - Vec2::splat(11.0 * z), rect.max);
    let response = ui.interact(grip, Id::new((node_id, "dancer-grip")), Sense::click_and_drag());
    let hot = response.hovered() || response.dragged();
    let ink = if hot { theme::text::PRIMARY } else { face::with_alpha(theme::text::SECONDARY, 110) };
    for i in 1..=2 {
        let d = 3.5 * z * i as f32;
        painter.line_segment([rect.max - Vec2::new(d, 2.0 * z), rect.max - Vec2::new(2.0 * z, d)], Stroke::new(1.0 * z, ink));
    }
    if hot {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
    }
    let mut changed = None;
    if response.double_clicked() {
        changed = Some(1.0);
    } else if response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let reach = (pointer - rect.min) / base;
            changed = Some((reach.x.max(reach.y).clamp(1.0, MAX_SIZE) * 20.0).round() / 20.0);
        }
    }
    let response = response.on_hover_text("Drag to make the stage bigger; double-click to put it back");

    ui.ctx().data_mut(|d| d.insert_temp(id, stage));
    // Standing about still breathes, so keep the frames coming, gently
    if live || response.dragged() {
        ui.ctx().request_repaint();
    } else {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    changed.filter(|&new| (new - size).abs() > 1e-3).map(|new| ("Size".to_string(), new))
}

/// 0 to 1, eased at both ends.
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp_colour(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A trail frame back into joints.
fn unpack(all: &[Vec2; 12]) -> Joints {
    let [head, neck, shoulders, hip, e0, e1, h0, h1, k0, k1, f0, f1] = *all;
    Joints { head, neck, shoulders, hip, elbows: [e0, e1], hands: [h0, h1], knees: [k0, k1], feet: [f0, f1] }
}

/// The figure's limbs as strokes, and its head as a ring `radius` across,
/// filled with `fill`.
fn limbs(painter: &egui::Painter, j: &Joints, at: &impl Fn(Vec2) -> Pos2, stroke: Stroke, radius: f32, fill: Color32) {
    let chain = |points: &[Vec2]| Shape::line(points.iter().map(|p| at(*p)).collect(), stroke);
    painter.add(chain(&[j.hip, j.neck]));
    for side in 0..2 {
        painter.add(chain(&[j.shoulders, j.elbows[side], j.hands[side]]));
        painter.add(chain(&[j.hip, j.knees[side], j.feet[side]]));
    }
    painter.circle(at(j.head), radius, fill, stroke);
}

/// A filled ellipse, `rx` across and `ry` down.
fn ellipse(painter: &egui::Painter, centre: Pos2, rx: f32, ry: f32, colour: Color32) {
    let points = (0..28)
        .map(|i| {
            let a = i as f32 / 28.0 * std::f32::consts::TAU;
            centre + Vec2::new(a.cos() * rx, a.sin() * ry)
        })
        .collect();
    painter.add(Shape::convex_polygon(points, colour, Stroke::NONE));
}
