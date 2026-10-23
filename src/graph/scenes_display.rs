//! The Scenes module's face: a pad per scene, A to H, and the glide between
//! two of them.
//!
//! A pad with a scene in it draws the scene as a row of ticks, one per knob,
//! as high as the knob sits round its travel (the picture Mutate's chips
//! draw). The From and To pads glow in proportion to how near Morph has
//! carried the knobs to each, so the light crossfades from one to the other
//! as it glides. The pad the knobs stand at, however they got there, is
//! ringed. Along the bottom, Morph rides a line from From to To.
//!
//! A pad can learn a MIDI pad or CC to recall its scene (#423), and wears
//! a purple badge once it has.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use egui_node_graph2::NodeId;

use crate::app::theme;

use super::display_frame;
use super::scenes::{self, SLOTS};
use super::{SceneAction, SynthGraph, SynthGraphState, SynthResponse};
use crate::persistence::MidiControl;

/// Pads in a row.
const COLUMNS: usize = 4;
/// A pad's height, before zoom.
const PAD_HEIGHT: f32 = 24.0;
/// The morph line's strip, before zoom.
const STRIP: f32 = 14.0;
/// Most ticks a pad draws: more knobs than this share them.
const MOST_TICKS: usize = 16;

/// Draws the pads, and returns what a click on one, or its menu, asked for.
pub fn scenes_display(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &mut SynthGraphState,
    zoom: f32,
) -> Option<SynthResponse> {
    let z = zoom;
    let node = graph.nodes.get(node_id)?;
    let bank = &node.user_data.scenes;
    let (morph, from, to) = scenes::morph_at(graph, user_state, node_id)?;
    // Morph's own orange, as the CV that drives it
    let accent = theme::signal::CONTROL;

    let width = display_frame::knob_row_width(ui, 3, z);
    let gap = 4.0 * z;
    let rows = SLOTS.div_ceil(COLUMNS);
    let height = display_frame::HEADER * z + 8.0 * z + rows as f32 * (PAD_HEIGHT * z + gap) + STRIP * z;
    let (rect, _, painter) = display_frame::frame_of_height(ui, width, height, accent, z)?;

    // What it reaches, along the top
    let scope = match node.user_data.parent {
        None => "PATCH".to_string(),
        Some(group) => super::groups::GroupIndex::of(graph)
            .node(group)
            .and_then(|n| graph.nodes.get(n))
            .map_or("GROUP".to_string(), |n| n.user_data.display_name.to_uppercase()),
    };
    let small = FontId::proportional(7.5 * z);
    painter.text(rect.left_top() + vec2(5.0 * z, 3.0 * z), Align2::LEFT_TOP, scope, small.clone(), theme::text::QUIET);
    painter.text(
        rect.right_top() + vec2(-5.0 * z, 3.0 * z),
        Align2::RIGHT_TOP,
        format!("{:.0}%", morph * 100.0),
        small.clone(),
        accent.gamma_multiply(0.8),
    );

    let inner = Rect::from_min_max(rect.min + vec2(5.0 * z, (display_frame::HEADER + 6.0) * z), rect.max - vec2(5.0 * z, STRIP * z));
    let pad_width = (inner.width() - gap * (COLUMNS - 1) as f32) / COLUMNS as f32;
    let pad_rect = |slot: usize| {
        let (column, row) = (slot % COLUMNS, slot / COLUMNS);
        Rect::from_min_size(
            pos2(inner.left() + column as f32 * (pad_width + gap), inner.top() + row as f32 * (PAD_HEIGHT * z + gap)),
            vec2(pad_width, PAD_HEIGHT * z),
        )
    };

    // How much of each end the knobs are at
    let weight = |slot: usize| {
        if from == to {
            if slot == from { 1.0 } else { 0.0 }
        } else if slot == from {
            1.0 - morph
        } else if slot == to {
            morph
        } else {
            0.0
        }
    };
    let current = (0..SLOTS)
        .find(|&slot| bank.get(scenes::letter(slot)).is_some_and(|scene| scenes::matches(graph, user_state, node_id, scene)));

    let engine_id = user_state.get_engine_node_id(node_id);
    let now = ui.input(|i| i.time);
    let mut clicked = None;
    for slot in 0..SLOTS {
        let pad = pad_rect(slot);
        let id = ui.scope_id().with(("scene_pad", node_id, slot));
        let response = ui.interact(pad, id, Sense::click());
        let scene = bank.get(scenes::letter(slot));
        let hovered = response.hovered();

        let fill = if hovered { theme::background::WIDGET_HOVERED } else { theme::background::WIDGET };
        painter.rect_filled(pad, 3.0 * z, if scene.is_some() { fill } else { fill.gamma_multiply(0.55) });
        let glow = weight(slot);
        if glow > 0.0 {
            painter.rect_filled(pad, 3.0 * z, accent.gamma_multiply(0.30 * glow));
        }
        let ring = if current == Some(slot) {
            Stroke::new(1.5 * z, theme::accent::PRIMARY)
        } else if glow > 0.0 {
            Stroke::new(1.0 * z, accent.gamma_multiply(0.35 + 0.65 * glow))
        } else {
            Stroke::new(1.0 * z, theme::background::WIDGET_ACTIVE)
        };
        painter.rect_stroke(pad, 3.0 * z, ring, egui::StrokeKind::Middle);

        let letter = scenes::letter(slot);
        match scene {
            Some(scene) => {
                painter.text(pad.left_top() + vec2(3.0 * z, 2.0 * z), Align2::LEFT_TOP, letter, small.clone(), theme::text::SECONDARY);
                let travels = scenes::travels(graph, user_state, node_id, scene);
                ticks(&painter, pad, &travels, z);
            }
            None => {
                let ink = if hovered { theme::text::SECONDARY } else { theme::text::DISABLED };
                let text = if hovered { "+" } else { letter };
                painter.text(pad.center(), Align2::CENTER_CENTER, text, FontId::proportional(9.0 * z), ink);
            }
        }

        // A MIDI pad or CC recalls it: a purple badge in its corner
        let mapping = engine_id.and_then(|eid| user_state.scene_midi_mappings.get(&(eid, slot)).cloned());
        let learning = engine_id.is_some_and(|eid| user_state.midi_learn_scene == Some((eid, slot)));
        if mapping.is_some() || learning {
            let alpha = if learning { 0.4 + 0.6 * (now * 4.0).sin().abs() as f32 } else { 1.0 };
            painter.circle_filled(pad.right_top() + vec2(-3.0 * z, 3.0 * z), 2.2 * z, theme::signal::MIDI.gamma_multiply(alpha));
            if learning {
                crate::app::pace::animate(ui.ctx());
            }
        }

        let mut tip = match scene {
            Some(scene) => format!(
                "Scene {letter} · {} knobs\nClick to recall it\nShift+click to keep the knobs here instead\nAlt+click to clear it",
                scene.values().map(|params| params.len()).sum::<usize>(),
            ),
            None => format!("Scene {letter} is empty: click to keep the knobs here"),
        };
        match &mapping {
            Some(mapping) => tip.push_str(&format!("\nMIDI {} recalls it. Right-click to change", mapping.text())),
            None => tip.push_str("\nRight-click to recall it from a MIDI pad"),
        }
        let response = response.on_hover_text(tip);
        if response.clicked() {
            let modifiers = ui.input(|i| i.modifiers);
            clicked = Some(match (scene.is_some(), modifiers.alt, modifiers.shift) {
                (true, true, _) => SynthResponse::Scene { node_id, slot, action: SceneAction::Clear },
                (true, false, false) => SynthResponse::Scene { node_id, slot, action: SceneAction::Recall },
                (_, false, _) => SynthResponse::Scene { node_id, slot, action: SceneAction::Store },
                (false, true, _) => continue,
            });
        }
        let menu = crate::widgets::menu::context_menu(&response, |ui| {
            let Some(eid) = engine_id else { return };
            if learning {
                if ui.button("Cancel MIDI Learn").clicked() {
                    clicked = Some(SynthResponse::MidiLearnCancel);
                    ui.close();
                }
                return;
            }
            let learn = SynthResponse::MidiLearnStart {
                engine_node_id: eid,
                param_index: 0,
                param_name: format!("Scene {letter}"),
                min_value: 0.0,
                max_value: 1.0,
                control: MidiControl::Button,
                scene: Some(slot as u8),
            };
            if let Some(chosen) = super::node_data::midi_items(ui, mapping.as_ref(), learn) {
                clicked = Some(chosen);
            }
        });
        if menu.is_some() {
            user_state.widget_context_menu_open = true;
        }
    }

    // Morph, riding a line from From to To
    let strip = Rect::from_min_max(pos2(inner.left(), inner.bottom()), pos2(inner.right(), rect.bottom()));
    let y = strip.center().y;
    let (left, right) = (strip.left() + 8.0 * z, strip.right() - 8.0 * z);
    painter.text(pos2(strip.left(), y), Align2::LEFT_CENTER, scenes::letter(from), small.clone(), accent);
    painter.text(pos2(strip.right(), y), Align2::RIGHT_CENTER, scenes::letter(to), small, accent);
    painter.line_segment([pos2(left + 2.0 * z, y), pos2(right - 2.0 * z, y)], Stroke::new(1.0 * z, theme::background::GRID_MAJOR.gamma_multiply(1.8)));
    let at = pos2(left + 2.0 * z + (right - left - 4.0 * z) * morph, y);
    painter.line_segment([pos2(left + 2.0 * z, y), at], Stroke::new(1.5 * z, accent.gamma_multiply(0.7)));
    painter.circle_filled(at, 4.0 * z, accent.gamma_multiply(0.25));
    painter.circle_filled(at, 2.2 * z, accent);

    clicked
}

/// A scene's knobs as a row of ticks along the bottom of its pad.
fn ticks(painter: &egui::Painter, pad: Rect, travels: &[f32], z: f32) {
    if travels.is_empty() {
        return;
    }
    let area = Rect::from_min_max(pad.min + vec2(11.0 * z, 4.0 * z), pad.max - vec2(3.0 * z, 3.0 * z));
    let count = travels.len().min(MOST_TICKS);
    // Many knobs share the ticks: each shows the knobs it stands for, averaged
    let per = travels.len().div_ceil(count);
    let step = area.width() / count as f32;
    let tick = (step * 0.6).min(2.5 * z);
    let ink: Color32 = theme::signal::CONTROL;
    for (i, group) in travels.chunks(per).enumerate() {
        let travel = group.iter().sum::<f32>() / group.len() as f32;
        let x = area.left() + i as f32 * step;
        let height = 1.5 * z + travel * (area.height() - 1.5 * z);
        painter.rect_filled(Rect::from_min_max(pos2(x, area.bottom() - height), pos2(x + tick, area.bottom())), 0.5 * z, ink);
    }
}
