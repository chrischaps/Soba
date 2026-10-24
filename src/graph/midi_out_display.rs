//! The MIDI Out's face: which device it plays, and what it's playing.
//!
//! - **Along the top**, a lamp, the port and its device: green while
//!   messages go out, amber when the port has no device or it's unplugged,
//!   red when it won't open. Click the device to choose another; every
//!   MIDI Out on the port follows. At the right, the program, if one is
//!   chosen, glowing as it's sent, and the channel.
//! - **Two wheels** at the left, as a keyboard has: the bend, from the
//!   middle, and the pressure, from the bottom (#439).
//! - **A keyboard** of five octaves (C2 to C7), each held note lit, so a
//!   chord shows as its shape. A note beyond either end lights an arrow.
//! - **Four bars** for the CC lanes, each with what it sends (a CC, a
//!   14-bit pair or an NRPN, chosen from its right-click menu), and at the
//!   right the Clock's beat while the port sends clock (the Rig sets that,
//!   for the port: #437).

use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::link::midi::{CcMode, ClockMode, PortStatus, MAX_14, MIDI, PORTS, PORT_LABELS};
use crate::modules::midi_out::CC_LANES;
use crate::modules::MidiOut;

use super::display_frame::{self, PANEL};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// The keyboard's lowest note, C2, and how many it shows: five octaves and
/// the top C.
const LOWEST: u8 = 36;
const KEYS: u8 = 61;

/// How long a note's strike glows, in seconds.
const FLASH: f64 = 0.2;

/// How long the port's budget counts as full after it last thinned, in seconds.
const FULL_FOR: f64 = 0.5;

/// How a lane sends, as its parameters say.
struct Lane {
    mode: CcMode,
    number: u8,
    nrpn: u16,
}

impl Lane {
    fn read(graph: &SynthGraph, node_id: NodeId, lane: usize) -> Self {
        let value = |name: String| display_frame::value_of(graph, node_id, &name).unwrap_or(0.0);
        Self {
            mode: CcMode::from_param(value(format!("CC Mode {}", lane + 1))),
            number: value(format!("CC {} #", lane + 1)).round().clamp(0.0, 127.0) as u8,
            nrpn: value(format!("NRPN {}", lane + 1)).round().clamp(0.0, MAX_14 as f32) as u16,
        }
    }

    /// What it sends, short, for under its bar: "CC 74", "CC 1+33", "NRPN 1:20".
    fn label(&self) -> String {
        match self.mode {
            CcMode::Bits7 => format!("CC {}", self.number),
            CcMode::Bits14 => {
                let number = self.number.min(31);
                format!("CC {number}+{}", number + 32)
            }
            CcMode::Nrpn => format!("NRPN {}:{}", self.nrpn >> 7, self.nrpn & 0x7F),
        }
    }
}

/// Draws the face, and returns what a lane's menu asked for.
pub fn midi_out_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &mut SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let mut asked = Vec::new();
    let z = zoom;
    let port = (display_frame::value_of(graph, node_id, "Port").unwrap_or(0.0).round().max(0.0) as usize).min(PORTS - 1);
    let channel = display_frame::value_of(graph, node_id, "Channel").unwrap_or(0.0).round() as usize + 1;
    let readout = display_frame::readout(user_state, node_id).unwrap_or_default();
    let values = &readout.values;
    let live = values[MidiOut::READOUT_LIVE] > 0.5;
    let status = live.then(|| MIDI.status(port));
    let device = MIDI.chosen(port);
    // As it's called now, if the system has renumbered it since (#436)
    let found = device.as_ref().and(MIDI.found()[port].clone());

    let width = display_frame::knob_row_width(ui, CC_LANES, z);
    let accent = theme::module::OUTPUT;
    let Some((rect, response, painter)) = display_frame::frame_of_height(ui, width, 76.0 * z, accent, z) else { return asked };
    let small = egui::FontId::proportional(8.0 * z);
    let tiny = egui::FontId::proportional(6.5 * z);
    let purple = theme::signal::MIDI;
    let orange = theme::signal::CONTROL;
    let green = theme::signal::GATE;
    let amber = Color32::from_rgb(240, 190, 80);
    let red = Color32::from_rgb(240, 90, 90);
    let now = ui.input(|i| i.time);

    // --- The header: a lamp, the port, its device, and the channel ---
    let lamp = match status {
        None | Some(PortStatus::Idle) => theme::text::DISABLED,
        Some(PortStatus::Sending) => green,
        Some(PortStatus::NoDevice | PortStatus::Missing) => amber,
        Some(PortStatus::Failed) => red,
    };
    let lamp_at = rect.left_top() + Vec2::new(7.0 * z, 6.5 * z);
    if status == Some(PortStatus::Sending) {
        // A flash as each note goes out
        let flash = flash_of(ui, Id::unique(("midi_out_flash", node_id)), values[MidiOut::READOUT_NOTES_SENT] as u32, now, FLASH);
        painter.circle_filled(lamp_at, (4.0 + 2.0 * flash) * z, lamp.gamma_multiply(0.12 + 0.3 * flash));
    }
    painter.circle_filled(lamp_at, 2.5 * z, lamp);
    let name = match (&device, status) {
        (None, _) => "choose a device".to_string(),
        (Some(name), Some(PortStatus::Missing)) => format!("{name} (unplugged)"),
        (Some(name), Some(PortStatus::Failed)) => format!("{name} (in use)"),
        (Some(name), _) => found.clone().unwrap_or_else(|| name.clone()),
    };
    // Room for the channel, and the program if there is one
    let header_room = if display_frame::value_of(graph, node_id, "Program").unwrap_or(-1.0).round() >= 0.0 { 54.0 } else { 26.0 };
    let label_rect = Rect::from_min_max(
        lamp_at + Vec2::new(5.0 * z, -5.0 * z),
        Pos2::new(rect.right() - header_room * z, lamp_at.y + 5.0 * z),
    );
    let picker = ui.interact(label_rect, Id::unique(("midi_out_device", node_id)), Sense::click());
    let text_color = if picker.hovered() && live {
        theme::text::PRIMARY
    } else if device.is_some() && live {
        theme::text::SECONDARY
    } else {
        theme::text::DISABLED
    };
    let galley = painter.layout_no_wrap(format!("{} · {name}", PORT_LABELS[port]), small.clone(), text_color);
    let clip = painter.with_clip_rect(label_rect.intersect(painter.clip_rect()));
    clip.galley(Pos2::new(label_rect.left(), lamp_at.y - galley.size().y / 2.0), galley, text_color);
    // The program, if one is chosen, glowing as it goes; then the channel
    let program = display_frame::value_of(graph, node_id, "Program").unwrap_or(-1.0).round();
    if program >= 0.0 {
        let sent = flash_of(ui, Id::unique(("midi_out_program", node_id)), values[MidiOut::READOUT_PROGRAMS_SENT] as u32, now, FLASH * 3.0);
        let ink = theme::text::DISABLED.lerp_to_gamma(purple.lerp_to_gamma(theme::text::PRIMARY, 0.4), 0.45 + 0.55 * sent);
        if sent > 0.0 {
            crate::app::pace::animate(ui.ctx());
        }
        painter.text(rect.right_top() + Vec2::new(-28.0 * z, 2.0 * z), egui::Align2::RIGHT_TOP, format!("P{program:.0}"), small.clone(), ink);
    }
    painter.text(
        rect.right_top() + Vec2::new(-4.0 * z, 2.0 * z),
        egui::Align2::RIGHT_TOP,
        format!("Ch {channel}"),
        small.clone(),
        theme::text::SECONDARY,
    );
    device_menu(ui, &picker, port, device.as_deref(), found.as_deref());

    // --- The wheels: the bend from the middle, the pressure from the bottom ---
    let wheels_top = rect.top() + 15.0 * z;
    let wheels_bottom = rect.top() + 44.0 * z;
    // As sent, while a cable is in: the readout's rest is no cable's
    let bend = if display_frame::input_patched(graph, node_id, "Bend") { values[MidiOut::READOUT_BEND].max(-1.0) } else { -2.0 };
    let pressure = if display_frame::input_patched(graph, node_id, "Pressure") { values[MidiOut::READOUT_PRESSURE].max(0.0) } else { -1.0 };
    let wheel = |x: f32| Rect::from_min_max(Pos2::new(x, wheels_top), Pos2::new(x + 4.5 * z, wheels_bottom));
    let (bend_wheel, pressure_wheel) = (wheel(rect.left() + 6.0 * z), wheel(rect.left() + 13.0 * z));
    for (r, on) in [(bend_wheel, bend >= -1.0), (pressure_wheel, pressure >= 0.0)] {
        painter.rect(r, 2.0 * z, theme::background::MAIN, Stroke::new(0.5 * z, if on { orange.gamma_multiply(0.5) } else { theme::background::GRID_MAJOR }), egui::StrokeKind::Middle);
    }
    let middle = bend_wheel.center().y;
    painter.line_segment([Pos2::new(bend_wheel.left() + 0.8 * z, middle), Pos2::new(bend_wheel.right() - 0.8 * z, middle)], Stroke::new(0.5 * z, theme::background::GRID_MAJOR));
    if bend >= -1.0 {
        let to = middle - bend.clamp(-1.0, 1.0) * (bend_wheel.height() / 2.0 - 0.8 * z);
        let fill = Rect::from_x_y_ranges(bend_wheel.shrink(0.8 * z).x_range(), middle.min(to)..=middle.max(to));
        painter.rect_filled(fill.expand2(Vec2::new(0.0, 0.4 * z)), 1.2 * z, orange);
    }
    if pressure > 0.0 {
        let inner = pressure_wheel.shrink(0.8 * z);
        let fill = Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - inner.height() * pressure.min(1.0)), inner.right_bottom());
        painter.rect_filled(fill, 1.2 * z, orange);
    }
    let wheels = bend_wheel.union(pressure_wheel).expand(1.5 * z);
    let poly = values[MidiOut::READOUT_POLY] > 0.5;
    ui.interact(wheels, Id::unique(("midi_out_wheels", node_id)), Sense::hover()).on_hover_text(format!(
        "Bend {} · Pressure {}",
        if bend >= -1.0 { format!("{bend:+.2}") } else { "unpatched".to_string() },
        match (pressure >= 0.0, poly) {
            (false, _) => "unpatched".to_string(),
            (true, false) => format!("{pressure:.2}, the channel's"),
            (true, true) => format!("up to {pressure:.2}, each note's own"),
        }
    ));

    // --- The keyboard: white keys across, black keys over them ---
    let board = Rect::from_min_max(rect.left_top() + Vec2::new(27.0 * z, 15.0 * z), rect.right_top() + Vec2::new(-9.0 * z, 44.0 * z));
    let whites = (0..KEYS).filter(|k| !is_black(LOWEST + k)).count() as f32;
    let key_w = board.width() / whites;
    let lit = |note: u8| MidiOut::note_in(&readout, note);
    let mut white = 0.0;
    let mut blacks = Vec::new();
    for key in 0..KEYS {
        let note = LOWEST + key;
        if is_black(note) {
            blacks.push((note, board.left() + white * key_w));
            continue;
        }
        let r = Rect::from_min_size(Pos2::new(board.left() + white * key_w, board.top()), Vec2::new(key_w, board.height())).shrink2(Vec2::new(0.35 * z, 0.0));
        let on = lit(note);
        painter.rect_filled(r, 1.2 * z, if on { purple } else { Color32::from_rgb(196, 196, 214) });
        if on {
            painter.rect_filled(r.expand(1.2 * z), 2.0 * z, purple.gamma_multiply(0.25));
        }
        if note.is_multiple_of(12) {
            // Each C named, faintly, to read the octave by
            painter.text(
                Pos2::new(r.center().x, r.bottom() - 1.0 * z),
                egui::Align2::CENTER_BOTTOM,
                format!("C{}", note / 12 - 1),
                egui::FontId::proportional(4.6 * z),
                if on { PANEL } else { Color32::from_rgb(120, 120, 150) },
            );
        }
        white += 1.0;
    }
    for (note, x) in blacks {
        let r = Rect::from_center_size(Pos2::new(x, board.top() + board.height() * 0.31), Vec2::new(key_w * 0.62, board.height() * 0.62));
        painter.rect_filled(r, 1.0 * z, if lit(note) { purple } else { PANEL });
    }
    // Notes beyond the keyboard: an arrow at that end
    let below = (0..LOWEST).any(lit);
    let above = (LOWEST + KEYS..128).any(lit);
    for (beyond, x, sign) in [(below, board.left() - 4.5 * z, -1.0), (above, board.right() + 4.5 * z, 1.0)] {
        let y = board.center().y;
        let tip = Pos2::new(x + sign * 2.5 * z, y);
        let points = vec![tip, Pos2::new(x - sign * 2.0 * z, y - 3.5 * z), Pos2::new(x - sign * 2.0 * z, y + 3.5 * z)];
        let color = if beyond { purple } else { theme::background::GRID_MAJOR };
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }

    // --- The CC lanes: a bar each, its number beneath ---
    let row = Rect::from_min_max(Pos2::new(rect.left() + 6.0 * z, rect.bottom() - 26.0 * z), Pos2::new(rect.right() - 40.0 * z, rect.bottom() - 13.0 * z));
    let pitch = row.width() / CC_LANES as f32;
    for lane in 0..CC_LANES {
        let value = values[MidiOut::READOUT_CC + lane];
        let patched = value >= 0.0;
        let view = Lane::read(graph, node_id, lane);
        let slot = Rect::from_min_size(Pos2::new(row.left() + pitch * lane as f32 + 2.0 * z, row.top()), Vec2::new(pitch - 4.0 * z, 5.0 * z));
        let column = Rect::from_min_max(slot.min - Vec2::splat(1.0 * z), Pos2::new(slot.right() + 1.0 * z, row.bottom()));
        let lane_response = ui.interact(column, Id::unique(("midi_out_lane", node_id, lane)), Sense::click());
        if lane_response.hovered() {
            painter.rect_filled(column, 2.0 * z, Color32::from_white_alpha(6));
        }
        painter.rect(slot, 2.0 * z, theme::background::MAIN, Stroke::new(0.5 * z, theme::background::GRID_MAJOR), egui::StrokeKind::Middle);
        if patched {
            let fill = Rect::from_min_size(slot.min, Vec2::new(slot.width() * value / 127.0, slot.height())).shrink(0.8 * z);
            if fill.width() > 0.2 * z {
                painter.rect_filled(fill, 1.5 * z, orange);
            }
        }
        painter.text(
            Pos2::new(slot.center().x, slot.bottom() + 1.5 * z),
            egui::Align2::CENTER_TOP,
            view.label(),
            tiny.clone(),
            if patched { orange } else { theme::text::DISABLED },
        );
        let tip = match view.mode {
            CcMode::Bits7 => format!("CC {} lane: sends {} wherever its cable moves", lane + 1, view.label()),
            CcMode::Bits14 => format!("CC {} lane: sends {}, coarse then fine, 16,384 steps", lane + 1, view.label()),
            CcMode::Nrpn => format!("CC {} lane: sends {} (MSB:LSB), with 14 bits of value", lane + 1, view.label()),
        };
        let lane_response = lane_response.on_hover_text(format!("{tip}\nRight-click for 7-bit, 14-bit or NRPN"));
        let menu = crate::widgets::menu::context_menu(&lane_response, |ui| lane_menu(ui, node_id, lane, &view, &mut asked));
        if menu.is_some() {
            user_state.widget_context_menu_open = true;
        }
    }

    // --- The clock: four beat lamps, lit in turn while the port sends it ---
    let clock = MIDI.clock(port);
    let ticking = live && clock != ClockMode::Off && values[MidiOut::READOUT_PLAYING] > 0.5;
    let beat = values[MidiOut::READOUT_BEAT].max(0.0) as usize;
    let clock_y = row.top() + 2.5 * z;
    for i in 0..4 {
        let at = Pos2::new(rect.right() - 30.0 * z + i as f32 * 7.0 * z, clock_y);
        let on = ticking && beat % 4 == i;
        let color = if on {
            if i == 0 { accent } else { theme::text::SECONDARY }
        } else {
            theme::background::GRID_MAJOR
        };
        painter.circle_filled(at, if on { 2.6 * z } else { 1.8 * z }, color);
    }
    painter.text(
        Pos2::new(rect.right() - 19.5 * z, clock_y + 4.0 * z),
        egui::Align2::CENTER_TOP,
        "clock",
        tiny,
        if clock != ClockMode::Off { theme::text::SECONDARY } else { theme::text::DISABLED },
    );

    let dropped = values[MidiOut::READOUT_DROPPED] > 0.0;
    // The port's budget full: its CC streams thinning (#439)
    let thinned = MIDI.traffic(port).thinned;
    let full = live && MIDI.budget(port) > 0 && since(ui, Id::unique(("midi_out_thinned", port)), thinned, now) < FULL_FOR;
    if full {
        painter.circle_stroke(lamp_at, 4.2 * z, Stroke::new(0.8 * z, amber));
        crate::app::pace::animate(ui.ctx());
    }
    response.on_hover_text(match status {
        // The browser holds the sound, and so MIDI Out, until a click
        None => "MIDI Out sends once the sound starts".to_string(),
        Some(PortStatus::NoDevice) | Some(PortStatus::Idle) if device.is_none() => {
            format!("Port {} has no device yet: click its name to choose one", PORT_LABELS[port])
        }
        Some(PortStatus::Missing) => "The device isn't plugged in. Notes go out again once it is".to_string(),
        Some(PortStatus::Failed) => "The device wouldn't open. Another app may be using it: close that, and Soba tries again".to_string(),
        _ if dropped => "Some messages were lost: the patch sent more than the device could take".to_string(),
        _ if full => format!(
            "Port {} is sending as fast as its budget allows ({} a second, in the Rig): CC streams are thinned to their newest values, and notes and clock keep their time",
            PORT_LABELS[port],
            MIDI.budget(port)
        ),
        _ => format!(
            "Playing port {} on channel {channel}: gates and pitch as notes, bend and pressure, the CC lanes as controllers{}",
            PORT_LABELS[port],
            match clock {
                ClockMode::Off => ". The port sends no clock: the Rig (Ctrl+Shift+H) turns it on",
                ClockMode::Transport => ". The port sends the Clock as MIDI clock, with Start and Stop",
                ClockMode::ClockOnly => ". The port sends the Clock's ticks, without Start and Stop",
            }
        ),
    });
    asked
}

/// A lane's right-click menu: how it sends, and which NRPN.
fn lane_menu(ui: &mut egui::Ui, node_id: NodeId, lane: usize, view: &Lane, asked: &mut Vec<SynthResponse>) {
    let n = lane + 1;
    let set = |asked: &mut Vec<SynthResponse>, param_name: String, value: f32| asked.push(SynthResponse::ParameterChanged { node_id, param_name, value });
    ui.label(egui::RichText::new(format!("CC {n} lane · {}", view.label())).small().weak());
    let tips = [
        "One CC, 0 to 127: what most gear listens for",
        "Two CCs, the number and 32 above it, for 16,384 steps: for the controllers 0 to 31",
        "A Non-Registered Parameter, chosen by number, with 14 bits of value",
    ];
    for (index, mode) in CcMode::ALL.into_iter().enumerate() {
        if ui.selectable_label(view.mode == mode, CcMode::LABELS[index]).on_hover_text(tips[index]).clicked() {
            set(asked, format!("CC Mode {n}"), index as f32);
        }
    }
    match view.mode {
        CcMode::Bits14 if view.number > 31 => {
            ui.separator();
            ui.label(egui::RichText::new(format!("CC {} has no fine half: this sends CC 31 and 63. Choose 0 to 31 on its knob", view.number)).small().color(theme::accent::WARNING));
        }
        CcMode::Nrpn => {
            ui.separator();
            let (mut msb, mut lsb) = ((view.nrpn >> 7) as u32, (view.nrpn & 0x7F) as u32);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("NRPN");
                changed |= ui.add(egui::DragValue::new(&mut msb).range(0..=127).prefix("MSB ")).changed();
                changed |= ui.add(egui::DragValue::new(&mut lsb).range(0..=127).prefix("LSB ")).changed();
            });
            if changed {
                set(asked, format!("NRPN {n}"), (msb * 128 + lsb) as f32);
            }
            ui.label(egui::RichText::new(format!("Number {} · as the device's manual lists it", msb * 128 + lsb)).small().weak());
        }
        _ => {}
    }
}

/// The menu a click on the device's name opens: every output found, or none.
pub(super) fn device_menu(ui: &mut egui::Ui, picker: &egui::Response, port: usize, current: Option<&str>, found: Option<&str>) {
    let popup = picker.id.with("menu");
    if picker.clicked() {
        MIDI.rescan();
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    egui::Popup::from_response(picker).id(popup).open_memory(None).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
        ui.set_min_width(160.0);
        ui.label(egui::RichText::new(format!("Port {} plays", PORT_LABELS[port])).small().weak());
        // In the browser, the devices wait for Connect MIDI
        #[cfg(target_arch = "wasm32")]
        {
            use crate::engine::web_midi::{self, Access};
            match web_midi::access() {
                Access::Allowed => {}
                Access::Unsupported => {
                    ui.label(egui::RichText::new("This browser has no Web MIDI").weak());
                    return;
                }
                access => {
                    let button = ui.add_enabled(access != Access::Asking, egui::Button::new("🎹 Connect MIDI"));
                    if button.clicked() {
                        web_midi::request(ui.ctx());
                    }
                    return;
                }
            }
        }
        let devices = MIDI.devices();
        if devices.is_empty() {
            ui.label(egui::RichText::new("No MIDI outputs found").weak());
        }
        for device in &devices {
            if ui.selectable_label(found == Some(device.as_str()), device).clicked() {
                MIDI.choose(port, Some(device));
            }
        }
        // Chosen before, on another day, and not plugged in now
        if let Some(missing) = current.filter(|_| found.is_none()) {
            let _ = ui.selectable_label(true, format!("{missing} (unplugged)"));
        }
        ui.separator();
        if ui.selectable_label(current.is_none(), "None").clicked() {
            MIDI.choose(port, None);
        }
        let keys = crate::app::shortcuts::keys(ui.ctx(), "Ctrl+Shift+H");
        ui.label(egui::RichText::new(format!("Every port: the Rig ({keys})")).small().weak());
    });
}

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// How bright a flash is, 0 to 1: full when `count` has moved since the
/// last frame, fading over `length` seconds.
pub(super) fn flash_of(ui: &egui::Ui, id: Id, count: u32, now: f64, length: f64) -> f32 {
    (1.0 - since(ui, id, count, now) / length).clamp(0.0, 1.0) as f32
}

/// Seconds since `count` last moved, as the frames have seen it: none
/// passed while it hasn't moved since the face first looked.
fn since(ui: &egui::Ui, id: Id, count: u32, now: f64) -> f64 {
    let (seen, at) = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || (count, f64::NEG_INFINITY)));
    if seen != count {
        ui.ctx().data_mut(|d| d.insert_temp(id, (count, now)));
        return 0.0;
    }
    now - at
}
