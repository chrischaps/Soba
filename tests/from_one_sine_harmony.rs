//! From One Sine's harmony: one Chord Sequencer plays the sixteen-bar
//! progression that four Step Sequencers used to, a root and three pad
//! voices. These tests play the patch's own Clock, Divider and Chord
//! Sequencer, block by block as the engine does, and check every bar's
//! notes against the voicings the four sequencers sang.

use soba::dsp::{DspModule, ProcessContext, SignalBuffer, SignalType};
use soba::modules::{ChordSequencer, Clock, ClockDivider};
use soba::persistence::{patch_from_json, NodeData, Patch};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;
/// A bar at 112 BPM.
const BAR_SECONDS: f32 = 4.0 * 60.0 / 112.0;

/// The progression as the four Step Sequencers played it, a bar a step: the
/// root (which the bass saw plays an octave down, and which transposes the
/// arpeggio and the bass line), then the pad's three upper voices.
const VOICINGS: [(i32, [i32; 3]); 16] = [
    (62, [65, 69, 76]), // Dm(add9): F A E
    (58, [65, 69, 74]), // Bbmaj7: F A D
    (65, [64, 69, 72]), // Fmaj7: E A C
    (60, [64, 67, 74]), // Cadd9: E G D
    (62, [65, 69, 76]),
    (58, [65, 69, 74]),
    (65, [64, 69, 72]),
    (60, [64, 67, 74]),
    (55, [65, 70, 74]), // Gm7: F Bb D
    (58, [65, 72, 74]), // Bbadd9: F C D
    (62, [65, 69, 74]), // Dm: F A D
    (60, [64, 67, 72]), // C: E G C
    (55, [65, 70, 74]), // Gm7
    (58, [65, 69, 74]), // Bbmaj7
    (60, [65, 67, 72]), // Csus4: F G C
    (60, [64, 67, 72]), // C
];

fn from_one_sine() -> Patch {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("patches/from-one-sine.json");
    patch_from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A module's parameters as the patch sets them, in parameter order.
fn params_of(module: &dyn DspModule, node: &NodeData) -> Vec<f32> {
    module
        .parameters()
        .iter()
        .map(|p| node.parameters.iter().find(|n| n.name == p.name).map_or(p.default, |n| n.value.as_f32()))
        .collect()
}

fn param(node: &NodeData, name: &str) -> f32 {
    node.parameters.iter().find(|p| p.name == name).unwrap_or_else(|| panic!("{} has {name}", node.module_id)).value.as_f32()
}

fn rises(signal: &[f32]) -> Vec<usize> {
    (0..signal.len()).filter(|&t| signal[t] > 0.5 && (t == 0 || signal[t - 1] <= 0.5)).collect()
}

/// The MIDI note a V/Oct pitch is.
fn note(voct: f32) -> i32 {
    (voct * 12.0).round() as i32 + 60
}

fn node(patch: &Patch, id: u64) -> &NodeData {
    patch.nodes.iter().find(|n| n.id == id).unwrap()
}

/// The Chord Sequencer, and the Divider that clocks it.
fn harmony(patch: &Patch) -> (&NodeData, &NodeData) {
    let chords = patch.nodes.iter().filter(|n| n.module_id == "seq.chord").collect::<Vec<_>>();
    assert_eq!(chords.len(), 1, "one Chord Sequencer plays the harmony");
    let clock = patch.connections.iter().find(|c| c.to_node == chords[0].id && c.to_port == "Clock").expect("clocked");
    (chords[0], node(patch, clock.from_node))
}

/// Plays `bars` bars and returns, for each, the root and the pad's notes the
/// Chord Sequencer puts out as the bar starts.
fn play(bars: usize) -> Vec<(i32, Vec<i32>)> {
    let patch = from_one_sine();
    let (chord_node, divider_node) = harmony(&patch);
    assert_eq!(param(divider_node, "Divide"), 16.0, "a chord a bar");

    let mut clock = Clock::new();
    let mut divider = ClockDivider::new();
    let mut seq = ChordSequencer::new();
    let clock_params = params_of(&clock, patch.nodes.iter().find(|n| n.module_id == "util.clock").unwrap());
    let divider_params = params_of(&divider, divider_node);
    let seq_params = params_of(&seq, chord_node);
    for module in [&mut clock as &mut dyn DspModule, &mut divider, &mut seq] {
        module.prepare(RATE, BLOCK);
    }

    let total = (bars as f32 * BAR_SECONDS * RATE) as usize + BLOCK;
    let mut clock_out: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::gate(BLOCK)).collect();
    let mut divider_out: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::gate(BLOCK)).collect();
    let mut seq_out: Vec<SignalBuffer> = (0..6).map(|_| SignalBuffer::polyphonic(BLOCK, SignalType::Control)).collect();
    let idle = SignalBuffer::gate(BLOCK);
    let mut run = SignalBuffer::gate(BLOCK);
    run.samples.fill(1.0);
    let (mut clocks, mut root, mut pad) = (Vec::new(), Vec::new(), Vec::new());
    let ctx = ProcessContext::new(RATE, BLOCK);
    for _ in 0..total.div_ceil(BLOCK) {
        clock.process(&[], &mut clock_out, &clock_params, &ctx);
        let ctx = ProcessContext::with_transport(RATE, BLOCK, clock.transport(&clock_params).unwrap_or_default());
        divider.process(&[&clock_out[0], &idle], &mut divider_out, &divider_params, &ctx);
        seq.process(&[&divider_out[0], &idle, &run], &mut seq_out, &seq_params, &ctx);
        clocks.extend_from_slice(&divider_out[0].samples);
        root.extend_from_slice(&seq_out[ChordSequencer::PORT_ROOT].samples);
        let pitch = &seq_out[ChordSequencer::PORT_PITCH];
        for i in 0..BLOCK {
            pad.push((0..pitch.channels()).map(|c| note(pitch.voice(c).samples[i])).collect::<Vec<_>>());
        }
    }

    let onsets = rises(&clocks);
    assert!(onsets.len() >= bars, "a bar's clock for each bar: {}", onsets.len());
    onsets.iter().take(bars).map(|&t| (note(root[t]), pad[t].clone())).collect()
}

#[test]
fn every_bar_sounds_as_the_four_sequencers_did() {
    // Three times round, so the loop back to the top is heard twice
    for (bar, (root, pad)) in play(48).into_iter().enumerate() {
        let (want_root, want_pad) = VOICINGS[bar % 16];
        assert_eq!((root, pad.as_slice()), (want_root, &want_pad[..]), "bar {}", bar + 1);
    }
}

#[test]
fn the_steps_name_the_progression() {
    let patch = from_one_sine();
    let (chord_node, _) = harmony(&patch);
    let params = params_of(&ChordSequencer::new(), chord_node);
    let names: Vec<String> = (0..16).map(|step| ChordSequencer::spec(&params, step).name()).collect();
    let turn = ["Dmadd9", "Bbmaj7", "Fmaj7", "Cadd9"];
    let answer = ["Gm7", "Bbadd9", "Dm", "C", "Gm7", "Bbmaj7", "Csus4", "C"];
    assert_eq!(names, [&turn[..], &turn, &answer].concat());
    // Only two steps say what goes on top; voice leading finds the rest
    let tops: Vec<usize> = (0..16).filter(|&step| ChordSequencer::spec(&params, step).top.is_some()).collect();
    assert_eq!(tops, [0, 12]);
}

#[test]
fn the_root_plays_the_bass_and_moves_the_arp_and_bass_line() {
    let patch = from_one_sine();
    let (chord_node, _) = harmony(&patch);
    let from_root: Vec<(&str, &str)> = patch
        .connections
        .iter()
        .filter(|c| c.from_node == chord_node.id && c.from_port == "Root")
        .map(|c| (node(&patch, c.to_node).module_id.as_str(), c.to_port.as_str()))
        .collect();
    assert_eq!(from_root.iter().filter(|(_, port)| *port == "Exp FM").count(), 2, "the arp and the bass line: {from_root:?}");
    assert!(from_root.contains(&("osc.sine", "V/Oct")), "the pad's bass saw: {from_root:?}");
}

#[test]
fn the_pad_voices_meet_as_one_sound_before_the_filter() {
    // The chord's saws are summed with the bass saw into one channel, so
    // the Ladder drives them together, as it drove four mono saws
    let patch = from_one_sine();
    let (chord_node, _) = harmony(&patch);
    let saw = patch.connections.iter().find(|c| c.from_node == chord_node.id && c.from_port == "Pitch").expect("the chord plays a saw");
    let into_mix = patch.connections.iter().find(|c| c.from_node == saw.to_node).expect("the saw goes on");
    let mix = node(&patch, into_mix.to_node);
    assert_eq!(mix.module_id, "util.mix");
    assert_eq!(param(mix, "Sum Voices"), 1.0);
    let after = patch.connections.iter().find(|c| c.from_node == mix.id).unwrap();
    assert_eq!(node(&patch, after.to_node).module_id, "filter.ladder");
}
