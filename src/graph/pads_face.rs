//! The Pads module's face (#428): eight pads in two rows of four, lit as
//! they're hit.
//!
//! A hit flashes its pad in the Gate green, brighter the harder it was
//! struck, and the flash fades over a third of a second. A pad whose gate
//! is open (held, or latched) stays softly lit, a little brighter the
//! harder it's pressed. Each pad wears its note quietly in a corner.
//!
//! Press a pad on the face to play it: the harder toward its top. Learn,
//! along the top, takes pad 1's note and channel from the next pad hit on
//! the controller, and pads 2 to 8 follow. Right-click a pad to learn it
//! on its own, give it back to the row, or choose the channel. While a pad
//! learns, it blinks in MIDI purple.

use std::sync::OnceLock;

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use egui_node_graph2::NodeId;

use crate::app::module_learn::{LearnKinds, ModuleLearn};
use crate::app::theme;
use crate::dsp::{DspModule, NoteClaims};
use crate::modules::pads::{self, Listen, Mode, FOLLOW, PADS};
use crate::modules::Pads;
use crate::persistence::MidiControl;

use super::display_frame;
use super::sampler_take::MIN_PRESS_SECONDS;
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// Pads in a row.
const COLUMNS: usize = 4;
/// A pad's height, before zoom.
const PAD_HEIGHT: f32 = 34.0;
/// The gap between pads, before zoom.
const GAP: f32 = 5.0;
/// The strip along the top, before zoom.
const TOP: f32 = 15.0;
/// How long a hit's flash takes to fade, in seconds: its time constant.
const FLASH: f32 = 0.12;
/// How soft the softest press on the face is, at a pad's foot.
const SOFTEST_PRESS: f32 = 0.35;

/// The Pads module's parameter names and defaults, in its order.
fn parameters() -> &'static [(&'static str, f32)] {
    static NAMES: OnceLock<Vec<(&'static str, f32)>> = OnceLock::new();
    NAMES.get_or_init(|| Pads::new().parameters().iter().map(|p| (p.name, p.default)).collect())
}

/// A Pads node's parameters, in the module's order, as the node holds them.
pub fn params_of(graph: &SynthGraph, node_id: NodeId) -> Vec<f32> {
    parameters().iter().map(|&(name, default)| display_frame::value_of(graph, node_id, name).unwrap_or(default)).collect()
}

/// The notes every Pads module in `graph` claims (#428), for the UI to
/// leave off the pianos and their ears as the engine leaves them off Note
/// In: each from its Device (#454), given the slot of each input open.
pub fn note_claims(graph: &SynthGraph, slot_of: impl Fn(&str) -> Option<u8>) -> NoteClaims {
    let mut claims = NoteClaims::EMPTY;
    for (node_id, node) in graph.nodes.iter() {
        if node.user_data.module_id == pads::ID {
            let device = super::midi_device::listen_to(super::midi_device::device(&node.user_data), &slot_of);
            pads::claim_notes(&params_of(graph, node_id), device, &mut claims);
        }
    }
    claims
}

/// A note's name, as the pianos have it: 36 is C2.
pub fn note_name(note: u8) -> String {
    let names = crate::modules::quantizer::NOTE_NAMES;
    format!("{}{}", names[note as usize % 12], note as i32 / 12 - 1)
}

/// A channel as a pad shows it.
fn channel_label(channel: u8) -> String {
    match channel {
        0 => "omni".to_string(),
        channel => format!("ch {channel}"),
    }
}

/// The learn that teaches pad `pad` the next note it hears, and its
/// channel: pad 1's are the Base and Channel the rest follow.
pub fn learn(pad: usize) -> ModuleLearn {
    ModuleLearn {
        kinds: LearnKinds { note: Some(0.0), ..LearnKinds::NONE },
        kind: None,
        number: Some(Pads::note_param(pad)),
        channel: Some(Pads::channel_param(pad)),
        encoding: None,
    }
}

/// Draws the pads, and returns what a press, Learn or a pad's menu asked for.
pub fn pads_face(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &mut SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut asked = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return asked };
    let name = node.user_data.display_name.clone();
    let green = theme::signal::GATE;
    let width = display_frame::knob_row_width(ui, COLUMNS, z);
    let rows = PADS.div_ceil(COLUMNS);
    let height = (TOP + rows as f32 * (PAD_HEIGHT + GAP)) * z;
    let Some((rect, _, painter)) = display_frame::frame_of_height(ui, width, height, green, z) else { return asked };

    let params = params_of(graph, node_id);
    let listens = pads::listens(&params);
    let mode = Mode::from_param(params[Pads::PARAM_MODE]);
    let readout = display_frame::readout(user_state, node_id);
    let engine_id = user_state.get_engine_node_id(node_id);
    let now = ui.input(|i| i.time);
    let blink = 0.4 + 0.6 * (now * 4.0).sin().abs() as f32;
    let small = FontId::proportional(7.0 * z);
    let tiny = FontId::proportional(6.0 * z);

    // Along the top: the channel the row hears, and Learn
    let learning_any = engine_id.is_some_and(|eid| (0..PADS).any(|pad| user_state.is_midi_learn_target(eid, Pads::note_param(pad))));
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), TOP * z));
    painter.text(pos2(top.left() + 6.0 * z, top.center().y), Align2::LEFT_CENTER, channel_label(listens[0].channel), small.clone(), theme::text::QUIET);
    let learn_rect = Rect::from_min_max(pos2(top.right() - 40.0 * z, top.top()), top.right_bottom());
    let learn_button = ui.interact(learn_rect, ui.scope_id().with(("pads_learn", node_id)), Sense::click());
    let (learn_text, learn_ink) = if learning_any {
        ("cancel", theme::signal::MIDI.gamma_multiply(blink))
    } else if learn_button.hovered() {
        ("learn", theme::signal::MIDI)
    } else {
        ("learn", theme::text::QUIET)
    };
    painter.text(pos2(top.right() - 6.0 * z, top.center().y), Align2::RIGHT_CENTER, learn_text, small.clone(), learn_ink);
    let learn_tip = if learning_any {
        "Click to stop learning"
    } else {
        "Learn the pads: hit pad 1 on your controller, and pads 2 to 8 follow on the next seven notes"
    };
    if learn_button.on_hover_text(learn_tip).clicked() {
        if learning_any {
            asked.push(SynthResponse::MidiLearnCancel);
        } else if let Some(eid) = engine_id {
            asked.push(learn_start(eid, &name, 0));
        }
    }
    if let Some(mode_word) = match mode {
        Mode::Momentary => None,
        Mode::Latch => Some("latch"),
        Mode::Trigger => Some("trigger"),
    } {
        painter.text(top.center(), Align2::CENTER_CENTER, mode_word, tiny.clone(), theme::text::DISABLED);
    }

    let inner = Rect::from_min_max(pos2(rect.left() + 5.0 * z, top.bottom()), pos2(rect.right() - 5.0 * z, rect.bottom() - GAP * z));
    let pad_width = (inner.width() - GAP * z * (COLUMNS - 1) as f32) / COLUMNS as f32;
    for pad in 0..PADS {
        let (column, row) = (pad % COLUMNS, pad / COLUMNS);
        let cell = Rect::from_min_size(
            pos2(inner.left() + column as f32 * (pad_width + GAP * z), inner.top() + row as f32 * (PAD_HEIGHT + GAP) * z),
            vec2(pad_width, PAD_HEIGHT * z),
        );
        let id = ui.scope_id().with(("pads_pad", node_id, pad));
        let response = ui.interact(cell, id, Sense::click_and_drag());
        let listen = listens[pad];
        let learning = engine_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, Pads::note_param(pad)));

        // What it's doing, from the readout
        let value = |at: usize| readout.map_or(0.0, |r| r.values[at + pad]);
        let open = value(Pads::READOUT_GATE) > 0.5;
        let velocity = value(Pads::READOUT_VELOCITY);
        let quiet = readout.map_or(f32::MAX, |r| r.values[Pads::READOUT_QUIET + pad]);
        let pressure = value(Pads::READOUT_PRESSURE);
        let flash = velocity.powf(0.8) * (-quiet / FLASH).exp();
        if flash > 0.01 {
            crate::app::pace::animate(ui.ctx());
        }

        draw_pad(&painter, cell, z, PadLight { flash, open, pressure, hovered: response.hovered(), learning: learning.then_some(blink) });

        // Its note, quietly, and its own channel if it has one
        let note_ink = if flash > 0.2 { Color32::from_rgb(20, 40, 28) } else { theme::text::DISABLED };
        let note = listen.note.map_or("·".to_string(), note_name);
        painter.text(cell.left_bottom() + vec2(3.5 * z, -3.0 * z), Align2::LEFT_BOTTOM, note, tiny.clone(), note_ink);
        if listen.own && listen.channel != listens[0].channel {
            painter.text(cell.right_bottom() + vec2(-3.5 * z, -3.0 * z), Align2::RIGHT_BOTTOM, channel_label(listen.channel), tiny.clone(), note_ink);
        }
        if learning {
            painter.text(cell.center(), Align2::CENTER_CENTER, "hit a pad", tiny.clone(), theme::signal::MIDI.gamma_multiply(blink));
            crate::app::pace::animate(ui.ctx());
        }

        // In Map Mode a click learns the pad instead of playing it (#426)
        let learn = engine_id.map(|eid| learn_start(eid, &name, pad));
        let caught = learn.as_ref().and_then(|learn| super::map_mode::catch(ui, cell, super::map_mode::Outline::around(cell, z), learn, user_state));
        let in_map_mode = caught.is_some();
        let response = match caught {
            Some(caught) => {
                asked.extend(caught.learn);
                caught.response
            }
            None => response,
        };

        // Pressed on the face: down for at least MIN_PRESS_SECONDS, as
        // hard as how near its top
        let press = Pads::press_name(pad);
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

        let tip = format!(
            "Pad {} · {}{}\nPress it to play it, harder toward the top\nRight-click to learn it from your controller",
            pad + 1,
            listen.note.map_or("no note".to_string(), |note| format!("{} ({note})", note_name(note))),
            format_args!(" on {}", channel_label(listen.channel)),
        );
        let response = response.on_hover_text(tip);
        let menu = crate::widgets::menu::context_menu(&response, |ui| {
            pad_menu(ui, node_id, engine_id, &name, pad, &listen, learning, &mut asked);
        });
        if menu.is_some() {
            user_state.widget_context_menu_open = true;
        }
    }
    asked
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
    /// The blink while it learns.
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

/// The response that starts pad `pad`'s learn.
fn learn_start(engine_node_id: u64, name: &str, pad: usize) -> SynthResponse {
    SynthResponse::MidiLearnStart {
        engine_node_id,
        param_index: Pads::note_param(pad),
        param_name: if pad == 0 { format!("{name}, pad 1 (pads 2 to 8 follow)") } else { format!("{name}, pad {}", pad + 1) },
        min_value: 0.0,
        max_value: 1.0,
        control: MidiControl::Button,
        scene: None,
        into: Some(Box::new(learn(pad))),
    }
}

/// A pad's right-click menu: learn it, give it back to the row, its note
/// and its channel.
#[allow(clippy::too_many_arguments)]
fn pad_menu(ui: &mut egui::Ui, node_id: NodeId, engine_id: Option<u64>, name: &str, pad: usize, listen: &Listen, learning: bool, asked: &mut Vec<SynthResponse>) {
    let set = |asked: &mut Vec<SynthResponse>, param: usize, value: f32| {
        let param_name = parameters()[param].0.to_string();
        asked.push(SynthResponse::ParameterChanged { node_id, param_name, value });
    };
    let note = listen.note.map_or("no note".to_string(), |note| format!("{} ({note})", note_name(note)));
    ui.label(egui::RichText::new(format!("Pad {} · {note} · {}", pad + 1, channel_label(listen.channel))).small().weak());
    if learning {
        if ui.button("Cancel MIDI Learn").clicked() {
            asked.push(SynthResponse::MidiLearnCancel);
            ui.close();
        }
    } else if let Some(eid) = engine_id {
        let (label, tip) = if pad == 0 {
            ("Learn MIDI", "Hit pad 1 on your controller: pads 2 to 8 follow on the next seven notes")
        } else {
            ("Learn MIDI for this pad", "Hit a pad on your controller: this pad takes its note and channel, and keeps them")
        };
        if ui.button(label).on_hover_text(tip).clicked() {
            asked.push(learn_start(eid, name, pad));
            ui.close();
        }
    }
    if pad > 0 && listen.own && ui.button("Follow pad 1").on_hover_text("Forget this pad's own note and channel: it follows pad 1 again").clicked() {
        set(asked, Pads::note_param(pad), FOLLOW);
        set(asked, Pads::channel_param(pad), FOLLOW);
        ui.close();
    }
    ui.separator();

    let mut number = listen.note.unwrap_or(127) as u32;
    ui.horizontal(|ui| {
        ui.label(if pad == 0 { "Note (pad 1)" } else { "Note" });
        if ui.add(egui::DragValue::new(&mut number).range(0..=127)).changed() {
            set(asked, Pads::note_param(pad), number as f32);
        }
    });
    let channel_tip = if pad == 0 { "The channel every pad that follows pad 1 hears" } else { "The channel this pad hears" };
    crate::widgets::menu::menu_button(ui, "Channel", |ui| {
        if ui.selectable_label(listen.channel == 0, "Omni").on_hover_text("Hear every channel").clicked() {
            set(asked, Pads::channel_param(pad), 0.0);
        }
        egui::Grid::new(("pads_channels", node_id, pad)).spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            for channel in 1..=16u8 {
                if ui.selectable_label(listen.channel == channel, format!("{channel:>2}")).on_hover_text(channel_tip).clicked() {
                    set(asked, Pads::channel_param(pad), channel as f32);
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
    fn a_pad_learns_into_its_own_params_and_pad_one_into_the_rows() {
        let first = learn(0);
        assert_eq!(first.number, Some(Pads::PARAM_BASE));
        assert_eq!(first.channel, Some(Pads::PARAM_CHANNEL));
        let fifth = learn(4);
        assert_eq!(fifth.number, Some(Pads::note_param(4)));
        assert_eq!(fifth.channel, Some(Pads::channel_param(4)));
        assert!(fifth.kinds.note.is_some() && fifth.kinds.cc.is_none() && fifth.kinds.bend.is_none());
        assert_eq!(fifth.prompt(), "Hit a pad or key");
        // Learn writes 1-16, as the module reads them
        let heard = fifth.hear(&crate::engine::MidiEvent::NoteOn { channel: 9, note: 40, velocity: 99 }).unwrap();
        let mut params: Vec<f32> = parameters().iter().map(|&(_, default)| default).collect();
        for (param, value) in heard {
            params[param] = value;
        }
        assert_eq!(pads::listens(&params)[4], Listen { note: Some(40), channel: 10, own: true });
    }

    #[test]
    fn notes_are_named_as_the_pianos_name_them() {
        assert_eq!(note_name(36), "C2");
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(42), "F#2");
        assert_eq!(parameters()[Pads::press_param(0)].0, Pads::press_name(0));
    }
}
