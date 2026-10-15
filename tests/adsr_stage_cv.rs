//! The ADSR's stage CV, patched as a patch file patches it and rendered:
//! velocity into Decay rings harder notes longer, and a held CV into Release
//! lengthens every tail.

use soba::dsp::MidiMessage;
use soba::engine::OfflineRenderer;
use soba::persistence::{ConnectionData, NamedParameter, NodeData, ParameterValue, Patch};

const RATE: f32 = 48_000.0;

fn node(id: u64, module_id: &str, params: &[(&str, f32)]) -> NodeData {
    let mut node = NodeData::new(id, module_id, (0.0, 0.0));
    node.parameters = params.iter().map(|&(n, v)| NamedParameter::new(n, ParameterValue::Number(v))).collect();
    node
}

/// A MIDI Note playing a sine through a VCA its ADSR opens, a pluck: no
/// sustain. `extra` adds cables into the ADSR (node 4); node 6 is an
/// Attenuverter with nothing in, putting out its Offset of `offset`.
fn pluck(offset: f32, extra: &[(u64, &str, &str)]) -> Patch {
    let mut patch = Patch::new("Pluck");
    patch.nodes = vec![
        node(1, "output.audio", &[("Volume", 1.0)]),
        node(2, "input.midi_note", &[]),
        node(3, "osc.sine", &[]),
        node(4, "mod.adsr", &[("Attack", 0.001), ("Decay", 0.2), ("Sustain", 0.0), ("Release", 0.1)]),
        node(5, "util.vca", &[("Level", 0.5)]),
        node(6, "util.attenuverter", &[("Offset", offset)]),
    ];
    let wire = |a, ap: &str, b, bp: &str| ConnectionData::new(a, ap, b, bp);
    patch.connections = vec![
        wire(2, "Pitch", 3, "V/Oct"),
        wire(2, "Gate", 4, "Gate"),
        wire(4, "Out", 5, "CV"),
        wire(3, "Out", 5, "In"),
        wire(5, "Out", 1, "Mono"),
    ];
    patch.connections.extend(extra.iter().map(|&(from, port, to)| wire(from, port, 4, to)));
    patch
}

/// How long a note held for `held` seconds at `velocity` (1-127) sounds:
/// from its onset until it's 40 dB under its peak for good.
fn rings_for(patch: &Patch, velocity: u8, held: f32) -> f32 {
    let (mut r, compiled) = OfflineRenderer::from_patch(patch, RATE, 256).unwrap();
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
    r.queue_midi(0, 0, MidiMessage::NoteOn { note: 69, velocity });
    r.queue_midi((held * RATE) as u64, 0, MidiMessage::NoteOff { note: 69, velocity: 0 });
    let out = r.render((2.0 * RATE) as usize).left;
    let peak = out.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    let onset = out.iter().position(|&s| s.abs() > peak * 0.01).unwrap();
    let end = out.iter().rposition(|&s| s.abs() > peak * 0.01).unwrap();
    (end - onset) as f32 / RATE
}

#[test]
fn velocity_into_decay_rings_harder_notes_longer() {
    let patch = pluck(0.0, &[(2, "Velocity", "Decay")]);
    let (hard, soft) = (rings_for(&patch, 127, 1.0), rings_for(&patch, 1, 1.0));
    // +1 at full velocity doubles the decay; the softest note is the knob's
    let ratio = hard / soft;
    assert!((1.9..2.1).contains(&ratio), "hard {hard:.3}s, soft {soft:.3}s: {ratio:.2}x");
}

#[test]
fn cv_into_release_lengthens_the_tail() {
    // A short note, let go mid-decay: the release is the tail
    let knob = rings_for(&pluck(0.0, &[]), 100, 0.02);
    let held = rings_for(&pluck(1.0, &[(6, "Out", "Release")]), 100, 0.02);
    let tail = |total: f32| total - 0.02;
    let ratio = tail(held) / tail(knob);
    assert!((1.9..2.1).contains(&ratio), "knob {knob:.3}s, +1 {held:.3}s: tail {ratio:.2}x");

    // Patched but at 0, nothing changes
    let zero = rings_for(&pluck(0.0, &[(6, "Out", "Release")]), 100, 0.02);
    assert!((zero - knob).abs() < 1e-3, "{zero} vs {knob}");
}

/// How long a held note at a high pitch (A7, so the wave is fine-grained)
/// takes to rise from its onset to 90% of its peak, with a 50 ms Attack
/// knob. Node 6 is an Attenuverter with nothing in, putting out `offset`
/// (an LFO at one end of its swing); if `range` is Some, a second
/// Attenuverter at Amount 1 and that Range carries it into Attack.
fn attack_time(offset: f32, range: Option<usize>) -> f32 {
    let mut patch = Patch::new("Swell");
    patch.nodes = vec![
        node(1, "output.audio", &[("Volume", 1.0)]),
        node(2, "input.midi_note", &[]),
        node(3, "osc.sine", &[]),
        node(4, "mod.adsr", &[("Attack", 0.05), ("Decay", 0.2), ("Sustain", 1.0), ("Release", 0.1)]),
        node(5, "util.vca", &[("Level", 0.5)]),
        node(6, "util.attenuverter", &[("Offset", offset)]),
    ];
    let wire = |a, ap: &str, b, bp: &str| ConnectionData::new(a, ap, b, bp);
    patch.connections = vec![
        wire(2, "Pitch", 3, "V/Oct"),
        wire(2, "Gate", 4, "Gate"),
        wire(4, "Out", 5, "CV"),
        wire(3, "Out", 5, "In"),
        wire(5, "Out", 1, "Mono"),
    ];
    if let Some(range) = range {
        let mut scale = node(7, "util.attenuverter", &[("Amount", 1.0)]);
        scale.parameters.push(NamedParameter::new("Range", ParameterValue::Select(range)));
        patch.nodes.push(scale);
        patch.connections.extend([wire(6, "Out", 7, "In"), wire(7, "Out", 4, "Attack")]);
    }
    let (mut r, compiled) = OfflineRenderer::from_patch(&patch, RATE, 256).unwrap();
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
    r.queue_midi(0, 0, MidiMessage::NoteOn { note: 105, velocity: 127 });
    let out = r.render((1.5 * RATE) as usize).left;
    let peak = out.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    let onset = out.iter().position(|&s| s.abs() > peak * 0.01).unwrap();
    let top = out.iter().position(|&s| s.abs() > peak * 0.9).unwrap();
    (top - onset) as f32 / RATE
}

#[test]
fn an_attenuverter_at_x4_sweeps_the_attack_far() {
    // #215: ±1 into Attack only halves or doubles a 50 ms attack. Through an
    // Attenuverter at ×4 it's ±4 octaves: from about 3 ms to 0.8 s
    let knob = attack_time(0.0, None);
    let (snappy, slow) = (attack_time(-1.0, Some(1)), attack_time(1.0, Some(1)));
    assert!((snappy / knob - 1.0 / 16.0).abs() < 0.1 / 16.0, "knob {knob:.4}s, -4 {snappy:.4}s");
    assert!((slow / knob - 16.0).abs() < 0.1 * 16.0, "knob {knob:.4}s, +4 {slow:.4}s");

    // At ×1 it's the doubling it always was
    let doubled = attack_time(1.0, Some(0));
    assert!((doubled / knob - 2.0).abs() < 0.1 * 2.0, "knob {knob:.4}s, +1 {doubled:.4}s");
}
