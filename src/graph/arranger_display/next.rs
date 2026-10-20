//! A section's Next in the editor: where it may go when it ends, drawn as
//! faint arcs over the timeline and set in the section's menu.
//!
//! An arc leaves the end of a section and lands on the start of each place
//! it may go, fainter the less likely it is. Once the dice are thrown, on
//! the section's last bar, the arc to where they sent it lights green, so
//! the song shows where it's going a bar before it gets there. Anywhere is
//! a small "?" on the section's end.

use egui_node_graph2::NodeId;
use eframe::egui::{self, epaint::CubicBezierShape, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke};

use crate::app::theme;
use crate::modules::arranger::{Arranger as Arr, Next, To, Way};

use super::{arranger_follow, edit, position, section_name, Ctx, Layout, Song, SynthGraph, SynthResponse};

/// The words for where a section goes on as written: "on to Rain", "back
/// to Dust" (Loop) or "stop" (the end, with Loop off).
fn written(graph: &SynthGraph, node_id: NodeId, song: &Song, section: usize) -> String {
    if section + 1 < song.count() {
        format!("on to {}", section_name(graph, node_id, section + 1))
    } else {
        match Arr::loop_to(&song.values) {
            Some(to) => format!("back to {}", section_name(graph, node_id, to)),
            None => "stop".to_string(),
        }
    }
}

fn place(graph: &SynthGraph, node_id: NodeId, to: To) -> String {
    match to {
        To::Anywhere => "anywhere".to_string(),
        To::Section(section) => section_name(graph, node_id, section),
    }
}

fn percent(chance: f32) -> String {
    format!("{:.0}%", chance * 100.0)
}

/// Where a section may go, in words, if it may wander: "on to Rain 25% ·
/// Window 50% · anywhere 25%".
pub(super) fn describe(graph: &SynthGraph, node_id: NodeId, song: &Song, section: usize) -> Option<String> {
    let next = Arr::next(&song.values, section);
    if !next.wanders() {
        return None;
    }
    let chances = next.chances();
    let mut parts = Vec::new();
    if next.on > 0 {
        parts.push(format!("{} {}", written(graph, node_id, song, section), percent(chances[0])));
    }
    for (way, chance) in next.ways.iter().zip(&chances[1..]) {
        if way.weight > 0 {
            parts.push(format!("{} {}", place(graph, node_id, way.to), percent(*chance)));
        }
    }
    Some(parts.join(" · "))
}

/// A Next with each section it names moved to where `moves` says that
/// section is now; a way to one that's gone goes too.
pub(super) fn renumbered(value: f32, moves: &[Option<usize>]) -> f32 {
    let mut next = Next::decode(value);
    for way in &mut next.ways {
        if let To::Section(section) = way.to {
            *way = match moves.get(section).copied().flatten() {
                Some(now) => Way { to: To::Section(now), ..*way },
                None => Way::NONE,
            };
        }
    }
    if next == Next::decode(value) {
        value
    } else {
        next.encode()
    }
}

/// The arcs over the section blocks, from each section that may wander to
/// where it may go, and the one the dice chose lit.
pub(super) fn arcs(painter: &egui::Painter, cx: Ctx, layout: &Layout, blocks: Rect, z: f32) {
    let Ctx { node_id, user_state, song, .. } = cx;
    // A follower throws no dice: its leader's throw is where it's going
    let head = arranger_follow::head(&user_state.arranger_leaders, node_id);
    let thrown = position(user_state, head).filter(|p| p.started && !p.ended).and_then(|p| Some((p.section, p.next?)));
    let base = blocks.bottom() - 2.5 * z;
    let tallest = blocks.height() * 0.62;
    for section in 0..song.count() {
        let next = Arr::next(&song.values, section);
        let chosen = thrown.filter(|&(from, _)| from == section).map(|(_, to)| to);
        if !next.wanders() && chosen.is_none() {
            continue;
        }
        let from = layout.span(section).1 - 1.5 * z;
        let chances = next.chances();
        for (way, &chance) in next.ways.iter().zip(&chances[1..]) {
            match way.to {
                _ if way.weight == 0 => {}
                To::Section(to) if to < song.count() && chosen != Some(to) => {
                    let ink = theme::text::SECONDARY.gamma_multiply(0.22 + 0.5 * chance);
                    arc(painter, from, layout.span(to).0 + 1.5 * z, base, tallest, Stroke::new(1.0 * z, ink), z);
                }
                To::Section(_) => {}
                To::Anywhere => {
                    let lit = chosen.is_some_and(|to| !next.ways.iter().any(|w| w.weight > 0 && w.to == To::Section(to)));
                    let ink = if lit { theme::signal::GATE } else { theme::text::SECONDARY.gamma_multiply(0.35 + 0.5 * chance) };
                    // Inside the block's end, clear of the timeline's edge
                    let centre = Pos2::new(from - 4.5 * z, blocks.center().y);
                    painter.circle(centre, 4.0 * z, theme::background::PANEL, Stroke::new(0.8 * z, ink));
                    painter.text(centre, Align2::CENTER_CENTER, "?", FontId::proportional(6.5 * z), ink);
                }
            }
        }
        if let Some(to) = chosen {
            arc(painter, from, layout.span(to).0 + 1.5 * z, base, tallest, Stroke::new(1.6 * z, theme::signal::GATE), z);
        }
    }
}

/// An arc from `from` to `to` along `base`, rising higher the farther it
/// goes, with a head where it lands.
fn arc(painter: &egui::Painter, from: f32, to: f32, base: f32, tallest: f32, stroke: Stroke, z: f32) {
    let rise = (6.0 * z + 0.22 * (to - from).abs()).min(tallest);
    let (a, b) = (Pos2::new(from, base), Pos2::new(to, base));
    let points = [a, Pos2::new(from, base - rise), Pos2::new(to, base - rise), b];
    painter.add(CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, stroke));
    let head = 2.2 * z;
    painter.add(Shape::convex_polygon(vec![b, Pos2::new(to - head, base - 2.0 * head), Pos2::new(to + head, base - 2.0 * head)], stroke.color, Stroke::NONE));
}

/// The section menu's "When it ends": how likely it goes on as written,
/// and two other places it may go.
pub(super) fn menu(ui: &mut egui::Ui, cx: Ctx, section: usize, edits: &mut Vec<SynthResponse>) {
    let Ctx { node_id, graph, params, song, .. } = cx;
    let name = section_name(graph, node_id, section);
    let next = Arr::next(&song.values, section);
    let set = |new: Next, what: String, edits: &mut Vec<SynthResponse>| {
        if new != next {
            edits.push(edit(node_id, format!("{name} {what}"), vec![(params.name(Arr::PARAM_NEXT + section), new.encode())]));
        }
    };
    ui.label("When it ends").on_hover_text(
        "Where the song goes after this section, by weight: each place's chance is its weight against the others'. \
         The dice are thrown on its last bar, and the timeline shows where they sent it",
    );
    let weight = |ui: &mut egui::Ui, value: u8| {
        let mut value = value as u32;
        let changed = ui.add(egui::DragValue::new(&mut value).range(0..=9).speed(0.05)).on_hover_text("Its weight, 0 to 9, against the others'").changed();
        changed.then_some(value as u8)
    };
    let chances = next.chances();
    egui::Grid::new(("arranger-next", node_id, section)).num_columns(3).spacing([8.0, 2.0]).show(ui, |ui| {
        ui.label(written(graph, node_id, song, section));
        if let Some(on) = weight(ui, if next.wanders() { next.on } else { 1 }) {
            set(Next { on, ..next }, format!("goes on with weight {on}"), edits);
        }
        ui.weak(percent(chances[0]));
        ui.end_row();
        for (index, way) in next.ways.iter().enumerate() {
            let shown = if way.weight == 0 { "Or…".to_string() } else { place(graph, node_id, way.to) };
            ui.menu_button(shown, |ui| {
                let mut choose = |ui: &mut egui::Ui, to: Option<To>, text: String| {
                    if ui.button(text).clicked() {
                        let mut new = next;
                        new.ways[index] = match to {
                            Some(to) => Way { to, weight: way.weight.max(1) },
                            None => Way::NONE,
                        };
                        // The first place to wander to: even odds with going on
                        if !next.wanders() && new.wanders() {
                            new.on = 1;
                        }
                        let what = to.map_or("goes one place fewer".to_string(), |to| format!("may go to {}", place(graph, node_id, to)));
                        set(new, what, edits);
                        ui.close();
                    }
                };
                choose(ui, Some(To::Anywhere), "Anywhere".to_string());
                for to in 0..song.count() {
                    choose(ui, Some(To::Section(to)), format!("{} · {}", to + 1, section_name(graph, node_id, to)));
                }
                if way.weight > 0 {
                    ui.separator();
                    choose(ui, None, "Nowhere".to_string());
                }
            });
            if way.weight > 0 {
                if let Some(value) = weight(ui, way.weight) {
                    let mut new = next;
                    new.ways[index].weight = value;
                    set(new, format!("goes to {} with weight {value}", place(graph, node_id, way.to)), edits);
                }
                ui.weak(percent(chances[1 + index]));
            }
            ui.end_row();
        }
    });
    if next.wanders() && ui.button("Always on as written").clicked() {
        set(Next::WRITTEN, "always goes on as written".to_string(), edits);
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renumbering_follows_the_sections_and_drops_the_gone() {
        let next = Next { on: 1, ways: [Way { to: To::Section(4), weight: 2 }, Way { to: To::Anywhere, weight: 1 }] };
        // Section 5 moves to 6; anywhere stays anywhere
        let moves: Vec<Option<usize>> = (0..8).map(|s| Some(if s < 2 { s } else { s + 1 })).collect();
        let moved = Next::decode(renumbered(next.encode(), &moves));
        assert_eq!(moved.ways, [Way { to: To::Section(5), weight: 2 }, Way { to: To::Anywhere, weight: 1 }]);
        // Deleted: the way goes, the rest stay
        let moves: Vec<Option<usize>> = (0..8).map(|s| (s != 4).then_some(s)).collect();
        let moved = Next::decode(renumbered(next.encode(), &moves));
        assert_eq!(moved, Next { ways: [Way::NONE, Way { to: To::Anywhere, weight: 1 }], ..next });
        // As written is left exactly as it was
        assert_eq!(renumbered(0.0, &moves), 0.0);
    }
}
