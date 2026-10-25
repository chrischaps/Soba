//! The FM Voice's face (#448): the voice playing, how its six operators
//! are wired and shaped, and the garden it grew in.
//!
//! - **Along the top**, a lamp, the port and its device, as an
//!   Instrument's face has them: it flashes as a voice goes out. Click the
//!   device to choose another. **Send** sends the voice again.
//! - **The voice's name**, its algorithm and feedback, and where it came
//!   from: a seed, a file, a mutant of one voice or a cross of two.
//! - **The algorithm**, drawn as the DX7 draws it: the carriers, the
//!   operators heard, along the bottom in the blue of audio, and the
//!   modulators stacked above them in the orange of modulation, each
//!   brighter the louder it is. Beside it, **each operator**: its ratio,
//!   its envelope's shape, and its output level.
//! - **The garden**: up to 32 voices, a bank's worth, in the order a bank
//!   saves them. Brighter chips are brighter voices. The playing one is
//!   ringed, with lines to the voices it came from; the one picked to
//!   cross it with is ringed green. Click a chip to play it, Shift+click or
//!   right-click to pick it.
//! - **Mutate** makes a child of the playing voice, as far from it as the
//!   Amount knob says; **Cross** a child of it and the picked one; **Keep**
//!   saves the voice from weeding once the garden's full. The menu at the
//!   right opens and saves `.syx` files and sends the garden as a bank.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke};
use egui_node_graph2::{NodeId, NodeResponse};

use crate::app::theme;
use crate::hardware::dx7::{self, op, Frequency, Target, Voice, Wiring, OPERATORS};
use crate::hardware::garden::{self, Garden, ROOM};
use crate::link::midi::{PortStatus, MIDI, PORTS, PORT_LABELS};
use crate::modules::fm_voice::FmVoice;

use super::display_frame;
use super::midi_out_display::device_menu;
use super::node_data::SynthNodeData;
use super::{SynthGraph, SynthResponse};

/// The face's width (patch units).
const WIDTH: f32 = 252.0;
/// The device line.
const DEVICE: f32 = 14.0;
/// The voice's name and where it came from.
const TITLE: f32 = 27.0;
/// The algorithm and the operators beside it.
const BODY: f32 = 68.0;
/// The algorithm's share of the body's width.
const DIAGRAM: f32 = 82.0;
/// A row of the garden: eight chips.
const GARDEN_ROW: f32 = 13.0;
const PER_ROW: usize = 8;
/// The buttons.
const ACTIONS: f32 = 22.0;
/// How long the lamp glows after a voice goes, in seconds.
const FLASH: f64 = 0.35;

/// What the face asks the app to do with a file, or the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceFile {
    /// Plant the voices of a `.syx` file in the garden.
    Open,
    /// Save the playing voice as a single-voice `.syx`.
    SaveVoice,
    /// Save the garden as a 32-voice bank.
    SaveBank,
    /// Send the garden as a bank: into the device's memories.
    SendBank,
}

/// Where node `node_id`'s voice goes: its port, its channel (from 0) and
/// what it's sent to.
pub fn where_to(graph: &SynthGraph, node_id: NodeId) -> (usize, u8, Target) {
    let value = |name: &str| display_frame::value_of(graph, node_id, name).unwrap_or(0.0).round().max(0.0);
    ((value("Port") as usize).min(PORTS - 1), (value("Channel") as u8).min(15), FmVoice::target(value("Format")))
}

/// Where the app notes the moment a node's voice went, for the lamp.
pub fn sent_id(node_id: NodeId) -> Id {
    Id::unique(("fm_voice_sent", node_id))
}

/// How tall the face is (patch units), for its garden's rows.
fn height(garden: &Garden) -> f32 {
    let rows = garden.voices.len().div_ceil(PER_ROW).max(1);
    DEVICE + TITLE + BODY + 6.0 + rows as f32 * GARDEN_ROW + ACTIONS
}

/// Draws the face and returns what it asked for.
pub fn fm_voice_face(data: &SynthNodeData, ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, zoom: f32) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    let z = zoom;
    let mut responses = Vec::new();
    let garden = Garden::of(&data.labels);
    let Some((rect, response, painter)) = display_frame::frame_of_height(ui, WIDTH * z, height(&garden) * z, theme::module::OUTPUT, z) else {
        return responses;
    };
    let playing = garden.playing();
    let voice = playing.voice;
    let (port, channel, target) = where_to(graph, node_id);
    let small = FontId::proportional(7.5 * z);
    let now = ui.input(|i| i.time);
    let mut animate = false;

    // --- The device line ---
    let status = MIDI.status(port);
    let device = MIDI.chosen(port);
    let found = device.as_ref().and(MIDI.found()[port].clone());
    let lamp = match status {
        PortStatus::Idle | PortStatus::Sending => theme::signal::MIDI,
        PortStatus::NoDevice | PortStatus::Missing => theme::accent::WARNING,
        PortStatus::Failed => theme::accent::ERROR,
    };
    let lamp = if device.is_some() { lamp } else { theme::text::DISABLED };
    let lamp_at = rect.left_top() + vec2(7.0 * z, DEVICE * z / 2.0 + 1.0 * z);
    let since = ui.ctx().data(|d| d.get_temp::<f64>(sent_id(node_id))).map_or(f64::MAX, |at| now - at);
    let flash = (1.0 - since / FLASH).clamp(0.0, 1.0) as f32;
    if flash > 0.0 {
        painter.circle_filled(lamp_at, (4.0 + 3.0 * flash) * z, theme::signal::MIDI.gamma_multiply(0.15 + 0.35 * flash));
        animate = true;
    }
    painter.circle_filled(lamp_at, 2.5 * z, lamp);
    let name = match (&device, status) {
        (None, _) => "choose a device".to_string(),
        (Some(name), PortStatus::Missing) => format!("{name} (unplugged)"),
        (Some(name), PortStatus::Failed) => format!("{name} (in use)"),
        (Some(name), _) => found.clone().unwrap_or_else(|| name.clone()),
    };
    let send_rect = Rect::from_min_max(pos2(rect.right() - 30.0 * z, lamp_at.y - 6.0 * z), pos2(rect.right() - 3.0 * z, lamp_at.y + 6.0 * z));
    let device_rect = Rect::from_min_max(lamp_at + vec2(5.0 * z, -6.0 * z), pos2(send_rect.left() - 4.0 * z, lamp_at.y + 6.0 * z));
    let picker = ui.interact(device_rect, Id::unique(("fm_voice_device", node_id)), Sense::click());
    let ink = if picker.hovered() { theme::text::PRIMARY } else if device.is_some() { theme::text::SECONDARY } else { theme::text::DISABLED };
    let format = match target {
        Target::Volca => "",
        Target::Dx7 => " · as a DX7",
    };
    painter.with_clip_rect(device_rect.intersect(painter.clip_rect())).text(pos2(device_rect.left(), lamp_at.y), Align2::LEFT_CENTER, format!("{} · {name}{format}", PORT_LABELS[port]), small.clone(), ink);
    device_menu(ui, &picker, port, device.as_deref(), found.as_deref());
    let send = ui.interact(send_rect, Id::unique(("fm_voice_send", node_id)), Sense::click());
    painter.text(send_rect.right_center(), Align2::RIGHT_CENTER, "Send", small.clone(), if send.hovered() { theme::text::PRIMARY } else { theme::text::SECONDARY });
    if send.on_hover_text("Sends the voice again: opening the patch does this too, and every change to it").clicked() {
        responses.push(NodeResponse::User(SynthResponse::SendAll(node_id)));
    }

    // --- The voice's name, its algorithm, and where it came from ---
    let title_y = rect.top() + (DEVICE + 9.0) * z;
    let title = FontId::new(12.0 * z, egui::FontFamily::Name(theme::TITLE_FAMILY.into()));
    let named = if voice.name().is_empty() { "(no name)".to_string() } else { voice.name() };
    painter.text(pos2(rect.left() + 6.0 * z, title_y), Align2::LEFT_CENTER, named, title, theme::text::PRIMARY);
    let place = garden.place(playing.id).unwrap_or(0) + 1;
    painter.text(
        pos2(rect.right() - 5.0 * z, title_y),
        Align2::RIGHT_CENTER,
        format!("ALG {} · FB {}", voice.algorithm() + 1, voice.feedback()),
        FontId::monospace(7.5 * z),
        theme::text::SECONDARY,
    );
    let generation = match playing.generation {
        0 => String::new(),
        g => format!(" · generation {g}"),
    };
    painter.text(
        pos2(rect.left() + 6.0 * z, title_y + 11.0 * z),
        Align2::LEFT_CENTER,
        format!("{}{generation} · {place} of {}", garden.lineage(playing.id), garden.voices.len()),
        small.clone(),
        theme::text::QUIET,
    );
    response.on_hover_text(format!(
        "A DX7 voice, sent by SysEx to port {} as it changes{}. The volca fm doesn't keep it until you write it to a program on the volca itself",
        PORT_LABELS[port],
        if target == Target::Dx7 { format!(", on channel {}", channel + 1) } else { String::new() }
    ));

    // --- The algorithm, and each operator beside it ---
    let body = Rect::from_min_size(pos2(rect.left(), rect.top() + (DEVICE + TITLE) * z), vec2(rect.width(), BODY * z));
    let wiring = dx7::wiring(voice.algorithm());
    let diagram = Rect::from_min_max(body.left_top() + vec2(4.0 * z, 2.0 * z), pos2(body.left() + DIAGRAM * z, body.bottom() - 2.0 * z));
    draw_algorithm(&painter, diagram, &voice, &wiring, z);
    let rows = Rect::from_min_max(pos2(diagram.right() + 6.0 * z, body.top() + 1.0 * z), pos2(body.right() - 5.0 * z, body.bottom() - 1.0 * z));
    for n in 1..=OPERATORS {
        let row = Rect::from_min_size(pos2(rows.left(), rows.top() + (n - 1) as f32 * rows.height() / OPERATORS as f32), vec2(rows.width(), rows.height() / OPERATORS as f32));
        let hover = ui.interact(row, Id::unique(("fm_voice_op", node_id, n)), Sense::hover());
        if hover.hovered() {
            painter.rect_filled(row, 2.0 * z, theme::background::WIDGET_HOVERED.gamma_multiply(0.5));
        }
        draw_operator(&painter, row, &voice, n, wiring.carriers[n - 1], z);
        hover.on_hover_text(describe_operator(&voice, n, wiring.carriers[n - 1]));
    }

    // --- The garden ---
    let garden_top = body.bottom() + 6.0 * z;
    let cell = vec2(rect.width() / PER_ROW as f32, GARDEN_ROW * z);
    let chip_at = |place: usize| pos2(rect.left() + ((place % PER_ROW) as f32 + 0.5) * cell.x, garden_top + ((place / PER_ROW) as f32 + 0.5) * cell.y);
    let weed = garden.weed();
    let mut hovered = None;
    let mut chips = Vec::with_capacity(garden.voices.len());
    for (place, grown) in garden.voices.iter().enumerate() {
        let at = chip_at(place);
        let hit = ui.interact(Rect::from_center_size(at, cell), Id::unique(("fm_voice_chip", node_id, grown.id)), Sense::click());
        if hit.hovered() {
            hovered = Some(grown.id);
        }
        chips.push((place, grown, at, hit));
    }
    // Lines to the playing voice's parents, and the hovered one's, beneath the chips
    for (id, strength) in [(Some(playing.id), 0.55), (hovered.filter(|&h| h != playing.id), 0.35)] {
        let Some(child) = id.and_then(|id| garden.get(id)) else { continue };
        let from = chip_at(garden.place(child.id).unwrap_or(0));
        for parent in child.parents.iter().filter_map(|&p| garden.place(p)) {
            lineage_line(&painter, from, chip_at(parent), theme::signal::MIDI.gamma_multiply(strength), z);
        }
    }
    let shift = ui.input(|i| i.modifiers.shift);
    for (place, grown, at, hit) in chips {
        let bright = garden::brightness(&grown.voice);
        let fill = Color32::from_rgb(70, 52, 110).lerp_to_gamma(Color32::from_rgb(222, 196, 255), bright);
        let current = grown.id == playing.id;
        let picked = garden.partner == Some(grown.id);
        let radius = if current { 4.6 } else { 3.6 } * z;
        if current {
            painter.circle_filled(at, radius + 3.0 * z, theme::signal::MIDI.gamma_multiply(0.25));
        }
        painter.circle_filled(at, radius, fill);
        if current {
            painter.circle_stroke(at, radius + 1.4 * z, Stroke::new(1.2 * z, theme::text::PRIMARY));
        }
        if picked {
            painter.circle_stroke(at, radius + 1.6 * z, Stroke::new(1.2 * z, theme::signal::GATE));
        }
        if grown.kept {
            painter.circle_filled(at + vec2(radius + 1.5 * z, -radius - 0.5 * z), 1.4 * z, theme::accent::WARNING);
        }
        if weed == Some(place) {
            // The next to go, faintly crossed
            painter.line_segment([at + vec2(-radius, radius), at + vec2(radius, -radius)], Stroke::new(0.8 * z, theme::text::DISABLED));
        }
        if hit.hovered() {
            painter.text(at + vec2(0.0, -radius - 1.0 * z), Align2::CENTER_BOTTOM, (place + 1).to_string(), FontId::proportional(6.0 * z), theme::text::SECONDARY);
        }
        let tip = format!(
            "{} · {}\n{}{}\n{}",
            place + 1,
            if grown.voice.name().is_empty() { "(no name)".to_string() } else { grown.voice.name() },
            garden.lineage(grown.id),
            if grown.kept { " · kept" } else { "" },
            if current { "Playing. Right-click to keep it" } else { "Click to play it, Shift+click to pick it to cross with" }
        );
        let hit = hit.on_hover_text(tip);
        if hit.clicked() && !current {
            let mut next = garden.clone();
            let label = if shift {
                next.pick(grown.id);
                "Pick FM voice to cross"
            } else {
                next.recall(grown.id);
                "Play FM voice"
            };
            responses.push(set(node_id, &next, label));
        }
        hit.context_menu(|ui| {
            chip_menu(ui, node_id, &garden, grown.id, &mut responses);
        });
    }

    // --- Mutate, Cross, Keep, and the menu ---
    let line = Rect::from_min_max(pos2(rect.left() + 5.0 * z, rect.bottom() - (ACTIONS - 3.0) * z), pos2(rect.right() - 5.0 * z, rect.bottom() - 4.0 * z));
    let amount = display_frame::value_of(graph, node_id, "Amount").unwrap_or(0.35);
    let seed = || {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (node_id, now.to_bits()).hash(&mut hasher);
        hasher.finish()
    };
    let room = garden.has_room();
    let mutate = Rect::from_min_size(line.left_top(), vec2(50.0 * z, line.height()));
    let full = "The garden is full of kept voices: let one go to grow another";
    if button(ui, &painter, mutate, Id::unique(("fm_voice_mutate", node_id)), "Mutate", room, z)
        .on_hover_text(if room { format!("A child of {}, {:.0}% of the way to a stranger (the Amount knob). It sounds as loud: only the timbre moves", voice.name(), amount * 100.0) } else { full.to_string() })
        .clicked()
    {
        let mut next = garden.clone();
        if next.mutate(amount, seed()).is_some() {
            responses.push(set(node_id, &next, "Mutate FM voice"));
        }
    }
    let partner = garden.partner.and_then(|p| garden.get(p));
    let cross = Rect::from_min_size(pos2(mutate.right() + 5.0 * z, line.top()), vec2(108.0 * z, line.height()));
    let cross_text = match partner {
        Some(p) => format!("Cross with {}", p.voice.name()),
        None => "Cross".to_string(),
    };
    let can_cross = partner.is_some() && room;
    let cross_tip = match (partner, room) {
        (None, _) => "Shift+click a voice in the garden to pick it, then cross the playing voice with it: each operator whole from one or the other".to_string(),
        (Some(_), false) => full.to_string(),
        (Some(p), true) => format!("A child of {} and {}: each operator whole from one or the other, the algorithm and its loudness from one", voice.name(), p.voice.name()),
    };
    if button(ui, &painter, cross, Id::unique(("fm_voice_cross", node_id)), &cross_text, can_cross, z).on_hover_text(cross_tip).clicked() {
        let mut next = garden.clone();
        if next.cross(seed()).is_some() {
            responses.push(set(node_id, &next, "Cross FM voices"));
        }
    }
    let keep = Rect::from_min_size(pos2(cross.right() + 5.0 * z, line.top()), vec2(40.0 * z, line.height()));
    let kept = playing.kept;
    if button(ui, &painter, keep, Id::unique(("fm_voice_keep", node_id)), if kept { "Kept" } else { "Keep" }, true, z)
        .on_hover_text(if kept { "Kept through every weeding: click to let it go" } else { "Keeps the voice through every weeding: once the garden holds 32, a new voice takes the oldest one's place that isn't kept" })
        .clicked()
    {
        let mut next = garden.clone();
        next.keep(playing.id, !kept);
        responses.push(set(node_id, &next, if kept { "Let FM voice go" } else { "Keep FM voice" }));
    }
    let menu = Rect::from_min_max(pos2(line.right() - 22.0 * z, line.top()), line.right_bottom());
    let opener = button(ui, &painter, menu, Id::unique(("fm_voice_menu", node_id)), "⋯", true, z).on_hover_text("Open and save .syx files, send the garden as a bank, start over");
    let popup = opener.id.with("menu");
    if opener.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    egui::Popup::from_response(&opener).id(popup).open_memory(None).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
        ui.set_min_width(220.0);
        file_menu(ui, node_id, &garden, port, &mut responses);
    });

    if animate {
        crate::app::pace::animate(ui.ctx());
    }
    responses
}

/// The labels that keep `garden`, as an undo step named `label`.
fn set(node_id: NodeId, garden: &Garden, label: &str) -> NodeResponse<SynthResponse, SynthNodeData> {
    NodeResponse::User(SynthResponse::SetLabels { node_id, label: label.to_string(), labels: garden.labels() })
}

/// A small pill of a button, drawn in patch units so it scales smoothly.
fn button(ui: &mut egui::Ui, painter: &egui::Painter, rect: Rect, id: Id, text: &str, enabled: bool, z: f32) -> egui::Response {
    let response = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let fill = match (enabled, response.hovered()) {
        (false, _) => theme::background::WIDGET.gamma_multiply(0.5),
        (true, true) => theme::background::WIDGET_HOVERED,
        (true, false) => theme::background::WIDGET,
    };
    painter.rect_filled(rect, 3.0 * z, fill);
    let ink = if enabled { theme::text::PRIMARY } else { theme::text::DISABLED };
    painter.rect_stroke(rect, 3.0 * z, Stroke::new(0.8 * z, theme::signal::MIDI.gamma_multiply(if enabled { 0.6 } else { 0.25 })), egui::StrokeKind::Inside);
    painter.with_clip_rect(rect.shrink(2.0 * z).intersect(painter.clip_rect())).text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(8.0 * z), ink);
    response
}

/// A soft curve from a child's chip up or across to its parent's.
fn lineage_line(painter: &egui::Painter, from: Pos2, to: Pos2, color: Color32, z: f32) {
    let lift = vec2(0.0, -6.0 * z - (from.x - to.x).abs() * 0.12);
    let control = pos2((from.x + to.x) / 2.0, from.y.min(to.y)) + lift;
    let points: Vec<Pos2> = (0..=16)
        .map(|k| {
            let t = k as f32 / 16.0;
            let (a, b) = (from.lerp(control, t), control.lerp(to, t));
            a.lerp(b, t)
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.0 * z, color)));
}

/// An operator's colour: a carrier is heard, so audio's blue; a modulator
/// shapes, so modulation's orange.
fn role_color(carrier: bool) -> Color32 {
    if carrier {
        theme::signal::AUDIO
    } else {
        theme::signal::CONTROL
    }
}

/// Where each operator sits in the drawing: (column, row up from the
/// carriers), and how many columns and rows there are.
fn layout(wiring: &Wiring) -> ([(f32, usize); OPERATORS], f32, usize) {
    let mut depth = [0usize; OPERATORS + 1];
    for _ in 0..OPERATORS {
        for &(from, to) in &wiring.modulations {
            depth[from] = depth[from].max(depth[to] + 1);
        }
    }
    // Each modulator stands over the deepest operator it modulates
    let primary = |n: usize| wiring.modulations.iter().filter(|&&(from, _)| from == n).map(|&(_, to)| to).max_by_key(|&to| (depth[to], std::cmp::Reverse(to)));
    let children = |t: usize| -> Vec<usize> { (1..=OPERATORS).filter(|&n| primary(n) == Some(t)).collect() };
    fn width(n: usize, children: &dyn Fn(usize) -> Vec<usize>) -> f32 {
        children(n).iter().map(|&c| width(c, children)).sum::<f32>().max(1.0)
    }
    fn place(n: usize, left: f32, children: &dyn Fn(usize) -> Vec<usize>, depth: &[usize], at: &mut [(f32, usize); OPERATORS]) {
        let span = width(n, children);
        at[n - 1] = (left + span / 2.0, depth[n]);
        let mut cursor = left;
        for c in children(n) {
            place(c, cursor, children, depth, at);
            cursor += width(c, children);
        }
    }
    let mut at = [(0.0, 0); OPERATORS];
    let mut cursor = 0.0;
    for n in (1..=OPERATORS).filter(|&n| wiring.carriers[n - 1]) {
        place(n, cursor, &children, &depth, &mut at);
        cursor += width(n, &children);
    }
    let rows = depth.iter().max().copied().unwrap_or(0) + 1;
    (at, cursor, rows)
}

/// The algorithm, as the DX7 draws it, in `rect`.
fn draw_algorithm(painter: &egui::Painter, rect: Rect, voice: &Voice, wiring: &Wiring, z: f32) {
    let (at, columns, rows) = layout(wiring);
    let cell_w = (rect.width() / columns).min(17.0 * z);
    let cell_h = ((rect.height() - 8.0 * z) / rows as f32).min(14.0 * z);
    let radius = (cell_w.min(cell_h) * 0.36).min(5.2 * z);
    let left = rect.center().x - cell_w * columns / 2.0;
    // Stood in the middle of the room, the output line under it
    let tall = rows as f32 * cell_h + 8.0 * z;
    let base = rect.center().y + tall / 2.0 - 8.0 * z - cell_h / 2.0;
    let centre = |n: usize| pos2(left + at[n - 1].0 * cell_w, base - at[n - 1].1 as f32 * cell_h);
    let wire = Stroke::new(1.0 * z, theme::text::QUIET.gamma_multiply(0.7));

    // The carriers into the output line
    let carriers: Vec<usize> = (1..=OPERATORS).filter(|&n| wiring.carriers[n - 1]).collect();
    let out_y = base + cell_h / 2.0 + 5.0 * z;
    if let (Some(&first), Some(&last)) = (carriers.first(), carriers.last()) {
        painter.line_segment([pos2(centre(first).x, out_y), pos2(centre(last).x, out_y)], Stroke::new(1.2 * z, theme::signal::AUDIO.gamma_multiply(0.6)));
        for &n in &carriers {
            painter.line_segment([centre(n) + vec2(0.0, radius), pos2(centre(n).x, out_y)], Stroke::new(1.0 * z, theme::signal::AUDIO.gamma_multiply(0.6)));
        }
    }
    for &(from, to) in &wiring.modulations {
        painter.line_segment([centre(from) + vec2(0.0, radius), centre(to) - vec2(0.0, radius)], wire);
    }
    // The feedback loop: round the side of one that feeds itself, or from
    // the one that feeds back up to the one it feeds
    let (out, into) = wiring.feedback;
    let fb_ink = Stroke::new(1.0 * z, theme::signal::CONTROL.gamma_multiply(0.35 + 0.6 * voice.feedback() as f32 / 7.0));
    if out == into {
        let c = centre(out);
        let points: Vec<Pos2> = (0..=14)
            .map(|k| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * 1.5 * k as f32 / 14.0;
                c + vec2(radius * 1.25 + radius * 0.9 * (1.0 + a.cos()), -radius * 0.9 + radius * 1.4 * a.sin() * 0.7)
            })
            .collect();
        painter.add(egui::Shape::line(points, fb_ink));
    } else {
        let (a, b) = (centre(out) + vec2(radius, 0.0), centre(into) + vec2(radius, 0.0));
        let side = a.x.max(b.x) + radius * 1.6;
        painter.add(egui::Shape::line(vec![a, pos2(side, a.y), pos2(side, b.y), b], fb_ink));
    }
    for n in 1..=OPERATORS {
        let carrier = wiring.carriers[n - 1];
        let level = voice.op(n, op::LEVEL) as f32 / 99.0;
        let color = role_color(carrier);
        let c = centre(n);
        painter.circle_filled(c, radius, theme::background::PANEL.lerp_to_gamma(color, 0.15 + 0.7 * level));
        painter.circle_stroke(c, radius, Stroke::new(1.0 * z, color.gamma_multiply(0.5 + 0.5 * level)));
        painter.text(c, Align2::CENTER_CENTER, n.to_string(), FontId::proportional(radius * 1.35), if level > 0.55 { theme::background::MAIN } else { theme::text::PRIMARY });
    }
}

/// Operator `n`'s row: its number, ratio, envelope and level.
fn draw_operator(painter: &egui::Painter, row: Rect, voice: &Voice, n: usize, carrier: bool, z: f32) {
    let color = role_color(carrier);
    let mid = row.center().y;
    let level = voice.op(n, op::LEVEL) as f32 / 99.0;
    painter.text(pos2(row.left() + 3.0 * z, mid), Align2::CENTER_CENTER, n.to_string(), FontId::proportional(7.0 * z), color);
    let fixed = voice.op(n, op::MODE) == 1;
    let ratio_ink = if fixed { theme::accent::WARNING } else { theme::text::SECONDARY };
    painter.text(pos2(row.left() + 9.0 * z, mid), Align2::LEFT_CENTER, voice.frequency(n).label(), FontId::monospace(6.8 * z), ratio_ink);

    // The envelope, its shape from its rates and levels
    let env = Rect::from_min_max(pos2(row.left() + 46.0 * z, row.top() + 1.5 * z), pos2(row.right() - 40.0 * z, row.bottom() - 1.5 * z));
    let at = Voice::op_at(n);
    let rates: [u8; 4] = voice.0[at + op::RATES..at + op::RATES + 4].try_into().expect("four rates");
    let levels: [u8; 4] = voice.0[at + op::LEVELS..at + op::LEVELS + 4].try_into().expect("four levels");
    let points: Vec<Pos2> = envelope(rates, levels).iter().map(|&(x, y)| pos2(env.left() + x * env.width(), env.bottom() - y * env.height())).collect();
    for pair in points.windows(2) {
        painter.add(egui::Shape::convex_polygon(
            vec![pair[0], pair[1], pos2(pair[1].x, env.bottom()), pos2(pair[0].x, env.bottom())],
            color.gamma_multiply(0.12 + 0.18 * level),
            Stroke::NONE,
        ));
    }
    painter.add(egui::Shape::line(points, Stroke::new(1.0 * z, color.gamma_multiply(0.45 + 0.55 * level))));

    // The output level
    let bar = Rect::from_min_max(pos2(row.right() - 34.0 * z, mid - 2.0 * z), pos2(row.right(), mid + 2.0 * z));
    painter.rect_filled(bar, 1.5 * z, theme::background::MAIN);
    painter.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * level, bar.height())), 1.5 * z, color.gamma_multiply(0.5 + 0.5 * level));
}

/// The points of an envelope's shape, x and y from 0 to 1: from where the
/// last note left it (L4) up to L1, on to L2 and L3, held, then back to L4
/// as the key lifts. Faster rates take less room; the hold is fixed.
fn envelope(rates: [u8; 4], levels: [u8; 4]) -> [(f32, f32); 6] {
    let y = |l: u8| l.min(99) as f32 / 99.0;
    // A rate of 99 is near instant, 0 takes many seconds: shown on a
    // gentler scale, so both fit
    let span = |rate: u8, from: u8, to: u8| (0.12 + (y(from) - y(to)).abs()) * 2f32.powf(-(rate.min(99) as f32) / 22.0);
    let segments = [span(rates[0], levels[3], levels[0]), span(rates[1], levels[0], levels[1]), span(rates[2], levels[1], levels[2]), 0.0, span(rates[3], levels[2], levels[3])];
    let moving: f32 = segments.iter().sum::<f32>().max(1e-3);
    let hold = 0.22;
    let scale = (1.0 - hold) / moving;
    let mut x = 0.0;
    let mut points = [(0.0, y(levels[3])); 6];
    let targets = [levels[0], levels[1], levels[2], levels[2], levels[3]];
    for (k, (&seg, &to)) in segments.iter().zip(targets.iter()).enumerate() {
        x += if k == 3 { hold } else { seg * scale };
        points[k + 1] = (x.min(1.0), y(to));
    }
    points
}

/// What an operator's row says when it's hovered.
fn describe_operator(voice: &Voice, n: usize, carrier: bool) -> String {
    let at = Voice::op_at(n);
    let b = |field: usize| voice.0[at + field];
    let frequency = match voice.frequency(n) {
        Frequency::Ratio(r) => format!("{r:.2} times the note"),
        Frequency::Fixed(hz) => format!("fixed at {hz:.1} Hz, whatever the note"),
    };
    let detune = b(op::DETUNE) as i32 - 7;
    let detune = if detune == 0 { String::new() } else { format!(", detuned {detune:+}") };
    format!(
        "Operator {n}, a {}: {frequency}{detune}\nOutput level {} · velocity {} · rate scaling {}\nEnvelope rates {} {} {} {}, levels {} {} {} {}",
        if carrier { "carrier, heard" } else { "modulator, shaping what it feeds" },
        b(op::LEVEL),
        b(op::VELOCITY),
        b(op::RATE_SCALING),
        b(op::RATES),
        b(op::RATES + 1),
        b(op::RATES + 2),
        b(op::RATES + 3),
        b(op::LEVELS),
        b(op::LEVELS + 1),
        b(op::LEVELS + 2),
        b(op::LEVELS + 3),
    )
}

/// A chip's right-click menu.
fn chip_menu(ui: &mut egui::Ui, node_id: NodeId, garden: &Garden, id: u32, responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>) {
    let Some(grown) = garden.get(id) else { return };
    ui.label(egui::RichText::new(format!("{} · {}", garden.place(id).unwrap_or(0) + 1, grown.voice.name())).small().weak());
    let playing = garden.current == id;
    if !playing && ui.button("Play it").clicked() {
        let mut next = garden.clone();
        next.recall(id);
        responses.push(set(node_id, &next, "Play FM voice"));
        ui.close();
    }
    if !playing {
        let picked = garden.partner == Some(id);
        if ui.button(if picked { "Don't cross with it" } else { "Pick it to cross with" }).clicked() {
            let mut next = garden.clone();
            next.pick(id);
            responses.push(set(node_id, &next, "Pick FM voice to cross"));
            ui.close();
        }
    }
    if ui.button(if grown.kept { "Let it go" } else { "Keep it" }).clicked() {
        let mut next = garden.clone();
        next.keep(id, !grown.kept);
        responses.push(set(node_id, &next, if grown.kept { "Let FM voice go" } else { "Keep FM voice" }));
        ui.close();
    }
    if !playing && garden.voices.len() > 1 && ui.button("Weed it out").clicked() {
        let mut next = garden.clone();
        next.voices.retain(|g| g.id != id);
        if next.partner == Some(id) {
            next.partner = None;
        }
        responses.push(set(node_id, &next, "Weed out FM voice"));
        ui.close();
    }
}

/// The menu at the right of the buttons.
fn file_menu(ui: &mut egui::Ui, node_id: NodeId, garden: &Garden, port: usize, responses: &mut Vec<NodeResponse<SynthResponse, SynthNodeData>>) {
    let mut file = |action| responses.push(NodeResponse::User(SynthResponse::FmVoiceFile { node_id, action }));
    if ui.button("Open .syx…").on_hover_text("Plants a DX7 voice or bank in the garden, after the voices growing: a bank of 32 makes a garden of its own, but for the voices kept").clicked() {
        file(VoiceFile::Open);
    }
    if ui.button("Save voice as .syx…").on_hover_text("The playing voice, as a DX7 single voice: Dexed and a DX7 read it too").clicked() {
        file(VoiceFile::SaveVoice);
    }
    if ui.button("Save garden as a bank…").on_hover_text(format!("All {} voices as one DX7 bank of 32, in the garden's order: INIT VOICE fills the rest", garden.voices.len())).clicked() {
        file(VoiceFile::SaveBank);
    }
    ui.separator();
    if ui
        .button(format!("Send garden to port {}'s memories…", PORT_LABELS[port]))
        .on_hover_text("Sends the garden as a bank of 32. A volca fm writes it over all 32 of its programs, so it asks first")
        .clicked()
    {
        file(VoiceFile::SendBank);
    }
    ui.separator();
    if ui.button("Start over with the seeds").on_hover_text("Soft Keys, Glass Bell, Wood Bass and INIT VOICE, and the voices kept").clicked() {
        let mut next = Garden::seeded();
        let kept: Vec<_> = garden.voices.iter().filter(|g| g.kept).cloned().collect();
        let base = garden.next;
        for g in &mut next.voices {
            g.id += base;
        }
        next.current += base;
        next.next += base;
        let room = ROOM - next.voices.len();
        next.voices.splice(0..0, kept.into_iter().take(room));
        responses.push(set(node_id, &next, "Start FM garden over"));
    }
}
