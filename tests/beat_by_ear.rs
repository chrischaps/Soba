//! The Dancer finds the beat by ear in the examples with one, and dances on
//! it rather than between (#281).
//!
//! Ignored by default: it renders a few minutes of music. Run it in release
//! mode:
//!
//! ```text
//! cargo test --release --test beat_by_ear -- --ignored --nocapture
//! ```
//!
//! Each example is rendered, mixed to mono as an Audio Input would hear it,
//! and played to a fresh [`BeatTracker`]. Every beat it counts is measured
//! against the song's own beat, read from the tempo of its Clock, which
//! starts on a beat at the top. Beats are measured against the song's
//! tempo, not the tracker's: measuring against its own would hide a drift.
//!
//! For each 10 s window from the moment the drums come in, it prints how
//! far the tracker's beats land from the song's (their median, in ms) and
//! how many of them land within [`ON_BEAT_SECONDS`]. An example passes when
//! every window after the first holds the beat: a tracker dancing on the
//! *and* lands half a beat out. The few windows known not to are listed in
//! [`LOOSE`], with why; they're printed but don't fail.

use std::thread;

use soba::dsp::beat_tracker::BeatTracker;
use soba::engine::OfflineRenderer;
use soba::persistence::sample_files::SampleBase;
use soba::persistence::EXAMPLES;

const SAMPLE_RATE: f32 = 48_000.0;
/// The tracker's beat is read this often, as the Dancer draws it.
const READ_SECONDS: f32 = 0.01;
/// The window the offsets are gathered over.
const WINDOW_SECONDS: f32 = 10.0;
/// A beat this close to the song's is on it.
const ON_BEAT_SECONDS: f32 = 0.025;
/// A window holds the beat when its median lands this close, and this share
/// of its beats are on it.
const MEDIAN_SECONDS: f32 = 0.012;
const HELD_SHARE: f32 = 0.8;

/// The examples with a beat to find: (file, BPM, when the beat comes in
/// and how long to listen, in seconds). Canopy's drums come in with its
/// Groove, From One Sine's with its Pulse, and Canopy is heard to the end
/// of its Night, when they stop. Interlock is heard through both of the
/// stretches its LFO silences one of its two voices for, when three against
/// four scores about as well as the beat.
const SONGS: &[(&str, f32, f32, f32)] = &[
    ("canopy.json", 116.0, 33.0, 243.0),
    ("backbeat.json", 96.0, 0.0, 40.0),
    ("drum-machine.json", 100.0, 0.0, 40.0),
    ("interlock.json", 100.0, 0.0, 80.0),
    ("cut-up.json", 112.0, 0.0, 40.0),
    ("front-porch.json", 96.0, 0.0, 40.0),
    ("roll-call.json", 104.0, 0.0, 40.0),
    ("from-one-sine.json", 112.0, 34.3, 94.3),
    ("afterglow.json", 92.0, 0.0, 40.0),
];

/// Windows known not to hold the beat: (file, from and to, in seconds, and why).
const LOOSE: &[(&str, f32, f32, &str)] = &[
    ("canopy.json", 113.0, 143.0, "the Clearing drops the drums and the beat is lost; it's found again on the and, and turned 10 s into the Peak (#287)"),
    ("interlock.json", 10.0, 20.0, "no drums and one voice at first: it hears dotted eighths, turns to the beat at 8 s and is still settling (#288)"),
];

/// One window of an example heard: its start, the median offset of the
/// beats counted in it from the song's, and the share on the beat.
struct Window {
    from: f32,
    beats: usize,
    median: Option<f32>,
    on_beat: f32,
    /// The beat heard, in seconds, at the window's end.
    heard: Option<f32>,
}

#[test]
#[ignore = "slow: renders a few minutes of music; run with --release"]
fn the_beat_is_danced_on_in_every_example_with_one() {
    let heard: Vec<(&str, Vec<Window>)> = thread::scope(|scope| {
        let jobs: Vec<_> = SONGS.iter().map(|&(file, bpm, drums, seconds)| scope.spawn(move || (file, hear(file, bpm, drums, seconds)))).collect();
        jobs.into_iter().map(|job| job.join().unwrap()).collect()
    });

    let mut failures = Vec::new();
    for (file, windows) in &heard {
        println!("{file}");
        for (i, window) in windows.iter().enumerate() {
            let median = window.median.map_or("    -".to_string(), |m| format!("{:+5.0}", m * 1000.0));
            let heard = window.heard.map_or("    -".to_string(), |s| format!("{:5.0}", 60.0 / s));
            println!("  {:>5.0} s  {:>3} beats at {heard} BPM  median {median} ms  {:>3.0}% on the beat", window.from, window.beats, window.on_beat * 100.0);
            // The first window is for finding the beat
            let held = window.median.is_some_and(|m| m.abs() <= MEDIAN_SECONDS) && window.on_beat >= HELD_SHARE;
            let loose = LOOSE.iter().find(|&&(f, from, to, _)| f == *file && (from..to).contains(&window.from));
            match loose {
                Some((.., why)) if !held => println!("         known: {why}"),
                None if i > 0 && !held => failures.push(format!("{file} from {:.0} s", window.from)),
                _ => {}
            }
        }
    }
    assert!(failures.is_empty(), "the beat isn't held: {failures:?}");
}

/// Renders an example and plays it to a tracker, measuring each window's
/// beats from when the drums come in.
fn hear(file: &str, bpm: f32, drums: f32, seconds: f32) -> Vec<Window> {
    let example = EXAMPLES.iter().find(|e| e.file_name == file).unwrap_or_else(|| panic!("no example {file}"));
    let patch = example.patch().unwrap();
    let (mut renderer, compiled) = OfflineRenderer::from_patch(&patch, SAMPLE_RATE, 256).unwrap();
    let missing = renderer.load_samples(&compiled, SampleBase::Example);
    assert!(missing.is_empty(), "{file}: {missing:?}");
    let audio = renderer.render_seconds(seconds);

    let mut tracker = BeatTracker::new();
    tracker.prepare(SAMPLE_RATE);
    let read_every = (READ_SECONDS * SAMPLE_RATE) as usize;
    let beat = 60.0 / bpm;
    // The time of each beat the tracker counts, and its offset from the song's
    let mut counted: Vec<(f32, f32)> = Vec::new();
    let mut heard: Vec<Option<f32>> = Vec::new();
    let mut last: Option<f64> = None;
    for (n, (l, r)) in audio.left.iter().zip(&audio.right).enumerate() {
        tracker.hear(0.5 * (l + r));
        if n % read_every != read_every - 1 {
            continue;
        }
        heard.push(tracker.beat().map(|(_, seconds)| seconds));
        let now = tracker.beat().map(|(position, _)| position);
        if let (Some(before), Some(after)) = (last, now) {
            if after.floor() > before.floor() {
                let through = ((after.floor() - before) / (after - before)) as f32;
                let at = (n + 1) as f32 / SAMPLE_RATE + (through - 1.0) * READ_SECONDS;
                let offset = ((at / beat + 0.5).rem_euclid(1.0) - 0.5) * beat;
                counted.push((at, offset));
            }
        }
        last = now;
    }

    let mut windows = Vec::new();
    let mut from = drums;
    while from + WINDOW_SECONDS <= seconds + 0.01 {
        let mut offsets: Vec<f32> = counted.iter().filter(|(at, _)| (from..from + WINDOW_SECONDS).contains(at)).map(|&(_, o)| o).collect();
        offsets.sort_by(f32::total_cmp);
        let on = offsets.iter().filter(|o| o.abs() <= ON_BEAT_SECONDS).count();
        // Beats the window should have had, so beats not counted count against it
        let expected = (WINDOW_SECONDS / beat).floor();
        windows.push(Window {
            from,
            beats: offsets.len(),
            median: (!offsets.is_empty()).then(|| offsets[offsets.len() / 2]),
            on_beat: on as f32 / expected,
            heard: heard.get(((from + WINDOW_SECONDS) / READ_SECONDS) as usize - 1).copied().flatten(),
        });
        from += WINDOW_SECONDS;
    }
    windows
}
