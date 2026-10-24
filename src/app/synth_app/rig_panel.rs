//! The Rig panel (#436): every MIDI input and every MIDI Out port on one
//! panel, for a room of hardware.
//!
//! - **MIDI In**, a row for each input found and each remembered one that
//!   isn't plugged in: a lamp that flashes as it's heard, a tick to open
//!   or close it, its slot, the last message heard, and the modules set to
//!   hear it alone.
//! - **MIDI Out**, a row for each port, A to H: a lamp that flashes as it
//!   sends, its device to choose, what it sends of the Clock (#437), its
//!   offset, how it's doing, the last message sent (and a dot on each beat
//!   while clock goes out), and the MIDI Outs that play it.
//! - **Thru** (#446), a row for each route from a MIDI In straight to a
//!   port, past the patch: see [`thru`].
//!
//! Both are this computer's settings, not the patch's. A device the system
//! renumbers after a reboot ("2- Digitakt", now "3- Digitakt") is found
//! again under its new name, and the row says so (see
//! [`device_names`](crate::engine::device_names)). An input's device
//! spans the ports' clock and offset, so the columns after line up.
//!
//! By each port's offset, a ping glyph measures its device's delay and
//! offers the offset that lines it up (#449): see [`measure`].

mod measure;

use super::*;
use crate::engine::device_names;
use crate::engine::MidiEvent;
use crate::graph::midi_log_display::format_midi_event;
use crate::link::midi::{ClockMode, PortStatus, Traffic, MIDI, OFFSET_RANGE, PORTS, PORT_LABELS};
use crate::persistence::graph_io::clocked_ports;
use measure::{measure_line, ping_button, Measure, MeasureEdit};

mod thru;

/// The panel: open or not.
#[derive(Debug, Default)]
pub(super) struct RigPanel {
    pub open: bool,
    /// A port's device being measured, or the last one measured (#449).
    measure: Measure,
}

/// How long a lamp's flash glows, in seconds.
const FLASH: f64 = 0.25;
/// How long the last message stays bright before it settles, in seconds.
const FRESH: f64 = 4.0;
/// The columns' widths, in order: the lamp, the tick or port, the device,
/// the port's clock and offset, how it's doing, the last message, and the
/// modules. An input's row has no clock or offset: see [`in_columns`].
const COLUMNS: [f32; 8] = [14.0, 22.0, 184.0, 116.0, 86.0, 84.0, 160.0, 96.0];
/// The space between columns.
const GAP: f32 = 6.0;
const ROW_HEIGHT: f32 = 22.0;
/// The clock ticks to a beat.
const PPQN: u32 = 24;

/// An input's row.
struct InRow {
    name: String,
    /// Its place in the inputs found, or `None` while it's unplugged.
    index: Option<usize>,
    /// Its slot while it's open.
    slot: Option<u8>,
    traffic: crate::app::midi_session::InputTraffic,
    /// The modules set to hear this device alone.
    heard_by: Vec<String>,
}

/// A port's row.
struct OutRow {
    port: usize,
    chosen: Option<String>,
    /// What its device is called now, if it's plugged in.
    found: Option<String>,
    status: PortStatus,
    traffic: Traffic,
    /// What it sends of the Clock.
    clock: ClockMode,
    /// Whether its device leads the Clock (#438), and is sent none of it.
    leads: bool,
    /// How much later than the sound it sends, in milliseconds.
    offset: f32,
    /// The MIDI Outs on this port.
    played_by: Vec<String>,
}

/// What a row asked for.
enum Edit {
    Toggle(usize),
    Forget(String),
    Choose(usize, Option<String>),
    Clock(usize, ClockMode),
    Offset(usize, f32),
    Measure(MeasureEdit),
    Refresh,
}

impl SynthApp {
    /// Opens the Rig panel, or closes it.
    pub(super) fn toggle_rig_panel(&mut self) {
        self.rig_panel.open = !self.rig_panel.open;
        if self.rig_panel.open {
            MIDI.rescan();
        }
    }

    /// The Rig panel, while it's open.
    pub(super) fn show_rig_panel(&mut self, ctx: &egui::Context) {
        self.tend_measurement(self.rig_panel.open);
        if !self.rig_panel.open {
            return;
        }
        let ins = self.rig_ins();
        let outs = self.rig_outs();
        let devices = MIDI.devices();
        let error = self.midi.error.clone();
        let has_inputs = self.midi.engine.is_some();
        let mut open = true;
        let mut edits = Vec::new();
        let editor = self.editor_rect;
        let width: f32 = COLUMNS.iter().sum::<f32>() + GAP * (COLUMNS.len() - 1) as f32;
        let earliest = MIDI.earliest();
        let now = ctx.input(|i| i.time);
        let routes = self.follow_thru_routes();
        let measure = &self.rig_panel.measure;
        let input_width = self.audio.engine.as_ref().ok().and_then(|engine| engine.input_channels()).map_or(2, usize::from);
        let input = (self.user_state.audio_input_name.as_deref(), input_width);
        egui::Window::new("The Rig")
            .id(egui::Id::unique("rig_panel"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(editor.right_top() + egui::vec2(-16.0, 16.0))
            .show(ctx, |ui| {
                ui.set_width(width);
                let listening = ins.iter().filter(|row| row.slot.is_some()).count();
                let playing = outs.iter().filter(|row| row.chosen.is_some()).count();
                if heading(ui, listening, playing) {
                    open = false;
                }
                #[cfg(target_arch = "wasm32")]
                if !web_access(ui) {
                    return;
                }
                // The eight ports always show; the inputs scroll in what's left
                let room = (editor.height() - 300.0 - PORTS as f32 * (ROW_HEIGHT + 4.0) - thru::thru_height(routes.len())).max(3.0 * ROW_HEIGHT);

                ui.add_space(10.0);
                section(ui, "MIDI In", "what plays Soba");
                header(ui, &["", "", "Device", "", "Last heard", "Heard by"], &in_columns());
                if !has_inputs {
                    quiet_line(ui, error.as_deref().unwrap_or("MIDI in isn't available here: a DAW hands the plugin its MIDI"));
                } else if ins.is_empty() {
                    quiet_line(ui, "No MIDI inputs found. Plug something in: it shows up here within a couple of seconds");
                } else {
                    egui::ScrollArea::vertical().id_salt("rig_ins").max_height(room).auto_shrink([false, true]).show(ui, |ui| {
                        for row in &ins {
                            in_row(ui, row, now, &mut edits);
                        }
                    });
                }
                if let Some(error) = error.as_deref().filter(|_| has_inputs) {
                    ui.label(RichText::new(error).small().color(theme::accent::ERROR));
                }

                ui.add_space(12.0);
                section(ui, "MIDI Out", "what Soba plays");
                header(ui, &["", "Port", "Device", "Clock", "Offset", "", "Last sent", "Played by"], &COLUMNS);
                for row in &outs {
                    out_row(ui, row, &devices, earliest, now, measure, &mut edits);
                    measure_line(ui, measure, row.port, input, &mut edits);
                }

                ui.add_space(12.0);
                thru::thru_section(ui, &ins, &outs, &routes, now, &mut edits);

                ui.add_space(8.0);
                footer(ui, &mut edits);
            });
        self.rig_panel.open &= open;
        // The sender's lamps move without input to wake a frame
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        for edit in edits {
            match edit {
                Edit::Toggle(index) => self.toggle_midi_device(index),
                Edit::Forget(name) => self.midi.remembered.retain(|n| *n != name),
                Edit::Choose(port, device) => MIDI.choose(port, device.as_deref()),
                Edit::Clock(port, mode) => MIDI.set_clock(port, mode),
                Edit::Offset(port, ms) => MIDI.set_offset(port, ms),
                Edit::Measure(edit) => self.measure_edit(edit),
                Edit::Refresh => {
                    self.refresh_midi_devices();
                    MIDI.rescan();
                }
            }
        }
    }

    /// The inputs: each found, then each remembered and not plugged in.
    fn rig_ins(&self) -> Vec<InRow> {
        let found = self.midi.devices.iter().map(|device| (device.name.clone(), Some(device.index)));
        let unplugged = self
            .midi
            .remembered
            .iter()
            .filter(|name| !self.midi.devices.iter().any(|d| d.name == **name))
            .map(|name| (name.clone(), None));
        found
            .chain(unplugged)
            .map(|(name, index)| {
                let slot = index.and_then(|_| self.midi.engine.as_ref()?.slot_of(&name));
                let traffic = slot.and_then(|slot| self.midi.traffic.get(slot as usize).copied()).unwrap_or_default();
                let heard_by = self.module_names(|data| crate::graph::midi_device::device(data).is_some_and(|device| device_names::same(device, &name)));
                InRow { name, index, slot, traffic, heard_by }
            })
            .collect()
    }

    /// The ports, A to H.
    fn rig_outs(&self) -> Vec<OutRow> {
        let found = MIDI.found();
        let graph = &self.graph_state.graph;
        (0..PORTS)
            .map(|port| {
                let played_by = graph
                    .nodes
                    .iter()
                    .filter(|(_, node)| node.user_data.module_id == crate::modules::midi_out::ID)
                    .filter(|(_, node)| {
                        let input = node.inputs.iter().find(|(name, _)| name == "Port").map(|(_, id)| graph.get_input(*id));
                        let on = match input.map(|input| &input.value) {
                            Some(SynthValueType::Select { value, .. }) => *value,
                            _ => 0,
                        };
                        on.min(PORTS - 1) == port
                    })
                    .map(|(_, node)| node.user_data.display_name.clone())
                    .collect();
                OutRow {
                    port,
                    chosen: MIDI.chosen(port),
                    found: found[port].clone(),
                    status: MIDI.status(port),
                    traffic: MIDI.traffic(port),
                    clock: MIDI.clock(port),
                    leads: MIDI.led_ports() & 1 << port != 0 && MIDI.clock(port) != ClockMode::Off,
                    offset: MIDI.offset(port),
                    played_by,
                }
            })
            .collect()
    }

    /// The names of the modules whose data `matches`.
    fn module_names(&self, matches: impl Fn(&SynthNodeData) -> bool) -> Vec<String> {
        self.graph_state.graph.nodes.iter().filter(|(_, node)| matches(&node.user_data)).map(|(_, node)| node.user_data.display_name.clone()).collect()
    }
}

/// Turns on the clock of each port an old patch's MIDI Outs sent it on, with
/// Send Clock, before clock was the port's (#437), unless it's on already.
/// Says which, for the patch's notes on opening.
pub(super) fn clock_old_midi_outs(patch: &crate::persistence::Patch) -> Vec<String> {
    clocked_ports(patch)
        .into_iter()
        .filter(|&port| MIDI.clock(port) == ClockMode::Off)
        .map(|port| {
            MIDI.set_clock(port, ClockMode::Transport);
            format!(
                "A MIDI Out's Send Clock is now port {}'s, in the Rig (Ctrl+Shift+H): turned on there, for this computer",
                PORT_LABELS[port]
            )
        })
        .collect()
}

/// An input's columns: [`COLUMNS`] with the device spanning the ports'
/// clock and offset.
fn in_columns() -> [f32; 6] {
    let [lamp, tick, device, clock, offset, state, last, who] = COLUMNS;
    [lamp, tick, device + clock + offset + 2.0 * GAP, state, last, who]
}

/// The panel's name, after a short purple bar, what's open, and the way
/// to close it. True when it's closed.
fn heading(ui: &mut egui::Ui, listening: usize, playing: usize) -> bool {
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 15.0), egui::Sense::hover());
        ui.painter().rect_filled(bar, 1.5, theme::signal::MIDI);
        let family = egui::FontFamily::Name(theme::TITLE_FAMILY.into());
        ui.label(RichText::new("The Rig").font(egui::FontId::new(16.0, family)).color(theme::text::PRIMARY));
        let count = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        let summary = format!("{} · {}", count(listening, "input open", "inputs open"), count(playing, "port with a device", "ports with devices"));
        ui.label(RichText::new(summary).small().color(theme::text::QUIET));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let keys = shortcuts::keys(ui.ctx(), "Ctrl+Shift+H");
            cross(ui, 14.0).on_hover_text(format!("Close the Rig ({keys})")).clicked()
        })
        .inner
    })
    .inner
}

/// A section's name, and a word on which way the MIDI goes.
fn section(ui: &mut egui::Ui, name: &str, says: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).strong().color(theme::signal::MIDI.lerp_to_gamma(theme::text::PRIMARY, 0.45)));
        ui.label(RichText::new(says).small().color(theme::text::QUIET));
    });
}

/// The columns' names, `widths` wide, and a line under them.
fn header(ui: &mut egui::Ui, names: &[&str], widths: &[f32]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (name, &width) in names.iter().zip(widths) {
            cell(ui, width, |ui| {
                ui.label(RichText::new(*name).small().color(theme::text::QUIET));
            });
        }
    });
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(line, 0.0, theme::background::GRID_MAJOR);
}

/// A line of quiet text across the panel.
fn quiet_line(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.add(egui::Label::new(RichText::new(text).color(theme::text::SECONDARY)).wrap());
}

/// A column's cell, `width` wide, its contents centred down the row.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(egui::vec2(width, ROW_HEIGHT), Layout::left_to_right(Align::Center), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

/// An input's row, its edits joining `edits`.
fn in_row(ui: &mut egui::Ui, row: &InRow, now: f64, edits: &mut Vec<Edit>) {
    let id = egui::Id::unique(("rig_in", &row.name));
    let since = since(ui, id, row.traffic.count, now);
    let green = theme::signal::GATE;
    let (lamp_color, state, state_color) = match (row.index, row.slot) {
        (None, _) => (theme::accent::WARNING, "unplugged".to_string(), theme::accent::WARNING),
        (Some(_), Some(slot)) => (green, format!("In {}", slot + 1), theme::text::SECONDARY),
        (Some(_), None) => (theme::background::GRID_MAJOR, "closed".to_string(), theme::text::DISABLED),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let [lamp_w, tick_w, device_w, state_w, last_w, who_w] = in_columns();
        cell(ui, lamp_w, |ui| lamp(ui, lamp_color, row.slot.map(|_| since)));
        cell(ui, tick_w, |ui| match row.index {
            Some(index) => {
                let mut on = row.slot.is_some();
                let tip = if on { "Close it: Soba stops listening" } else { "Open it: Note Ins, Pads, Controls, the Clock and MIDI Learn hear it" };
                if ui.checkbox(&mut on, "").on_hover_text(tip).clicked() {
                    edits.push(Edit::Toggle(index));
                }
            }
            None => {
                if cross(ui, 12.0).on_hover_text("Forget it: Soba stops looking for it").clicked() {
                    edits.push(Edit::Forget(row.name.clone()));
                }
            }
        });
        cell(ui, device_w, |ui| {
            let ink = if row.slot.is_some() { theme::text::PRIMARY } else { theme::text::SECONDARY };
            ui.add(egui::Label::new(RichText::new(&row.name).color(ink)).truncate());
        });
        cell(ui, state_w, |ui| {
            ui.label(RichText::new(state).small().color(state_color));
        });
        cell(ui, last_w, |ui| last(ui, row.traffic.last, since));
        cell(ui, who_w, |ui| who(ui, &row.heard_by, "module", "modules", "set to hear this device alone"));
    });
}

/// A port's row, its edits joining `edits`.
fn out_row(ui: &mut egui::Ui, row: &OutRow, devices: &[String], earliest: f32, now: f64, measure: &Measure, edits: &mut Vec<Edit>) {
    let id = egui::Id::unique(("rig_out", row.port));
    let since = since(ui, id, row.traffic.messages, now);
    // A port sending clock is in use, whether a MIDI Out plays it or not
    let used = !row.played_by.is_empty() || row.clock != ClockMode::Off;
    let (state, color) = match row.status {
        PortStatus::Sending => ("Sending", theme::signal::GATE),
        PortStatus::Failed => ("In use", theme::accent::ERROR),
        PortStatus::Missing => ("Unplugged", theme::accent::WARNING),
        PortStatus::NoDevice => ("No device", theme::accent::WARNING),
        PortStatus::Idle if row.chosen.is_none() => ("", theme::background::GRID_MAJOR),
        PortStatus::Idle if row.found.is_none() => ("Unplugged", theme::accent::WARNING.gamma_multiply(0.7)),
        PortStatus::Idle => ("Ready", theme::signal::GATE.gamma_multiply(0.55)),
    };
    let flashing = (row.status == PortStatus::Sending).then_some(since);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let [lamp_w, port_w, device_w, clock_w, offset_w, state_w, last_w, who_w] = COLUMNS;
        cell(ui, lamp_w, |ui| lamp(ui, color, flashing));
        cell(ui, port_w, |ui| badge(ui, PORT_LABELS[row.port], row.chosen.is_some() || used));
        cell(ui, device_w, |ui| device_menu(ui, row, devices, edits));
        cell(ui, clock_w, |ui| clock_menu(ui, row, edits));
        cell(ui, offset_w, |ui| {
            offset_drag(ui, row, earliest, edits);
            ping_button(ui, measure, row.port, edits);
        });
        cell(ui, state_w, |ui| {
            let text = if state.is_empty() && used { "No device" } else { state };
            let ink = if state.is_empty() && used { theme::accent::WARNING } else if state == "Ready" || state.is_empty() { theme::text::QUIET } else { color };
            ui.label(RichText::new(text).small().color(ink));
            beat(ui, id, row.traffic.clocks, now);
        });
        cell(ui, last_w, |ui| {
            let event = row.traffic.last.and_then(|message| MidiEvent::from_bytes(message.bytes()));
            last(ui, event, since);
        });
        cell(ui, who_w, |ui| who(ui, &row.played_by, "MIDI Out", "MIDI Outs", "playing this port"));
    });
}

/// The device a port plays, to choose: every output found, or none. A
/// device the system has renumbered shows as it's called now.
fn device_menu(ui: &mut egui::Ui, row: &OutRow, devices: &[String], edits: &mut Vec<Edit>) {
    let shown = match (&row.chosen, &row.found) {
        (None, _) => "None".to_string(),
        (Some(_), Some(found)) => found.clone(),
        (Some(chosen), None) => format!("{chosen} (unplugged)"),
    };
    // Slim, so a row is no taller than an input's
    ui.spacing_mut().button_padding.y = 1.0;
    ui.spacing_mut().interact_size.y = ROW_HEIGHT - 4.0;
    let response = egui::ComboBox::from_id_salt(("rig_out_device", row.port))
        .width(COLUMNS[2] - 4.0)
        .truncate()
        .selected_text(RichText::new(shown).color(if row.chosen.is_some() { theme::text::PRIMARY } else { theme::text::DISABLED }))
        .show_ui(ui, |ui| {
            ui.set_min_width(200.0);
            if devices.is_empty() {
                ui.label(RichText::new("No MIDI outputs found").weak());
            }
            for device in devices {
                let current = row.found.as_deref() == Some(device.as_str());
                if ui.selectable_label(current, device).clicked() {
                    edits.push(Edit::Choose(row.port, Some(device.clone())));
                }
            }
            // Chosen before, on another day, and not plugged in now
            if let Some(missing) = row.chosen.as_ref().filter(|_| row.found.is_none()) {
                let _ = ui.selectable_label(true, format!("{missing} (unplugged)"));
            }
            ui.separator();
            if ui.selectable_label(row.chosen.is_none(), "None").clicked() {
                edits.push(Edit::Choose(row.port, None));
            }
        })
        .response;
    match (&row.chosen, &row.found) {
        (Some(chosen), Some(found)) if chosen != found => {
            response.on_hover_text(format!("Chosen as “{chosen}”. The system has renumbered it since, and port {} found it again", PORT_LABELS[row.port]));
        }
        _ => {
            response.on_hover_text(format!("Which device port {} plays, on this computer", PORT_LABELS[row.port]));
        }
    }
}

/// What a port sends of the Clock, to choose.
fn clock_menu(ui: &mut egui::Ui, row: &OutRow, edits: &mut Vec<Edit>) {
    ui.spacing_mut().button_padding.y = 1.0;
    ui.spacing_mut().interact_size.y = ROW_HEIGHT - 4.0;
    // A device the Clock follows reads "leads", in MIDI purple, until it stops
    let ink = match row.clock {
        _ if row.leads => theme::signal::MIDI,
        ClockMode::Off => theme::text::DISABLED,
        _ => theme::text::PRIMARY,
    };
    let selected = if row.leads { "leads" } else { row.clock.label() };
    let response = egui::ComboBox::from_id_salt(("rig_out_clock", row.port))
        .width(COLUMNS[3] - 4.0)
        .truncate()
        .selected_text(RichText::new(selected).small().color(ink))
        .show_ui(ui, |ui| {
            ui.set_min_width(150.0);
            for mode in ClockMode::ALL {
                let hint = match mode {
                    ClockMode::Off => "No clock: the device keeps its own tempo",
                    ClockMode::Transport => "The Clock's tempo, and Start, Stop and where the song is: the device plays when Soba plays",
                    ClockMode::ClockOnly => "The Clock's tempo alone: the device follows it, and starts and stops on its own",
                };
                if ui.selectable_label(row.clock == mode, mode.label()).on_hover_text(hint).clicked() {
                    edits.push(Edit::Clock(row.port, mode));
                }
            }
        })
        .response;
    response.on_hover_text(if row.leads {
        format!(
            "Port {}'s device leads the Clock, so it's sent none of the clock back. When it stops, Soba plays on and sends it {} again, starting it with Soba's next Start",
            PORT_LABELS[row.port],
            row.clock.label()
        )
    } else {
        format!("What port {} sends of the patch's Clock, once for the port however many MIDI Outs play it", PORT_LABELS[row.port])
    });
}

/// A port's offset, in milliseconds, to drag: amber when it asks for
/// earlier than the output's delay allows (`earliest`).
fn offset_drag(ui: &mut egui::Ui, row: &OutRow, earliest: f32, edits: &mut Vec<Edit>) {
    let mut ms = row.offset;
    let clipped = ms < earliest - 0.05;
    let ink = if clipped {
        theme::accent::WARNING
    } else if ms == 0.0 {
        theme::text::DISABLED
    } else {
        theme::text::PRIMARY
    };
    // Slim and small, as the clock and device beside it
    ui.visuals_mut().override_text_color = Some(ink);
    ui.style_mut().override_text_style = Some(egui::TextStyle::Small);
    ui.spacing_mut().button_padding.y = 1.0;
    ui.spacing_mut().interact_size.y = ROW_HEIGHT - 4.0;
    let drag = egui::DragValue::new(&mut ms)
        .range(-OFFSET_RANGE..=OFFSET_RANGE)
        .speed(0.2)
        .custom_formatter(|ms, _| if ms.abs() < 0.05 { "0".to_string() } else { format!("{ms:+.1}") })
        .suffix(" ms");
    // Room beside it for the ping glyph
    let response = ui.add_sized([COLUMNS[4] - 26.0, ROW_HEIGHT - 4.0], drag);
    if response.changed() {
        edits.push(Edit::Offset(row.port, ms));
    }
    if clipped {
        response.on_hover_text(format!(
            "Port {} can go no earlier than {earliest:.1} ms: a message can't leave before its sound is made, and the output's delay is all the head start there is. A larger buffer gives more",
            PORT_LABELS[row.port]
        ));
    } else {
        response.on_hover_text(format!(
            "How much later than the sound port {}'s notes, CCs and clock go out: nudge it until a slow device lands with Soba. Negative goes earlier, as far as {earliest:.1} ms",
            PORT_LABELS[row.port]
        ));
    }
}

/// A lamp: `color`, with a halo as bright as how lately it flashed, `since`
/// seconds ago, while it can flash.
fn lamp(ui: &mut egui::Ui, color: egui::Color32, since: Option<f64>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(COLUMNS[0], ROW_HEIGHT), egui::Sense::hover());
    let flash = since.map_or(0.0, |since| (1.0 - since / FLASH).clamp(0.0, 1.0) as f32);
    let center = rect.center();
    if flash > 0.0 {
        ui.painter().circle_filled(center, 4.0 + 2.5 * flash, color.gamma_multiply(0.12 + 0.35 * flash));
    }
    ui.painter().circle_filled(center, 3.0, color);
}

/// A port's letter in a small rounded plate, lit while the port is in use.
fn badge(ui: &mut egui::Ui, letter: &str, lit: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 16.0), egui::Sense::hover());
    let purple = theme::signal::MIDI;
    let (fill, ink) = if lit { (purple.gamma_multiply(0.28), purple.lerp_to_gamma(theme::text::PRIMARY, 0.6)) } else { (theme::background::WIDGET, theme::text::DISABLED) };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, letter, egui::FontId::monospace(11.0), ink);
}

/// A dot by a port's state that lights on each beat while clock goes out.
fn beat(ui: &mut egui::Ui, id: egui::Id, clocks: u32, now: f64) {
    let since = since(ui, id.with("clock"), clocks, now);
    if since > 0.5 {
        return;
    }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    let on = clocks % PPQN < PPQN / 4;
    let color = if on { theme::signal::MIDI.lerp_to_gamma(theme::text::PRIMARY, 0.3) } else { theme::background::GRID_MAJOR };
    ui.painter().circle_filled(rect.center(), if on { 2.8 } else { 2.0 }, color);
    response.on_hover_text("Clock is going out on this port");
}

/// The last message, as the MIDI Monitor writes it: in its colour while
/// it's fresh, settling grey over [`FRESH`].
fn last(ui: &mut egui::Ui, event: Option<MidiEvent>, since: f64) {
    let Some(event) = event else {
        ui.label(RichText::new("—").small().color(theme::text::DISABLED));
        return;
    };
    let (text, color) = format_midi_event(&event);
    let fresh = (1.0 - since / FRESH).clamp(0.0, 1.0) as f32;
    let ink = theme::text::DISABLED.lerp_to_gamma(color, 0.35 + 0.65 * fresh);
    ui.add(egui::Label::new(RichText::new(text).small().monospace().color(ink)).truncate());
}

/// How many modules there are, `one` or `many`, the names on hover.
fn who(ui: &mut egui::Ui, names: &[String], one: &str, many: &str, doing: &str) {
    if names.is_empty() {
        return;
    }
    let text = if names.len() == 1 { format!("1 {one}") } else { format!("{} {many}", names.len()) };
    let response = ui.label(RichText::new(text).small().color(theme::text::SECONDARY));
    response.on_hover_text(format!("{}, {doing}", names.join(", ")));
}

/// Seconds since `count` last moved, as the frames have seen it: none
/// passed while it hasn't moved since the panel first looked.
fn since(ui: &egui::Ui, id: egui::Id, count: u32, now: f64) -> f64 {
    let (seen, at) = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || (count, f64::NEG_INFINITY)));
    if seen != count {
        ui.ctx().data_mut(|d| d.insert_temp(id, (count, now)));
        return 0.0;
    }
    now - at
}

/// A small cross to click, `size` across: brighter under the pointer.
fn cross(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let ink = if response.hovered() { theme::text::PRIMARY } else { theme::text::QUIET };
    let arm = rect.shrink(size * 0.22);
    let stroke = egui::Stroke::new(1.5, ink);
    ui.painter().line_segment([arm.left_top(), arm.right_bottom()], stroke);
    ui.painter().line_segment([arm.right_top(), arm.left_bottom()], stroke);
    response
}

/// Looking again, and a word on whose settings these are.
fn footer(ui: &mut egui::Ui, edits: &mut Vec<Edit>) {
    ui.horizontal(|ui| {
        if ui.button("🔄 Refresh").on_hover_text("Look for MIDI devices again now").clicked() {
            edits.push(Edit::Refresh);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("This computer's, not the patch's · a renumbered device is found again").small().color(theme::text::QUIET));
        });
    });
}

/// In the browser the devices wait for Connect MIDI: true once they're there.
#[cfg(target_arch = "wasm32")]
fn web_access(ui: &mut egui::Ui) -> bool {
    use crate::engine::web_midi::{self, Access};
    match web_midi::access() {
        Access::Allowed => true,
        Access::Unsupported => {
            quiet_line(ui, "This browser has no Web MIDI");
            false
        }
        access => {
            ui.add_space(8.0);
            let button = ui.add_enabled(access != Access::Asking, egui::Button::new("🎹 Connect MIDI"));
            if button.clicked() {
                web_midi::request(ui.ctx());
            }
            false
        }
    }
}
