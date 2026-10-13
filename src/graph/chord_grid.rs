//! The Chord Sequencer's face: the voicing on a keyboard, then its steps,
//! each named by its chord.
//!
//! - The keyboard strip shows the chord on the cables. Each voice is a dot
//!   over its key that glides to its next note, so voice leading can be
//!   watched: most dots creep a step or stay put while the chord changes.
//! - A step shows its chord's name ("Am9", with "/G#" under it for a slash
//!   chord), its velocity as a line along its foot, a notch in its head
//!   when it names the tone on top, and a small arrow in its corner when it
//!   names its own strum, up or down.
//! - Click switches a step on or off, Shift+click its tie; drag up or down
//!   moves its root by semitones (Shift: octaves), as on the Step Sequencer.
//! - Right-click opens the chord under the step: a piano for the root, rows
//!   for the slash bass and the tone on top, and the chord types; its
//!   velocity, tie and stroke under them. Choosing a type writes the step
//!   and moves to the next, so a progression goes in as a lead sheet reads:
//!   root, type, root, type.
//! - Each choice is heard as it's made: a type, bass or top clicked sounds
//!   the step's chord for a moment, as it now stands, and a key on the
//!   piano sounds its note alone. Resting the pointer on a chip sounds its
//!   chord without writing it, so two can be compared before one's chosen
//!   (see [`super::chord_chime`]).
//! - Or play the chord, on a MIDI keyboard or the letter keys, while the
//!   popover is open. When the hand lifts, the chord is named (see
//!   [`recognise`]) and written, and the popover moves to the next step, the
//!   way the Step Sequencer's piano step-records. Its top note is written as
//!   the step's top where voice leading wouldn't put it there (see
//!   [`Played::new`]). The keys light on its piano while they're held,
//!   and the chord's name shows as it's played.
//!   A MIDI keyboard's touch writes the step's velocity too: the mean of
//!   the chord's notes, so a progression's accents go in as they're played.
//!   The letter keys leave it as it was.
//! - While the hand is down, the chord sounds through the sequencer's own
//!   cables as it will play once written: named, voiced by Range, Voices and
//!   Bass Voice, led from the chord before it, and strummed the way its step
//!   will be struck, clock or no clock (see [`audition_of`]). A MIDI
//!   keyboard's chords are left to the patch when a MIDI module there plays
//!   them.

use std::sync::OnceLock;

use eframe::egui::{self, Color32, CursorIcon, Id, Key, Modifiers, PopupCloseBehavior, Rect, RichText, Sense, Stroke};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::{Audition, DspModule, MAX_CHANNELS};
use crate::modules::chord_sequencer::recognition::{pitch_classes, recognise, Heard};
use crate::modules::chord_sequencer::{kept_tones, looped_voicings, voice_chord, Chord, ChordSequencer, ChordSpec, StrumDirection, VoicingSettings, Voicing, CHORD_TYPES, MAX_STEPS};
use crate::modules::sequencer::{note_to_name, SequenceDirection};
use crate::widgets::{piano_keys, PianoConfig, PianoData};

use super::chord_chime::{ring_for, Chimes, Chip};
use super::chord_ear::{take_letter_keys, ChordEar, Hand};
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
/// Modules that play a MIDI keyboard's notes themselves.
const MIDI_MODULES: [&str; 2] = ["input.midi_note", "input.poly_midi"];
const HINT: &str = "Click: chord on/off · Shift+click: tie\nDrag up or down: root (Shift: by octave)\nRight-click: edit the chord";

/// A Chord Sequencer's pattern and settings, as its face shows them.
struct ChordPattern {
    steps: usize,
    specs: [ChordSpec; MAX_STEPS],
    gates: [bool; MAX_STEPS],
    ties: [bool; MAX_STEPS],
    velocities: [f32; MAX_STEPS],
    /// Each step's own stroke, or `None` to follow `strum_dir`.
    strums: [Option<StrumDirection>; MAX_STEPS],
    strum_dir: StrumDirection,
    /// Seconds between one string's strike and the next.
    strum: f32,
    settings: VoicingSettings,
    channels: usize,
    bass_voice: bool,
    leading: bool,
    direction: SequenceDirection,
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
            strums: std::array::from_fn(|s| StrumDirection::from_step_param(params[ChordSequencer::step_strum_param(s)])),
            strum_dir: StrumDirection::from_param(params[ChordSequencer::PARAM_STRUM_DIR]),
            strum: params[ChordSequencer::PARAM_STRUM] / 1000.0,
            settings,
            channels,
            bass_voice,
            leading: params[ChordSequencer::PARAM_VOICE_LEADING] > 0.5,
            direction: SequenceDirection::from_param(params[ChordSequencer::PARAM_DIRECTION]),
        }
    }

    /// A step's chord as it would sound on its own, in root position: what
    /// the keyboard shows before the engine has played anything.
    fn preview(&self, step: usize) -> Vec<i32> {
        self.voiced(&self.specs[step])
    }

    /// A chord voiced in root position, the bass voice first if on.
    fn voiced(&self, spec: &ChordSpec) -> Vec<i32> {
        self.on_channels(spec, &voice_chord(spec, &self.settings, None))
    }

    /// A voicing of `spec` as the channels carry it, the bass voice first
    /// if on.
    fn on_channels(&self, spec: &ChordSpec, chord: &Chord) -> Vec<i32> {
        let mut notes = Vec::with_capacity(self.channels);
        if self.bass_voice {
            notes.push(spec.bass_note());
        }
        if !self.bass_voice || self.channels > 1 {
            notes.extend(chord.notes());
        }
        notes
    }

    /// How `spec` would sound written into `step` with its top left to the
    /// voicing, once the pattern has looped a while: led from the chord
    /// before it, or in root position with Voice Leading off. `None` when no
    /// one chord comes before it, played ping-pong or at random.
    fn left_to_voicing(&self, step: usize, spec: ChordSpec) -> Option<Chord> {
        self.looped(step, ChordSpec { top: None, ..spec })
    }

    /// Whether `step` is strummed from its highest note down: by its own
    /// stroke, or Strum Dir's. Alt's hand moves down on every other clock
    /// after a reset, so a step takes the stroke of its place the first time
    /// round, as its clock falls in the bar.
    fn strummed_down(&self, step: usize) -> bool {
        match self.strums[step].unwrap_or(self.strum_dir) {
            StrumDirection::Up => false,
            StrumDirection::Down => true,
            StrumDirection::Alternate => {
                let clocks = match self.direction {
                    SequenceDirection::Backward => self.steps - 1 - step,
                    _ => step,
                };
                clocks % 2 == 1
            }
        }
    }

    /// How `spec` sounds written into `step`, top and all, once the pattern
    /// has looped a while. See [`Self::left_to_voicing`].
    fn looped(&self, step: usize, spec: ChordSpec) -> Option<Chord> {
        if !self.leading {
            return Some(voice_chord(&spec, &self.settings, None));
        }
        let order: Vec<usize> = match self.direction {
            SequenceDirection::Forward => (0..self.steps).collect(),
            SequenceDirection::Backward => (0..self.steps).rev().collect(),
            SequenceDirection::PingPong | SequenceDirection::Random => return None,
        };
        let (mut specs, mut gates) = (self.specs, self.gates);
        specs[step] = spec;
        gates[step] = true;
        looped_voicings(&specs, &gates, &order, &self.settings)[step]
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

/// The popover's ear, the chord it last wrote, the chimes of chords chosen
/// on its chips, and the chord sounding on the cables, played or chosen.
#[derive(Clone, Debug, Default)]
struct Listening {
    ear: ChordEar,
    last: Option<Played>,
    chimes: Chimes,
    sounding: Option<Audition>,
    /// Whether what's sounding is a chime, not the hand.
    chiming: bool,
}

/// A chord played into a step: the notes, what they were heard as (with the
/// top it writes), how hard they were struck, if a MIDI keyboard played
/// them, and the hand's top note, when the voicing keeps it.
#[derive(Clone, Debug)]
struct Played {
    notes: Vec<u8>,
    heard: Heard,
    velocity: Option<u8>,
    step: usize,
    top: Option<i32>,
}

impl Played {
    /// Names the notes played into `step` (lowest first). A note alone, or
    /// in octaves, is a root: the step keeps its type and top, without a
    /// slash bass.
    ///
    /// A chord keeps the voicing it was played in: its top note goes on top,
    /// if it's one of the tones the voicing keeps. The step's Top is written
    /// only where Voice Leading would put another tone there, so most steps
    /// stay on Auto and go on following the chords around them.
    fn new(hand: Hand, step: usize, pattern: &ChordPattern) -> Option<Self> {
        let Hand { notes, velocity } = hand;
        let lowest = *notes.first()? as i32;
        if notes.iter().all(|&n| (n as i32 - lowest) % 12 == 0) {
            let spec = ChordSpec { root: lowest, bass: None, ..pattern.specs[step] };
            return Some(Self { notes, heard: Heard { spec, extra: 0, missing: 0 }, velocity, step, top: None });
        }
        let mut heard = recognise(&notes.iter().map(|&n| n as i32).collect::<Vec<_>>())?;
        let highest = *notes.last()? as i32 % 12;
        let top = kept_tones(&heard.spec, &pattern.settings).contains(&highest).then_some(highest);
        let on_top_anyway = |pc: i32| pattern.left_to_voicing(step, heard.spec).is_some_and(|chord| chord.notes()[chord.len - 1] % 12 == pc);
        heard.spec.top = top.filter(|&pc| !on_top_anyway(pc));
        Some(Self { notes, heard, velocity, step, top })
    }

    /// The chord as it sounds: with the hand's top on top, whether or not the
    /// step needs to name it.
    fn sounding(&self) -> ChordSpec {
        ChordSpec { top: self.top.or(self.heard.spec.top), ..self.heard.spec }
    }

    /// The step's parameters, as written: root, type, bass, top and gate, and
    /// velocity when it was played with a touch.
    fn changes(&self) -> Vec<(String, f32)> {
        let n = self.step + 1;
        let spec = &self.heard.spec;
        let mut changes = vec![
            (format!("Step {n} Root"), spec.root.clamp(0, 127) as f32),
            (format!("Step {n} Type"), spec.kind as f32),
            (format!("Step {n} Bass"), spec.bass.map_or(0.0, |pc| pc as f32 + 1.0)),
            (format!("Step {n} Top"), spec.top.map_or(0.0, |pc| pc as f32 + 1.0)),
            (format!("Step {n} Gate"), 1.0),
        ];
        if let Some(velocity) = self.velocity {
            changes.push((format!("Step {n} Velocity"), velocity as f32));
        }
        changes
    }
}

/// The chord being played into `step`, as the sequencer will play it once
/// written: named as it will be, with the hand's top note on top, as hard as
/// it's struck (or at the step's velocity, on the letter keys), and voiced
/// as the pattern will reach it, led from the chord before. A progression
/// typed in then sounds as it will play, whatever order its steps are
/// written in. Ping-pong and random come to a step from more than one chord,
/// so there it's voiced in root position, as the face shows a step before
/// it's played. It's strummed the way the step will be struck. `None` while
/// nothing's held.
fn audition_of(hand: Hand, step: usize, pattern: &ChordPattern) -> Option<Audition> {
    let played = Played::new(hand, step, pattern)?;
    let velocity = played.velocity.map_or(pattern.velocities[step], |v| v as f32 / 127.0);
    Some(chord_audition(played.sounding(), step, velocity, pattern))
}

/// `spec` written into `step`, as the sequencer will play it: voiced as the
/// pattern will reach it and strummed as the step will be struck, as hard
/// as `velocity`. See [`audition_of`].
fn chord_audition(spec: ChordSpec, step: usize, velocity: f32, pattern: &ChordPattern) -> Audition {
    let voiced = match pattern.looped(step, spec) {
        Some(chord) => pattern.on_channels(&spec, &chord),
        None => pattern.voiced(&spec),
    };
    let mut audition = Audition {
        len: voiced.len().min(MAX_CHANNELS) as u8,
        root: spec.root.clamp(0, 127) as u8,
        bass: spec.bass_note().clamp(0, 127) as u8,
        velocity,
        down: pattern.strummed_down(step),
        ..Audition::default()
    };
    for (slot, &note) in audition.notes.iter_mut().zip(&voiced) {
        *slot = note.clamp(0, 127) as u8;
    }
    audition
}

/// What a chip chosen for `step` sounds: the step's chord as the chip
/// leaves it, or a root on the piano alone, as a piano would play it, on
/// the first channel at the step's velocity.
fn chime_of(sound: Sound, step: usize, pattern: &ChordPattern) -> Audition {
    let velocity = pattern.velocities[step];
    match sound {
        Sound::Chord(spec) => chord_audition(spec, step, velocity, pattern),
        Sound::Note(note) => {
            let mut audition = Audition { len: 1, root: note, bass: note, velocity, ..Audition::default() };
            audition.notes[0] = note;
            audition
        }
    }
}

/// What a chip sounds: a chord, or a note on the piano.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sound {
    Chord(ChordSpec),
    Note(u8),
}

/// The popover's chips under the pointer this frame: the one clicked, if
/// any, and the one it's over, each with the step it's for and what it
/// sounds.
#[derive(Default)]
struct Touch {
    clicked: Option<(Chip, usize, Sound)>,
    hovered: Option<(Chip, usize, Sound)>,
}

impl Touch {
    /// Follows a chip's response: clicked, or under the pointer.
    fn follow(&mut self, response: &egui::Response, chip: Chip, step: usize, sound: Sound) {
        if response.clicked() {
            self.clicked = Some((chip, step, sound));
        }
        if response.hovered() {
            self.hovered = Some((chip, step, sound));
        }
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
    // Changes to the chord sounding, to send to the engine
    let mut auditions = Vec::new();
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
            // Its own stroke: a small arrow in the corner, up the chord or down
            if let Some(stroke) = pattern.strums[step] {
                stroke_arrow(painter, *rect, stroke == StrumDirection::Up, if on { Color32::from_rgba_unmultiplied(18, 40, 20, 150) } else { OFF_TEXT }, z);
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
        let midi = user_state.midi_held();
        let held = listening.ear.held(&midi);
        if let Some(chord) = listening.ear.listen(&midi).and_then(|hand| Played::new(hand, open.step, &pattern)) {
            open.step = step_along(open.step, 1, pattern.steps);
            played = Some(chord.clone());
            listening.last = Some(chord);
        }
        // Heard while the hand is down, unless the patch already plays it
        let plays_midi = graph.nodes.iter().any(|(_, node)| MIDI_MODULES.contains(&node.user_data.module_id));
        let hand = Hand { notes: listening.ear.gathering().to_vec(), velocity: listening.ear.velocity() };
        let by_hand = (listening.ear.keyed() || !plays_midi).then(|| audition_of(hand, open.step, &pattern)).flatten();

        let anchor = &cells[open.anchor].1;
        let mut touch = Touch::default();
        egui::popup_below_widget(ui, entry_id, anchor, PopupCloseBehavior::CloseOnClickOutside, |ui| {
            chord_popover(ui, &mut open, &pattern, &held, &listening, &mut touch, &mut set);
        });

        // A chip clicked sounds the step as it now stands, and one rested on
        // as it would, without writing it. The hand, while it's down, goes
        // over them both
        let now = ui.input(|i| i.time);
        let chimes = &mut listening.chimes;
        let rested = chimes.rest_on(touch.hovered.map(|(chip, ..)| chip), now).then_some(touch.hovered).flatten();
        let struck = touch.clicked.or(rested).filter(|_| by_hand.is_none()).is_some_and(|(chip, step, sound)| {
            let chime = chime_of(sound, step, &pattern);
            chimes.strike(chip, chime, ring_for(&chime, pattern.strum), now)
        });
        if by_hand.is_some() {
            chimes.hush();
        }
        let chime = chimes.ringing(now);
        let sounding = by_hand.or(chime);
        // A new chime, or the hand coming down on one, strikes afresh: the
        // engine would only move the pitches under a chord still sounding
        let afresh = struck || (by_hand.is_some() && listening.chiming);
        if afresh && listening.sounding.is_some() {
            auditions.push(None);
        }
        if afresh || sounding != listening.sounding {
            auditions.push(sounding);
        }
        listening.sounding = sounding;
        listening.chiming = by_hand.is_none() && chime.is_some();
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
            // Closed with the hand still down, or a chime ringing: the
            // chord stops with it
            if listening.sounding.is_some() {
                auditions.push(None);
            }
        }
    });
    for chord in auditions {
        edits.push(SynthResponse::Audition { node_id, chord });
    }
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

/// A step's own stroke, in its top right corner: a small triangle pointing
/// up for a strum from the lowest note, down for one from the highest.
fn stroke_arrow(painter: &egui::Painter, rect: Rect, up: bool, color: Color32, z: f32) {
    let centre = egui::pos2(rect.right() - 5.0 * z, rect.top() + 5.0 * z);
    let (w, h) = (2.5 * z, 2.0 * z);
    let (tip, base) = if up { (-h, h) } else { (h, -h) };
    let points = vec![
        egui::pos2(centre.x, centre.y + tip),
        egui::pos2(centre.x - w, centre.y + base),
        egui::pos2(centre.x + w, centre.y + base),
    ];
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
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
fn chord_popover(ui: &mut egui::Ui, open: &mut ChordEntry, pattern: &ChordPattern, held: &[u8], listening: &Listening, touch: &mut Touch, set: &mut impl FnMut(String, f32)) {
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
                    let note = octave + key;
                    if response.on_hover_text(note_to_name(note)).clicked() {
                        played = Some(note);
                        touch.clicked = Some((Chip::Root(note), step, Sound::Note(note)));
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
    heard_line(ui, listening, pattern, step);

    // Bass
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(RichText::new("Bass").weak());
        let current = spec.bass.map_or(0, |pc| pc as usize + 1);
        for choice in 0..=12usize {
            // Spelled as the chord's name will spell it: G# under Am, not Ab
            let chosen = ChordSpec { bass: choice.checked_sub(1).map(|pc| pc as i32), ..spec };
            let label = match choice {
                0 => format!("{} (root)", spec.root_name()),
                _ => chosen.bass_name().unwrap_or_default(),
            };
            let chip = ui.selectable_label(current == choice, label);
            touch.follow(&chip, Chip::Bass(choice), step, Sound::Chord(chosen));
            if chip.clicked() {
                set(format!("Step {} Bass", step + 1), choice as f32);
            }
        }
    });

    // Top: the chord tones the voicing keeps, any of which can go on top
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(RichText::new("Top").weak());
        let auto = ui.selectable_label(spec.top.is_none(), "Auto").on_hover_text("Leave the top to the voicing");
        touch.follow(&auto, Chip::Top(None), step, Sound::Chord(ChordSpec { top: None, ..spec }));
        if auto.clicked() {
            set(format!("Step {} Top", step + 1), 0.0);
        }
        let tones = kept_tones(&spec, &pattern.settings);
        for &pc in &tones {
            let chip = ui.selectable_label(spec.top == Some(pc), spec.spell(pc));
            touch.follow(&chip, Chip::Top(Some(pc)), step, Sound::Chord(ChordSpec { top: Some(pc), ..spec }));
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
            touch.follow(&chip, Chip::Type(index), step, Sound::Chord(ChordSpec { kind: index, ..spec }));
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
    strum_row(ui, step, pattern, set);
    ui.label(RichText::new("Play a root, pick a bass if it has one, then a type: it writes the step and moves on").small().weak())
        .on_hover_text("Each choice sounds as it's made. Rest the pointer on a chip to hear its chord without writing it");
    ui.label(RichText::new("Or hold the chord on a MIDI keyboard or the letter keys, and let go").small().weak())
        .on_hover_text("The letter keys play this piano: Z is its lowest C, and ‹ › move them too. The chord sounds through this sequencer while it's held, as it will be written");
}

/// The step's own stroke: Auto, following Strum dir, or Up or Down for this
/// step alone, so a written rhythm puts an upstroke where it belongs.
fn strum_row(ui: &mut egui::Ui, step: usize, pattern: &ChordPattern, set: &mut impl FnMut(String, f32)) {
    let follows = match pattern.strum_dir {
        StrumDirection::Up => "Up",
        StrumDirection::Down => "Down",
        StrumDirection::Alternate => "Alt, by the clock",
    };
    let choices = [
        (None, "Auto", format!("Follows Strum dir ({follows})")),
        (Some(StrumDirection::Up), "Up ▴", "Strums this chord from its lowest note, as a guitar's downstroke".to_string()),
        (Some(StrumDirection::Down), "Down ▾", "Strums this chord from its highest note, as a guitar's upstroke".to_string()),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(RichText::new("Strum").weak());
        for (value, (stroke, label, hint)) in choices.into_iter().enumerate() {
            if ui.selectable_label(pattern.strums[step] == stroke, label).on_hover_text(hint).clicked() {
                set(format!("Step {} Strum", step + 1), value as f32);
            }
        }
    });
}

/// Under the piano: the chord being played into `step`, named as it's held,
/// or else the last one written, with the top it names and what didn't fit
/// when it was only the nearest.
fn heard_line(ui: &mut egui::Ui, listening: &Listening, pattern: &ChordPattern, step: usize) {
    let gathering = listening.ear.gathering();
    let hand = Hand { notes: gathering.to_vec(), velocity: listening.ear.velocity() };
    let hearing = (!gathering.is_empty()).then(|| Played::new(hand, step, pattern)).flatten();
    let (verb, played, step) = match (&hearing, &listening.last) {
        _ if !gathering.is_empty() => ("Hearing", hearing.as_ref(), None),
        (_, Some(last)) => ("Heard", Some(last), Some(last.step)),
        (_, None) => return,
    };
    let Some(played) = played else { return };
    let (notes, heard) = (&played.notes, &played.heard);
    let spec = &heard.spec;
    // Each pitch once, from the bass up
    let mut classes: Vec<i32> = Vec::new();
    for pc in notes.iter().map(|&n| n as i32 % 12) {
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
        // The hand's top, named for the step or left to voice leading
        match (spec.top.filter(|_| !one_note), played.top) {
            (Some(top), _) => {
                ui.label(RichText::new(format!("{} on top", spec.spell(top))).color(theme::signal::CONTROL));
            }
            (None, Some(top)) => {
                ui.label(RichText::new(format!("{} on top", spec.spell(top))).weak())
                    .on_hover_text("Voice leading puts it on top here already, so the step's Top stays on Auto");
            }
            (None, None) => {}
        }
        if let Some(step) = step {
            ui.label(RichText::new(format!("in step {}", step + 1)).weak());
        }
        if let Some(velocity) = played.velocity {
            ui.label(RichText::new(format!("at velocity {velocity}")).weak())
                .on_hover_text("How hard it's played, the mean of its notes: written as the step's Velocity");
        }
        if let Some(misfit) = misfit(heard).filter(|_| !one_note) {
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

    /// The default pattern (Cmaj7, Am7, Fmaj7, G7), `voices` channels wide.
    fn pattern(voices: usize, bass_voice: bool) -> ChordPattern {
        let params: Vec<f32> = ChordSequencer::new().parameters().iter().map(|p| p.default).collect();
        ChordPattern {
            steps: 4,
            specs: std::array::from_fn(|s| ChordSequencer::spec(&params, s)),
            gates: [true; MAX_STEPS],
            ties: [false; MAX_STEPS],
            velocities: [0.75; MAX_STEPS],
            strums: [None; MAX_STEPS],
            strum_dir: StrumDirection::Up,
            strum: 0.0,
            settings: VoicingSettings { voicing: Voicing::Close, voices: voices - bass_voice as usize, range: 3, rootless: bass_voice },
            channels: voices,
            bass_voice,
            leading: true,
            direction: SequenceDirection::Forward,
        }
    }

    /// Notes played on the letter keys, with no touch.
    fn keys(notes: &[u8]) -> Hand {
        Hand { notes: notes.to_vec(), velocity: None }
    }

    /// Two chords looping, C then Dm(add9), voiced in three voices without
    /// their roots, as a pad over a bass is.
    fn c_then_d_minor_add_9() -> ChordPattern {
        let mut two = pattern(3, false);
        two.steps = 2;
        two.specs[0] = ChordSpec { root: 48, kind: 0, bass: None, top: None };
        two.specs[1] = Played::new(keys(&[50, 53, 57, 64]), 1, &two).unwrap().heard.spec;
        two.settings.rootless = true;
        two
    }

    /// The Top a chord played into `step` writes, as the parameter holds it.
    fn top_written(notes: &[u8], step: usize, pattern: &ChordPattern) -> f32 {
        let played = Played::new(keys(notes), step, pattern).unwrap();
        let name = format!("Step {} Top", step + 1);
        played.changes().into_iter().find(|(param, _)| *param == name).unwrap().1
    }

    #[test]
    fn a_chord_played_keeps_its_top_note() {
        let two = c_then_d_minor_add_9();
        assert_eq!(CHORD_TYPES[two.specs[1].kind].suffix, "madd9");
        // Led from E G C, Dm(add9) would take E F A, with A on top. Played
        // F A E over D, the E stays on top
        assert_eq!(two.left_to_voicing(1, two.specs[1]).unwrap().notes(), [52, 53, 57]);
        assert_eq!(top_written(&[50, 53, 57, 64], 1, &two), 5.0, "E on top");
        // Played D E F A, it's what voice leading picks: Top stays Auto
        assert_eq!(top_written(&[50, 52, 53, 57], 1, &two), 0.0);
        // ...though it sounds with the A on top while it's held
        let held = audition_of(keys(&[50, 52, 53, 57]), 1, &two).unwrap();
        assert_eq!(held.notes().last().map(|n| n % 12), Some(9));
    }

    #[test]
    fn a_top_note_the_voicing_leaves_out_is_left_to_it() {
        let mut two = c_then_d_minor_add_9();
        two.specs[1].top = Some(5);
        // Rootless, the D isn't kept: the step goes back to Auto
        assert_eq!(top_written(&[53, 57, 62, 64, 74], 1, &two), 0.0);
        // One note is only a root: the step keeps the top it had
        assert_eq!(top_written(&[50], 1, &two), 6.0);
    }

    #[test]
    fn without_one_chord_before_it_the_top_is_always_written() {
        let mut two = c_then_d_minor_add_9();
        // With Voice Leading off, F A E is root position: nothing to write
        two.leading = false;
        assert_eq!(top_written(&[50, 53, 57, 64], 1, &two), 0.0);
        // Ping-pong and random come to a step from more than one chord, so
        // the top played is written, to be sure of it
        two.leading = true;
        for direction in [SequenceDirection::PingPong, SequenceDirection::Random] {
            two.direction = direction;
            assert_eq!(top_written(&[50, 53, 57, 64], 1, &two), 5.0, "{direction:?}");
        }
        // Backward, the loop starts on the Dm(add9), in root position, and
        // settles there with the E on top: it's the A that needs writing
        two.direction = SequenceDirection::Backward;
        assert_eq!(top_written(&[50, 53, 57, 64], 1, &two), 0.0);
        assert_eq!(top_written(&[50, 52, 53, 57], 1, &two), 10.0);
    }

    #[test]
    fn a_chord_is_heard_as_it_will_be_written() {
        // In root position, with Voice Leading off
        let mut four = pattern(4, false);
        four.leading = false;
        // C E G played up high sounds in the Range octave, as the step will,
        // with the G on top, as it was played
        let c = audition_of(keys(&[72, 76, 79]), 0, &four).unwrap();
        assert_eq!(c.notes(), [55, 60, 64, 67]);
        // ...and the Root output plays the root in the octave it was played in
        assert_eq!((c.root, c.bass, c.velocity), (72, 72, 0.75));
        // One note is a root: the step keeps its type, Cmaj7 becomes Amaj7
        let a = audition_of(keys(&[57]), 0, &four).unwrap();
        assert_eq!(a.notes(), [57, 61, 64, 68]);
        assert_eq!(audition_of(keys(&[]), 0, &four), None, "nothing held");

        // G# A C E is Am over G#: with Bass Voice, the G# is the first channel
        let bass = audition_of(keys(&[56, 57, 60, 64]), 1, &pattern(4, true)).unwrap();
        assert_eq!((bass.root, bass.notes()[0]), (57, bass.bass));
        assert_eq!(bass.bass % 12, 8);
        assert_eq!(bass.notes().len(), 4);
    }

    #[test]
    fn a_chord_is_heard_led_from_the_chord_before_it() {
        // Fmaj7 played into step 3, after Am7: it sounds as the loop will
        // reach it, not in root position. Played A C E F, with the F on top
        // where voice leading puts it anyway
        let mut four = pattern(4, false);
        let fmaj7 = keys(&[57, 60, 64, 65]);
        let spec = Played::new(fmaj7.clone(), 2, &four).unwrap().sounding();
        let led = four.left_to_voicing(2, spec).unwrap();
        let heard = audition_of(fmaj7.clone(), 2, &four).unwrap();
        assert_eq!(heard.notes().iter().map(|&n| n as i32).collect::<Vec<_>>(), led.notes());
        assert_ne!(led.notes(), four.voiced(&spec), "led, not in root position");
        // A top the hand names narrows the voicing as the step's would
        let on_top = audition_of(keys(&[53, 57, 64, 72]), 2, &four).unwrap();
        assert_eq!(on_top.notes().last().map(|n| n % 12), Some(0), "C on top");
        // With a Bass Voice the bass still comes first
        let bassed = audition_of(fmaj7.clone(), 2, &pattern(4, true)).unwrap();
        assert_eq!(bassed.notes()[0], bassed.bass);
        // Ping-pong and random reach it from more than one chord: root position
        for direction in [SequenceDirection::PingPong, SequenceDirection::Random] {
            four.direction = direction;
            let heard = audition_of(fmaj7.clone(), 2, &four).unwrap();
            assert_eq!(heard.notes().iter().map(|&n| n as i32).collect::<Vec<_>>(), four.voiced(&spec), "{direction:?}");
        }
    }

    #[test]
    fn a_chord_is_heard_strummed_as_its_step_will_be_struck() {
        let mut four = pattern(4, false);
        let down = |pattern: &ChordPattern, step: usize| audition_of(keys(&[57, 60, 64]), step, pattern).unwrap().down;
        assert!(!down(&four, 1), "Strum Dir Up");
        four.strum_dir = StrumDirection::Down;
        assert!(down(&four, 1));
        // A step's own stroke goes over Strum Dir
        four.strums[1] = Some(StrumDirection::Up);
        assert!(!down(&four, 1));
        // Alt: the hand moves down on every other clock, the first one up
        four.strum_dir = StrumDirection::Alternate;
        four.strums[1] = None;
        assert_eq!((0..4).map(|step| down(&four, step)).collect::<Vec<_>>(), [false, true, false, true]);
        // Backward, the first clock plays the last step
        four.direction = SequenceDirection::Backward;
        assert_eq!((0..4).map(|step| down(&four, step)).collect::<Vec<_>>(), [true, false, true, false]);
    }

    #[test]
    fn a_chord_played_with_a_touch_writes_how_hard() {
        let four = pattern(4, false);
        let struck = Hand { notes: vec![57, 60, 64], velocity: Some(127) };
        // It's heard as hard as it's struck...
        assert_eq!(audition_of(struck.clone(), 1, &four).unwrap().velocity, 1.0);
        // ...and written so
        let played = Played::new(struck, 1, &four).unwrap();
        assert!(played.changes().contains(&("Step 2 Velocity".to_string(), 127.0)));
        // The letter keys leave the step's velocity as it was
        let keyed = Played::new(keys(&[57, 60, 64]), 1, &four).unwrap();
        assert!(keyed.changes().iter().all(|(name, _)| !name.ends_with("Velocity")));
    }

    #[test]
    fn a_chord_chosen_sounds_as_one_played_would() {
        // Step 3's Fmaj7 made Fm7 on its chip: heard led from the Am7, as
        // the loop will reach it, the way a chord played into it would be
        let four = pattern(4, false);
        let m7 = CHORD_TYPES.iter().position(|kind| kind.suffix == "m7").unwrap();
        let fm7 = ChordSpec { kind: m7, ..four.specs[2] };
        let chosen = chime_of(Sound::Chord(fm7), 2, &four);
        let led = four.left_to_voicing(2, fm7).unwrap();
        assert_eq!(chosen.notes().iter().map(|&n| n as i32).collect::<Vec<_>>(), led.notes());
        assert_ne!(led.notes(), four.voiced(&fm7), "led, not in root position");
        assert_eq!((chosen.root, chosen.velocity), (fm7.root as u8, 0.75), "at the step's velocity");
        // A bass chosen under it goes on the first channel with Bass Voice on
        let bassed = pattern(4, true);
        let over_eb = ChordSpec { bass: Some(3), ..fm7 };
        let chime = chime_of(Sound::Chord(over_eb), 2, &bassed);
        assert_eq!((chime.notes()[0], chime.notes()[0] % 12), (chime.bass, 3));
    }

    #[test]
    fn a_key_on_the_piano_sounds_alone() {
        let four = pattern(4, false);
        let d = chime_of(Sound::Note(50), 1, &four);
        assert_eq!(d.notes(), [50]);
        assert_eq!((d.root, d.bass, d.velocity), (50, 50, 0.75));
    }

    #[test]
    fn the_popover_piano_opens_with_the_root_in_its_upper_octave() {
        assert_eq!(popover_base(36), 24);
        assert_eq!(popover_base(5), 0);
        assert_eq!(popover_base(127), TOP_BASE);
    }
}
