//! The Control module's face (#483): one slim fader, moving with the
//! hardware. Four Controls in a group are a controller's four faders, as
//! eight Pads are its pads.
//!
//! The track is in the Control orange, filled from the bottom, or from the
//! middle when its range is bipolar, with a cap where the control is (as
//! the cable carries it, slew and all). The cap warms when it hears its
//! control and cools again as it rests. Beside the track, what it hears
//! ("CC 74", "Bend", "Mod") and the value it's sending; along the top, its
//! channel and Learn.
//!
//! Learn takes whatever moves next on the controller: a fader, a knob, a
//! wheel or a press. Right-click the fader to learn it, or to choose what
//! it hears, its channel, its range and whether it's an endless encoder.
//! While it learns, it blinks in MIDI purple. While its group learns in
//! order (#481), the top says where in the walk it comes, as a Pad's does.
//!
//! [`fader_control`] is the fader alone, drawn in any cell, so a group's
//! face can show the Controls pinned to it: **Show on group**, in the
//! fader's menu, puts it there, wearing the name it's given on the face, as
//! a Pad's pad goes (#482).

use std::sync::OnceLock;

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use egui_node_graph2::NodeId;

use crate::app::module_learn::{LearnKinds, ModuleLearn};
use crate::app::theme;
use crate::dsp::DspModule;
use crate::modules::control::{self, Listen, Range, Source, ENCODING_LABELS};
use crate::modules::Control;
use crate::persistence::{MidiControl, MidiEncoding};

use super::display_frame;
use super::groups::CONTROL_FADER;
use super::learn_walk::Mark;
use super::node_data::KnobPlace;
use super::{SynthGraph, SynthGraphState, SynthResponse, WalkAction};

/// How many knobs wide the face is.
const KNOBS_WIDE: usize = 2;
/// The face's height, before zoom.
const HEIGHT: f32 = 92.0;
/// The strip along the top, before zoom.
const TOP: f32 = 15.0;
/// Room above and below the track, before zoom.
const MARGIN: f32 = 9.0;
/// Where the track sits, from the face's left, before zoom.
const TRACK_X: f32 = 20.0;
/// A track's width, before zoom.
const TRACK: f32 = 3.0;
/// A cap's size, before zoom.
const CAP: egui::Vec2 = vec2(16.0, 5.0);
/// How long a cap stays warm after its control moves, in seconds.
const WARM: f32 = 0.6;

/// The Control's parameter names and defaults, in its order.
fn parameters() -> &'static [(&'static str, f32)] {
    static NAMES: OnceLock<Vec<(&'static str, f32)>> = OnceLock::new();
    NAMES.get_or_init(|| Control::new().parameters().iter().map(|p| (p.name, p.default)).collect())
}

/// What the Control `node_id` hears, as its node holds it.
pub fn listen_of(graph: &SynthGraph, node_id: NodeId) -> Listen {
    let params: Vec<f32> = parameters().iter().map(|&(name, default)| display_frame::value_of(graph, node_id, name).unwrap_or(default)).collect();
    Listen::of(&params)
}

/// Its channel, as the face shows it.
fn channel_label(channel: u8) -> String {
    match channel {
        0 => "omni".to_string(),
        channel => format!("ch {channel}"),
    }
}

/// What it hears, in words, for its tooltip.
fn describe(listen: &Listen) -> String {
    let what = match listen.source {
        Source::Cc if listen.cc == 1 => "The mod wheel (CC 1)".to_string(),
        Source::Cc => format!("CC {}", listen.cc),
        Source::Bend => "The pitch bend".to_string(),
        Source::ModWheel => "The mod wheel (CC 1)".to_string(),
        Source::Pressure => "Channel pressure".to_string(),
    };
    let channel = match listen.channel {
        0 => "on any channel".to_string(),
        channel => format!("on channel {channel}"),
    };
    let encoder = match (listen.source, listen.encoding) {
        (Source::Cc, MidiEncoding::Relative64 | MidiEncoding::TwosComplement) => ", an endless encoder",
        _ => "",
    };
    format!("{what} {channel}{encoder}, as {}", range_label(listen))
}

fn range_label(listen: &Listen) -> &'static str {
    if listen.bipolar() { "-1 to 1" } else { "0 to 1" }
}

/// The learn that teaches a Control the next control it hears: a CC, the
/// pitch bend or the pressure, on its channel.
pub fn learn() -> ModuleLearn {
    ModuleLearn {
        kinds: LearnKinds {
            cc: Some(Source::Cc as u8 as f32),
            bend: Some(Source::Bend as u8 as f32),
            pressure: Some(Source::Pressure as u8 as f32),
            ..LearnKinds::NONE
        },
        kind: Some(Control::PARAM_SOURCE),
        number: Some(Control::PARAM_CC),
        channel: Some(Control::PARAM_CHANNEL),
        encoding: Some(Control::PARAM_ENCODING),
    }
}

/// The response that starts a Control's learn.
pub(super) fn learn_start(engine_node_id: u64, name: &str) -> SynthResponse {
    SynthResponse::MidiLearnStart {
        engine_node_id,
        param_index: Control::PARAM_SOURCE,
        param_name: name.to_string(),
        min_value: 0.0,
        max_value: 1.0,
        control: MidiControl::Knob,
        scene: None,
        into: Some(Box::new(learn())),
    }
}

/// Draws the fader, and returns what Learn or its menu asked for.
pub fn controls_face(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &mut SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut asked = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return asked };
    let name = node.user_data.display_name.clone();
    let accent = theme::signal::CONTROL;
    let width = display_frame::knob_row_width(ui, KNOBS_WIDE, z);
    let Some((rect, _, painter)) = display_frame::frame_of_height(ui, width, HEIGHT * z, accent, z) else { return asked };

    let listen = listen_of(graph, node_id);
    let readout = display_frame::readout(user_state, node_id);
    let engine_id = user_state.get_engine_node_id(node_id);
    let learning = engine_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, Control::PARAM_SOURCE));
    let now = ui.input(|i| i.time);
    let blink = 0.4 + 0.6 * (now * 4.0).sin().abs() as f32;
    let midi = theme::signal::MIDI;
    let small = FontId::proportional(7.0 * z);

    // Along the top: the channel it hears, and Learn, or where it comes in
    // its group's walk
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), TOP * z));
    painter.text(pos2(top.left() + 6.0 * z, top.center().y), Align2::LEFT_CENTER, channel_label(listen.channel), small.clone(), theme::text::QUIET);
    let learn_rect = Rect::from_min_max(pos2(top.right() - 40.0 * z, top.top()), top.right_bottom());
    let learn_button = ui.interact(learn_rect, ui.scope_id().with(("control_learn", node_id)), Sense::click());
    let mark = user_state.learn_walk.as_ref().and_then(|walk| walk.mark(node_id));
    let (learn_text, learn_ink, learn_tip) = match mark {
        Some(Mark::Now) => ("skip".to_string(), midi.gamma_multiply(blink), "Its turn: move a control, or click to leave this one as it is (Tab)".to_string()),
        Some(Mark::Next(n)) => (n.to_string(), midi.gamma_multiply(0.55), format!("Learning in order: this one comes {}", super::pads_face::ordinal(n))),
        // A tick, drawn below
        Some(Mark::Learned) => (String::new(), midi, "Learned in this walk".to_string()),
        Some(Mark::Skipped) => ("kept".to_string(), theme::text::QUIET, "Skipped: it keeps what it hears".to_string()),
        None if learning => ("cancel".to_string(), midi.gamma_multiply(blink), "Click to stop learning".to_string()),
        None => (
            "learn".to_string(),
            if learn_button.hovered() { midi } else { theme::text::QUIET },
            "Learn it: move a fader or knob, bend, or press a key on your controller, and this one hears it".to_string(),
        ),
    };
    painter.text(pos2(top.right() - 6.0 * z, top.center().y), Align2::RIGHT_CENTER, learn_text, small.clone(), learn_ink);
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
            None => asked.extend(engine_id.map(|eid| learn_start(eid, &name))),
        }
    }

    // The body: the fader, and beside it what it hears and sends
    let body = Rect::from_min_max(pos2(rect.left(), top.bottom()), rect.max);
    let response = ui.interact(body, ui.scope_id().with(("control_fader", node_id)), Sense::click());
    let walked = mark == Some(Mark::Learned);
    if response.hovered() || learning {
        painter.rect_filled(body.shrink(1.5 * z), 3.0 * z, Color32::from_white_alpha(6));
    }
    if learning || walked {
        let ink = midi.gamma_multiply(if learning { blink } else { 0.45 });
        painter.rect_stroke(body.shrink(1.5 * z), 3.0 * z, Stroke::new(1.0 * z, ink), egui::StrokeKind::Inside);
    }

    // Where it is, and how lately it moved
    let travel = readout.map_or(listen.source.rest(), |r| r.values[Control::READOUT_TRAVEL]).clamp(0.0, 1.0);
    let quiet = readout.map_or(f32::MAX, |r| r.values[Control::READOUT_QUIET]);
    let warmth = (-quiet / WARM).exp();
    if warmth > 0.01 {
        crate::app::pace::animate(ui.ctx());
    }
    let bipolar = listen.bipolar();
    let x = body.left() + TRACK_X * z;
    draw_fader(&painter, x, body.top() + MARGIN * z, body.bottom() - MARGIN * z, travel, warmth, bipolar, z);

    // What it hears, the value it sends, and its range; asking for a move
    // while it learns
    let text_x = body.left() + (TRACK_X + 18.0) * z;
    let value = if bipolar { travel * 2.0 - 1.0 } else { travel };
    let patched = display_frame::output_patched(graph, user_state, node_id, "Out");
    if learning {
        painter.text(pos2(text_x, body.top() + 10.0 * z), Align2::LEFT_TOP, "learn", FontId::proportional(10.0 * z), midi.gamma_multiply(blink));
        painter.text(pos2(text_x, body.top() + 25.0 * z), Align2::LEFT_TOP, "move it", small.clone(), theme::text::QUIET.gamma_multiply(blink));
        crate::app::pace::animate(ui.ctx());
    } else {
        painter.text(pos2(text_x, body.top() + 10.0 * z), Align2::LEFT_TOP, listen.label(), FontId::proportional(10.0 * z), theme::text::SECONDARY);
        let value_ink = if patched { accent.gamma_multiply(0.6 + 0.4 * warmth) } else { theme::text::QUIET };
        painter.text(pos2(text_x, body.top() + 25.0 * z), Align2::LEFT_TOP, format!("{value:+.2}"), FontId::monospace(8.0 * z), value_ink);
    }
    painter.text(pos2(text_x, body.bottom() - MARGIN * z), Align2::LEFT_BOTTOM, range_label(&listen), FontId::proportional(6.5 * z), theme::text::DISABLED);

    // In Map Mode a click learns it, as a pad's does (#426)
    let learn = engine_id.map(|eid| learn_start(eid, &name));
    let caught = learn.as_ref().and_then(|learn| super::map_mode::catch(ui, body, super::map_mode::Outline::around(body, z), learn, user_state));
    let response = match caught {
        Some(caught) => {
            asked.extend(caught.learn);
            caught.response
        }
        None => response,
    };

    let tip = format!("{name}: {}\nNow {value:.2}\nRight-click to learn it from your controller, or choose what it hears", describe(&listen));
    let response = response.on_hover_text(tip);
    let walking = mark == Some(Mark::Now);
    let data = &node.user_data;
    let menu = crate::widgets::menu::context_menu(&response, |ui| {
        control_menu(ui, node_id, engine_id, &name, &listen, learning, walking, &mut asked);
        // Onto its group's face, or off it
        if data.parent.is_some() {
            ui.separator();
            asked.extend(data.pin_items(ui, node_id, CONTROL_FADER, KnobPlace::Module));
        }
    });
    if menu.is_some() {
        user_state.widget_context_menu_open = true;
    }
    asked
}

/// One Control's fader in `cell`, at `place` (a group's face): moving with
/// its readout, under what it's called there, learned in Map Mode and from
/// its right-click menu, which also takes it off the face. What it asked
/// for goes on `asked`.
#[allow(clippy::too_many_arguments)]
pub(super) fn fader_control(
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
    let listen = listen_of(graph, node_id);
    let face_name = data.face_label(CONTROL_FADER).filter(|_| place.is_face());
    let readout = display_frame::readout(user_state, node_id);
    let engine_id = user_state.get_engine_node_id(node_id);
    let learning = engine_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, Control::PARAM_SOURCE));
    let mark = user_state.learn_walk.as_ref().and_then(|walk| walk.mark(node_id));
    let blink = 0.4 + 0.6 * (ui.input(|i| i.time) * 4.0).sin().abs() as f32;
    let response = ui.interact(cell, ui.scope_id().with(("control_face_fader", node_id)), Sense::click());

    // A slot of its own, lit purple while it learns or once a walk has
    // learned it
    let slot = if response.hovered() { Color32::from_rgb(33, 35, 46) } else { Color32::from_rgb(26, 28, 37) };
    painter.rect_filled(cell, 3.0 * z, slot);
    let rim = match (learning, mark) {
        (true, _) | (_, Some(Mark::Now)) => Some(blink),
        (_, Some(Mark::Learned)) => Some(0.45),
        _ => None,
    };
    if let Some(alpha) = rim {
        painter.rect_stroke(cell, 3.0 * z, Stroke::new(1.0 * z, theme::signal::MIDI.gamma_multiply(alpha)), egui::StrokeKind::Inside);
        crate::app::pace::animate(ui.ctx());
    }

    let travel = readout.map_or(listen.source.rest(), |r| r.values[Control::READOUT_TRAVEL]).clamp(0.0, 1.0);
    let quiet = readout.map_or(f32::MAX, |r| r.values[Control::READOUT_QUIET]);
    let warmth = (-quiet / WARM).exp();
    if warmth > 0.01 {
        crate::app::pace::animate(ui.ctx());
    }
    let bipolar = listen.bipolar();
    let label_room = 13.0 * z;
    draw_fader(painter, cell.center().x + 2.0 * z, cell.top() + 7.0 * z, cell.bottom() - label_room, travel, warmth, bipolar, z);

    // Its name along its foot: what the face calls it, in its module's
    // colour, or what it hears
    let (label, ink) = match face_name {
        Some(face_name) => (face_name.to_string(), data.category.color()),
        None => (listen.label(), theme::text::SECONDARY),
    };
    let ink = if learning { theme::signal::MIDI.gamma_multiply(blink) } else { ink };
    let foot = pos2(cell.center().x, cell.bottom() - label_room * 0.5);
    painter.with_clip_rect(cell.shrink(1.0 * z)).text(foot, Align2::CENTER_CENTER, label, FontId::proportional(6.5 * z), ink);

    // In Map Mode a click learns it
    let learn = engine_id.map(|eid| learn_start(eid, &name));
    let caught = learn.as_ref().and_then(|learn| super::map_mode::catch(ui, cell, super::map_mode::Outline::around(cell, z), learn, user_state));
    let response = match caught {
        Some(caught) => {
            asked.extend(caught.learn);
            caught.response
        }
        None => response,
    };

    let value = if bipolar { travel * 2.0 - 1.0 } else { travel };
    let called = face_name.map_or_else(|| name.clone(), |face_name| format!("{face_name} ({name})"));
    let tip = format!("{called}: {}\nNow {value:.2}\nRight-click to learn it from your controller, or choose what it hears", describe(&listen));
    let response = response.on_hover_text(tip);
    if place.is_face() && response.secondary_clicked() {
        super::group_face::reset_name_field(ui.ctx(), super::group_face::face_name_id(node_id, CONTROL_FADER));
    }
    let walking = mark == Some(Mark::Now);
    let menu = crate::widgets::menu::context_menu(&response, |ui| {
        // On a face, the name it goes by there first, ready to type over
        if place.is_face() {
            let named = face_name.map_or_else(|| listen.label(), str::to_string);
            asked.extend(data.face_name_item(ui, node_id, CONTROL_FADER, &named));
        }
        control_menu(ui, node_id, engine_id, &name, &listen, learning, walking, asked);
        ui.separator();
        asked.extend(data.pin_items(ui, node_id, CONTROL_FADER, place));
    });
    if menu.is_some() {
        user_state.widget_context_menu_open = true;
    }
}

/// A fader at `x`, its track from `top` to `bottom`: a tick every quarter
/// (a longer one at the middle when it's bipolar), filled to `travel`, and
/// its cap, warm while the control moves.
#[allow(clippy::too_many_arguments)]
fn draw_fader(painter: &egui::Painter, x: f32, top: f32, bottom: f32, travel: f32, warmth: f32, bipolar: bool, z: f32) {
    let accent = theme::signal::CONTROL;
    let track_ink = Color32::from_rgb(38, 40, 54);
    let tick_ink = Color32::from_rgb(52, 55, 72);
    let track = Rect::from_min_max(pos2(x - TRACK * 0.5 * z, top), pos2(x + TRACK * 0.5 * z, bottom));
    let y_of = |t: f32| track.bottom() - t * track.height();
    for quarter in 0..=4 {
        let y = y_of(quarter as f32 / 4.0);
        let reach = if quarter == 2 && bipolar { 6.0 } else { 3.0 };
        painter.line_segment([pos2(track.left() - (2.0 + reach) * z, y), pos2(track.left() - 2.0 * z, y)], Stroke::new(0.75 * z, tick_ink));
    }
    painter.rect_filled(track, TRACK * 0.5 * z, track_ink);
    let (from, to) = if bipolar { (y_of(0.5), y_of(travel)) } else { (track.bottom(), y_of(travel)) };
    let fill = Rect::from_x_y_ranges(track.x_range(), from.min(to)..=from.max(to));
    if fill.height() > 0.25 * z {
        painter.rect_filled(fill.expand2(vec2(2.5 * z, 0.0)), 3.0 * z, accent.gamma_multiply(0.06 + 0.14 * warmth));
        painter.rect_filled(fill, TRACK * 0.5 * z, accent.gamma_multiply(0.55 + 0.35 * warmth));
    }

    let cap = Rect::from_center_size(pos2(x, y_of(travel)), CAP * z);
    if warmth > 0.01 {
        painter.rect_filled(cap.expand2(vec2(4.0 * z, 3.5 * z)), 5.0 * z, accent.gamma_multiply(0.20 * warmth));
    }
    painter.rect_filled(cap.expand(0.75 * z), 2.5 * z, Color32::from_rgb(14, 15, 21));
    let cap_ink = lerp(Color32::from_rgb(178, 180, 192), Color32::from_rgb(255, 226, 190), warmth);
    painter.rect_filled(cap, 2.0 * z, cap_ink);
    painter.line_segment([pos2(cap.left() + 3.0 * z, cap.center().y), pos2(cap.right() - 3.0 * z, cap.center().y)], Stroke::new(0.75 * z, Color32::from_rgb(90, 92, 104)));
}

/// The fader's right-click menu: Learn (or, on its turn in a walk, Skip
/// and Stop), then what it hears, its channel, its range and its hardware.
#[allow(clippy::too_many_arguments)]
fn control_menu(ui: &mut egui::Ui, node_id: NodeId, engine_id: Option<u64>, name: &str, listen: &Listen, learning: bool, walking: bool, asked: &mut Vec<SynthResponse>) {
    let set = |asked: &mut Vec<SynthResponse>, param: usize, value: f32| {
        let param_name = parameters()[param].0.to_string();
        asked.push(SynthResponse::ParameterChanged { node_id, param_name, value });
    };
    ui.label(egui::RichText::new(format!("{name} · {} · {}", listen.label(), channel_label(listen.channel))).small().weak());
    if walking {
        // Its turn in its group's walk (#481)
        if ui.button("Skip").on_hover_text("Leave this one as it is, and go on to the next (Tab)").clicked() {
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
    } else if let Some(engine_node_id) = engine_id {
        let tip = "Move a fader or knob, bend, or press a key down, on your MIDI controller";
        if ui.button("Learn MIDI").on_hover_text(tip).clicked() {
            asked.push(learn_start(engine_node_id, name));
            ui.close();
        }
    }
    ui.separator();

    crate::widgets::menu::menu_button(ui, "Source", |ui| {
        for (index, label) in Source::LABELS.iter().enumerate() {
            let source = Source::from_param(index as f32);
            let chosen = listen.source == source || (source == Source::ModWheel && listen.source == Source::Cc && listen.cc == 1);
            if ui.selectable_label(chosen, *label).clicked() {
                set(asked, Control::PARAM_SOURCE, index as f32);
            }
        }
        if listen.source == Source::Cc {
            ui.separator();
            let mut cc = listen.cc as u32;
            ui.horizontal(|ui| {
                ui.label("CC number");
                if ui.add(egui::DragValue::new(&mut cc).range(0..=127)).changed() {
                    set(asked, Control::PARAM_CC, cc as f32);
                }
            });
        }
    });
    crate::widgets::menu::menu_button(ui, "Channel", |ui| {
        if ui.selectable_label(listen.channel == 0, "Omni").on_hover_text("Hear every channel").clicked() {
            set(asked, Control::PARAM_CHANNEL, 0.0);
        }
        egui::Grid::new(("control_channels", node_id)).spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            for channel in 1..=16u8 {
                if ui.selectable_label(listen.channel == channel, format!("{channel:>2}")).clicked() {
                    set(asked, Control::PARAM_CHANNEL, channel as f32);
                }
                if channel % 4 == 0 {
                    ui.end_row();
                }
            }
        });
    });
    crate::widgets::menu::menu_button(ui, "Range", |ui| {
        let tips = ["-1 to 1 for the pitch bend, which rests in the middle; 0 to 1 for the rest", "The bottom is 0, the top 1", "The middle is 0"];
        for (index, label) in Range::LABELS.iter().enumerate() {
            if ui.selectable_label(listen.range == Range::from_param(index as f32), *label).on_hover_text(tips[index]).clicked() {
                set(asked, Control::PARAM_RANGE, index as f32);
            }
        }
    });
    if listen.source == Source::Cc {
        crate::widgets::menu::menu_button(ui, "Hardware", |ui| {
            let tips = [
                "It has ends, and sends where it is",
                "Endless: sends 65 for a step up, 63 for a step down",
                "Endless: sends 1 for a step up, 127 for a step down",
            ];
            for (index, label) in ENCODING_LABELS.iter().enumerate() {
                let encoding = control::encoding_from_param(index as f32);
                if ui.selectable_label(listen.encoding == encoding, *label).on_hover_text(tips[index]).clicked() {
                    set(asked, Control::PARAM_ENCODING, index as f32);
                }
            }
        });
    }
}

/// `a` toward `b` by `t` (0 to 1).
fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_learns_into_its_own_params() {
        let learn = learn();
        assert_eq!(learn.kind, Some(Control::PARAM_SOURCE));
        assert_eq!(learn.number, Some(Control::PARAM_CC));
        assert_eq!(learn.channel, Some(Control::PARAM_CHANNEL));
        assert_eq!(learn.encoding, Some(Control::PARAM_ENCODING));
        assert_eq!(learn.kinds.note, None, "a note isn't a control");
        // What it writes is what the module reads
        let heard = learn.hear(&crate::engine::MidiEvent::ControlChange { channel: 2, controller: 74, value: 9 }).unwrap();
        let mut params: Vec<f32> = parameters().iter().map(|&(_, default)| default).collect();
        for (param, value) in heard {
            params[param] = value;
        }
        let listen = Listen::of(&params);
        assert_eq!((listen.source, listen.cc, listen.channel, listen.encoding), (Source::Cc, 74, 3, MidiEncoding::Absolute));
        assert_eq!(Source::from_param(learn.kinds.bend.unwrap()), Source::Bend);
        assert_eq!(Source::from_param(learn.kinds.pressure.unwrap()), Source::Pressure);
    }
}
