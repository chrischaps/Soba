//! The ADSR's stage CV on its face.
//!
//! A cable into a stage's jack moves the stage around its knob: Attack,
//! Decay and Release by octaves of time, Sustain by level. The knob still
//! sets the stage and can still be turned, and shows what the envelope is
//! running (see [`super::knob_cv`]); the display draws the stages patched
//! like that the way they're running, with their letters lit.

use egui_node_graph2::NodeId;

use crate::dsp::Readout;
use crate::modules::envelope::AdsrEnvelope;

use super::{SynthGraph, SynthGraphState};

/// The ADSR's module ID.
pub const MODULE: &str = "mod.adsr";

/// The stages with a jack, in order, and where each sits in the readout.
const STAGES: [(&str, usize); 4] = [
    ("Attack", AdsrEnvelope::READOUT_ATTACK),
    ("Decay", AdsrEnvelope::READOUT_DECAY),
    ("Sustain", AdsrEnvelope::READOUT_SUSTAIN),
    ("Release", AdsrEnvelope::READOUT_RELEASE),
];

/// Whether something is patched into an ADSR node's `stage` jack.
pub fn patched(graph: &SynthGraph, node_id: NodeId, stage: &str) -> bool {
    let Some(node) = graph.nodes.get(node_id) else { return false };
    let Some(&(_, input)) = node.inputs.iter().find(|(name, _)| name == stage) else { return false };
    super::patched::input(graph, input)
}

/// Which stages are patched, A, D, S, R.
pub fn patched_stages(graph: &SynthGraph, node_id: NodeId) -> [bool; 4] {
    STAGES.map(|(stage, _)| patched(graph, node_id, stage))
}

/// The latest readout from an ADSR node.
pub fn readout(user_state: &SynthGraphState, node_id: NodeId) -> Option<&Readout> {
    user_state.get_engine_node_id(node_id).and_then(|id| user_state.readouts.get(&id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{create_editor_state, SynthNodeTemplate};
    use egui_node_graph2::NodeTemplateTrait;

    #[test]
    fn the_patched_stages_are_found() {
        let mut graph = create_editor_state().graph;
        let mut add = |module_id: &str| {
            let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
            let mut scratch = SynthGraphState::new();
            graph.add_node(
                template.node_graph_label(&mut scratch),
                template.user_data(&mut scratch),
                |graph, node_id| template.build_node(graph, &mut scratch, node_id),
            )
        };
        let (adsr, lfo) = (add(MODULE), add("mod.lfo"));
        assert_eq!(patched_stages(&graph, adsr), [false; 4]);

        let out = graph[lfo].get_output("Out").unwrap();
        for stage in ["Release", "Sustain"] {
            graph.add_connection(out, graph[adsr].get_input(stage).unwrap(), 0);
        }
        assert_eq!(patched_stages(&graph, adsr), [false, false, true, true]);
        assert!(!patched(&graph, adsr, "Decay Curve"));
    }
}
