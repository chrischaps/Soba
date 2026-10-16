//! A Link Clock's knob and switch follow what its peers do.
//!
//! When another app in the Link session changes the tempo, or starts or
//! stops, the Clock follows it on the audio thread straight away (see
//! [`crate::modules::clock::link`]) and counts the change in its readout.
//! Here the app moves the Clock's Tempo knob and Run switch to match, so
//! they always say what the session does: turning the knob carries on from
//! the session's tempo, and Run reads whether everyone is playing.
//!
//! A peer's change isn't an edit of the patch, so it isn't an undo step,
//! just as a MIDI controller turning a knob isn't.

use crate::app::undo::History;
use crate::dsp::Readout;
use crate::graph::port_mapping::parameter_inputs;
use crate::graph::{follows_link, SynthGraph, SynthGraphState};
use crate::modules::clock::Clock;

/// Moves a Link Clock's Tempo knob and Run switch to match its session, if
/// a peer changed it since the Clock's `previous` readout.
pub(super) fn follow_peers(
    graph: &mut SynthGraph,
    history: &mut History,
    user_state: &SynthGraphState,
    engine_node_id: u64,
    previous: Option<Readout>,
) {
    let Some(readout) = user_state.readouts.get(&engine_node_id) else { return };
    let Some(node_id) = user_state.node_id_map.iter().find(|(_, &id)| id == engine_node_id).map(|(&node_id, _)| node_id) else {
        return;
    };
    let is_link_clock = graph.nodes.get(node_id).is_some_and(|node| node.user_data.module_id == "util.clock") && follows_link(graph, node_id);
    if !is_link_clock {
        return;
    }

    let params = parameter_inputs(graph, node_id);
    let mut set = |index: usize, value: f32| {
        let Some(&input) = params.get(index) else { return };
        let value_type = &mut graph.inputs[input].value;
        value_type.set_actual_value(value);
        history.absorb_param(engine_node_id, index, value_type.actual_value());
    };

    let changed = |index: usize| previous.map_or(0.0, |previous| previous.values[index]) != readout.values[index];
    if changed(Clock::READOUT_LINK_TEMPO_CHANGES) {
        // A tempo past the knob's ends stays on the readout only: setting
        // the knob to its end would propose that to the session
        let tempo = readout.values[Clock::READOUT_TEMPO];
        if (Clock::MIN_TEMPO as f32..=Clock::MAX_TEMPO as f32).contains(&tempo) {
            set(Clock::PARAM_TEMPO, tempo);
        }
    }
    if changed(Clock::READOUT_LINK_PLAY_CHANGES) {
        set(Clock::PARAM_RUN, readout.values[Clock::READOUT_LINK_PLAYING]);
    }
}
