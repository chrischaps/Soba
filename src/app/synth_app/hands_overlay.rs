//! The Hands overlay (#431), the app's side: a quiet strip along the foot
//! of the canvas that sketches the controller in Soba's own line style
//! (the encoders and faders it was taught, in rows of up to eight, and,
//! when the patch maps any, eight pads) and says beside each what it does
//! right now: `Cutoff` over `Filter`,
//! in the filter's colour. The encoders' pointers and the faders' caps sit
//! where their knobs do and move as they move, a control brightens as its
//! CC arrives, and a fader that hasn't picked its knob up yet shows a
//! purple tick where the hardware is. Controls that do nothing stay faint.
//!
//! It's toggled from the MIDI In menu or with Ctrl+H, and remembered. It's
//! a screen overlay rather than part of the patch: it keeps its size as
//! the canvas zooms, sits clear of the status bar, and takes only the
//! clicks that land on it. What each control does is worked out in
//! `crate::app::hands_overlay`.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};

use super::*;
use crate::app::hands::Step;
use crate::app::hands_overlay::{self as model, Control, HandsNow, Hardware, Sketch, Source};
use crate::engine::MidiEvent;
use crate::graph::steady_text;
use crate::persistence::{MidiControl, MidiSource};
use crate::widgets::knob::KnobConfig;

/// The storage key of whether the strip is shown.
const SHOWN_KEY: &str = "hands_overlay";

/// How long a control stays bright after it's heard, in seconds.
const HEARD_GLOW: f64 = 0.6;

/// The strip's measures, in screen points at full size.
mod size {
    /// Room round the inside of the strip.
    pub const MARGIN: f32 = 10.0;
    /// One row of encoders or pads.
    pub const ROW: f32 = 32.0;
    /// The device's name and what the strip shows.
    pub const PLATE: f32 = 118.0;
    /// An encoder with its label beside it.
    pub const ENCODER: f32 = 118.0;
    /// A fader with its label under it.
    pub const FADER: f32 = 72.0;
    /// A pad with its label beside it.
    pub const PAD: f32 = 98.0;
    /// Between one kind of control and the next, with a hairline in it.
    pub const GAP: f32 = 18.0;
    /// From the canvas's foot.
    pub const LIFT: f32 = 12.0;
    /// An encoder's radius.
    pub const DIAL: f32 = 10.5;
    /// A pad's side.
    pub const PAD_SIDE: f32 = 22.0;
    /// The name's text, and the module's under it.
    pub const NAME: f32 = 12.5;
    pub const MODULE: f32 = 10.5;
}

/// The strip: shown or not, and what the controller last sent.
#[derive(Debug, Default)]
pub(super) struct HandsOverlay {
    pub on: bool,
    /// Each control heard, by (a note, its number, channel 1-16): the last
    /// value it sent (a note's velocity, 0 let go) and when.
    heard: HashMap<(bool, u8, u8), (u8, f64)>,
}

impl HandsOverlay {
    /// Notes a CC or note from the controller, to brighten its control.
    pub fn hear(&mut self, event: MidiEvent, now: f64) {
        let (key, value) = match event {
            MidiEvent::ControlChange { channel, controller, value } => ((false, controller, channel + 1), value),
            MidiEvent::NoteOn { channel, note, velocity } => ((true, note, channel + 1), velocity),
            MidiEvent::NoteOff { channel, note, .. } => ((true, note, channel + 1), 0),
            _ => return,
        };
        self.heard.insert(key, (value, now));
    }

    /// What `hardware` last sent, and when: on any channel, for a mapping
    /// that listens on them all.
    fn last(&self, hardware: Hardware) -> Option<(u8, f64)> {
        let note = hardware.source == MidiSource::Note;
        self.heard
            .iter()
            .filter(|((n, number, channel), _)| *n == note && *number == hardware.number && (hardware.channel == 0 || *channel == hardware.channel))
            .map(|(_, &heard)| heard)
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    pub fn load(&mut self, storage: &dyn eframe::Storage) {
        self.on = storage.get_string(SHOWN_KEY).is_some_and(|v| v == "on");
    }

    pub fn store(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(SHOWN_KEY, if self.on { "on" } else { "off" }.to_string());
    }
}

/// Which kind of control a face is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Encoder,
    Fader,
    Pad,
}

/// One control as the strip draws it.
#[derive(Clone, Debug)]
struct Face {
    kind: Kind,
    /// Its number on the hands' badges: "3", "F1"; a pad's note.
    number: String,
    /// What it turns ("Cutoff"), and on what ("Filter").
    name: Option<String>,
    module: Option<String>,
    /// The module's category colour, or a quiet grey.
    ink: Color32,
    /// Where it stands round its travel, 0-1: the knob's, or the hardware's.
    travel: Option<f32>,
    /// Where the hardware is, while a fader hasn't picked its knob up.
    hardware_at: Option<f32>,
    /// How bright it is from being heard just now, 0-1.
    heard: f32,
    /// A pad held down.
    pressed: bool,
    /// An endless encoder.
    endless: bool,
    /// Soba is being taught this one now: it breathes.
    teaching: bool,
    /// What it sends, said faintly while it does nothing: "CC 74".
    sends: Option<String>,
    /// The words under the pointer.
    tip: String,
}

impl Face {
    fn does_something(&self) -> bool {
        self.name.is_some()
    }
}

impl SynthApp {
    /// Shows the strip, or hides it.
    pub(super) fn toggle_hands_overlay(&mut self) {
        self.hands_overlay.on = !self.hands_overlay.on;
    }

    /// What each control on the controller does right now.
    fn hands_sketch(&self) -> Sketch {
        let hands = &self.midi.hands;
        let teaching = hands.teaching().map(|t| (t.taught(), t.step()));
        let holding = hands.active().then(|| hands.mappings().as_slice());
        // The first Pads module's pads (#428), on the notes they hear
        let graph = &self.graph_state.graph;
        let pads_node = graph.nodes.iter().find(|(_, n)| n.user_data.module_id == crate::modules::pads::ID).map(|(id, _)| id);
        let notes: Vec<Option<(u8, u8)>> = pads_node
            .map(|id| crate::modules::pads::listens(&crate::graph::pads_params(graph, id)).iter().map(|l| Some((l.note?, l.channel))).collect())
            .unwrap_or_default();
        let pads = pads_node.and_then(|id| self.user_state.get_engine_node_id(id)).map(|node_id| model::PadsModule { node_id, notes: &notes });
        model::sketch(HandsNow { controller: hands.controller(), holding, teaching, pads }, self.midi.mappings.as_slice())
    }

    /// Where a Pads module's pad is patched: what it plays ("Kick", or the
    /// input's name), the module, and the module's colour.
    fn pad_target(&self, engine_node_id: u64, pad: usize) -> Option<(String, String, Color32)> {
        let graph = &self.graph_state.graph;
        let node = graph.nodes.get(self.user_state.graph_node_id(engine_node_id)?)?;
        let gate = format!("Gate {}", pad + 1);
        let (_, output) = node.outputs.iter().find(|(name, _)| *name == gate)?;
        let (input, _) = graph.iter_connections().find(|(_, from)| from == output)?;
        let to = graph.nodes.get(graph.get_input(input).node)?;
        let (port, _) = to.inputs.iter().find(|(_, id)| *id == input)?;
        // A drum says which drum it is
        let kind = to.inputs.iter().find(|(name, _)| name == "Type").and_then(|(_, id)| match &graph.get_input(*id).value {
            SynthValueType::Select { value, options, .. } => options.get(*value).cloned(),
            _ => None,
        });
        Some((kind.unwrap_or_else(|| port.clone()), to.user_data.display_name.clone(), to.user_data.category.color()))
    }

    /// A control as the strip draws it, at `now`: the `index`th of its
    /// kind, from 0.
    fn hands_face(&self, kind: Kind, index: usize, control: &Control, teaching: bool, now: f64) -> Face {
        let mut face = Face {
            kind,
            number: match kind {
                Kind::Pad => control.hardware.map_or_else(|| (index + 1).to_string(), |h| short_text(h).trim_start_matches("CC ").to_string()),
                Kind::Encoder => Step::Encoder(index).badge(),
                Kind::Fader => Step::Fader(index).badge(),
            },
            name: None,
            module: None,
            ink: theme::text::DISABLED,
            travel: None,
            hardware_at: None,
            heard: 0.0,
            pressed: false,
            endless: control.hardware.is_some_and(|h| h.endless),
            teaching,
            sends: control.hardware.filter(|_| control.does.is_none()).filter(|_| kind != Kind::Pad).map(short_text),
            tip: String::new(),
        };
        let what = match kind {
            Kind::Encoder => format!("Encoder {}", index + 1),
            Kind::Fader => format!("Fader {}", index + 1),
            Kind::Pad => format!("Pad {}", index + 1),
        };
        face.tip = match control.hardware {
            Some(hardware) => format!("{what} · {}{}", hardware.text(), if hardware.endless { ", endless" } else { "" }),
            None => format!("{what} · not taught"),
        };

        let last = control.hardware.and_then(|h| self.hands_overlay.last(h));
        if let Some((value, at)) = last {
            let since = ((now - at) / HEARD_GLOW).clamp(0.0, 1.0) as f32;
            face.heard = (1.0 - since).powi(2);
            // A pad is held while its note is, or its CC is up
            let note = control.hardware.is_some_and(|h| h.source == MidiSource::Note);
            face.pressed = kind == Kind::Pad && if note { value > 0 } else { value >= 64 };
            // A fader or knob with ends is where it last said it was
            if kind != Kind::Pad && !face.endless {
                face.travel = Some(value as f32 / 127.0);
            }
        }
        let Some(mapping) = &control.does else {
            // A Pads module's pad plays whatever its gate is patched into
            if let Some((node, pad)) = control.pad {
                let pads = self.module_name(node).unwrap_or("Pads").to_string();
                match self.pad_target(node, pad) {
                    Some((name, module, ink)) => {
                        face.tip.push_str(&format!("\nGate {} of {pads}, into {module}", pad + 1));
                        (face.name, face.module, face.ink, face.sends) = (Some(name), Some(module), ink, None);
                    }
                    None => face.tip.push_str(&format!("\nGate {} of {pads}, patched into nothing", pad + 1)),
                }
            } else if control.hardware.is_some() {
                face.tip.push_str("\nNothing in the patch follows it");
            }
            return face;
        };
        let node = self.user_state.graph_node_id(mapping.node_id).and_then(|id| self.graph_state.graph.nodes.get(id).map(|n| (id, n)));
        face.name = Some(model::short_name(&mapping.param_name).to_string());
        if let Some((_, node)) = node {
            face.module = Some(node.user_data.display_name.clone());
            face.ink = node.user_data.category.color();
        }
        let more = match control.also {
            0 => String::new(),
            n => format!(" (and {n} more)"),
        };
        face.tip.push_str(&format!("\n{} on {}{more}", mapping.param_name, face.module.as_deref().unwrap_or("a module that's gone")));

        // Where its knob stands, as the hardware would show it
        let input = node.filter(|_| mapping.scene.is_none()).and_then(|(id, _)| self.param_input(id, mapping.param_index));
        if let Some(input) = input.filter(|_| kind != Kind::Pad) {
            let value = &self.graph_state.graph.get_input(input).value;
            let travel = mapped_travel(value, value.actual_value(), mapping.min_value, mapping.max_value, mapping.control);
            face.travel = Some(if mapping.invert { 1.0 - travel } else { travel });
            let waiting = self.user_state.midi_pickups.get(&(mapping.node_id, mapping.param_index)).filter(|(_, caught)| caught.is_none());
            face.hardware_at = waiting.map(|&(at, _)| if mapping.invert { 1.0 - at } else { at });
        }
        face
    }

    /// The strip along the foot of the canvas, while it's shown.
    pub(super) fn show_hands_overlay(&mut self, ctx: &egui::Context) {
        if !self.hands_overlay.on || self.embedded || self.editor_rect.width() < 200.0 {
            return;
        }
        let now = ctx.input(|i| i.time);
        let sketch = self.hands_sketch();
        let waiting_for = match sketch.source {
            Source::Teaching(slot) => slot,
            _ => None,
        };
        let faces: Vec<Face> = sketch
            .slots
            .iter()
            .enumerate()
            .map(|(slot, control)| {
                let (kind, index) = match slot.checked_sub(sketch.encoders) {
                    None => (Kind::Encoder, slot),
                    Some(fader) => (Kind::Fader, fader),
                };
                self.hands_face(kind, index, control, waiting_for == Some(slot), now)
            })
            .collect();
        let (encoders, faders) = faces.split_at(sketch.encoders);
        let pads: Vec<Face> = sketch.pads.iter().enumerate().map(|(i, pad)| self.hands_face(Kind::Pad, i, pad, false, now)).collect();
        let plate = self.hands_plate(&sketch);

        // However many it has (#485)
        let layout = Layout::of(encoders.len(), faders.len());
        // Full size where it fits, smaller in a narrow window
        let mut width = 2.0 * size::MARGIN + size::PLATE;
        if !encoders.is_empty() {
            width += size::GAP + layout.encoder_columns as f32 * size::ENCODER;
        }
        if !faders.is_empty() {
            width += size::GAP + layout.fader_columns as f32 * size::FADER;
        }
        if !pads.is_empty() {
            width += size::GAP + (model::PADS / 2) as f32 * size::PAD;
        }
        let span = layout.rows as f32 * size::ROW;
        let height = 2.0 * size::MARGIN + span;
        let room = self.editor_rect.width() - 2.0 * size::LIFT;
        let scale = (room / width).min(1.0);
        let strip = Rect::from_center_size(
            pos2(self.editor_rect.center().x, self.editor_rect.bottom() - size::LIFT - height * scale / 2.0),
            vec2(width, height) * scale,
        );

        let mut asked = None;
        egui::Area::new(egui::Id::unique("hands_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(strip.min)
            .movable(false)
            .show(ctx, |ui| {
                // The whole strip takes its clicks and drags, and only it
                let (rect, _) = ui.allocate_exact_size(strip.size(), Sense::click_and_drag());
                let painter = ui.painter_at(rect.expand(4.0));
                painter.rect_filled(rect, 12.0 * scale, theme::background::PANEL.gamma_multiply(0.97));
                painter.rect_stroke(rect, 12.0 * scale, Stroke::new(1.0, theme::background::GRID_MAJOR), StrokeKind::Inside);

                let s = scale;
                let top = rect.top() + size::MARGIN * s;
                let mut x = rect.left() + size::MARGIN * s;
                asked = draw_plate(ui, &painter, Rect::from_min_size(pos2(x, top), vec2(size::PLATE, 2.0 * size::ROW) * s), &plate, s);
                x += size::PLATE * s;
                let breath = ((now * 3.0).sin().abs()) as f32;
                if !encoders.is_empty() {
                    divider(&painter, x + size::GAP * s / 2.0, top, span * s);
                    x += size::GAP * s;
                    let columns = layout.encoder_columns;
                    for (i, face) in encoders.iter().enumerate() {
                        let (row, column) = (i / columns, i % columns);
                        let cell = Rect::from_min_size(pos2(x + column as f32 * size::ENCODER * s, top + row as f32 * size::ROW * s), vec2(size::ENCODER, size::ROW) * s);
                        draw_encoder(&painter, cell, face, s, breath);
                        tip(ui, cell, face, ("hands_encoder", i));
                    }
                    x += columns as f32 * size::ENCODER * s;
                }
                if !faders.is_empty() {
                    divider(&painter, x + size::GAP * s / 2.0, top, span * s);
                    x += size::GAP * s;
                    let columns = layout.fader_columns;
                    for (i, face) in faders.iter().enumerate() {
                        let (row, column) = (i / columns, i % columns);
                        let cell = Rect::from_min_size(pos2(x + column as f32 * size::FADER * s, top + 2.0 * row as f32 * size::ROW * s), vec2(size::FADER, 2.0 * size::ROW) * s);
                        draw_fader(&painter, cell, face, s, breath);
                        tip(ui, cell, face, ("hands_fader", i));
                    }
                    x += columns as f32 * size::FADER * s;
                }
                if !pads.is_empty() {
                    divider(&painter, x + size::GAP * s / 2.0, top, span * s);
                    x += size::GAP * s;
                    let columns = model::PADS / 2;
                    for (i, face) in pads.iter().enumerate() {
                        let (row, column) = (i / columns, i % columns);
                        let cell = Rect::from_min_size(pos2(x + column as f32 * size::PAD * s, top + row as f32 * size::ROW * s), vec2(size::PAD, size::ROW) * s);
                        draw_pad(&painter, cell, face, s);
                        tip(ui, cell, face, ("hands_pad", i));
                    }
                }
            });

        // Something moving, or about to: keep drawing while it settles
        let settling = encoders.iter().chain(faders).chain(&pads).any(|f| f.heard > 0.0 || f.teaching || f.hardware_at.is_some());
        if settling {
            crate::app::pace::animate(ctx);
        }
        match asked {
            Some(Ask::Teach) => self.start_teaching(),
            Some(Ask::Map) => self.controller_map.open = true,
            None => {}
        }
    }

    /// What the strip's plate says: the controller, and what its controls
    /// are doing.
    fn hands_plate(&self, sketch: &Sketch) -> Plate {
        let device = Some(self.midi.hands.device()).filter(|d| !d.is_empty()).unwrap_or("Controller").to_string();
        let quiet = theme::text::QUIET;
        let (doing, ink, ask) = match sketch.source {
            Source::Teaching(Some(slot)) if slot < sketch.encoders => (format!("Teaching: encoder {}", slot + 1), theme::signal::MIDI, None),
            Source::Teaching(Some(slot)) => (format!("Teaching: fader {}", slot - sketch.encoders + 1), theme::signal::MIDI, None),
            Source::Teaching(None) => ("Teaching".to_string(), theme::signal::MIDI, None),
            Source::Following => {
                let selected = self.graph_state.selected_nodes.first().and_then(|&id| self.graph_state.graph.nodes.get(id));
                match selected {
                    Some(node) => (format!("Follows {}", node.user_data.display_name), node.user_data.category.color(), None),
                    None => ("Select a module".to_string(), quiet, None),
                }
            }
            Source::Patch => ("The patch's mappings".to_string(), quiet, None),
            Source::Untaught => ("Not taught yet".to_string(), quiet, Some(("Teach Your Controller…".to_string(), Ask::Teach))),
        };
        let teaching = matches!(sketch.source, Source::Teaching(_));
        let more = match sketch.unshown {
            1 => "1 more mapping…".to_string(),
            n => format!("{n} more mappings…"),
        };
        let ask = ask.or_else(|| (sketch.unshown > 0 && !teaching).then_some((more, Ask::Map)));
        Plate { device, doing, ink, ask }
    }
}

/// The strip's plate: whose controller, what its controls are doing, and
/// a link to do something about it.
struct Plate {
    device: String,
    doing: String,
    ink: Color32,
    ask: Option<(String, Ask)>,
}

/// How the strip lays out a controller's encoders and faders, in columns
/// and rows: a MiniLab's eight encoders are two rows of four, as on the
/// hardware, a BeatStep's sixteen two rows of eight, and a Launch
/// Control's twenty-four three rows of eight. The faders go in rows of
/// eight, each two rows tall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    encoder_columns: usize,
    fader_columns: usize,
    /// The strip's height in rows: at least the plate's two.
    rows: usize,
}

impl Layout {
    /// The most in a row.
    const ROW: usize = 8;

    fn of(encoders: usize, faders: usize) -> Self {
        let encoder_columns = if encoders <= 2 * Self::ROW { encoders.div_ceil(2) } else { Self::ROW }.max(1);
        let fader_columns = faders.clamp(1, Self::ROW);
        let rows = encoders.div_ceil(encoder_columns).max(2 * faders.div_ceil(fader_columns)).max(2);
        Self { encoder_columns, fader_columns, rows }
    }
}

/// A link on the plate, clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Ask {
    Teach,
    Map,
}

/// How far round its travel `actual` sits between a mapping's ends, along
/// its knob's curve: a frequency by octaves.
fn mapped_travel(value: &SynthValueType, actual: f32, min: f32, max: f32, control: MidiControl) -> f32 {
    let (low, high) = (min.min(max), min.max(max));
    if high <= low {
        return 0.0;
    }
    let logarithmic = matches!(value, SynthValueType::Number { spec, .. } if spec.logarithmic) && control == MidiControl::Knob && low >= 0.0 && high > 0.0;
    let config = KnobConfig { range: low..=high, logarithmic, ..Default::default() };
    let travel = config.travel(actual.clamp(low, high)).clamp(0.0, 1.0);
    if min > max { 1.0 - travel } else { travel }
}

/// What a control sends, without its channel: "CC 74", "C2".
fn short_text(hardware: Hardware) -> String {
    Hardware { channel: 0, ..hardware }.text()
}

/// The plate. Returns the link, if it was clicked.
fn draw_plate(ui: &mut egui::Ui, painter: &egui::Painter, rect: Rect, plate: &Plate, s: f32) -> Option<Ask> {
    let ctx = ui.ctx().clone();
    let title = FontId::new(14.0 * s, egui::FontFamily::Name(theme::TITLE_FAMILY.into()));
    let small = FontId::proportional(size::MODULE * s);
    let room = rect.width() - 6.0 * s;
    // A short purple bar, as the Controller Map's heading has
    let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 4.0 * s), vec2(3.0 * s, 14.0 * s));
    painter.rect_filled(bar, 1.5 * s, theme::signal::MIDI);
    let device = steady_text::fit(&ctx, &plate.device, &title, room - 10.0 * s);
    painter.text(pos2(rect.left() + 9.0 * s, bar.center().y), Align2::LEFT_CENTER, device, title, theme::text::PRIMARY);
    let doing = steady_text::fit(&ctx, &plate.doing, &small, room);
    painter.text(pos2(rect.left(), rect.top() + 33.0 * s), Align2::LEFT_CENTER, doing, small.clone(), plate.ink);

    let (text, ask) = plate.ask.as_ref()?;
    let text = steady_text::fit(&ctx, text, &small, room);
    let at = pos2(rect.left(), rect.top() + 51.0 * s);
    let wide = steady_text::text_width(ui, &text, &small);
    let hit = Rect::from_min_size(at - vec2(2.0, 8.0 * s), vec2(wide + 4.0, 16.0 * s));
    let response = ui.interact(hit, ui.scope_id().with(("hands_plate_ask", *ask)), Sense::click());
    let link = theme::signal::MIDI.lerp_to_gamma(theme::text::PRIMARY, if response.hovered() { 0.45 } else { 0.2 });
    painter.text(at, Align2::LEFT_CENTER, &text, small, link);
    if response.hovered() {
        painter.hline(at.x..=at.x + wide, at.y + 7.0 * s, Stroke::new(1.0, link.gamma_multiply(0.6)));
    }
    let response = response.on_hover_text(match ask {
        Ask::Teach => "Turn each encoder, then move each fader, so Soba knows them. Once per controller",
        Ask::Map => "Mappings on controls the sketch doesn't have: see them all in the Controller Map",
    });
    response.clicked().then_some(*ask)
}

/// A hairline between one kind of control and the next, down a strip
/// `span` tall.
fn divider(painter: &egui::Painter, x: f32, top: f32, span: f32) {
    painter.vline(x, top + 0.18 * span..=top + 0.82 * span, Stroke::new(1.0, theme::background::GRID_MAJOR));
}

/// The pointer over a control, saying what it is and does.
fn tip(ui: &mut egui::Ui, cell: Rect, face: &Face, id: (&str, usize)) {
    ui.interact(cell, ui.scope_id().with(id), Sense::hover()).on_hover_text(&face.tip);
}

/// The ring's ink: the module's colour when it does something, faint when
/// it doesn't, brighter as it's heard, and purple while it's taught.
fn ring_ink(face: &Face, breath: f32) -> Color32 {
    if face.teaching {
        return theme::signal::MIDI.gamma_multiply(0.45 + 0.55 * breath);
    }
    let base = if face.does_something() { face.ink.gamma_multiply(0.85) } else { theme::background::WIDGET_ACTIVE };
    base.lerp_to_gamma(theme::text::PRIMARY, 0.5 * face.heard)
}

/// Where round an encoder's 270° sweep `travel` (0-1) points, as an angle
/// from +x, clockwise on screen: from the lower left round to the lower
/// right, as Soba's knobs turn.
fn angle(travel: f32) -> f32 {
    std::f32::consts::PI * (0.75 + 1.5 * travel)
}

fn on_circle(center: Pos2, radius: f32, angle: f32) -> Pos2 {
    center + radius * vec2(angle.cos(), angle.sin())
}

/// An arc round `center` from `from` to `to` (travels, 0-1).
fn arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, stroke: Stroke) {
    let steps = ((to - from).abs() * 40.0).ceil().max(2.0) as usize;
    let points: Vec<Pos2> = (0..=steps).map(|i| on_circle(center, radius, angle(from + (to - from) * i as f32 / steps as f32))).collect();
    painter.add(Shape::line(points, stroke));
}

/// A small purple pill with the control's number, as the canvas's hand
/// badges have (#429): so the strip's encoder 3 is the knob badged 3.
fn number_badge(painter: &egui::Painter, center: Pos2, number: &str, strength: f32, s: f32) {
    let height = 11.0 * s;
    // Wider for "16", and again for "F12"
    let width = height + number.chars().count().saturating_sub(1) as f32 * 5.0 * s;

    let rect = Rect::from_center_size(center, vec2(width, height));
    painter.rect_filled(rect, height / 2.0, theme::signal::MIDI.gamma_multiply(0.8 * strength));
    painter.text(rect.center() + vec2(0.0, 0.25 * s), Align2::CENTER_CENTER, number, FontId::proportional(8.0 * s), Color32::WHITE.gamma_multiply(0.92 * strength.max(0.5)));
}

/// What a control does, beside it or under it: its name, and its module's
/// in the module's colour. A control that does nothing says what it sends,
/// faintly, so the free ones can be found.
fn label(painter: &egui::Painter, at: Pos2, align: Align2, room: f32, face: &Face, s: f32) {
    let ctx = &painter.ctx().clone();
    let name_font = FontId::proportional(size::NAME * s);
    let module_font = FontId::proportional(size::MODULE * s);
    let gap = 7.0 * s;
    if face.teaching {
        let (ask, how) = if face.kind == Kind::Encoder { ("Turn it", "a few slow clicks") } else { ("Move it", "up or down") };
        painter.text(at - vec2(0.0, gap), align, ask, name_font, theme::signal::MIDI);
        painter.text(at + vec2(0.0, gap), align, how, module_font, theme::text::QUIET);
        return;
    }
    match &face.name {
        Some(name) => {
            let name = steady_text::fit(ctx, name, &name_font, room);
            painter.text(at - vec2(0.0, gap), align, name, name_font, theme::text::PRIMARY.gamma_multiply(0.92));
            if let Some(module) = &face.module {
                let module = steady_text::fit(ctx, module, &module_font, room);
                painter.text(at + vec2(0.0, gap), align, module, module_font, face.ink.gamma_multiply(0.9));
            }
        }
        None => {
            if let Some(hardware) = &face.sends {
                let hardware = steady_text::fit(ctx, hardware, &module_font, room);
                painter.text(at, align, hardware, module_font, theme::text::DISABLED.gamma_multiply(0.8));
            }
        }
    }
}

fn draw_encoder(painter: &egui::Painter, cell: Rect, face: &Face, s: f32, breath: f32) {
    let r = size::DIAL * s;
    let lit = face.does_something() || face.teaching;
    // Its number at its left, as the knob it turns is badged
    number_badge(painter, pos2(cell.left() + 5.5 * s, cell.center().y), &face.number, if lit { 1.0 } else { 0.35 }, s);
    let center = pos2(cell.left() + 17.0 * s + r, cell.center().y);
    let ink = ring_ink(face, breath);

    // The heard glow: a soft ring that fades
    if face.heard > 0.0 {
        painter.circle_stroke(center, r + 3.0 * s, Stroke::new(3.0 * s, ink.gamma_multiply(0.25 * face.heard)));
    }
    painter.circle_filled(center, r, theme::background::MAIN.gamma_multiply(if lit { 0.9 } else { 0.5 }));
    if face.endless {
        // An endless encoder: a whole ring, knurled
        painter.circle_stroke(center, r, Stroke::new(1.0 * s, ink.gamma_multiply(0.55)));
        for i in 0..12 {
            let a = i as f32 * std::f32::consts::TAU / 12.0;
            painter.line_segment([on_circle(center, r + 1.0 * s, a), on_circle(center, r + 2.6 * s, a)], Stroke::new(0.8 * s, ink.gamma_multiply(0.35)));
        }
    } else {
        // A knob with ends: its sweep, with a gap at the foot
        arc(painter, center, r, 0.0, 1.0, Stroke::new(1.0 * s, ink.gamma_multiply(0.55)));
    }
    if let Some(travel) = face.travel {
        if lit {
            arc(painter, center, r, 0.0, travel, Stroke::new(2.0 * s, ink));
        }
        let pointer = if lit { theme::text::PRIMARY.gamma_multiply(0.9) } else { ink };
        painter.line_segment([on_circle(center, 0.25 * r, angle(travel)), on_circle(center, 0.85 * r, angle(travel))], Stroke::new(1.6 * s, pointer));
    }
    if let Some(at) = face.hardware_at {
        painter.line_segment([on_circle(center, r - 2.5 * s, angle(at)), on_circle(center, r + 3.0 * s, angle(at))], Stroke::new(1.6 * s, theme::signal::MIDI));
    }

    let text_at = pos2(center.x + r + 9.0 * s, cell.center().y);
    let room = cell.right() - text_at.x - 4.0 * s;
    label(painter, text_at, Align2::LEFT_CENTER, room, face, s);
}

fn draw_fader(painter: &egui::Painter, cell: Rect, face: &Face, s: f32, breath: f32) {
    let ink = ring_ink(face, breath);
    let lit = face.does_something() || face.teaching;
    let x = cell.center().x;
    let (top, bottom) = (cell.top() + 16.0 * s, cell.top() + 40.0 * s);
    let at = |travel: f32| bottom - travel.clamp(0.0, 1.0) * (bottom - top);

    // The slot, and the throw below the cap in the module's colour
    if face.heard > 0.0 {
        let glow = Rect::from_min_max(pos2(x - 5.0 * s, top - 2.0 * s), pos2(x + 5.0 * s, bottom + 2.0 * s));
        painter.rect_filled(glow, 5.0 * s, ink.gamma_multiply(0.12 * face.heard));
    }
    painter.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(2.0 * s, ink.gamma_multiply(0.4)));
    for tick in 0..=4 {
        let y = at(tick as f32 / 4.0);
        painter.line_segment([pos2(x - 7.0 * s, y), pos2(x - 4.5 * s, y)], Stroke::new(0.8 * s, theme::background::WIDGET_ACTIVE));
    }
    if let Some(travel) = face.travel {
        let y = at(travel);
        if lit {
            painter.line_segment([pos2(x, y), pos2(x, bottom)], Stroke::new(2.0 * s, ink));
        }
        let cap = Rect::from_center_size(pos2(x, y), vec2(14.0 * s, 5.0 * s));
        painter.rect_filled(cap, 1.5 * s, if lit { theme::text::PRIMARY.gamma_multiply(0.85) } else { theme::background::WIDGET_ACTIVE });
        painter.hline(cap.x_range().shrink(3.0 * s), y, Stroke::new(1.0 * s, theme::background::MAIN.gamma_multiply(0.8)));
    } else {
        // A fader never heard: its cap faint at the foot
        let cap = Rect::from_center_size(pos2(x, bottom), vec2(14.0 * s, 5.0 * s));
        painter.rect_stroke(cap, 1.5 * s, Stroke::new(1.0 * s, ink.gamma_multiply(0.6)), StrokeKind::Inside);
    }
    if let Some(at_hardware) = face.hardware_at {
        let y = at(at_hardware);
        painter.line_segment([pos2(x + 4.0 * s, y), pos2(x + 10.0 * s, y)], Stroke::new(1.6 * s, theme::signal::MIDI));
    }
    number_badge(painter, pos2(x, cell.top() + 5.5 * s), &face.number, if lit { 1.0 } else { 0.35 }, s);

    let text_at = pos2(x, cell.top() + 54.0 * s);
    label(painter, text_at, Align2::CENTER_CENTER, cell.width() - 6.0 * s, face, s);
}

fn draw_pad(painter: &egui::Painter, cell: Rect, face: &Face, s: f32) {
    let ctx = painter.ctx().clone();
    let side = size::PAD_SIDE * s;
    let pad = Rect::from_min_size(pos2(cell.left() + 2.0 * s, cell.center().y - side / 2.0), vec2(side, side));
    let ink = ring_ink(face, 0.0);
    if face.heard > 0.0 {
        painter.rect_stroke(pad.expand(2.5 * s), 6.5 * s, Stroke::new(3.0 * s, ink.gamma_multiply(0.25 * face.heard)), StrokeKind::Middle);
    }
    let fill = if face.pressed { ink.gamma_multiply(0.55) } else { theme::background::MAIN.gamma_multiply(0.9) };
    painter.rect_filled(pad, 4.5 * s, fill);
    painter.rect_stroke(pad, 4.5 * s, Stroke::new(1.0 * s, ink.gamma_multiply(if face.does_something() { 0.9 } else { 0.5 })), StrokeKind::Inside);
    let number_ink = if face.pressed { theme::text::PRIMARY } else { theme::text::QUIET };
    let number = steady_text::fit(&ctx, &face.number, &FontId::proportional(8.5 * s), side - 3.0 * s);
    painter.text(pad.center(), Align2::CENTER_CENTER, number, FontId::proportional(8.5 * s), number_ink);

    let text_at = pos2(pad.right() + 8.0 * s, cell.center().y);
    let room = cell.right() - text_at.x - 4.0 * s;
    label(painter, text_at, Align2::LEFT_CENTER, room, face, s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::hands::{Controller, Taught};
    use crate::persistence::{MidiEncoding, MidiMapping};

    fn add(app: &mut SynthApp, module_id: &str) -> egui_node_graph2::NodeId {
        let template = SynthNodeTemplate::from_module_id(module_id).expect("registered");
        let (node_id, _) = editing::add_module(&mut app.graph_state, &mut app.user_state, template, egui::pos2(0.0, 0.0));
        node_id
    }

    #[test]
    fn the_strip_names_each_control_and_its_module_and_sits_where_the_knob_does() {
        let mut app = SynthApp::new();
        let filter = add(&mut app, crate::modules::filter::ID);
        let engine = app.user_state.get_engine_node_id(filter).unwrap();
        let taught = |cc| Some(Taught { cc, channel: 1, encoding: MidiEncoding::Relative64 });
        app.midi.hands.know("", Controller { encoders: (70..78).map(taught).collect(), faders: Vec::new() });

        // Encoder 2 turns the filter's first knob, across its whole range
        let input = app.param_input(filter, 0).unwrap();
        let (min, max) = app.graph_state.graph.get_input(input).value.range();
        let name = app.graph_state.graph[filter].inputs.iter().find(|(_, id)| *id == input).map(|(n, _)| n.clone()).unwrap();
        app.midi.mappings.learn(MidiMapping::new(71, 1, engine, 0, name.clone(), min, max));

        let sketch = app.hands_sketch();
        assert_eq!(sketch.source, Source::Patch);
        let face = app.hands_face(Kind::Encoder, 1, &sketch.slots[1], false, 0.0);
        assert_eq!(face.name.as_deref(), Some(name.as_str()));
        assert_eq!(face.module.as_deref(), Some(app.graph_state.graph[filter].user_data.display_name.as_str()));
        assert_eq!(face.ink, app.graph_state.graph[filter].user_data.category.color());
        assert!(face.endless);
        let travel = face.travel.expect("where its knob stands");
        assert!((0.0..=1.0).contains(&travel));
        assert!(face.tip.starts_with("Encoder 2 · CC 71 · ch 1, endless"), "{}", face.tip);

        // Encoder 1 does nothing: faint, and says what it sends
        let free = app.hands_face(Kind::Encoder, 0, &sketch.slots[0], false, 0.0);
        assert!(!free.does_something());
        assert_eq!(free.sends.as_deref(), Some("CC 70"));

        // Heard just now, it brightens, and the glow fades
        app.hands_overlay.hear(MidiEvent::ControlChange { channel: 0, controller: 70, value: 65 }, 10.0);
        assert_eq!(app.hands_face(Kind::Encoder, 0, &sketch.slots[0], false, 10.0).heard, 1.0);
        assert_eq!(app.hands_face(Kind::Encoder, 0, &sketch.slots[0], false, 10.0 + HEARD_GLOW).heard, 0.0);
    }

    #[test]
    fn a_log_knob_sits_by_octaves_and_an_inverted_one_upside_down() {
        let spec = crate::graph::NumberSpec { min: 20.0, max: 20000.0, default: 1000.0, unit: "Hz", logarithmic: true, stepped: false };
        let value = SynthValueType::Number { value: 632.0, spec };
        let middle = mapped_travel(&value, 632.0, 20.0, 20000.0, MidiControl::Knob);
        assert!((middle - 0.5).abs() < 0.01, "{middle}");
        let flipped = mapped_travel(&value, 632.0, 20000.0, 20.0, MidiControl::Knob);
        assert!((flipped - 0.5).abs() < 0.01);
        assert_eq!(mapped_travel(&value, 20000.0, 20.0, 20000.0, MidiControl::Knob), 1.0);
    }

    #[test]
    fn the_strip_is_remembered() {
        #[derive(Default)]
        struct Memory(HashMap<String, String>);
        impl eframe::Storage for Memory {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_string(), value);
            }
            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }
            fn flush(&mut self) {}
        }
        let mut app = SynthApp::new();
        app.toggle_hands_overlay();
        let mut storage = Memory::default();
        app.hands_overlay.store(&mut storage);
        let mut again = HandsOverlay::default();
        again.load(&storage);
        assert!(again.on);
    }
}
