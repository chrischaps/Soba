//! The Device a module listens to (#435). Note In, Pads, Controls, Clock
//! and the MIDI Monitor can hear one of the MIDI inputs open, or any.
//!
//! It's kept in the module's labels by the device's name, so a patch carries
//! it from one machine to the next, and undo, copy and paste carry it as they
//! carry an Arranger's section names. Until a device of that name is open,
//! the module hears no device's MIDI, though it still hears Soba's own keys.
//! The engine's side is [`ListenTo`].

use eframe::egui;

use crate::dsp::ListenTo;
use crate::engine::device_names::same;

use super::{SynthNodeData, SynthResponse};

/// The key a module's Device is kept under in its labels.
pub const DEVICE_LABEL: &str = "Device";

/// Does the module with this ID hear MIDI from the devices, and so have a
/// Device to choose?
pub fn listens_to_devices(module_id: &str) -> bool {
    use crate::modules::{clock, controls, midi_monitor, note_in, pads};
    [note_in::ID, pads::ID, controls::ID, clock::ID, midi_monitor::ID].contains(&module_id)
}

/// The device a module listens to, by name, or `None` for any.
pub fn device(data: &SynthNodeData) -> Option<&str> {
    data.labels.get(DEVICE_LABEL).map(String::as_str)
}

/// What a module set to `device` hears, given the slot of each input open.
pub fn listen_to(device: Option<&str>, slot_of: impl Fn(&str) -> Option<u8>) -> ListenTo {
    match device {
        None => ListenTo::Any,
        Some(name) => slot_of(name).map_or(ListenTo::Unplugged, ListenTo::Port),
    }
}

/// What follows the module's title: the device it listens to, behind the
/// toolbar's dot for whether it's open (● or ○). Nothing for a module
/// that hears any.
pub fn title_note(data: &SynthNodeData, open: &[String]) -> Option<String> {
    let device = device(data)?;
    let dot = if open.iter().any(|name| same(name, device)) { '●' } else { '○' };
    Some(format!("{dot} {device}"))
}

/// The Device submenu of the module's right-click menu: Any, each input
/// open, and the one it's set to if that isn't open. Choosing one is an undo
/// step, like naming a section.
pub fn device_menu(ui: &mut egui::Ui, node_id: egui_node_graph2::NodeId, data: &SynthNodeData, open: &[String]) -> Option<SynthResponse> {
    let current = device(data);
    let mut chosen = None;
    let mut choose = |ui: &mut egui::Ui, selected: bool, label: &str, device: Option<&str>| {
        if ui.selectable_label(selected, label).clicked() {
            chosen = Some(SynthResponse::SetLabels {
                node_id,
                label: "Device".to_string(),
                labels: vec![(DEVICE_LABEL.to_string(), device.unwrap_or_default().to_string())],
            });
            ui.close();
        }
    };
    crate::widgets::menu::menu_button(ui, "Device", |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        choose(ui, current.is_none(), "Any", None);
        if !open.is_empty() || current.is_some() {
            ui.separator();
        }
        for name in open {
            choose(ui, current.is_some_and(|current| same(current, name)), name, Some(name));
        }
        // Set on another machine, or unplugged: kept until another is chosen
        if let Some(name) = current.filter(|name| !open.iter().any(|n| same(n, name))) {
            choose(ui, true, &format!("{name} (not open)"), Some(name));
        }
    })
    .response
    .on_hover_text("Which MIDI device it hears: Any, or one of those open under MIDI In");
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_midi_modules_have_a_device_and_the_rest_dont() {
        for id in ["input.poly_midi", "input.pads", "input.controls", "util.clock", "util.midi_monitor"] {
            assert!(listens_to_devices(id), "{id}");
        }
        assert!(!listens_to_devices("output.midi"), "MIDI Out sends; its ports are its devices");
        assert!(!listens_to_devices("osc.sine"));
    }

    #[test]
    fn a_device_is_heard_while_open_and_nothing_is_while_it_isnt() {
        let slots = |name: &str| (name == "Fantom-06").then_some(3);
        assert_eq!(listen_to(None, slots), ListenTo::Any);
        assert_eq!(listen_to(Some("Fantom-06"), slots), ListenTo::Port(3));
        assert_eq!(listen_to(Some("MiniLab3 MIDI"), slots), ListenTo::Unplugged);
    }
}
