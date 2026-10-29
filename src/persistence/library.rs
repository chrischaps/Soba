//! The Library: ready-made groups that ship with the app.
//!
//! Each is a patch holding one group, in `library/` at the repository root,
//! the same kind of file Save to My Modules writes. They're compiled into
//! the binary, so the Library is there wherever the app runs, the browser
//! included. To add one, build it as a group, save it to My Modules, copy the
//! file into `library/` and list it here.
//!
//! The voices are built only from polyphonic modules, so one voice plays
//! chords from Note In as readily as single notes, and
//! they're levelled to sit near one another: one note peaks around -12 dBFS.

use super::{patch_from_json, Patch, PatchError};

/// Where a group sits in the Library's menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Played from Pitch, Gate and Velocity, out through one Out.
    Voices,
    Drums,
    /// Make notes or sound on their own, with nothing plugged in.
    Generators,
    /// Take audio in and give it back changed.
    Effects,
    /// A controller's pads and knobs, as gates and CV (#480).
    Controllers,
}

impl Section {
    pub const ALL: [Section; 5] = [Section::Voices, Section::Drums, Section::Generators, Section::Effects, Section::Controllers];

    pub fn name(self) -> &'static str {
        match self {
            Section::Voices => "Voices",
            Section::Drums => "Drums",
            Section::Generators => "Generators",
            Section::Effects => "Effects",
            Section::Controllers => "Controllers",
        }
    }
}

/// A group that ships with the app.
#[derive(Debug, PartialEq)]
pub struct LibraryGroup {
    /// The group's name, as it appears in menus and on its node.
    pub name: &'static str,
    /// File name in `library/`.
    pub file_name: &'static str,
    pub section: Section,
    /// What it is and how to play it, in a line.
    pub description: &'static str,
    json: &'static str,
}

impl LibraryGroup {
    /// The group, as a patch to paste.
    pub fn patch(&self) -> Result<Patch, PatchError> {
        patch_from_json(self.json)
    }
}

/// Every group in the Library, in menu order.
pub const LIBRARY: &[LibraryGroup] = &[
    LibraryGroup {
        name: "Subtractive Voice",
        file_name: "subtractive-voice.json",
        section: Section::Voices,
        description: "The classic: a saw through a ladder filter, one envelope opening the filter and one shaping the level",
        json: include_str!("../../library/subtractive-voice.json"),
    },
    LibraryGroup {
        name: "FM Voice",
        file_name: "fm-voice.json",
        section: Section::Voices,
        description: "Two-operator FM at 1:1: an electric-piano bark that mellows as it rings. Octave and Semitone set the ratio",
        json: include_str!("../../library/fm-voice.json"),
    },
    LibraryGroup {
        name: "FM Bell",
        file_name: "fm-bell.json",
        section: Section::Voices,
        description: "Two-operator FM at 1:3.5: clangorous partials that fade to a pure tone over a long ring",
        json: include_str!("../../library/fm-bell.json"),
    },
    LibraryGroup {
        name: "Electric Piano",
        file_name: "electric-piano.json",
        section: Section::Voices,
        description: "A Rhodes-style tine: FM at 1:1 that barks the harder you play and mellows as it rings, under a slow tremolo",
        json: include_str!("../../library/electric-piano.json"),
    },
    LibraryGroup {
        name: "Supersaw Pad",
        file_name: "supersaw-pad.json",
        section: Section::Voices,
        description: "Seven detuned saws, a slow swell and a filter that breathes. Hold chords from Note In",
        json: include_str!("../../library/supersaw-pad.json"),
    },
    LibraryGroup {
        name: "String Machine",
        file_name: "string-machine.json",
        section: Section::Voices,
        description: "A Solina-style ensemble: detuned saws at 8' and 4', thinned and swelling, through a three-voice chorus",
        json: include_str!("../../library/string-machine.json"),
    },
    LibraryGroup {
        name: "Mallet",
        file_name: "mallet.json",
        section: Section::Voices,
        description: "A short strike rings a resonant filter tuned to the note, like a marimba bar",
        json: include_str!("../../library/mallet.json"),
    },
    LibraryGroup {
        name: "Acid Bass",
        file_name: "acid-bass.json",
        section: Section::Voices,
        description: "A resonant, driven ladder snapped open on every note, two octaves down. Velocity accents; add Glide on the Keyboard",
        json: include_str!("../../library/acid-bass.json"),
    },
    LibraryGroup {
        name: "Reese Bass",
        file_name: "reese-bass.json",
        section: Section::Voices,
        description: "Three detuned saws beating against each other, two octaves down, with a slow LFO in the filter",
        json: include_str!("../../library/reese-bass.json"),
    },
    LibraryGroup {
        name: "Drum Kit",
        file_name: "drum-kit.json",
        section: Section::Drums,
        description: "Kick, snare and hats on a stereo mixer, the closed hat choking the open one. Patch a sequencer's gates into each",
        json: include_str!("../../library/drum-kit.json"),
    },
    LibraryGroup {
        name: "Random Melody",
        file_name: "random-melody.json",
        section: Section::Generators,
        description: "Plays itself: a wandering voltage, sampled on the beat and kept in a scale, A minor pentatonic until you change Root and Scale",
        json: include_str!("../../library/random-melody.json"),
    },
    LibraryGroup {
        name: "Wind",
        file_name: "wind.json",
        section: Section::Generators,
        description: "Plays itself: pink noise through a wandering band-pass, whistling higher as each gust rises",
        json: include_str!("../../library/wind.json"),
    },
    LibraryGroup {
        name: "Stereo Space",
        file_name: "stereo-space.json",
        section: Section::Effects,
        description: "Chorus, ping-pong tape echo and a room, in the order a pedalboard runs them. Mono in plays on both sides",
        json: include_str!("../../library/stereo-space.json"),
    },
    LibraryGroup {
        name: "Pump",
        file_name: "pump.json",
        section: Section::Effects,
        description: "Each trigger ducks the sound and lets it swell back, like a compressor keyed from the kick",
        json: include_str!("../../library/pump.json"),
    },
    LibraryGroup {
        name: "Auto-Pan",
        file_name: "auto-pan.json",
        section: Section::Effects,
        description: "An LFO sweeps a mono sound from side to side. Tempo locks the sweep to the Clock",
        json: include_str!("../../library/auto-pan.json"),
    },
    LibraryGroup {
        name: "Slicer",
        file_name: "slicer.json",
        section: Section::Effects,
        description: "The Slicer pedal rebuilt from stock modules, to open and see how it works: a synced LFO clocks a row of steps that open a VCA and a step filter",
        json: include_str!("../../library/slicer.json"),
    },
    LibraryGroup {
        name: "Pads ×8",
        file_name: "pads-x8.json",
        section: Section::Controllers,
        description: "Eight Pads on the General MIDI drum notes, 36 to 43 on channel 10, as most pad controllers send them: Gate and Vel for each, in the order a Trigger Sequencer's lanes come",
        json: include_str!("../../library/pads-x8.json"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{amp_to_db, peak, rms, Spectrum};
    use crate::dsp::MidiMessage;
    use crate::engine::OfflineRenderer;
    use crate::graph::groups::{self, GroupIndex};
    use crate::persistence::{capture_patch, compile_patch, stage_patch, ConnectionData, GroupData, JackData, NodeData, ParameterValue};

    #[test]
    fn every_library_file_is_listed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("library");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".json"))
            .collect();
        files.sort();
        let mut listed: Vec<String> = LIBRARY.iter().map(|g| g.file_name.to_string()).collect();
        listed.sort();
        assert_eq!(files, listed, "library/ and LIBRARY must list the same files");
    }

    #[test]
    fn each_is_one_group_that_compiles_cleanly() {
        for entry in LIBRARY {
            let patch = entry.patch().unwrap_or_else(|e| panic!("{}: {e}", entry.name));
            assert_eq!(patch.name, entry.name);
            assert!(patch.nodes.is_empty() && patch.connections.is_empty(), "{}: modules outside the group", entry.name);
            let [group] = patch.groups.as_slice() else { panic!("{}: not one group", entry.name) };
            assert_eq!(group.name, entry.name);
            assert_eq!(patch.version, patch.required_version(), "{}", entry.name);
            let compiled = compile_patch(&patch).unwrap();
            assert!(compiled.warnings.is_empty(), "{}: {:?}", entry.name, compiled.warnings);

            // Every jack is wired inside
            for jack in &group.inputs {
                assert!(
                    group.connections.iter().any(|c| c.from_node == group.id && c.from_port == jack.name),
                    "{}: input jack {} goes nowhere",
                    entry.name,
                    jack.name
                );
            }
            for jack in &group.outputs {
                assert!(
                    group.connections.iter().any(|c| c.to_node == group.id && c.to_port == jack.name),
                    "{}: output jack {} has nothing behind it",
                    entry.name,
                    jack.name
                );
            }
            // Every pin shows on the face, knob, dropdown or toggle. A
            // controller's pads wait to be pinned as pads (#482)
            if entry.section != Section::Controllers {
                assert!(group.nodes.iter().any(|n| !n.pinned.is_empty()), "{}: nothing on its face", entry.name);
            }
            let staged = stage_patch(&patch).unwrap();
            let graph = &staged.graph;
            let id = graph.nodes.values().find_map(|n| n.user_data.kind.group()).unwrap();
            let shown: Vec<_> = groups::face_controls(graph, |_| None, &GroupIndex::of(graph), id)
                .into_iter()
                .map(|control| (control.node, control.param))
                .collect();
            for node in &group.nodes {
                let graph_id = staged.nodes.iter().find(|n| n.patch_id == node.id).unwrap().graph_id;
                for name in node.pinned.keys() {
                    assert!(shown.contains(&(graph_id, name.clone())), "{}: {name} is pinned but not on the face", entry.name);
                }
                // A face's names are for controls that are on it
                for (name, label) in &node.face_labels {
                    assert!(node.pinned.contains_key(name), "{}: {name} is named {label} but not pinned", entry.name);
                    assert!(!label.trim().is_empty(), "{}: {name} has a blank name", entry.name);
                }
            }
        }
    }

    #[test]
    fn voices_are_polyphonic() {
        // Note In's chords pass through a voice only if every module in it is
        let polyphonic = ["osc.sine", "source.noise", "filter.svf", "filter.ladder", "mod.adsr", "util.vca", "util.attenuverter", "util.mix"];
        for entry in LIBRARY.iter().filter(|g| g.section == Section::Voices) {
            let patch = entry.patch().unwrap();
            for node in patch.all_nodes() {
                // A mono modulator shared by every voice is fine, and so is
                // an effect hearing the voices summed, as a string
                // machine's ensemble hears the whole chord
                if ["mod.lfo", "fx.chorus"].contains(&node.module_id.as_str()) {
                    continue;
                }
                assert!(polyphonic.contains(&node.module_id.as_str()), "{}: {} isn't polyphonic", entry.name, node.module_id);
            }
            let jacks: Vec<_> = patch.groups[0].inputs.iter().map(|j| j.name.as_str()).collect();
            assert_eq!(jacks, ["Pitch", "Gate", "Velocity"], "{}", entry.name);
        }
    }

    #[test]
    fn files_are_as_the_app_saves_them() {
        // Every parameter written, in the node's own order, as Save to My
        // Modules would: so a Library group saved again comes out unchanged
        for entry in LIBRARY {
            let patch = entry.patch().unwrap();
            let staged = stage_patch(&patch).unwrap();
            let ids: std::collections::BTreeMap<_, _> = staged.nodes.iter().map(|n| (n.graph_id, n.patch_id)).collect();
            let positions: std::collections::BTreeMap<_, _> = staged
                .nodes
                .iter()
                .map(|n| (n.graph_id, n.position))
                .chain(staged.parts.iter().map(|p| (p.graph_id, p.position)))
                .collect();
            let saved = capture_patch(
                entry.name,
                &staged.graph,
                |n| ids.get(&n).copied(),
                |n| positions.get(&n).copied().unwrap_or_default(),
                &[],
            );
            assert_eq!(saved.groups[0].nodes, patch.groups[0].nodes, "{}", entry.name);
        }
    }

    /// A patch that plays `entry`: Note In into a voice's jacks, a clock
    /// and dividers into a kit's, a plucked saw into an effect, and a plain
    /// voice to play a generator's notes. Its sound goes to the output.
    fn played(entry: &LibraryGroup) -> Patch {
        let mut patch = entry.patch().unwrap();
        let group = patch.groups[0].clone();
        let g = group.id;
        let has = |jacks: &[JackData], name: &str| jacks.iter().any(|j| j.name == name);
        let node = |id, module_id: &str, params: &[(&str, ParameterValue)]| {
            let mut node = NodeData::new(id, module_id, (0.0, 0.0));
            node.parameters = params.iter().map(|(n, v)| crate::persistence::NamedParameter::new(*n, v.clone())).collect();
            node
        };
        let wire = |a, ap: &str, b, bp: &str| ConnectionData::new(a, ap, b, bp);
        let (number, select) = (ParameterValue::Number, ParameterValue::Select);

        let mut nodes = vec![node(9000, "output.audio", &[("Volume", number(1.0)), ("Limiter", ParameterValue::Toggle(false))])];
        let mut wires = Vec::new();
        if has(&group.inputs, "Pitch") {
            nodes.push(node(9001, "input.poly_midi", &[("Voices", number(4.0))]));
            wires.extend(["Pitch", "Gate", "Velocity"].map(|j| wire(9001, j, g, j)));
        }
        if has(&group.inputs, "Kick") {
            nodes.push(node(9002, "util.clock", &[("Division", select(3))]));
            nodes.push(node(9003, "util.divider", &[("Divide", number(2.0))]));
            nodes.push(node(9004, "util.divider", &[("Divide", number(4.0)), ("Offset", number(2.0))]));
            wires.extend([
                wire(9002, "Gate", 9003, "Clock"),
                wire(9002, "Gate", 9004, "Clock"),
                wire(9003, "Trig", g, "Kick"),
                wire(9004, "Trig", g, "Snare"),
                wire(9002, "Gate", g, "Hat"),
            ]);
        }
        let audio_in = ["In", "In L"].into_iter().find(|j| has(&group.inputs, j));
        if let Some(jack) = audio_in {
            nodes.push(node(9010, "input.poly_midi", &[("Voices", number(4.0))]));
            nodes.push(node(9011, "osc.sine", &[("Waveform", select(1))]));
            nodes.push(node(9012, "mod.adsr", &[("Decay", number(0.25)), ("Sustain", number(0.2))]));
            nodes.push(node(9013, "util.vca", &[("Level", number(0.4))]));
            wires.extend([
                wire(9010, "Pitch", 9011, "V/Oct"),
                wire(9010, "Gate", 9012, "Gate"),
                wire(9011, "Out", 9013, "In"),
                wire(9012, "Out", 9013, "CV"),
                wire(9013, "Out", g, jack),
            ]);
        }
        if has(&group.outputs, "Gate 1") {
            // Pads into the Library's Drum Kit, as GM drum notes play one
            let kit = drum_kit(9100);
            let k = kit.id;
            patch.groups.push(kit);
            wires.extend([
                wire(g, "Gate 1", k, "Kick"),
                wire(g, "Gate 3", k, "Snare"),
                wire(g, "Gate 7", k, "Hat"),
                wire(g, "Vel 1", k, "Accent"),
                wire(k, "Out L", 9000, "Left"),
                wire(k, "Out R", 9000, "Right"),
            ]);
        }
        if has(&group.inputs, "Trig") {
            nodes.push(node(9020, "util.clock", &[]));
            wires.push(wire(9020, "Gate", g, "Trig"));
        }
        if has(&group.outputs, "Pitch") {
            nodes.push(node(9030, "osc.sine", &[("Waveform", select(3))]));
            nodes.push(node(9031, "mod.adsr", &[("Sustain", number(0.3))]));
            nodes.push(node(9032, "util.vca", &[("Level", number(0.4))]));
            wires.extend([
                wire(g, "Pitch", 9030, "V/Oct"),
                wire(g, "Gate", 9031, "Gate"),
                wire(9030, "Out", 9032, "In"),
                wire(9031, "Out", 9032, "CV"),
                wire(9032, "Out", 9000, "Mono"),
            ]);
        }
        if has(&group.outputs, "Out") {
            wires.push(wire(g, "Out", 9000, "Mono"));
        }
        if has(&group.outputs, "Out L") {
            wires.extend([wire(g, "Out L", 9000, "Left"), wire(g, "Out R", 9000, "Right")]);
        }
        patch.nodes = nodes;
        patch.connections = wires;
        patch
    }

    /// The Library's Drum Kit, its IDs from `from` up so it sits beside
    /// another Library group.
    fn drum_kit(from: u64) -> GroupData {
        let mut kit = LIBRARY.iter().find(|g| g.name == "Drum Kit").unwrap().patch().unwrap().groups.remove(0);
        let ids: Vec<u64> = std::iter::once(kit.id).chain(kit.nodes.iter().map(|n| n.id)).collect();
        let new = |id: u64| ids.iter().position(|&old| old == id).map_or(id, |at| from + at as u64);
        kit.id = new(kit.id);
        for node in &mut kit.nodes {
            node.id = new(node.id);
        }
        for cable in &mut kit.connections {
            (cable.from_node, cable.to_node) = (new(cable.from_node), new(cable.to_node));
        }
        kit
    }

    #[test]
    fn pads_played_into_the_drum_kit_play_a_kit() {
        // The acceptance (#480): the GM drum beat on Pads ×8, into the Drum
        // Kit, strikes the kick, snare and hat, each as hard as its pad
        let entry = LIBRARY.iter().find(|g| g.name == "Pads ×8").unwrap();
        let patch = played(entry);
        let sample_rate = 48_000.0;
        let (mut renderer, compiled) = OfflineRenderer::from_patch(&patch, sample_rate, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        let audio = renderer.render_audition(&patch, 2.0);
        let window = |at: f32| {
            let start = (at * sample_rate) as usize;
            peak(&audio.left[start..start + 2400])
        };
        let sixteenth = 60.0 / crate::engine::offline::PAD_BEAT_BPM / 4.0;
        // The kick and hat on the downbeat, the snare on beat 2, and
        // nothing between the hat on the 2nd sixteenth and the snare
        assert!(window(0.0) > 0.1, "the downbeat: {}", window(0.0));
        assert!(window(4.0 * sixteenth) > 0.1, "the snare: {}", window(4.0 * sixteenth));
        let gap = window(3.0 * sixteenth);
        assert!(gap < window(4.0 * sixteenth), "quieter between hits");
    }

    #[test]
    fn each_makes_sound_at_a_matching_level() {
        let sample_rate = 48_000.0;
        for entry in LIBRARY {
            let patch = played(entry);
            let (mut renderer, compiled) = OfflineRenderer::from_patch(&patch, sample_rate, 256).unwrap();
            assert!(compiled.warnings.is_empty(), "{}: {:?}", entry.name, compiled.warnings);
            let audio = renderer.render_audition(&patch, 5.0);
            for (side, channel) in [("left", &audio.left), ("right", &audio.right)] {
                assert!(channel.iter().all(|s| s.is_finite()), "{}: {side} isn't finite", entry.name);
                let (peak_db, rms_db) = (amp_to_db(peak(channel)), amp_to_db(rms(channel)));
                assert!(rms_db > -35.0, "{}: {side} too quiet, {rms_db:.1} dBFS RMS", entry.name);
                assert!(peak_db < 0.0, "{}: {side} clips, {peak_db:.1} dBFS", entry.name);
            }
            // A voice's first note, alone: near its neighbours', so swapping
            // one voice for another doesn't jump in level
            if entry.section == Section::Voices {
                let first_note = &audio.left[..(0.45 * sample_rate) as usize];
                let peak_db = amp_to_db(peak(first_note));
                assert!((-16.0..=-9.0).contains(&peak_db), "{}: one note peaks at {peak_db:.1} dBFS", entry.name);
            }
        }
    }

    /// Sixteen sixteenths of a steady tone at -12 dBFS through `slicer` (the
    /// group's node or the module's), free-running at 120 BPM: each step's
    /// RMS.
    fn step_levels(mut patch: Patch, slicer: u64) -> Vec<f32> {
        let tone = NodeData::new(9100, "osc.sine", (0.0, 0.0));
        let mut quieter = NodeData::new(9102, "util.vca", (0.0, 0.0));
        quieter.parameters = vec![crate::persistence::NamedParameter::new("Level", ParameterValue::Number(0.25))];
        let mut out = NodeData::new(9101, "output.audio", (0.0, 0.0));
        out.parameters = vec![
            crate::persistence::NamedParameter::new("Volume", ParameterValue::Number(1.0)),
            crate::persistence::NamedParameter::new("Limiter", ParameterValue::Toggle(false)),
        ];
        let (in_port, out_port) = if patch.groups.is_empty() { ("In L", "Out L") } else { ("In", "Out") };
        patch.nodes.extend([tone, quieter, out]);
        patch.connections.extend([
            ConnectionData::new(9100, "Out", 9102, "In"),
            ConnectionData::new(9102, "Out", slicer, in_port),
            ConnectionData::new(slicer, out_port, 9101, "Mono"),
        ]);
        let (mut renderer, compiled) = OfflineRenderer::from_patch(&patch, 48_000.0, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        let audio = renderer.render(16 * 6000);
        audio.left.chunks(6000).map(rms).collect()
    }

    #[test]
    fn slicer_group_slices_as_the_slicer_does() {
        // Both at their defaults play the Trance row: each step as loud, and
        // the rests as quiet, in the group as in the module it rebuilds
        let entry = LIBRARY.iter().find(|g| g.name == "Slicer").unwrap();
        let group = entry.patch().unwrap();
        let id = group.groups[0].id;
        let mut module = Patch::new("Slicer module");
        module.nodes.push(NodeData::new(1, "fx.slicer", (0.0, 0.0)));
        let (built, original) = (step_levels(group, id), step_levels(module, 1));
        for (step, (b, o)) in built.iter().zip(&original).enumerate() {
            let (b_db, o_db) = (amp_to_db(*b), amp_to_db(*o));
            if o_db < -60.0 {
                assert!(b_db < -60.0, "step {}: a rest lets {b_db:.1} dB through", step + 1);
            } else {
                // The Sequencer times its first gate before it has measured
                // a step, so the first slice comes out a little short
                let within = if step == 0 { 1.5 } else { 0.5 };
                assert!((b_db - o_db).abs() < within, "step {}: group {b_db:.1} dB, module {o_db:.1} dB", step + 1);
            }
        }
    }

    /// The first `seconds` of one middle C played at `velocity`, alone.
    fn one_note(entry: &LibraryGroup, velocity: u8, seconds: f32) -> Vec<f32> {
        let sample_rate = 48_000.0;
        let patch = played(entry);
        let (mut renderer, _) = OfflineRenderer::from_patch(&patch, sample_rate, 256).unwrap();
        renderer.queue_midi(0, 0, MidiMessage::NoteOn { note: 60, velocity });
        renderer.render((seconds * sample_rate) as usize).left
    }

    /// Where a sound's power sits on average, in Hz: higher is brighter,
    /// whatever its level. FM at 1:1 throws a sideband onto 0 Hz, so
    /// everything below 40 Hz is left out.
    fn centroid(samples: &[f32]) -> f64 {
        let spectrum = Spectrum::of(samples, 48_000.0);
        let audible = spectrum.magnitudes.iter().enumerate().map(|(bin, m)| (bin as f64 * spectrum.bin_hz, m * m)).filter(|&(hz, _)| hz >= 40.0);
        let weighted: f64 = audible.clone().map(|(hz, p)| hz * p).sum();
        weighted / audible.map(|(_, p)| p).sum::<f64>()
    }

    #[test]
    fn slicer_group_steps_the_filter() {
        // With Filter turned down, each slice strikes the lowpass open and it
        // closes through the step: the slice starts brighter than it ends
        let entry = LIBRARY.iter().find(|g| g.name == "Slicer").unwrap();
        let mut patch = entry.patch().unwrap();
        let id = patch.groups[0].id;
        let filter = patch.groups[0].nodes.iter_mut().find(|n| n.module_id == "filter.svf").unwrap();
        filter.parameters.iter_mut().find(|p| p.name == "Cutoff").unwrap().value = ParameterValue::Number(150.0);
        let mut saw = NodeData::new(9100, "osc.sine", (0.0, 0.0));
        saw.parameters = vec![crate::persistence::NamedParameter::new("Waveform", ParameterValue::Select(1))];
        let mut quieter = NodeData::new(9102, "util.vca", (0.0, 0.0));
        quieter.parameters = vec![crate::persistence::NamedParameter::new("Level", ParameterValue::Number(0.25))];
        patch.nodes.extend([saw, quieter, NodeData::new(9101, "output.audio", (0.0, 0.0))]);
        patch.connections.extend([
            ConnectionData::new(9100, "Out", 9102, "In"),
            ConnectionData::new(9102, "Out", id, "In"),
            ConnectionData::new(id, "Out", 9101, "Mono"),
        ]);
        let (mut renderer, _) = OfflineRenderer::from_patch(&patch, 48_000.0, 256).unwrap();
        // Step 4, an accent: the sixteenth from 18000 samples, open for 55% of it
        let left = renderer.render(4 * 6000).left;
        let (early, late) = (centroid(&left[18_100..19_300]), centroid(&left[20_100..21_300]));
        assert!(early > 1.5 * late, "the slice centres on {early:.0} Hz as it opens, {late:.0} Hz later");
    }

    #[test]
    fn electric_piano_brightens_with_velocity() {
        // As on a Rhodes, a harder note barks: it changes the tone, not
        // only the level. The bark is in the attack, so hear its first 50 ms
        let piano = LIBRARY.iter().find(|g| g.name == "Electric Piano").unwrap();
        let attack = |velocity| centroid(&one_note(piano, velocity, 0.05));
        let (soft, hard) = (attack(40), attack(120));
        assert!(hard > 1.3 * soft, "velocity 40 centres on {soft:.0} Hz, 120 on {hard:.0} Hz");
    }
}
