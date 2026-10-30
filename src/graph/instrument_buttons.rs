//! An Instrument's buttons on its face (#474), under its knobs: one row
//! each for the buttons its profile has (a looper's Rec/Play, a synth's
//! Hold), with its Gate jack on the node's edge beside it. A gate presses
//! a momentary button while it's high and flips a toggle each time it
//! rises; a click presses it by hand. Each lights while it's on, as a
//! Pads pad does.
//!
//! The button rows make no rows among the jacks: their jacks sit beside
//! them (see [`places`]), and a button that holds nothing has none. A
//! device that's all buttons and no knobs (the RC-505) shows its buttons
//! in place of the knobs, and its empty knob jacks go too.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Id, Rect, Sense};
use egui_node_graph2::{AnyParameterId, NodeId, NodeResponse};

use crate::app::theme;
use crate::hardware::{self, instrument, Button, ButtonMode};
use crate::modules::instrument::{self as module, Instrument, BUTTONS, BUTTON_NAMES, PRESS_NAMES, SLOTS, SLOT_NAMES};

use super::display_frame;
use super::node_data::SynthNodeData;
use super::pads_face::{draw_pad, PadLight};
use super::sampler_take::MIN_PRESS_SECONDS;
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// A button's row (patch units).
pub const ROW: f32 = 20.0;
/// The gap between a button and the next.
const GAP: f32 = 3.0;
/// How long a press's flash takes to fade, in seconds: its time constant.
const FLASH: f64 = 0.15;

/// The buttons node `node_id` holds, each with its jack's input.
fn held(graph: &SynthGraph, node_id: NodeId) -> [Option<Button>; BUTTONS] {
    let Some(node) = graph.nodes.get(node_id) else { return Default::default() };
    let profiles = hardware::known();
    std::array::from_fn(|index| instrument::button(&node.user_data.labels, index, &profiles))
}

/// Whether an Instrument shows no knobs: none holds a parameter or has a
/// cable, and it has buttons to show in their place.
pub fn knobless(graph: &SynthGraph, node_id: NodeId) -> bool {
    let Some(node) = graph.nodes.get(node_id) else { return false };
    let data = &node.user_data;
    if data.module_id != module::ID || held(graph, node_id).iter().all(Option::is_none) {
        return false;
    }
    let profiles = hardware::known();
    (0..SLOTS).all(|slot| {
        let patched = node.get_input(SLOT_NAMES[slot]).is_ok_and(|input| super::patched::input(graph, input));
        instrument::held(&data.labels, slot, &profiles).is_none() && !patched
    })
}

/// Whether the Instrument lays out jack `param` itself: every button's,
/// and a knobless one's knob jacks, which it hides.
pub fn places(node_id: NodeId, graph: &SynthGraph, param: AnyParameterId) -> bool {
    let AnyParameterId::Input(input) = param else { return false };
    let Some(node) = graph.nodes.get(node_id).filter(|node| node.user_data.module_id == module::ID) else { return false };
    let Some((name, _)) = node.inputs.iter().find(|(_, id)| *id == input) else { return false };
    BUTTON_NAMES.contains(&name.as_str()) || (SLOT_NAMES.contains(&name.as_str()) && knobless(graph, node_id))
}

/// What a button's jack is called: the button it presses, so a gate is
/// patched into Rec/Play, not Button 1.
pub fn jack_name(data: &SynthNodeData, param_name: &str) -> Option<String> {
    let index = BUTTON_NAMES.iter().position(|&name| name == param_name)?;
    instrument::button(&data.labels, index, &hardware::known()).map(|button| button.name)
}

/// Hides a knobless Instrument's knob jacks. Called each frame its face is.
pub fn hide_knob_jacks(ui: &egui::Ui, node_id: NodeId, graph: &SynthGraph) {
    let Some(node) = graph.nodes.get(node_id) else { return };
    for name in SLOT_NAMES {
        if let Ok(input) = node.get_input(name) {
            egui_node_graph2::hide_port(ui, node_id, AnyParameterId::Input(input));
        }
    }
}

/// How many rows: up to the last button that holds one or has a cable.
fn rows(graph: &SynthGraph, node_id: NodeId) -> usize {
    let Some(node) = graph.nodes.get(node_id) else { return 0 };
    let held = held(graph, node_id);
    (0..BUTTONS)
        .rev()
        .find(|&index| held[index].is_some() || node.get_input(BUTTON_NAMES[index]).is_ok_and(|input| super::patched::input(graph, input)))
        .map_or(0, |last| last + 1)
}

/// Draws the buttons in `width`, places their jacks beside them, and
/// returns what a press or a menu asked for. Called each frame the face is,
/// so every button's jack is placed or hidden.
pub fn buttons(data: &SynthNodeData, ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, width: f32, z: f32) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    let mut responses = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return responses };
    let rows = rows(graph, node_id);
    let held = held(graph, node_id);
    if rows > 0 {
        ui.add_space(6.0 * z);
    }
    let (strip, _) = ui.allocate_exact_size(vec2(width, rows as f32 * ROW * z), Sense::hover());
    let readout = display_frame::readout(user_state, node_id);
    let lit = readout.as_ref().map_or(0, |r| r.values[Instrument::READOUT_BUTTONS] as u32);
    let now = ui.input(|i| i.time);
    let painter = ui.painter_at(strip.expand(2.0 * z));
    let font = FontId::proportional(9.0 * z);
    let small = FontId::proportional(7.0 * z);
    for index in 0..BUTTONS {
        let Ok(input) = node.get_input(BUTTON_NAMES[index]) else { continue };
        let jack = AnyParameterId::Input(input);
        let row = Rect::from_min_size(pos2(strip.left(), strip.top() + index as f32 * ROW * z), vec2(width, ROW * z));
        if index >= rows || (held[index].is_none() && !super::patched::input(graph, input)) {
            egui_node_graph2::hide_port(ui, node_id, jack);
            continue;
        }
        egui_node_graph2::place_port(ui, node_id, jack, row.center().y);
        let rect = Rect::from_min_max(row.min + vec2(4.0 * z, GAP * z / 2.0), row.max - vec2(4.0 * z, GAP * z / 2.0));
        let id = Id::unique(("instrument_button", node_id, index));
        let response = ui.interact(rect, id, Sense::click());
        super::marks::mark(ui, node_id, format_args!("button:{}", index + 1), rect);

        // On: lit, with a flash as it turns on that outlasts a trigger
        let on = lit & (1 << index) != 0;
        let since: (bool, f64) = ui.ctx().data(|d| d.get_temp(id.with("lit"))).unwrap_or((false, f64::MIN));
        let turned_on = if on && !since.0 { now } else { since.1 };
        if on != since.0 {
            ui.ctx().data_mut(|d| d.insert_temp(id.with("lit"), (on, turned_on)));
        }
        let flash = (-(now - turned_on) / FLASH).exp() as f32;
        if flash > 0.01 {
            crate::app::pace::animate(ui.ctx());
        }
        draw_pad(&painter, rect, z, PadLight { flash, open: on, pressure: 1.0, hovered: response.hovered(), learning: None });

        let (name, sends, ink) = match &held[index] {
            Some(button) => (button.name.clone(), detail(button), if flash > 0.3 { Color32::from_rgb(20, 40, 28) } else { theme::text::PRIMARY }),
            None => ("—".to_string(), String::new(), theme::text::DISABLED),
        };
        let clip = painter.with_clip_rect(rect.shrink(1.0 * z).intersect(painter.clip_rect()));
        clip.text(pos2(rect.left() + 5.0 * z, rect.center().y), Align2::LEFT_CENTER, name, font.clone(), ink);
        clip.text(pos2(rect.right() - 5.0 * z, rect.center().y), Align2::RIGHT_CENTER, sends, small.clone(), theme::text::SECONDARY);

        // Pressed by hand: down for at least MIN_PRESS_SECONDS
        let press = PRESS_NAMES[index];
        let pressed_at: Option<f64> = ui.ctx().data(|d| d.get_temp(id));
        let holding = held[index].is_some() && (response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) || response.clicked());
        match pressed_at {
            None if holding => {
                ui.ctx().data_mut(|d| d.insert_temp(id, now));
                responses.push(NodeResponse::User(SynthResponse::PlayParameter { node_id, param_name: press.to_string(), value: 1.0 }));
            }
            Some(at) if !response.is_pointer_button_down_on() => {
                let left = MIN_PRESS_SECONDS - (now - at);
                if left <= 0.0 {
                    ui.ctx().data_mut(|d| d.remove::<f64>(id));
                    responses.push(NodeResponse::User(SynthResponse::PlayParameter { node_id, param_name: press.to_string(), value: 0.0 }));
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(left));
                }
            }
            _ => {}
        }

        let tip = match &held[index] {
            Some(button) if button.mode == ButtonMode::Toggle => format!("{} · {}\nEach rise of its gate flips it, as a click does. It lets go when the Clock stops\nRight-click to choose another", button.name, detail(button)),
            Some(button) => format!("{} · {}\nHeld while its gate is high, or while you hold it down. It lets go when the Clock stops\nRight-click to choose another", button.name, detail(button)),
            None => "This button presses nothing: right-click to choose one".to_string(),
        };
        let response = response.on_hover_text(tip);
        response.context_menu(|ui| {
            responses.extend(menu(ui, data, node_id, index, held[index].as_ref()).into_iter().map(NodeResponse::User));
        });
    }
    responses
}

/// What a button sends, as its row's right end says it: "CC 14", "CC 65 ·
/// toggle", "CC 64 ch 3".
fn detail(button: &Button) -> String {
    let channel = button.channel.map(|c| format!(" ch {c}")).unwrap_or_default();
    let toggle = if button.mode == ButtonMode::Toggle { " · toggle" } else { "" };
    format!("CC {}{channel}{toggle}", button.cc)
}

/// A button's right-click menu: which of the profile's buttons it presses.
fn menu(ui: &mut egui::Ui, data: &SynthNodeData, node_id: NodeId, index: usize, current: Option<&Button>) -> Vec<SynthResponse> {
    let mut asked = Vec::new();
    let profiles = hardware::known();
    let heading = match current {
        Some(button) => format!("{} · {}", button.name, detail(button)),
        None => format!("{} · empty", BUTTON_NAMES[index]),
    };
    ui.label(egui::RichText::new(heading).small().weak());
    let Some(known) = instrument::profile_name(&data.labels).and_then(|name| profiles.find(name)) else {
        ui.label(egui::RichText::new("Choose an instrument on the face to give this button one of its own").small().weak());
        return asked;
    };
    let mut give = |button: Option<&Button>| {
        let (label, text) = match button {
            Some(button) => (format!("Button to {}", button.name), instrument::written_button(button)),
            None => ("Empty button".to_string(), String::new()),
        };
        asked.push(SynthResponse::SetLabels { node_id, label, labels: vec![(instrument::button_key(index), text)] });
    };
    crate::widgets::menu::menu_button(ui, "Presses", |ui| {
        ui.set_min_width(150.0);
        for button in &known.profile.buttons {
            if ui.selectable_label(current.is_some_and(|c| c.name == button.name), &button.name).on_hover_text(detail(button)).clicked() {
                give(Some(button));
                ui.close();
            }
        }
        ui.separator();
        if ui.selectable_label(current.is_none(), "Nothing").clicked() {
            give(None);
            ui.close();
        }
    });
    asked
}
