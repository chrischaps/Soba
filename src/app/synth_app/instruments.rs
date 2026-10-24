//! Instruments' knobs into the engine (#442): what each knob sends, from
//! the parameter it holds, handed over wherever it isn't what the engine
//! has. That covers a patch opened, a profile chosen, a knob given another
//! parameter, an undo and a paste alike, and each sends the knob afresh.
//! Send all from the face hands every knob over again.

use egui_node_graph2::NodeId;

use crate::engine::EngineCommand;
use crate::hardware::instrument;
use crate::modules::instrument::{ID, SLOTS};

use super::SynthApp;

impl SynthApp {
    /// Hands the engine every Instrument's knobs where they've changed.
    /// Call once a frame, after the parameters are synced.
    pub(super) fn sync_instruments(&mut self) {
        let sends = self.instrument_sends(None);
        for command in sends {
            self.send_command(command);
        }
    }

    /// Hands the engine every knob of the Instrument `node_id` again, so
    /// each sends its value: the hardware comes back to what the patch says.
    pub(super) fn send_all(&mut self, node_id: NodeId) {
        let sends = self.instrument_sends(Some(node_id));
        for command in sends {
            self.send_command(command);
        }
    }

    /// The knobs to hand over: those that differ from what the engine has,
    /// or all of `again`'s.
    fn instrument_sends(&self, again: Option<NodeId>) -> Vec<EngineCommand> {
        let Some(handle) = self.ui_handle.as_ref() else { return Vec::new() };
        let profiles = crate::hardware::known();
        let mut sends = Vec::new();
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if node.user_data.module_id != ID || again.is_some_and(|again| again != node_id) {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            if !handle.graph().contains_module(engine_id) {
                continue;
            }
            let had = handle.graph().slot_maps(engine_id);
            let maps = instrument::maps(&node.user_data.labels, &profiles);
            for (slot, map) in maps.into_iter().enumerate().take(SLOTS) {
                let fresh = match had {
                    Some(had) => had[slot] != map,
                    // A new Instrument holds nothing yet
                    None => !map.is_empty(),
                };
                if fresh || again.is_some() {
                    sends.push(EngineCommand::MapSlot { node_id: engine_id, slot, map });
                }
            }
        }
        sends
    }
}
