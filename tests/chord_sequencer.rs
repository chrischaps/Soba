//! The Chord Sequencer playing every Library voice: each chord tone sounds
//! on its own channel, and a tie carries a chord on without a new attack.

use soba::dsp::analysis::{amp_to_db, peak, rms};
use soba::engine::OfflineRenderer;
use soba::persistence::{ConnectionData, LibraryGroup, NamedParameter, NodeData, ParameterValue, Patch, Section, LIBRARY};

const RATE: f32 = 48_000.0;

fn voices() -> impl Iterator<Item = &'static LibraryGroup> {
    LIBRARY.iter().filter(|g| g.section == Section::Voices)
}

fn node(id: u64, module_id: &str, params: &[(&str, ParameterValue)]) -> NodeData {
    let mut node = NodeData::new(id, module_id, (0.0, 0.0));
    node.parameters = params.iter().map(|(n, v)| NamedParameter::new(*n, v.clone())).collect();
    node
}

/// `entry` played by a Chord Sequencer at 60 BPM (a chord a second) with
/// `params`, its sound to the output.
fn played(entry: &LibraryGroup, params: &[(&str, ParameterValue)]) -> Patch {
    played_at(entry, 60.0, params)
}

fn played_at(entry: &LibraryGroup, bpm: f32, params: &[(&str, ParameterValue)]) -> Patch {
    let mut patch = entry.patch().unwrap();
    let g = patch.groups[0].id;
    patch.nodes = vec![
        node(9000, "output.audio", &[("Volume", ParameterValue::Number(1.0)), ("Limiter", ParameterValue::Toggle(false))]),
        node(9001, "util.clock", &[("Tempo", ParameterValue::Number(bpm))]),
        node(9002, "seq.chord", params),
    ];
    let wire = |a, ap: &str, b, bp: &str| ConnectionData::new(a, ap, b, bp);
    patch.connections = vec![
        wire(9001, "Gate", 9002, "Clock"),
        wire(9002, "Pitch", g, "Pitch"),
        wire(9002, "Gate", g, "Gate"),
        wire(9002, "Velocity", g, "Velocity"),
        wire(g, "Out", 9000, "Mono"),
    ];
    patch
}

fn render(patch: &Patch, seconds: f32) -> Vec<f32> {
    let (mut renderer, compiled) = OfflineRenderer::from_patch(patch, RATE, 256).unwrap();
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
    renderer.render((seconds * RATE) as usize).left
}

/// How strongly `samples` hold a frequency within a few cents of `hz`
/// (detuned unison voices spread a note a little).
fn strength(samples: &[f32], hz: f32) -> f32 {
    (-4..=4)
        .map(|cents| {
            let f = hz * 2f32.powf(cents as f32 * 5.0 / 1200.0);
            let w = 2.0 * std::f32::consts::PI * f / RATE;
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (n, &s) in samples.iter().enumerate() {
                // A Hann window, so a loud neighbour doesn't leak in
                let hann = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / samples.len() as f32).cos();
                re += s * hann * (w * n as f32).cos();
                im -= s * hann * (w * n as f32).sin();
            }
            (re * re + im * im).sqrt() / samples.len() as f32
        })
        .fold(0.0, f32::max)
}

fn hz(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

/// One chord, C major in Range 3 (C3 E3 G3), or only its root.
fn c_major(voices: f32) -> Vec<(&'static str, ParameterValue)> {
    vec![
        ("Steps", ParameterValue::Number(1.0)),
        ("Voices", ParameterValue::Number(voices)),
        ("Range", ParameterValue::Number(3.0)),
        ("Step 1 Root", ParameterValue::Number(48.0)),
        ("Step 1 Type", ParameterValue::Select(0)),
    ]
}

#[test]
fn every_library_voice_plays_each_chord_tone() {
    for entry in voices() {
        let chord = render(&played(entry, &c_major(3.0)), 1.0);
        let root = render(&played(entry, &c_major(1.0)), 1.0);
        for (side, audio) in [("chord", &chord), ("root", &root)] {
            assert!(audio.iter().all(|s| s.is_finite()), "{}: {side} isn't finite", entry.name);
            assert!(amp_to_db(peak(audio)) < 0.0, "{}: {side} clips", entry.name);
            assert!(amp_to_db(rms(audio)) > -45.0, "{}: {side} is silent", entry.name);
        }

        // Past the attack. The 3rd and 5th only sound when the cables carry
        // them: each is far stronger in the chord than over the root alone,
        // in the octave the voice plays it (a bass plays two down)
        let window = |audio: &[f32]| audio[(0.1 * RATE) as usize..(0.6 * RATE) as usize].to_vec();
        let (chord, root) = (window(&chord), window(&root));
        let gain = |note: i32| amp_to_db(strength(&chord, hz(note))) - amp_to_db(strength(&root, hz(note)));
        for (name, note) in [("E", 52), ("G", 55)] {
            let best = [-24, -12, 0, 12].map(|octave| gain(note + octave)).into_iter().fold(f32::MIN, f32::max);
            assert!(best > 12.0, "{}: {name} only {best:.1} dB stronger in the chord", entry.name);
        }
    }
}

#[test]
fn a_tie_carries_the_chord_on_without_a_new_attack() {
    // C major tied into C major at 60 BPM sounds exactly as C major held
    // alone, here by a clock too slow to reach its second beat in time
    let held = c_major(3.0);
    let pair = |tie: bool| {
        let mut params = c_major(3.0);
        params[0] = ("Steps", ParameterValue::Number(2.0));
        params.extend([
            ("Step 2 Root", ParameterValue::Number(48.0)),
            ("Step 2 Type", ParameterValue::Select(0)),
            ("Step 1 Tie", ParameterValue::Toggle(tie)),
        ]);
        params
    };
    let difference = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);

    let mut struck_again = 0;
    for entry in voices() {
        let held = render(&played_at(entry, 30.0, &held), 1.5);
        let tied = render(&played(entry, &pair(true)), 1.5);
        let untied = render(&played(entry, &pair(false)), 1.5);
        assert!(peak(&held[RATE as usize..]) > 1e-3, "{}: still sounding at the second beat", entry.name);
        assert_eq!(difference(&held, &tied), 0.0, "{}: the tie changed the sound", entry.name);
        if difference(&held, &untied) > 1e-3 {
            struck_again += 1;
        }
    }
    // Played again untied, the chord strikes anew, so the comparison can
    // tell. A voice already sustaining at its envelope's peak (the String
    // Machine) re-attacks from full to full, and sounds the same either way
    let voices = voices().count();
    assert!(struck_again >= voices - 1, "only {struck_again} of {voices} voices struck again untied");
}

/// C major (C3 E3 G3) strummed up, `ms` between notes.
fn strummed(ms: f32) -> Vec<(&'static str, ParameterValue)> {
    let mut params = c_major(3.0);
    params.extend([("Strum", ParameterValue::Number(ms)), ("Strum Dir", ParameterValue::Select(0))]);
    params
}

fn struck_voices() -> impl Iterator<Item = &'static LibraryGroup> {
    voices().filter(|g| ["Electric Piano", "Mallet"].contains(&g.name))
}

#[test]
fn a_strum_sounds_the_chord_from_the_bottom_up() {
    // 80 ms apart: C on the beat, E at 80 ms, G at 160 ms. Before G's turn
    // it's far quieter than in the same chord struck at once (not silent: an
    // octave up, it's C's 3rd harmonic), and after it it's there
    assert_eq!(struck_voices().count(), 2);
    for entry in struck_voices() {
        let rolled = render(&played(entry, &strummed(80.0)), 0.6);
        let struck = render(&played(entry, &strummed(0.0)), 0.6);
        let window = |audio: &[f32], from: f32, to: f32| audio[(from * RATE) as usize..(to * RATE) as usize].to_vec();
        let g = |audio: &[f32]| [-24, -12, 0, 12].map(|octave| amp_to_db(strength(audio, hz(55 + octave)))).into_iter().fold(f32::MIN, f32::max);
        let early = g(&window(&struck, 0.02, 0.14)) - g(&window(&rolled, 0.02, 0.14));
        assert!(early > 10.0, "{}: G before its turn only {early:.1} dB quieter", entry.name);
        let late = g(&window(&struck, 0.25, 0.55)) - g(&window(&rolled, 0.25, 0.55));
        assert!(late.abs() < 6.0, "{}: G after its turn {late:.1} dB off", entry.name);
    }
}

#[test]
fn a_strummed_tie_strikes_nothing_again() {
    // As unstrummed: C major strummed and tied into itself sounds exactly as
    // the strummed chord held alone
    for entry in struck_voices() {
        let held = render(&played_at(entry, 30.0, &strummed(80.0)), 1.5);
        let mut pair = strummed(80.0);
        pair[0] = ("Steps", ParameterValue::Number(2.0));
        pair.extend([
            ("Step 2 Root", ParameterValue::Number(48.0)),
            ("Step 2 Type", ParameterValue::Select(0)),
            ("Step 1 Tie", ParameterValue::Toggle(true)),
        ]);
        let tied = render(&played(entry, &pair), 1.5);
        let difference = held.iter().zip(&tied).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
        assert_eq!(difference, 0.0, "{}: the tie struck again", entry.name);
    }
}

#[test]
fn a_tapered_strum_brushes_the_last_string() {
    // Strummed up with Taper 75%: C, struck first, is as loud as ever, E
    // softer, and G, struck last at a quarter of the step's velocity,
    // softer still. (The Mallet's velocity only reaches its strike, and its
    // ring barely follows it: a quarter of the velocity is 1.5 dB. The
    // Electric Piano's level follows the touch, so it's the one to hear)
    for entry in struck_voices().filter(|g| g.name == "Electric Piano") {
        let even = render(&played(entry, &strummed(80.0)), 0.6);
        let mut params = strummed(80.0);
        params.push(("Strum Taper", ParameterValue::Number(75.0)));
        let tapered = render(&played(entry, &params), 0.6);
        let after = |audio: &[f32]| audio[(0.25 * RATE) as usize..(0.55 * RATE) as usize].to_vec();
        let level = |audio: &[f32], note: i32| amp_to_db(strength(&after(audio), hz(note)));
        let [c, e, g] = [48, 52, 55].map(|note| level(&even, note) - level(&tapered, note));
        assert!(c.abs() < 0.5, "{}: C {c:.1} dB off", entry.name);
        assert!(e > 1.0 && g > e + 1.0, "{}: E {e:.1} dB and G {g:.1} dB softer, not stepping down", entry.name);
    }
}
