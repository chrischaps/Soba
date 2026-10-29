//! The Pad module's face (#480): one pad, lit as it's hit. Eight Pads in a
//! group are a controller's pads, as eight drums in a group are a kit.
//!
//! A hit flashes the pad in the Gate green, brighter the harder it was
//! struck, and the flash fades over a third of a second. While its gate is
//! open (held, or latched) it stays softly lit, a little brighter the
//! harder it's pressed. It wears its note quietly in a corner.
//!
//! Press the pad to play it: the harder toward its top. Learn, along the
//! top, takes its note and channel from the next pad hit on the controller.
//! Right-click it to learn it, set its note (or Any note, for a drum
//! machine's track, pitched trigs and all: #469) or choose the channel.
//! While it learns, it blinks in MIDI purple.
//!
//! While its group learns in order (#481), the top says where in the walk
//! it comes: its number while it waits its turn, Skip on its turn, and a
//! tick once it's learned, its rim staying softly purple.
//!
//! [`pad_control`] is the pad itself, drawn in any cell, so a group's face
//! can show a Pad pinned to it (#482): **Show on group**, in the pad's menu,
//! puts it there, wearing the name it's given on the face in its corner
//! (on its own face too, so a kit's Pads say which track each is). It
//! plays the Pad's own Press, so it's the same pad in two places.

use std::sync::OnceLock;

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use egui_node_graph2::NodeId;

use crate::app::module_learn::{LearnKinds, ModuleLearn};
use crate::app::theme;
use super::learn_walk::Mark;
use crate::dsp::{DspModule, NoteClaims};
use crate::modules::pad::{self, Listen};
use crate::modules::Pad;
use crate::persistence::MidiControl;

use super::display_frame;
use super::groups::PAD_PRESS;
use super::node_data::KnobPlace;
use super::sampler_take::MIN_PRESS_SECONDS;
use super::{SynthGraph, SynthGraphState, SynthResponse, WalkAction};

/// The pad's height, before zoom.
const PAD_HEIGHT: f32 = 60.0;
/// The gap around the pad, before zoom.
const GAP: f32 = 5.0;
/// The strip along the top, before zoom.
const TOP: f32 = 15.0;
/// How long a hit's flash takes to fade, in seconds: its time constant.
const FLASH: f32 = 0.12;
/// How soft the softest press on the face is, at a pad's foot.
const SOFTEST_PRESS: f32 = 0.35;

/// The Pad's parameter names and defaults, in its order.
pub(super) fn parameters() -> &'static [(&'static str, f32)] {
    static NAMES: OnceLock<Vec<(&'static str, f32)>> = OnceLock::new();
    NAMES.get_or_init(|| Pad::new().parameters().iter().map(|p| (p.name, p.default)).collect())
}

/// A Pad node's parameters, in the module's order, as the node holds them.
pub fn params_of(graph: &SynthGraph, node_id: NodeId) -> Vec<f32> {
    parameters().iter().map(|&(name, default)| display_frame::value_of(graph, node_id, name).unwrap_or(default)).collect()
}

/// What the Pad `node_id` listens to, as its node holds it.
pub fn listen_of(graph: &SynthGraph, node_id: NodeId) -> Listen {
    pad::listen(&params_of(graph, node_id))
}

/// The notes every Pad in `graph` claims (#428), for the UI to leave off
/// the pianos and their ears as the engine leaves them off Note In: each
/// from its Device (#454), given the slot of each input open and those open
/// for their thru routes alone (#463).
pub fn note_claims(graph: &SynthGraph, slot_of: impl Fn(&str) -> Option<u8>, thru_only: u8) -> NoteClaims {
    let mut claims = NoteClaims::EMPTY;
    for (node_id, node) in graph.nodes.iter() {
        if node.user_data.module_id == pad::ID {
            // Its own Device, or a MIDI In's device and channel (#479)
            let heard = super::midi_device::heard(graph, node_id);
            let device = super::midi_device::listen_to(heard.device, &slot_of, thru_only);
            match heard.channel {
                None => pad::claim_notes(&params_of(graph, node_id), device, &mut claims),
                Some(channel) => {
                    let mut own = NoteClaims::EMPTY;
                    pad::claim_notes(&params_of(graph, node_id), device, &mut own);
                    claims.merge_channel(&own, channel);
                }
            }
        }
    }
    claims
}

/// A note's name, as the pianos have it: 36 is C2.
pub fn note_name(note: u8) -> String {
    let names = crate::modules::quantizer::NOTE_NAMES;
    format!("{}{}", names[note as usize % 12], note as i32 / 12 - 1)
}

/// The note a pad hears, as it wears it: "C2", or "any" on Any Note.
pub fn heard_note(listen: Listen) -> String {
    if listen.any { "any".to_string() } else { note_name(listen.note) }
}

/// The note a pad hears, in full: "C2 (36)", or "any note".
fn heard_in_full(listen: Listen) -> String {
    if listen.any { "any note".to_string() } else { format!("{} ({})", note_name(listen.note), listen.note) }
}

/// A channel as a pad shows it.
fn channel_label(channel: u8) -> String {
    match channel {
        0 => "omni".to_string(),
        channel => format!("ch {channel}"),
    }
}

/// The learn that teaches a Pad the next note it hears, and its channel.
pub fn learn() -> ModuleLearn {
    ModuleLearn {
        kinds: LearnKinds { note: Some(0.0), ..LearnKinds::NONE },
        kind: None,
        number: Some(Pad::PARAM_NOTE),
        channel: Some(Pad::PARAM_CHANNEL),
        encoding: None,
    }
}

/// Draws the Pad's face, and returns what a press, Learn or its menu asked
/// for.
pub fn pads_face(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &mut SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut asked = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return asked };
    let name = node.user_data.display_name.clone();
    let width = display_frame::knob_row_width(ui, 4, z);
    let height = (TOP + PAD_HEIGHT + GAP) * z;
    let Some((rect, _, painter)) = display_frame::frame_of_height(ui, width, height, theme::signal::GATE, z) else { return asked };

    let listen = listen_of(graph, node_id);
    let engine_id = user_state.get_engine_node_id(node_id);
    let learning = engine_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, Pad::PARAM_NOTE));
    let blink = 0.4 + 0.6 * (ui.input(|i| i.time) * 4.0).sin().abs() as f32;
    let small = FontId::proportional(7.0 * z);

    // Along the top: the channel it hears, and Learn
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), TOP * z));
    painter.text(pos2(top.left() + 6.0 * z, top.center().y), Align2::LEFT_CENTER, channel_label(listen.channel), small.clone(), theme::text::QUIET);
    let learn_rect = Rect::from_min_max(pos2(top.right() - 40.0 * z, top.top()), top.right_bottom());
    let learn_button = ui.interact(learn_rect, ui.scope_id().with(("pad_learn", node_id)), Sense::click());
    // While its group learns in order, where in the walk it comes
    let mark = user_state.learn_walk.as_ref().and_then(|walk| walk.mark(node_id));
    let midi = theme::signal::MIDI;
    let (learn_text, learn_ink, learn_tip) = match mark {
        Some(Mark::Now) => ("skip".to_string(), midi.gamma_multiply(blink), "Its turn: hit a pad, or click to leave this one as it is (Tab)".to_string()),
        Some(Mark::Next(n)) => (n.to_string(), midi.gamma_multiply(0.55), format!("Learning in order: this one comes {}", ordinal(n))),
        // A tick, drawn below
        Some(Mark::Learned) => (String::new(), midi, "Learned in this walk".to_string()),
        Some(Mark::Skipped) => ("kept".to_string(), theme::text::QUIET, "Skipped: it keeps its note".to_string()),
        None if learning => ("cancel".to_string(), midi.gamma_multiply(blink), "Click to stop learning".to_string()),
        None => (
            "learn".to_string(),
            if learn_button.hovered() { midi } else { theme::text::QUIET },
            "Learn the pad: hit a pad on your controller, and this one takes its note and channel".to_string(),
        ),
    };
    painter.text(pos2(top.right() - 6.0 * z, top.center().y), Align2::RIGHT_CENTER, learn_text, small, learn_ink);
    if mark == Some(Mark::Learned) {
        let at = pos2(top.right() - 10.0 * z, top.center().y);
        let tick = [at + vec2(-3.0, 0.0) * z, at + vec2(-1.0, 2.0) * z, at + vec2(3.0, -2.5) * z];
        painter.add(egui::Shape::line(tick.to_vec(), Stroke::new(1.2 * z, learn_ink)));
    }
    if learn_button.on_hover_text(learn_tip).clicked() {
        match mark {
            Some(Mark::Now) => asked.push(SynthResponse::LearnWalk(WalkAction::Skip)),
            Some(_) => {}
            None if learning => asked.push(SynthResponse::MidiLearnCancel),
            None => asked.extend(engine_id.map(|eid| learn_start(eid, &name, listen))),
        }
    }

    let cell = Rect::from_min_max(pos2(rect.left() + GAP * z, top.bottom()), pos2(rect.right() - GAP * z, rect.bottom() - GAP * z));
    pad_control(ui, &painter, cell, node_id, graph, user_state, z, KnobPlace::Module, &mut asked);
    asked
}

/// One Pad's pad in `cell`, at `place`: lit from its readout, played by a
/// press (as hard as how high), learned in Map Mode and from its
/// right-click menu, which pins it to its group's face or takes it off.
/// On a face it wears the name it's been given there. What it asked for
/// goes on `asked`.
#[allow(clippy::too_many_arguments)]
pub(super) fn pad_control(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    cell: Rect,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &mut SynthGraphState,
    z: f32,
    place: KnobPlace,
    asked: &mut Vec<SynthResponse>,
) {
    let Some(node) = graph.nodes.get(node_id) else { return };
    let data = &node.user_data;
    let name = data.display_name.clone();
    let face_name = data.face_label(PAD_PRESS);
    let listen = listen_of(graph, node_id);
    let readout = display_frame::readout(user_state, node_id);
    let engine_id = user_state.get_engine_node_id(node_id);
    let learning = engine_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, Pad::PARAM_NOTE));
    let now = ui.input(|i| i.time);
    let blink = 0.4 + 0.6 * (now * 4.0).sin().abs() as f32;
    let tiny = FontId::proportional(6.0 * z);
    let id = ui.scope_id().with(("pad_pad", node_id));
    let response = ui.interact(cell, id, Sense::click_and_drag());

    // What it's doing, from the readout
    let value = |at: usize| readout.map_or(0.0, |r| r.values[at]);
    let open = value(Pad::READOUT_GATE) > 0.5;
    let quiet = readout.map_or(f32::MAX, |r| r.values[Pad::READOUT_QUIET]);
    let flash = value(Pad::READOUT_VELOCITY).powf(0.8) * (-quiet / FLASH).exp();
    if flash > 0.01 {
        crate::app::pace::animate(ui.ctx());
    }
    // Learned in a walk still going, its rim stays softly lit
    let walked = user_state.learn_walk.as_ref().and_then(|walk| walk.mark(node_id)) == Some(Mark::Learned);
    let rim = if learning { Some(blink) } else { walked.then_some(0.45) };
    let light = PadLight { flash, open, pressure: value(Pad::READOUT_PRESSURE), hovered: response.hovered(), learning: rim };
    draw_pad(painter, cell, z, light);

    // Its note, quietly
    let note_ink = if flash > 0.2 { Color32::from_rgb(20, 40, 28) } else { theme::text::DISABLED };
    painter.text(cell.left_bottom() + vec2(3.5 * z, -3.0 * z), Align2::LEFT_BOTTOM, heard_note(listen), tiny.clone(), note_ink);
    // Its name on a face, in its module's colour, darkening as it lights
    if let Some(face_name) = face_name {
        let ink = data.category.color().lerp_to_gamma(Color32::from_rgb(20, 40, 28), (flash * 1.5).min(1.0));
        let at = cell.left_top() + vec2(3.5 * z, 3.0 * z);
        let font = FontId::proportional(7.0 * z);
        let galley = painter.layout(face_name.to_string(), font, ink, cell.width() - 6.0 * z);
        painter.with_clip_rect(cell.shrink(1.0 * z)).galley(at, galley, ink);
    }
    if learning {
        painter.text(cell.center(), Align2::CENTER_CENTER, "hit a pad", tiny, theme::signal::MIDI.gamma_multiply(blink));
        crate::app::pace::animate(ui.ctx());
    }

    // In Map Mode a click learns the pad instead of playing it (#426)
    let learn = engine_id.map(|eid| learn_start(eid, &name, listen));
    let caught = learn.as_ref().and_then(|learn| super::map_mode::catch(ui, cell, super::map_mode::Outline::around(cell, z), learn, user_state));
    let in_map_mode = caught.is_some();
    let response = match caught {
        Some(caught) => {
            asked.extend(caught.learn);
            caught.response
        }
        None => response,
    };

    // Pressed on the face: down for at least MIN_PRESS_SECONDS, as hard as
    // how near its top
    let press = parameters()[Pad::PARAM_PRESS].0;
    let pressed_at: Option<f64> = ui.ctx().data(|d| d.get_temp(id));
    let holding = !in_map_mode && response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) || response.clicked();
    match pressed_at {
        None if holding => {
            let y = response.interact_pointer_pos().map_or(0.5, |at| ((cell.bottom() - at.y) / cell.height()).clamp(0.0, 1.0));
            let velocity = SOFTEST_PRESS + (1.0 - SOFTEST_PRESS) * y;
            ui.ctx().data_mut(|d| d.insert_temp(id, now));
            asked.push(SynthResponse::PlayParameter { node_id, param_name: press.to_string(), value: velocity });
        }
        Some(at) if !response.is_pointer_button_down_on() => {
            let left = MIN_PRESS_SECONDS - (now - at);
            if left <= 0.0 {
                ui.ctx().data_mut(|d| d.remove::<f64>(id));
                asked.push(SynthResponse::PlayParameter { node_id, param_name: press.to_string(), value: 0.0 });
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
        }
        _ => {}
    }

    let called = face_name.map_or_else(|| name.clone(), |face_name| format!("{face_name} ({name})"));
    let tip = format!(
        "{called} · {} on {}\nPress it to play it, harder toward the top\nRight-click to learn it from your controller",
        heard_in_full(listen),
        channel_label(listen.channel),
    );
    let response = response.on_hover_text(tip);
    if place.is_face() && response.secondary_clicked() {
        super::group_face::reset_name_field(ui.ctx(), super::group_face::face_name_id(node_id, PAD_PRESS));
    }
    let walking = user_state.learn_walk.as_ref().and_then(|walk| walk.mark(node_id)) == Some(Mark::Now);
    let menu = crate::widgets::menu::context_menu(&response, |ui| {
        // On a face, the name it goes by there first, ready to type over
        if place.is_face() {
            let named = face_name.map_or_else(|| heard_note(listen), str::to_string);
            asked.extend(data.face_name_item(ui, node_id, PAD_PRESS, &named));
        }
        pad_menu(ui, node_id, engine_id, &name, listen, learning, walking, asked);
        // Onto its group's face, or off it
        if data.parent.is_some() || place.is_face() {
            ui.separator();
            asked.extend(data.pin_items(ui, node_id, PAD_PRESS, place));
        }
    });
    if menu.is_some() {
        user_state.widget_context_menu_open = true;
    }
}

/// How a pad is lit. A Kit's pads (#443) are lit the same way.
pub(super) struct PadLight {
    /// A hit's flash, 0 to 1, fading.
    pub flash: f32,
    /// Whether its gate is open: held, or latched.
    pub open: bool,
    /// How hard it's pressed, 0 to 1.
    pub pressure: f32,
    pub hovered: bool,
    /// The blink while it learns, or a steady glow once a walk has learned it.
    pub learning: Option<f32>,
}

/// A pad: soft rubber, lit from within.
pub(super) fn draw_pad(painter: &egui::Painter, cell: Rect, z: f32, light: PadLight) {
    let green = theme::signal::GATE;
    let radius = 4.0 * z;
    let rubber = if light.hovered { Color32::from_rgb(40, 43, 56) } else { Color32::from_rgb(33, 35, 46) };
    painter.rect_filled(cell, radius, rubber);
    // A faint sheen along its top edge
    let sheen = Rect::from_min_size(cell.min + vec2(1.5 * z, 1.0 * z), vec2(cell.width() - 3.0 * z, 1.0 * z));
    painter.rect_filled(sheen, 0.5 * z, Color32::from_white_alpha(7));

    // Open, softly, and more so as it's pressed
    let held = if light.open { 0.16 + 0.24 * light.pressure } else { 0.0 };
    let lit = held.max(light.flash * 0.85);
    if lit > 0.005 {
        painter.rect_filled(cell, radius, green.gamma_multiply(lit));
        // A hard hit blooms from the middle, a soft glow wider the harder
        if light.flash > 0.05 {
            let reach = cell.size() * (0.30 + 0.25 * light.flash);
            bloom(painter, cell.center(), reach, Color32::from_rgb(214, 255, 226).gamma_multiply(0.55 * light.flash * light.flash));
        }
    }
    let rim = match light.learning {
        Some(blink) => Stroke::new(1.25 * z, theme::signal::MIDI.gamma_multiply(blink)),
        None if light.open || light.flash > 0.05 => Stroke::new(1.0 * z, green.gamma_multiply(0.45 + 0.55 * lit.min(1.0))),
        None => Stroke::new(1.0 * z, Color32::from_rgb(52, 55, 72)),
    };
    painter.rect_stroke(cell, radius, rim, egui::StrokeKind::Inside);
}

/// A soft oval of light, `color` at `center` fading to nothing `reach`
/// away (its half width and half height).
fn bloom(painter: &egui::Painter, center: egui::Pos2, reach: egui::Vec2, color: Color32) {
    const SIDES: usize = 32;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    for i in 0..SIDES {
        let angle = i as f32 / SIDES as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + vec2(angle.cos() * reach.x, angle.sin() * reach.y), Color32::TRANSPARENT);
        mesh.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % SIDES) as u32);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// "1st", "2nd", "3rd", "4th"...
pub(super) fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The response that starts a Pad's learn.
pub(super) fn learn_start(engine_node_id: u64, name: &str, listen: Listen) -> SynthResponse {
    SynthResponse::MidiLearnStart {
        engine_node_id,
        param_index: Pad::PARAM_NOTE,
        param_name: format!("{name} on {}", note_name(listen.note)),
        min_value: 0.0,
        max_value: 1.0,
        control: MidiControl::Button,
        scene: None,
        into: Some(Box::new(learn())),
    }
}

/// The pad's right-click menu: learn it, its note and its channel.
#[allow(clippy::too_many_arguments)]
fn pad_menu(ui: &mut egui::Ui, node_id: NodeId, engine_id: Option<u64>, name: &str, listen: Listen, learning: bool, walking: bool, asked: &mut Vec<SynthResponse>) {
    let set = |asked: &mut Vec<SynthResponse>, param: usize, value: f32| {
        let param_name = parameters()[param].0.to_string();
        asked.push(SynthResponse::ParameterChanged { node_id, param_name, value });
    };
    ui.label(egui::RichText::new(format!("{name} · {} · {}", heard_in_full(listen), channel_label(listen.channel))).small().weak());
    if walking {
        // Its turn in its group's walk (#481)
        if ui.button("Skip").on_hover_text("Leave this pad as it is, and go on to the next (Tab)").clicked() {
            asked.push(SynthResponse::LearnWalk(WalkAction::Skip));
            ui.close();
        }
        if ui.button("Stop learning in order").on_hover_text("Keep what's learned so far (Esc)").clicked() {
            asked.push(SynthResponse::LearnWalk(WalkAction::Stop));
            ui.close();
        }
    } else if learning {
        if ui.button("Cancel MIDI Learn").clicked() {
            asked.push(SynthResponse::MidiLearnCancel);
            ui.close();
        }
    } else if let Some(eid) = engine_id {
        if ui.button("Learn MIDI").on_hover_text("Hit a pad on your controller: this one takes its note and channel").clicked() {
            asked.push(learn_start(eid, name, listen));
            ui.close();
        }
    }
    ui.separator();

    let mut number = listen.note as u32;
    let mut any = listen.any;
    ui.horizontal(|ui| {
        ui.label("Note");
        if ui.add_enabled(!any, egui::DragValue::new(&mut number).range(0..=127)).changed() {
            set(asked, Pad::PARAM_NOTE, number as f32);
        }
        let tip = "Hear every note on its channel: a drum machine's track sends each trig's own pitch, so a pitched trig still strikes it";
        if ui.checkbox(&mut any, "Any note").on_hover_text(tip).changed() {
            set(asked, Pad::PARAM_ANY, if any { 1.0 } else { 0.0 });
        }
    });
    crate::widgets::menu::menu_button(ui, "Channel", |ui| {
        if ui.selectable_label(listen.channel == 0, "Omni").on_hover_text("Hear every channel").clicked() {
            set(asked, Pad::PARAM_CHANNEL, 0.0);
        }
        egui::Grid::new(("pad_channels", node_id)).spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            for channel in 1..=16u8 {
                if ui.selectable_label(listen.channel == channel, format!("{channel:>2}")).on_hover_text("The channel the pad hears").clicked() {
                    set(asked, Pad::PARAM_CHANNEL, channel as f32);
                }
                if channel % 4 == 0 {
                    ui.end_row();
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pad_learns_its_note_and_channel() {
        let learn = learn();
        assert_eq!(learn.number, Some(Pad::PARAM_NOTE));
        assert_eq!(learn.channel, Some(Pad::PARAM_CHANNEL));
        assert!(learn.kinds.note.is_some() && learn.kinds.cc.is_none() && learn.kinds.bend.is_none());
        assert_eq!(learn.prompt(), "Hit a pad or key");
        // Learn writes 1-16, as the module reads them
        let heard = learn.hear(&crate::engine::MidiEvent::NoteOn { channel: 9, note: 40, velocity: 99 }).unwrap();
        let mut params: Vec<f32> = parameters().iter().map(|&(_, default)| default).collect();
        for (param, value) in heard {
            params[param] = value;
        }
        assert_eq!(pad::listen(&params), Listen { note: 40, channel: 10, any: false });
    }

    #[test]
    fn notes_are_named_as_the_pianos_name_them() {
        assert_eq!(note_name(36), "C2");
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(42), "F#2");
        assert_eq!(parameters()[Pad::PARAM_PRESS].0, "Press");
        assert_eq!(parameters()[Pad::PARAM_ANY].0, "Any Note");
        assert_eq!(heard_note(Listen { note: 2, channel: 3, any: true }), "any");
        assert_eq!([1, 2, 3, 4, 11, 12, 13, 21, 22, 111].map(ordinal), ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "111th"]);
    }
}
