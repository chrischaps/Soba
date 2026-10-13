//! The Slicer's face: the row of steps, each drawn as the slice it cuts.
//!
//! Every slice is its own envelope to proportion: it rises over as long as
//! Shape swells it, holds for its Duty, and falls away, as tall as its step
//! is loud: an accent to the top, a hit most of the way, a ghost under
//! half. A tie carries the slice before it on through its step, so a held
//! slice is one long slab. A rest is an empty floor. Swing pushes every
//! second column late, so a swung row looks swung.
//!
//! Below full Depth, a faint band across the floor shows how much passes
//! in the gaps. Turned up, the Filter draws a line through each slice falling as the
//! step filter closes. With Ping-Pong, slices bound for the right wear a
//! violet of their own. While it plays, a dot rides the envelope where the
//! slice is.
//!
//! Click an empty step to open a slice, as loud as how high you click it;
//! click a slice to rest it, or drag it up or down for its level.
//! Shift+click ties a step to the slice before it. Right-click for the
//! patterns, and to turn the row round.

use eframe::egui::{self, Color32, CursorIcon, Id, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::slicer::{pattern_steps, Slice, Slicer, FILTER_CLOSE, PATTERNS, RATES, STEPS, STEP_NAMES, SWELL_ATTACK, SWELL_RELEASE};

use super::display_frame::{self, value_of, with_alpha, PANEL};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// The violet a slice bound for the right wears under Ping-Pong.
const RIGHT: Color32 = Color32::from_rgb(179, 136, 255);

/// How tall the face is, before zoom.
const HEIGHT: f32 = 64.0;

/// Where each step's column starts and how wide it is, as fractions of the
/// row, for `count` steps swung by `swing` (0.5 straight, to 0.75): every
/// second column starts late and is narrower for it.
pub fn columns(count: usize, swing: f32) -> Vec<(f32, f32)> {
    let split = 2.0 * swing.clamp(0.5, 0.75);
    let mut cells: Vec<(f32, f32)> = (0..count)
        .map(|j| {
            let pair = (j / 2) as f32 * 2.0;
            if j % 2 == 0 { (pair, split) } else { (pair + split, 2.0 - split) }
        })
        .collect();
    let total = cells.last().map_or(1.0, |(start, len)| start + len);
    for (start, len) in &mut cells {
        *start /= total;
        *len /= total;
    }
    cells
}

/// The pattern `steps` is, played for `count` steps, if it's one of the
/// patterns as written.
pub fn pattern_named(steps: &[Slice; STEPS], count: usize) -> Option<&'static str> {
    PATTERNS
        .iter()
        .find(|(_, symbols, length)| *length == count && pattern_steps(symbols)[..count] == steps[..count])
        .map(|(name, _, _)| *name)
}

/// The first `count` steps turned round by `by`: later for a positive `by`,
/// so the slice on step 1 moves to step `1 + by`.
pub fn rotated(steps: &[Slice; STEPS], count: usize, by: isize) -> [Slice; STEPS] {
    let mut turned = *steps;
    for j in 0..count {
        let from = (j as isize - by).rem_euclid(count as isize) as usize;
        turned[j] = steps[from];
    }
    turned
}

/// The level step `j` plays at: its own, or for a tie the slice it holds.
fn run_level(steps: &[Slice; STEPS], j: usize, count: usize) -> f32 {
    (0..count)
        .map(|back| steps[(j + count - back) % count])
        .find(|s| *s != Slice::Tie)
        .map_or(Slice::Hit.level(), Slice::level)
}

/// The step a click or a drag at `height` (0 the floor, 1 the top) asks for.
fn slice_at_height(height: f32) -> Slice {
    if height > 0.88 {
        Slice::Accent
    } else if height > 0.6 {
        Slice::Hit
    } else {
        Slice::Ghost
    }
}

fn edit(node_id: NodeId, label: impl Into<String>, changes: Vec<(String, f32)>) -> SynthResponse {
    SynthResponse::EditParameters { node_id, label: label.into(), changes }
}

/// The changes that write `steps` over the row.
fn write(steps: &[Slice; STEPS]) -> Vec<(String, f32)> {
    steps.iter().enumerate().map(|(j, s)| (STEP_NAMES[j].to_string(), s.encode())).collect()
}

/// Draws the face, and returns what the user did with it.
pub fn slicer_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let number = |name: &str, default: f32| value_of(graph, node_id, name).unwrap_or(default);
    let mut steps = [Slice::Rest; STEPS];
    for (j, step) in steps.iter_mut().enumerate() {
        *step = Slice::decode(number(STEP_NAMES[j], 0.0));
    }
    let count = Slicer::step_count(number("Steps", 16.0));
    let duty = (number("Duty", 55.0) / 100.0).clamp(0.01, 1.0);
    let shape = number("Shape", 0.1).clamp(0.0, 1.0);
    let filter = number("Filter", 0.0).clamp(0.0, 1.0);
    let swing = number("Swing", 50.0) / 100.0;
    let ping_pong = number("Ping-Pong", 0.0) > 0.5;
    let depth = number("Depth", 1.0).clamp(0.0, 1.0);
    let rate = RATES.get(number("Rate", 1.0).round().max(0.0) as usize).copied().unwrap_or("1/16");

    // Where the row is, while it runs
    let readout = display_frame::readout(user_state, node_id);
    let live = readout.filter(|r| r.values[Slicer::READOUT_STEP] >= 0.0).map(|r| {
        let v = r.values;
        let level = v[Slicer::READOUT_LEFT].max(v[Slicer::READOUT_RIGHT]);
        ((v[Slicer::READOUT_STEP].round() as usize).min(count - 1), v[Slicer::READOUT_ALONG].clamp(0.0, 1.0), level)
    });

    let width = display_frame::knob_row_width(ui, 4, z);
    ui.add_space(8.0 * z);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, HEIGHT * z), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return Vec::new();
    }
    let cyan = crate::dsp::ModuleCategory::Effect.color();
    let teal = crate::dsp::ModuleCategory::Filter.color();
    ui.painter().hline(rect.x_range(), rect.top() - 4.0 * z, Stroke::new(1.0 * z, with_alpha(cyan, 64)));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0 * z, PANEL);

    let name = pattern_named(&steps, count);
    let right = if ping_pong { Some(("L ⇄ R".to_string(), RIGHT.gamma_multiply(0.9))) } else { name.map(|n| (n.to_string(), theme::text::SECONDARY)) };
    let swung = if swing > 0.505 { format!(" · swing {:.0}%", swing * 100.0) } else { String::new() };
    display_frame::header(&painter, rect, z, Some((format!("{rate} × {count}{swung}"), theme::text::SECONDARY)), right);

    let pad = 4.0 * z;
    let plot = Rect::from_min_max(rect.min + Vec2::new(pad, pad + 10.0 * z), rect.max - Vec2::new(pad, pad));
    let floor = plot.bottom();
    let cells = columns(count, swing);
    let cell_rect = |j: usize| {
        let (start, len) = cells[j];
        Rect::from_min_max(Pos2::new(plot.left() + start * plot.width(), plot.top()), Pos2::new(plot.left() + (start + len) * plot.width(), floor))
    };

    // The floor, and a guide at each step, brighter on the beat
    painter.line_segment([Pos2::new(plot.left(), floor), Pos2::new(plot.right(), floor)], Stroke::new(0.8 * z, with_alpha(Color32::WHITE, 28)));
    for j in 1..count {
        let x = cell_rect(j).left();
        let alpha = if j % 4 == 0 { 26 } else { 10 };
        display_frame::guide(&painter, Pos2::new(x, plot.top()), Pos2::new(x, floor), alpha, z);
    }

    // What passes in the gaps: nothing at full Depth, all of it at 0
    if depth < 0.995 {
        let band = Rect::from_min_max(Pos2::new(plot.left(), floor - (1.0 - depth) * plot.height()), Pos2::new(plot.right(), floor));
        painter.rect_filled(band, 0.0, with_alpha(cyan, 22));
        let y = band.top();
        painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(0.6 * z, with_alpha(cyan, 60)));
    }

    // Which side each slice plays on: they alternate, from the left
    let mut on_right = [false; STEPS];
    let mut heads = 0;
    let mut side = false;
    for j in 0..count {
        match steps[j] {
            Slice::Ghost | Slice::Hit | Slice::Accent => {
                side = ping_pong && heads % 2 == 1;
                heads += 1;
            }
            Slice::Tie => {}
            Slice::Rest => side = false,
        }
        on_right[j] = side;
    }

    // Each step's piece of its slice
    let swell = shape * shape;
    for j in 0..count {
        let cell = cell_rect(j);
        let slice = steps[j];
        let level = run_level(&steps, j, count);
        let head = !matches!(slice, Slice::Rest | Slice::Tie);
        let held = steps[(j + 1) % count] == Slice::Tie;
        if slice == Slice::Rest || level <= 0.0 {
            if slice == Slice::Tie {
                // A tie with nothing to hold
                let y = floor - 2.0 * z;
                painter.line_segment([Pos2::new(cell.left() + 2.0 * z, y), Pos2::new(cell.right() - 2.0 * z, y)], Stroke::new(0.8 * z, with_alpha(cyan, 50)));
            }
            continue;
        }
        let w = cell.width();
        let top = floor - level * plot.height();
        let rise = if head { (0.08 + swell * SWELL_ATTACK) * w } else { 0.0 };
        let open_to = if held { cell.right() } else { cell.left() + duty * w };
        let fall = if held { 0.0 } else { (0.08 + swell * SWELL_RELEASE) * w };
        let start = cell.left() + if head { 0.5 * z } else { 0.0 };
        let left_top = (start + rise).min(open_to);
        let gone = (open_to + fall).min(cell.right() - if held { 0.0 } else { 0.5 * z });
        let points = vec![Pos2::new(start, floor), Pos2::new(left_top, top), Pos2::new(open_to, top), Pos2::new(gone.max(open_to), floor)];
        let hue = if on_right[j] { RIGHT } else { cyan };
        let alpha: f32 = match level {
            l if l >= 1.0 => 0.82,
            l if l >= 0.8 => 0.58,
            _ => 0.34,
        };
        let here = live.is_some_and(|(step, _, _)| step == j);
        let fill = hue.gamma_multiply(if here { (alpha + 0.18).min(1.0) } else { alpha });
        painter.add(Shape::convex_polygon(points.clone(), fill, Stroke::NONE));
        painter.line_segment([Pos2::new(left_top, top), Pos2::new(open_to, top)], Stroke::new(1.2 * z, hue));

        // The step filter closing through the slice, from as bright as it struck
        if filter > 0.0 {
            let since = (0..count).map(|back| (j + count - back) % count).take_while(|&k| k == j || steps[(k + 1) % count] == Slice::Tie).count() - 1;
            let points: Vec<Pos2> = (0..=12)
                .map(|s| {
                    let along = s as f32 / 12.0;
                    let steps_in = since as f32 + along;
                    let opening = level * (0.25 + 0.75 * (-steps_in / FILTER_CLOSE).exp());
                    let dark = filter * (1.0 - opening);
                    Pos2::new(cell.left() + along * w, top + dark * (floor - top))
                })
                .collect();
            display_frame::curve(&painter, points, with_alpha(teal, 200), 1.0 * z, false);
        }
    }

    // Where it is: a guide down the step, and a dot riding the slice
    if let Some((step, along, level)) = live {
        let cell = cell_rect(step);
        let x = cell.left() + along * cell.width();
        display_frame::guide(&painter, Pos2::new(x, plot.top()), Pos2::new(x, floor), 70, z);
        let at = Pos2::new(x, floor - level * plot.height());
        let hue = if on_right[step] { RIGHT } else { cyan };
        painter.circle_filled(at, 5.0 * z, with_alpha(hue, 70));
        painter.circle_filled(at, 2.2 * z, Color32::WHITE);
    }

    // Editing
    let mut edits = Vec::new();
    let height_at = |y: f32| ((floor - y) / plot.height()).clamp(0.0, 1.0);
    for j in 0..count {
        let cell = cell_rect(j);
        let what = format!("Step {}", j + 1);
        let response = ui.interact(cell, Id::new((node_id, "slicer-step", j)), Sense::click_and_drag()).on_hover_cursor(CursorIcon::PointingHand);
        let slice = steps[j];
        let set = |to: Slice| vec![(STEP_NAMES[j].to_string(), to.encode())];

        if response.clicked() {
            let shift = ui.input(|input| input.modifiers.shift);
            if shift {
                let to = if slice == Slice::Tie { Slice::Rest } else { Slice::Tie };
                let label = if to == Slice::Tie { format!("Tie {what}") } else { format!("Rest {what}") };
                edits.push(edit(node_id, label, set(to)));
            } else if matches!(slice, Slice::Rest | Slice::Tie) {
                let to = response.interact_pointer_pos().map_or(Slice::Hit, |p| slice_at_height(height_at(p.y)));
                edits.push(edit(node_id, format!("{} on {what}", to.name()), set(to)));
            } else {
                edits.push(edit(node_id, format!("Rest {what}"), set(Slice::Rest)));
            }
        }
        if response.dragged() && !matches!(slice, Slice::Rest | Slice::Tie) {
            if let Some(p) = response.interact_pointer_pos() {
                let to = slice_at_height(height_at(p.y));
                if to != slice {
                    edits.push(edit(node_id, format!("{} on {what}", to.name()), set(to)));
                }
            }
        }

        let hint = format!(
            "{what}: {}\nClick {} · drag a slice up or down for its level\nShift+click ties it to the slice before · right-click for patterns",
            match slice {
                Slice::Tie if run_level(&steps, j, count) <= 0.0 => "Tie, with nothing to hold".to_string(),
                Slice::Tie => "Tie, holding the slice before open".to_string(),
                other => other.name().to_string(),
            },
            if matches!(slice, Slice::Rest | Slice::Tie) { "high for an accent, low for a ghost" } else { "to rest it" },
        );
        let response = response.on_hover_text(hint);
        response.context_menu(|ui| step_menu(ui, node_id, &steps, count, j, &mut edits));
    }
    edits
}

/// The right-click menu: the step, then the whole row.
fn step_menu(ui: &mut egui::Ui, node_id: NodeId, steps: &[Slice; STEPS], count: usize, j: usize, edits: &mut Vec<SynthResponse>) {
    let what = format!("Step {}", j + 1);
    ui.horizontal(|ui| {
        for slice in Slice::ALL {
            if ui.selectable_label(steps[j] == slice, slice.name()).clicked() {
                edits.push(edit(node_id, format!("{} on {what}", slice.name()), vec![(STEP_NAMES[j].to_string(), slice.encode())]));
                ui.close_menu();
            }
        }
    });
    ui.separator();
    let current = pattern_named(steps, count);
    ui.menu_button("Patterns", |ui| {
        for (name, symbols, length) in PATTERNS {
            let shown: String = symbols.chars().take(*length).collect();
            let response = ui.selectable_label(current == Some(*name), *name).on_hover_text(format!("{shown}\nx a hit · X an accent · . a ghost · = a tie · - a rest"));
            if response.clicked() {
                let mut changes = write(&pattern_steps(symbols));
                changes.push(("Steps".to_string(), *length as f32));
                edits.push(edit(node_id, format!("Slicer pattern: {name}"), changes));
                ui.close_menu();
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button("◀ Rotate").on_hover_text("Moves every step one earlier").clicked() {
            edits.push(edit(node_id, "Rotate the row earlier", write(&rotated(steps, count, -1))));
            ui.close_menu();
        }
        if ui.button("Rotate ▶").on_hover_text("Moves every step one later").clicked() {
            edits.push(edit(node_id, "Rotate the row later", write(&rotated(steps, count, 1))));
            ui.close_menu();
        }
    });
    if ui.button("Clear").on_hover_text("Rests every step").clicked() {
        edits.push(edit(node_id, "Clear the Slicer's row", write(&[Slice::Rest; STEPS])));
        ui.close_menu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_columns_fill_the_row_and_swing_pushes_the_off_steps_late() {
        let straight = columns(16, 0.5);
        assert_eq!(straight.len(), 16);
        assert!((straight[1].0 - 1.0 / 16.0).abs() < 1e-6);
        let last = straight[15];
        assert!((last.0 + last.1 - 1.0).abs() < 1e-6);

        let swung = columns(16, 0.66);
        assert!(swung[1].0 > straight[1].0 && swung[1].1 < swung[0].1);
        assert!((swung[2].0 - straight[2].0).abs() < 1e-6, "every pair starts on its own beat");
        // An odd row still ends at the edge
        let odd = columns(5, 0.6);
        assert!((odd[4].0 + odd[4].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_a_row_as_written_is_named_and_a_changed_one_isnt() {
        for (name, symbols, count) in PATTERNS {
            assert_eq!(pattern_named(&pattern_steps(symbols), *count), Some(*name));
        }
        let mut steps = pattern_steps(PATTERNS[0].1);
        steps[1] = Slice::Accent;
        assert_eq!(pattern_named(&steps, 16), None);
    }

    #[test]
    fn test_rotating_turns_only_the_steps_that_play() {
        let steps = pattern_steps("X-x.------------");
        let later = rotated(&steps, 4, 1);
        assert_eq!(&later[..4], &pattern_steps(".X-x")[..4]);
        assert_eq!(rotated(&later, 4, -1), steps);
    }

    #[test]
    fn test_clicking_higher_asks_for_a_louder_slice() {
        assert_eq!(slice_at_height(0.2), Slice::Ghost);
        assert_eq!(slice_at_height(0.75), Slice::Hit);
        assert_eq!(slice_at_height(0.95), Slice::Accent);
        // Each is drawn inside its own band, so clicking a slab's top keeps it
        for slice in [Slice::Ghost, Slice::Hit, Slice::Accent] {
            assert_eq!(slice_at_height(slice.level() - 0.02), slice);
        }
    }
}
