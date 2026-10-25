//! Instruments' knobs into the engine (#442): what each knob sends, from
//! the parameter it holds, handed over wherever it isn't what the engine
//! has. That covers a patch opened, a profile chosen, a knob given another
//! parameter, an undo and a paste alike, and each sends the knob afresh.
//! Send all from the face hands every knob over again. A Kit's knobs go
//! the same way, and its lanes (#443) with them, and a Setlist's rows
//! (#445).

use egui_node_graph2::NodeId;

use crate::engine::EngineCommand;
use crate::hardware::{instrument, setlist};
use crate::modules::instrument::SLOTS;

use super::SynthApp;

impl SynthApp {
    /// Hands the engine every Instrument's knobs where they've changed.
    /// Call once a frame, after the parameters are synced.
    pub(super) fn sync_instruments(&mut self) {
        let mut sends = self.instrument_sends(None);
        sends.extend(self.setlist_sends());
        for command in sends {
            self.send_command(command);
        }
    }

    /// Every Setlist's rows that differ from what the engine has: what
    /// each sends, from the profile its device names.
    fn setlist_sends(&self) -> Vec<EngineCommand> {
        let Some(handle) = self.ui_handle.as_ref() else { return Vec::new() };
        let mut sends = Vec::new();
        let mut profiles = None;
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if node.user_data.module_id != crate::modules::setlist::ID {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            if !handle.graph().contains_module(engine_id) {
                continue;
            }
            let profiles = profiles.get_or_insert_with(crate::hardware::known);
            let maps = setlist::maps(&node.user_data.labels, profiles);
            let had = handle.graph().row_maps(engine_id);
            for (section, rows) in maps.iter().enumerate() {
                for (row, &map) in rows.iter().enumerate() {
                    if had.map_or(map.is_some(), |had| had[section][row] != map) {
                        sends.push(EngineCommand::MapRow { node_id: engine_id, section, row, map });
                    }
                }
            }
        }
        sends
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
    /// or all of `again`'s. A Kit's lanes, where they differ.
    fn instrument_sends(&self, again: Option<NodeId>) -> Vec<EngineCommand> {
        let Some(handle) = self.ui_handle.as_ref() else { return Vec::new() };
        let profiles = crate::hardware::known();
        let mut sends = Vec::new();
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if !instrument::plays_a_profile(node.user_data.module_id) || again.is_some_and(|again| again != node_id) {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            if !handle.graph().contains_module(engine_id) {
                continue;
            }
            // The lanes first: a knob on a track goes to the track's channel
            if node.user_data.module_id == crate::modules::kit::ID {
                let had = handle.graph().lane_maps(engine_id);
                for (lane, map) in instrument::lane_maps(&node.user_data.labels, &profiles).into_iter().enumerate() {
                    let fresh = match had {
                        Some(had) => had[lane] != map,
                        None => !map.is_empty(),
                    };
                    if fresh {
                        sends.push(EngineCommand::MapLane { node_id: engine_id, lane, map });
                    }
                }
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
