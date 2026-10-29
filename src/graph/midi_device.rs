//! The Device a module listens to (#435). Note In, Pads, Controls, Clock
//! and the MIDI Monitor can hear one of the MIDI inputs open, or any.
//!
//! It's kept in the module's labels by the device's name, so a patch carries
//! it from one machine to the next, and undo, copy and paste carry it as they
//! carry an Arranger's section names. Until a device of that name is open,
//! the module hears no device's MIDI, though it still hears Soba's own keys.
//! The engine's side is [`ListenTo`].
//!
//! A purple MIDI cable from a MIDI In sets it instead (#479): the module
//! hears that MIDI In's Device and Channel, whatever its own Device says,
//! through any groups between them. [`heard`] says which, and the Device
//! menu greys while a cable sets it, as a knob does under a cable.

use eframe::egui;
use egui_node_graph2::NodeId;

use crate::dsp::ListenTo;
use crate::engine::device_names::same;
use crate::modules::midi_in;

use super::groups::{leaf_source, GroupIndex};
use super::{SynthGraph, SynthNodeData, SynthResponse};

/// The key a module's Device is kept under in its labels.
pub const DEVICE_LABEL: &str = "Device";

/// Does the module with this ID hear MIDI from the devices, and so have a
/// Device to choose? The MIDI In has one too, for the modules it's patched
/// into to hear.
pub fn listens_to_devices(module_id: &str) -> bool {
    use crate::modules::{clock, controls, midi_monitor, note_in, pads};
    [note_in::ID, pads::ID, controls::ID, clock::ID, midi_monitor::ID, midi_in::ID].contains(&module_id)
}

/// The device a module listens to, by name, or `None` for any.
pub fn device(data: &SynthNodeData) -> Option<&str> {
    data.labels.get(DEVICE_LABEL).map(String::as_str)
}

/// What a module hears (#479): the Device and Channel of the MIDI In its
/// MIDI input is patched from, or its own Device on every channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Heard<'a> {
    /// The device, by name, or `None` for any.
    pub device: Option<&'a str>,
    /// The one MIDI channel (0-15), or `None` for every one.
    pub channel: Option<u8>,
    /// The MIDI In that says so, if a cable brings one.
    pub from: Option<NodeId>,
}

/// What the module `node_id` hears: from the MIDI In its MIDI cable comes
/// from, through any groups' jacks between, or else its own Device.
pub fn heard(graph: &SynthGraph, node_id: NodeId) -> Heard<'_> {
    let Some(node) = graph.nodes.get(node_id) else { return Heard::default() };
    let own = Heard { device: device(&node.user_data), ..Heard::default() };
    let Some(from) = midi_in_of(graph, node_id) else { return own };
    let channel = super::display_frame::value_of(graph, from, "Channel").and_then(midi_in::channel_of);
    Heard { device: device(&graph[from].user_data), channel, from: Some(from) }
}

/// The MIDI In the module's MIDI input is patched from, following the cable
/// through any groups, or `None` if it isn't (or a group's jack it comes
/// through has nothing plugged in).
pub fn midi_in_of(graph: &SynthGraph, node_id: NodeId) -> Option<NodeId> {
    let input = graph.nodes.get(node_id)?.get_input(midi_in::PORT_NAME).ok()?;
    let output = graph.connections.get(input)?.first().copied()?;
    // Groups are looked at only for a module that has a cable in
    let source = leaf_source(graph, &GroupIndex::of(graph), output)?;
    let from = graph.outputs.get(source)?.node;
    (graph.nodes.get(from)?.user_data.module_id == midi_in::ID).then_some(from)
}

/// What a module set to `device` hears, given the slot of each input open
/// and those open for their thru routes alone (`thru_only`, a bit a slot):
/// any device leaves them out (#463), but naming one hears it.
pub fn listen_to(device: Option<&str>, slot_of: impl Fn(&str) -> Option<u8>, thru_only: u8) -> ListenTo {
    match device {
        None if thru_only != 0 => ListenTo::AllBut(thru_only),
        None => ListenTo::Any,
        Some(name) => slot_of(name).map_or(ListenTo::Unplugged, ListenTo::Port),
    }
}

/// What follows the module's title: the device it hears, behind the
/// toolbar's dot for whether it's open (● or ○), and the channel a MIDI In
/// sets. Nothing for a module that hears any device on every channel.
pub fn title_note(heard: Heard, open: &[String]) -> Option<String> {
    let channel = heard.channel.map(|channel| format!(" · ch {}", channel + 1)).unwrap_or_default();
    match heard.device {
        Some(device) => {
            let dot = if open.iter().any(|name| same(name, device)) { '●' } else { '○' };
            Some(format!("{dot} {device}{channel}"))
        }
        None if heard.channel.is_some() => Some(format!("any device{channel}")),
        None => None,
    }
}

/// The Device submenu of the module's right-click menu: Any, each input
/// open, and the one it's set to if that isn't open. Choosing one is an undo
/// step, like naming a section. Greyed while a MIDI cable sets the device
/// (`routed`), as a knob is under a cable.
pub fn device_menu(ui: &mut egui::Ui, node_id: egui_node_graph2::NodeId, data: &SynthNodeData, open: &[String], routed: bool) -> Option<SynthResponse> {
    if routed {
        ui.add_enabled_ui(false, |ui| crate::widgets::menu::menu_button(ui, "Device", |_| {}))
            .response
            .on_disabled_hover_text("Set by the MIDI In its MIDI cable comes from: unplug the cable to choose here");
        return None;
    }
    let mut chosen = None;
    crate::widgets::menu::menu_button(ui, "Device", |ui| chosen = device_choices(ui, node_id, device(data), open))
        .response
        .on_hover_text("Which MIDI device it hears: Any, or one of those open under MIDI In");
    chosen
}

/// The devices to choose from, in a menu or a popup: Any, each input open,
/// and `current` if that isn't open. The choice, as an undo step.
pub fn device_choices(ui: &mut egui::Ui, node_id: NodeId, current: Option<&str>, open: &[String]) -> Option<SynthResponse> {
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
    chosen
}

/// Whether `param` is the MIDI input of a module that hears MIDI. It has
/// no row: its jack sits on the left end of the title bar (see
/// [`place_midi_jack`]), so a module grows no taller for it and a MIDI In's
/// cable comes in at its head.
pub fn is_midi_jack(node_id: NodeId, graph: &SynthGraph, param: egui_node_graph2::AnyParameterId) -> bool {
    let egui_node_graph2::AnyParameterId::Input(input) = param else { return false };
    graph
        .nodes
        .get(node_id)
        .is_some_and(|node| node.user_data.is_module() && node.inputs.iter().any(|(name, id)| *id == input && name == midi_in::PORT_NAME))
}

/// Puts the module's MIDI jack, if it has one, at height `y` on its left
/// edge: the title bar's middle. Called each frame its title bar is drawn.
pub fn place_midi_jack(ui: &egui::Ui, node_id: NodeId, graph: &SynthGraph, y: f32) {
    if let Some(input) = graph.nodes.get(node_id).and_then(|node| node.get_input(midi_in::PORT_NAME).ok()) {
        egui_node_graph2::place_port(ui, node_id, egui_node_graph2::AnyParameterId::Input(input), y);
    }
}

/// The modules patched from the MIDI In `from`, through any groups (but not
/// the groups between).
pub fn hearing(graph: &SynthGraph, from: NodeId) -> Vec<NodeId> {
    graph
        .nodes
        .iter()
        .filter(|(node_id, node)| node.user_data.is_module() && midi_in_of(graph, *node_id) == Some(from))
        .map(|(node_id, _)| node_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_midi_modules_have_a_device_and_the_rest_dont() {
        for id in ["input.poly_midi", "input.pads", "input.controls", "util.clock", "util.midi_monitor", "input.midi"] {
            assert!(listens_to_devices(id), "{id}");
        }
        assert!(!listens_to_devices("output.midi"), "MIDI Out sends; its ports are its devices");
        assert!(!listens_to_devices("osc.sine"));
    }

    #[test]
    fn a_device_is_heard_while_open_and_nothing_is_while_it_isnt() {
        let slots = |name: &str| (name == "Fantom-06").then_some(3);
        assert_eq!(listen_to(None, slots, 0), ListenTo::Any);
        assert_eq!(listen_to(Some("Fantom-06"), slots, 0), ListenTo::Port(3));
        assert_eq!(listen_to(Some("MiniLab3 MIDI"), slots, 0), ListenTo::Unplugged);
    }

    #[test]
    fn any_device_leaves_out_a_thru_only_input_but_naming_it_hears_it() {
        let slots = |name: &str| (name == "Fantom-06").then_some(3);
        assert_eq!(listen_to(None, slots, 1 << 3), ListenTo::AllBut(1 << 3));
        assert_eq!(listen_to(Some("Fantom-06"), slots, 1 << 3), ListenTo::Port(3));
    }
}

/// MIDI cables (#479): what a module hears from the MIDI In it's patched from.
#[cfg(test)]
mod cable_tests {
    use super::*;
    use egui::{pos2, vec2, Rect};
    use egui_node_graph2::NodeTemplateTrait;

    use crate::graph::groups::{group, GroupId};
    use crate::graph::{create_editor_state, SynthGraphEditorState, SynthGraphState, SynthNodeTemplate, SynthValueType};
    use crate::modules::{note_in, pads};

    fn add(editor: &mut SynthGraphEditorState, module_id: &str, x: f32) -> NodeId {
        let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
        let mut user_state = SynthGraphState::new();
        let node_id = editor.graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        );
        editor.node_positions.insert(node_id, pos2(x, 0.0));
        editor.node_order.push(node_id);
        node_id
    }

    fn patch(editor: &mut SynthGraphEditorState, from: NodeId, to: NodeId) {
        let graph = &mut editor.graph;
        let (output, input) = (graph[from].get_output(midi_in::PORT_NAME).unwrap(), graph[to].get_input(midi_in::PORT_NAME).unwrap());
        graph.add_connection(output, input, 0);
    }

    fn set_device(editor: &mut SynthGraphEditorState, node_id: NodeId, device: &str) {
        editor.graph[node_id].user_data.labels.insert(DEVICE_LABEL.to_string(), device.to_string());
    }

    fn set_channel(editor: &mut SynthGraphEditorState, node_id: NodeId, channel: usize) {
        let input = editor.graph[node_id].get_input("Channel").unwrap();
        editor.graph.inputs[input].value.set_actual_value(channel as f32);
        assert!(matches!(editor.graph.inputs[input].value, SynthValueType::Select { value, .. } if value == channel));
    }

    fn wrap(editor: &mut SynthGraphEditorState, nodes: &[NodeId], id: u64) -> NodeId {
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 300.0));
        group(editor, nodes, GroupId(id), "Pads", bounds).expect("grouped").node
    }

    #[test]
    fn a_note_in_patched_from_a_midi_in_hears_its_device_and_channel() {
        let mut editor = create_editor_state();
        let midi_in = add(&mut editor, midi_in::ID, 0.0);
        let keys = add(&mut editor, note_in::ID, 200.0);
        set_device(&mut editor, keys, "Fantom-06");
        assert_eq!(heard(&editor.graph, keys), Heard { device: Some("Fantom-06"), channel: None, from: None }, "its own, unpatched");

        // Device B, channel 2, whatever its own Device says
        set_device(&mut editor, midi_in, "MiniLab3 MIDI");
        set_channel(&mut editor, midi_in, 2);
        patch(&mut editor, midi_in, keys);
        assert_eq!(heard(&editor.graph, keys), Heard { device: Some("MiniLab3 MIDI"), channel: Some(1), from: Some(midi_in) });
        assert_eq!(hearing(&editor.graph, midi_in), [keys]);
        assert_eq!(title_note(heard(&editor.graph, keys), &["MiniLab3 MIDI".to_string()]).as_deref(), Some("● MiniLab3 MIDI · ch 2"));

        // A MIDI In on any device and Omni hears every device, every channel
        editor.graph[midi_in].user_data.labels.clear();
        set_channel(&mut editor, midi_in, 0);
        assert_eq!(heard(&editor.graph, keys), Heard { device: None, channel: None, from: Some(midi_in) });

        // Unplugged, it hears its own again
        let input = editor.graph[keys].get_input(midi_in::PORT_NAME).unwrap();
        editor.graph.remove_connection(input, editor.graph[midi_in].get_output(midi_in::PORT_NAME).unwrap());
        assert_eq!(heard(&editor.graph, keys).device, Some("Fantom-06"));
        assert_eq!(heard(&editor.graph, keys).from, None);
        assert!(hearing(&editor.graph, midi_in).is_empty());
    }

    #[test]
    fn the_device_follows_the_cable_through_a_group_and_a_group_inside_it() {
        let mut editor = create_editor_state();
        let midi_in = add(&mut editor, midi_in::ID, 0.0);
        let pads_a = add(&mut editor, pads::ID, 300.0);
        let pads_b = add(&mut editor, pads::ID, 300.0);
        set_device(&mut editor, midi_in, "MPD218");
        set_channel(&mut editor, midi_in, 10);
        patch(&mut editor, midi_in, pads_a);
        patch(&mut editor, midi_in, pads_b);

        // One group of the two Pads: one purple jack between them and the MIDI In
        let inner = wrap(&mut editor, &[pads_a, pads_b], 1000);
        let jacks = super::super::groups::input_jacks(&editor.graph, inner);
        assert_eq!(jacks.len(), 1, "one MIDI jack for both");
        assert_eq!(jacks[0].signal, crate::dsp::SignalType::Midi);
        for pads in [pads_a, pads_b] {
            assert_eq!(heard(&editor.graph, pads), Heard { device: Some("MPD218"), channel: Some(9), from: Some(midi_in) });
        }

        // And that group inside another
        wrap(&mut editor, &[inner], 1001);
        assert_eq!(heard(&editor.graph, pads_b).device, Some("MPD218"));
        assert_eq!(hearing(&editor.graph, midi_in).len(), 2);
    }

    #[test]
    fn a_routed_pads_claims_its_midi_in_s_device_on_its_channel() {
        let mut editor = create_editor_state();
        let midi_in = add(&mut editor, midi_in::ID, 0.0);
        let pads = add(&mut editor, pads::ID, 300.0);
        set_device(&mut editor, pads, "Fantom-06");
        set_device(&mut editor, midi_in, "MPD218");
        let slot_of = |name: &str| match name {
            "Fantom-06" => Some(0),
            "MPD218" => Some(1),
            _ => None,
        };
        // On its own Device, the Fantom's note 36 on channel 10 is the pads'
        let claims = super::super::pads_claims(&editor.graph, slot_of, 0);
        assert!(claims.claims(0, 9, 36) && !claims.claims(1, 9, 36));

        // Patched from the MIDI In, the MPD's is, and the Fantom's plays on
        patch(&mut editor, midi_in, pads);
        let claims = super::super::pads_claims(&editor.graph, slot_of, 0);
        assert!(claims.claims(1, 9, 36) && !claims.claims(0, 9, 36));

        // On channel 2, the pads hear nothing on 10, and claim nothing there
        set_channel(&mut editor, midi_in, 2);
        let claims = super::super::pads_claims(&editor.graph, slot_of, 0);
        assert!(!claims.claims(1, 9, 36));
    }
}
