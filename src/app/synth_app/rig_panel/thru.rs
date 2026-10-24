//! The Rig's Thru section (#446): routes that pass a MIDI In straight to
//! a MIDI Out port, past the patch. A row a route: a lamp that flashes as
//! it passes, a tick to set it aside, where it comes from and goes, the
//! channel it passes and the one it moves them to, the kinds it passes, and
//! what it's doing. The routing itself is [`crate::link::midi_thru`]'s.

use super::*;
use crate::link::midi_thru::{is_loop, Kinds, RouteState, ThruRoute, ROUTES, THRU};

/// A route's columns: the lamp, the tick, where it comes from, an arrow,
/// the port, the channel, the channel it becomes, the kinds, what it's
/// doing, and the cross that takes it away.
const THRU_COLUMNS: [f32; 10] = [14.0, 22.0, 150.0, 12.0, 128.0, 66.0, 66.0, 166.0, 70.0, 14.0];
/// Each kind's chip, as wide as its name.
const CHIP_WIDTHS: [f32; 5] = [38.0, 24.0, 32.0, 34.0, 30.0];

impl SynthApp {
    /// Hands the MIDI engine's callbacks every route as it stands now: the
    /// slot its input is open in, and whether its port's device would make
    /// it a loop. Each frame, as inputs open and ports change. Returns what
    /// each route is doing.
    pub(in crate::app::synth_app) fn follow_thru_routes(&self) -> Vec<RouteState> {
        if THRU.is_empty() {
            // Clears any route just taken away; nothing else to look up
            return THRU.follow(|_| None, &Default::default(), &Default::default());
        }
        THRU.follow(|name| self.midi.slot_of(name), &MIDI.all_chosen(), &MIDI.found())
    }
}

/// How tall the Thru section is with `routes` routes: for the inputs to
/// leave it room.
pub(super) fn thru_height(routes: usize) -> f32 {
    (routes.max(1) + 3) as f32 * (ROW_HEIGHT + 4.0)
}

/// The Thru section: the routes, and a way to add one. Edits are made on
/// [`THRU`]; opening a route's input joins `edits`.
pub(super) fn thru_section(ui: &mut egui::Ui, ins: &[InRow], outs: &[OutRow], states: &[RouteState], now: f64, edits: &mut Vec<Edit>) {
    section(ui, "Thru", "a MIDI In straight to a port, past the patch");
    let mut routes = THRU.routes();
    let before = routes.clone();
    if routes.is_empty() {
        quiet_line(ui, "No routes. A route plays one device from another with the least delay Soba has: the Fantom's keys on the MicroFreak, say, while the patch does something else");
    } else {
        header(ui, &["", "", "From", "", "To", "Channel", "Becomes", "Passes", "", ""], &THRU_COLUMNS);
        let mut gone = None;
        for (index, route) in routes.iter_mut().enumerate() {
            let state = states.get(index).copied().unwrap_or(RouteState::Off);
            if route_row(ui, index, route, state, ins, outs, now, edits) {
                gone = Some(index);
            }
        }
        if let Some(index) = gone {
            routes.remove(index);
        }
    }
    ui.add_space(2.0);
    let full = routes.len() >= ROUTES;
    let tip = if ins.is_empty() {
        "Plug in a MIDI input first"
    } else if full {
        "That's every route there can be"
    } else {
        "A new route, from an input to a port: it passes everything but clock until you say otherwise"
    };
    if ui.add_enabled(!ins.is_empty() && !full, egui::Button::new("+ Route").small()).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
        routes.push(new_route(ins, outs));
    }
    if routes != before {
        THRU.edit(|kept| *kept = routes);
    }
}

/// A route to start from: from the first input open (or found), to the
/// first port with a device it wouldn't loop to.
fn new_route(ins: &[InRow], outs: &[OutRow]) -> ThruRoute {
    let from = ins.iter().find(|row| row.slot.is_some()).or(ins.first()).map(|row| row.name.clone()).unwrap_or_default();
    let to = outs
        .iter()
        .find(|row| row_device(row).is_some_and(|device| !is_loop(&from, device)))
        .map_or(0, |row| row.port);
    ThruRoute::new(from, to)
}

/// The device a port plays, as it's called now if it's plugged in.
fn row_device(row: &OutRow) -> Option<&str> {
    row.found.as_deref().or(row.chosen.as_deref())
}

/// A route's row. True when its cross was clicked.
#[allow(clippy::too_many_arguments)]
fn route_row(ui: &mut egui::Ui, index: usize, route: &mut ThruRoute, state: RouteState, ins: &[InRow], outs: &[OutRow], now: f64, edits: &mut Vec<Edit>) -> bool {
    let id = egui::Id::unique(("rig_thru", index));
    let since = since(ui, id, THRU.forwarded(index), now);
    let port = PORT_LABELS[route.to.min(PORTS - 1)];
    let (lamp_color, text, ink, tip) = match state {
        RouteState::Passing => (theme::signal::GATE, "Thru".to_string(), theme::text::SECONDARY, format!("Passing what {} plays to port {port}, at once", route.from)),
        RouteState::Off => (theme::background::GRID_MAJOR, "Off".to_string(), theme::text::DISABLED, "Set aside: tick it to pass again".to_string()),
        RouteState::Closed => (
            theme::accent::WARNING,
            "Open input".to_string(),
            theme::accent::WARNING,
            "Its input is closed, so Soba hears nothing to pass. Click to open it (modules set to any device hear it too)".to_string(),
        ),
        RouteState::Loop => (
            theme::accent::ERROR,
            "Loop".to_string(),
            theme::accent::ERROR,
            format!("Port {port} plays the device this route comes from: it would play itself, so the route is refused"),
        ),
        RouteState::NoDevice => (theme::accent::WARNING, "No device".to_string(), theme::accent::WARNING, format!("Port {port} has no device: choose one under MIDI Out")),
        RouteState::Unplugged => (theme::accent::WARNING, "Unplugged".to_string(), theme::accent::WARNING, format!("Port {port}'s device isn't plugged in: the route passes again when it is")),
    };
    let mut gone = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let [lamp_w, tick_w, from_w, arrow_w, to_w, channel_w, remap_w, kinds_w, state_w, cross_w] = THRU_COLUMNS;
        cell(ui, lamp_w, |ui| lamp(ui, lamp_color, (state == RouteState::Passing).then_some(since)));
        cell(ui, tick_w, |ui| {
            ui.checkbox(&mut route.on, "").on_hover_text(if route.on { "Set it aside: it passes nothing" } else { "Pass again" });
        });
        cell(ui, from_w, |ui| from_menu(ui, index, route, ins));
        cell(ui, arrow_w, |ui| {
            ui.label(RichText::new("→").color(theme::text::QUIET));
        });
        cell(ui, to_w, |ui| to_menu(ui, index, route, outs));
        cell(ui, channel_w, |ui| {
            channel_menu(ui, ("rig_thru_channel", index), &mut route.channel, "All", "The channel it passes: one, or all of them")
        });
        cell(ui, remap_w, |ui| {
            channel_menu(ui, ("rig_thru_remap", index), &mut route.remap, "Same", "The channel they go out on: each as it came, or all on one")
        });
        cell(ui, kinds_w, |ui| kind_chips(ui, &mut route.kinds));
        cell(ui, state_w, |ui| {
            let input = ins.iter().find(|row| row.name == route.from).and_then(|row| row.index);
            match input.filter(|_| state == RouteState::Closed) {
                Some(input) => {
                    let label = egui::Label::new(RichText::new(text).small().color(ink)).sense(egui::Sense::click());
                    if ui.add(label).on_hover_text(tip).clicked() {
                        edits.push(Edit::Toggle(input));
                    }
                }
                // Not plugged in: nothing to open
                None if state == RouteState::Closed => {
                    let tip = format!("{} isn't plugged in: the route passes again once it's back, and open under MIDI In", route.from);
                    ui.label(RichText::new("No input").small().color(ink)).on_hover_text(tip);
                }
                None => {
                    ui.label(RichText::new(text).small().color(ink)).on_hover_text(tip);
                }
            }
        });
        cell(ui, cross_w, |ui| {
            gone = cross(ui, 12.0).on_hover_text("Take the route away").clicked();
        });
    });
    gone
}

/// Slim, so a route's row is no taller than an input's.
fn slim(ui: &mut egui::Ui) {
    ui.spacing_mut().button_padding.y = 1.0;
    ui.spacing_mut().interact_size.y = ROW_HEIGHT - 4.0;
}

/// The input a route hears, to choose from those found and remembered.
fn from_menu(ui: &mut egui::Ui, index: usize, route: &mut ThruRoute, ins: &[InRow]) {
    slim(ui);
    let known = ins.iter().find(|row| row.name == route.from);
    let shown = match known {
        Some(row) if row.index.is_none() => format!("{} (unplugged)", route.from),
        Some(_) => route.from.clone(),
        None => format!("{} (not found)", route.from),
    };
    let ink = if known.is_some_and(|row| row.slot.is_some()) { theme::text::PRIMARY } else { theme::text::SECONDARY };
    egui::ComboBox::from_id_salt(("rig_thru_from", index))
        .width(THRU_COLUMNS[2] - 4.0)
        .truncate()
        .selected_text(RichText::new(shown).color(ink))
        .show_ui(ui, |ui| {
            ui.set_min_width(200.0);
            for row in ins {
                let closed = if row.index.is_none() { " (unplugged)" } else if row.slot.is_none() { " (closed)" } else { "" };
                if ui.selectable_label(row.name == route.from, format!("{}{closed}", row.name)).clicked() {
                    route.from = row.name.clone();
                }
            }
        })
        .response
        .on_hover_text("The MIDI In it passes on");
}

/// The port a route plays, to choose: one whose device is the route's
/// input can't be, as it would loop.
fn to_menu(ui: &mut egui::Ui, index: usize, route: &mut ThruRoute, outs: &[OutRow]) {
    slim(ui);
    let label = |row: &OutRow| format!("{}  {}", PORT_LABELS[row.port], row_device(row).unwrap_or("—"));
    let current = outs.iter().find(|row| row.port == route.to);
    let ink = if current.and_then(row_device).is_some() { theme::text::PRIMARY } else { theme::text::DISABLED };
    egui::ComboBox::from_id_salt(("rig_thru_to", index))
        .width(THRU_COLUMNS[4] - 4.0)
        .truncate()
        .selected_text(RichText::new(current.map(label).unwrap_or_default()).color(ink))
        .show_ui(ui, |ui| {
            ui.set_min_width(200.0);
            for row in outs {
                let looped = row_device(row).is_some_and(|device| is_loop(&route.from, device));
                let response = ui.add_enabled(!looped, egui::Button::selectable(row.port == route.to, label(row)));
                let response = response.on_disabled_hover_text(format!("Port {} plays {}, the device this route comes from: it would loop", PORT_LABELS[row.port], route.from));
                if response.clicked() {
                    route.to = row.port;
                }
            }
        })
        .response
        .on_hover_text("The MIDI Out port it plays: the port's device, as set under MIDI Out");
}

/// A channel to choose, or `none` for every one (or each as it is).
fn channel_menu(ui: &mut egui::Ui, salt: (&str, usize), channel: &mut Option<u8>, none: &str, tip: &str) {
    slim(ui);
    let shown = channel.map_or(none.to_string(), |c| format!("Ch {}", c + 1));
    let ink = if channel.is_some() { theme::text::PRIMARY } else { theme::text::DISABLED };
    egui::ComboBox::from_id_salt(salt)
        .width(THRU_COLUMNS[5] - 4.0)
        .truncate()
        .selected_text(RichText::new(shown).small().color(ink))
        .show_ui(ui, |ui| {
            ui.set_min_width(70.0);
            if ui.selectable_label(channel.is_none(), none).clicked() {
                *channel = None;
            }
            for c in 0..16u8 {
                if ui.selectable_label(*channel == Some(c), format!("Ch {}", c + 1)).clicked() {
                    *channel = Some(c);
                }
            }
        })
        .response
        .on_hover_text(tip);
}

/// The kinds a route passes, a chip each to click on and off.
fn kind_chips(ui: &mut egui::Ui, kinds: &mut Kinds) {
    ui.spacing_mut().item_spacing.x = 3.0;
    for ((kind, name, _), width) in Kinds::EACH.into_iter().zip(CHIP_WIDTHS) {
        let on = kinds.has(kind);
        let tip = match kind {
            Kinds::NOTES => "Notes on and off",
            Kinds::CONTROL => "Control changes: knobs, faders, the mod wheel",
            Kinds::EXPRESSION => "Pitch bend and pressure, channel and per note",
            Kinds::CLOCK => "Clock, Start, Stop and Song Position. Off by default: beside the port's own Clock it would run the device at double time",
            _ => "Program changes",
        };
        if chip(ui, name, width, on).on_hover_text(tip).clicked() {
            *kinds = kinds.with(kind, !on);
        }
    }
}

/// A small rounded plate with `text` in it, lit while `on`: the port's
/// badge, as a toggle.
fn chip(ui: &mut egui::Ui, text: &str, width: f32, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 16.0), egui::Sense::click());
    let purple = theme::signal::MIDI;
    let (fill, ink) = if on { (purple.gamma_multiply(0.28), purple.lerp_to_gamma(theme::text::PRIMARY, 0.6)) } else { (theme::background::WIDGET, theme::text::DISABLED) };
    let fill = if response.hovered() { fill.lerp_to_gamma(purple, 0.15) } else { fill };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(10.5), ink);
    response
}
