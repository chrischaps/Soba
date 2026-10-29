//! Headless patch compilation.
//!
//! Turns a [`Patch`] into the [`EngineCommand`]s that build it in an audio
//! engine, without a UI. The patch is staged with [`stage_patch`], the same
//! loader the editor uses, and ports and parameters are resolved with the
//! shared [`port_mapping`] helpers, so the indices always match what the
//! editor sends.

use std::collections::HashMap;

use egui_node_graph2::NodeId;

use crate::dsp::onsets::{HandMarkers, Onsets, DEFAULT_SENSITIVITY};
use crate::dsp::primitives::{Markers, Slices};
use crate::dsp::SampleData;
use crate::engine::{EngineCommand, NodeId as EngineNodeId};
use crate::graph::{groups, port_mapping, scenes, SynthGraph};
use crate::modules::plays_a_sample;

use super::graph_io::patch_order;
use super::{stage_patch, Patch, PatchError};

/// The engine commands that build a patch, plus bookkeeping for callers.
#[derive(Debug, Default)]
pub struct CompiledPatch {
    /// Commands in the order they must be applied: modules, parameters, connections.
    pub commands: Vec<EngineCommand>,
    /// Engine node ID assigned to each patch node ID.
    pub node_ids: HashMap<u64, EngineNodeId>,
    /// Problems that were skipped rather than failing the whole patch,
    /// e.g. an unknown module or a connection naming a port that no longer exists.
    pub warnings: Vec<String>,
    /// The file each module plays (a Sampler's), as the patch saved it:
    /// relative paths still need resolving against where the patch is.
    pub files: Vec<(EngineNodeId, String)>,
    /// How each Sampler sliced by Onsets is cut, for the markers to be
    /// found once its file is read.
    pub slicing: Vec<(EngineNodeId, Slicing)>,
}

/// How a Sampler sliced by Onsets is cut, as the patch saved it.
#[derive(Clone, Debug, Default)]
pub struct Slicing {
    pub sensitivity: f32,
    /// Start and End, in order, as fractions of the recording.
    pub start: f32,
    pub end: f32,
    /// Markers placed by hand, if they were.
    pub hand: Option<HandMarkers>,
}

impl Slicing {
    /// The markers that cut `sample`: those placed by hand while they
    /// stand, or else where its onsets are, found now.
    pub fn markers(&self, sample: &SampleData) -> Markers {
        let places = match self.hand.as_ref().filter(|hand| hand.stand_at(self.sensitivity)) {
            Some(hand) => hand.places.clone(),
            None => Onsets::detect(sample).places(self.sensitivity),
        };
        Markers::between(&places, self.start, self.end)
    }
}

/// Compiles a patch into engine commands.
///
/// Fails only on an incompatible version. Anything else that can't be built
/// is skipped and reported in [`CompiledPatch::warnings`], exactly as when the
/// editor loads the patch.
pub fn compile_patch(patch: &Patch) -> Result<CompiledPatch, PatchError> {
    let staged = stage_patch(patch)?;
    let graph = &staged.graph;
    let mut compiled = CompiledPatch {
        warnings: staged.warnings.clone(),
        ..Default::default()
    };
    let mut engine_ids = HashMap::new();

    // Modules in patch ID order, so a patch numbers its modules the same
    // whether or not some of them are in groups
    let mut modules: Vec<_> = staged.nodes.iter().collect();
    modules.sort_by_key(|node| node.patch_id);
    for (engine_node_id, node) in (0..).zip(modules) {
        engine_ids.insert(node.graph_id, engine_node_id);
        compiled.node_ids.insert(node.patch_id, engine_node_id);

        compiled.commands.push(EngineCommand::AddModule {
            node_id: engine_node_id,
            module_id: node.template.module_id(),
        });
        // Its noise by its ID in the file, as the editor seeds it
        compiled.commands.push(EngineCommand::SeedModule { node_id: engine_node_id, seed: node.patch_id });

        // Send every parameter the way the editor's initial parameter sync does
        for (param_index, input_id) in port_mapping::parameter_inputs(graph, node.graph_id).into_iter().enumerate() {
            compiled.commands.push(EngineCommand::SetParameter {
                node_id: engine_node_id,
                param_index,
                value: graph.get_input(input_id).value.actual_value(),
            });
        }

        // An Instrument's knobs, from its profile or as the patch wrote them,
        // and a Kit's knobs and lanes
        if crate::hardware::instrument::plays_a_profile(node.template.module_id()) {
            let labels = &graph[node.graph_id].user_data.labels;
            let maps = crate::hardware::instrument::maps(labels, &crate::hardware::known());
            for (slot, map) in maps.into_iter().enumerate().filter(|(_, map)| !map.is_empty()) {
                compiled.commands.push(EngineCommand::MapSlot { node_id: engine_node_id, slot, map });
            }
            if node.template.module_id() == crate::modules::kit::ID {
                let lanes = crate::hardware::instrument::lane_maps(labels, &crate::hardware::known());
                for (lane, map) in lanes.into_iter().enumerate().filter(|(_, map)| !map.is_empty()) {
                    compiled.commands.push(EngineCommand::MapLane { node_id: engine_node_id, lane, map });
                }
            }
        }

        // A Setlist's rows
        if node.template.module_id() == crate::modules::setlist::ID {
            let maps = crate::hardware::setlist::maps(&graph[node.graph_id].user_data.labels, &crate::hardware::known());
            for (section, rows) in maps.iter().enumerate() {
                for (row, &map) in rows.iter().enumerate().filter(|(_, map)| map.is_some()) {
                    compiled.commands.push(EngineCommand::MapRow { node_id: engine_node_id, section, row, map });
                }
            }
        }

        // An FM Voice's playing voice (#472), for it to play
        if node.template.module_id() == crate::modules::fm_voice::ID {
            let voice = crate::hardware::garden::Garden::of(&graph[node.graph_id].user_data.labels).playing().voice;
            compiled.commands.push(EngineCommand::SetFmVoice { node_id: engine_node_id, voice });
        }

        if let Some(file) = &graph[node.graph_id].user_data.file {
            compiled.files.push((engine_node_id, file.clone()));
        }
        if let Some(slicing) = slicing(graph, node.graph_id) {
            compiled.slicing.push((engine_node_id, slicing));
        }

        if graph[node.graph_id].user_data.bypassed {
            compiled.commands.push(EngineCommand::SetBypass { node_id: engine_node_id, bypassed: true });
        }
    }

    // Module to module, through any groups between them, in the order
    // staging took them in to skip any that would close a loop
    let patch_ids: HashMap<NodeId, u64> = staged.nodes.iter().map(|n| (n.graph_id, n.patch_id)).collect();
    let mut cables = groups::leaf_cables(graph);
    patch_order(graph, &mut cables, &patch_ids);
    for (output_id, input_id) in cables {
        let from = graph.get_output(output_id).node;
        let to = graph.get_input(input_id).node;
        // Staging only keeps connections between ports that map to the engine
        let (Some(from_port), Some(to_port)) = (
            port_mapping::output_port_index(graph, from, output_id),
            port_mapping::input_port_index(graph, to, input_id),
        ) else {
            continue;
        };
        compiled.commands.push(EngineCommand::Connect {
            from_node: engine_ids[&from],
            from_port,
            to_node: engine_ids[&to],
            to_port,
        });
    }

    // Scenes modules turn their knobs in the engine as Morph moves. Their
    // scenes name modules by ID in the file
    let graph_ids: HashMap<u64, NodeId> = patch_ids.iter().map(|(&graph_id, &patch_id)| (patch_id, graph_id)).collect();
    for node in &staged.nodes {
        if graph[node.graph_id].user_data.module_id != crate::modules::scenes::ID {
            continue;
        }
        let knobs = scenes::engine_knobs(graph, node.graph_id, |id| graph_ids.get(&id).copied(), |n| engine_ids.get(&n).copied());
        if !knobs.is_empty() {
            compiled.commands.push(EngineCommand::SetScenes { node_id: engine_ids[&node.graph_id], knobs });
        }
    }

    Ok(compiled)
}

/// How a Sampler (or a Chopper) is cut, if it's sliced by Onsets.
fn slicing(graph: &SynthGraph, node_id: NodeId) -> Option<Slicing> {
    let node = &graph[node_id];
    if !plays_a_sample(node.user_data.module_id) {
        return None;
    }
    let value = |name: &str| node.inputs.iter().find(|(input, _)| input == name).map(|(_, id)| graph.get_input(*id).value.actual_value());
    if !value("Slices").is_some_and(Slices::is_marked_param) {
        return None;
    }
    let start = value("Start").unwrap_or(0.0).clamp(0.0, 1.0);
    let end = value("End").unwrap_or(1.0).clamp(0.0, 1.0);
    Some(Slicing {
        sensitivity: value("Sensitivity").unwrap_or(DEFAULT_SENSITIVITY),
        start: start.min(end),
        end: start.max(end),
        hand: node.user_data.slice_markers.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{ConnectionData, NamedParameter, NodeData, ParameterValue};

    fn osc_to_output() -> Patch {
        let mut patch = Patch::new("test");
        let mut osc = NodeData::new(10, "osc.sine", (0.0, 0.0));
        osc.parameters = vec![
            NamedParameter::new("Octave", ParameterValue::Number(-2.0)),
            NamedParameter::new("Semitone", ParameterValue::Number(9.0)),
            NamedParameter::new("FM Depth", ParameterValue::Number(0.0)),
            NamedParameter::new("Waveform", ParameterValue::Select(1)),
            NamedParameter::new("Pulse Width", ParameterValue::LinearRange(0.5)),
        ];
        patch.nodes.push(osc);
        patch.nodes.push(NodeData::new(20, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(10, "Out", 20, "Mono"));
        patch
    }

    #[test]
    fn test_an_instrument_sends_every_knob_it_holds_as_it_loads() {
        // Send all, on opening the patch (#442): a known profile's knob and
        // an unknown one's, each as the patch wrote it down
        let mut patch = Patch::new("rig");
        let mut freak = NodeData::new(1, crate::modules::instrument::ID, (0.0, 0.0));
        freak.labels.insert("profile".into(), "MicroFreak".into());
        freak.labels.insert("slot 1".into(), r#"{"name":"Cutoff","cc":1}"#.into());
        freak.labels.insert("slot 4".into(), r#"{"name":"Fold","cc":31}"#.into());
        patch.nodes.push(freak);
        let compiled = compile_patch(&patch).unwrap();
        let maps: Vec<(usize, crate::hardware::SlotMap)> = compiled
            .commands
            .iter()
            .filter_map(|command| match command {
                EngineCommand::MapSlot { slot, map, .. } => Some((*slot, *map)),
                _ => None,
            })
            .collect();
        assert_eq!(maps.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(), [0, 3], "only the knobs that hold something");
        let profiles = crate::hardware::known();
        let cutoff = profiles.find("MicroFreak").unwrap().profile.parameter("Cutoff").unwrap();
        assert_eq!(maps[0].1, crate::hardware::SlotMap::of(cutoff), "the profile's own Cutoff, not CC 1 as written");
        assert_eq!(maps[1].1.address, Some(crate::hardware::Address::Cc(31)));
    }

    #[test]
    fn test_a_kit_hands_over_its_lanes_and_knobs_as_it_loads() {
        // A Digitakt's pads as the patch chose them (#443): lane 2 on Track 5
        let mut patch = Patch::new("rig");
        let mut kit = NodeData::new(1, crate::modules::kit::ID, (0.0, 0.0));
        kit.labels.insert("profile".into(), "Digitakt".into());
        kit.labels.insert("lane 1".into(), r#"{"name":"Track 1","note":0,"channel":1}"#.into());
        kit.labels.insert("lane 2".into(), r#"{"name":"Track 5","note":4,"channel":5}"#.into());
        kit.labels.insert("slot 1".into(), r#"{"name":"Filter Freq","cc":74}"#.into());
        patch.nodes.push(kit);
        let compiled = compile_patch(&patch).unwrap();
        let lanes: Vec<(usize, crate::hardware::LaneMap)> = compiled
            .commands
            .iter()
            .filter_map(|command| match command {
                EngineCommand::MapLane { lane, map, .. } => Some((*lane, *map)),
                _ => None,
            })
            .collect();
        assert_eq!(lanes.len(), 2, "only the lanes that play something: {lanes:?}");
        assert_eq!((lanes[1].0, lanes[1].1.note(), lanes[1].1.channel(0)), (1, 4, 4));
        assert!(lanes[1].1.track, "the Digitakt's knobs follow its tracks");
        assert!(compiled.commands.iter().any(|command| matches!(command, EngineCommand::MapSlot { slot: 0, .. })));
    }

    #[test]
    fn test_an_fm_voice_hands_over_its_playing_voice() {
        // A new node's garden is the seeds, Soft Keys playing (#472)
        let mut patch = Patch::new("fm");
        patch.nodes.push(NodeData::new(1, crate::modules::fm_voice::ID, (0.0, 0.0)));
        let compiled = compile_patch(&patch).unwrap();
        let voices: Vec<_> = compiled.commands.iter().filter_map(|command| match command {
            EngineCommand::SetFmVoice { voice, .. } => Some(voice.name()),
            _ => None,
        }).collect();
        assert_eq!(voices, ["SOFT KEYS"]);
    }

    #[test]
    fn test_compiles_modules_params_and_connections() {
        let compiled = compile_patch(&osc_to_output()).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);

        let osc = compiled.node_ids[&10];
        let out = compiled.node_ids[&20];

        assert!(compiled.commands.iter().any(|c| matches!(c,
            EngineCommand::AddModule { node_id, module_id: "osc.sine" } if *node_id == osc)));

        // Semitone is parameter 1 and keeps its saved value
        assert!(compiled.commands.iter().any(|c| matches!(c,
            EngineCommand::SetParameter { node_id, param_index: 1, value }
                if *node_id == osc && *value == 9.0)));

        // Oscillator "Out" is port 5 (after 5 inputs); output "Mono" is input port 2
        assert!(compiled.commands.iter().any(|c| matches!(c,
            EngineCommand::Connect { from_node, from_port: 5, to_node, to_port: 2 }
                if *from_node == osc && *to_node == out)));
    }

    #[test]
    fn test_a_tide_minutes_long_loads_at_its_own_rate() {
        // 213 s round: slower than the LFO's Rate knob once reached
        let mut patch = Patch::new("tide");
        let mut lfo = NodeData::new(1, "mod.lfo", (0.0, 0.0));
        lfo.parameters = vec![NamedParameter::new("Rate", ParameterValue::Number(0.0047))];
        patch.nodes.push(lfo);
        let compiled = compile_patch(&patch).unwrap();
        let rate = compiled.commands.iter().find_map(|command| match *command {
            EngineCommand::SetParameter { param_index: 0, value, .. } => Some(value),
            _ => None,
        });
        assert_eq!(rate, Some(0.0047));
    }

    #[test]
    fn test_missing_parameters_fall_back_to_defaults() {
        // Patches saved before the Character toggle don't name it; Limiter
        // keeps its saved value and Character falls back to its default (off)
        let mut patch = osc_to_output();
        patch.nodes[1].parameters = vec![
            NamedParameter::new("Volume", ParameterValue::Scalar(0.5)),
            NamedParameter::new("Limiter", ParameterValue::Toggle(false)),
        ];
        let compiled = compile_patch(&patch).unwrap();

        let out = compiled.node_ids[&20];
        let params: Vec<(usize, f32)> = compiled
            .commands
            .iter()
            .filter_map(|c| match c {
                EngineCommand::SetParameter { node_id, param_index, value } if *node_id == out => {
                    Some((*param_index, *value))
                }
                _ => None,
            })
            .collect();
        assert_eq!(params, vec![(0, 0.5), (1, 0.0), (2, 0.0)]);
    }

    #[test]
    fn test_unknown_module_is_skipped_with_a_warning() {
        let mut patch = osc_to_output();
        patch.nodes.push(NodeData::new(30, "does.not.exist", (0.0, 0.0)));
        patch.connections.push(ConnectionData::new(30, "Out", 20, "Left"));
        let compiled = compile_patch(&patch).unwrap();

        // The other nodes and their connection still build
        assert_eq!(compiled.node_ids.len(), 2);
        assert!(!compiled.node_ids.contains_key(&30));
        assert_eq!(
            compiled.commands.iter().filter(|c| matches!(c, EngineCommand::Connect { .. })).count(),
            1
        );
        // One warning for the node, one for its connection
        assert_eq!(compiled.warnings.len(), 2, "{:?}", compiled.warnings);
    }

    #[test]
    fn test_incompatible_version_is_an_error() {
        let mut patch = osc_to_output();
        patch.version = crate::persistence::PATCH_VERSION + 1;
        assert!(matches!(compile_patch(&patch), Err(PatchError::IncompatibleVersion { .. })));
    }

    #[test]
    fn test_bad_port_name_is_a_warning() {
        let mut patch = osc_to_output();
        patch.connections.push(ConnectionData::new(10, "Nope", 20, "Left"));
        let compiled = compile_patch(&patch).unwrap();
        assert_eq!(compiled.warnings.len(), 1);
    }

    fn connects(compiled: &CompiledPatch) -> usize {
        compiled.commands.iter().filter(|c| matches!(c, EngineCommand::Connect { .. })).count()
    }

    #[test]
    fn test_a_cable_closing_a_loop_is_skipped_with_a_warning() {
        // The filter's LowPass back into the oscillator that feeds it
        let mut patch = osc_to_output();
        patch.nodes.push(NodeData::new(30, "filter.svf", (100.0, 0.0)));
        patch.connections.push(ConnectionData::new(10, "Out", 30, "In"));
        patch.connections.push(ConnectionData::new(30, "LowPass", 10, "FM"));
        let compiled = compile_patch(&patch).unwrap();

        assert_eq!(connects(&compiled), 2);
        assert_eq!(compiled.warnings.len(), 1, "{:?}", compiled.warnings);
        let warning = &compiled.warnings[0];
        assert!(warning.starts_with("Skipped connection SVF Filter (node 30) 'LowPass' -> Oscillator (node 10) 'FM'"), "{warning}");
        assert!(warning.ends_with("it would close the loop Oscillator → SVF Filter → Oscillator"), "{warning}");

        // Nor does the editor show it
        let staged = stage_patch(&patch).unwrap();
        let osc = staged.graph_id(10).unwrap();
        let fm = staged.graph[osc].get_input("FM").unwrap();
        assert_eq!(staged.graph.connection(fm), None);
    }

    #[test]
    fn test_a_loop_through_a_group_is_skipped_too() {
        use crate::persistence::{GroupData, JackData};
        let jack = |name: &str| JackData { name: name.into(), signal: "Audio".into() };
        let mut patch = osc_to_output();
        patch.groups.push(GroupData {
            id: 9000,
            name: "Tone".into(),
            position: (100.0, 0.0),
            inputs: vec![jack("In")],
            outputs: vec![jack("LowPass")],
            inputs_position: (0.0, 0.0),
            outputs_position: (300.0, 0.0),
            nodes: vec![NodeData::new(30, "filter.svf", (150.0, 0.0))],
            connections: vec![
                ConnectionData::new(9000, "In", 30, "In"),
                ConnectionData::new(30, "LowPass", 9000, "LowPass"),
            ],
            groups: Vec::new(),
        });
        patch.connections.push(ConnectionData::new(10, "Out", 9000, "In"));
        patch.connections.push(ConnectionData::new(9000, "LowPass", 10, "FM"));
        let compiled = compile_patch(&patch).unwrap();

        assert_eq!(connects(&compiled), 2);
        assert_eq!(compiled.warnings.len(), 1, "{:?}", compiled.warnings);
        assert!(compiled.warnings[0].contains("SVF Filter (node 30) 'LowPass' -> Oscillator (node 10) 'FM'"), "{:?}", compiled.warnings);
    }

    #[test]
    fn test_a_loop_closed_through_a_late_input_is_kept() {
        // A mixer's send through a filter and back into its Return
        let mut patch = osc_to_output();
        patch.connections.clear();
        patch.nodes.push(NodeData::new(30, "filter.svf", (100.0, 0.0)));
        patch.nodes.push(NodeData::new(40, "util.mixer", (100.0, 0.0)));
        patch.connections.push(ConnectionData::new(10, "Out", 40, "Ch 1"));
        patch.connections.push(ConnectionData::new(40, "Send L", 30, "In"));
        patch.connections.push(ConnectionData::new(30, "LowPass", 40, "Return L"));
        patch.connections.push(ConnectionData::new(40, "Out L", 20, "Left"));
        let compiled = compile_patch(&patch).unwrap();

        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        assert_eq!(connects(&compiled), 4);
    }
}
