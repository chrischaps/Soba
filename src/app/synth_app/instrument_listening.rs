//! Listening for a profile (#441), the app's side: a small window from
//! the MIDI In menu. Choose a device that sends its knobs as CCs, turn
//! each knob on it, and name each row as it appears; Save writes a
//! profile of your own to `Documents/Soba/Instruments`. What's heard and
//! how it becomes a profile is [`crate::hardware::listening`].

use super::*;
use crate::hardware::listening::{HeardKnob, Listening};

/// How long a row's lamp glows after its knob moves, in seconds.
const FLASH: f64 = 0.3;
const WIDTH: f32 = 380.0;

/// What the window asked for.
enum Edit {
    Device(String),
    Forget(usize),
    Save,
    Close,
}

impl SynthApp {
    /// Starts listening to the first device open, or stops.
    pub(super) fn toggle_instrument_listening(&mut self) {
        self.midi.listening = match self.midi.listening {
            Some(_) => None,
            None => Some(Listening::new(self.midi.open_names().into_iter().next().unwrap_or_default())),
        };
    }

    /// The window, while Soba is listening.
    pub(super) fn show_instrument_listening(&mut self, ctx: &egui::Context) {
        let open = self.midi.open_names();
        let Some(listening) = self.midi.listening.as_mut() else { return };
        let now = ctx.input(|i| i.time);
        let mut edits = Vec::new();
        egui::Window::new("Listen to an Instrument")
            .id(egui::Id::unique("instrument_listening"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .default_pos(self.editor_rect.center_top() + egui::vec2(-WIDTH / 2.0, 60.0))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                heading(ui, &mut edits);
                ui.add_space(6.0);
                device_row(ui, &listening.device, &open, &mut edits);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Call it").color(theme::text::SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut listening.name).desired_width(WIDTH - 60.0).hint_text("MicroFreak"));
                });
                ui.add_space(8.0);
                if listening.knobs.is_empty() {
                    quiet(ui, "Turn a knob on the device: it shows up here, ready to name. Then the next, in the order you'd want them on the node");
                } else {
                    egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                        for (index, knob) in listening.knobs.iter_mut().enumerate() {
                            knob_row(ui, index, knob, now, &mut edits);
                        }
                    });
                    match listening.off_channel() {
                        0 => {}
                        n => quiet(ui, &format!("{n} heard on another channel than the rest: each keeps its own")),
                    }
                }
                ui.add_space(6.0);
                footer(ui, listening.not_ready(), &mut edits);
            });
        // A knob turning on the device wakes no frame of its own
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        for edit in edits {
            match edit {
                Edit::Device(device) => self.midi.listening = Some(Listening::new(device)),
                Edit::Forget(index) => {
                    if let Some(listening) = self.midi.listening.as_mut() {
                        listening.forget(index);
                    }
                }
                Edit::Save => self.save_instrument_profile(),
                Edit::Close => self.midi.listening = None,
            }
        }
    }

    /// Saves the profile heard to the Instruments folder, and stops.
    fn save_instrument_profile(&mut self) {
        let Some(listening) = self.midi.listening.as_ref() else { return };
        let profile = listening.profile();
        self.status_message = Some(match crate::hardware::save_in(&crate::hardware::folder(), &profile) {
            Ok(path) => {
                self.midi.listening = None;
                // The Instruments' menus offer it at once
                crate::hardware::reload();
                format!("Saved {} with {} knobs to {}", profile.name, profile.parameters.len(), path.display())
            }
            Err(e) => format!("Couldn't save the profile: {e}"),
        });
    }
}

/// The window's name after a short purple bar, and the way to close it.
fn heading(ui: &mut egui::Ui, edits: &mut Vec<Edit>) {
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 15.0), egui::Sense::hover());
        ui.painter().rect_filled(bar, 1.5, theme::signal::MIDI);
        let family = egui::FontFamily::Name(theme::TITLE_FAMILY.into());
        ui.label(RichText::new("Listen to an Instrument").font(egui::FontId::new(16.0, family)).color(theme::text::PRIMARY));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if cross(ui, 14.0).on_hover_text("Stop listening, saving nothing").clicked() {
                edits.push(Edit::Close);
            }
        });
    });
}

/// The device listened to, from those open.
fn device_row(ui: &mut egui::Ui, device: &str, open: &[String], edits: &mut Vec<Edit>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Device").color(theme::text::SECONDARY));
        if open.is_empty() {
            ui.label(RichText::new("Open a MIDI input first, from the MIDI In menu").color(theme::accent::WARNING));
            return;
        }
        let shown = if device.is_empty() { "Choose one" } else { device };
        egui::ComboBox::from_id_salt("instrument_listening_device").width(WIDTH - 60.0).selected_text(shown).show_ui(ui, |ui| {
            for name in open {
                if ui.selectable_label(name == device, name).clicked() && name != device {
                    edits.push(Edit::Device(name.clone()));
                }
            }
        });
    });
}

/// A knob heard: a lamp that glows as it moves, its CC and where it is,
/// its name to type, and a cross to forget it.
fn knob_row(ui: &mut egui::Ui, index: usize, knob: &mut HeardKnob, now: f64, edits: &mut Vec<Edit>) {
    let id = egui::Id::unique(("instrument_knob", knob.cc, knob.channel));
    let (seen, at) = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || (knob.moves, now)));
    let since = if seen == knob.moves { now - at } else {
        ui.ctx().data_mut(|d| d.insert_temp(id, (knob.moves, now)));
        0.0
    };
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 18.0), egui::Sense::hover());
        let glow = (1.0 - since / FLASH).clamp(0.0, 1.0) as f32;
        let color = theme::signal::MIDI;
        if glow > 0.0 {
            ui.painter().circle_filled(rect.center(), 4.0 + 2.5 * glow, color.gamma_multiply(0.15 + 0.35 * glow));
        }
        ui.painter().circle_filled(rect.center(), 3.0, color.gamma_multiply(0.5 + 0.5 * glow));
        let label = format!("CC {:>3} · ch {:<2}", knob.cc, knob.channel);
        ui.label(RichText::new(label).small().monospace().color(theme::text::SECONDARY));
        // Where the knob stands, as a thin bar
        let (bar, _) = ui.allocate_exact_size(egui::vec2(36.0, 4.0), egui::Sense::hover());
        ui.painter().rect_filled(bar, 2.0, theme::background::GRID_MAJOR);
        let filled = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * knob.value as f32 / 127.0, bar.height()));
        ui.painter().rect_filled(filled, 2.0, color.lerp_to_gamma(theme::text::PRIMARY, 0.3));
        let name = ui.add(egui::TextEdit::singleline(&mut knob.name).desired_width(150.0).hint_text(format!("CC {}", knob.cc)));
        // The newest row takes the typing, so turn, type, turn, type
        if knob.moves == 1 && since == 0.0 {
            name.request_focus();
        }
        if cross(ui, 12.0).on_hover_text("Forget this one: turned by mistake").clicked() {
            edits.push(Edit::Forget(index));
        }
    });
}

/// Save, or why not yet, and where it goes.
fn footer(ui: &mut egui::Ui, not_ready: Option<String>, edits: &mut Vec<Edit>) {
    ui.horizontal(|ui| {
        let save = ui.add_enabled(not_ready.is_none(), egui::Button::new("Save Profile"));
        let save = match &not_ready {
            Some(why) => save.on_disabled_hover_text(why),
            None => save.on_hover_text("Write it to Documents/Soba/Instruments, where Instruments find it"),
        };
        if save.clicked() {
            edits.push(Edit::Save);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Unnamed knobs keep their CC").small().color(theme::text::QUIET));
        });
    });
}

/// A line of quiet, wrapping text.
fn quiet(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(theme::text::QUIET)).wrap());
}

/// A small cross to click, `size` across: brighter under the pointer, as
/// the Rig's (the fonts have no ✕).
fn cross(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let ink = if response.hovered() { theme::text::PRIMARY } else { theme::text::QUIET };
    let arm = rect.shrink(size * 0.22);
    let stroke = egui::Stroke::new(1.5, ink);
    ui.painter().line_segment([arm.left_top(), arm.right_bottom()], stroke);
    ui.painter().line_segment([arm.right_top(), arm.left_bottom()], stroke);
    response
}
