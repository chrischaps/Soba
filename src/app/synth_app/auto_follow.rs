//! An Auto Clock's Tempo knob follows the device that leads it (#438).
//!
//! While a drum machine leads, the Clock plays at its tempo, and its knob
//! reads that tempo, locked (see `clock_display::knob_takeover`). When the
//! machine stops, the Clock takes the lead back on the audio thread at once,
//! carrying on at the machine's tempo, and counts the handover in its
//! readout. Here the app moves the Tempo knob to that tempo, so the knob
//! says what Soba plays and turning it carries on from there.
//!
//! The handover isn't an edit of the patch, so it isn't an undo step, just
//! as a Link peer's tempo isn't (`link_follow`).

use crate::app::undo::History;
use crate::dsp::Readout;
use crate::graph::port_mapping::parameter_inputs;
use crate::graph::{follows_auto, SynthGraph, SynthGraphState};
use crate::modules::clock::{self, Clock};

/// Moves an Auto Clock's Tempo knob to the tempo it carried on at, if a
/// device handed the lead back since the Clock's `previous` readout.
pub(super) fn follow_handover(
    graph: &mut SynthGraph,
    history: &mut History,
    user_state: &SynthGraphState,
    engine_node_id: u64,
    previous: Option<Readout>,
) {
    let Some(readout) = user_state.readouts.get(&engine_node_id) else { return };
    let handed_back = previous.map_or(0.0, |previous| previous.values[Clock::READOUT_HANDOVERS]) != readout.values[Clock::READOUT_HANDOVERS];
    let tempo = readout.values[Clock::READOUT_CARRIED];
    if !handed_back || tempo <= 0.0 {
        return;
    }
    let Some(node_id) = user_state.node_id_map.iter().find(|(_, &id)| id == engine_node_id).map(|(&node_id, _)| node_id) else {
        return;
    };
    let is_auto_clock = graph.nodes.get(node_id).is_some_and(|node| node.user_data.module_id == clock::ID) && follows_auto(graph, node_id);
    if !is_auto_clock {
        return;
    }
    let Some(&input) = parameter_inputs(graph, node_id).get(Clock::PARAM_TEMPO) else { return };
    let value_type = &mut graph.inputs[input].value;
    value_type.set_actual_value(tempo);
    history.absorb_param(engine_node_id, Clock::PARAM_TEMPO, value_type.actual_value());
}
