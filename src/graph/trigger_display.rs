//! The Trigger Sequencer's grid, drawn beside its jacks.
//!
//! Each lane sits on the two output rows its **Gate** and **Vel** jacks hang
//! from, so a lane reads straight across into its cables: its pads on the
//! Gate row in the gate's green, each hit's velocity as an orange bar on the
//! Vel row. The accent row sits by the Accent jack, above them all, with
//! notation's accent mark on each accented step.
//!
//! - A pad fills from the bottom by its probability, so a step that plays
//!   half the time is half full, and splits into slivers for a ratchet.
//! - A lane is named for what its Gate cable plays: a Drum's type, or the
//!   module's name.
//! - Steps past the end of a lane (its own Length, or the bar's Steps) fade.
//! - Under the grid are tabs for the pattern being edited, the one playing
//!   marked with a green dot, and the Chain, its playing slot underlined.
//!
//! Every edit is one undo step (see [`SynthResponse::EditParameters`]).

use eframe::egui::{self, Color32, CursorIcon, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::{InputId, NodeId};

use crate::app::theme;
use crate::modules::trigger_sequencer::{
    Position, Step, TriggerSequencer as Seq, CHAIN_SLOTS, GATE_NAMES, LANES, MAX_RATCHET, PATTERNS,
    PATTERN_NAMES, STEPS, VEL_NAMES,
};

use super::sequencer_common::{pattern_menu, MenuHints};
use super::steady_text;
use super::{SynthGraph, SynthGraphState, SynthResponse, SynthValueType};

/// Height of one output row, in unzoomed points.
const ROW: f32 = 13.0;
/// Room for the lane names, left of the steps.
const LABEL: f32 = 34.0;
/// A step's width.
const CELL: f32 = 12.0;
/// Between steps, and the extra between beats.
const GAP: f32 = 2.0;
const BEAT: f32 = 4.0;
/// Room for the jack labels, right of the steps.
const JACK_LABEL: f32 = 38.0;

/// The chances Ctrl+click steps through.
const PROBABILITIES: [u8; 5] = [100, 75, 50, 25, 10];
/// Velocity points per point dragged.
const DRAG_RATE: f32 = 1.0;

/// Where a step starts, from the row's left edge.
fn step_left(step: usize, z: f32) -> f32 {
    (LABEL + step as f32 * (CELL + GAP) + (step / 4) as f32 * BEAT) * z
}

fn grid_width(z: f32) -> f32 {
    step_left(STEPS - 1, z) + CELL * z
}

/// The node's parameters, by index.
struct Params<'a> {
    graph: &'a SynthGraph,
    inputs: &'a [(String, InputId)],
}

impl<'a> Params<'a> {
    fn of(graph: &'a SynthGraph, node_id: NodeId) -> Option<Self> {
        let node = graph.nodes.get(node_id)?;
        let first = node.inputs.iter().position(|(name, _)| name == "Steps")?;
        let inputs = &node.inputs[first..];
        (inputs.len() >= Seq::PARAM_COUNT).then_some(Self { graph, inputs })
    }

    fn value(&self, index: usize) -> f32 {
        self.graph.get_input(self.inputs[index].1).value.actual_value()
    }

    fn name(&self, index: usize) -> String {
        self.inputs[index].0.clone()
    }

    fn step(&self, pattern: usize, lane: usize, step: usize) -> Step {
        Step::decode(self.value(Seq::step_param(pattern, lane, step)))
    }

    fn accent(&self, pattern: usize, step: usize) -> bool {
        self.value(Seq::accent_param(pattern, step)) > 0.5
    }

    /// The bar's length.
    fn steps(&self) -> usize {
        (self.value(Seq::PARAM_STEPS).round() as usize).clamp(1, STEPS)
    }

    /// A lane's own Length, or 0 if it follows the bar.
    fn length(&self, lane: usize) -> usize {
        self.value(Seq::PARAM_LENGTH + lane).round().clamp(0.0, STEPS as f32) as usize
    }

    /// How many of a lane's steps play.
    fn extent(&self, lane: usize) -> usize {
        match self.length(lane) {
            0 => self.steps(),
            length => length,
        }
    }

    /// The Chain's slots: 0 for empty, else the pattern + 1.
    fn chain(&self) -> [usize; CHAIN_SLOTS] {
        std::array::from_fn(|slot| (self.value(Seq::PARAM_CHAIN + slot).round().max(0.0) as usize).min(PATTERNS))
    }

    fn set_step(&self, pattern: usize, lane: usize, step: usize, to: Step) -> (String, f32) {
        (self.name(Seq::step_param(pattern, lane, step)), to.encode())
    }
}

fn edit(node_id: NodeId, label: impl Into<String>, changes: Vec<(String, f32)>) -> SynthResponse {
    SynthResponse::EditParameters { node_id, label: label.into(), changes }
}

/// The pattern the grid shows. It's the editor's choice, not the patch's,
/// so it isn't saved and isn't an edit. The Step Sequencer's grid keeps its
/// own the same way.
pub(super) fn edit_pattern(ctx: &egui::Context, node_id: NodeId) -> usize {
    ctx.data(|data| data.get_temp::<usize>(Id::unique((node_id, "edit-pattern")))).unwrap_or(0).min(PATTERNS - 1)
}

fn set_edit_pattern(ctx: &egui::Context, node_id: NodeId, pattern: usize) {
    ctx.data_mut(|data| data.insert_temp(Id::unique((node_id, "edit-pattern")), pattern));
}

/// Whether the grid follows the pattern playing, as the Follow toggle beside
/// the tabs sets it. Like the pattern shown, it's the editor's.
fn following(ctx: &egui::Context, node_id: NodeId) -> bool {
    ctx.data(|data| data.get_temp::<bool>(Id::unique((node_id, "follow-pattern")))).unwrap_or(false)
}

fn set_following(ctx: &egui::Context, node_id: NodeId, on: bool) {
    ctx.data_mut(|data| data.insert_temp(Id::unique((node_id, "follow-pattern")), on));
}

/// The pattern the grid shows: the one being edited, or while it follows,
/// `playing`, so the grid turns over as the Chain (or the Pattern CV) moves
/// on. Read before drawing, so the grid and its tabs agree in every frame.
pub(super) fn shown_pattern(ctx: &egui::Context, node_id: NodeId, playing: Option<usize>) -> usize {
    if let Some(pattern) = playing.filter(|_| following(ctx, node_id)) {
        set_edit_pattern(ctx, node_id, pattern.min(PATTERNS - 1));
    }
    edit_pattern(ctx, node_id)
}

fn position(user_state: &SynthGraphState, node_id: NodeId) -> Option<Position> {
    let engine_id = user_state.get_engine_node_id(node_id)?;
    user_state.readouts.get(&engine_id).map(Position::from_readout)
}

/// Whether an output is high right now.
fn lit(user_state: &SynthGraphState, node_id: NodeId, output: usize) -> bool {
    user_state
        .get_engine_node_id(node_id)
        .and_then(|engine_id| user_state.get_output_value(engine_id, output))
        .is_some_and(|value| value > 0.5)
}

/// A drum type's name, short enough for the lane.
fn short_drum(name: &str) -> String {
    match name {
        "Closed Hat" => "C Hat",
        "Open Hat" => "O Hat",
        "Cymbal" => "Cym",
        "Cowbell" => "Bell",
        other => other,
    }
    .to_string()
}

/// A Kit's part as short as a lane's name wants it: a word and a number
/// ("Track 3", "Pad 13") as its letter and the number ("T3", "P13").
fn short_part(name: &str) -> String {
    match name.split_once(' ') {
        Some((word, number)) if name.len() > 5 && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) => {
            format!("{}{number}", word.chars().next().unwrap_or(' '))
        }
        _ => name.to_string(),
    }
}

/// What a lane's Gate cable plays: a Drum's type, a Kit's part (#443), or
/// the module's name.
fn lane_name(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId, lane: usize) -> Option<String> {
    let output = graph.nodes.get(node_id)?.get_output(GATE_NAMES[lane]).ok()?;
    let input = user_state.cables_from.first(output)?;
    let target = graph.nodes.get(graph.get_input(input).node)?;
    if target.user_data.module_id == "source.drum" {
        let type_input = target.get_input("Type").ok()?;
        if let SynthValueType::Select { value, options, .. } = &graph.get_input(type_input).value {
            return options.get(*value).map(|name| short_drum(name));
        }
    }
    if target.user_data.module_id == crate::modules::kit::ID {
        let jack = target.inputs.iter().find(|(_, id)| *id == input).map(|(name, _)| name.as_str());
        if let Some(part) = jack.and_then(|jack| super::kit_face::jack_name(&target.user_data, jack)) {
            return Some(short_part(&part));
        }
    }
    Some(target.label.clone())
}

/// Which output row is being drawn.
#[derive(Clone, Copy)]
enum Row {
    Accent,
    Gate(usize),
    Vel,
}

/// Draws the output row for `output`, with its part of the grid, and
/// returns the edits made on it. `None` if this isn't a Trigger Sequencer's
/// output, so the row is drawn as usual.
pub fn output_row(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    output: &str,
) -> Option<Vec<SynthResponse>> {
    let row = if output == "Accent" {
        Row::Accent
    } else if let Some(lane) = GATE_NAMES.iter().position(|name| *name == output) {
        Row::Gate(lane)
    } else {
        VEL_NAMES.iter().position(|name| *name == output).map(|_| Row::Vel)?
    };
    let params = Params::of(graph, node_id)?;
    let z = user_state.zoom;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(grid_width(z) + JACK_LABEL * z, ROW * z), Sense::hover());

    // The jack's label, small enough for the rows, flush with the edge
    let short = match row {
        Row::Accent => "Accent",
        Row::Gate(_) => "Gate",
        Row::Vel => "Vel",
    };
    let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let galley = ui.painter().layout_no_wrap(short.to_string(), FontId::proportional(10.0 * z), ink);
    let label_rect = Rect::from_min_size(
        Pos2::new(rect.right() - galley.size().x, rect.center().y - galley.size().y / 2.0),
        galley.size(),
    );
    super::node_data::defer_output_label(ui, node_id, output, label_rect, vec![Shape::galley(label_rect.min, galley, ink)]);

    // A lane reaches down over its Vel row
    if !ui.is_rect_visible(rect.expand2(Vec2::new(0.0, 2.0 * ROW * z))) {
        return Some(Vec::new());
    }
    let position = position(user_state, node_id).filter(|p| p.started);
    let pattern = shown_pattern(ui.ctx(), node_id, position.map(|p| p.pattern));
    let mut edits = Vec::new();
    match row {
        Row::Accent => accent_row(ui, rect, node_id, &params, pattern, Live { position, lit: lit(user_state, node_id, Seq::OUT_ACCENT) }, &mut edits),
        Row::Gate(lane) => {
            let name = lane_name(graph, user_state, node_id, lane);
            let lit = lit(user_state, node_id, 1 + 2 * lane);
            lane_rows(ui, rect, node_id, &params, LaneAt { pattern, lane, name }, Live { position, lit }, &mut edits);
        }
        Row::Vel => {}
    }
    Some(edits)
}

/// Where the sequencer is, and whether a row's output is high.
#[derive(Clone, Copy)]
struct Live {
    position: Option<Position>,
    lit: bool,
}

/// A lane in the pattern being edited, and its name, if it's been given one.
struct LaneAt {
    pattern: usize,
    lane: usize,
    name: Option<String>,
}

/// One step of a lane, and what the lane's called.
struct StepAt<'a> {
    pattern: usize,
    lane: usize,
    step: usize,
    lane_label: &'a str,
}

/// The accent row: the pattern being edited at the left, then an accent
/// mark (>) on each accented step.
fn accent_row(ui: &mut egui::Ui, rect: Rect, node_id: NodeId, params: &Params, pattern: usize, live: Live, edits: &mut Vec<SynthResponse>) {
    let Live { position, lit } = live;
    let z = rect.height() / ROW;
    let painter = ui.painter().clone();
    let orange = theme::signal::CONTROL;
    let green = theme::signal::GATE;
    let playing = position.is_some_and(|p| p.pattern == pattern);
    let letter = PATTERN_NAMES[pattern];

    // Which pattern this is, green while it plays
    let pill = Rect::from_center_size(Pos2::new(rect.left() + 9.0 * z, rect.center().y), Vec2::new(14.0 * z, 11.0 * z));
    painter.rect_filled(pill, 3.0 * z, if playing { green.gamma_multiply(0.85) } else { theme::background::WIDGET_ACTIVE });
    painter.text(pill.center(), egui::Align2::CENTER_CENTER, letter, FontId::proportional(9.0 * z), if playing { Color32::BLACK } else { theme::text::PRIMARY });

    let steps = params.steps();
    for step in 0..STEPS {
        let left = rect.left() + step_left(step, z);
        let cell = Rect::from_min_size(Pos2::new(left, rect.top()), Vec2::new(CELL * z, rect.height()));
        let accented = params.accent(pattern, step);
        let fade = if step < steps { 1.0 } else { 0.3 };
        let here = position.is_some_and(|p| p.bar_step == step);
        let lit = lit && playing;

        if here {
            let strength = if !playing { 0.04 } else if lit { 0.16 } else { 0.08 };
            painter.rect_filled(cell.shrink(0.5 * z), 2.0 * z, Color32::WHITE.gamma_multiply(strength));
        }
        let c = cell.center();
        if accented {
            // Notation's accent: a wedge opening to the left
            let (w, h) = (3.2 * z, 3.4 * z);
            let wedge = vec![Pos2::new(c.x - w, c.y - h), Pos2::new(c.x + w, c.y), Pos2::new(c.x - w, c.y + h)];
            let color = if here && lit { orange.lerp_to_gamma(Color32::WHITE, 0.5) } else { orange };
            painter.add(Shape::line(wedge, Stroke::new(1.7 * z, color.gamma_multiply(fade))));
        } else {
            painter.circle_filled(c, 1.1 * z, theme::background::GRID_MAJOR.gamma_multiply(fade));
        }

        super::marks::mark(ui.ctx(), node_id, format_args!("accent:{}", step + 1), cell);
        let response = ui
            .interact(cell, Id::unique((node_id, "trigger-accent", step)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("Accent on step {} of {letter}: every lane playing here plays louder. Click to switch", step + 1));
        if response.clicked() {
            let verb = if accented { "Remove" } else { "Add" };
            edits.push(edit(
                node_id,
                format!("{verb} accent on {letter} step {}", step + 1),
                vec![(params.name(Seq::accent_param(pattern, step)), if accented { 0.0 } else { 1.0 })],
            ));
        }
    }
}

/// A lane, over its Gate and Vel rows: its name, then a pad and a velocity
/// bar for each step.
fn lane_rows(ui: &mut egui::Ui, rect: Rect, node_id: NodeId, params: &Params, at: LaneAt, live: Live, edits: &mut Vec<SynthResponse>) {
    let LaneAt { pattern, lane, name } = at;
    let Live { position, lit } = live;
    let z = rect.height() / ROW;
    let pitch = rect.height() + ui.spacing().item_spacing.y;
    let painter = ui.painter().clone();
    let green = theme::signal::GATE;
    let orange = theme::signal::CONTROL;
    let letter = PATTERN_NAMES[pattern];
    let lane_label = name.clone().unwrap_or_else(|| format!("Lane {}", lane + 1));

    // Pads on the Gate row, a little taller than it; velocity on the Vel row
    let pad_top = rect.top() + 0.5 * z;
    let pad_bottom = rect.bottom() + 1.5 * z;
    let vel_y = rect.top() + pitch + rect.height() / 2.0;
    let block = Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + pitch + rect.height()));

    // The lane's name, or its number, between its rows
    let font = FontId::proportional(9.5 * z);
    let text = steady_text::fit(painter.ctx(), name.as_deref().unwrap_or(&(lane + 1).to_string()), &font, (LABEL - 5.0) * z);
    let ink = if name.is_some() { theme::text::PRIMARY } else { theme::text::DISABLED };
    let name_rect = Rect::from_min_size(block.left_top(), Vec2::new((LABEL - 3.0) * z, block.height()));
    painter.text(Pos2::new(name_rect.left() + 1.0 * z, name_rect.center().y), egui::Align2::LEFT_CENTER, text, font, ink);
    let length = params.length(lane);
    let name_response = ui
        .interact(name_rect, Id::unique((node_id, "trigger-lane", lane)), Sense::click())
        .on_hover_text(format!("{lane_label}: right-click for its length, or to clear it"));
    crate::widgets::menu::context_menu(&name_response, |ui| lane_menu(ui, node_id, params, pattern, lane, &lane_label, edits));

    let extent = params.extent(lane);
    // The playhead shows on whichever pattern is up, bright on the one playing
    let playing = position.is_some_and(|p| p.pattern == pattern);
    let lit = lit && playing;
    for step in 0..STEPS {
        let left = rect.left() + step_left(step, z);
        let pad = Rect::from_min_max(Pos2::new(left, pad_top), Pos2::new(left + CELL * z, pad_bottom));
        let cell = Rect::from_min_max(Pos2::new(left, block.top()), Pos2::new(left + CELL * z, block.bottom()));
        let s = params.step(pattern, lane, step);
        let fade = if step < extent { 1.0 } else { 0.3 };
        let here = position.is_some_and(|p| p.lanes[lane] == step);
        let round = 2.0 * z;

        painter.rect_filled(pad, round, theme::background::WIDGET.gamma_multiply(fade));
        if s.on {
            let color = if here && lit { green.lerp_to_gamma(Color32::WHITE, 0.45) } else { green };
            if here && lit {
                painter.rect_filled(pad.expand(1.5 * z), round + 1.5 * z, green.gamma_multiply(0.3));
            }
            // Full for a sure hit; filled to its chance otherwise
            let share = s.probability as f32 / 100.0;
            let fill = Rect::from_min_max(Pos2::new(pad.left(), pad.bottom() - pad.height() * share.max(0.12)), pad.max);
            painter.rect_filled(fill, round, color.gamma_multiply(fade));
            if s.probability < 100 {
                painter.rect_stroke(pad.shrink(0.5 * z), round, Stroke::new(1.0 * z, color.gamma_multiply(0.8 * fade)), egui::StrokeKind::Middle);
            }
            // A ratchet splits the pad into its hits
            for cut in 1..s.ratchet {
                let x = pad.left() + pad.width() * cut as f32 / s.ratchet as f32;
                painter.vline(x, pad.y_range(), Stroke::new(1.3 * z, theme::background::PANEL));
            }
            // Velocity, as long as the hit is loud
            let track = Rect::from_center_size(Pos2::new(pad.center().x, vel_y), Vec2::new(CELL * z, 3.0 * z));
            painter.rect_filled(track, 1.5 * z, theme::background::WIDGET.gamma_multiply(fade));
            let bar = Rect::from_min_size(track.min, Vec2::new(track.width() * s.level().max(0.04), track.height()));
            painter.rect_filled(bar, 1.5 * z, orange.gamma_multiply(fade));
        }
        if here {
            let strength = if playing { 0.85 } else { 0.3 };
            painter.rect_stroke(pad.expand(0.5 * z), round, Stroke::new(1.2 * z, Color32::WHITE.gamma_multiply(strength)), egui::StrokeKind::Middle);
        }
        // The end of a lane that loops on its own
        if length > 0 && step + 1 == length {
            let x = pad.right() + GAP * z / 2.0 + 0.5 * z;
            painter.vline(x, block.y_range().shrink(1.0 * z), Stroke::new(1.2 * z, orange.gamma_multiply(0.8)));
        }

        super::marks::mark(ui.ctx(), node_id, format_args!("cell:{}:{}", lane + 1, step + 1), cell);
        let response = ui
            .interact(cell, Id::unique((node_id, "trigger-step", lane, step)), Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::PointingHand);
        let what = format!("{lane_label} step {} of {letter}", step + 1);
        let describe = |s: Step| {
            let mut text = format!("{}%", s.velocity);
            if s.ratchet > 1 {
                text += &format!(", ×{}", s.ratchet);
            }
            if s.probability < 100 {
                text += &format!(", {}% chance", s.probability);
            }
            text
        };

        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            if modifiers.alt {
                let to = if length == step + 1 { 0 } else { step + 1 };
                edits.push(length_edit(node_id, params, lane, &lane_label, to));
            } else if modifiers.shift {
                let next = Step { on: true, ratchet: s.ratchet % MAX_RATCHET + 1, ..s };
                edits.push(edit(node_id, format!("Ratchet ×{} on {what}", next.ratchet), vec![params.set_step(pattern, lane, step, next)]));
            } else if modifiers.command {
                let at = PROBABILITIES.iter().position(|&p| p <= s.probability).unwrap_or(0);
                let next = Step { on: true, probability: PROBABILITIES[(at + 1) % PROBABILITIES.len()], ..s };
                edits.push(edit(node_id, format!("{}% chance on {what}", next.probability), vec![params.set_step(pattern, lane, step, next)]));
            } else {
                let next = Step { on: !s.on, ..s };
                let verb = if next.on { "Add" } else { "Remove" };
                edits.push(edit(node_id, format!("{verb} {what}"), vec![params.set_step(pattern, lane, step, next)]));
            }
        }

        // Dragging up or down sets the velocity, and plays the step
        let drag_id = Id::unique((node_id, "trigger-drag", lane, step));
        if response.drag_started() {
            ui.ctx().data_mut(|data| data.insert_temp(drag_id, s.velocity as f32));
        }
        if response.dragged() {
            let start = ui.ctx().data(|data| data.get_temp::<f32>(drag_id)).unwrap_or(s.velocity as f32);
            let now = (start - response.drag_delta().y / z * DRAG_RATE).clamp(1.0, 100.0);
            ui.ctx().data_mut(|data| data.insert_temp(drag_id, now));
            let next = Step { on: true, velocity: now.round() as u8, ..s };
            if next != s {
                edits.push(edit(node_id, format!("Velocity {}% on {what}", next.velocity), vec![params.set_step(pattern, lane, step, next)]));
            }
        }

        let hint = if s.on {
            format!("{what}: {}\nClick to remove · drag up or down for velocity\nShift+click ratchet · Ctrl+click chance · Alt+click ends the lane here", describe(s))
        } else {
            format!("{what}: off\nClick to add · drag up or down to add it at a velocity")
        };
        let response = response.on_hover_text(hint);
        crate::widgets::menu::context_menu(&response, |ui| step_menu(ui, node_id, params, &StepAt { pattern, lane, step, lane_label: &lane_label }, s, &what, edits));
    }
}

fn length_edit(node_id: NodeId, params: &Params, lane: usize, lane_label: &str, to: usize) -> SynthResponse {
    let label = match to {
        0 => format!("{lane_label} follows the bar"),
        n => format!("{lane_label} loops {n} steps"),
    };
    edit(node_id, label, vec![(params.name(Seq::PARAM_LENGTH + lane), to as f32)])
}

/// A step's right-click menu: its velocity, chance and ratchet.
fn step_menu(ui: &mut egui::Ui, node_id: NodeId, params: &Params, at: &StepAt, s: Step, what: &str, edits: &mut Vec<SynthResponse>) {
    let &StepAt { pattern, lane, step, lane_label } = at;
    ui.label(egui::RichText::new(what).strong());
    ui.separator();
    let set = |label: String, next: Step, edits: &mut Vec<SynthResponse>| {
        edits.push(edit(node_id, label, vec![params.set_step(pattern, lane, step, next)]));
    };

    let mut on = s.on;
    if ui.checkbox(&mut on, "Plays").changed() {
        set(format!("{} {what}", if on { "Add" } else { "Remove" }), Step { on, ..s }, edits);
    }
    ui.horizontal(|ui| {
        ui.label("Velocity");
        for velocity in [100, 80, 60, 40, 20] {
            if ui.selectable_label(s.velocity == velocity, format!("{velocity}")).clicked() {
                set(format!("Velocity {velocity}% on {what}"), Step { on: true, velocity, ..s }, edits);
                ui.close();
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Chance").on_hover_text("How often the step plays when its turn comes (Ctrl+click)");
        for probability in PROBABILITIES {
            if ui.selectable_label(s.probability == probability, format!("{probability}%")).clicked() {
                set(format!("{probability}% chance on {what}"), Step { on: true, probability, ..s }, edits);
                ui.close();
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Ratchet").on_hover_text("Hits spread evenly across the step, for rolls (Shift+click)");
        for ratchet in 1..=MAX_RATCHET {
            if ui.selectable_label(s.ratchet == ratchet, format!("×{ratchet}")).clicked() {
                set(format!("Ratchet ×{ratchet} on {what}"), Step { on: true, ratchet, ..s }, edits);
                ui.close();
            }
        }
    });
    ui.separator();
    let length = params.length(lane);
    if length == step + 1 {
        if ui.button(format!("{lane_label} follows the bar")).clicked() {
            edits.push(length_edit(node_id, params, lane, lane_label, 0));
            ui.close();
        }
    } else if ui.button(format!("End {lane_label} here (loops {} steps)", step + 1)).on_hover_text("Alt+click").clicked() {
        edits.push(length_edit(node_id, params, lane, lane_label, step + 1));
        ui.close();
    }
}

/// A lane's right-click menu: its length, and clearing it.
fn lane_menu(
    ui: &mut egui::Ui,
    node_id: NodeId,
    params: &Params,
    pattern: usize,
    lane: usize,
    lane_label: &str,
    edits: &mut Vec<SynthResponse>,
) {
    ui.label(egui::RichText::new(lane_label).strong());
    ui.separator();
    let length = params.length(lane);
    ui.label("Loops");
    if ui.selectable_label(length == 0, "With the bar").clicked() {
        edits.push(length_edit(node_id, params, lane, lane_label, 0));
        ui.close();
    }
    egui::Grid::new(("trigger-lengths", node_id, lane)).spacing([2.0, 2.0]).show(ui, |ui| {
        for steps in 1..=STEPS {
            if ui.selectable_label(length == steps, format!("{steps:>2}")).clicked() {
                edits.push(length_edit(node_id, params, lane, lane_label, steps));
                ui.close();
            }
            if steps % 8 == 0 {
                ui.end_row();
            }
        }
    });
    ui.separator();
    let letter = PATTERN_NAMES[pattern];
    if ui.button(format!("Clear {lane_label} in {letter}")).clicked() {
        let changes = (0..STEPS)
            .map(|step| params.set_step(pattern, lane, step, Step { on: false, ..params.step(pattern, lane, step) }))
            .collect();
        edits.push(edit(node_id, format!("Clear {lane_label} in {letter}"), changes));
        ui.close();
    }
}

/// Under the grid: tabs for the pattern being edited, and the Chain.
pub fn pattern_bar(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    zoom: f32,
) -> Vec<SynthResponse> {
    let z = zoom;
    let mut edits = Vec::new();
    let Some(params) = Params::of(graph, node_id) else { return edits };
    let reported = position(user_state, node_id);
    let width = grid_width(z);

    // Separator, as the other displays have
    ui.add_space(6.0 * z);
    let accent = crate::dsp::ModuleCategory::Utility.color();
    let left = ui.cursor().left();
    ui.painter().hline(
        (left + LABEL * z)..=(left + width),
        ui.cursor().top(),
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64)),
    );
    ui.add_space(6.0 * z);

    let bar = PatternBar {
        inset: LABEL * z,
        width,
        chain: params.chain(),
        chain_names: std::array::from_fn(|slot| params.name(Seq::PARAM_CHAIN + slot)),
        playing: reported.filter(|p| p.started).map(|p| Playing { pattern: p.pattern, chain_slot: p.chain_slot, next: p.next_pattern }),
        pattern_cv: reported.is_some_and(|p| p.pattern_cv),
    };
    edits.extend(pattern_tabs(ui, node_id, z, &bar, |ui, pattern, edits| pattern_tab_menu(ui, node_id, &params, pattern, edits)));
    edits
}

/// Where a sequencer is in its chain, as the tabs show it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Playing {
    pub pattern: usize,
    pub chain_slot: usize,
    /// The pattern that plays next.
    pub next: usize,
}

/// What a sequencer's pattern tabs and Chain show.
pub(super) struct PatternBar {
    /// Room left of the tabs, for their captions.
    pub inset: f32,
    pub width: f32,
    /// The Chain's slots: 0 for empty, else the pattern + 1.
    pub chain: [usize; CHAIN_SLOTS],
    /// The names of the Chain slots' parameters.
    pub chain_names: [String; CHAIN_SLOTS],
    /// Where it is, once a step has played.
    pub playing: Option<Playing>,
    /// Pattern is patched, so the CV picks patterns rather than the Chain.
    pub pattern_cv: bool,
}

/// Tabs for the pattern being edited, the one playing marked with a green
/// dot, and the Chain under them, its playing slot underlined. Shared by
/// the Trigger and Step Sequencers; `menu` fills a tab's right-click menu.
pub(super) fn pattern_tabs(
    ui: &mut egui::Ui,
    node_id: NodeId,
    zoom: f32,
    bar: &PatternBar,
    mut menu: impl FnMut(&mut egui::Ui, usize, &mut Vec<SynthResponse>),
) -> Vec<SynthResponse> {
    let z = zoom;
    let mut edits = Vec::new();
    let PatternBar { inset, width, pattern_cv, .. } = *bar;
    let position = bar.playing;
    let editing = edit_pattern(ui.ctx(), node_id);
    let green = theme::signal::GATE;

    let caption = FontId::proportional(9.5 * z);
    let letter_font = FontId::proportional(10.0 * z);
    let tab = Vec2::new(20.0 * z, 15.0 * z);

    // --- The pattern tabs ---
    let (row, _) = ui.allocate_exact_size(Vec2::new(width, tab.y), Sense::hover());
    let painter = ui.painter().clone();
    painter.text(Pos2::new(row.left(), row.center().y), egui::Align2::LEFT_CENTER, "Edit", caption.clone(), theme::text::SECONDARY);
    for (pattern, letter) in PATTERN_NAMES.iter().enumerate() {
        let rect = Rect::from_min_size(Pos2::new(row.left() + inset + pattern as f32 * (tab.x + 4.0 * z), row.top()), tab);
        let selected = pattern == editing;
        let playing = position.is_some_and(|p| p.pattern == pattern);
        let next = position.is_some_and(|p| p.next == pattern && p.pattern != pattern);
        let fill = if selected { theme::background::WIDGET_ACTIVE } else { theme::background::WIDGET };
        painter.rect_filled(rect, 3.0 * z, fill);
        if selected {
            painter.rect_stroke(rect, 3.0 * z, Stroke::new(1.0 * z, theme::text::SECONDARY), egui::StrokeKind::Middle);
        }
        let ink = if selected { theme::text::PRIMARY } else { theme::text::SECONDARY };
        painter.text(rect.center() - Vec2::new(2.0 * z, 0.0), egui::Align2::CENTER_CENTER, letter, letter_font.clone(), ink);
        // Green while it plays; a ring if it plays next
        let dot = Pos2::new(rect.right() - 4.5 * z, rect.center().y);
        if playing {
            painter.circle_filled(dot, 2.4 * z, green);
        } else if next {
            painter.circle_stroke(dot, 2.2 * z, Stroke::new(1.0 * z, green));
        }

        super::marks::mark(ui.ctx(), node_id, format_args!("pattern:{letter}"), rect);
        let response = ui
            .interact(rect, Id::unique((node_id, "trigger-tab", pattern)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("Edit pattern {letter}. Right-click to copy or clear it"));
        if response.clicked() {
            // Picking a pattern to edit stops the grid turning over under you
            set_edit_pattern(ui.ctx(), node_id, pattern);
            set_following(ui.ctx(), node_id, false);
        }
        crate::widgets::menu::context_menu(&response, |ui| menu(ui, pattern, &mut edits));
    }
    let after_tabs = row.left() + inset + PATTERNS as f32 * (tab.x + 4.0 * z) + 6.0 * z;
    follow_toggle(ui, node_id, z, Rect::from_min_size(Pos2::new(after_tabs, row.top()), Vec2::new(46.0 * z, tab.y)), position.map(|p| p.pattern));

    ui.add_space(3.0 * z);

    // --- The Chain ---
    let (row, _) = ui.allocate_exact_size(Vec2::new(width, tab.y), Sense::hover());
    let painter = ui.painter().clone();
    let fade = if pattern_cv { 0.4 } else { 1.0 };
    painter.text(Pos2::new(row.left(), row.center().y), egui::Align2::LEFT_CENTER, "Chain", caption.clone(), theme::text::SECONDARY);
    let chain = bar.chain;
    let filled: Vec<usize> = (0..CHAIN_SLOTS).filter(|&slot| chain[slot] > 0).collect();
    let slot_size = Vec2::new(14.0 * z, tab.y);
    let mut x = row.left() + inset;
    for (index, &slot) in filled.iter().enumerate() {
        let rect = Rect::from_min_size(Pos2::new(x, row.top()), slot_size);
        x += slot_size.x + 2.0 * z;
        let pattern = chain[slot] - 1;
        let playing = !pattern_cv && position.is_some_and(|p| p.chain_slot == index);
        painter.rect_filled(rect, 2.5 * z, theme::background::WIDGET.gamma_multiply(fade));
        let ink = if pattern == editing { theme::text::PRIMARY } else { theme::text::SECONDARY };
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, PATTERN_NAMES[pattern], letter_font.clone(), ink.gamma_multiply(fade));
        if playing {
            painter.hline(rect.x_range().shrink(2.0 * z), rect.bottom() - 1.0 * z, Stroke::new(1.6 * z, green));
        }

        let response = ui
            .interact(rect, Id::unique((node_id, "trigger-chain", slot)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("A bar of this pattern. Click for the next pattern; right-click to pick one or remove it");
        if response.clicked() {
            let next = (pattern + 1) % PATTERNS;
            edits.push(edit(
                node_id,
                format!("Chain {} → {}", index + 1, PATTERN_NAMES[next]),
                vec![(bar.chain_names[slot].clone(), (next + 1) as f32)],
            ));
        }
        crate::widgets::menu::context_menu(&response, |ui| {
            for (choice, letter) in PATTERN_NAMES.iter().enumerate() {
                if ui.selectable_label(choice == pattern, *letter).clicked() {
                    edits.push(edit(
                        node_id,
                        format!("Chain {} → {letter}", index + 1),
                        vec![(bar.chain_names[slot].clone(), (choice + 1) as f32)],
                    ));
                    ui.close();
                }
            }
            if filled.len() > 1 {
                ui.separator();
                if ui.button("Remove").clicked() {
                    // The rest move up, so the chain has no gaps
                    let mut rest: Vec<usize> = filled.iter().filter(|&&s| s != slot).map(|&s| chain[s]).collect();
                    rest.resize(CHAIN_SLOTS, 0);
                    let changes = rest.iter().enumerate().map(|(s, &v)| (bar.chain_names[s].clone(), v as f32)).collect();
                    edits.push(edit(node_id, format!("Remove chain {}", index + 1), changes));
                    ui.close();
                }
            }
        });
    }

    // Add a bar: another of the pattern being edited
    if filled.len() < CHAIN_SLOTS {
        let rect = Rect::from_min_size(Pos2::new(x, row.top()), slot_size);
        let response = ui
            .interact(rect, Id::unique((node_id, "trigger-chain-add")), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("Add a bar of {} to the chain", PATTERN_NAMES[editing]));
        let ink = if response.hovered() { theme::text::PRIMARY } else { theme::text::DISABLED };
        painter.rect_stroke(rect.shrink(0.5 * z), 2.5 * z, Stroke::new(1.0 * z, ink.gamma_multiply(fade)), egui::StrokeKind::Middle);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "+", letter_font.clone(), ink.gamma_multiply(fade));
        x += slot_size.x + 2.0 * z;
        if response.clicked() {
            // Packed to the front, so the new bar comes last
            let mut slots: Vec<usize> = filled.iter().map(|&s| chain[s]).collect();
            slots.push(editing + 1);
            slots.resize(CHAIN_SLOTS, 0);
            let changes: Vec<(String, f32)> = slots.iter().enumerate().map(|(s, &v)| (bar.chain_names[s].clone(), v as f32)).collect();
            edits.push(edit(node_id, format!("Add {} to the chain", PATTERN_NAMES[editing]), changes));
        }
    }
    if pattern_cv {
        painter.text(
            Pos2::new(x + 4.0 * z, row.center().y),
            egui::Align2::LEFT_CENTER,
            "Pattern CV picks",
            caption,
            theme::signal::CONTROL,
        );
    }
    ui.add_space(4.0 * z);
    edits
}

/// Follow, after the tabs, in `rect`: while it's on, the grid shows whichever
/// pattern is playing. Green like the playing dot when on, and sized in
/// patch units so it holds still as the view zooms.
fn follow_toggle(ui: &mut egui::Ui, node_id: NodeId, z: f32, rect: Rect, playing: Option<usize>) {
    let was_on = following(ui.ctx(), node_id);
    let green = theme::signal::GATE;
    super::marks::mark(ui.ctx(), node_id, format_args!("follow"), rect);
    let response = ui
        .interact(rect, Id::unique((node_id, "trigger-follow")), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if was_on {
            "Following the pattern playing. Click to stop, or click a tab to edit one"
        } else {
            "Follow the pattern playing, so the grid turns over as the Chain moves on"
        });
    let on = was_on != response.clicked();
    if response.clicked() {
        set_following(ui.ctx(), node_id, on);
        if let Some(pattern) = playing.filter(|_| on) {
            set_edit_pattern(ui.ctx(), node_id, pattern);
        }
    }

    let painter = ui.painter();
    let (fill, stroke, ink) = if on {
        (green.gamma_multiply(0.18), green.gamma_multiply(0.8), theme::text::PRIMARY)
    } else if response.hovered() {
        (theme::background::WIDGET_ACTIVE, theme::text::SECONDARY, theme::text::PRIMARY)
    } else {
        (theme::background::WIDGET, Color32::TRANSPARENT, theme::text::SECONDARY)
    };
    painter.rect_filled(rect, 3.0 * z, fill);
    painter.rect_stroke(rect, 3.0 * z, Stroke::new(1.0 * z, stroke), egui::StrokeKind::Middle);
    // A playhead's arrow, filled green while it follows
    let tip = Pos2::new(rect.left() + 10.0 * z, rect.center().y);
    let arrow = vec![tip + Vec2::new(-4.0 * z, -3.5 * z), tip + Vec2::new(1.0 * z, 0.0), tip + Vec2::new(-4.0 * z, 3.5 * z)];
    if on {
        painter.add(Shape::convex_polygon(arrow, green, Stroke::NONE));
    } else {
        painter.add(Shape::closed_line(arrow, Stroke::new(1.0 * z, ink)));
    }
    painter.text(Pos2::new(rect.left() + 13.0 * z, rect.center().y), egui::Align2::LEFT_CENTER, "Follow", FontId::proportional(9.5 * z), ink);
}

/// A pattern tab's right-click menu: copy it to another pattern, or clear it.
fn pattern_tab_menu(ui: &mut egui::Ui, node_id: NodeId, params: &Params, pattern: usize, edits: &mut Vec<SynthResponse>) {
    let hints = MenuHints { copy: "Steps and accents, to start a variation or a fill from", clear: "Every step off and every accent gone, keeping their ratchets, chances and velocities" };
    let copy_to = |to| {
        let mut changes = Vec::with_capacity(STEPS * (LANES + 1));
        for step in 0..STEPS {
            changes.push((params.name(Seq::accent_param(to, step)), params.value(Seq::accent_param(pattern, step))));
            for lane in 0..LANES {
                changes.push(params.set_step(to, lane, step, params.step(pattern, lane, step)));
            }
        }
        changes
    };
    let clear = || {
        let mut changes = Vec::with_capacity(STEPS * (LANES + 1));
        for step in 0..STEPS {
            changes.push((params.name(Seq::accent_param(pattern, step)), 0.0));
            for lane in 0..LANES {
                changes.push(params.set_step(pattern, lane, step, Step { on: false, ..params.step(pattern, lane, step) }));
            }
        }
        changes
    };
    pattern_menu(ui, node_id, pattern, &hints, copy_to, clear, edits);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::trigger_sequencer::CHAIN_CHOICES;

    #[test]
    fn a_kits_parts_are_named_short_enough_for_a_lane() {
        assert_eq!(short_part("Track 3"), "T3");
        assert_eq!(short_part("Pad 13"), "P13");
        assert_eq!(short_part("Kick"), "Kick");
        assert_eq!(short_part("Low Tom"), "Low Tom", "a name, not a number, is kept");
    }

    #[test]
    fn chain_choices_name_the_patterns() {
        assert_eq!(&CHAIN_CHOICES[1..], &PATTERN_NAMES);
    }

    #[test]
    fn the_grid_fits_sixteen_steps_in_four_beats() {
        // Each beat's first step starts a beat gap after the last beat's end
        let z = 1.0;
        let beat = step_left(4, z) - step_left(3, z);
        assert!((beat - (CELL + GAP + BEAT)).abs() < 1e-4);
        assert!((grid_width(z) - (LABEL + 16.0 * CELL + 15.0 * GAP + 3.0 * BEAT)).abs() < 1e-3);
    }
}
