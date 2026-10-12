//! The Step Sequencer's grid: a button per step in rows of eight, with its
//! note underneath.
//!
//! - Click switches a step's gate, Shift+click its tie, Ctrl+click its slide.
//! - Drag a step up or down to move its note a semitone at a time, or with
//!   Shift an octave at a time. The note it will land on shows above it.
//! - Right-click opens a two-octave piano under the step, for writing notes
//!   the way a hardware sequencer's step-record does: each key played writes
//!   the step and moves along to the next one, so a melody goes in key after
//!   key. The piano stays put, and the step it will write next is outlined
//!   in the pitch cable's orange.
//!
//! A tied step reaches across the gap into the next one, the way a held note
//! looks on a piano roll. At the end of a row it reaches out of its right
//! side and into the next step's left. At the end of the pattern it goes
//! wherever the Chain goes next: round into step 1 when that's this pattern
//! again, or off the edge toward another, with its letter (`→B`). Step 1 has
//! a tab into its left side when the pattern before it in the Chain ties
//! into it. With the Pattern CV patched the next pattern isn't known, so the
//! tie only leaves.
//!
//! A slide step has a slur over it from the step before, in the pitch cable's
//! orange, since it's the pitch that glides. A tie is about the gate, so it
//! stays green. The slur arches over the tops of the two steps it joins, and
//! where they're on different rows it breaks at its peak, the way a printed
//! slur breaks at the end of a line.
//!
//! Under the steps are the Trigger Sequencer's tabs: one for each pattern,
//! A to D, which the grid shows and edits, and the Chain that plays them.

use std::collections::HashMap;

use eframe::egui::{self, Color32, CursorIcon, Id, Key, LayerId, Modifiers, Order, PopupCloseBehavior, RichText, Sense};
use egui_node_graph2::{NodeId, NodeResponse};

use crate::app::theme;
use crate::modules::sequencer::{note_to_name, PatternPosition, StepField, StepSequencer as Seq, MAX_STEPS, PATTERNS, PATTERN_NAMES};
use crate::modules::trigger_sequencer::{chain_from, CHAIN_NAMES};
use crate::widgets::{piano_keys, PianoConfig, PianoData};

use super::trigger_display::{edit_pattern, pattern_tabs, PatternBar, Playing};
use super::{SynthGraph, SynthGraphState, SynthNodeData, SynthResponse};

const GATE_ON: Color32 = Color32::from_rgb(100, 200, 100);
const GATE_OFF: Color32 = Color32::from_rgb(60, 60, 70);
/// Points of upward drag per semitone, before zoom: an octave is 72.
const DRAG_PER_SEMITONE: f32 = 6.0;
/// Points of drag per octave with Shift held, before zoom.
const DRAG_PER_OCTAVE: f32 = 18.0;
/// The highest C the piano's lower octave can start on, so its two octaves
/// stay inside MIDI's 0–127.
const TOP_BASE: u8 = 96;
/// One octave of the piano, in points.
const OCTAVE_WIDTH: f32 = 140.0;
const KEY_HEIGHT: f32 = 48.0;
/// The gap between steps, before zoom.
const STEP_SPACING: f32 = 3.0;
const HINT: &str = "Click: gate · Shift+click: tie · Ctrl+click: slide\nDrag up or down: note (Shift: by octave)\nRight-click: piano";

/// The grid's width, before zoom.
const GRID_WIDTH: f32 = 220.0;
/// Room left of the pattern tabs for their captions, before zoom.
const TAB_INSET: f32 = 34.0;
/// The Step output, which the playhead follows.
const OUT_STEP: usize = 3;

/// A Step Sequencer's pattern, as its grid shows it.
pub(super) struct StepPattern {
    /// Which pattern, A to D, as 0 to 3.
    pub pattern: usize,
    pub steps: usize,
    /// The step sounding now, if it's in this pattern.
    pub current: Option<usize>,
    pub pitches: [u8; 16],
    pub gates: [bool; 16],
    pub ties: [bool; 16],
    /// Steps slurred into from the step before.
    pub slides: [bool; 16],
    /// Where a tie on the last step goes, and whether one arrives on step 1.
    pub seam: Seam,
}

/// How a pattern joins the patterns either side of it in the Chain, as far
/// as ties go: a tie on its last step carries into whatever pattern plays
/// next, which isn't always this one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Seam {
    /// The Chain comes round to this pattern again, so a tie on its last
    /// step goes into its own step 1.
    pub home: bool,
    /// The other patterns a tie on the last step goes into.
    pub onward: [bool; PATTERNS],
    /// Another pattern that plays just before this one ties out of its last
    /// step, into this one's step 1.
    pub arrives: bool,
}

impl Seam {
    /// The seam for `pattern` in `chain`. `tied_out` says whether a pattern
    /// ties out of its last step. With the Pattern CV patched the next
    /// pattern isn't known, so there's nothing either side. A pattern the
    /// Chain doesn't play is drawn as though it played on its own.
    fn of(pattern: usize, chain: &[usize], pattern_cv: bool, tied_out: impl Fn(usize) -> bool) -> Self {
        let mut seam = Self::default();
        if pattern_cv {
            return seam;
        }
        let len = chain.len();
        let slots: Vec<usize> = (0..len).filter(|&slot| chain[slot] == pattern).collect();
        if slots.is_empty() {
            seam.home = true;
        }
        for slot in slots {
            match chain[(slot + 1) % len] {
                next if next == pattern => seam.home = true,
                next => seam.onward[next] = true,
            }
            let before = chain[(slot + len - 1) % len];
            seam.arrives |= before != pattern && tied_out(before);
        }
        seam
    }

    /// The patterns a tie leaves for, as a caption: `→B`, or `→B·C`.
    fn caption(&self) -> Option<String> {
        let names: Vec<&str> = (0..PATTERNS).filter(|&p| self.onward[p]).map(|p| PATTERN_NAMES[p]).collect();
        (!names.is_empty()).then(|| format!("→{}", names.join("·")))
    }
}

impl StepPattern {
    /// The parameter that holds one of a step's fields in this pattern.
    fn param(&self, step: usize, field: StepField) -> String {
        Seq::step_param_name(self.pattern, step, field).to_string()
    }
}

/// A Step Sequencer node's parameter values, by name.
struct Values(HashMap<String, f32>);

impl Values {
    fn of(graph: &SynthGraph, node_id: NodeId) -> Option<Self> {
        let node = graph.nodes.get(node_id)?;
        Some(Self(node.inputs.iter().map(|(name, id)| (name.clone(), graph.get_input(*id).value.actual_value())).collect()))
    }

    fn get(&self, name: &str) -> Option<f32> {
        self.0.get(name).copied()
    }

    fn field(&self, pattern: usize, step: usize, field: StepField) -> Option<f32> {
        self.get(Seq::step_param_name(pattern, step, field))
    }
}

/// The Step Sequencer's display: the grid for the pattern being edited,
/// and under it the pattern tabs and the Chain.
pub(super) fn step_sequencer_display(
    ui: &mut egui::Ui,
    zoom: f32,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
) {
    let Some(values) = Values::of(graph, node_id) else { return };
    let editing = edit_pattern(ui.ctx(), node_id);
    let engine_id = user_state.get_engine_node_id(node_id);
    let position = engine_id.and_then(|id| user_state.readouts.get(&id)).map(PatternPosition::from_readout);

    let steps = values.get("Steps").map_or(8, |v| (v as usize).clamp(1, MAX_STEPS));
    // The playhead only shows on the pattern playing
    let current = (position.map_or(0, |p| p.pattern) == editing).then(|| {
        engine_id
            .and_then(|id| user_state.get_output_value(id, OUT_STEP))
            .map_or(0, |v| ((v * (steps - 1).max(1) as f32).round() as usize).min(steps - 1))
    });
    let (chain, len) = chain_from(&CHAIN_NAMES.map(|name| values.get(name).unwrap_or(0.0)));
    let pattern_cv = position.is_some_and(|p| p.pattern_cv);
    let tied_out = |pattern: usize| {
        let field = |field| values.field(pattern, steps - 1, field);
        field(StepField::Tie).is_some_and(|v| v > 0.5) && field(StepField::Gate).is_none_or(|v| v > 0.5)
    };
    let pattern = StepPattern {
        seam: Seam::of(editing, &chain[..len], pattern_cv, tied_out),
        pattern: editing,
        steps,
        current,
        pitches: std::array::from_fn(|step| values.field(editing, step, StepField::Pitch).map_or(60, |v| v as u8)),
        gates: std::array::from_fn(|step| values.field(editing, step, StepField::Gate).is_none_or(|v| v > 0.5)),
        ties: std::array::from_fn(|step| values.field(editing, step, StepField::Tie).is_some_and(|v| v > 0.5)),
        slides: std::array::from_fn(|step| values.field(editing, step, StepField::Slide).is_some_and(|v| v > 0.5)),
    };
    step_grid(ui, zoom, &pattern, node_id, responses);

    ui.add_space(6.0 * zoom);
    let bar = PatternBar {
        inset: TAB_INSET * zoom,
        width: GRID_WIDTH * zoom,
        chain: std::array::from_fn(|slot| values.get(CHAIN_NAMES[slot]).map_or(0, |v| (v.round().max(0.0) as usize).min(PATTERNS))),
        chain_names: std::array::from_fn(|slot| CHAIN_NAMES[slot].to_string()),
        playing: position.filter(|p| p.started).map(|p| Playing { pattern: p.pattern, chain_slot: p.chain_slot, next: p.next_pattern }),
        pattern_cv,
    };
    let edits = pattern_tabs(ui, node_id, zoom, &bar, |ui, pattern, edits| pattern_menu(ui, node_id, &values, pattern, edits));
    responses.extend(edits.into_iter().map(NodeResponse::User));
}

/// A pattern tab's right-click menu: copy the pattern to another, or clear
/// it to rests, ready to write a new line over with the piano.
fn pattern_menu(ui: &mut egui::Ui, node_id: NodeId, values: &Values, pattern: usize, edits: &mut Vec<SynthResponse>) {
    const FIELDS: [StepField; 5] = StepField::ALL;
    let from = PATTERN_NAMES[pattern];
    let edit = |label: String, changes: Vec<(String, f32)>| SynthResponse::EditParameters { node_id, label, changes };
    ui.label(RichText::new(format!("Pattern {from}")).strong());
    ui.separator();
    for to in (0..PATTERNS).filter(|&p| p != pattern) {
        let copy = ui.button(format!("Copy to {}", PATTERN_NAMES[to])).on_hover_text("Every step's note, gate, velocity, tie and slide, to start a variation from");
        if copy.clicked() {
            let mut changes = Vec::with_capacity(MAX_STEPS * FIELDS.len());
            for step in 0..MAX_STEPS {
                for field in FIELDS {
                    if let Some(value) = values.field(pattern, step, field) {
                        changes.push((Seq::step_param_name(to, step, field).to_string(), value));
                    }
                }
            }
            edits.push(edit(format!("Copy pattern {from} to {}", PATTERN_NAMES[to]), changes));
            ui.close_menu();
        }
    }
    ui.separator();
    if ui.button(format!("Clear {from}")).on_hover_text("Every step a rest, keeping its note").clicked() {
        let changes = (0..MAX_STEPS)
            .flat_map(|step| {
                [StepField::Gate, StepField::Tie, StepField::Slide].map(|field| (Seq::step_param_name(pattern, step, field).to_string(), 0.0))
            })
            .collect();
        edits.push(edit(format!("Clear pattern {from}"), changes));
        ui.close_menu();
    }
}



/// The piano's place in the pattern while it's open: the step its next key
/// writes, the C its lower octave starts on, and the step it opened under.
/// It stays under that one as the writing moves along, so the keys don't
/// slide away from the hand playing them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StepEntry {
    step: usize,
    base: u8,
    anchor: usize,
}

/// A drag under way: the step, and its note when the drag began.
#[derive(Clone, Copy, Debug)]
struct StepDrag {
    step: usize,
    from: u8,
}

/// The note a drag lands on: `dy` points down the screen from where it
/// began, so dragging up raises the note.
pub(super) fn dragged_pitch(from: u8, dy: f32, zoom: f32, octaves: bool) -> u8 {
    let semitones = if octaves {
        (-dy / (DRAG_PER_OCTAVE * zoom)).round() as i32 * 12
    } else {
        (-dy / (DRAG_PER_SEMITONE * zoom)).round() as i32
    };
    (from as i32 + semitones).clamp(0, 127) as u8
}

/// Where the piano opens for a step's note: the note in its upper octave,
/// so there's an octave below it and most of one above.
fn popover_base(pitch: u8) -> u8 {
    shift_base(pitch / 12 * 12, -12)
}

/// The piano moved by `semitones` (a multiple of 12), kept on the keyboard.
fn shift_base(base: u8, semitones: i32) -> u8 {
    (base as i32 + semitones).clamp(0, TOP_BASE as i32) as u8
}

/// The step `by` steps along from `step`, round the loop.
fn step_along(step: usize, by: isize, steps: usize) -> usize {
    (step as isize + by).rem_euclid(steps.max(1) as isize) as usize
}

pub(super) fn step_grid(
    ui: &mut egui::Ui,
    zoom: f32,
    pattern: &StepPattern,
    node_id: NodeId,
    responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
) {
    let step_size = 24.0 * zoom;
    let step_spacing = STEP_SPACING * zoom;
    let mut set = |param_name: String, value: f32| {
        responses.push(NodeResponse::User(SynthResponse::ParameterChanged { node_id, param_name, value }));
    };

    let entry_id = Id::new(("step_entry", node_id));
    let drag_id = Id::new(("step_drag", node_id));
    let mut entry = ui
        .memory(|m| m.is_popup_open(entry_id))
        .then(|| ui.data(|d| d.get_temp::<StepEntry>(entry_id)))
        .flatten()
        .map(|e| StepEntry { step: e.step.min(pattern.steps - 1), anchor: e.anchor.min(pattern.steps - 1), ..e });

    ui.vertical(|ui| {
        ui.set_min_width(GRID_WIDTH * zoom);

        // Lay out every step first, so a tie can be drawn under both ends
        let mut cells = Vec::with_capacity(pattern.steps);
        for row_start in (0..pattern.steps).step_by(8) {
            if row_start > 0 {
                ui.add_space(2.0 * zoom);
            }
            ui.horizontal(|ui| {
                for _ in row_start..(row_start + 8).min(pattern.steps) {
                    let (rect, response) = ui
                        .allocate_exact_size(egui::vec2(step_size, step_size + 12.0 * zoom), Sense::click_and_drag());
                    cells.push((egui::Rect::from_min_size(rect.min, egui::vec2(step_size, step_size)), response.on_hover_text(HINT)));
                    ui.add_space(step_spacing);
                }
            });
        }

        // Ties, under the steps
        let painter = ui.painter();
        let band = step_size * 0.22;
        let reach = step_spacing * 2.0;
        let bar = |left: f32, right: f32, y: f32| egui::Rect::from_min_max(egui::pos2(left, y - band), egui::pos2(right, y + band));
        let tab_in = |to: egui::Rect| painter.rect_filled(bar(to.left() - reach, to.center().x, to.center().y), band, GATE_ON);
        let last = pattern.steps - 1;
        for step in (0..pattern.steps).filter(|&s| pattern.ties[s] && pattern.gates[s]) {
            let from = cells[step].0;
            let to = cells[(step + 1) % pattern.steps].0;
            if step < last && (to.center().y - from.center().y).abs() < 1.0 {
                painter.rect_filled(bar(from.center().x, to.center().x, from.center().y), 0.0, GATE_ON);
                continue;
            }
            painter.rect_filled(bar(from.center().x, from.right() + reach, from.center().y), band, GATE_ON);
            if step < last || pattern.seam.home {
                tab_in(to);
            }
            // Off the end of the pattern, into the next one in the Chain
            if let Some(caption) = pattern.seam.caption().filter(|_| step == last) {
                painter.text(
                    egui::pos2(from.right() + reach + 1.5 * zoom, from.center().y),
                    egui::Align2::LEFT_CENTER,
                    caption,
                    egui::FontId::proportional(9.0 * zoom),
                    GATE_ON,
                );
            }
        }
        // A tie from the end of the pattern before, unless it's drawn already
        if pattern.seam.arrives && !(pattern.seam.home && pattern.ties[last] && pattern.gates[last]) {
            tab_in(cells[0].0);
        }

        for (step, (step_rect, response)) in cells.iter().enumerate() {
            let is_current = pattern.current == Some(step);
            let base_color = if pattern.gates[step] { GATE_ON } else { GATE_OFF };
            let color = if is_current {
                // Brighten current step
                Color32::from_rgb(
                    (base_color.r() as u16 + 100).min(255) as u8,
                    (base_color.g() as u16 + 100).min(255) as u8,
                    (base_color.b() as u16 + 50).min(255) as u8,
                )
            } else {
                base_color
            };
            painter.rect_filled(*step_rect, 3.0, color);
            if is_current {
                painter.rect_stroke(*step_rect, 3.0, egui::Stroke::new(2.0, Color32::WHITE));
            }

            // Note name below
            let pitch = pattern.pitches[step];
            painter.text(
                egui::pos2(step_rect.center().x, step_rect.bottom() + 2.0 * zoom),
                egui::Align2::CENTER_TOP,
                note_to_name(pitch),
                egui::FontId::proportional(8.0 * zoom),
                Color32::from_gray(180),
            );

            if response.clicked() {
                let modifiers = ui.input(|i| i.modifiers);
                if modifiers.shift {
                    set(pattern.param(step, StepField::Tie), if pattern.ties[step] { 0.0 } else { 1.0 });
                } else if modifiers.command {
                    set(pattern.param(step, StepField::Slide), if pattern.slides[step] { 0.0 } else { 1.0 });
                } else {
                    set(pattern.param(step, StepField::Gate), if pattern.gates[step] { 0.0 } else { 1.0 });
                }
            }
            if response.secondary_clicked() {
                entry = Some(StepEntry { step, base: popover_base(pitch), anchor: step });
                ui.memory_mut(|m| m.open_popup(entry_id));
            }

            // Dragging up or down moves the note
            if response.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, StepDrag { step, from: pitch }));
            }
            let drag = ui.data(|d| d.get_temp::<StepDrag>(drag_id)).filter(|d| d.step == step);
            if let Some(drag) = drag.filter(|_| response.dragged()) {
                let (origin, now, octaves) =
                    ui.input(|i| (i.pointer.press_origin(), i.pointer.interact_pos(), i.modifiers.shift));
                if let (Some(origin), Some(now)) = (origin, now) {
                    let to = dragged_pitch(drag.from, now.y - origin.y, zoom, octaves);
                    if to != pitch {
                        set(pattern.param(step, StepField::Pitch), to as f32);
                    }
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
                    drag_badge(ui, node_id, *step_rect, &note_to_name(to), zoom);
                }
            }
            if response.drag_stopped() {
                ui.data_mut(|d| d.remove::<StepDrag>(drag_id));
            }
        }

        // Slurs, over the steps. A slide after a rest is struck, so its slur
        // is only a faint mark. Step 1's note before may be in another
        // pattern, so it's always drawn whole
        for step in (0..pattern.steps).filter(|&s| pattern.slides[s] && pattern.gates[s]) {
            let before = (step + pattern.steps - 1) % pattern.steps;
            let alpha = if step == 0 || pattern.gates[before] { 255 } else { 90 };
            slur(painter, cells[before].0, cells[step].0, step == 0, alpha, zoom);
        }

        let Some(mut open) = entry else {
            return;
        };
        // The step the next key writes
        painter.rect_stroke(cells[open.step].0.expand(2.0 * zoom), 4.0, egui::Stroke::new(2.0, theme::signal::CONTROL));

        let (back, on, escape) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            ui.memory_mut(|m| m.close_popup());
        }
        open.step = step_along(open.step, on as isize - back as isize, pattern.steps);

        let anchor = &cells[open.anchor].1;
        egui::popup_below_widget(ui, entry_id, anchor, PopupCloseBehavior::CloseOnClickOutside, |ui| {
            step_piano(ui, &mut open, pattern, &mut set);
        });
        if ui.memory(|m| m.is_popup_open(entry_id)) {
            ui.data_mut(|d| d.insert_temp(entry_id, open));
        } else {
            ui.data_mut(|d| d.remove::<StepEntry>(entry_id));
        }
    });
}

/// The slur from the step `from` into the slide step `to`: an arch from the
/// top of one to the top of the other. Where the slide comes from the row
/// above, or round from the pattern's end, the arch is broken at its peak:
/// half leaves `from` to the right and half arrives at `to` from the left.
fn slur(painter: &egui::Painter, from: egui::Rect, to: egui::Rect, wraps: bool, alpha: u8, zoom: f32) {
    use egui::{epaint::CubicBezierShape, pos2, vec2, Pos2};

    let color = theme::signal::CONTROL;
    let color = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
    let shadow = Color32::from_black_alpha(alpha / 2);
    // Control points this far above the feet put the arch's peak three
    // quarters of the way up, a little above the steps' tops
    let rise = vec2(0.0, -7.0 * zoom);
    let foot = |rect: egui::Rect| pos2(rect.center().x, rect.top() + 1.5 * zoom);
    let draw = |points: [Pos2; 4]| {
        for (width, color) in [(3.5 * zoom, shadow), (1.8 * zoom, color)] {
            painter.add(CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, egui::Stroke::new(width, color)));
        }
    };
    let arch = |a: Pos2, b: Pos2| [a, a + rise, b + rise, b];

    if !wraps && (to.center().y - from.center().y).abs() < 1.0 {
        draw(arch(foot(from), foot(to)));
    } else {
        // The arch between neighbouring steps, split at its middle
        let along = vec2(to.width() + STEP_SPACING * zoom, 0.0);
        let [a0, a1, a2, a3] = arch(foot(from), foot(from) + along).map(|p| p.to_vec2());
        draw([a0, (a0 + a1) / 2.0, (a0 + 2.0 * a1 + a2) / 4.0, (a0 + 3.0 * a1 + 3.0 * a2 + a3) / 8.0].map(|v| v.to_pos2()));
        let [b0, b1, b2, b3] = arch(foot(to) - along, foot(to)).map(|p| p.to_vec2());
        draw([(b0 + 3.0 * b1 + 3.0 * b2 + b3) / 8.0, (b1 + 2.0 * b2 + b3) / 4.0, (b2 + b3) / 2.0, b3].map(|v| v.to_pos2()));
    }
}

/// The note a drag will land on, floated above its step where the node's
/// edge can't clip it.
pub(super) fn drag_badge(ui: &egui::Ui, node_id: NodeId, step: egui::Rect, name: &str, zoom: f32) {
    let painter = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new(("step_drag_badge", node_id))));
    let font = egui::FontId::proportional((12.0 * zoom).max(11.0));
    let galley = painter.layout_no_wrap(name.to_string(), font, Color32::from_rgb(30, 22, 12));
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = egui::Rect::from_center_size(egui::pos2(step.center().x, step.top() - 4.0 * zoom - size.y / 2.0), size);
    painter.rect_filled(rect, 4.0, theme::signal::CONTROL);
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::PLACEHOLDER);
}

/// The piano under a step: two octaves, the step's note lit. A key writes
/// its note to the step, switches a rest on, and moves along to the next step.
fn step_piano(ui: &mut egui::Ui, open: &mut StepEntry, pattern: &StepPattern, set: &mut impl FnMut(String, f32)) {
    let step = open.step;
    let pitch = pattern.pitches[step];

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} · Step {}", PATTERN_NAMES[pattern.pattern], step + 1)).strong());
        ui.label(RichText::new(note_to_name(pitch)).color(theme::signal::CONTROL));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("▸").on_hover_text("Next step (→)").clicked() {
                open.step = step_along(step, 1, pattern.steps);
            }
            if ui.small_button("◂").on_hover_text("Previous step (←)").clicked() {
                open.step = step_along(step, -1, pattern.steps);
            }
        });
    });

    ui.horizontal(|ui| {
        if ui.small_button("‹").on_hover_text("Octave down").clicked() {
            open.base = shift_base(open.base, -12);
        }
        let range = format!("{} – {}", note_to_name(open.base), note_to_name(open.base + 23));
        ui.label(RichText::new(range).weak());
        if ui.small_button("›").on_hover_text("Octave up").clicked() {
            open.base = shift_base(open.base, 12);
        }
    });

    let config = PianoConfig::scale(theme::signal::CONTROL).with_size(OCTAVE_WIDTH, KEY_HEIGHT);
    let played = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            let mut played = None;
            for octave in [open.base, open.base + 12] {
                let lit = (octave..octave + 12).contains(&pitch);
                let data = PianoData { active_notes: if lit { vec![pitch] } else { Vec::new() }, base_note: octave, ..Default::default() };
                let (response, key) = piano_keys(ui, &data, &config);
                if let Some(key) = key {
                    let note = octave + key;
                    if response.on_hover_text(note_to_name(note)).clicked() {
                        played = Some(note);
                    }
                }
            }
            played
        })
        .inner;
    if let Some(note) = played {
        set(pattern.param(step, StepField::Pitch), note as f32);
        if !pattern.gates[step] {
            set(pattern.param(step, StepField::Gate), 1.0);
        }
        open.step = step_along(step, 1, pattern.steps);
    }

    ui.separator();
    let mut tie = pattern.ties[step];
    let hint = "Holds this note into the next step, which continues it without a new attack (Shift+click)";
    if ui.checkbox(&mut tie, "Tie into next step").on_hover_text(hint).changed() {
        set(pattern.param(step, StepField::Tie), if tie { 1.0 } else { 0.0 });
    }
    let mut slide = pattern.slides[step];
    let hint = "Glides into this note from the one before, without a new attack (Ctrl+click)";
    if ui.checkbox(&mut slide, "Slide into this step").on_hover_text(hint).changed() {
        set(pattern.param(step, StepField::Slide), if slide { 1.0 } else { 0.0 });
    }
    ui.label(RichText::new("Each key writes this step, then moves to the next").small().weak());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_up_raises_the_note_a_semitone_per_six_points() {
        assert_eq!(dragged_pitch(60, 0.0, 1.0, false), 60);
        assert_eq!(dragged_pitch(60, -6.0, 1.0, false), 61);
        assert_eq!(dragged_pitch(60, -72.0, 1.0, false), 72);
        assert_eq!(dragged_pitch(60, 12.0, 1.0, false), 58);
        // Under half a semitone's travel stays put
        assert_eq!(dragged_pitch(60, -2.9, 1.0, false), 60);
    }

    #[test]
    fn drag_distance_grows_with_zoom() {
        assert_eq!(dragged_pitch(60, -12.0, 2.0, false), 61);
    }

    #[test]
    fn shift_drags_by_octaves() {
        assert_eq!(dragged_pitch(60, -18.0, 1.0, true), 72);
        assert_eq!(dragged_pitch(60, 36.0, 1.0, true), 36);
        assert_eq!(dragged_pitch(64, -5.0, 1.0, true), 64);
    }

    #[test]
    fn drags_stop_at_the_ends_of_midi() {
        assert_eq!(dragged_pitch(125, -60.0, 1.0, false), 127);
        assert_eq!(dragged_pitch(2, 60.0, 1.0, false), 0);
        assert_eq!(dragged_pitch(120, -18.0, 1.0, true), 127);
    }

    #[test]
    fn the_piano_opens_with_the_note_in_its_upper_octave() {
        assert_eq!(popover_base(60), 48); // C4 shows C3–B4
        assert_eq!(popover_base(71), 48);
        assert_eq!(popover_base(5), 0);
        assert_eq!(popover_base(127), TOP_BASE);
    }

    #[test]
    fn the_piano_stays_on_the_keyboard() {
        assert_eq!(shift_base(48, 12), 60);
        assert_eq!(shift_base(0, -12), 0);
        assert_eq!(shift_base(TOP_BASE, 12), TOP_BASE);
        assert!(TOP_BASE as u32 + 23 <= 127);
    }

    #[test]
    fn stepping_wraps_round_the_pattern() {
        assert_eq!(step_along(3, 1, 8), 4);
        assert_eq!(step_along(7, 1, 8), 0);
        assert_eq!(step_along(0, -1, 5), 4);
        assert_eq!(step_along(2, 0, 5), 2);
    }

    const A: usize = 0;
    const B: usize = 1;
    const C: usize = 2;
    const D: usize = 3;

    #[test]
    fn a_chain_of_one_ties_round_into_itself() {
        let seam = Seam::of(A, &[A], false, |_| true);
        assert_eq!(seam, Seam { home: true, ..Seam::default() });
        assert_eq!(seam.caption(), None);
    }

    #[test]
    fn a_tie_leaves_toward_the_next_pattern_and_arrives_there() {
        let a = Seam::of(A, &[A, B], false, |p| p == A);
        assert_eq!((a.home, a.onward, a.arrives), (false, [false, true, false, false], false));
        assert_eq!(a.caption().as_deref(), Some("→B"));
        let b = Seam::of(B, &[A, B], false, |p| p == A);
        assert_eq!((b.home, b.arrives, b.caption().as_deref()), (false, true, Some("→A")));
        // Nothing arrives when the pattern before doesn't tie out
        assert!(!Seam::of(B, &[A, B], false, |_| false).arrives);
    }

    #[test]
    fn a_pattern_played_twice_joins_every_neighbour() {
        // A A B: A comes round to itself and also leaves for B
        let a = Seam::of(A, &[A, A, B], false, |p| p == A);
        assert!(a.home && a.onward[B] && !a.arrives);
        // A B A C: A leaves for B and C, and both tie back into it
        let a = Seam::of(A, &[A, B, A, C], false, |p| p == C);
        assert_eq!((a.home, a.arrives, a.caption().as_deref()), (false, true, Some("→B·C")));
    }

    #[test]
    fn a_pattern_off_the_chain_looks_as_though_it_played_alone() {
        assert_eq!(Seam::of(D, &[A, B], false, |_| true), Seam { home: true, ..Seam::default() });
    }

    #[test]
    fn with_the_pattern_cv_patched_a_tie_only_leaves() {
        assert_eq!(Seam::of(A, &[A, B], true, |_| true), Seam::default());
        assert_eq!(Seam::of(A, &[A], true, |_| true), Seam::default());
    }
}
