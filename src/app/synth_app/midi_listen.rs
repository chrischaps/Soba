//! Which MIDI input each module hears (#435), the app's side: the Device a
//! module is set to, by name, made into the slot the engine filters by, as
//! inputs open and close, a patch opens, or an edit or an undo changes it.
//! What a Device is is `crate::graph::midi_device`.
//!
//! The bus hears which MIDI Out ports play each input's device here too, so
//! a device the Clock follows isn't sent the clock back (#438).
//!
//! An input open for its thru routes alone (#463) is left out of "any
//! device" here: the engine's modules, the log, the ears and MIDI Learn are
//! all told which slots those are.

use super::*;
use crate::engine::device_names::in_and_out;
use crate::graph::midi_device::{device, listen_to, listens_to_devices};
use crate::link::midi::MIDI;
use crate::link::midi_thru::THRU;

impl SynthApp {
    /// Has every module set to a device hear its input, and the menus and
    /// titles know the inputs open. The engine's graph says what each
    /// module hears now, so a patch reopened, a node added back by undo or
    /// a device plugged in is told without anything kept here.
    pub(super) fn follow_midi_listening(&mut self) {
        let open = self.midi.open_names();
        if self.user_state.midi_inputs != open {
            self.user_state.midi_inputs = open;
        }
        let slots = self.midi.open_slots();
        let chosen = MIDI.all_chosen();
        for input in 0..crate::dsp::MIDI_INPUTS {
            let name = slots.iter().find(|(slot, _)| *slot as usize == input).map(|(_, name)| name.as_str());
            let ports = chosen
                .iter()
                .enumerate()
                .filter(|(_, output)| name.is_some_and(|name| in_and_out(name, output)))
                .fold(0u8, |ports, (port, _)| ports | 1 << port);
            MIDI.set_same_device(input, ports);
        }
        let thru_only = THRU.thru_only_slots(slots.iter().map(|(slot, name)| (*slot, name.as_str())));
        self.midi.thru_only = thru_only;
        self.user_state.midi_thru_only = thru_only;
        if self.user_state.midi_input_slots != slots {
            self.user_state.midi_input_slots = slots;
        }
        let Some(handle) = self.ui_handle.as_ref() else { return };
        let graph = handle.graph();
        let mut commands = Vec::new();
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if !listens_to_devices(node.user_data.module_id) {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            let listen = listen_to(device(&node.user_data), |name| self.midi.slot_of(name), thru_only);
            if graph.listen(engine_id).is_some_and(|now| now != listen) {
                commands.push(EngineCommand::Listen { node_id: engine_id, listen });
            }
        }
        for command in commands {
            self.send_command(command);
        }
    }
}
