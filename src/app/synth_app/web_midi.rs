//! Connect MIDI: how the browser build gets at MIDI devices.
//!
//! A page may use MIDI only once the visitor allows it, so until then the
//! toolbar's **MIDI In** menu is a **Connect MIDI** button, and so is a
//! MIDI Out's device menu. Once allowed, both list Web MIDI's devices and
//! follow them as they're plugged in and out (`engine::web_midi`).
//!
//! Compiled only for the web build.

use std::cell::RefCell;

use eframe::egui::{self, RichText};

use crate::engine::web_midi::{self, Access};
use super::super::theme;
use super::actions::AppAction;
use super::SynthApp;

/// What Connect MIDI says when the browser has no Web MIDI.
const UNSUPPORTED: &str = "This browser has no Web MIDI, so Soba can't reach MIDI devices here. Chrome, Edge and Firefox have it, and so does the desktop app";

thread_local! {
    /// The device changes and the access last followed.
    static SEEN: RefCell<(u32, Access)> = const { RefCell::new((0, Access::Unasked)) };
}

/// The Connect MIDI button, while the browser hasn't allowed MIDI yet.
/// Returns whether it was drawn: once allowed, the device menu takes its place.
pub fn connect_button(ui: &mut egui::Ui) -> bool {
    let access = web_midi::access();
    let (label, hover) = match &access {
        Access::Allowed => return false,
        Access::Asking => ("Asking…", "Your browser is asking whether Soba may use MIDI devices"),
        Access::Unsupported => ("No Web MIDI", UNSUPPORTED),
        Access::Unasked | Access::Refused(_) => (
            "🎹 Connect MIDI",
            "Play Soba from a MIDI keyboard or controller, and MIDI Out to synths and other apps. Your browser asks first",
        ),
    };
    if ui.add_enabled(access != Access::Asking, egui::Button::new(label)).on_hover_text(hover).clicked() {
        web_midi::request(ui.ctx());
    }
    true
}

impl SynthApp {
    /// The toolbar's MIDI In: Connect MIDI, then the inputs to choose from.
    /// No Refresh: the list follows the devices by itself.
    pub(super) fn web_midi_in(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        ui.label(RichText::new("MIDI In").color(theme::text::SECONDARY));
        ui.add_space(8.0);
        if connect_button(ui) {
            return;
        }
        let connected = self.midi.selected_device();
        let shown = connected.map_or("None", |d| d.name.as_str());
        // Keyed on the count: egui keeps a menu's size from when it first
        // opened, which would cut off a device plugged in since
        egui::ComboBox::from_id_salt(("midi_device_selector", self.midi.devices.len()))
            .selected_text(format!("{}{shown}", if connected.is_some() { "● " } else { "○ " }))
            .width(180.0)
            .truncate()
            .show_ui(ui, |ui| {
                if ui.selectable_label(connected.is_none(), "None (Disconnect)").clicked() {
                    actions.push(AppAction::DisconnectMidi);
                }
                ui.separator();
                if self.midi.devices.is_empty() {
                    ui.label(RichText::new("No MIDI inputs plugged in").color(theme::text::DISABLED).italics());
                }
                for device in &self.midi.devices {
                    if ui.selectable_label(self.midi.selected == Some(device.index), &device.name).clicked() {
                        actions.push(AppAction::ConnectMidi(device.index));
                    }
                }
            });
    }

    /// Lists the MIDI inputs again when Web MIDI says they changed, and
    /// says in the status bar when MIDI can't be had.
    pub(super) fn follow_web_midi(&mut self) {
        let (changes, access) = (web_midi::changes(), web_midi::access());
        let (seen_changes, seen_access) = SEEN.with_borrow(|seen| seen.clone());
        if changes != seen_changes {
            // The list is numbered afresh: MIDI In follows its device by
            // name, and lets go (and of its notes) if it was unplugged
            let selected = self.midi.selected_device().map(|d| d.name.clone());
            self.refresh_midi_devices();
            if let Some(name) = selected {
                match self.midi.devices.iter().position(|d| d.name == name) {
                    Some(index) => self.midi.selected = Some(index),
                    None => self.disconnect_midi_device(),
                }
            }
        }
        if access != seen_access {
            match &access {
                Access::Refused(why) => self.raise_notice(why.clone()),
                Access::Unsupported => self.raise_notice(UNSUPPORTED.to_string()),
                _ => {}
            }
        }
        SEEN.set((changes, access));
    }
}
