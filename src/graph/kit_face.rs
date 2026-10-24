//! A Kit's lanes on its face (#443), between the Instrument's header and
//! its knobs: eight pads in two rows of four, each named for the part its
//! lane plays (Kick, Track 3, Pad 13) and lit as the hardware is struck,
//! as the Pads module's are. Beside them, the Velocity knob that strikes
//! lanes whose Vel jack is unpatched.
//!
//! Click a pad to choose the part its lane plays. On a device that takes
//! the same CCs on each track's channel (the Digitakt), the pad the knobs
//! turn wears an orange bar, and its menu turns them to another.

use eframe::egui::{self, pos2, vec2, Align2, FontId, Id, Rect, Sense, Stroke, UiBuilder};
use egui_node_graph2::{NodeId, NodeResponse};

use crate::app::theme;
use crate::hardware::{self, instrument, LaneMap, Part};
use crate::modules::kit::{Kit, GATE_NAMES, LANES, VEL_NAMES};

use super::display_frame;
use super::node_data::{KnobCellCtx, SynthNodeData};
use super::pads_face::{draw_pad, PadLight};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// Pads to a row.
const COLUMNS: usize = 4;
/// The strip's height (patch units): two rows of pads, as tall as a row
/// of knobs so the Velocity knob sits beside them.
pub const STRIP: f32 = 72.0;
/// The gap between pads.
const GAP: f32 = 5.0;
/// How long a strike's flash takes to fade, in seconds: its time constant.
const FLASH: f32 = 0.12;

/// What a Kit's lane jack is called: its part's name, so a Trigger
/// Sequencer's lane is patched into Kick, not Gate 1.
pub fn jack_name(data: &SynthNodeData, param_name: &str) -> Option<String> {
    let (lane, vel) = match (GATE_NAMES.iter().position(|&n| n == param_name), VEL_NAMES.iter().position(|&n| n == param_name)) {
        (Some(lane), _) => (lane, false),
        (_, Some(lane)) => (lane, true),
        _ => return None,
    };
    let part = instrument::part(&data.labels, lane, &hardware::known())?;
    Some(if vel { format!("{} Vel", part.name) } else { part.name })
}

/// Draws the lanes' pads and the Velocity knob in `width`, and returns
/// what a pad's menu asked for.
#[allow(clippy::too_many_arguments)]
pub fn lanes(data: &SynthNodeData, ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &mut SynthGraphState, width: f32, cell: f32, z: f32) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    let mut responses = Vec::new();
    let (strip, _) = ui.allocate_exact_size(vec2(width, STRIP * z), Sense::hover());
    let profiles = hardware::known();
    let maps = instrument::lane_maps(&data.labels, &profiles);
    let parts: [Option<Part>; LANES] = std::array::from_fn(|lane| instrument::part(&data.labels, lane, &profiles));
    let known = instrument::profile_name(&data.labels).and_then(|name| profiles.find(name));
    let track = display_frame::value_of(graph, node_id, "Track").unwrap_or(0.0).round().clamp(0.0, (LANES - 1) as f32) as usize;
    let channel = display_frame::value_of(graph, node_id, "Channel").unwrap_or(0.0).round() as u8;
    let readout = display_frame::readout(user_state, node_id);
    let painter = ui.painter_at(strip);
    let small = FontId::proportional(7.0 * z);
    let tiny = FontId::proportional(5.5 * z);

    // The pads, across all but the last knob cell
    let area = Rect::from_min_max(strip.min + vec2(5.0 * z, 2.0 * z), pos2(strip.right() - cell * z, strip.bottom() - 4.0 * z));
    let pad_width = (area.width() - GAP * z * (COLUMNS - 1) as f32) / COLUMNS as f32;
    let rows = LANES.div_ceil(COLUMNS);
    let pad_height = (area.height() - GAP * z * (rows - 1) as f32) / rows as f32;
    let mut lit = false;
    for lane in 0..LANES {
        let (column, row) = (lane % COLUMNS, lane / COLUMNS);
        let rect = Rect::from_min_size(pos2(area.left() + column as f32 * (pad_width + GAP * z), area.top() + row as f32 * (pad_height + GAP * z)), vec2(pad_width, pad_height));
        let response = ui.interact(rect, Id::unique(("kit_lane", node_id, lane)), Sense::click());
        let (struck, held, quiet) = readout.as_ref().map_or((0.0, false, f32::MAX), |r| Kit::lane_in(r, lane));
        let flash = struck.powf(0.8) * (-quiet / FLASH).exp();
        lit |= flash > 0.01;
        draw_pad(&painter, rect, z, PadLight { flash, open: held, pressure: struck, hovered: response.hovered(), learning: None });

        // Its part's name, and its lane's number quietly in a corner
        let ink = match (&parts[lane], flash > 0.2) {
            (None, _) => theme::text::DISABLED,
            (Some(_), true) => egui::Color32::from_rgb(20, 40, 28),
            (Some(_), false) => theme::text::SECONDARY,
        };
        let name = parts[lane].as_ref().map_or("—", |part| part.name.as_str());
        let galley = painter.layout_no_wrap(name.to_string(), small.clone(), ink);
        let at = pos2(rect.center().x - galley.size().x.min(rect.width() - 4.0 * z) / 2.0, rect.center().y - galley.size().y / 2.0);
        painter.with_clip_rect(rect.shrink(2.0 * z)).galley(at, galley, ink);
        painter.text(rect.left_top() + vec2(3.0 * z, 2.0 * z), Align2::LEFT_TOP, (lane + 1).to_string(), tiny.clone(), theme::text::DISABLED);
        let turns = maps[lane].track && lane == track;
        if turns {
            // The knobs turn this lane's track
            let bar = Rect::from_min_max(pos2(rect.left() + 4.0 * z, rect.bottom() - 3.0 * z), pos2(rect.right() - 4.0 * z, rect.bottom() - 1.5 * z));
            painter.rect_filled(bar, 1.0 * z, theme::signal::CONTROL);
        }

        let tip = match &parts[lane] {
            Some(part) => format!(
                "Lane {} plays {}{}\nClick to choose another part{}",
                lane + 1,
                describe(part, &maps[lane], channel),
                if turns { "\nThe knobs turn this track" } else { "" },
                if maps[lane].track && !turns { ", or turn the knobs to its track" } else { "" },
            ),
            None if known.is_some() => format!("Lane {} plays nothing: click to give it a part", lane + 1),
            None => format!("Lane {} plays nothing: choose a kit on the face first", lane + 1),
        };
        let response = response.on_hover_text(tip);
        let popup = response.id.with("menu");
        if response.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup);
        }
        egui::Popup::from_response(&response).id(popup).open_memory(None).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            ui.set_min_width(140.0);
            lane_menu(ui, node_id, lane, &parts[lane], &maps[lane], track, known.map(|k| k.profile.parts.as_slice()).unwrap_or_default(), &mut responses);
        });
    }
    if lit {
        crate::app::pace::animate(ui.ctx());
    }
    if parts.iter().all(Option::is_none) {
        painter.text(area.center(), Align2::CENTER_CENTER, "choose a kit", small, theme::text::DISABLED);
    }

    // The Velocity knob, in the last cell
    let knob_width = 44.0 * z;
    let centre = strip.right() - cell * z / 2.0;
    let knob_rect = Rect::from_min_size(pos2(centre - knob_width / 2.0 + 4.0 * z, strip.top()), vec2(knob_width, STRIP * z));
    if let Some(knob) = data.knob_params.iter().find(|knob| knob.param_name == "Velocity") {
        let mut cell_ui = ui.new_child(UiBuilder::new().max_rect(knob_rect).id_salt(("kit_velocity", node_id)));
        data.knob_cell(&mut cell_ui, knob, &mut KnobCellCtx::module(node_id, graph, user_state, z, &mut responses));
    }
    let divider = strip.right() - cell * z;
    painter.line_segment([pos2(divider, strip.top() + 8.0 * z), pos2(divider, strip.bottom() - 10.0 * z)], Stroke::new(1.0 * z, theme::text::DISABLED.gamma_multiply(0.3)));
    responses
}

/// What a lane strikes, as its tooltip says it: "Kick · note 36 (C2) on
/// channel 10".
fn describe(part: &Part, map: &LaneMap, channel: u8) -> String {
    let note = match part.note {
        Some(note) => format!("note {note} ({})", super::pads_face::note_name(note)),
        None => "any note".to_string(),
    };
    format!("{} · {note} on channel {}", part.name, map.channel(channel) + 1)
}

/// A pad's menu: which of the profile's parts its lane plays, and, where
/// the device takes CCs a track a channel, the knobs to its track.
#[allow(clippy::too_many_arguments)]
fn lane_menu(
    ui: &mut egui::Ui,
    node_id: NodeId,
    lane: usize,
    playing: &Option<Part>,
    map: &LaneMap,
    track: usize,
    parts: &[Part],
    responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
) {
    let heading = match playing {
        Some(part) => format!("Lane {} · {}", lane + 1, part.name),
        None => format!("Lane {} · empty", lane + 1),
    };
    ui.label(egui::RichText::new(heading).small().weak());
    if map.track && track != lane {
        if ui.selectable_label(false, "Knobs turn this track").clicked() {
            responses.push(NodeResponse::User(SynthResponse::ParameterChanged { node_id, param_name: "Track".to_string(), value: lane as f32 }));
        }
        ui.separator();
    }
    if parts.is_empty() {
        ui.label(egui::RichText::new("Choose a kit on the face to give this lane one of its parts").small().weak());
        return;
    }
    let mut give = |part: Option<&Part>| {
        let (label, text) = match part {
            Some(part) => (format!("Lane {} to {}", lane + 1, part.name), instrument::written_part(part)),
            None => (format!("Empty lane {}", lane + 1), String::new()),
        };
        responses.push(NodeResponse::User(SynthResponse::SetLabels { node_id, label, labels: vec![(instrument::lane_key(lane), text)] }));
    };
    let current = playing.as_ref().map(|part| part.name.as_str());
    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
        for part in parts {
            if ui.selectable_label(current == Some(part.name.as_str()), &part.name).clicked() {
                give(Some(part));
            }
        }
    });
    ui.separator();
    if ui.selectable_label(playing.is_none(), "Nothing").clicked() {
        give(None);
    }
}
