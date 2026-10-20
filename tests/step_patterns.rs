//! The Step Sequencer's Pattern input, driven by a real Arranger lane: From
//! One Sine's own Clock and Cues Arranger, played block by block as the
//! engine does. The Arranger runs first, so a lane that jumps on the clock
//! edge starting a section picks the sequencer's pattern on that same edge.

use soba::dsp::{DspModule, ProcessContext, SignalBuffer};
use soba::modules::arranger::Arranger;
use soba::modules::sequencer::{StepField, StepSequencer, PATTERNS};
use soba::modules::Clock;
use soba::persistence::{patch_from_json, NodeData};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;

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

#[test]
fn an_arranger_lane_switches_the_pattern_on_the_sample_a_section_starts() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("patches/from-one-sine.json");
    let patch = patch_from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let clock_node = patch.nodes.iter().find(|n| n.module_id == "util.clock").unwrap();
    let cues = patch
        .nodes
        .iter()
        .find(|n| n.module_id == "seq.arranger" && n.labels.get("Lane 1").map(String::as_str) == Some("Drums"))
        .expect("the Cues Arranger, whose first lane picks the drum pattern");

    let mut clock = Clock::new();
    let mut arranger = Arranger::new();
    let mut seq = StepSequencer::new();
    let clock_params = params_of(&clock, clock_node);
    let arranger_params = params_of(&arranger, cues);
    // A bar of sixteenths a pass, and each pattern its own note: A is C4,
    // B D4, C E4, D F#4
    let mut seq_params: Vec<f32> = seq.parameters().iter().map(|p| p.default).collect();
    seq_params[StepSequencer::PARAM_STEPS] = 16.0;
    for pattern in 0..PATTERNS {
        for step in 0..16 {
            seq_params[StepSequencer::step_param(pattern, step, StepField::Pitch)] = (60 + 2 * pattern) as f32;
        }
    }
    clock.prepare(RATE, BLOCK);
    arranger.prepare(RATE, BLOCK);
    seq.prepare(RATE, BLOCK);

    let seconds = 175.0;
    let total = (seconds * RATE) as usize;
    let mut clock_out: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::gate(BLOCK)).collect();
    let mut arranger_out: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(BLOCK)).collect();
    let mut seq_out: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(BLOCK)).collect();
    let idle = SignalBuffer::gate(BLOCK);
    let mut run = SignalBuffer::gate(BLOCK);
    run.samples.fill(1.0);
    let (mut section_trig, mut lane, mut pitch) = (Vec::new(), Vec::new(), Vec::new());
    let ctx = ProcessContext::new(RATE, BLOCK);
    for _ in 0..total.div_ceil(BLOCK) {
        clock.process(&[], &mut clock_out, &clock_params, &ctx);
        let ctx = ProcessContext::with_transport(RATE, BLOCK, clock.transport(&clock_params).unwrap_or_default());
        arranger.process(&[&clock_out[0], &idle, &idle, &idle], &mut arranger_out, &arranger_params, &ctx);
        let pattern_cv = &arranger_out[Arranger::OUT_LANES];
        seq.process(&[&clock_out[0], &idle, &run, pattern_cv], &mut seq_out, &seq_params, &ctx);
        section_trig.extend_from_slice(&arranger_out[Arranger::OUT_SECTION_TRIG].samples);
        lane.extend_from_slice(&pattern_cv.samples);
        pitch.extend_from_slice(&seq_out[0].samples);
    }

    // Every section that changes the lane's pattern changes the sequencer's
    // on its first sample, never a step late
    let zone = |cv: f32| ((cv * 4.0).floor().max(0.0) as usize).min(3);
    let mut changes = 0;
    for t in rises(&section_trig).into_iter().filter(|&t| t > 0) {
        let (before, after) = (zone(lane[t - 1]), zone(lane[t]));
        assert_eq!(note(pitch[t - 1]), 60 + 2 * before as i32, "the old pattern up to sample {t}");
        assert_eq!(note(pitch[t]), 60 + 2 * after as i32, "the new pattern from sample {t}");
        changes += usize::from(before != after);
    }
    // The drop at bar 81 is one of them: D's Build into C's groove
    assert!(changes >= 2, "the song changes drum pattern at least twice by the drop: {changes}");
}
