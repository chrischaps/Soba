//! The Step Sequencer's grid: a button per step in rows of eight, with its
//! note underneath.
//!
//! - Click switches a step's gate, Shift+click its tie, Ctrl+click its slide.
//! - Drag a step up or down to move its note a semitone at a time, or with
//!   Shift an octave at a time. The note it will land on shows above it,
//!   and sounds, on the sequencer's own outputs like the piano's below.
//! - Right-click opens a two-octave piano under the step, for writing notes
//!   the way a hardware sequencer's step-record does: each key played writes
//!   the step and moves along to the next one, so a melody goes in key after
//!   key. The piano stays put, and the step it will write next is outlined
//!   in the pitch cable's orange. A MIDI keyboard, or the letter keys, step-
//!   record too, writing each note's touch and its legato (see [`super::step_ear`]).
//!   The note's heard as it's written, on the sequencer's own outputs: a key
//!   for as long as it's held, a click for a Gate Length. A MIDI note is
//!   left to the patch's MIDI modules if it has any, so it doesn't sound twice.
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
use crate::dsp::Audition;
use crate::modules::sequencer::{note_to_name, PatternPosition, StepField, StepSequencer as Seq, MAX_STEPS, PATTERNS, PATTERN_NAMES};
use crate::modules::trigger_sequencer::{chain_from, CHAIN_NAMES};
use crate::widgets::{piano_keys, PianoConfig, PianoData};

use super::chord_ear::take_letter_keys;
use super::sequencer_common::{pattern_menu, popover_base, step_along, MenuHints, GATE_OFF, GATE_ON, OCTAVE_WIDTH, TAB_INSET, TOP_BASE};
use super::step_ear::{StepEar, Strike};
use super::trigger_display::{pattern_tabs, shown_pattern, PatternBar, Playing};
use super::{SynthGraph, SynthGraphState, SynthNodeData, SynthResponse};

/// Points of upward drag per semitone, before zoom: an octave is 72.
const DRAG_PER_SEMITONE: f32 = 6.0;
/// Points of drag per octave with Shift held, before zoom.
const DRAG_PER_OCTAVE: f32 = 18.0;
const KEY_HEIGHT: f32 = 48.0;
/// The gap between steps, before zoom.
const STEP_SPACING: f32 = 3.0;
const HINT: &str = "Click: gate · Shift+click: tie · Ctrl+click: slide\nDrag up or down: note (Shift: by octave)\nRight-click: piano";

/// The grid's width, before zoom.
const GRID_WIDTH: f32 = 220.0;
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
    pub velocities: [u8; 16],
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
    let engine_id = user_state.get_engine_node_id(node_id);
    let position = engine_id.and_then(|id| user_state.readouts.get(&id)).map(PatternPosition::from_readout);
    let editing = shown_pattern(ui.ctx(), node_id, position.filter(|p| p.started).map(|p| p.pattern));

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
        velocities: std::array::from_fn(|step| values.field(editing, step, StepField::Velocity).map_or(100, |v| v as u8)),
        ties: std::array::from_fn(|step| values.field(editing, step, StepField::Tie).is_some_and(|v| v > 0.5)),
        slides: std::array::from_fn(|step| values.field(editing, step, StepField::Slide).is_some_and(|v| v > 0.5)),
    };
    let plays_midi = super::note_in_face::plays_midi(graph);
    step_grid(ui, zoom, &pattern, plays_midi, node_id, user_state, responses);

    ui.add_space(6.0 * zoom);
    let bar = PatternBar {
        inset: TAB_INSET * zoom,
        width: GRID_WIDTH * zoom,
        chain: std::array::from_fn(|slot| values.get(CHAIN_NAMES[slot]).map_or(0, |v| (v.round().max(0.0) as usize).min(PATTERNS))),
        chain_names: std::array::from_fn(|slot| CHAIN_NAMES[slot].to_string()),
        playing: position.filter(|p| p.started).map(|p| Playing { pattern: p.pattern, chain_slot: p.chain_slot, next: p.next_pattern }),
        pattern_cv,
    };
    let edits = pattern_tabs(ui, node_id, zoom, &bar, |ui, pattern, edits| pattern_tab_menu(ui, node_id, &values, pattern, edits));
    responses.extend(edits.into_iter().map(NodeResponse::User));
}

/// A pattern tab's right-click menu: copy the pattern to another, or clear
/// it to rests, ready to write a new line over with the piano.
fn pattern_tab_menu(ui: &mut egui::Ui, node_id: NodeId, values: &Values, pattern: usize, edits: &mut Vec<SynthResponse>) {
    let hints = MenuHints { copy: "Every step's note, gate, velocity, tie and slide, to start a variation from", clear: "Every step a rest, keeping its note" };
    let copy_to = |to| {
        (0..MAX_STEPS)
            .flat_map(|step| StepField::ALL.map(|field| (step, field)))
            .filter_map(|(step, field)| values.field(pattern, step, field).map(|value| (Seq::step_param_name(to, step, field).to_string(), value)))
            .collect()
    };
    let clear = || {
        (0..MAX_STEPS)
            .flat_map(|step| [StepField::Gate, StepField::Tie, StepField::Slide].map(|field| (Seq::step_param_name(pattern, step, field).to_string(), 0.0)))
            .collect()
    };
    pattern_menu(ui, node_id, pattern, &hints, copy_to, clear, edits);
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
    /// The step the last note played in wrote, which the next ties from if
    /// it's struck legato.
    written: Option<usize>,
}

/// What the open piano hears, and the note it has sounding.
#[derive(Clone, Debug)]
struct Listening {
    ear: StepEar,
    /// Each note's touch as it was last written, for a letter key's
    /// audition: it sounds as hard as the step it went into will.
    touch: [u8; 128],
    /// The note last sent to sound, while the hand's down.
    sounding: Option<Audition>,
}

impl Listening {
    fn new(seen: u64) -> Self {
        Self { ear: StepEar::new(seen), touch: [100; 128], sounding: None }
    }
}

/// A note sounded from the piano, `touch` as a step's Velocity (0 to 127).
fn audition_of(note: u8, touch: u8) -> Audition {
    let mut audition = Audition { len: 1, root: note, bass: note, velocity: touch.min(127) as f32 / 127.0, ..Audition::default() };
    audition.notes[0] = note;
    audition
}

/// What to send the engine to sound `held` (the note under the hand now)
/// after `sounding` (the one sent last). `struck` is the last note struck
/// this frame that the sequencer sounds, if any, and whether it was played
/// legato: a note under the same hand moves Pitch without striking, as it's
/// written tied, while one struck afresh is let go of and struck again. A
/// note struck and lifted within the frame still sounds, as a click does.
fn auditions(sounding: Option<Audition>, held: Option<Audition>, struck: Option<(Audition, bool)>) -> Vec<Option<Audition>> {
    let mut sent = Vec::new();
    let afresh = struck.is_some_and(|(_, legato)| !legato);
    if afresh && sounding.is_some() {
        sent.push(None);
    }
    match struck {
        Some((note, _)) if held.is_none() => sent.extend([Some(note), None]),
        _ if afresh || held != sounding => sent.push(held),
        _ => {}
    }
    sent
}

/// A drag under way: the step, its note when the drag began, and the note
/// it has sounding.
#[derive(Clone, Copy, Debug)]
struct StepDrag {
    step: usize,
    from: u8,
    sounding: Option<Audition>,
}

impl StepDrag {
    /// What to send the engine as the drag holds `held`, or lets go with
    /// None. The note it begins on is struck, each one it moves to after
    /// moves Pitch under the held gate, legato, the way a key held under the
    /// hand does, and letting go lets the gate fall.
    fn sound(&mut self, held: Option<Audition>) -> Vec<Option<Audition>> {
        auditions(std::mem::replace(&mut self.sounding, held), held, None)
    }
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

/// The piano moved by `semitones` (a multiple of 12), kept on the keyboard.
fn shift_base(base: u8, semitones: i32) -> u8 {
    (base as i32 + semitones).clamp(0, TOP_BASE as i32) as u8
}

/// What a note played in writes to the step the piano's on, as one undo
/// step: the note, a rest switched on, and its touch if it has one. Its tie
/// waits on the next note: struck legato, that one ties this one into it, if
/// it's the step just before. Then the piano moves along.
fn write_strike(strike: Strike, open: &mut StepEntry, pattern: &StepPattern) -> (String, Vec<(String, f32)>) {
    let step = open.step;
    let mut changes = vec![
        (pattern.param(step, StepField::Pitch), strike.note as f32),
        (pattern.param(step, StepField::Gate), 1.0),
        (pattern.param(step, StepField::Tie), 0.0),
    ];
    if let Some(velocity) = strike.velocity {
        changes.push((pattern.param(step, StepField::Velocity), velocity as f32));
    }
    let before = step_along(step, -1, pattern.steps);
    if open.written.is_some_and(|written| written == before && written != step) {
        changes.push((pattern.param(before, StepField::Tie), if strike.legato { 1.0 } else { 0.0 }));
    }
    open.written = Some(step);
    open.step = step_along(step, 1, pattern.steps);
    (format!("Play {} into step {}", note_to_name(strike.note), step + 1), changes)
}

pub(super) fn step_grid(
    ui: &mut egui::Ui,
    zoom: f32,
    pattern: &StepPattern,
    plays_midi: bool,
    node_id: NodeId,
    user_state: &SynthGraphState,
    responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
) {
    let step_size = 24.0 * zoom;
    // Notes played in, each one undo step, sent once the grid's drawn, and
    // the notes to sound as they're written
    let mut played = Vec::new();
    let mut sounded = Vec::new();
    let step_spacing = STEP_SPACING * zoom;
    let mut set = |param_name: String, value: f32| {
        responses.push(NodeResponse::User(SynthResponse::ParameterChanged { node_id, param_name, value }));
    };

    let entry_id = Id::new(("step_entry", node_id));
    let ear_id = Id::new(("step_ear", node_id));
    let drag_id = Id::new(("step_drag", node_id));
    let mut entry = egui::Popup::is_id_open(ui.ctx(), entry_id)
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
                    let pad = egui::Rect::from_min_size(rect.min, egui::vec2(step_size, step_size));
                    super::marks::mark(ui.ctx(), node_id, format_args!("step:{}", cells.len() + 1), pad);
                    cells.push((pad, response.on_hover_text(HINT)));
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
            painter.rect_filled(*step_rect, 3.0 * zoom, color);
            if is_current {
                painter.rect_stroke(*step_rect, 3.0 * zoom, egui::Stroke::new(2.0 * zoom, Color32::WHITE), egui::StrokeKind::Middle);
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
                entry = Some(StepEntry { step, base: popover_base(pitch as i32), anchor: step, written: None });
                egui::Popup::open_id(ui.ctx(), entry_id);
            }

            // Dragging up or down moves the note, sounding it as it goes, at
            // the step's Velocity. With the clock running the sequence takes
            // over again at its next note once it's let go of
            if response.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, StepDrag { step, from: pitch, sounding: None }));
            }
            let drag = ui.data(|d| d.get_temp::<StepDrag>(drag_id)).filter(|d| d.step == step);
            if let Some(mut drag) = drag.filter(|_| response.dragged()) {
                let (origin, now, octaves) =
                    ui.input(|i| (i.pointer.press_origin(), i.pointer.interact_pos(), i.modifiers.shift));
                if let (Some(origin), Some(now)) = (origin, now) {
                    let to = dragged_pitch(drag.from, now.y - origin.y, zoom, octaves);
                    if to != pitch {
                        set(pattern.param(step, StepField::Pitch), to as f32);
                    }
                    sounded.extend(drag.sound(Some(audition_of(to, pattern.velocities[step]))));
                    ui.data_mut(|d| d.insert_temp(drag_id, drag));
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
                    drag_badge(ui, node_id, *step_rect, &note_to_name(to), zoom);
                }
            }
            if response.drag_stopped() {
                if let Some(mut drag) = ui.data(|d| d.get_temp::<StepDrag>(drag_id)) {
                    sounded.extend(drag.sound(None));
                }
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
        painter.rect_stroke(cells[open.step].0.expand(2.0 * zoom), 4.0, egui::Stroke::new(2.0_f32, theme::signal::CONTROL), egui::StrokeKind::Middle);

        let (back, on, escape) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            egui::Popup::close_id(ui.ctx(), entry_id);
        }
        open.step = step_along(open.step, on as isize - back as isize, pattern.steps);

        // Notes played on the keys write their steps and move along. A note
        // held as the piano opened is the hand's, not one played in
        let count = user_state.midi_note_count();
        let mut listening = ui.data(|d| d.get_temp::<Listening>(ear_id)).unwrap_or_else(|| Listening::new(count));
        let midi = user_state.midi_notes_since(listening.ear.seen());
        // Heard by the sequencer, unless the patch plays MIDI itself
        let midi_too = !plays_midi;
        let mut struck = None;
        for strike in ui.input(|i| listening.ear.hear(&i.raw.events, open.base, &midi, count)) {
            let touch = strike.velocity.unwrap_or(pattern.velocities[open.step]);
            listening.touch[strike.note as usize] = touch;
            if strike.velocity.is_none() || midi_too {
                struck = Some((audition_of(strike.note, touch), strike.legato));
            }
            let (label, changes) = write_strike(strike, &mut open, pattern);
            played.push(SynthResponse::EditParameters { node_id, label, changes });
        }
        let held = listening.ear.sounding(midi_too).map(|(note, felt)| audition_of(note, felt.unwrap_or(listening.touch[note as usize])));
        sounded.extend(auditions(listening.sounding, held, struck));
        listening.sounding = held;

        let anchor = &cells[open.anchor].1;
        egui::Popup::from_response(anchor).id(entry_id).open_memory(None).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            if let Some((note, step)) = step_piano(ui, &mut open, pattern, &mut set) {
                // Written with a click, which has no legato to tie from,
                // and sounds for a Gate Length, over the hand if it's down
                open.written = None;
                if listening.sounding.take().is_some() {
                    sounded.push(None);
                }
                sounded.extend([Some(audition_of(note, pattern.velocities[step])), None]);
            }
        });
        if egui::Popup::is_id_open(ui.ctx(), entry_id) {
            ui.data_mut(|d| {
                d.insert_temp(entry_id, open);
                d.insert_temp(ear_id, listening);
            });
            take_letter_keys(ui.ctx());
            // MIDI arrives between frames: keep looking while the piano listens
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        } else {
            ui.data_mut(|d| {
                d.remove::<StepEntry>(entry_id);
                d.remove::<Listening>(ear_id);
            });
            // Closed with the hand still down: the note stops with it
            if listening.sounding.is_some() {
                sounded.push(None);
            }
        }
    });
    responses.extend(sounded.into_iter().map(|chord| NodeResponse::User(SynthResponse::Audition { node_id, chord })));
    responses.extend(played.into_iter().map(NodeResponse::User));
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
/// Gives back the note a click wrote, and its step.
fn step_piano(ui: &mut egui::Ui, open: &mut StepEntry, pattern: &StepPattern, set: &mut impl FnMut(String, f32)) -> Option<(u8, usize)> {
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
    let wrote = played.map(|note| (note, step));

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
    ui.label(RichText::new("Each key writes this step, then moves to the next").small().weak()).on_hover_text(
        "So does a note on a MIDI keyboard, with its touch, or on the letter keys (Z is the lowest C). One held into the next ties into it. \
         Each sounds on the sequencer's outputs as it's written",
    );
    wrote
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
    fn the_piano_stays_on_the_keyboard() {
        assert_eq!(shift_base(48, 12), 60);
        assert_eq!(shift_base(0, -12), 0);
        assert_eq!(shift_base(TOP_BASE, 12), TOP_BASE);
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

    /// Pattern A, `steps` long, every step a C4 with its gate on.
    fn pattern_of(steps: usize) -> StepPattern {
        StepPattern {
            pattern: A,
            steps,
            current: None,
            pitches: [60; 16],
            gates: [true; 16],
            velocities: [100; 16],
            ties: [false; 16],
            slides: [false; 16],
            seam: Seam::default(),
        }
    }

    fn opened_on(step: usize) -> StepEntry {
        StepEntry { step, base: 48, anchor: step, written: None }
    }

    /// The edits notes played into the piano make, MIDI notes struck and let
    /// go between two frames.
    fn play(open: &mut StepEntry, pattern: &StepPattern, midi: &[(u8, u8)]) -> Vec<(String, Vec<(String, f32)>)> {
        let mut ear = StepEar::new(0);
        ear.hear(&[], open.base, midi, midi.len() as u64).into_iter().map(|strike| write_strike(strike, open, pattern)).collect()
    }

    fn change(edit: &(String, Vec<(String, f32)>), param: &str) -> Option<f32> {
        edit.1.iter().find(|(name, _)| name == param).map(|&(_, value)| value)
    }

    #[test]
    fn notes_played_in_write_their_touch_and_move_along() {
        let pattern = pattern_of(8);
        let mut open = opened_on(0);
        // C4 soft, let go, then E4 hard
        let edits = play(&mut open, &pattern, &[(60, 40), (60, 0), (64, 110)]);
        assert_eq!(edits.len(), 2, "one undo step a note");
        assert_eq!(edits[0].0, "Play C4 into step 1");
        assert_eq!(change(&edits[0], "Step 1 Pitch"), Some(60.0));
        assert_eq!(change(&edits[0], "Step 1 Velocity"), Some(40.0));
        assert_eq!(edits[1].0, "Play E4 into step 2");
        assert_eq!(change(&edits[1], "Step 2 Pitch"), Some(64.0));
        assert_eq!(change(&edits[1], "Step 2 Velocity"), Some(110.0));
        // Played detached, so step 1 doesn't tie into step 2
        assert_eq!(change(&edits[1], "Step 1 Tie"), Some(0.0));
        assert_eq!(open.step, 2, "the next-step outline is on step 3");
    }

    #[test]
    fn a_note_held_into_the_next_ties_into_it() {
        let pattern = pattern_of(8);
        let mut open = opened_on(4);
        let edits = play(&mut open, &pattern, &[(60, 90), (62, 90), (60, 0)]);
        assert_eq!(change(&edits[1], "Step 5 Tie"), Some(1.0));
        assert_eq!(change(&edits[1], "Step 6 Tie"), Some(0.0), "its own tie waits on the note after");
    }

    #[test]
    fn a_rest_played_into_sounds() {
        let mut pattern = pattern_of(8);
        pattern.gates[0] = false;
        let edits = play(&mut opened_on(0), &pattern, &[(67, 100)]);
        assert_eq!(change(&edits[0], "Step 1 Gate"), Some(1.0));
    }

    #[test]
    fn the_last_step_ties_round_into_the_first() {
        let pattern = pattern_of(4);
        let mut open = opened_on(3);
        let edits = play(&mut open, &pattern, &[(60, 90), (64, 90)]);
        assert_eq!(edits[1].0, "Play E4 into step 1");
        assert_eq!(change(&edits[1], "Step 4 Tie"), Some(1.0));
        assert_eq!(open.step, 1);
    }

    #[test]
    fn only_the_step_just_written_is_tied_from() {
        let pattern = pattern_of(8);
        // A step written with a click, or skipped past with →, isn't tied from
        let mut open = StepEntry { written: Some(1), ..opened_on(3) };
        let edits = play(&mut open, &pattern, &[(60, 90)]);
        assert_eq!(edits[0].1.iter().filter(|(name, _)| name.ends_with("Tie")).count(), 1, "only its own tie: {:?}", edits[0].1);
        // A pattern of one step doesn't tie into itself
        let mut open = opened_on(0);
        let edits = play(&mut open, &pattern_of(1), &[(60, 90), (64, 90)]);
        assert_eq!(edits[1].1.iter().filter(|(name, _)| name == "Step 1 Tie").count(), 1);
    }

    #[test]
    fn the_letter_keys_leave_the_touch_as_it_was() {
        let pattern = pattern_of(8);
        let mut open = opened_on(0);
        let z = egui::Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let mut ear = StepEar::new(0);
        let strikes = ear.hear(&[z], open.base, &[], 0);
        let edit = write_strike(strikes[0], &mut open, &pattern);
        assert_eq!(edit.0, "Play C3 into step 1");
        assert_eq!(change(&edit, "Step 1 Velocity"), None);
    }

    #[test]
    fn a_key_struck_sounds_and_lifting_it_stops() {
        let c = audition_of(60, 100);
        assert_eq!(auditions(None, Some(c), Some((c, false))), vec![Some(c)]);
        // Held: nothing more to send
        assert_eq!(auditions(Some(c), Some(c), None), vec![]);
        assert_eq!(auditions(Some(c), None, None), vec![None]);
    }

    #[test]
    fn a_key_struck_legato_moves_the_note_and_one_struck_afresh_strikes_again() {
        let (c, d) = (audition_of(60, 100), audition_of(62, 100));
        assert_eq!(auditions(Some(c), Some(d), Some((d, true))), vec![Some(d)]);
        // C lifted and D struck within the frame
        assert_eq!(auditions(Some(c), Some(d), Some((d, false))), vec![None, Some(d)]);
        // C struck again as fast as it was lifted
        assert_eq!(auditions(Some(c), Some(c), Some((c, false))), vec![None, Some(c)]);
    }

    #[test]
    fn a_key_tapped_within_a_frame_still_blips() {
        let e = audition_of(64, 100);
        assert_eq!(auditions(None, None, Some((e, false))), vec![Some(e), None]);
    }

    #[test]
    fn a_drag_strikes_its_note_then_moves_it_legato() {
        let mut drag = StepDrag { step: 0, from: 60, sounding: None };
        let [c, cs, d] = [60, 61, 62].map(|note| audition_of(note, 100));
        assert_eq!(drag.sound(Some(c)), vec![Some(c)], "the drag's start strikes once");
        assert_eq!(drag.sound(Some(c)), vec![], "and holds while it stays on the note");
        assert_eq!(drag.sound(Some(cs)), vec![Some(cs)], "each semitone moves Pitch without letting go");
        assert_eq!(drag.sound(Some(d)), vec![Some(d)]);
        assert_eq!(drag.sound(None), vec![None], "letting go lets the gate fall");
        assert_eq!(drag.sound(None), vec![], "once");
    }

    #[test]
    fn an_audition_sounds_as_hard_as_its_step() {
        assert_eq!(audition_of(60, 127).velocity, 1.0);
        assert_eq!(audition_of(60, 0).velocity, 0.0);
        assert_eq!(audition_of(67, 100).notes(), &[67]);
    }
}
