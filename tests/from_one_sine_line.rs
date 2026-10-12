//! From One Sine's lead line: the motif's Step Sequencer holds four 4-bar
//! patterns, and the Cues Arranger's Line lane picks which one each pass
//! plays. These tests play the patch's own Clock, Divider, Cues and motif
//! sequencer, block by block as the engine does, and read off the line.

use soba::dsp::{DspModule, ProcessContext, SignalBuffer};
use soba::modules::arranger::Arranger;
use soba::modules::sequencer::{StepField, StepSequencer, PATTERNS};
use soba::modules::{Clock, ClockDivider};
use soba::persistence::{patch_from_json, NodeData, Patch};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;
/// 4 bars at 112 BPM.
const PASS_SECONDS: f32 = 4.0 * 4.0 * 60.0 / 112.0;

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

fn rises(signal: &[f32]) -> Vec<usize> {
    (0..signal.len()).filter(|&t| signal[t] > 0.5 && (t == 0 || signal[t - 1] <= 0.5)).collect()
}

/// The MIDI note a V/Oct pitch is.
fn note(voct: f32) -> i32 {
    (voct * 12.0).round() as i32 + 60
}

/// The quarter-note Divider and the motif sequencer it clocks.
fn motif(patch: &Patch) -> (&NodeData, &NodeData) {
    let node = |id: u64| patch.nodes.iter().find(|n| n.id == id).unwrap();
    let quarters = |n: &NodeData| n.parameters.iter().any(|p| p.name == "Divide" && p.value.as_f32() == 4.0);
    patch
        .connections
        .iter()
        .filter(|c| c.from_port == "Trig" && c.to_port == "Clock")
        .map(|c| (node(c.from_node), node(c.to_node)))
        .find(|(from, to)| from.module_id == "util.divider" && quarters(from) && to.module_id == "seq.step")
        .expect("a Step Sequencer playing quarters")
}

/// Plays the song once and returns, for each 4-bar pass, the note each of
/// its quarters starts (None for a rest) and the pattern letter it matches.
fn the_line() -> Vec<(Vec<Option<i32>>, char)> {
    let patch = from_one_sine();
    let node = |id: &str| patch.nodes.iter().find(|n| n.module_id == id).unwrap();
    let cues = patch
        .nodes
        .iter()
        .find(|n| n.module_id == "seq.arranger" && n.labels.get("Name").map(String::as_str) == Some("Cues"))
        .unwrap();
    let (divider_node, motif_node) = motif(&patch);
    let (lane, _) = cues.labels.iter().find(|(_, v)| v.as_str() == "Line").expect("Cues has a Line lane");
    assert!(
        patch.connections.iter().any(|c| c.from_node == cues.id
            && &c.from_port == lane
            && c.to_node == motif_node.id
            && c.to_port == "Pattern"),
        "the Line lane picks the motif's pattern"
    );
    let lane: usize = lane.trim_start_matches("Lane ").parse().unwrap();

    let mut clock = Clock::new();
    let mut divider = ClockDivider::new();
    let mut arranger = Arranger::new();
    let mut seq = StepSequencer::new();
    let clock_params = params_of(&clock, node("util.clock"));
    let divider_params = params_of(&divider, divider_node);
    let arranger_params = params_of(&arranger, cues);
    let seq_params = params_of(&seq, motif_node);
    for module in [&mut clock as &mut dyn DspModule, &mut divider, &mut arranger, &mut seq] {
        module.prepare(RATE, BLOCK);
    }
    // What each pattern plays, quarter by quarter
    let patterns: Vec<Vec<Option<i32>>> = (0..PATTERNS)
        .map(|p| {
            (0..16)
                .map(|s| {
                    let gate = seq_params[StepSequencer::step_param(p, s, StepField::Gate)] > 0.5;
                    gate.then(|| seq_params[StepSequencer::step_param(p, s, StepField::Pitch)] as i32)
                })
                .collect()
        })
        .collect();

    let passes = 32;
    let total = (passes as f32 * PASS_SECONDS * RATE) as usize + BLOCK;
    let mut clock_out: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::gate(BLOCK)).collect();
    let mut divider_out: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::gate(BLOCK)).collect();
    let mut arranger_out: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(BLOCK)).collect();
    let mut seq_out: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(BLOCK)).collect();
    let idle = SignalBuffer::gate(BLOCK);
    let mut run = SignalBuffer::gate(BLOCK);
    run.samples.fill(1.0);
    let (mut quarters, mut pitch, mut gate) = (Vec::new(), Vec::new(), Vec::new());
    let ctx = ProcessContext::new(RATE, BLOCK);
    for _ in 0..total.div_ceil(BLOCK) {
        clock.process(&[], &mut clock_out, &clock_params, &ctx);
        let ctx = ProcessContext::with_transport(RATE, BLOCK, clock.transport(&clock_params).unwrap_or_default());
        divider.process(&[&clock_out[0], &idle], &mut divider_out, &divider_params, &ctx);
        arranger.process(&[&clock_out[0], &idle, &idle, &idle], &mut arranger_out, &arranger_params, &ctx);
        let line = &arranger_out[Arranger::OUT_LANES + 2 * (lane - 1)];
        seq.process(&[&divider_out[0], &idle, &run, line], &mut seq_out, &seq_params, &ctx);
        quarters.extend_from_slice(&divider_out[0].samples);
        pitch.extend_from_slice(&seq_out[0].samples);
        gate.extend_from_slice(&seq_out[1].samples);
    }

    let onsets = rises(&quarters);
    assert!(onsets.len() >= passes * 16, "a song's worth of quarters: {}", onsets.len());
    onsets
        .chunks(16)
        .take(passes)
        .map(|pass| {
            let heard: Vec<Option<i32>> = pass.iter().map(|&t| (gate[t] > 0.5).then(|| note(pitch[t]))).collect();
            let letter = patterns.iter().position(|p| *p == heard).map_or('?', |p| (b'A' + p as u8) as char);
            (heard, letter)
        })
        .collect()
}

#[test]
fn the_line_grows_from_the_motif_and_returns_to_it() {
    let line: String = the_line().iter().map(|(_, letter)| *letter).collect();
    // A pass is 4 bars, so a group of four is one turn of the progression
    let expected = [
        "AAAA", // First Sound: the motif alone on the sine, as the Looper keeps it
        "AAAA", // Pulse (the sine rests)
        "AAAA", // Groove
        "ABCD", // Lift: the lead plays the whole line
        "DDAD", // Memory (silent), then the sine answers its ghost, and the Build turns
        "ABCD", // Everything
        "ABCD", // Second Wave on the sine, the lead returning for C and D
        "AAAA", // Return and One Sine: the motif alone again
    ]
    .concat();
    assert_eq!(line, expected, "the pattern each 4-bar pass plays");
}

#[test]
fn the_line_keeps_the_motif_and_answers_it() {
    let line = the_line();
    let pattern = |letter: char| &line.iter().find(|(_, l)| *l == letter).unwrap().0;
    let (a, b, c, d) = (pattern('A'), pattern('B'), pattern('C'), pattern('D'));
    // The motif is the line's first four bars, as it always was
    let motif = [69, 69, 74, 76, 77, 77, 74, 74, 72, 72, 69, 72, 67, 67, 67];
    assert_eq!(a[..15].iter().map(|n| n.unwrap()).collect::<Vec<_>>(), motif);
    // Each pattern breathes on its last quarter, as the motif does
    for p in [a, b, c, d] {
        assert_eq!(p[15], None);
    }
    // B answers from the motif's first note; D comes home on its cadence
    assert_eq!(b[0], a[0]);
    assert_eq!(d[8..], a[8..], "D's last two bars are the motif's");
    // C is the line's peak
    let top = |p: &Vec<Option<i32>>| p.iter().flatten().copied().max().unwrap();
    assert!(top(c) > top(a).max(top(b)).max(top(d)), "C climbs highest");
}
