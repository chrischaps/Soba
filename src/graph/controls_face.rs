//! The Controls module's face (#427): a slim fader per slot, moving with
//! the hardware.
//!
//! Each slot is a track in the Control orange, filled from the bottom, or
//! from the middle when its range is bipolar, with a cap where the control
//! is (as the cable carries it, slew and all). The cap warms when the
//! slot hears its control and cools again as it rests. Under the track,
//! what it hears: "CC 74", "Bend", "Mod", and its channel.
//!
//! Right-click a slot to learn its control from the hardware, or to choose
//! what it hears, its channel, its range and whether it's an endless
//! encoder. While it learns, its label blinks in MIDI purple.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use egui_node_graph2::NodeId;

use crate::app::module_learn::{LearnKinds, ModuleLearn};
use crate::app::theme;
use crate::modules::controls::{self, Range, Source, ENCODING_LABELS, SLOTS};
use crate::modules::Controls;
use crate::persistence::{MidiControl, MidiEncoding};

use super::display_frame;
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// The face's height, before zoom.
const HEIGHT: f32 = 118.0;
/// Room above the tracks for the slot letters, before zoom.
const TOP: f32 = 22.0;
/// Room below them for what each hears, before zoom.
const BOTTOM: f32 = 24.0;
/// A track's width, before zoom.
const TRACK: f32 = 3.0;
/// A cap's size, before zoom.
const CAP: egui::Vec2 = vec2(14.0, 4.5);
/// How long a cap stays warm after its control moves, in seconds.
const WARM: f32 = 0.6;

/// A slot's settings, read off the node.
struct SlotView {
    source: Source,
    cc: u8,
    channel: u8,
    encoding: MidiEncoding,
    range: Range,
}

impl SlotView {
    fn read(graph: &SynthGraph, node_id: NodeId, slot: usize) -> Self {
        let letter = Controls::letter(slot);
        let value = |name: &str| display_frame::value_of(graph, node_id, &format!("{name} {letter}")).unwrap_or(0.0);
        Self {
            source: Source::from_param(value("Source")),
            cc: value("CC").round().clamp(0.0, 127.0) as u8,
            channel: value("Channel").round().clamp(0.0, 16.0) as u8,
            encoding: controls::encoding_from_param(value("Encoding")),
            range: Range::from_param(value("Range")),
        }
    }

    fn label(&self) -> String {
        Controls::source_label(self.source, self.cc)
    }

    fn channel_label(&self) -> String {
        match self.channel {
            0 => "omni".to_string(),
            channel => format!("ch {channel}"),
        }
    }

    fn bipolar(&self) -> bool {
        self.range.bipolar(self.source)
    }

    /// What it hears, in words, for its tooltip and menu.
    fn describe(&self) -> String {
        let what = match self.source {
            Source::Cc if self.cc == 1 => "The mod wheel (CC 1)".to_string(),
            Source::Cc => format!("CC {}", self.cc),
            Source::Bend => "The pitch bend".to_string(),
            Source::ModWheel => "The mod wheel (CC 1)".to_string(),
            Source::Pressure => "Channel pressure".to_string(),
        };
        let channel = match self.channel {
            0 => "on any channel".to_string(),
            channel => format!("on channel {channel}"),
        };
        let encoder = match (self.source, self.encoding) {
            (Source::Cc, MidiEncoding::Relative64 | MidiEncoding::TwosComplement) => ", an endless encoder",
            _ => "",
        };
        let range = if self.bipolar() { "-1 to 1" } else { "0 to 1" };
        format!("{what} {channel}{encoder}, as {range}")
    }
}

/// The learn that teaches slot `slot` the next control it hears: a CC, the
/// pitch bend or the pressure, on its channel (#427).
pub fn learn(slot: usize) -> ModuleLearn {
    ModuleLearn {
        kinds: LearnKinds {
            cc: Some(Source::Cc as u8 as f32),
            bend: Some(Source::Bend as u8 as f32),
            pressure: Some(Source::Pressure as u8 as f32),
            ..LearnKinds::NONE
        },
        kind: Some(Controls::source_param(slot)),
        number: Some(Controls::cc_param(slot)),
        channel: Some(Controls::channel_param(slot)),
        encoding: Some(Controls::encoding_param(slot)),
    }
}

/// Draws the faders, and returns what a slot's menu asked for.
pub fn controls_face(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &mut SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut asked = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return asked };
    let name = node.user_data.display_name.clone();
    let accent = theme::signal::CONTROL;
    let width = display_frame::knob_row_width(ui, SLOTS, z);
    let Some((rect, _, painter)) = display_frame::frame_of_height(ui, width, HEIGHT * z, accent, z) else { return asked };

    let readout = display_frame::readout(user_state, node_id);
    let engine_id = user_state.get_engine_node_id(node_id);
    let now = ui.input(|i| i.time);
    let letter_font = FontId::proportional(7.5 * z);
    let label_font = FontId::proportional(7.5 * z);
    let small = FontId::proportional(6.5 * z);
    let track_ink = Color32::from_rgb(38, 40, 54);
    let tick_ink = Color32::from_rgb(52, 55, 72);

    let inner = rect.shrink2(vec2(4.0 * z, 0.0));
    let pitch = inner.width() / SLOTS as f32;
    for slot in 0..SLOTS {
        let view = SlotView::read(graph, node_id, slot);
        let column = Rect::from_min_size(pos2(inner.left() + pitch * slot as f32, inner.top()), vec2(pitch, inner.height()));
        let response = ui.interact(column, ui.scope_id().with(("controls_slot", node_id, slot)), Sense::click());
        let learning = engine_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, Controls::source_param(slot)));
        let patched = display_frame::output_patched(graph, user_state, node_id, Controls::letter(slot));

        if response.hovered() || learning {
            painter.rect_filled(column.shrink(1.5 * z), 3.0 * z, Color32::from_white_alpha(6));
        }

        // Where it is, and how lately it moved
        let rest = if view.source == Source::Bend { 0.5 } else { 0.0 };
        let travel = readout.map_or(rest, |r| r.values[Controls::READOUT_TRAVEL + slot]).clamp(0.0, 1.0);
        let quiet = readout.map_or(f32::MAX, |r| r.values[Controls::READOUT_QUIET + slot]);
        let warmth = (-quiet / WARM).exp();
        if warmth > 0.01 {
            crate::app::pace::animate(ui.ctx());
        }

        // The letter along the top, lit when a cable leaves it
        let x = column.center().x;
        let letter_ink = if patched { accent.gamma_multiply(0.9) } else { theme::text::QUIET };
        painter.text(pos2(x, column.top() + 4.0 * z), Align2::CENTER_TOP, Controls::letter(slot), letter_font.clone(), letter_ink);

        // The track, with a tick every quarter and a longer one at the middle
        let track = Rect::from_min_max(pos2(x - TRACK * 0.5 * z, column.top() + TOP * z), pos2(x + TRACK * 0.5 * z, column.bottom() - BOTTOM * z));
        let y_of = |t: f32| track.bottom() - t * track.height();
        for quarter in 0..=4 {
            let y = y_of(quarter as f32 / 4.0);
            let reach = if quarter == 2 && view.bipolar() { 6.0 } else { 3.0 };
            painter.line_segment([pos2(track.left() - (2.0 + reach) * z, y), pos2(track.left() - 2.0 * z, y)], Stroke::new(0.75 * z, tick_ink));
        }
        painter.rect_filled(track, TRACK * 0.5 * z, track_ink);
        let (from, to) = if view.bipolar() { (y_of(0.5), y_of(travel)) } else { (track.bottom(), y_of(travel)) };
        let fill = Rect::from_x_y_ranges(track.x_range(), from.min(to)..=from.max(to));
        if fill.height() > 0.25 * z {
            painter.rect_filled(fill.expand2(vec2(2.5 * z, 0.0)), 3.0 * z, accent.gamma_multiply(0.06 + 0.14 * warmth));
            painter.rect_filled(fill, TRACK * 0.5 * z, accent.gamma_multiply(0.55 + 0.35 * warmth));
        }

        // The cap, warm while the control moves
        let cap = Rect::from_center_size(pos2(x, y_of(travel)), CAP * z);
        if warmth > 0.01 {
            painter.rect_filled(cap.expand2(vec2(4.0 * z, 3.5 * z)), 5.0 * z, accent.gamma_multiply(0.20 * warmth));
        }
        painter.rect_filled(cap.expand(0.75 * z), 2.5 * z, Color32::from_rgb(14, 15, 21));
        let cap_ink = lerp(Color32::from_rgb(178, 180, 192), Color32::from_rgb(255, 226, 190), warmth);
        painter.rect_filled(cap, 2.0 * z, cap_ink);
        painter.line_segment([pos2(cap.left() + 3.0 * z, cap.center().y), pos2(cap.right() - 3.0 * z, cap.center().y)], Stroke::new(0.75 * z, Color32::from_rgb(90, 92, 104)));

        // What it hears, and on which channel; blinking while it learns
        let label_y = column.bottom() - (BOTTOM - 4.0) * z;
        if learning {
            let alpha = 0.4 + 0.6 * (now * 4.0).sin().abs() as f32;
            painter.text(pos2(x, label_y), Align2::CENTER_TOP, "learn", label_font.clone(), theme::signal::MIDI.gamma_multiply(alpha));
            painter.text(pos2(x, label_y + 9.0 * z), Align2::CENTER_TOP, "move it", small.clone(), theme::text::QUIET.gamma_multiply(alpha));
            painter.circle_filled(pos2(column.right() - 5.0 * z, column.top() + 6.5 * z), 2.2 * z, theme::signal::MIDI.gamma_multiply(alpha));
            crate::app::pace::animate(ui.ctx());
        } else {
            painter.text(pos2(x, label_y), Align2::CENTER_TOP, view.label(), label_font.clone(), theme::text::SECONDARY);
            painter.text(pos2(x, label_y + 9.0 * z), Align2::CENTER_TOP, view.channel_label(), small.clone(), theme::text::DISABLED);
        }

        let value = if view.bipolar() { travel * 2.0 - 1.0 } else { travel };
        let tip = format!(
            "{} {}: {}\nNow {value:.2}\nRight-click to learn it from your controller, or choose what it hears",
            name,
            Controls::letter(slot),
            view.describe(),
        );
        let response = response.on_hover_text(tip);
        let menu = crate::widgets::menu::context_menu(&response, |ui| {
            slot_menu(ui, node_id, engine_id, &name, slot, &view, learning, &mut asked);
        });
        if menu.is_some() {
            user_state.widget_context_menu_open = true;
        }
    }
    asked
}

/// A slot's right-click menu: Learn, then what it hears, its channel, its
/// range and its hardware.
#[allow(clippy::too_many_arguments)]
fn slot_menu(ui: &mut egui::Ui, node_id: NodeId, engine_id: Option<u64>, name: &str, slot: usize, view: &SlotView, learning: bool, asked: &mut Vec<SynthResponse>) {
    let letter = Controls::letter(slot);
    let set = |asked: &mut Vec<SynthResponse>, param: &str, value: f32| {
        asked.push(SynthResponse::ParameterChanged { node_id, param_name: format!("{param} {letter}"), value });
    };
    ui.label(egui::RichText::new(format!("{letter} · {} · {}", view.label(), view.channel_label())).small().weak());
    if learning {
        if ui.button("Cancel MIDI Learn").clicked() {
            asked.push(SynthResponse::MidiLearnCancel);
            ui.close();
        }
    } else if let Some(engine_node_id) = engine_id {
        let tip = "Move a fader or knob, bend, or press a key, on your MIDI controller";
        if ui.button("Learn MIDI").on_hover_text(tip).clicked() {
            asked.push(SynthResponse::MidiLearnStart {
                engine_node_id,
                param_index: Controls::source_param(slot),
                param_name: format!("{name} {letter}"),
                min_value: 0.0,
                max_value: 1.0,
                control: MidiControl::Knob,
                scene: None,
                into: Some(Box::new(learn(slot))),
            });
            ui.close();
        }
    }
    ui.separator();

    crate::widgets::menu::menu_button(ui, "Source", |ui| {
        for (index, label) in Source::LABELS.iter().enumerate() {
            let source = Source::from_param(index as f32);
            let chosen = view.source == source || (source == Source::ModWheel && view.source == Source::Cc && view.cc == 1);
            if ui.selectable_label(chosen, *label).clicked() {
                set(asked, "Source", index as f32);
            }
        }
        if view.source == Source::Cc {
            ui.separator();
            let mut cc = view.cc as u32;
            ui.horizontal(|ui| {
                ui.label("CC number");
                if ui.add(egui::DragValue::new(&mut cc).range(0..=127)).changed() {
                    set(asked, "CC", cc as f32);
                }
            });
        }
    });
    crate::widgets::menu::menu_button(ui, "Channel", |ui| {
        if ui.selectable_label(view.channel == 0, "Omni").on_hover_text("Hear every channel").clicked() {
            set(asked, "Channel", 0.0);
        }
        egui::Grid::new(("controls_channels", node_id, slot)).spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            for channel in 1..=16u8 {
                if ui.selectable_label(view.channel == channel, format!("{channel:>2}")).clicked() {
                    set(asked, "Channel", channel as f32);
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
            if ui.selectable_label(view.range == Range::from_param(index as f32), *label).on_hover_text(tips[index]).clicked() {
                set(asked, "Range", index as f32);
            }
        }
    });
    if view.source == Source::Cc {
        crate::widgets::menu::menu_button(ui, "Hardware", |ui| {
            let tips = [
                "It has ends, and sends where it is",
                "Endless: sends 65 for a step up, 63 for a step down",
                "Endless: sends 1 for a step up, 127 for a step down",
            ];
            for (index, label) in ENCODING_LABELS.iter().enumerate() {
                let encoding = controls::encoding_from_param(index as f32);
                if ui.selectable_label(view.encoding == encoding, *label).on_hover_text(tips[index]).clicked() {
                    set(asked, "Encoding", index as f32);
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
    fn a_slot_learns_into_its_own_params() {
        let learn = learn(2);
        assert_eq!(learn.kind, Some(Controls::source_param(2)));
        assert_eq!(learn.number, Some(Controls::cc_param(2)));
        assert_eq!(learn.channel, Some(Controls::channel_param(2)));
        assert_eq!(learn.encoding, Some(Controls::encoding_param(2)));
        assert_eq!(learn.kinds.note, None, "a note isn't a control");
        // What it writes is what the module reads
        assert_eq!(Source::from_param(learn.kinds.bend.unwrap()), Source::Bend);
        assert_eq!(Source::from_param(learn.kinds.pressure.unwrap()), Source::Pressure);
        assert_eq!(Source::from_param(learn.kinds.cc.unwrap()), Source::Cc);
    }
}
