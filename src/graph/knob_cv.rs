//! Where CV has carried a modulatable knob.
//!
//! A knob whose jack modulates it keeps its own setting: the pointer stays
//! where the hand left it, the base the CV works around. What the module
//! actually runs is somewhere else, and this is where: each modulatable
//! knob's law, the way its module combines the knob with its CV, so the knob
//! can draw an arc out to a bead there and read it under the dial.
//!
//! Most laws are a line of arithmetic, copied from the module's process loop
//! (each entry names the line it mirrors). A module that already reports what
//! CV has made of a parameter is read instead.
//!
//! A knob's CV usually comes in through its own jack, but not always: a VCA's
//! CV jack scales its Level, an Oscillator's PWM jack moves its Pulse Width.
//! [`jack`] names those, and the engine watches them for the knob's sake.

use egui_node_graph2::NodeId;

use crate::modules::clock::Clock;
use crate::modules::envelope::AdsrEnvelope;
use crate::modules::slope::Slope;
use crate::modules::vocoder::Vocoder;

use super::envelope_cv::MODULE as ADSR;
use super::{SynthGraph, SynthGraphState};

/// How a module combines a knob with its CV.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CvLaw {
    /// `base + scale * cv`
    Add { scale: f32 },
    /// `base * 2^cv`: a unit of CV is an octave
    Octaves,
    /// `base * 2^(cv * depth)`, the octaves per unit set by another knob
    OctavesBy(&'static str),
    /// `base * (1 + scale * cv)`
    Stretch { scale: f32 },
    /// `base * (1 - amount + amount * cv)`, `cv` held to 0..1: at 0 it takes
    /// off `amount` of the knob, at 1 none of it
    Gain { amount: f32 },
    /// `Gain`, the amount set by another knob: a VCA's CV Amount
    GainBy(&'static str),
    /// `base - by * (1 - cv)`, `cv` held to 0..1: at 0 it takes `by` off
    Dim { by: f32 },
    /// The module reports it, at this place in its readout
    Readout(usize),
}

/// The law for a modulatable knob, if it has one.
pub fn law(module_id: &str, param: &str) -> Option<CvLaw> {
    use CvLaw::*;
    Some(match (module_id, param) {
        ("source.noise", "Level") => Add { scale: 1.0 }, // noise.rs: knob + cv
        ("source.noise", "Rate") => Octaves,             // noise.rs: knob * 2^cv
        ("source.drum", "Tune") => Add { scale: 12.0 },  // drum.rs: semitones + 12 cv
        ("source.drum", "Decay") => Add { scale: 1.0 },
        ("source.sampler", "Start" | "Speed") => Add { scale: 1.0 },
        // cloud.rs: Position + cv, Size and Density * 2^cv, semitones + 12 cv
        ("source.cloud", "Position") => Add { scale: 1.0 },
        ("source.cloud", "Size" | "Density") => Octaves,
        ("source.cloud", "Pitch") => Add { scale: 12.0 },
        // filter.rs and ladder.rs: octaves in the log domain, times the depth knob
        ("filter.svf" | "filter.ladder", "Cutoff") => OctavesBy("Cutoff CV"),
        ("filter.svf" | "filter.ladder", "Resonance") => Add { scale: 0.5 },
        (ADSR, "Attack") => Readout(AdsrEnvelope::READOUT_ATTACK),
        (ADSR, "Decay") => Readout(AdsrEnvelope::READOUT_DECAY),
        (ADSR, "Sustain") => Readout(AdsrEnvelope::READOUT_SUSTAIN),
        (ADSR, "Release") => Readout(AdsrEnvelope::READOUT_RELEASE),
        ("mod.lfo", "Rate") => Octaves, // lfo.rs: base_rate * 2^cv
        ("mod.slope", "Rise") => Readout(Slope::READOUT_RISE),
        ("mod.slope", "Fall") => Readout(Slope::READOUT_FALL),
        ("util.clock", "Tempo") => Readout(Clock::READOUT_TEMPO),
        // mixer.rs: every strip's knobs, and the return, add their CV
        ("util.mixer", _) => Add { scale: 1.0 },
        ("util.quantizer", "Transpose") => Add { scale: 12.0 }, // quantizer.rs: knob + 12 cv
        ("fx.delay", "Time") => Stretch { scale: 0.5 },         // delay.rs: ms * (1 + cv/2)
        ("fx.delay", "Feedback") => Add { scale: 0.5 },
        ("fx.distortion", "Drive") => Add { scale: 0.5 },
        ("fx.chorus", "Rate") => Stretch { scale: 0.5 },
        ("fx.chorus", "Depth") => Add { scale: 0.5 },
        ("fx.tape", "Wow") => Add { scale: 1.0 },
        ("fx.slicer", "Depth") => Add { scale: 1.0 }, // slicer.rs: knob + cv
        ("fx.vocoder", "Formant") => Readout(Vocoder::READOUT_FORMANT),
        // Knobs whose CV comes through another jack (see `jack`)
        ("util.vca", "Level") => GainBy("CV Amount"),      // vca.rs: level * (1 - amt + cv * amt)
        ("osc.sine", "Pulse Width") => Add { scale: 0.4 }, // oscillator.rs: width + 0.4 pwm
        // drum.rs Voicing::new: a softer hit is darker and less snappy
        ("source.drum", "Tone") => Dim { by: 0.25 },
        ("source.drum", "Snap") => Gain { amount: 0.5 },
        _ => return None,
    })
}

/// The jack a knob's CV comes through, when it isn't the knob's own.
pub fn jack(module_id: &str, param: &str) -> Option<&'static str> {
    Some(match (module_id, param) {
        ("util.vca", "Level") => "CV",
        ("osc.sine", "Pulse Width") => "PWM",
        ("source.drum", "Tone" | "Snap") => "Accent",
        _ => return None,
    })
}

/// Where `cv` has carried a knob set to `base`, unclamped, or `None` if the
/// knob's law isn't known (or the module hasn't reported yet).
pub fn effective(
    module_id: &str,
    param: &str,
    base: f32,
    cv: f32,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
) -> Option<f32> {
    Some(match law(module_id, param)? {
        CvLaw::Add { scale } => base + scale * cv,
        CvLaw::Octaves => base * cv.exp2(),
        CvLaw::OctavesBy(depth) => base * (cv * super::clock_display::number(graph, node_id, depth)?).exp2(),
        CvLaw::Stretch { scale } => base * (1.0 + scale * cv),
        CvLaw::Gain { amount } => gain(base, amount, cv),
        CvLaw::GainBy(amount) => gain(base, super::clock_display::number(graph, node_id, amount)?, cv),
        CvLaw::Dim { by } => base - by * (1.0 - cv.clamp(0.0, 1.0)),
        CvLaw::Readout(index) => {
            let engine_id = user_state.get_engine_node_id(node_id)?;
            user_state.readouts.get(&engine_id)?.values[index]
        }
    })
}

/// `base * (1 - amount + amount * cv)`, `cv` held to 0..1.
fn gain(base: f32, amount: f32, cv: f32) -> f32 {
    base * (1.0 - amount + amount * cv.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Readout;
    use crate::graph::node_data::KnobInputMode;
    use crate::graph::{create_editor_state, AllNodeTemplates, SynthNodeTemplate, SynthValueType};
    use egui_node_graph2::{InputParamKind, NodeTemplateIter, NodeTemplateTrait};

    /// The value of a node's Number parameter `name`.
    fn base(graph: &SynthGraph, node_id: NodeId, name: &str) -> f32 {
        let node = &graph[node_id];
        let (_, input) = node.inputs.iter().find(|(input, _)| input == name).unwrap();
        match graph.get_input(*input).value {
            SynthValueType::Number { value, .. } => value,
            _ => panic!("{name} isn't a number"),
        }
    }

    /// A graph holding one node of `module_id`.
    fn one(module_id: &str) -> (SynthGraph, SynthGraphState, NodeId) {
        let mut graph = create_editor_state().graph;
        let mut user_state = SynthGraphState::new();
        let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
        let node_id = graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        );
        (graph, user_state, node_id)
    }

    #[test]
    fn every_modulatable_knob_has_a_law() {
        // A new modulatable knob would otherwise show only a dot when patched
        let mut user_state = SynthGraphState::new();
        let missing: Vec<String> = AllNodeTemplates
            .all_kinds()
            .into_iter()
            .flat_map(|template| {
                let data = template.user_data(&mut user_state);
                data.knob_params
                    .into_iter()
                    .filter(|knob| knob.input_mode == KnobInputMode::Modulatable)
                    .filter(move |knob| law(data.module_id, &knob.param_name).is_none())
                    .map(move |knob| format!("{} {}", data.module_id, knob.param_name))
            })
            .collect();
        assert!(missing.is_empty(), "modulatable knobs with no CV law: {missing:?}");
    }

    #[test]
    fn laws_carry_the_knob_as_their_modules_do() {
        let (graph, user_state, node_id) = one("fx.delay");
        let at = |module, param, base, cv| effective(module, param, base, cv, node_id, &graph, &user_state).unwrap();
        assert_eq!(at("util.mixer", "Send 2", 0.25, 0.5), 0.75);
        assert_eq!(at("filter.svf", "Resonance", 0.5, 0.4), 0.7);
        assert_eq!(at("util.quantizer", "Transpose", 0.0, -0.5), -6.0);
        assert_eq!(at("mod.lfo", "Rate", 2.0, 1.0), 4.0);
        assert_eq!(at("mod.lfo", "Rate", 2.0, -1.0), 1.0);
        assert_eq!(at("fx.delay", "Time", 400.0, 1.0), 600.0);
        assert!(law("fx.delay", "Mix").is_none());
        assert!(effective("fx.delay", "Mix", 0.5, 1.0, node_id, &graph, &user_state).is_none());
    }

    #[test]
    fn a_cutoff_moves_by_its_depth_knob() {
        let (mut graph, user_state, node_id) = one("filter.svf");
        let cutoff = |graph: &SynthGraph| effective("filter.svf", "Cutoff", 1000.0, 1.0, node_id, graph, &user_state).unwrap();
        assert_eq!(base(&graph, node_id, "Cutoff CV"), 1.0);
        assert!((cutoff(&graph) - 2000.0).abs() < 0.01, "an octave up at the default depth");

        let (_, depth) = *graph[node_id].inputs.iter().find(|(name, _)| name == "Cutoff CV").unwrap();
        if let SynthValueType::Number { value, .. } = &mut graph.inputs[depth].value {
            *value = -2.0;
        }
        assert!((cutoff(&graph) - 250.0).abs() < 0.01, "two octaves down with the depth turned negative");
    }

    #[test]
    fn every_jack_named_is_on_its_module_with_a_law() {
        // A misspelt jack would leave its knob showing nothing, silently
        let mut user_state = SynthGraphState::new();
        let mut named = 0;
        for template in AllNodeTemplates.all_kinds() {
            let data = template.user_data(&mut user_state);
            let (graph, _, node_id) = one(data.module_id);
            for knob in &data.knob_params {
                let Some(jack) = jack(data.module_id, &knob.param_name) else { continue };
                named += 1;
                let input = graph[node_id].inputs.iter().find(|(name, _)| name == jack);
                let is_jack = input.is_some_and(|(_, id)| matches!(graph.get_input(*id).kind, InputParamKind::ConnectionOnly));
                assert!(is_jack, "{} has no {jack} jack for {}", data.module_id, knob.param_name);
                assert!(law(data.module_id, &knob.param_name).is_some(), "{} {} has a jack but no law", data.module_id, knob.param_name);
            }
        }
        assert!(named >= 4, "the VCA's, the Oscillator's and the Drum's at least");
    }

    #[test]
    fn a_vca_level_follows_its_cv_by_the_cv_amount() {
        let (mut graph, user_state, node_id) = one("util.vca");
        let level = |graph: &SynthGraph, cv| effective("util.vca", "Level", 0.8, cv, node_id, graph, &user_state).unwrap();
        assert_eq!(base(&graph, node_id, "CV Amount"), 1.0);
        assert!((level(&graph, 0.5) - 0.4).abs() < 1e-6, "at full amount the CV scales the level");
        assert_eq!(level(&graph, 1.5), 0.8, "CV above 1 opens it no further");
        assert_eq!(level(&graph, -1.0), 0.0, "nor below 0 closes it further");

        let (_, amount) = *graph[node_id].inputs.iter().find(|(name, _)| name == "CV Amount").unwrap();
        if let SynthValueType::Number { value, .. } = &mut graph.inputs[amount].value {
            *value = 0.25;
        }
        assert!((level(&graph, 0.0) - 0.6).abs() < 1e-6, "a quarter amount takes off a quarter at most");
    }

    #[test]
    fn a_soft_drum_hit_is_darker_and_less_snappy() {
        // As drum.rs voices it
        use crate::modules::drum::{DrumType, Strike, Voicing};
        let (graph, user_state, node_id) = one("source.drum");
        let at = |param, base, cv| effective("source.drum", param, base, cv, node_id, &graph, &user_state).unwrap();
        for accent in [0.0, 0.3, 1.0] {
            let strike = Strike { tone: 0.6, snap: 0.8, accent, ..Strike::default() };
            let voicing = Voicing::new(DrumType::Kick, strike);
            assert!((at("Tone", 0.6, accent) - voicing.tone).abs() < 1e-6, "tone at accent {accent}");
            assert!((at("Snap", 0.8, accent) - voicing.snap).abs() < 1e-6, "snap at accent {accent}");
        }
    }

    #[test]
    fn a_module_that_reports_it_is_read() {
        let (graph, mut user_state, node_id) = one("mod.adsr");
        let release = |user_state: &SynthGraphState| effective("mod.adsr", "Release", 0.3, 1.0, node_id, &graph, user_state);
        assert_eq!(release(&user_state), None, "nothing reported yet");

        let engine = user_state.allocate_engine_node_id(node_id);
        let mut readout = Readout::default();
        readout.values[AdsrEnvelope::READOUT_RELEASE] = 2.5;
        user_state.readouts.insert(engine, readout);
        assert_eq!(release(&user_state), Some(2.5));
    }
}
