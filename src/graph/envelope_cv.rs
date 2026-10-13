//! The ADSR's stage CV on its face.
//!
//! A cable into a stage's jack moves the stage around its knob: Attack,
//! Decay and Release by octaves of time, Sustain by level. The knob still
//! sets the stage and can still be turned, but while its jack is patched it
//! reads what the envelope is running, so a lane stretching the release
//! shows on the Rel knob as well as in the display.

use egui_node_graph2::NodeId;

use crate::dsp::Readout;
use crate::modules::envelope::AdsrEnvelope;
use crate::widgets::ParamFormat;

use super::clock_display::KnobTakeover;
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
    graph.iter_connections().any(|(connected, _)| connected == input)
}

/// Which stages are patched, A, D, S, R.
pub fn patched_stages(graph: &SynthGraph, node_id: NodeId) -> [bool; 4] {
    STAGES.map(|(stage, _)| patched(graph, node_id, stage))
}

/// The latest readout from an ADSR node.
pub fn readout(user_state: &SynthGraphState, node_id: NodeId) -> Option<&Readout> {
    user_state.get_engine_node_id(node_id).and_then(|id| user_state.readouts.get(&id))
}

/// While a stage's jack is patched, its knob reads what CV has made of it.
pub fn stage_takeover(
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    node_id: NodeId,
    param: &str,
) -> Option<KnobTakeover> {
    let &(stage, index) = STAGES.iter().find(|(stage, _)| *stage == param)?;
    if !patched(graph, node_id, stage) {
        return None;
    }
    let value = readout(user_state, node_id)?.values[index];
    let format = if stage == "Sustain" { ParamFormat::Percent } else { ParamFormat::Time };
    Some(KnobTakeover { value: None, text: format.format(value), locked: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{create_editor_state, SynthNodeTemplate};
    use egui_node_graph2::NodeTemplateTrait;

    /// An ADSR whose engine has reported `values`, and an LFO to patch from.
    fn rig(values: [f32; 4]) -> (SynthGraph, SynthGraphState, NodeId, NodeId) {
        let mut graph = create_editor_state().graph;
        let mut user_state = SynthGraphState::new();
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
        let engine = user_state.allocate_engine_node_id(adsr);
        let mut readout = Readout::default();
        for ((_, index), value) in STAGES.iter().zip(values) {
            readout.values[*index] = value;
        }
        user_state.readouts.insert(engine, readout);
        (graph, user_state, adsr, lfo)
    }

    #[test]
    fn a_patched_stage_reads_what_cv_made_of_it() {
        let (mut graph, user_state, adsr, lfo) = rig([0.02, 0.4, 0.85, 2.5]);
        let takeover = |graph: &SynthGraph, param| stage_takeover(graph, &user_state, adsr, param);

        // Unpatched, the knobs read their own values
        assert!(STAGES.iter().all(|(stage, _)| takeover(&graph, stage).is_none()));
        assert_eq!(patched_stages(&graph, adsr), [false; 4]);

        let out = graph[lfo].get_output("Out").unwrap();
        for stage in ["Release", "Sustain"] {
            graph.add_connection(out, graph[adsr].get_input(stage).unwrap(), 0);
        }
        assert_eq!(patched_stages(&graph, adsr), [false, false, true, true]);

        // Patched, they read the envelope, and can still be turned
        let release = takeover(&graph, "Release").unwrap();
        assert_eq!(release.text, "2.50 s");
        assert!(!release.locked && release.value.is_none());
        assert_eq!(takeover(&graph, "Sustain").unwrap().text, "85%");
        assert!(takeover(&graph, "Decay").is_none());
        assert!(takeover(&graph, "Decay Curve").is_none());
    }
}
