//! A real patch from every released format, loaded by this build.
//!
//! Each fixture was saved by the code of its day, through that commit's own
//! `stage_patch` and `capture_patch`: every module it had, each knob moved off
//! its default, cables between them and three MIDI mappings on the filter. So a
//! parameter or port renamed since, a range narrowed under a saved value, or a
//! migration that goes wrong shows up here rather than in someone's old patch.
//!
//! | Fixture | Saved by |
//! |---|---|
//! | `patch_v3.json` | `ceb9178`, the last commit before v4 |
//! | `patch_v4.json` | `d8c64e0`, the last commit before v5 |
//! | `patch_v5.json` | v0.2.0, the last release before groups |
//! | `patch_v6.json` | v0.7.0. Also the library's Electric Piano two groups deep, a Sampler's file, a bypassed Delay, a frame and a note |
//!
//! `patch_v2.json` (in `graph_io`'s own tests) covers v2, which went by
//! position. When the format version goes up, write the outgoing format's
//! fixture with [`write_a_fixture_of_this_format`] and add it here.

use super::tests::reload;
use super::*;
use crate::graph::{AllNodeTemplates, GroupId, SynthValueType};
use crate::persistence::{patch_from_json, patch_to_json, save_to_file, FrameData, JackData, MidiMapping, NoteData};
use egui_node_graph2::NodeTemplateIter;

const FIXTURES: [(u32, &str); 4] = [
    (3, include_str!("fixtures/patch_v3.json")),
    (4, include_str!("fixtures/patch_v4.json")),
    (5, include_str!("fixtures/patch_v5.json")),
    (6, include_str!("fixtures/patch_v6.json")),
];

/// Cables older patches lose on loading, by module and port. Each is a
/// break waiting on a migration: fix it, take it out of here, and the test
/// holds that fixture to loading without a warning.
const KNOWN_DROPS: &[(&str, &str, &str)] = &[
    // #373: the Mixer's mono Out went in b3ee02f
    ("util.mixer", "Out", "no such port"),
];

fn fixture(version: u32) -> Patch {
    let (_, json) = FIXTURES.iter().find(|(v, _)| *v == version).unwrap();
    patch_from_json(json).unwrap()
}

/// Cables on a level and every level inside it.
fn cables(connections: &[ConnectionData], groups: &[GroupData]) -> usize {
    connections.len() + groups.iter().map(|g| cables(&g.connections, &g.groups)).sum::<usize>()
}

fn param(node: &NodeData, name: &str) -> f32 {
    node.parameters.iter().find(|p| p.name == name).unwrap().value.as_f32()
}

fn only<'a>(patch: &'a Patch, module_id: &str) -> &'a NodeData {
    let mut found = patch.all_nodes().into_iter().filter(|n| n.module_id == module_id);
    let node = found.next().unwrap();
    assert!(found.next().is_none(), "more than one {module_id}");
    node
}

/// The warnings a patch's known drops give, one a cable.
fn known_drops(patch: &Patch) -> Vec<String> {
    let module = |id| patch.nodes.iter().find(|n| n.id == id).map(|n| n.module_id.as_str());
    patch
        .connections
        .iter()
        .filter_map(|c| {
            let (_, _, why) = KNOWN_DROPS.iter().find(|(m, port, _)| module(c.from_node) == Some(*m) && c.from_port == *port)?;
            Some(format!("Skipped connection '{}' -> '{}': {}", c.from_port, c.to_port, why))
        })
        .collect()
}

#[test]
fn test_every_format_loads_whole_and_without_warnings() {
    for (version, json) in FIXTURES {
        let saved_as: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(saved_as["version"], version);

        let patch = patch_from_json(json).unwrap();
        let (saved, warnings) = reload(&patch, 1);
        let expected = known_drops(&patch);
        assert_eq!(warnings, expected, "v{version}");
        assert_eq!(saved.all_nodes().len(), patch.all_nodes().len(), "v{version}: modules");
        assert_eq!(
            cables(&saved.connections, &saved.groups),
            cables(&patch.connections, &patch.groups) - expected.len(),
            "v{version}: cables"
        );
        let mappings: Vec<_> = saved.midi_mappings.iter().map(|m| (m.cc_number, m.param_name.as_str())).collect();
        assert_eq!(mappings, [(74, "Cutoff"), (71, "Resonance"), (20, "Drive")], "v{version}");
    }
}

#[test]
fn test_every_saved_value_comes_back() {
    for (version, _) in FIXTURES {
        let patch = fixture(version);
        let (saved, _) = reload(&patch, 1);
        let mut changed = Vec::new();
        for (before, after) in patch.all_nodes().into_iter().zip(saved.all_nodes()) {
            assert_eq!(before.module_id, after.module_id, "v{version}");
            for p in &before.parameters {
                let Some(now) = after.parameters.iter().find(|q| q.name == p.name).map(|q| q.value.as_f32()) else {
                    changed.push(format!("{} {}: gone", before.module_id, p.name));
                    continue;
                };
                if (now - p.value.as_f32()).abs() > 1e-4 * now.abs().max(1.0) {
                    changed.push(format!("{} {}: {} became {}", before.module_id, p.name, p.value.as_f32(), now));
                }
            }
        }
        assert!(changed.is_empty(), "v{version}: {changed:#?}");
    }
}

#[test]
fn test_every_format_resaves_byte_identically() {
    for (version, _) in FIXTURES {
        let (first, _) = reload(&fixture(version), 1);
        let (second, _) = reload(&first, 1);
        assert_eq!(patch_to_json(&second).unwrap(), patch_to_json(&first).unwrap(), "v{version}");
    }
}

#[test]
fn test_v3_drive_moves_into_real_units() {
    let v3 = fixture(3);
    // Saved as 0.25 of the old 0-1 knob
    assert_eq!(param(only(&v3, "filter.svf"), "Drive"), 3.25);
    let drive = v3.midi_mappings.iter().find(|m| m.param_name == "Drive").unwrap();
    assert!((drive.min_value - 2.8).abs() < 1e-5 && (drive.max_value - 8.2).abs() < 1e-5, "{drive:?}");

    // Already in real units from v4 on
    for version in [4, 5, 6] {
        let patch = fixture(version);
        let drive = patch.midi_mappings.iter().find(|m| m.param_name == "Drive").unwrap();
        assert_eq!((drive.min_value, drive.max_value), (1.0, 10.0), "v{version}");
    }
}

#[test]
fn test_v3_and_v4_oscillators_keep_their_pitch() {
    for version in [3, 4] {
        let patch = fixture(version);
        let osc = only(&patch, "osc.sine");
        // Saved at 130.81 Hz: C3, an octave under C4
        assert_eq!(param(osc, "Octave"), -1.0, "v{version}");
        assert_eq!(param(osc, "Semitone"), 0.0, "v{version}");
        assert!(param(osc, "Fine").abs() < 0.01, "v{version}");
        // 0.25 Hz of FM at that pitch
        assert!((param(osc, "FM Depth") - 0.25 / 130.81279).abs() < 1e-6, "v{version}");
        assert!(osc.parameters.iter().all(|p| p.name != "Frequency"), "v{version}");
    }
}

#[test]
fn test_sequencers_saved_before_gate_mode_keep_fixed_gates() {
    use crate::modules::sequencer::GateMode;
    for version in [3, 4, 5] {
        let patch = fixture(version);
        assert_eq!(param(only(&patch, "seq.step"), "Gate Mode"), GateMode::Fixed as usize as f32, "v{version}");
    }
    // Saved with one, which is kept
    let (_, v6) = FIXTURES[3];
    let saved_as: Patch = serde_json::from_str(v6).unwrap();
    let mode = param(only(&saved_as, "seq.step"), "Gate Mode");
    assert_eq!(param(only(&fixture(6), "seq.step"), "Gate Mode"), mode);
}

#[test]
fn test_v6_keeps_groups_files_frames_and_notes() {
    let patch = fixture(6);
    let (saved, _) = reload(&patch, 1);

    let rig = &saved.groups[0];
    assert_eq!(rig.name, "Rig");
    let piano = &rig.groups[0];
    assert_eq!(piano.name, "Electric Piano");
    let jacks: Vec<_> = piano.inputs.iter().map(|j| (j.name.as_str(), j.signal.as_str())).collect();
    assert_eq!(jacks, [("Pitch", "Control"), ("Gate", "Gate"), ("Velocity", "Control")]);
    let before = &patch.groups[0];
    assert_eq!(cables(&rig.connections, &rig.groups), cables(&before.connections, &before.groups));
    let tine = piano.nodes.iter().find(|n| n.face_labels.contains_key("FM Depth")).unwrap();
    assert_eq!(tine.face_labels["FM Depth"], "Tine");

    assert_eq!(only(&saved, "source.sampler").file.as_deref(), Some("samples/fm-bell-c4.wav"));
    assert!(only(&saved, "fx.delay").bypassed);

    // The app keeps these beside the graph; they only need to read back
    assert_eq!(patch.frames.len(), 1);
    assert_eq!(patch.frames[0].color, "teal");
    assert!(patch.notes[0].text.contains("**v0.7.0**"));
}

#[test]
fn test_keyboards_and_midi_notes_open_as_one_voice_note_ins() {
    use crate::modules::note_in::{self, Source};
    for (version, json) in FIXTURES {
        let patch = fixture(version);
        assert!(!json.contains(note_in::ID) || version >= 5, "v{version}: Poly MIDI came in v5");
        let ids: Vec<&str> = patch.all_nodes().iter().map(|n| n.module_id.as_str()).collect();
        assert!(!ids.contains(&"input.keyboard") && !ids.contains(&"input.midi_note"), "v{version}");

        // Where the Keyboard and the MIDI Note were, as saved
        let saved: Patch = serde_json::from_str(json).unwrap();
        let id_of = |module| saved.nodes.iter().find(|n| n.module_id == module).unwrap().id;
        let node = |id| *patch.all_nodes().iter().find(|n| n.id == id).unwrap();
        let (keyboard, midi_note) = (node(id_of("input.keyboard")), node(id_of("input.midi_note")));
        for (node, source) in [(keyboard, Source::Keys), (midi_note, Source::Midi)] {
            assert_eq!(node.module_id, note_in::ID, "v{version}");
            assert_eq!(param(node, "Voices"), 1.0, "v{version}");
            assert_eq!(param(node, "Source"), source as i32 as f32, "v{version}");
        }
        assert_eq!(param(keyboard, "Key Velocity"), 0.5, "v{version}: the Keyboard's Velocity");
        assert_eq!(param(midi_note, "Retrigger"), 1.0, "v{version}");
    }
}

#[test]
fn test_v5_keeps_bypass() {
    assert!(only(&fixture(5), "fx.delay").bypassed);
}

/// Moves a value off its default: a toggle flips, a choice steps on, a
/// number halves (or becomes a quarter, from zero).
fn nudge(value: &mut SynthValueType) {
    match value {
        SynthValueType::Toggle { value, .. } => *value = !*value,
        SynthValueType::Select { value, options, .. } => *value = (*value + 1) % options.len().max(1),
        other => {
            let v = other.actual_value();
            other.set_actual_value(if v == 0.0 { 0.25 } else { v * 0.5 });
        }
    }
}

/// Writes this format's fixture, as the ones here were written by the code
/// of their day. When the format version goes up, run it on the last commit
/// of the outgoing format, then add the file to [`FIXTURES`]:
///
/// `FIXTURE_OUT=src/persistence/fixtures/patch_v<N>.json cargo test --lib write_a_fixture -- --ignored`
#[test]
#[ignore]
fn write_a_fixture_of_this_format() {
    let out = std::env::var("FIXTURE_OUT").expect("FIXTURE_OUT: where to write it");
    let name = format!("Every Module (v{PATCH_VERSION})");
    let mut patch = Patch::new(&name);
    for (i, t) in AllNodeTemplates.all_kinds().iter().enumerate() {
        let mut node = NodeData::new(i as u64 + 100, t.module_id(), ((i % 8) as f32 * 300.0, (i / 8) as f32 * 420.0));
        match t.module_id() {
            "source.sampler" => node.file = Some("samples/fm-bell-c4.wav".into()),
            "fx.delay" => node.bypassed = true,
            _ => {}
        }
        patch.nodes.push(node);
    }
    let id_of = |patch: &Patch, module: &str| patch.nodes.iter().find(|n| n.module_id == module).unwrap().id;
    let (filter, output, keyboard) = (id_of(&patch, "filter.svf"), id_of(&patch, "output.audio"), id_of(&patch, crate::modules::note_in::ID));
    patch.midi_mappings = vec![
        MidiMapping::new(74, 0, filter, 0, "Cutoff", 200.0, 8000.0),
        MidiMapping::new(71, 1, filter, 0, "Resonance", 0.0, 1.0),
        MidiMapping::new(20, 0, filter, 0, "Drive", 1.0, 10.0),
    ];

    // A Rig holding the library's Electric Piano: groups two deep, played
    // from the Note In and heard through the Audio Output
    let piano = patch_from_json(include_str!("../../library/electric-piano.json")).unwrap().groups.remove(0);
    let jack = |name: &str, signal: &str| JackData { name: name.into(), signal: signal.into() };
    let conn = |from, from_port: &str, to, to_port: &str| ConnectionData::new(from, from_port, to, to_port);
    let rig = 9000;
    patch.groups.push(GroupData {
        id: rig,
        name: "Rig".into(),
        position: (0.0, -600.0),
        inputs: vec![jack("Pitch", "Control"), jack("Gate", "Gate")],
        outputs: vec![jack("Out", "Audio")],
        inputs_position: (-320.0, 0.0),
        outputs_position: (640.0, 0.0),
        nodes: Vec::new(),
        connections: vec![
            conn(rig, "Pitch", piano.id, "Pitch"),
            conn(rig, "Gate", piano.id, "Gate"),
            conn(piano.id, "Out", rig, "Out"),
        ],
        groups: vec![piano],
    });
    patch.connections = vec![
        conn(keyboard, "Pitch", rig, "Pitch"),
        conn(keyboard, "Gate", rig, "Gate"),
        conn(rig, "Out", output, "Mono"),
    ];

    let mut staged = stage_patch(&patch).unwrap();
    assert!(staged.warnings.is_empty(), "{:?}", staged.warnings);
    let graph = &mut staged.graph;
    let modules: Vec<NodeId> = staged.nodes.iter().filter(|n| !n.nested).map(|n| n.graph_id).collect();
    for &node in &modules {
        for (_, input) in graph[node].inputs.clone() {
            nudge(&mut graph.inputs[input].value);
        }
    }

    // Each module's first two outputs into the next few modules, where the
    // signals match
    for (i, &from) in modules.iter().enumerate() {
        for (_, output) in graph[from].outputs.clone().into_iter().take(2) {
            let typ = graph.get_output(output).typ;
            'next: for &to in modules.iter().skip(i + 1).take(4) {
                for (_, input) in graph[to].inputs.clone() {
                    // What loading accepts: a cable the editor would make
                    let found = graph.get_input(input);
                    let fits = validate_connection(typ.0, found.typ.0).error_message().is_none();
                    if fits && port_mapping::is_connectable(found.kind) && graph.connection(input).is_none() {
                        graph.add_connection(output, input, 0);
                        break 'next;
                    }
                }
            }
        }
    }

    // As the editor loads: groups and then modules take IDs from one count
    let mut next = 1;
    renumber_groups(&mut staged.graph, || {
        next += 1;
        GroupId(next - 1)
    });
    let engine: HashMap<NodeId, EngineNodeId> = staged.nodes.iter().zip(next..).map(|(n, id)| (n.graph_id, id)).collect();
    let engine_id = |g| engine.get(&g).copied();
    let mappings = staged.remap_midi_mappings(engine_id);
    let position = |g| {
        let node = staged.nodes.iter().find(|n| n.graph_id == g).map(|n| n.position);
        node.or_else(|| staged.parts.iter().find(|p| p.graph_id == g).map(|p| p.position)).unwrap()
    };
    let mut saved = capture_patch(&name, &staged.graph, engine_id, position, &mappings);
    saved.frames = vec![FrameData {
        title: "Every module".into(),
        position: (-40.0, -80.0),
        size: (2440.0, 2200.0),
        color: "teal".into(),
    }];
    saved.notes = vec![NoteData {
        text: format!("Saved by Soba **{}**.", env!("CARGO_PKG_VERSION")),
        position: (-40.0, -760.0),
        width: 320.0,
    }];
    save_to_file(&saved, std::path::Path::new(&out)).unwrap();

    // Only cables this build would keep, so the fixture starts out clean
    let (_, warnings) = reload(&patch_from_json(&std::fs::read_to_string(&out).unwrap()).unwrap(), 1);
    assert!(warnings.is_empty(), "{warnings:#?}");
}
