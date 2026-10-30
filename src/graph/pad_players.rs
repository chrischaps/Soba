//! What each pad of a controller plays in the patch, for its lights
//! (#430): see [`crate::app::pad_lights`].
//!
//! - A **Pad** module plays in the colour of the module its Gate is cabled
//!   into (through any groups), else its Velocity's, else Gate green. It
//!   flashes as its face does, and glows while its gate is open.
//! - A **mapping** learned from a pad plays in its module's colour, and
//!   glows while its note is held.
//! - A **Scenes** pad learned from a pad plays in the Scenes' colour, and
//!   glows while its scene is the current one.

use std::collections::HashMap;

use egui_node_graph2::{NodeId, OutputId};

use crate::app::pad_lights::{flash, Player};
use crate::app::theme;
use crate::modules::{pad, Pad};
use crate::persistence::{MidiMapping, MidiSource};

use super::scenes::{self, SLOTS};
use super::{display_frame, groups, midi_device, pads_face, SynthGraph, SynthGraphState};

/// Everything in `graph` that a pad plays: its Pads, and the mappings
/// learned from notes. `held` is the notes held on the controllers, as
/// (note, velocity).
pub fn players(graph: &SynthGraph, user_state: &SynthGraphState, mappings: &[MidiMapping], held: &[(u8, u8)]) -> Vec<Player> {
    let mut players = Vec::new();
    // Where each output's first cable goes, through any groups
    let mut into: HashMap<OutputId, NodeId> = HashMap::new();
    for (output, input) in groups::leaf_cables(graph) {
        if let Some(target) = graph.inputs.get(input) {
            into.entry(output).or_insert(target.node);
        }
    }
    let colour_of = |node: NodeId| graph.nodes.get(node).map(|n| n.user_data.category.color());

    for (node_id, node) in graph.nodes.iter() {
        if node.user_data.module_id != pad::ID {
            continue;
        }
        let listen = pads_face::listen_of(graph, node_id);
        let heard = midi_device::heard(graph, node_id);
        let readout = display_frame::readout(user_state, node_id);
        let value = |at: usize| readout.map_or(0.0, |r| r.values[at]);
        // Its Gate's target, else its Velocity's
        let colour = node.outputs.iter().take(2).find_map(|(_, output)| into.get(output).and_then(|&target| colour_of(target)));
        players.push(Player {
            device: heard.device.map(str::to_string),
            // A MIDI In's channel, else its own (0 is Omni)
            channel: heard.channel.or(listen.channel.checked_sub(1)),
            note: (!listen.any).then_some(listen.note),
            colour: colour.unwrap_or(theme::signal::GATE),
            open: value(Pad::READOUT_GATE) > 0.5,
            flash: flash(value(Pad::READOUT_VELOCITY), readout.map_or(f32::MAX, |r| r.values[Pad::READOUT_QUIET])),
        });
    }

    for mapping in mappings.iter().filter(|m| m.source == MidiSource::Note && m.macro_slot.is_none()) {
        let Some(node_id) = user_state.graph_node_id(mapping.node_id) else { continue };
        let Some(colour) = colour_of(node_id) else { continue };
        let open = match mapping.scene {
            Some(slot) => current_scene(graph, user_state, node_id) == Some(slot as usize),
            None => held.iter().any(|&(note, _)| note == mapping.cc_number),
        };
        players.push(Player {
            device: mapping.device.clone(),
            channel: mapping.channel.checked_sub(1),
            note: Some(mapping.cc_number),
            colour,
            open,
            flash: 0.0,
        });
    }
    players
}

/// The slot of the scene the Scenes `node_id`'s knobs are at, as its face
/// lights it.
fn current_scene(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId) -> Option<usize> {
    let bank = &graph.nodes.get(node_id)?.user_data.scenes;
    (0..SLOTS).find(|&slot| bank.get(scenes::letter(slot)).is_some_and(|scene| scenes::matches(graph, user_state, node_id, scene)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;
    use egui_node_graph2::GraphEditorState;

    use crate::app::editing;
    use crate::dsp::ModuleCategory;
    use crate::graph::{SynthGraphEditorState, SynthNodeTemplate};

    struct Rig {
        editor: SynthGraphEditorState,
        user_state: SynthGraphState,
    }

    impl Rig {
        fn new() -> Self {
            Self { editor: GraphEditorState::new(1.0), user_state: SynthGraphState::new() }
        }

        fn add(&mut self, module_id: &str) -> NodeId {
            let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
            editing::add_module(&mut self.editor, &mut self.user_state, template, pos2(0.0, 0.0)).0
        }

        fn connect(&mut self, from: NodeId, output: &str, to: NodeId, input: &str) {
            let graph = &mut self.editor.graph;
            let (output, input) = (graph[from].get_output(output).unwrap(), graph[to].get_input(input).unwrap());
            graph.add_connection(output, input, 0);
        }

        fn players(&self, mappings: &[MidiMapping], held: &[(u8, u8)]) -> Vec<Player> {
            players(&self.editor.graph, &self.user_state, mappings, held)
        }
    }

    #[test]
    fn a_pad_plays_in_the_colour_of_what_its_gate_strikes_or_gate_green() {
        let mut rig = Rig::new();
        let kick = rig.add(pad::ID);
        let loose = rig.add(pad::ID);
        let drum = rig.add(crate::modules::drum::ID);
        rig.connect(kick, "Gate", drum, "Trig");
        let players = rig.players(&[], &[]);
        let of = |node: NodeId| {
            let note = pads_face::listen_of(&rig.editor.graph, node).note;
            players.iter().find(|p| p.note == Some(note)).unwrap().clone()
        };
        let (kick, loose) = (of(kick), of(loose));
        assert_eq!(kick.colour, ModuleCategory::Source.color(), "a Drum's blue");
        assert_eq!((kick.channel, kick.note), (Some(9), Some(36)), "the GM kick on channel 10");
        assert_eq!(loose.colour, theme::signal::GATE, "nothing on it: Gate green");
        assert_eq!(loose.note, Some(37), "the next pad");
        assert!(!kick.open && kick.flash == 0.0, "never hit");
    }

    #[test]
    fn a_mapping_from_a_pad_plays_its_modules_colour_and_glows_while_held() {
        let mut rig = Rig::new();
        let filter = rig.add("filter.svf");
        let engine = rig.user_state.get_engine_node_id(filter).unwrap();
        let mut mapping = MidiMapping::new(40, 10, engine, 0, "Cutoff", 0.0, 1.0);
        mapping.source = MidiSource::Note;
        let cc = MidiMapping::new(74, 1, engine, 0, "Cutoff", 0.0, 1.0);
        let players = rig.players(&[mapping.clone(), cc], &[]);
        assert_eq!(players.len(), 1, "a CC's mapping lights no pad");
        assert_eq!(players[0].colour, ModuleCategory::Filter.color());
        assert_eq!((players[0].channel, players[0].note), (Some(9), Some(40)));
        assert!(!players[0].open);
        assert!(rig.players(&[mapping], &[(40, 100)])[0].open, "held down");
    }
}
