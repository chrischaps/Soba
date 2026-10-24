//! A learn into a module, from the app's side (#427): what the session
//! heard, written into the module's own parameters (see
//! [`crate::app::module_learn`]).

use crate::engine::EngineCommand;
use super::SynthApp;

impl SynthApp {
    /// Writes a parameter a learn into a module heard: sent at once, so the
    /// module hears its new control from the next block, and shown on the
    /// node. Unlike a turn from MIDI it's an edit, so undo takes it back.
    pub(super) fn learn_into_module(&mut self, node_id: u64, param_index: usize, value: f32) {
        let Some(input) = self.user_state.graph_node_id(node_id).and_then(|node| self.param_input(node, param_index)) else {
            return;
        };
        self.send_command(EngineCommand::SetParameter { node_id, param_index, value });
        self.cached_params.insert((node_id, param_index), value);
        if let Some(input) = self.graph_state.graph.inputs.get_mut(input) {
            input.value.set_actual_value(value);
        }
        self.history.name_next("Learn MIDI");
    }
}
