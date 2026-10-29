//! A drum machine's own sequencer playing Soba (#469): a group of Pads
//! built from a kit profile, one Pad a part.
//!
//! The Kit (#443) plays a drum machine from Soba. This is the other way
//! round: the Digitakt's sequencer runs, and each of its tracks strikes a
//! Soba Drum, Sampler or envelope. **Pads for ▸** in the Library's
//! Controllers lists the kit profiles, and adds a group named for the
//! device holding a Pad for each of its parts (eight for the Digitakt,
//! sixteen for an SP-404MKII's bank), with a MIDI In patched into it:
//!
//! - Each Pad hears its part's note on its part's channel, and wears the
//!   part's name ("Track 1") on the group's face and on its own. Its jacks
//!   are named for it too: **Track 1** for the gate, **Track 1 Vel** for
//!   how hard.
//! - A part with a channel to itself (the Digitakt's tracks, the volca
//!   drum's parts) is heard on any note of that channel: a track sends each
//!   trig's own pitch, so a pitched trig is a different note, and a Pad
//!   waiting for the track's note would miss it.
//! - The MIDI In hears the device, where an input open is it.
//!
//! What the Pads hear is in their parameters and their names in their face
//! labels, so undo, copy, save and an unknown profile need nothing more.

use std::collections::BTreeMap;

use crate::hardware::lane::MIDDLE_C;
use crate::hardware::{Part, Profile};
use crate::modules::midi_in;

use super::pads_group::{group_of_named_pads, PadJacks};
use super::patch::{ConnectionData, NodeData, Patch};

/// The MIDI In's ID, and the group's.
const MIDI_IN: u64 = 1;
const GROUP: u64 = 2;

/// Where the MIDI In sits, left of the group.
const MIDI_IN_AT: (f32, f32) = (0.0, 0.0);
const GROUP_AT: (f32, f32) = (260.0, -40.0);

/// What the Pad for `part` of `profile` hears, and whether it hears every
/// note of its channel: where the part has a channel no other part shares,
/// or no note at all.
pub fn pad_for(profile: &Profile, part: &Part) -> PadJacks {
    let channel = part.channel.or_else(|| profile.channels.first().copied()).map_or(0, |channel| channel.clamp(1, 16));
    let own_channel = part.channel.is_some_and(|channel| profile.parts.iter().filter(|other| other.channel == Some(channel)).count() == 1);
    PadJacks {
        note: part.note.map_or(MIDDLE_C, |note| note.min(127)),
        channel,
        velocity: true,
        pressure: false,
        any: own_channel || part.note.is_none(),
    }
}

/// A patch to paste: a MIDI In hearing `device` (any, for `None`), patched
/// into a group named for `profile` holding a Pad for each of its parts.
pub fn pads_for(profile: &Profile, device: Option<&str>) -> Patch {
    let mut patch = Patch::new(&profile.name);
    let mut midi_in = NodeData::new(MIDI_IN, midi_in::ID, MIDI_IN_AT);
    if let Some(device) = device {
        midi_in.labels.insert(crate::graph::midi_device::DEVICE_LABEL.to_string(), device.to_string());
    }
    patch.nodes.push(midi_in);
    let pads: Vec<(PadJacks, Option<&str>)> = profile.parts.iter().map(|part| (pad_for(profile, part), Some(part.name.as_str()))).collect();
    let mut next = GROUP;
    let group = group_of_named_pads(GROUP, &profile.name, GROUP_AT, &pads, 0, &BTreeMap::new(), &mut next);
    patch.groups.push(group);
    patch.connections.push(ConnectionData::new(MIDI_IN, midi_in::PORT_NAME, GROUP, midi_in::PORT_NAME));
    patch.version = patch.required_version();
    patch
}

/// The input open that is `profile`'s device, by the hint in its profile,
/// as the Rig suggests a profile for a port.
pub fn device_for<'a>(profile: &Profile, open: &'a [String]) -> Option<&'a str> {
    let hint = profile.port.to_lowercase();
    if hint.is_empty() {
        return None;
    }
    open.iter().find(|name| name.to_lowercase().contains(&hint)).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::MidiMessage;
    use crate::engine::OfflineRenderer;
    use crate::hardware::{bundled, instrument};
    use crate::modules::pad;
    use crate::persistence::{compile_patch, patch_from_json, patch_to_json, ParameterValue};

    fn kit(name: &str) -> Profile {
        bundled().into_iter().find(|p| p.name == name).unwrap_or_else(|| panic!("{name} ships"))
    }

    fn param(node: &NodeData, name: &str) -> f32 {
        node.parameters.iter().find(|p| p.name == name).map_or(f32::NAN, |p| p.value.as_f32())
    }

    /// Each Pad's name, note, channel and Any Note, in order.
    fn listens(patch: &Patch) -> Vec<(String, u8, u8, bool)> {
        patch.groups[0]
            .nodes
            .iter()
            .map(|n| (n.face_labels["Press"].clone(), param(n, "Note") as u8, param(n, "Channel") as u8, param(n, "Any Note") == 1.0))
            .collect()
    }

    #[test]
    fn the_digitakt_is_eight_tracks_each_on_its_channel_on_any_note() {
        let patch = pads_for(&kit("Digitakt"), Some("Elektron Digitakt"));
        let expected: Vec<(String, u8, u8, bool)> = (0..8u8).map(|t| (format!("Track {}", t + 1), t, t + 1, true)).collect();
        assert_eq!(listens(&patch), expected);
        let group = &patch.groups[0];
        assert_eq!(group.name, "Digitakt");
        let jacks: Vec<&str> = group.outputs.iter().map(|j| j.name.as_str()).take(4).collect();
        assert_eq!(jacks, ["Track 1", "Track 1 Vel", "Track 2", "Track 2 Vel"]);
        assert!(group.nodes.iter().all(|n| n.module_id == pad::ID && n.pinned["Press"] == 1));
        // A MIDI In on the device, its cable into the group's MIDI jack
        let [midi_in] = patch.nodes.as_slice() else { panic!("one MIDI In") };
        assert_eq!(midi_in.labels["Device"], "Elektron Digitakt");
        assert!(patch.connections.iter().any(|c| c.from_node == midi_in.id && c.to_node == group.id && c.to_port == "MIDI"));
        assert!(compile_patch(&patch).unwrap().warnings.is_empty());
    }

    #[test]
    fn every_bundled_kit_gives_a_pad_a_part() {
        // The SP-404MKII's pads share channel 1, so each hears its note
        let sp = pads_for(&kit("SP-404MKII"), None);
        let heard = listens(&sp);
        assert_eq!(heard.len(), 16, "as wide as the device, not eight");
        assert_eq!(heard[0], ("Pad 1".to_string(), 48, 1, false));
        assert_eq!(heard[12], ("Pad 13".to_string(), 36, 1, false));
        assert!(!sp.nodes[0].labels.contains_key("Device"), "no device open: any");
        // The volca drum's parts have only a channel: any note, middle C
        let volca = listens(&pads_for(&kit("volca drum"), None));
        assert_eq!(volca[3], ("Part 4".to_string(), 60, 4, true));

        for profile in bundled().iter().filter(|p| instrument::is_kit(p)) {
            let patch = pads_for(profile, None);
            assert_eq!(patch.groups[0].nodes.len(), profile.parts.len(), "{}", profile.name);
            let names: std::collections::BTreeSet<&str> = patch.groups[0].outputs.iter().map(|j| j.name.as_str()).collect();
            assert_eq!(names.len(), patch.groups[0].outputs.len(), "{}: every jack its own name", profile.name);
            assert!(compile_patch(&patch).unwrap().warnings.is_empty(), "{}", profile.name);
        }
    }

    #[test]
    fn saved_and_opened_it_keeps_the_names_and_listens() {
        let patch = pads_for(&kit("Digitakt"), Some("Digitakt"));
        let again = patch_from_json(&patch_to_json(&patch).unwrap()).unwrap();
        assert_eq!(listens(&again), listens(&patch));
    }

    #[test]
    fn a_pitched_trig_strikes_its_tracks_pad() {
        // Track 3's trigs on channel 3: its own note, then one p-locked up
        // a fifth, and Track 4's note, which isn't its
        // (each opening a VCA on a tone, Track 3's on the left and Track
        // 4's on the right)
        let mut patch = pads_for(&kit("Digitakt"), None);
        let node = |id: u64, module_id: &str, params: &[(&str, ParameterValue)]| {
            let mut node = NodeData::new(id, module_id, (900.0, 0.0));
            node.parameters = params.iter().map(|(name, value)| crate::persistence::NamedParameter::new(*name, value.clone())).collect();
            node
        };
        patch.nodes.push(node(90, "output.audio", &[("Volume", ParameterValue::Number(1.0)), ("Limiter", ParameterValue::Toggle(false))]));
        patch.nodes.push(node(91, "osc.sine", &[]));
        let wire = ConnectionData::new;
        for (vca, track, side) in [(92, "Track 3", "Left"), (93, "Track 4", "Right")] {
            patch.nodes.push(node(vca, "util.vca", &[]));
            patch.connections.extend([wire(91, "Out", vca, "In"), wire(GROUP, track, vca, "CV"), wire(vca, "Out", 90, side)]);
        }
        let sample_rate = 48_000.0;
        let (mut renderer, compiled) = OfflineRenderer::from_patch(&patch, sample_rate, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        let at = |seconds: f32| (seconds * sample_rate) as u64;
        for (time, channel, note) in [(0.1, 2, 2), (0.3, 2, 9), (0.5, 3, 3)] {
            renderer.queue_midi(at(time), channel, MidiMessage::NoteOn { note, velocity: 100 });
            renderer.queue_midi(at(time + 0.05), channel, MidiMessage::NoteOff { note, velocity: 0 });
        }
        let audio = renderer.render_seconds(0.7);
        // Open while its trig is held: from its note on to its note off
        let open = |side: &[f32], from: f32| crate::dsp::analysis::peak(&side[at(from + 0.01) as usize..at(from + 0.04) as usize]) > 0.1;
        let heard = |side: &[f32]| [0.1, 0.2, 0.3, 0.4, 0.5].map(|from| open(side, from));
        assert_eq!(heard(&audio.left), [true, false, true, false, false], "Track 3 hears its trig and the pitched one");
        assert_eq!(heard(&audio.right), [false, false, false, false, true], "Track 4 hears its own");
    }
}
