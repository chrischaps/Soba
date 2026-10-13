//! The Clock Divider schedules Backbeat's phrase. Backbeat used to play its
//! tom fill every fourth bar with a second Clock at a quarter of the tempo
//! and a bar counter to keep the two in step; `fixtures/backbeat-phrase-clock.json`
//! keeps that version. The example now uses one Clock Divider dividing the
//! sixteenths by 64. These tests render the two and check they play the
//! same bars.
//!
//! Since the snare joined the kit's room (#100), its reverb tail rings into
//! the step the fill is detected on. The renders take the snare out of the
//! room again, and time the room's return exactly, which makes the mix the
//! fixture's, the old one. Since the kit came up to the house level (#174),
//! its kick rings into that step too, and the bus goes to tape: the renders
//! put the kick, the tape and the levels back as well.

use std::path::{Path, PathBuf};

use soba::engine::read_wav;

/// A sixteenth at Backbeat's 96 BPM, at the render tool's 48 kHz.
const STEP: usize = 7500;

/// Backbeat's kick level envelope, by its id in the patch.
const KICK_LEVEL_ENVELOPE: u64 = 4;

/// Long enough for three fills (bars 1, 5 and 9) and the crash after the
/// third.
const SECONDS: &str = "26";

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soba-divider-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Renders a patch file with the `render` binary, in a process of its own
/// so its Noise modules get the same streams every time, and returns the
/// RMS of each sixteenth, in dBFS, both channels together.
fn steps_db(patch: &Path, wav: &Path) -> Vec<f32> {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_render"))
        .args([patch.as_os_str(), wav.as_os_str()])
        .args(["--seconds", SECONDS])
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "render failed for {}", patch.display());
    let (audio, rate) = read_wav(wav).unwrap();
    assert_eq!(rate, 48000);
    (0..audio.left.len() / STEP)
        .map(|step| {
            let range = step * STEP..(step + 1) * STEP;
            let energy: f32 = audio.left[range.clone()].iter().chain(&audio.right[range]).map(|s| s * s).sum();
            10.0 * (energy / (2 * STEP) as f32 + 1e-15).log10()
        })
        .collect()
}

/// The bars (from 1) that play the fill. Every other bar has an open hat
/// ringing on step 7, and nothing else plays there, so a fill bar, with its
/// hats pulled down, is silent on that step.
fn fill_bars(steps: &[f32]) -> Vec<usize> {
    steps.chunks_exact(16).enumerate().filter(|(_, bar)| bar[6] < -90.0).map(|(n, _)| n + 1).collect()
}

/// Backbeat with its divider's Offset set, and its kick and snare kept out
/// of the room: the drum-bus Mixer chained into the main one sends nothing. The
/// reverb returns through a loop, a 256-sample block late at 48 kHz, so its
/// pre-delay gives back exactly that much of the fixture's 8 ms. The kick's
/// level envelope falls in 450 ms, the main Mixer's Master and the Output's
/// Volume are the fixture's -1.4 dB and 80% between them, and the Tape is
/// taken out, its cables joined straight through.
fn backbeat(offset: f32, path: &Path) -> PathBuf {
    let mut patch: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(repo("patches/backbeat.json")).unwrap()).unwrap();
    let chained = patch["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["to_port"] == "Chain In")
        .map(|c| c["from_node"].clone())
        .expect("the kick and snare's drum bus chains into the main Mixer");
    let main = patch["connections"].as_array().unwrap().iter().find(|c| c["to_port"] == "Chain In").unwrap()["to_node"].clone();
    let tape = patch["nodes"].as_array().unwrap().iter().find(|n| n["module_id"] == "fx.tape").expect("the kit goes to tape")["id"].clone();
    let connections = patch["connections"].as_array_mut().unwrap();
    let (into_tape, rest): (Vec<_>, Vec<_>) = connections.drain(..).partition(|c| c["to_node"] == tape);
    let (out_of_tape, rest): (Vec<_>, Vec<_>) = rest.into_iter().partition(|c| c["from_node"] == tape);
    connections.extend(rest);
    for out in out_of_tape {
        let side = out["from_port"].as_str().unwrap().replace("Out", "In");
        let source = into_tape.iter().find(|c| c["to_port"] == side.as_str()).unwrap();
        let mut through = out.clone();
        through["from_node"] = source["from_node"].clone();
        through["from_port"] = source["from_port"].clone();
        connections.push(through);
    }
    let set = |node: &mut serde_json::Value, name: &str, value: f64| {
        node["parameters"].as_array_mut().unwrap().iter_mut().find(|p| p["name"] == name).unwrap()["value"] = value.into();
    };
    for node in patch["nodes"].as_array_mut().unwrap() {
        if node["id"] == KICK_LEVEL_ENVELOPE {
            set(node, "Decay", 0.45);
            set(node, "Release", 0.45);
        }
        if node["id"] == main {
            set(node, "Master", -1.4);
        }
        if node["module_id"] == "output.audio" {
            set(node, "Volume", 0.8);
        }
        let (divider, kick_and_snare) = (node["module_id"] == "util.divider", node["id"] == chained);
        if node["module_id"] == "fx.reverb" {
            let pre_delay = node["parameters"].as_array_mut().unwrap().iter_mut().find(|p| p["name"] == "Pre-Delay").unwrap();
            pre_delay["value"] = (8.0 - 256.0 / 48.0).into();
        }
        let params = node["parameters"].as_array_mut().unwrap();
        if divider {
            params.iter_mut().find(|p| p["name"] == "Offset").unwrap()["value"] = offset.into();
        }
        if kick_and_snare {
            for param in params.iter_mut().filter(|p| p["name"].as_str().unwrap().starts_with("Send ")) {
                param["value"] = 0.0.into();
            }
        }
    }
    std::fs::write(path, serde_json::to_string_pretty(&patch).unwrap()).unwrap();
    path.to_path_buf()
}

#[test]
fn backbeat_divider_plays_the_same_bars_as_the_phrase_clock() {
    let dir = scratch();
    let phrase_clock = steps_db(&repo("tests/fixtures/backbeat-phrase-clock.json"), &dir.join("phrase-clock.wav"));
    let divider = steps_db(&backbeat(0.0, &dir.join("divider.json")), &dir.join("divider.wav"));

    assert_eq!(fill_bars(&phrase_clock), vec![1, 5, 9]);
    assert_eq!(fill_bars(&divider), vec![1, 5, 9]);

    // Step for step, the same levels. The one difference: the phrase clock
    // closed its gate 100 ms into the bar after a fill, the divider at its
    // second sixteenth, so the hats' fader comes back up a moment later,
    // after the downbeat's closed hat has died away
    assert_eq!(phrase_clock.len(), divider.len());
    for (step, (a, b)) in phrase_clock.iter().zip(&divider).enumerate() {
        if *a > -80.0 {
            assert!((a - b).abs() < 0.1, "bar {} step {}: {a:.2} dB, divider {b:.2} dB", step / 16 + 1, step % 16 + 1);
        }
    }

    // A control: moved a bar later, the fill really does move, and the
    // comparison above would have caught it
    let moved = steps_db(&backbeat(16.0, &dir.join("moved.json")), &dir.join("moved.wav"));
    assert_eq!(fill_bars(&moved), vec![2, 6, 10]);
    let largest = phrase_clock.iter().zip(&moved).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    assert!(largest > 20.0, "moving the fill changed no step by more than {largest:.1} dB");

    std::fs::remove_dir_all(&dir).ok();
}
