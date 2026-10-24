//! Which MIDI input each module hears (#435), the app's side: the Device a
//! module is set to, by name, made into the slot the engine filters by, as
//! inputs open and close, a patch opens, or an edit or an undo changes it.
//! What a Device is is `crate::graph::midi_device`.

use super::*;
use crate::graph::midi_device::{device, listen_to, listens_to_devices};

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
        let Some(handle) = self.ui_handle.as_ref() else { return };
        let graph = handle.graph();
        let mut commands = Vec::new();
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if !listens_to_devices(node.user_data.module_id) {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            let listen = listen_to(device(&node.user_data), |name| self.midi.slot_of(name));
            if graph.listen(engine_id).is_some_and(|now| now != listen) {
                commands.push(EngineCommand::Listen { node_id: engine_id, listen });
            }
        }
        for command in commands {
            self.send_command(command);
        }
    }
}
