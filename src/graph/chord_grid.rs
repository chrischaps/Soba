//! The Chord Sequencer's face: the voicing on a keyboard, then its steps,
//! each named by its chord.
//!
//! - The keyboard strip shows the chord on the cables. Each voice is a dot
//!   over its key that glides to its next note, so voice leading can be
//!   watched: most dots creep a step or stay put while the chord changes.
//! - A step shows its chord's name ("Am9", with "/G#" under it for a slash
//!   chord), its velocity as a line along its foot, and a notch in its head
//!   when it names the tone on top.
//! - Click switches a step on or off, Shift+click its tie; drag up or down
//!   moves its root by semitones (Shift: octaves), as on the Step Sequencer.
//! - Right-click opens the chord under the step: a piano for the root, rows
//!   for the slash bass and the tone on top, and the chord types. Choosing a type writes the step
//!   and moves to the next, so a progression goes in as a lead sheet reads:
//!   root, type, root, type.
//! - Or play the chord, on a MIDI keyboard or the letter keys, while the
//!   popover is open. When the hand lifts, the chord is named (see
//!   [`recognise`]) and written, and the popover moves to the next step, the
//!   way the Step Sequencer's piano step-records. The keys light on its
//!   piano while they're held, and the chord's name shows as it's played.

use std::sync::OnceLock;

use eframe::egui::{self, Color32, CursorIcon, Id, Key, Modifiers, PopupCloseBehavior, Rect, RichText, Sense, Stroke};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::DspModule;
use crate::modules::chord_sequencer::recognition::{pitch_classes, recognise, Heard};
use crate::modules::chord_sequencer::{kept_tones, voice_chord, ChordSequencer, ChordSpec, VoicingSettings, Voicing, CHORD_TYPES, MAX_STEPS};
use crate::modules::sequencer::note_to_name;
use crate::widgets::{piano_keys, PianoConfig, PianoData};

use super::chord_ear::{take_letter_keys, ChordEar};
use super::step_grid::{drag_badge, dragged_pitch};
use super::{SynthGraph, SynthGraphState, SynthResponse, SynthValueType};

const GATE_ON: Color32 = Color32::from_rgb(100, 200, 100);
const GATE_OFF: Color32 = Color32::from_rgb(60, 60, 70);
const ON_TEXT: Color32 = Color32::from_rgb(18, 40, 20);
const OFF_TEXT: Color32 = Color32::from_rgb(150, 150, 165);
/// How many octaves the keyboard strip spans: two under the Range octave,
/// where a bass sits, the Range octave and the one over it.
const STRIP_OCTAVES: i32 = 4;
/// The highest C the popover piano's lower octave can start on.
const TOP_BASE: u8 = 96;
const OCTAVE_WIDTH: f32 = 140.0;
const KEY_HEIGHT: f32 = 44.0;
/// The order the popover lists the chord types in: as the Type dropdown,
/// but with each one beside its kin, where a later type joined the list.
const TYPE_ORDER: [usize; 19] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 18, 13, 14, 15, 16, 17];
const HINT: &str = "Click: chord on/off · Shift+click: tie\nDrag up or down: root (Shift: by octave)\nRight-click: edit the chord";

/// A Chord Sequencer's pattern and settings, as its face shows them.
struct ChordPattern {
    steps: usize,
    specs: [ChordSpec; MAX_STEPS],
    gates: [bool; MAX_STEPS],
    ties: [bool; MAX_STEPS],
    velocities: [f32; MAX_STEPS],
    settings: VoicingSettings,
    channels: usize,
    bass_voice: bool,
}

impl ChordPattern {
    /// Reads the pattern from the node's parameters.
    fn read(graph: &SynthGraph, node_id: NodeId) -> Self {
        // Each parameter's name and default, in the module's order
        static DEFINITIONS: OnceLock<Vec<(&'static str, f32)>> = OnceLock::new();
        let definitions = DEFINITIONS.get_or_init(|| ChordSequencer::new().parameters().iter().map(|p| (p.name, p.default)).collect());
        let mut params: Vec<f32> = definitions.iter().map(|&(_, default)| default).collect();
        if let Some(node) = graph.nodes.get(node_id) {
            for (name, input_id) in &node.inputs {
                let Some(index) = definitions.iter().position(|&(n, _)| n == name) else { continue };
                params[index] = match &graph.get_input(*input_id).value {
                    SynthValueType::Number { value, .. } => *value,
                    SynthValueType::Toggle { value, .. } => *value as u8 as f32,
                    SynthValueType::Select { value, .. } => *value as f32,
                    SynthValueType::Port => continue,
                };
            }
        }
        let steps = (params[ChordSequencer::PARAM_STEPS].round() as usize).clamp(1, MAX_STEPS);
        let channels = (params[ChordSequencer::PARAM_VOICES].round() as usize).clamp(1, 8);
        let bass_voice = params[ChordSequencer::PARAM_BASS_VOICE] > 0.5;
        let settings = VoicingSettings {
            voicing: Voicing::from_param(params[ChordSequencer::PARAM_VOICING]),
            voices: channels.saturating_sub(bass_voice as usize).max(1),
            range: params[ChordSequencer::PARAM_RANGE].round() as i32,
            rootless: bass_voice || params[ChordSequencer::PARAM_ROOTLESS] > 0.5,
        };
        Self {
            steps,
            specs: std::array::from_fn(|s| ChordSequencer::spec(&params, s)),
            gates: std::array::from_fn(|s| params[ChordSequencer::step_gate_param(s)] > 0.5),
            ties: std::array::from_fn(|s| params[ChordSequencer::step_tie_param(s)] > 0.5),
            velocities: std::array::from_fn(|s| params[ChordSequencer::step_velocity_param(s)] / 127.0),
            settings,
            channels,
            bass_voice,
        }
    }

    /// A step's chord as it would sound on its own, in root position: what
    /// the keyboard shows before the engine has played anything.
    fn preview(&self, step: usize) -> Vec<i32> {
        let spec = &self.specs[step];
        let mut notes = Vec::with_capacity(self.channels);
        if self.bass_voice {
            notes.push(spec.bass_note());
        }
        if !self.bass_voice || self.channels > 1 {
            notes.extend(voice_chord(spec, &self.settings, None).notes());
        }
        notes
    }
}

/// The popover's place in the pattern while it's open, as on the Step
/// Sequencer: the step it writes, the C its piano starts on, and the step it
/// opened under, which it stays beneath.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ChordEntry {
    step: usize,
    base: u8,
    anchor: usize,
}

/// The popover's ear, and the chord it last wrote.
#[derive(Clone, Debug, Default)]
struct Listening {
    ear: ChordEar,
    last: Option<Played>,
}

/// A chord played into a step: the notes, and what they were heard as.
#[derive(Clone, Debug)]
struct Played {
    notes: Vec<u8>,
    heard: Heard,
    step: usize,
}

impl Played {
    /// Names the notes played into `step` (lowest first). A note alone, or
    /// in octaves, is a root: the step keeps its type, without a slash bass.
    fn new(notes: Vec<u8>, step: usize, pattern: &ChordPattern) -> Option<Self> {
        let lowest = *notes.first()? as i32;
        let heard = if notes.iter().all(|&n| (n as i32 - lowest) % 12 == 0) {
            Heard { spec: ChordSpec { root: lowest, kind: pattern.specs[step].kind, bass: None, top: None }, extra: 0, missing: 0 }
        } else {
            recognise(&notes.iter().map(|&n| n as i32).collect::<Vec<_>>())?
        };
        Some(Self { notes, heard, step })
    }

    /// The step's parameters, as written: root, type, bass and gate.
    fn changes(&self) -> Vec<(String, f32)> {
        let n = self.step + 1;
        let spec = &self.heard.spec;
        vec![
            (format!("Step {n} Root"), spec.root.clamp(0, 127) as f32),
            (format!("Step {n} Type"), spec.kind as f32),
            (format!("Step {n} Bass"), spec.bass.map_or(0.0, |pc| pc as f32 + 1.0)),
            (format!("Step {n} Gate"), 1.0),
        ]
    }
}

/// Pitch classes as the chord spells them: G# under Am, not Ab.
fn spelled(spec: &ChordSpec, pitch_classes: impl IntoIterator<Item = i32>) -> String {
    pitch_classes.into_iter().map(|pc| spec.spell(pc)).collect::<Vec<_>>().join(" ")
}

/// What a chord heard didn't fit, as "no G · D left out", or `None` when
/// it fit.
fn misfit(heard: &Heard) -> Option<String> {
    let spec = &heard.spec;
    let mut parts = Vec::new();
    if heard.missing != 0 {
        parts.push(format!("no {}", spelled(spec, pitch_classes(heard.missing))));
    }
    if heard.extra != 0 {
        parts.push(format!("{} left out", spelled(spec, pitch_classes(heard.extra))));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A drag under way: the step, and its root when the drag began.
#[derive(Clone, Copy, Debug)]
struct RootDrag {
    step: usize,
    from: u8,
}

fn step_along(step: usize, by: isize, steps: usize) -> usize {
    (step as isize + by).rem_euclid(steps.max(1) as isize) as usize
}

fn popover_base(root: i32) -> u8 {
    ((root / 12 * 12 - 12).clamp(0, TOP_BASE as i32)) as u8
}

/// Draws the face and returns the parameter edits made on it.
pub(super) fn chord_grid(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let pattern = ChordPattern::read(graph, node_id);
    let mut edits = Vec::new();
    let mut set = |param_name: String, value: f32| edits.push(SynthResponse::ParameterChanged { node_id, param_name, value });

    // What the engine is playing, if it has said
    let engine = user_state.get_engine_node_id(node_id);
    let readout = engine.and_then(|id| user_state.readouts.get(&id)).copied();
    let current = readout.map_or(0, |r| (r.values[ChordSequencer::READOUT_STEP] as usize).min(pattern.steps - 1));
    let gate_high = engine.and_then(|id| user_state.get_output_value(id, ChordSequencer::PORT_GATE)).is_some_and(|g| g > 0.5);
    let sounding: Vec<i32> = match readout {
        Some(r) if r.values[ChordSequencer::READOUT_NOTES] >= 0.0 => r.values[ChordSequencer::READOUT_NOTES..]
            .iter()
            .take_while(|&&n| n >= 0.0)
            .map(|&n| n as i32)
            .collect(),
        _ => pattern.preview(current),
    };

    let entry_id = Id::new(("chord_entry", node_id));
    let ear_id = Id::new(("chord_ear", node_id));
    let drag_id = Id::new(("chord_drag", node_id));
    let mut played = None;
    let mut entry = ui
        .memory(|m| m.is_popup_open(entry_id))
        .then(|| ui.data(|d| d.get_temp::<ChordEntry>(entry_id)))
        .flatten()
        .map(|e| ChordEntry { step: e.step.min(pattern.steps - 1), anchor: e.anchor.min(pattern.steps - 1), ..e });

    let cell = egui::vec2(36.0 * z, 30.0 * z);
    let gap = 3.0 * z;
    let width = 8.0 * cell.x + 7.0 * gap;

    ui.vertical(|ui| {
        ui.set_min_width(width);
        let top = ui.cursor().top();
        ui.painter().hline(ui.cursor().left()..=ui.cursor().left() + width, top, Stroke::new(1.0 * z, theme::module::UTILITY.gamma_multiply(0.25)));
        ui.add_space(6.0 * z);
        let low = 12 * (pattern.settings.range - 1);
        keyboard_strip(ui, node_id, width, low, &sounding, pattern.bass_voice, gate_high || readout.is_none(), z);
        ui.add_space(5.0 * z);

        let mut cells = Vec::with_capacity(pattern.steps);
        for row_start in (0..pattern.steps).step_by(8) {
            if row_start > 0 {
                ui.add_space(gap);
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for _ in row_start..(row_start + 8).min(pattern.steps) {
                    let (rect, response) = ui.allocate_exact_size(cell, Sense::click_and_drag());
                    cells.push((rect, response.on_hover_text(HINT)));
                }
            });
        }

        let painter = ui.painter();
        // Ties, under the steps: a band across the gap into the next chord
        let band = cell.y * 0.18;
        for step in (0..pattern.steps).filter(|&s| pattern.ties[s] && pattern.gates[s]) {
            let from = cells[step].0;
            let to = cells[(step + 1) % pattern.steps].0;
            let same_row = step + 1 < pattern.steps && (to.center().y - from.center().y).abs() < 1.0;
            let bar = |left: f32, right: f32, y: f32| Rect::from_min_max(egui::pos2(left, y - band), egui::pos2(right, y + band));
            if same_row {
                painter.rect_filled(bar(from.center().x, to.center().x, from.center().y), 0.0, GATE_ON);
            } else {
                painter.rect_filled(bar(from.center().x, from.right() + gap * 2.0, from.center().y), band, GATE_ON);
                painter.rect_filled(bar(to.left() - gap * 2.0, to.center().x, to.center().y), band, GATE_ON);
            }
        }

        for (step, (rect, response)) in cells.iter().enumerate() {
            let spec = &pattern.specs[step];
            let on = pattern.gates[step];
            let playing = step == current && readout.is_some();
            let fill = match (on, playing) {
                (true, true) => Color32::from_rgb(170, 240, 160),
                (true, false) => GATE_ON,
                (false, true) => Color32::from_rgb(90, 90, 104),
                (false, false) => GATE_OFF,
            };
            painter.rect_filled(*rect, 4.0 * z, fill);
            if playing {
                painter.rect_stroke(*rect, 4.0 * z, Stroke::new(2.0, Color32::WHITE));
            }
            // Velocity, along the foot
            if on {
                let foot = Rect::from_min_size(egui::pos2(rect.left() + 3.0 * z, rect.bottom() - 3.5 * z), egui::vec2((rect.width() - 6.0 * z) * pattern.velocities[step], 1.5 * z));
                painter.rect_filled(foot, 0.0, Color32::from_rgba_unmultiplied(18, 40, 20, 110));
            }
            // A tone named for the top: a notch in the head
            if spec.top.is_some() {
                let notch = Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 2.5 * z), egui::vec2(8.0 * z, 1.5 * z));
                painter.rect_filled(notch, 0.0, if on { Color32::from_rgba_unmultiplied(18, 40, 20, 150) } else { OFF_TEXT });
            }
            chord_label(painter, *rect, spec, if on { ON_TEXT } else { OFF_TEXT }, z);

            if response.clicked() {
                if ui.input(|i| i.modifiers.shift) {
                    set(format!("Step {} Tie", step + 1), if pattern.ties[step] { 0.0 } else { 1.0 });
                } else {
                    set(format!("Step {} Gate", step + 1), if on { 0.0 } else { 1.0 });
                }
            }
            if response.secondary_clicked() {
                entry = Some(ChordEntry { step, base: popover_base(spec.root), anchor: step });
                ui.memory_mut(|m| m.open_popup(entry_id));
            }

            // Dragging up or down moves the root
            let root = spec.root as u8;
            if response.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, RootDrag { step, from: root }));
            }
            let drag = ui.data(|d| d.get_temp::<RootDrag>(drag_id)).filter(|d| d.step == step);
            if let Some(drag) = drag.filter(|_| response.dragged()) {
                let (origin, now, octaves) = ui.input(|i| (i.pointer.press_origin(), i.pointer.interact_pos(), i.modifiers.shift));
                if let (Some(origin), Some(now)) = (origin, now) {
                    let to = dragged_pitch(drag.from, now.y - origin.y, z, octaves);
                    if to != root {
                        set(format!("Step {} Root", step + 1), to as f32);
                    }
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
                    let name = ChordSpec { root: to as i32, ..*spec }.name();
                    drag_badge(ui, node_id, *rect, &format!("{name}  {}", note_to_name(to)), z);
                }
            }
            if response.drag_stopped() {
                ui.data_mut(|d| d.remove::<RootDrag>(drag_id));
            }
        }

        let Some(mut open) = entry else { return };
        painter.rect_stroke(cells[open.step].0.expand(2.0 * z), 5.0 * z, Stroke::new(2.0, theme::signal::CONTROL));

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

        // A chord played on the keys writes the step and moves along
        let mut listening = ui.data(|d| d.get_temp::<Listening>(ear_id)).unwrap_or_default();
        ui.input(|i| listening.ear.follow_keys(&i.raw.events, open.base));
        let midi = user_state.midi_active_notes();
        let held = listening.ear.held(midi);
        if let Some(chord) = listening.ear.listen(midi).and_then(|notes| Played::new(notes, open.step, &pattern)) {
            open.step = step_along(open.step, 1, pattern.steps);
            played = Some(chord.clone());
            listening.last = Some(chord);
        }

        let anchor = &cells[open.anchor].1;
        egui::popup_below_widget(ui, entry_id, anchor, PopupCloseBehavior::CloseOnClickOutside, |ui| {
            chord_popover(ui, &mut open, &pattern, &held, &listening, &mut set);
        });
        if ui.memory(|m| m.is_popup_open(entry_id)) {
            ui.data_mut(|d| {
                d.insert_temp(entry_id, open);
                d.insert_temp(ear_id, listening);
            });
            take_letter_keys(ui.ctx());
            // MIDI arrives between frames: keep looking while the popover listens
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        } else {
            ui.data_mut(|d| {
                d.remove::<ChordEntry>(entry_id);
                d.remove::<Listening>(ear_id);
            });
        }
    });
    // A chord played is one undo step, named for it
    if let Some(chord) = played {
        let label = format!("Play {} into step {}", chord.heard.spec.name(), chord.step + 1);
        edits.push(SynthResponse::EditParameters { node_id, label, changes: chord.changes() });
    }
    edits
}

/// A step's chord name inside its cell: the chord on top, a slash bass
/// under it, shrunk to fit.
fn chord_label(painter: &egui::Painter, rect: Rect, spec: &ChordSpec, color: Color32, z: f32) {
    let main = format!("{}{}", spec.root_name(), CHORD_TYPES[spec.kind].suffix);
    let fit = |text: String, size: f32| {
        let galley = painter.layout_no_wrap(text.clone(), egui::FontId::proportional(size * z), color);
        let room = rect.width() - 4.0 * z;
        if galley.size().x > room {
            painter.layout_no_wrap(text, egui::FontId::proportional(size * z * room / galley.size().x), color)
        } else {
            galley
        }
    };
    let top = fit(main, 11.0);
    match spec.bass_name() {
        Some(bass) => {
            let under = fit(format!("/{bass}"), 9.0);
            let height = top.size().y + under.size().y - 2.0 * z;
            let y = rect.center().y - height / 2.0 - 1.0 * z;
            painter.galley(egui::pos2(rect.center().x - top.size().x / 2.0, y), top.clone(), color);
            painter.galley(egui::pos2(rect.center().x - under.size().x / 2.0, y + top.size().y - 2.0 * z), under, color);
        }
        None => {
            let pos = rect.center() - top.size() / 2.0 - egui::vec2(0.0, 1.0 * z);
            painter.galley(pos, top, color);
        }
    }
}

/// Where a note's key sits along a strip starting on the C `low`, in white
/// keys: the middle of a white key or the line between two. `None` off the
/// strip.
fn key_position(note: i32, low: i32) -> Option<(f32, bool)> {
    const WHITE: [f32; 12] = [0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0];
    const BLACK: [bool; 12] = [false, true, false, true, false, false, true, false, true, false, true, false];
    if !(low..low + 12 * STRIP_OCTAVES).contains(&note) {
        return None;
    }
    let pc = (note - low).rem_euclid(12) as usize;
    let octave = (note - low) / 12;
    Some((octave as f32 * 7.0 + WHITE[pc] + 0.5, BLACK[pc]))
}

/// The keyboard strip: four octaves from the C `low`, the chord's keys lit,
/// and a dot for each voice gliding to the key it plays now.
#[allow(clippy::too_many_arguments)]
fn keyboard_strip(ui: &mut egui::Ui, node_id: NodeId, width: f32, low: i32, notes: &[i32], bass_voice: bool, lit: bool, z: f32) {
    let height = 28.0 * z;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    let key_w = rect.width() / (7 * STRIP_OCTAVES) as f32;
    let keys = Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 10.0 * z), rect.max);
    let accent = theme::signal::CONTROL;
    let bass_colour = Color32::from_rgb(255, 214, 140);
    let glow = |c: Color32, alpha: f32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha * if lit { 1.0 } else { 0.45 }) as u8);
    let is_bass = |i: usize| bass_voice && i == 0 && notes.len() > 1;
    let notes_on_strip = (low..low + 12 * STRIP_OCTAVES).filter_map(|note| key_position(note, low).map(|(x, black)| (note, x, black)));

    // White keys, the chord's lit
    painter.rect_filled(keys, 2.0 * z, Color32::from_rgb(205, 205, 212));
    for (note, x, _) in notes_on_strip.clone().filter(|&(_, _, black)| !black) {
        let key = Rect::from_min_max(egui::pos2(keys.left() + (x - 0.5) * key_w, keys.top()), egui::pos2(keys.left() + (x + 0.5) * key_w, keys.bottom()));
        if let Some(i) = notes.iter().position(|&n| n == note) {
            painter.rect_filled(key.shrink(0.5), 1.0, glow(if is_bass(i) { bass_colour } else { accent }, 255.0));
        }
        painter.vline(key.right(), keys.y_range(), Stroke::new(0.6, Color32::from_rgb(120, 120, 132)));
        if (note - low) % 12 == 0 && note > low {
            painter.vline(key.left(), keys.y_range(), Stroke::new(1.0, Color32::from_rgb(90, 90, 104)));
        }
    }
    // Black keys over them
    for (note, x, _) in notes_on_strip.filter(|&(_, _, black)| black) {
        let key = Rect::from_center_size(egui::pos2(keys.left() + x * key_w, keys.top() + keys.height() * 0.31), egui::vec2(key_w * 0.62, keys.height() * 0.62));
        let colour = match notes.iter().position(|&n| n == note) {
            Some(i) => glow(if is_bass(i) { bass_colour } else { accent }, 255.0),
            None => Color32::from_rgb(38, 38, 48),
        };
        painter.rect_filled(key, 1.0, colour);
    }

    // A dot per voice above the keys, gliding to its key
    for (i, &note) in notes.iter().enumerate() {
        let Some((x, _)) = key_position(note, low) else { continue };
        let target = keys.left() + x * key_w;
        let x = ui.ctx().animate_value_with_time(Id::new(("chord_voice", node_id, i)), target, 0.18);
        let colour = if is_bass(i) { bass_colour } else { accent };
        let centre = egui::pos2(x, rect.top() + 4.5 * z);
        painter.circle_filled(centre, 4.5 * z, glow(colour, 60.0));
        painter.circle_filled(centre, 2.6 * z, glow(colour, 255.0));
    }

    let names: Vec<String> = notes.iter().map(|&n| note_to_name(n.clamp(0, 127) as u8)).collect();
    response.on_hover_text(format!("Sounding: {}", names.join(" ")));
}

/// The chord under a step: a piano for the root, a row for the bass, and
/// the types. A type writes the step and moves along to the next, as does a
/// chord played on the keys. `held` are the notes down now.
fn chord_popover(ui: &mut egui::Ui, open: &mut ChordEntry, pattern: &ChordPattern, held: &[u8], listening: &Listening, set: &mut impl FnMut(String, f32)) {
    let step = open.step;
    let spec = pattern.specs[step];
    let switch_on = |set: &mut dyn FnMut(String, f32)| {
        if !pattern.gates[step] {
            set(format!("Step {} Gate", step + 1), 1.0);
        }
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Step {}", step + 1)).strong());
        ui.label(RichText::new(spec.name()).color(theme::signal::CONTROL).size(16.0));
        if let Some(top) = spec.top {
            ui.label(RichText::new(format!("{} on top", spec.spell(top))).weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("▸").on_hover_text("Next step (→)").clicked() {
                open.step = step_along(step, 1, pattern.steps);
            }
            if ui.small_button("◂").on_hover_text("Previous step (←)").clicked() {
                open.step = step_along(step, -1, pattern.steps);
            }
        });
    });

    // Root
    ui.horizontal(|ui| {
        ui.label(RichText::new("Root").weak());
        if ui.small_button("‹").on_hover_text("Octave down").clicked() {
            open.base = open.base.saturating_sub(12);
        }
        ui.label(RichText::new(format!("{} – {}", note_to_name(open.base), note_to_name(open.base + 23))).weak());
        if ui.small_button("›").on_hover_text("Octave up").clicked() {
            open.base = (open.base + 12).min(TOP_BASE);
        }
        ui.label(RichText::new(note_to_name(spec.root as u8)).color(theme::signal::CONTROL));
    });
    let config = PianoConfig::scale(theme::signal::CONTROL).with_size(OCTAVE_WIDTH, KEY_HEIGHT);
    let root = spec.root as u8;
    let played = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            let mut played = None;
            for octave in [open.base, open.base + 12] {
                // The keys held while a chord's played, or else the root
                let lit = if held.is_empty() { std::slice::from_ref(&root) } else { held };
                let active_notes = lit.iter().copied().filter(|n| (octave..octave + 12).contains(n)).collect();
                let data = PianoData { active_notes, base_note: octave, ..Default::default() };
                let (response, key) = piano_keys(ui, &data, &config);
                if let Some(key) = key {
                    if response.on_hover_text(note_to_name(octave + key)).clicked() {
                        played = Some(octave + key);
                    }
                }
            }
            played
        })
        .inner;
    if let Some(note) = played {
        set(format!("Step {} Root", step + 1), note as f32);
        switch_on(set);
    }
    heard_line(ui, listening);

    // Bass
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(RichText::new("Bass").weak());
        let current = spec.bass.map_or(0, |pc| pc as usize + 1);
        for choice in 0..=12 {
            // Spelled as the chord's name will spell it: G# under Am, not Ab
            let label = match choice {
                0 => format!("{} (root)", spec.root_name()),
                _ => ChordSpec { bass: Some(choice as i32 - 1), ..spec }.bass_name().unwrap_or_default(),
            };
            if ui.selectable_label(current == choice, label).clicked() {
                set(format!("Step {} Bass", step + 1), choice as f32);
            }
        }
    });

    // Top: the chord tones the voicing keeps, any of which can go on top
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(RichText::new("Top").weak());
        if ui.selectable_label(spec.top.is_none(), "Auto").on_hover_text("Leave the top to the voicing").clicked() {
            set(format!("Step {} Top", step + 1), 0.0);
        }
        let tones = kept_tones(&spec, &pattern.settings);
        for &pc in &tones {
            let chip = ui.selectable_label(spec.top == Some(pc), spec.spell(pc));
            if chip.on_hover_text("Voice the chord with this tone on top").clicked() {
                set(format!("Step {} Top", step + 1), pc as f32 + 1.0);
            }
        }
        // A top the voicing doesn't keep, written before the chord changed
        if let Some(pc) = spec.top.filter(|pc| !tones.contains(pc)) {
            ui.label(RichText::new(spec.spell(pc)).strikethrough().weak())
                .on_hover_text("Not one of the tones this chord keeps, so the voicing leaves it out");
        }
    });

    // Type: writes the step and moves on
    ui.add_space(2.0);
    ui.label(RichText::new("Type").weak());
    egui::Grid::new("chord_types").spacing([2.0, 2.0]).show(ui, |ui| {
        for (place, &index) in TYPE_ORDER.iter().enumerate() {
            let kind = &CHORD_TYPES[index];
            let name = format!("{}{}", spec.root_name(), kind.suffix);
            let chip = ui.add_sized([52.0, 20.0], egui::SelectableLabel::new(spec.kind == index, name));
            if chip.on_hover_text(kind.label).clicked() {
                set(format!("Step {} Type", step + 1), index as f32);
                switch_on(set);
                open.step = step_along(step, 1, pattern.steps);
            }
            if place % 6 == 5 {
                ui.end_row();
            }
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        let mut velocity = (pattern.velocities[step] * 127.0).round();
        if ui.add(egui::Slider::new(&mut velocity, 0.0..=127.0).text("Velocity").integer()).changed() {
            set(format!("Step {} Velocity", step + 1), velocity);
        }
        let mut tie = pattern.ties[step];
        if ui.checkbox(&mut tie, "Tie").on_hover_text("Holds this chord into the next step, which changes chord without a new attack (Shift+click)").changed() {
            set(format!("Step {} Tie", step + 1), if tie { 1.0 } else { 0.0 });
        }
    });
    ui.label(RichText::new("Play a root, pick a bass if it has one, then a type: it writes the step and moves on").small().weak());
    ui.label(RichText::new("Or hold the chord on a MIDI keyboard or the letter keys, and let go").small().weak())
        .on_hover_text("The letter keys play this piano: Z is its lowest C, and ‹ › move them too");
}

/// Under the piano: the chord being played, named as it's held, or else
/// the last one written, with what didn't fit when it was only the nearest.
fn heard_line(ui: &mut egui::Ui, listening: &Listening) {
    let gathering = listening.ear.gathering();
    let (verb, notes, heard, step) = match &listening.last {
        _ if !gathering.is_empty() => {
            let notes: Vec<i32> = gathering.iter().map(|&n| n as i32).collect();
            let heard = recognise(&notes);
            ("Hearing", notes, heard, None)
        }
        Some(last) => ("Heard", last.notes.iter().map(|&n| n as i32).collect(), Some(last.heard), Some(last.step)),
        None => return,
    };
    let Some(heard) = heard else { return };
    let spec = &heard.spec;
    // Each pitch once, from the bass up
    let mut classes: Vec<i32> = Vec::new();
    for pc in notes.iter().map(|n| n.rem_euclid(12)) {
        if !classes.contains(&pc) {
            classes.push(pc);
        }
    }
    let one_note = classes.len() == 1;
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(format!("{verb} {}  →", spelled(spec, classes.iter().copied()))).weak());
        let name = if one_note { format!("root {}", spec.root_name()) } else { spec.name() };
        ui.label(RichText::new(name).color(theme::signal::CONTROL).strong());
        if let Some(step) = step {
            ui.label(RichText::new(format!("in step {}", step + 1)).weak());
        }
        if let Some(misfit) = misfit(&heard).filter(|_| !one_note) {
            ui.label(RichText::new(format!("nearest: {misfit}")).weak().italics());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_popover_lists_every_type_once() {
        let mut order = TYPE_ORDER.to_vec();
        order.sort_unstable();
        assert_eq!(order, (0..CHORD_TYPES.len()).collect::<Vec<_>>());
    }

    #[test]
    fn keys_sit_along_the_strip() {
        assert_eq!(key_position(24, 24), Some((0.5, false)), "C1, the first white key");
        assert_eq!(key_position(25, 24), Some((1.0, true)), "C#1 between C and D");
        assert_eq!(key_position(36, 24), Some((7.5, false)), "C2 an octave of white keys along");
        assert_eq!(key_position(71, 24), Some((27.5, false)), "B4, the last of four octaves");
        assert_eq!(key_position(72, 24), None, "off the end");
        assert_eq!(key_position(23, 24), None, "below it");
    }

    #[test]
    fn the_popover_piano_opens_with_the_root_in_its_upper_octave() {
        assert_eq!(popover_base(36), 24);
        assert_eq!(popover_base(5), 0);
        assert_eq!(popover_base(127), TOP_BASE);
    }
}
