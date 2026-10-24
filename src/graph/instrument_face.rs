//! The Instrument's face (#442): the hardware it plays, and its knobs in
//! the hardware's own words.
//!
//! - **Along the top**, a lamp, the port and its device, as MIDI Out's
//!   face has them: green while messages go out, amber when the port has
//!   no device or it's unplugged, red when it won't open. Click the device
//!   to choose another. At the right, the channel.
//! - **Beneath**, the profile's name. Click it to choose another: every
//!   knob takes one of its parameters, starting from the chart's default,
//!   and is sent. **Send all** at the right sends every knob again, so the
//!   hardware comes back to the patch.
//! - **Twelve knobs**, four to a row, each named for the parameter it
//!   holds, coloured by its group (an oscillator's blue, a filter's teal),
//!   and reading out what it sends as the device names it: "Saw", "+12",
//!   "74". Right-click a knob to give it another parameter.
//!
//! A Kit (#443) wears the same face, with its lanes' pads between the
//! header and the knobs (see [`super::kit_face`]), and lists only kits.

use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, UiBuilder, Vec2};
use egui_node_graph2::{NodeId, NodeResponse};

use crate::app::theme;
use crate::hardware::{self, instrument, Known, Origin, Parameter, Profile, SlotMap};
use crate::link::midi::{PortStatus, MIDI, PORTS, PORT_LABELS};
use crate::modules::instrument::{Instrument, SLOTS, SLOT_NAMES};
use crate::modules::kit::{self, Kit, LANES};

use super::display_frame;
use super::midi_out_display::{device_menu, flash_of};
use super::node_data::{KnobCellCtx, SynthNodeData};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// Knobs to a row.
const PER_ROW: usize = 4;

/// A knob's cell, wide enough for a parameter's name under it (patch units).
const CELL: f32 = 68.0;

/// A row of knobs: the dial, its readout and its name.
const ROW: f32 = 72.0;

/// The header's height: the device line and the profile's.
const HEADER: f32 = 34.0;

/// How long the lamp glows after a message, in seconds.
const FLASH: f64 = 0.2;

/// How a knob is dressed for the parameter it holds.
pub struct Dress {
    /// The parameter's name, or a dash for an empty knob.
    pub label: String,
    /// What it sends, as the device names it.
    pub text: String,
    /// Its group's colour.
    pub accent: Color32,
}

/// The knob `slot` a parameter name is, if it's one of an Instrument's.
fn slot_of(param_name: &str) -> Option<usize> {
    SLOT_NAMES.iter().position(|&name| name == param_name)
}

/// How knob `param_name` of an Instrument at `travel` (0 to 1) is dressed.
pub fn dress(data: &SynthNodeData, param_name: &str, travel: f32) -> Option<Dress> {
    let slot = slot_of(param_name)?;
    let Some(held) = instrument::held(&data.labels, slot, &hardware::known()) else {
        return Some(Dress { label: "—".to_string(), text: String::new(), accent: theme::text::DISABLED });
    };
    let parameter = &held.parameter;
    let value = SlotMap::of(parameter).value(travel);
    Some(Dress { label: parameter.name.clone(), text: parameter.text(value), accent: hardware::group_color(&parameter.group) })
}

/// What an Instrument's knob jack is called: the parameter its knob holds,
/// so an LFO is patched into Cutoff, not Slot 1. A Kit's lane jacks are
/// named for their parts.
pub fn jack_name(data: &SynthNodeData, param_name: &str) -> Option<String> {
    if !instrument::plays_a_profile(data.module_id) {
        return None;
    }
    if data.module_id == kit::ID {
        if let Some(name) = super::kit_face::jack_name(data, param_name) {
            return Some(name);
        }
    }
    let slot = slot_of(param_name)?;
    instrument::held(&data.labels, slot, &hardware::known()).map(|held| held.parameter.name)
}

/// Draws the face and the knobs under it, and returns what they asked for.
pub fn instrument_face(
    data: &SynthNodeData,
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &mut SynthGraphState,
    zoom: f32,
) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    let z = zoom;
    let mut responses = Vec::new();
    let width = PER_ROW as f32 * CELL * z;
    if let Some((rect, response, painter)) = display_frame::frame_of_height(ui, width, HEADER * z, theme::module::OUTPUT, z) {
        header(data, ui, node_id, graph, user_state, rect, &response, &painter, z, &mut responses);
    }
    if data.module_id == kit::ID {
        ui.add_space(4.0 * z);
        responses.extend(super::kit_face::lanes(data, ui, node_id, graph, user_state, width, CELL, z));
    }

    // The knobs, each centred in a cell wide enough for its name
    ui.add_space(6.0 * z);
    let rows = SLOTS.div_ceil(PER_ROW);
    let (grid, _) = ui.allocate_exact_size(Vec2::new(width, rows as f32 * ROW * z), Sense::hover());
    let knob_width = 44.0 * z;
    for knob in &data.knob_params {
        let Some(index) = slot_of(&knob.param_name) else { continue };
        let (row, column) = (index / PER_ROW, index % PER_ROW);
        let centre = grid.left() + (column as f32 + 0.5) * CELL * z;
        let cell = Rect::from_min_size(Pos2::new(centre - knob_width / 2.0 + 4.0 * z, grid.top() + row as f32 * ROW * z), Vec2::new(knob_width, ROW * z));
        let mut cell_ui = ui.new_child(UiBuilder::new().max_rect(cell).id_salt(("instrument_knob", index)));
        data.knob_cell(&mut cell_ui, knob, &mut KnobCellCtx::module(node_id, graph, user_state, z, &mut responses));
    }
    responses
}

/// The device line and the profile's, with their menus.
#[allow(clippy::too_many_arguments)]
fn header(
    data: &SynthNodeData,
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    rect: Rect,
    response: &egui::Response,
    painter: &egui::Painter,
    z: f32,
    responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
) {
    let port = (display_frame::value_of(graph, node_id, "Port").unwrap_or(0.0).round().max(0.0) as usize).min(PORTS - 1);
    let channel = display_frame::value_of(graph, node_id, "Channel").unwrap_or(0.0).round() as usize + 1;
    let readout = display_frame::readout(user_state, node_id).unwrap_or_default();
    let values = &readout.values;
    let is_kit = data.module_id == kit::ID;
    // The Kit keeps its counts in its own places
    let (live_at, notes_at, knobs_at) = if is_kit {
        (Kit::READOUT_LIVE, Kit::READOUT_NOTES_SENT, Kit::READOUT_KNOBS_SENT)
    } else {
        (Instrument::READOUT_LIVE, Instrument::READOUT_NOTES_SENT, Instrument::READOUT_KNOBS_SENT)
    };
    let live = values[live_at] > 0.5;
    let status = live.then(|| MIDI.status(port));
    let device = MIDI.chosen(port);
    let found = device.as_ref().and(MIDI.found()[port].clone());
    let small = egui::FontId::proportional(8.0 * z);
    let now = ui.input(|i| i.time);
    let amber = Color32::from_rgb(240, 190, 80);

    // --- The device line: a lamp, the port, its device, and the channel ---
    let lamp = match status {
        None | Some(PortStatus::Idle) => theme::text::DISABLED,
        Some(PortStatus::Sending) => theme::signal::GATE,
        Some(PortStatus::NoDevice | PortStatus::Missing) => amber,
        Some(PortStatus::Failed) => Color32::from_rgb(240, 90, 90),
    };
    let lamp_at = rect.left_top() + Vec2::new(7.0 * z, 7.0 * z);
    if status == Some(PortStatus::Sending) {
        // A flash as a note or a knob goes out
        let sent = values[notes_at] as u32 * 4096 + values[knobs_at] as u32;
        let flash = flash_of(ui, Id::unique(("instrument_flash", node_id)), sent, now, FLASH);
        painter.circle_filled(lamp_at, (4.0 + 2.0 * flash) * z, lamp.gamma_multiply(0.12 + 0.3 * flash));
        if flash > 0.0 {
            crate::app::pace::animate(ui.ctx());
        }
    }
    painter.circle_filled(lamp_at, 2.5 * z, lamp);
    let name = match (&device, status) {
        (None, _) => "choose a device".to_string(),
        (Some(name), Some(PortStatus::Missing)) => format!("{name} (unplugged)"),
        (Some(name), Some(PortStatus::Failed)) => format!("{name} (in use)"),
        (Some(name), _) => found.clone().unwrap_or_else(|| name.clone()),
    };
    let device_rect = Rect::from_min_max(lamp_at + Vec2::new(5.0 * z, -5.0 * z), Pos2::new(rect.right() - 28.0 * z, lamp_at.y + 5.0 * z));
    let picker = ui.interact(device_rect, Id::unique(("instrument_device", node_id)), Sense::click());
    let ink = if picker.hovered() && live {
        theme::text::PRIMARY
    } else if device.is_some() && live {
        theme::text::SECONDARY
    } else {
        theme::text::DISABLED
    };
    let galley = painter.layout_no_wrap(format!("{} · {name}", PORT_LABELS[port]), small.clone(), ink);
    painter.with_clip_rect(device_rect.intersect(painter.clip_rect())).galley(Pos2::new(device_rect.left(), lamp_at.y - galley.size().y / 2.0), galley, ink);
    painter.text(rect.right_top() + Vec2::new(-4.0 * z, 2.5 * z), egui::Align2::RIGHT_TOP, format!("Ch {channel}"), small.clone(), theme::text::SECONDARY);
    device_menu(ui, &picker, port, device.as_deref(), found.as_deref());

    // --- The profile's line: its name, and Send all ---
    let profiles = hardware::known();
    let chosen = instrument::profile_name(&data.labels);
    let known = chosen.and_then(|name| profiles.find(name));
    let line_y = rect.top() + 23.0 * z;
    let send_rect = Rect::from_min_max(Pos2::new(rect.right() - 46.0 * z, line_y - 6.0 * z), Pos2::new(rect.right() - 2.0 * z, line_y + 6.0 * z));
    let name_rect = Rect::from_min_max(Pos2::new(rect.left() + 5.0 * z, line_y - 7.0 * z), Pos2::new(send_rect.left() - 4.0 * z, line_y + 7.0 * z));
    let chooser = ui.interact(name_rect, Id::unique(("instrument_profile", node_id)), Sense::click());
    let title = egui::FontId::new(11.0 * z, egui::FontFamily::Name(theme::TITLE_FAMILY.into()));
    let (text, ink) = match (chosen, known) {
        (None, _) => (if is_kit { "Choose a kit" } else { "Choose an instrument" }.to_string(), if chooser.hovered() { theme::text::PRIMARY } else { theme::text::DISABLED }),
        (Some(name), Some(_)) => (name.to_string(), if chooser.hovered() { theme::text::PRIMARY } else { theme::text::SECONDARY.lerp_to_gamma(theme::text::PRIMARY, 0.5) }),
        (Some(name), None) => (format!("{name} ?"), amber),
    };
    let galley = painter.layout_no_wrap(text, title, ink);
    let name_end = name_rect.left() + galley.size().x;
    painter.with_clip_rect(name_rect.intersect(painter.clip_rect())).galley(Pos2::new(name_rect.left(), line_y - galley.size().y / 2.0), galley, ink);
    if let Some(maker) = known.map(|k| &k.profile.maker).filter(|maker| !maker.is_empty()) {
        let at = Pos2::new(name_end + 4.0 * z, line_y + 1.0 * z);
        if at.x + 20.0 * z < name_rect.right() {
            painter.with_clip_rect(name_rect.intersect(painter.clip_rect())).text(at, egui::Align2::LEFT_CENTER, maker, small.clone(), theme::text::DISABLED);
        }
    }
    let chooser = match (chosen, known) {
        (Some(name), None) => chooser.on_hover_text(format!("This computer has no profile called {name}: its knobs play as the patch wrote them down. Add the profile to Documents/Soba/Instruments to name them again")),
        (Some(_), Some(known)) => chooser.on_hover_text(known.profile.about.clone()),
        (None, _) if is_kit => chooser.on_hover_text("Click to choose the drum machine or sampler it plays: its lanes take its tracks, pads or parts, and its knobs the device's own names"),
        (None, _) => chooser.on_hover_text("Click to choose the hardware it plays: its knobs take the device's own names"),
    };
    profile_menu(ui, &chooser, node_id, graph, chosen, profiles.all(), is_kit, responses);

    let send = ui.interact(send_rect, Id::unique(("instrument_send_all", node_id)), Sense::click());
    let can_send = chosen.is_some();
    let send_ink = match (can_send, send.hovered()) {
        (false, _) => theme::text::DISABLED,
        (true, true) => theme::text::PRIMARY,
        (true, false) => theme::text::SECONDARY,
    };
    painter.text(send_rect.right_center(), egui::Align2::RIGHT_CENTER, "Send all", small, send_ink);
    let send = send.on_hover_text("Sends every knob to the hardware again, so it comes back to the patch. Opening the patch does this too");
    if can_send && send.clicked() {
        responses.push(NodeResponse::User(SynthResponse::SendAll(node_id)));
    }

    response.clone().on_hover_text(match status {
        None => "The Instrument sends once the sound starts".to_string(),
        Some(PortStatus::NoDevice) | Some(PortStatus::Idle) if device.is_none() => format!("Port {} has no device yet: click its name to choose one", PORT_LABELS[port]),
        Some(PortStatus::Missing) => "The device isn't plugged in. Its knobs go out again once it is: Send all brings it back to the patch".to_string(),
        Some(PortStatus::Failed) => "The device wouldn't open. Another app may be using it: close that, and Soba tries again".to_string(),
        _ if is_kit => format!("Playing port {} on channel {channel}: each lane strikes its part, and each knob is the device's own control", PORT_LABELS[port]),
        _ => format!("Playing port {} on channel {channel}: notes from Gate and Pitch, and each knob as the device's own control", PORT_LABELS[port]),
    });
}

/// The menu a click on the profile's name opens: every profile this
/// computer knows, the shipped ones first. A Kit's lists only kits.
#[allow(clippy::too_many_arguments)]
fn profile_menu(
    ui: &mut egui::Ui,
    chooser: &egui::Response,
    node_id: NodeId,
    graph: &SynthGraph,
    chosen: Option<&str>,
    profiles: &[Known],
    is_kit: bool,
    responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
) {
    let popup = chooser.id.with("menu");
    if chooser.clicked() {
        // A profile saved in the folder since shows up at once
        hardware::reload();
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    egui::Popup::from_response(chooser).id(popup).open_memory(None).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
        ui.set_min_width(170.0);
        ui.label(egui::RichText::new("Plays").small().weak());
        let mut yours = false;
        for known in profiles.iter().filter(|known| !is_kit || instrument::is_kit(&known.profile)) {
            if matches!(known.origin, Origin::File(_)) && !yours {
                ui.separator();
                ui.label(egui::RichText::new("Yours").small().weak());
                yours = true;
            }
            let profile = &known.profile;
            let current = chosen.is_some_and(|name| name.eq_ignore_ascii_case(&profile.name));
            let text = if profile.maker.is_empty() { profile.name.clone() } else { format!("{} · {}", profile.name, profile.maker) };
            if ui.selectable_label(current, text).clicked() {
                responses.extend(choose(node_id, graph, profile).into_iter().map(NodeResponse::User));
            }
        }
        if chosen.is_some() {
            ui.separator();
            if ui.selectable_label(false, "None").clicked() {
                let mut labels = vec![(instrument::PROFILE.to_string(), String::new())];
                labels.extend((0..SLOTS).map(|slot| (instrument::slot_key(slot), String::new())));
                if is_kit {
                    labels.extend((0..LANES).map(|lane| (instrument::lane_key(lane), String::new())));
                }
                let label = if is_kit { "Clear Kit" } else { "Clear Instrument" }.to_string();
                responses.push(NodeResponse::User(SynthResponse::SetLabels { node_id, label, labels }));
            }
        }
        let folder = hardware::folder();
        ui.label(egui::RichText::new(format!("Your own: {}", folder.display())).small().weak());
    });
}

/// What choosing `profile` asks for, as one undo step: its name and its
/// first twelve parameters on the knobs, each knob at its default, the
/// profile's channel, and the port whose device it is, if one is. A Kit's
/// lanes take its first eight parts, and the knobs the first's track. Then
/// every knob is sent.
fn choose(node_id: NodeId, graph: &SynthGraph, profile: &Profile) -> Vec<SynthResponse> {
    let is_kit = graph[node_id].user_data.module_id == kit::ID;
    let labels = if is_kit { instrument::choose_kit(profile) } else { instrument::choose(profile) };
    let mut asked = vec![SynthResponse::SetLabels { node_id, label: format!("Play {}", profile.name), labels }];
    let set = |param_name: &str, value: f32| SynthResponse::ParameterChanged { node_id, param_name: param_name.to_string(), value };
    if is_kit {
        asked.push(set("Track", 0.0));
    }
    asked.extend(profile.parameters.iter().zip(SLOT_NAMES).map(|(parameter, slot)| set(slot, instrument::start(parameter))));
    asked.push(set("Channel", (profile.channel() as f32 - 1.0).clamp(0.0, 15.0)));
    if let Some(port) = port_of(profile) {
        asked.push(set("Port", port as f32));
    }
    // Chosen again, nothing changes: Send all sends it anyway
    if instrument::profile_name(&graph[node_id].user_data.labels).is_some_and(|name| name.eq_ignore_ascii_case(&profile.name)) {
        asked.push(SynthResponse::SendAll(node_id));
    }
    asked
}

/// The port whose device `profile` looks to be, if the Rig gives one.
fn port_of(profile: &Profile) -> Option<usize> {
    let hint = profile.port.to_lowercase();
    if hint.is_empty() {
        return None;
    }
    let found = MIDI.found();
    (0..PORTS).find(|&port| found[port].clone().or_else(|| MIDI.chosen(port)).is_some_and(|device| device.to_lowercase().contains(&hint)))
}

/// A knob's own items at the top of its right-click menu: which of the
/// profile's parameters it holds, by group.
pub fn slot_items(ui: &mut egui::Ui, data: &SynthNodeData, node_id: NodeId, param_name: &str) -> Vec<SynthResponse> {
    let mut asked = Vec::new();
    let Some(slot) = slot_of(param_name) else { return asked };
    let profiles = hardware::known();
    let held = instrument::held(&data.labels, slot, &profiles);
    let heading = match &held {
        Some(held) => format!("{} · {}{}", held.parameter.name, held.parameter.address.text(), held.parameter.channel.map(|c| format!(" ch {c}")).unwrap_or_default()),
        None => format!("{param_name} · empty"),
    };
    ui.label(egui::RichText::new(heading).small().weak());
    let Some(known) = instrument::profile_name(&data.labels).and_then(|name| profiles.find(name)) else {
        ui.label(egui::RichText::new("Choose an instrument on the face to give this knob one of its controls").small().weak());
        ui.separator();
        return asked;
    };
    let current = held.as_ref().map(|held| held.parameter.name.as_str());
    let mut give = |parameter: Option<&Parameter>| {
        let text = parameter.map(instrument::written).unwrap_or_default();
        let label = match parameter {
            Some(parameter) => format!("Knob to {}", parameter.name),
            None => "Empty knob".to_string(),
        };
        asked.push(SynthResponse::SetLabels { node_id, label, labels: vec![(instrument::slot_key(slot), text)] });
        if let Some(parameter) = parameter {
            asked.push(SynthResponse::ParameterChanged { node_id, param_name: param_name.to_string(), value: instrument::start(parameter) });
        }
    };
    crate::widgets::menu::menu_button(ui, "Holds", |ui| {
        ui.set_min_width(150.0);
        // By group, in the order the profile first names each
        let mut groups: Vec<&str> = Vec::new();
        for parameter in &known.profile.parameters {
            if !groups.contains(&parameter.group.as_str()) {
                groups.push(&parameter.group);
            }
        }
        for group in groups {
            let members = known.profile.parameters.iter().filter(|p| p.group == group);
            let title = if group.is_empty() { "Other" } else { group };
            crate::widgets::menu::menu_button(ui, egui::RichText::new(title).color(hardware::group_color(group)), |ui| {
                for parameter in members {
                    if ui.selectable_label(current == Some(parameter.name.as_str()), &parameter.name).on_hover_text(parameter.address.text()).clicked() {
                        give(Some(parameter));
                        ui.close();
                    }
                }
            });
        }
        ui.separator();
        if ui.selectable_label(held.is_none(), "Nothing").clicked() {
            give(None);
            ui.close();
        }
    });
    ui.separator();
    asked
}
