//! The Mixer's face: a console turned on its side, one row per channel.
//!
//! Each row is a whole channel strip, read left to right the way its signal
//! travels: its four jacks (the channel in, then CV for its level, pan and
//! send) on the node's edge, its number, a lane of the stereo field with a
//! light wherever it sounds, its meter, its four knobs (level, pan, width,
//! send) and its mute and solo. Below the strips, the bus row: the chain,
//! the effect return and the Return's CV coming in on the left, the Return
//! and Master knobs and the stereo meter, and every output leaving on the
//! right.
//!
//! The lights are drawn from the same pan law and fan the audio uses, so
//! what you see is where you hear it: one light for a mono cable, one per
//! voice for a polyphonic one, opened out by Width. A channel sent to the
//! effect blooms along its lane, wider the more it sends, as a sound spreads
//! into a room.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, UiBuilder, Vec2};
use egui_node_graph2::{place_port, AnyParameterId, NodeId, NodeResponse};

use crate::app::theme;
use crate::dsp::SignalType;
use crate::modules::mixer::{heard, voice_pan, Mixer, STRIPS};
use crate::widgets::{column_meter, PeakBallistics};

use super::hints::{self, Hint};
use super::node_data::{defer_output_label, KnobCellCtx, SynthNodeData};
use super::{SynthGraph, SynthGraphState, SynthResponse, SynthValueType};

/// Each channel's colour, for its number, its lane and its lights. Four
/// light, cool tints that read apart on the dark node and never compete with
/// the signal colours of the cables.
const CHANNEL_HUES: [Color32; STRIPS] = [
    Color32::from_rgb(110, 185, 255),
    Color32::from_rgb(110, 220, 185),
    Color32::from_rgb(185, 160, 255),
    Color32::from_rgb(245, 150, 190),
];

/// The knobs of a channel row, left to right: parameter and column title.
const STRIP_KNOBS: [(&str, &str); 4] = [("Level", "Level"), ("Pan", "Pan"), ("Width", "Width"), ("Send", "Send")];

/// Where things sit, unzoomed, measured from the display's top left.
mod geometry {
    /// Distance between neighbouring jacks on the edge.
    pub const JACK_PITCH: f32 = 18.0;
    /// The column titles above the strips.
    pub const HEADER: f32 = 14.0;
    /// One channel strip: its four jacks, beside one knob without a title.
    pub const ROW: f32 = 4.0 * JACK_PITCH;
    /// A knob's cell, centred in its row.
    pub const KNOB_CELL: f32 = 3.0 * JACK_PITCH;
    /// The gap above the bus row, where its hairline runs.
    pub const BUS_GAP: f32 = 8.0;
    /// The bus row: its five outputs down the right edge.
    pub const BUS: f32 = 5.0 * JACK_PITCH;

    /// Left to right: jack labels, number, lane, meter, knobs, buttons.
    pub const NUMBER_X: f32 = 36.0;
    pub const LANE_X: f32 = 46.0;
    pub const LANE_W: f32 = 96.0;
    pub const METER_X: f32 = 152.0;
    pub const KNOBS_X: f32 = 164.0;
    /// A knob's column, and where the dial's centre sits in it.
    pub const KNOB_COLUMN: f32 = 44.0;
    pub const KNOB_CENTRE: f32 = 18.0;
    pub const BUTTONS_X: f32 = KNOBS_X + 4.0 * KNOB_COLUMN + 4.0;
    pub const BUTTON_W: f32 = 20.0;
    pub const WIDTH: f32 = BUTTONS_X + BUTTON_W;

    /// The top of channel row `s`.
    pub fn row_top(s: usize) -> f32 {
        HEADER + s as f32 * ROW
    }

    /// The top of the bus row.
    pub fn bus_top() -> f32 {
        row_top(super::STRIPS) + BUS_GAP
    }

    pub fn height() -> f32 {
        bus_top() + BUS
    }
}

/// Which edge a jack sits on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Edge {
    Left,
    Right,
}

/// Where a jack sits: its edge, and its height below the display's top
/// (unzoomed), at the middle of its slot.
fn jack_slot(name: &str) -> Option<(Edge, f32)> {
    use geometry::*;
    let slot = |top: f32, k: usize| top + (k as f32 + 0.5) * JACK_PITCH;
    let strip = |prefix: &str| {
        let n: usize = name.strip_prefix(prefix)?.parse().ok()?;
        (1..=STRIPS).contains(&n).then_some(n - 1)
    };
    if let Some(s) = strip("Ch ") {
        return Some((Edge::Left, slot(row_top(s), 0)));
    }
    if let Some(s) = strip("Level ") {
        return Some((Edge::Left, slot(row_top(s), 1)));
    }
    if let Some(s) = strip("Pan ") {
        return Some((Edge::Left, slot(row_top(s), 2)));
    }
    if let Some(s) = strip("Send ") {
        return Some((Edge::Left, slot(row_top(s), 3)));
    }
    let bus = |edge, k| Some((edge, slot(bus_top(), k)));
    match name {
        "Chain In" => bus(Edge::Left, 0),
        "Return L" => bus(Edge::Left, 1),
        "Return R" => bus(Edge::Left, 2),
        "Return" => bus(Edge::Left, 3),
        "Out L" => bus(Edge::Right, 0),
        "Out R" => bus(Edge::Right, 1),
        "Send L" => bus(Edge::Right, 2),
        "Send R" => bus(Edge::Right, 3),
        "Chain Out" => bus(Edge::Right, 4),
        _ => None,
    }
}

/// Whether the display lays out this jack itself.
pub fn places(name: &str) -> bool {
    jack_slot(name).is_some()
}

/// The short label beside a jack on the left edge.
fn jack_label(name: &str) -> &'static str {
    match name.split(' ').next() {
        Some("Ch") => "in",
        Some("Level") => "lvl",
        Some("Pan") => "pan",
        Some("Send") => "snd",
        _ => match name {
            "Chain In" => "chain",
            "Return L" => "ret L",
            "Return R" => "ret R",
            _ => "return",
        },
    }
}

/// One channel's state, as the display needs it.
struct StripView {
    patched: bool,
    muted: bool,
    soloed: bool,
    /// Pan with any CV added, from -1 to 1.
    pan: f32,
    width: f32,
    /// Send with any CV added, from 0 to 1.
    send: f32,
    /// Each voice's last peak, or `None` before any reading.
    voices: Vec<Option<f32>>,
}

/// Draws the Mixer's face, and places its jacks beside their rows.
pub fn mixer_strips(
    data: &SynthNodeData,
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &mut SynthGraphState,
    zoom: f32,
) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    use geometry::*;
    let z = zoom;
    let mut responses = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else {
        return responses;
    };

    ui.add_space(2.0 * z);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH * z, height() * z), Sense::hover());
    let at = |x: f32, y: f32| rect.min + Vec2::new(x, y) * z;

    // Every jack is reported, whether or not the node is on screen, so
    // cables always land on it
    for (name, id) in &node.inputs {
        if let Some((_, y)) = jack_slot(name) {
            place_port(ui, node_id, AnyParameterId::Input(*id), rect.top() + y * z);
        }
    }
    for (name, id) in &node.outputs {
        if let Some((_, y)) = jack_slot(name) {
            place_port(ui, node_id, AnyParameterId::Output(*id), rect.top() + y * z);
        }
    }

    let input_of = |name: &str| node.inputs.iter().find(|(n, _)| n == name).map(|(_, id)| *id);
    let value_of = |name: &str| {
        input_of(name).map_or(0.0, |id| match &graph.get_input(id).value {
            SynthValueType::Number { value, .. } => *value,
            SynthValueType::Toggle { value, .. } => f32::from(u8::from(*value)),
            _ => 0.0,
        })
    };
    let feeding = |name: &str| {
        let input = input_of(name)?;
        graph.connections.get(input)?.first().copied()
    };
    let engine_node_id = user_state.get_engine_node_id(node_id);

    let strips: Vec<StripView> = (0..STRIPS)
        .map(|s| {
            let n = s + 1;
            let source = feeding(&format!("Ch {n}"));
            let voices = source
                .and_then(|output| {
                    let out = graph.outputs.get(output)?;
                    let peaks = user_state.output_peaks(out.node, graph.get_output_index(output)?)?;
                    Some((0..peaks.count()).map(|v| Some(peaks.peak(v))).collect())
                })
                .unwrap_or_else(|| vec![None]);
            let cv = |name: String, port: usize| match (feeding(&name), engine_node_id) {
                (Some(_), Some(eid)) => user_state.get_input_value(eid, port).unwrap_or(0.0),
                _ => 0.0,
            };
            let pan_cv = cv(format!("Pan {n}"), Mixer::PORT_PAN_CV + s);
            let send_cv = cv(format!("Send {n}"), Mixer::PORT_SEND_CV + s);
            StripView {
                patched: source.is_some(),
                muted: value_of(&format!("Mute {n}")) >= 0.5,
                soloed: value_of(&format!("Solo {n}")) >= 0.5,
                pan: (value_of(&format!("Pan {n}")) + pan_cv).clamp(-1.0, 1.0),
                width: value_of(&format!("Width {n}")),
                send: (value_of(&format!("Send {n}")) + send_cv).clamp(0.0, 1.0),
                voices,
            }
        })
        .collect();
    let any_solo = strips.iter().any(|strip| strip.soloed);
    let meters = engine_node_id.and_then(|eid| user_state.module_meters.get(&eid)).cloned();
    let silent = PeakBallistics::default();
    let meter = |index: usize| meters.as_ref().map_or(&silent, |m| m.get(index));

    if !ui.is_rect_visible(rect) {
        return responses;
    }
    let painter = ui.painter().clone();
    let small = egui::FontId::proportional(8.5 * z);
    let tiny = egui::FontId::proportional(8.0 * z);

    // --- Column titles ---
    let header_y = rect.top() + HEADER * 0.5 * z;
    painter.text(at(LANE_X + 4.0, HEADER * 0.5), egui::Align2::LEFT_CENTER, "L", small.clone(), theme::text::QUIET);
    painter.text(at(LANE_X + LANE_W - 4.0, HEADER * 0.5), egui::Align2::RIGHT_CENTER, "R", small.clone(), theme::text::QUIET);
    for (k, (_, title)) in STRIP_KNOBS.iter().enumerate() {
        let x = at(KNOBS_X + k as f32 * KNOB_COLUMN + KNOB_CENTRE, 0.0).x;
        painter.text(Pos2::new(x, header_y), egui::Align2::CENTER_CENTER, *title, small.clone(), theme::text::QUIET);
    }

    // --- The channel rows ---
    for (s, strip) in strips.iter().enumerate() {
        let n = s + 1;
        let top = row_top(s);
        let row = Rect::from_min_size(at(0.0, top), Vec2::new(WIDTH, ROW) * z);
        let mid = row.center().y;
        let hue = CHANNEL_HUES[s];
        let is_heard = heard(strip.muted, strip.soloed, any_solo);

        // Every other row a shade lighter, so the eye can follow a row across
        if s % 2 == 1 {
            painter.rect_filled(row.expand2(Vec2::new(4.0 * z, 0.0)), 4.0 * z, Color32::from_white_alpha(3));
        }

        // The jacks' labels, lit when patched
        for (k, name) in [format!("Ch {n}"), format!("Level {n}"), format!("Pan {n}"), format!("Send {n}")].iter().enumerate() {
            let signal = if k == 0 { SignalType::Audio } else { SignalType::Control };
            let caption = Caption { name, signal, patched: feeding(name).is_some(), level: 0.0 };
            jack_caption(ui, &painter, data, node_id, at(2.0, top + (k as f32 + 0.5) * JACK_PITCH), &tiny, caption);
        }

        // The number, in the channel's hue
        let number_color = if strip.patched { hue } else { hue.gamma_multiply(0.4) };
        painter.text(
            Pos2::new(at(NUMBER_X, 0.0).x, mid),
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            egui::FontId::proportional(11.0 * z),
            number_color,
        );

        // The lane: this channel's place in the stereo field
        let lane = Rect::from_min_size(Pos2::new(at(LANE_X, 0.0).x, mid - 11.0 * z), Vec2::new(LANE_W, 22.0) * z);
        let lights = Lights { hue, is_heard, meter: meter(s), playing: user_state.is_playing };
        draw_lane(&painter, lane, strip, lights, z);
        let lane_response = ui.interact(lane, ui.scope_id().with(("mixer_lane", node_id, s)), Sense::hover());
        lane_response.on_hover_text(format!(
            "Where channel {n} sits, left to right. A polyphonic channel has a light per voice, fanned out by its Width. With its Send up it blooms along the lane"
        ));

        // The meter, after the fader
        let bar = Rect::from_center_size(Pos2::new(at(METER_X, 0.0).x, mid), Vec2::new(6.0, ROW - 18.0) * z);
        column_meter(&painter, bar, meter(s), !is_heard);
        ui.interact(bar.expand2(Vec2::new(4.0 * z, 0.0)), ui.scope_id().with(("mixer_meter", node_id, s)), Sense::hover())
            .on_hover_text(format!("Channel {n}: {} after its fader", format_db(meter(s).level_db())));

        // The knobs, one per column, untitled: the header names them
        let knob_top = top + (ROW - KNOB_CELL) * 0.5 + 1.0;
        for (k, (param, _)) in STRIP_KNOBS.iter().enumerate() {
            let cell = Rect::from_min_size(at(KNOBS_X + k as f32 * KNOB_COLUMN, knob_top), Vec2::new(KNOB_COLUMN, KNOB_CELL - 1.0) * z);
            let mut cx = KnobCellCtx::module(node_id, graph, user_state, z, &mut responses);
            knob(data, ui, cell, &format!("{param} {n}"), "", &mut cx);
        }

        // Mute over solo
        let x = at(BUTTONS_X, 0.0).x;
        let mute = Rect::from_min_size(Pos2::new(x, mid - 16.0 * z), Vec2::new(BUTTON_W, 15.0) * z);
        let solo = Rect::from_min_size(Pos2::new(x, mid + 1.0 * z), Vec2::new(BUTTON_W, 15.0) * z);
        let tips = if strip.muted {
            [format!("Channel {n} is muted: click to hear it"), String::new()]
        } else {
            [format!("Mute channel {n}"), String::new()]
        };
        let latch = Latch { text: "M", on: strip.muted, lit: theme::accent::ERROR, tip: &tips[0] };
        if button(ui, &painter, mute, ("mixer_mute", node_id, s), latch, z) {
            responses.push(changed(node_id, format!("Mute {n}"), !strip.muted));
        }
        let solo_tip = if strip.soloed {
            format!("Channel {n} is soloed: click to hear the others again")
        } else {
            format!("Solo channel {n}: hear it alone (with any other soloed channel and the return)")
        };
        let latch = Latch { text: "S", on: strip.soloed, lit: theme::accent::WARNING, tip: &solo_tip };
        if button(ui, &painter, solo, ("mixer_solo", node_id, s), latch, z) {
            responses.push(changed(node_id, format!("Solo {n}"), !strip.soloed));
        }
    }

    // --- The bus row ---
    let top = bus_top();
    let accent = crate::dsp::ModuleCategory::Utility.color();
    let hairline = rect.top() + (top - BUS_GAP * 0.5) * z;
    painter.hline(rect.left()..=rect.right(), hairline, Stroke::new(1.0 * z, accent.gamma_multiply(0.25)));

    // What comes in: the chain and the return, their labels lit by their
    // level, and the CV that rides the Return
    for (k, (name, signal, level)) in [
        ("Chain In", SignalType::Bus, meter(Mixer::METER_CHAIN).fraction()),
        ("Return L", SignalType::Audio, meter(Mixer::METER_RETURN).fraction()),
        ("Return R", SignalType::Audio, meter(Mixer::METER_RETURN).fraction()),
        ("Return", SignalType::Control, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        let caption = Caption { name, signal, patched: feeding(name).is_some(), level };
        jack_caption(ui, &painter, data, node_id, at(2.0, top + (k as f32 + 0.5) * JACK_PITCH), &tiny, caption);
    }

    // Return and Master, titled, over the lane's column
    let knob_top = top + (BUS - 68.0) * 0.5;
    for (k, (param, title)) in [("Return", "Return"), ("Master", "Master")].into_iter().enumerate() {
        let cell = Rect::from_min_size(at(LANE_X + k as f32 * (KNOB_COLUMN + 6.0), knob_top), Vec2::new(KNOB_COLUMN, 68.0) * z);
        knob(data, ui, cell, param, title, &mut KnobCellCtx::module(node_id, graph, user_state, z, &mut responses));
    }

    // The stereo meter, under the strips' meters
    let meter_mid = rect.top() + (top + BUS * 0.5) * z;
    for side in 0..2 {
        let x = at(METER_X, 0.0).x + (side as f32 - 0.5) * 7.0 * z;
        let bar = Rect::from_center_size(Pos2::new(x, meter_mid - 4.0 * z), Vec2::new(5.0, 52.0) * z);
        column_meter(&painter, bar, meter(Mixer::METER_OUT_L + side), false);
        painter.text(Pos2::new(x, bar.bottom() + 6.0 * z), egui::Align2::CENTER_CENTER, ["L", "R"][side], tiny.clone(), theme::text::SECONDARY);
    }
    let master_area = Rect::from_center_size(Pos2::new(at(METER_X, 0.0).x, meter_mid), Vec2::new(18.0, 64.0) * z);
    ui.interact(master_area, ui.scope_id().with(("mixer_master", node_id)), Sense::hover()).on_hover_text(format!(
        "Master: L {} / R {}",
        format_db(meter(Mixer::METER_OUT_L).level_db()),
        format_db(meter(Mixer::METER_OUT_R).level_db()),
    ));

    // What leaves, down the right edge, each label beside its jack
    let font = egui::TextStyle::Body.resolve(ui.style());
    let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
    for (name, _) in &node.outputs {
        let Some((Edge::Right, y)) = jack_slot(name) else {
            continue;
        };
        let galley = painter.layout_no_wrap(name.clone(), font.clone(), ink);
        let size = galley.size();
        let label = Rect::from_min_size(Pos2::new(rect.right() - size.x, rect.top() + y * z - size.y * 0.5), size);
        defer_output_label(ui, node_id, name, label, vec![egui::Shape::galley(label.min, galley, ink)]);
    }

    responses
}

/// A jack's short label: its name, its signal, whether it's patched, and
/// how loud what comes in is (for the bus row's inputs; 0 for the rest).
struct Caption<'a> {
    name: &'a str,
    signal: SignalType,
    patched: bool,
    level: f32,
}

/// A jack's short label on the left edge, in its signal's colour: bright
/// while patched, or as bright as its level says, for the bus row's inputs.
fn jack_caption(ui: &egui::Ui, painter: &egui::Painter, data: &SynthNodeData, node_id: NodeId, left_centre: Pos2, font: &egui::FontId, caption: Caption) {
    let Caption { name, signal, patched, level } = caption;
    let base = signal.color();
    let color = if patched {
        base.lerp_to_gamma(Color32::WHITE, 0.15 + 0.5 * level.clamp(0.0, 1.0))
    } else {
        base.gamma_multiply(0.45)
    };
    let galley = painter.layout_no_wrap(jack_label(name).to_string(), font.clone(), color);
    let label = Rect::from_min_size(Pos2::new(left_centre.x, left_centre.y - galley.size().y * 0.5), galley.size());
    painter.galley(label.min, galley, color);
    let response = ui.interact(label.expand(2.0), ui.scope_id().with(("mixer_jack", node_id, name)), Sense::hover());
    hints::attach(response, Hint::input(data.module_id, name));
}

/// The knob for `param`, in `cell`, titled `title` (or untitled).
fn knob(data: &SynthNodeData, ui: &mut egui::Ui, cell: Rect, param: &str, title: &str, cx: &mut KnobCellCtx) {
    let Some(knob) = data.knob_params.iter().find(|knob| knob.param_name == param) else {
        return;
    };
    let mut knob = knob.clone();
    knob.label = title.to_string();
    let mut cell_ui = ui.new_child(UiBuilder::new().max_rect(cell).id_salt(("mixer_knob", param)));
    data.knob_cell(&mut cell_ui, &knob, cx);
}

/// A small latching button: its letter, whether it's latched, the colour
/// it lights when it is, and its tooltip.
struct Latch<'a> {
    text: &'a str,
    on: bool,
    lit: Color32,
    tip: &'a str,
}

/// A small latching button; true when clicked.
fn button(ui: &egui::Ui, painter: &egui::Painter, rect: Rect, id: impl egui::AsIdSalt, latch: Latch, z: f32) -> bool {
    let Latch { text, on, lit, tip } = latch;
    let response = ui.interact(rect, ui.scope_id().with(id), Sense::click());
    let hot = response.hovered();
    let (fill, stroke, ink) = if on {
        (lit.gamma_multiply(if hot { 1.0 } else { 0.85 }), lit, Color32::from_rgb(40, 18, 20))
    } else if hot {
        (theme::background::WIDGET_HOVERED, theme::background::WIDGET_ACTIVE, theme::text::PRIMARY)
    } else {
        (theme::background::WIDGET, theme::background::WIDGET_ACTIVE, theme::text::SECONDARY)
    };
    painter.rect(rect, 3.0 * z, fill, Stroke::new(1.0 * z, stroke), egui::StrokeKind::Middle);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(8.0 * z), ink);
    let clicked = response.clicked();
    response.on_hover_text(tip);
    clicked
}

/// How a channel's lights shine: in its hue, as loud as its meter, grey if
/// it can't be heard, and only where a voice sounds while the patch plays.
struct Lights<'a> {
    hue: Color32,
    is_heard: bool,
    meter: &'a PeakBallistics,
    playing: bool,
}

/// Draws one channel's lane of the stereo field.
fn draw_lane(painter: &egui::Painter, lane: Rect, strip: &StripView, lights: Lights, z: f32) {
    let Lights { hue, is_heard, meter, playing } = lights;
    painter.rect(lane, 4.0 * z, theme::background::MAIN, Stroke::new(1.0 * z, theme::background::GRID_MAJOR), egui::StrokeKind::Middle);
    let inset = 7.0 * z;
    let (x_left, x_right) = (lane.left() + inset, lane.right() - inset);
    let x_at = |pan: f32| x_left + (x_right - x_left) * (pan + 1.0) * 0.5;
    let y = lane.center().y;
    painter.line_segment(
        [Pos2::new(x_at(0.0), lane.top() + 3.0 * z), Pos2::new(x_at(0.0), lane.bottom() - 3.0 * z)],
        Stroke::new(1.0 * z, theme::background::GRID_MAJOR),
    );
    let lane_alpha = if strip.patched { 0.4 } else { 0.12 };
    painter.line_segment([Pos2::new(x_left, y), Pos2::new(x_right, y)], Stroke::new(1.0 * z, hue.gamma_multiply(lane_alpha)));
    if !strip.patched {
        return;
    }

    // Brightness follows the strip's meter, after its fader and Level CV,
    // so the lights breathe with the music rather than flicker
    let glow = if is_heard { meter.fraction().powf(1.5) } else { 0.0 };
    let count = strip.voices.len();
    // Many voices on one short lane: smaller lights, so each stays its own
    let crowd = (1.0 - (count as f32 - 1.0) / 14.0).clamp(0.55, 1.0);
    for (v, &peak) in strip.voices.iter().enumerate() {
        let center = Pos2::new(x_at(voice_pan(strip.pan, strip.width, v, count)), y);
        let sounding = !playing || peak.is_none_or(|p| p.abs() > 1e-4);
        if is_heard && strip.send > 0.01 {
            // The send: a soft smear along the lane, brightest at the
            // light, that swells as the channel plays
            let amount = strip.send.sqrt();
            let alpha = amount * (0.07 + 0.1 * glow);
            for layer in 1..=3 {
                let reach = (2.0 + 16.0 * amount * layer as f32 / 3.0) * z;
                let bloom = Rect::from_center_size(center, Vec2::new(2.0 * reach, 3.0 * z));
                painter.rect_filled(bloom, 1.5 * z, hue.gamma_multiply(alpha));
            }
        }
        if !is_heard {
            // Muted, or soloed out: a grey ring where it would be
            painter.circle_stroke(center, 2.4 * z, Stroke::new(1.0 * z, theme::text::DISABLED));
        } else if !sounding {
            // A voice with nothing to play: just its place
            painter.circle_stroke(center, 2.0 * z, Stroke::new(1.0 * z, hue.gamma_multiply(0.35)));
        } else {
            if glow > 0.02 {
                painter.circle_filled(center, (4.5 + 4.5 * glow) * crowd * z, hue.gamma_multiply(0.18 * glow));
            }
            painter.circle_filled(center, (2.2 + 1.5 * glow) * crowd * z, hue.gamma_multiply(0.45 + 0.55 * glow));
        }
    }
}

/// A parameter change from a button.
fn changed(node_id: NodeId, param_name: String, on: bool) -> NodeResponse<SynthResponse, SynthNodeData> {
    NodeResponse::User(SynthResponse::ParameterChanged { node_id, param_name, value: if on { 1.0 } else { 0.0 } })
}

/// A meter level for a tooltip.
fn format_db(db: f32) -> String {
    if db <= -48.0 {
        "-inf dBFS".to_string()
    } else {
        format!("{db:+.1} dBFS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::DspModule;

    #[test]
    fn test_every_jack_has_a_place() {
        let mixer = Mixer::new();
        for port in mixer.ports() {
            let (edge, _) = jack_slot(port.name).unwrap_or_else(|| panic!("{} has no place on the Mixer's face", port.name));
            assert_eq!(edge == Edge::Left, port.is_input(), "{} is on the wrong edge", port.name);
        }
        assert!(!places("Mute 1") && !places("Width 2"), "only jacks are placed");
    }

    #[test]
    fn test_jacks_never_crowd_each_other() {
        let mixer = Mixer::new();
        for edge in [Edge::Left, Edge::Right] {
            let mut ys: Vec<f32> = mixer.ports().iter().filter_map(|p| jack_slot(p.name)).filter(|(e, _)| *e == edge).map(|(_, y)| y).collect();
            ys.sort_by(f32::total_cmp);
            for pair in ys.windows(2) {
                assert!(pair[1] - pair[0] >= geometry::JACK_PITCH - 1e-3, "{edge:?} jacks {pair:?} overlap");
            }
            assert!(ys.iter().all(|&y| y > 0.0 && y < geometry::height()), "{edge:?} jack off the display");
        }
    }

    #[test]
    fn test_each_channels_jacks_sit_in_its_row() {
        for s in 0..STRIPS {
            let n = s + 1;
            let (top, bottom) = (geometry::row_top(s), geometry::row_top(s + 1));
            for name in [format!("Ch {n}"), format!("Level {n}"), format!("Pan {n}"), format!("Send {n}")] {
                let (_, y) = jack_slot(&name).unwrap();
                assert!(y > top && y < bottom, "{name} at {y} is outside its row {top}..{bottom}");
            }
        }
    }

    #[test]
    fn test_jack_labels() {
        assert_eq!(jack_label("Ch 3"), "in");
        assert_eq!(jack_label("Level 2"), "lvl");
        assert_eq!(jack_label("Pan 4"), "pan");
        assert_eq!(jack_label("Send 1"), "snd");
        assert_eq!(jack_label("Chain In"), "chain");
        assert_eq!(jack_label("Return R"), "ret R");
        assert_eq!(jack_label("Return"), "return");
    }
}
