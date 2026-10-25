//! The Setlist's face (#445): the song's sections, top to bottom, each with
//! the rows that change the room as it starts.
//!
//! - **Along the top**, a lamp that flashes as rows go, the Arranger it
//!   follows, and while a section is on its way, how long to its bar line.
//! - **Each section** is named as its Arranger names it. A green tab says
//!   the room plays it; an amber one that its rows are going ahead of the
//!   bar. "+" adds a row, a copy of the row above it in the song, a
//!   program on.
//! - **Each row** shows its port, lit as its Rig port is (green sending,
//!   amber without its device, red when it won't open), the device, the
//!   program as the device names it ("Pattern A05", "Scene A 12"), and
//!   its lead. A lamp glows as it goes. Click a row to change it.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Id, Rect, Sense, Stroke};
use egui_node_graph2::{NodeId, NodeResponse};

use crate::app::theme;
use crate::hardware::setlist::{self, Row, MAX_LEAD, ROWS, SECTIONS};
use crate::hardware::{self, Profiles};
use crate::link::midi::{PortStatus, MIDI, PORTS, PORT_LABELS};
use crate::modules::setlist::Setlist;

use super::display_frame;
use super::groups::{leaf_source, GroupIndex};
use super::midi_out_display::flash_of;
use super::node_data::SynthNodeData;
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// The face's width (patch units).
const WIDTH: f32 = 252.0;
/// The status line's height.
const HEADER: f32 = 16.0;
/// A section's name line.
const SECTION: f32 = 15.0;
/// A row's line.
const ROW: f32 = 13.0;
/// How far a row is set in from the section's name.
const INDENT: f32 = 12.0;
/// How long a row's lamp takes to fade, in seconds: its time constant.
const GLOW: f32 = 0.35;
/// How long the status lamp flashes after a row goes, in seconds.
const FLASH: f64 = 0.25;

/// The Arranger whose Section is patched into the Setlist `node_id`, if one is.
fn arranger_of(graph: &SynthGraph, node_id: NodeId) -> Option<NodeId> {
    let node = graph.nodes.get(node_id)?;
    let input = node.get_input("Section").ok()?;
    let output = graph.connections.get(input)?.first().copied()?;
    let output = leaf_source(graph, &GroupIndex::of(graph), output)?;
    let source = graph.outputs.get(output)?.node;
    let arranger = graph.nodes.get(source)?;
    (arranger.user_data.module_id == crate::modules::arranger::ID && arranger.get_output("Section").ok() == Some(output)).then_some(source)
}

/// A section's name, as its Arranger names it, or its number.
fn section_name(graph: &SynthGraph, arranger: Option<NodeId>, section: usize) -> String {
    arranger
        .and_then(|a| graph.nodes.get(a))
        .and_then(|a| a.user_data.labels.get(&super::arranger_display::section_key(section)).cloned())
        .unwrap_or_else(|| format!("Section {}", section + 1))
}

/// The lamp colour of a Rig port: green sending, amber without its device,
/// red when it won't open, dim while nothing plays it.
fn port_color(port: usize, live: bool) -> Color32 {
    match live.then(|| MIDI.status(port)) {
        None | Some(PortStatus::Idle) => theme::text::DISABLED,
        Some(PortStatus::Sending) => theme::signal::GATE,
        Some(PortStatus::NoDevice | PortStatus::Missing) => theme::accent::WARNING,
        Some(PortStatus::Failed) => theme::accent::ERROR,
    }
}

/// Draws the face and returns what its menus asked for.
pub fn setlist_face(data: &SynthNodeData, ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    let z = zoom;
    let mut responses = Vec::new();
    let arranger = arranger_of(graph, node_id);
    let sections = match arranger {
        Some(a) => display_frame::value_of(graph, a, "Sections").map_or(1, |n| (n.round().max(1.0) as usize).min(SECTIONS)),
        // On its own: every section with a row, and one more to add to
        None => setlist::last_section(&data.labels).map_or(1, |last| (last + 2).min(SECTIONS)),
    };
    let all: Vec<[Option<Row>; ROWS]> = (0..sections).map(|s| setlist::rows(&data.labels, s)).collect();
    let height = HEADER + all.iter().map(|rows| SECTION + rows.iter().flatten().count() as f32 * ROW).sum::<f32>() + 4.0;
    let Some((rect, response, painter)) = display_frame::frame_of_height(ui, WIDTH * z, height * z, theme::module::OUTPUT, z) else {
        return responses;
    };
    let readout = display_frame::readout(user_state, node_id).unwrap_or_default();
    let values = &readout.values;
    let live = values[Setlist::READOUT_LIVE] > 0.5;
    let section_of = |at: usize| (values[at] as usize).checked_sub(1);
    let (ready, ahead, shown) = (section_of(Setlist::READOUT_READY), section_of(Setlist::READOUT_AHEAD), section_of(Setlist::READOUT_SHOWN));
    let profiles = hardware::known();
    let small = FontId::proportional(7.5 * z);
    let name_font = FontId::proportional(8.5 * z);
    let now = ui.input(|i| i.time);
    let mut animate = false;

    // --- The status line ---
    let lamp_at = rect.left_top() + vec2(7.0 * z, HEADER * z / 2.0);
    let flash = flash_of(ui, Id::unique(("setlist_flash", node_id)), values[Setlist::READOUT_SENT] as u32, now, FLASH);
    if flash > 0.0 {
        painter.circle_filled(lamp_at, (4.0 + 2.0 * flash) * z, theme::signal::MIDI.gamma_multiply(0.15 + 0.35 * flash));
        animate = true;
    }
    painter.circle_filled(lamp_at, 2.5 * z, if live { theme::signal::MIDI } else { theme::text::DISABLED });
    let patched = |name: &str| display_frame::input_patched(graph, node_id, name);
    let (status, ink) = match arranger {
        None if !patched("Section") => ("Patch an Arranger's Section, Next and Bar".to_string(), theme::text::DISABLED),
        None => ("Following a Section".to_string(), theme::text::SECONDARY),
        Some(a) if !patched("Next") || !patched("Bar") => (format!("{} · patch Next and Bar to send ahead", super::arranger_follow::name(graph, a)), theme::text::SECONDARY),
        Some(a) => (format!("Follows {}", super::arranger_follow::name(graph, a)), theme::text::SECONDARY),
    };
    painter.text(lamp_at + vec2(6.0 * z, 0.0), Align2::LEFT_CENTER, status, small.clone(), ink);
    let left = values[Setlist::READOUT_LEFT];
    if ahead.is_some() && left >= 0.0 {
        painter.text(rect.right_top() + vec2(-5.0 * z, HEADER * z / 2.0), Align2::RIGHT_CENTER, format!("bar in {left:.1} s"), small.clone(), theme::accent::WARNING);
        animate = true;
    }
    response.on_hover_text("Each section's rows change the room as it starts: a device's pattern, scene, patch or memory, as Bank Select and Program Change. A row with a lead goes that long before the bar line, once the Arranger's Next says where the song goes");

    // --- The sections, each with its rows ---
    let mut y = rect.top() + HEADER * z;
    for (section, rows) in all.iter().enumerate() {
        let line = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), SECTION * z));
        let tab = Rect::from_min_size(pos2(rect.left() + 3.0 * z, line.top() + 3.0 * z), vec2(2.5 * z, line.height() - 5.0 * z));
        let pulse = 0.55 + 0.45 * (now * 6.0).sin() as f32;
        let (tab_color, name_ink) = if ahead == Some(section) {
            animate = true;
            (theme::accent::WARNING.gamma_multiply(pulse), theme::text::PRIMARY)
        } else if ready == Some(section) {
            (theme::signal::GATE, theme::text::PRIMARY)
        } else {
            (theme::text::DISABLED.gamma_multiply(0.5), theme::text::SECONDARY)
        };
        painter.rect_filled(tab, 1.0 * z, tab_color);
        painter.text(pos2(tab.right() + 5.0 * z, line.center().y), Align2::LEFT_CENTER, section_name(graph, arranger, section), name_font.clone(), name_ink);
        let count = rows.iter().flatten().count();
        if count == 0 {
            painter.text(pos2(line.right() - 22.0 * z, line.center().y), Align2::RIGHT_CENTER, "no change", small.clone(), theme::text::DISABLED);
        }

        // "+" adds a row in the first free place
        if let Some(free) = rows.iter().position(Option::is_none) {
            let plus = Rect::from_center_size(pos2(line.right() - 10.0 * z, line.center().y), vec2(14.0 * z, 12.0 * z));
            let add = ui.interact(plus, Id::unique(("setlist_add", node_id, section)), Sense::click());
            let ink = if add.hovered() { theme::text::PRIMARY } else { theme::text::QUIET };
            painter.text(plus.center(), Align2::CENTER_CENTER, "+", FontId::proportional(10.0 * z), ink);
            if add.on_hover_text("Adds a row: a copy of the one above it in the song, a program on").clicked() {
                let row = new_row(&data.labels, section, free, &profiles);
                responses.push(set_row(node_id, section, free, Some(&row), "Add Setlist row"));
                egui::Popup::open_id(ui.ctx(), row_id(node_id, section, free).with("menu"));
            }
        }
        y += SECTION * z;

        for (index, row) in rows.iter().enumerate() {
            let Some(row) = row else { continue };
            let line = Rect::from_min_size(pos2(rect.left() + INDENT * z, y), vec2(rect.width() - INDENT * z - 3.0 * z, ROW * z));
            let clicked = ui.interact(line, row_id(node_id, section, index), Sense::click());
            if clicked.hovered() {
                painter.rect_filled(line, 2.0 * z, theme::background::WIDGET_HOVERED.gamma_multiply(0.6));
            }
            let mid = line.center().y;

            // A lamp as it goes
            let age = if shown == Some(section) { values[Setlist::READOUT_AGES + index] } else { f32::MAX };
            let glow = (-age / GLOW).exp();
            let lamp = pos2(line.left() + 3.0 * z, mid);
            if glow > 0.01 {
                painter.circle_filled(lamp, (2.5 + 3.0 * glow) * z, theme::signal::MIDI.gamma_multiply(0.35 * glow));
                animate = true;
            }
            painter.circle_filled(lamp, 1.6 * z, theme::signal::MIDI.gamma_multiply(0.35 + 0.65 * glow));

            // The port, lit as the Rig has it
            let port = (row.port as usize).min(PORTS - 1);
            let pill = Rect::from_center_size(pos2(line.left() + 14.0 * z, mid), vec2(10.0 * z, 9.0 * z));
            let port_ink = port_color(port, live);
            painter.rect_stroke(pill, 2.0 * z, Stroke::new(1.0 * z, port_ink), egui::StrokeKind::Inside);
            painter.text(pill.center(), Align2::CENTER_CENTER, PORT_LABELS[port], FontId::proportional(6.5 * z), port_ink);

            // The device, the program, and the lead
            let device = if row.device.is_empty() { format!("Ch {}", row.channel) } else { row.device.clone() };
            let device_rect = Rect::from_min_max(pos2(pill.right() + 4.0 * z, line.top()), pos2(line.left() + 78.0 * z, line.bottom()));
            painter.with_clip_rect(device_rect.intersect(painter.clip_rect())).text(pos2(device_rect.left(), mid), Align2::LEFT_CENTER, device, small.clone(), theme::text::QUIET);
            let lead_text = (row.lead > 0).then(|| format!("{} ms", row.lead));
            let program_rect = Rect::from_min_max(pos2(device_rect.right() + 3.0 * z, line.top()), pos2(line.right() - 40.0 * z, line.bottom()));
            let known = row.device.is_empty() || profiles.find(&row.device).is_some();
            let program_ink = if known { theme::text::PRIMARY } else { theme::accent::WARNING };
            painter.with_clip_rect(program_rect.intersect(painter.clip_rect())).text(pos2(program_rect.left(), mid), Align2::LEFT_CENTER, row.name(&profiles), small.clone(), program_ink);
            if let Some(lead) = lead_text {
                painter.text(pos2(line.right() - 3.0 * z, mid), Align2::RIGHT_CENTER, lead, small.clone(), theme::signal::CONTROL.gamma_multiply(0.85));
            }

            let tip = describe(row, &profiles, known);
            let clicked = clicked.on_hover_text(tip);
            let popup = clicked.id.with("menu");
            if clicked.clicked() {
                egui::Popup::toggle_id(ui.ctx(), popup);
                // Opening, the device list shows the row's
                ui.ctx().data_mut(|d| d.insert_temp(popup.with("scroll"), true));
            }
            let name = section_name(graph, arranger, section);
            egui::Popup::from_response(&clicked).id(popup).open_memory(None).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(220.0);
                let scroll = ui.ctx().data_mut(|d| d.remove_temp::<bool>(popup.with("scroll"))).unwrap_or(false);
                row_editor(ui, node_id, (section, index), row, &name, scroll, &profiles, &mut responses);
            });
            y += ROW * z;
        }
    }
    if animate {
        crate::app::pace::animate(ui.ctx());
    }
    responses
}

/// The id a row's line answers to, and its menu after it.
fn row_id(node_id: NodeId, section: usize, row: usize) -> Id {
    Id::unique(("setlist_row", node_id, section, row))
}

/// What a row does, as its tooltip says it.
fn describe(row: &Row, profiles: &Profiles, known: bool) -> String {
    let map = row.map(profiles);
    let bank = match (map.program.bank_msb, map.program.bank_lsb) {
        (Some(msb), Some(lsb)) => format!("Bank Select {msb}/{lsb}, then "),
        (Some(msb), None) => format!("Bank Select {msb}, then "),
        _ => String::new(),
    };
    let when = match row.lead {
        0 => "on the downbeat".to_string(),
        lead => format!("{lead} ms before the bar line"),
    };
    let unknown = if known { String::new() } else { format!("\nThis computer has no profile called {}: it goes as the patch wrote it down", row.device) };
    format!("{}: {bank}Program Change {} on channel {}, port {}, {when}{unknown}\nClick to change it", row.name(profiles), map.program.program, map.channel + 1, PORT_LABELS[map.port as usize])
}

/// The labels that set row `row` of `section` to `row_of` (None takes it away).
fn set_row(node_id: NodeId, section: usize, row: usize, row_of: Option<&Row>, label: &str) -> NodeResponse<SynthResponse, SynthNodeData> {
    let text = row_of.map(setlist::written).unwrap_or_default();
    NodeResponse::User(SynthResponse::SetLabels { node_id, label: label.to_string(), labels: vec![(setlist::row_key(section, row), text)] })
}

/// A new row for place `row` of `section`: the row in that place in the
/// nearest section before it, or else the last row above it in this one,
/// a program on; or else plain numbers on port A.
fn new_row(labels: &std::collections::BTreeMap<String, String>, section: usize, row: usize, profiles: &Profiles) -> Row {
    let earlier = (0..section).rev().find_map(|s| setlist::row(labels, s, row));
    let above = || (0..row).rev().find_map(|r| setlist::row(labels, section, r));
    match earlier.or_else(above) {
        Some(mut copy) => {
            copy.program = (copy.program + 1).min(copy.count(profiles).saturating_sub(1));
            copy
        }
        None => Row::new(0, None),
    }
}

/// A row's menu: its device, port, channel, bank, program and lead.
#[allow(clippy::too_many_arguments)]
fn row_editor(
    ui: &mut egui::Ui,
    node_id: NodeId,
    (section, index): (usize, usize),
    row: &Row,
    section_name: &str,
    scroll: bool,
    profiles: &Profiles,
    responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
) {
    let mut edited = row.clone();
    ui.label(egui::RichText::new(format!("{section_name} · row {}", index + 1)).small().weak());

    // The device: one of the profiles with programs, or plain numbers
    ui.label(egui::RichText::new("Device").small());
    egui::ScrollArea::vertical().id_salt("setlist_devices").max_height(118.0).show(ui, |ui| {
        if ui.selectable_label(edited.device.is_empty(), "Plain numbers").clicked() && !edited.device.is_empty() {
            edited.device.clear();
            edited.bank = None;
        }
        for known in profiles.all().iter().filter(|k| k.profile.programs.is_some()) {
            let profile = &known.profile;
            let current = edited.device.eq_ignore_ascii_case(&profile.name);
            let what = profile.programs.as_ref().map_or("", |p| p.what.as_str());
            let item = ui.selectable_label(current, format!("{} · {what}", profile.name));
            if current && scroll {
                item.scroll_to_me(None);
            }
            if item.clicked() && !current {
                edited.choose(profile);
                if let Some(port) = super::instrument_face::port_of(profile) {
                    edited.port = port as u8;
                }
            }
        }
    });
    ui.separator();

    egui::Grid::new("setlist_row_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Port");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (port, label) in PORT_LABELS.iter().enumerate() {
                let device = MIDI.chosen(port).unwrap_or_else(|| "no device".to_string());
                if ui.selectable_label(edited.port as usize == port, *label).on_hover_text(device).clicked() {
                    edited.port = port as u8;
                }
            }
        });
        ui.end_row();

        ui.label("Channel");
        ui.add(egui::DragValue::new(&mut edited.channel).range(1..=16));
        ui.end_row();

        let programs = edited.programs(profiles).cloned();
        match &programs {
            Some(programs) if !programs.banks.is_empty() => {
                ui.label("Bank");
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(edited.bank.is_none(), "None").clicked() {
                        edited.set_bank(None, Some(programs));
                    }
                    for (b, bank) in programs.banks.iter().enumerate() {
                        if ui.selectable_label(edited.bank == Some(b as u8), &bank.name).clicked() {
                            edited.set_bank(Some(b as u8), Some(programs));
                        }
                    }
                });
                ui.end_row();
            }
            Some(_) => {}
            None => {
                ui.label("Bank");
                ui.horizontal(|ui| {
                    bank_byte(ui, &mut edited.msb, "MSB");
                    bank_byte(ui, &mut edited.lsb, "LSB");
                });
                ui.end_row();
            }
        }

        let what = programs.as_ref().map_or("Program".to_string(), |p| p.what.clone());
        ui.label(what);
        let count = edited.count(profiles).max(1);
        let mut program = edited.program.min(count - 1);
        let drag = match programs.clone() {
            Some(programs) => {
                let shown = programs.clone();
                egui::DragValue::new(&mut program)
                    .range(0..=count - 1)
                    .speed(0.15)
                    .custom_formatter(move |n, _| shown.label(n as u16))
                    .custom_parser(move |text| (0..programs.count()).find(|&i| programs.label(i).eq_ignore_ascii_case(text.trim())).map(f64::from))
            }
            None => egui::DragValue::new(&mut program).range(0..=127).speed(0.15),
        };
        ui.add(drag);
        edited.program = program;
        ui.end_row();

        ui.label("Lead");
        ui.add(egui::DragValue::new(&mut edited.lead).range(0..=MAX_LEAD).speed(5.0).suffix(" ms"))
            .on_hover_text("How long before the bar line it goes. Most devices switch at once: 0 lands on the downbeat. A Digitakt queues its next pattern for the end of the one playing, so give it a lead inside the last bar");
        ui.end_row();
    });

    if edited != *row {
        responses.push(set_row(node_id, section, index, Some(&edited), "Change Setlist row"));
    }
    ui.separator();
    if ui.button("Remove row").clicked() {
        responses.push(set_row(node_id, section, index, None, "Remove Setlist row"));
        ui.close();
    }
}

/// A Bank Select byte for plain numbers: 0 to 127, or off.
fn bank_byte(ui: &mut egui::Ui, byte: &mut Option<u8>, name: &str) {
    let mut value = byte.map_or(-1, i32::from);
    ui.add(egui::DragValue::new(&mut value).range(-1..=127).speed(0.15).prefix(format!("{name} ")).custom_formatter(|n, _| if n < 0.0 { "off".to_string() } else { format!("{n:.0}") }));
    *byte = (value >= 0).then_some(value as u8);
}
