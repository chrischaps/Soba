//! Every example sits near one level, so switching between them in the app
//! doesn't jump in volume.
//!
//! Ignored by default: it renders nearly four hours of music, which takes
//! a couple of minutes across a desktop's cores. Run it before a release,
//! in release mode:
//!
//! ```text
//! cargo test --release --test example_loudness -- --ignored --nocapture
//! ```
//!
//! Each example is heard for long enough to be itself: an arranged song
//! from top to bottom, a patch that waits for a player for the audition
//! phrase played into it, and a generative patch for two and a half
//! minutes, [`HEARINGS`] times over. Every Noise module draws the next
//! random stream there is, so a generative patch plays differently each
//! time it's opened, and some aren't the same loudness twice: Shoreline's
//! random sea measures 5 LU louder in one hearing than in another.
//! The energy mean of eight is steady to about half a dB.
//!
//! Each must measure within [`TOLERANCE_LU`] of [`HOUSE_LUFS`] (integrated,
//! BS.1770-4), and peak below [`PEAK_CEILING_DB`] going into the Audio
//! Output's limiter, which is there to catch accidents, not to shape the
//! sound. The few examples that can't reach the house level say why in
//! [`QUIETER`]. They may sit below the range, but never above it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

use soba::dsp::analysis::amp_to_db;
use soba::dsp::loudness::loudness;
use soba::engine::{OfflineRenderer, StereoBuffer, AUDITION};
use soba::persistence::sample_files::SampleBase;
use soba::persistence::{Example, EXAMPLES};

/// The level every example aims for, in LUFS, and how far from it one may sit.
const HOUSE_LUFS: f64 = -16.0;
const TOLERANCE_LU: f64 = 1.5;
/// The highest peak allowed into the output limiter, in dBFS.
const PEAK_CEILING_DB: f32 = -1.0;

const SAMPLE_RATE: f32 = 48_000.0;

/// How long a generative example is heard for, and how many times.
const GENERATIVE_SECONDS: f32 = 150.0;
const HEARINGS: usize = 8;
/// How long a played example is heard for: the audition phrase and its tail.
const PLAYED_SECONDS: f32 = 6.0;

/// The arranged songs, heard once through: (file, bars, BPM).
const SONGS: &[(&str, f32, f32)] = &[
    ("from-one-sine.json", 128.0, 112.0),
    ("clair-de-satellite.json", 93.0, 88.0),
    ("canopy.json", 124.0, 116.0),
];

/// Examples that sit below the house level, and why.
const QUIETER: &[(&str, &str)] = &[
    ("live-looper.json", "it plays what you play into it, as loud as you play it"),
];

/// One hearing of an example: its integrated loudness and the highest
/// peak into the limiter.
struct Hearing {
    seconds: f32,
    lufs: Option<f64>,
    peak_in_db: f32,
}

#[test]
#[ignore = "slow: renders every example at length; run with --release"]
fn every_example_sits_near_the_house_level() {
    let hearings = hear_every_example();

    let range = (HOUSE_LUFS - TOLERANCE_LU)..=(HOUSE_LUFS + TOLERANCE_LU);
    let mut failures = Vec::new();
    println!("{:<20} {:>8} {:>9} {:>11}", "example", "heard", "LUFS", "peak in");
    for (example, hearings) in EXAMPLES.iter().zip(&hearings) {
        let quieter = QUIETER.iter().find(|(file, _)| *file == example.file_name).map(|(_, why)| *why);
        let lufs = energy_mean(hearings.iter().map(|h| h.lufs.unwrap_or(f64::NEG_INFINITY)));
        let peak_in_db = hearings.iter().map(|h| h.peak_in_db).fold(f32::NEG_INFINITY, f32::max);
        let heard = format!("{}×{:.0} s", hearings.len(), hearings[0].seconds);
        println!(
            "{:<20} {:>8} {:>9.1} {:>11.1}{}",
            example.name,
            heard,
            lufs,
            peak_in_db,
            if quieter.is_some() { "   (quieter: see QUIETER)" } else { "" }
        );
        if peak_in_db >= PEAK_CEILING_DB {
            failures.push(format!("{} peaks at {:.1} dBFS into the limiter", example.name, peak_in_db));
        }
        let in_range = match quieter {
            Some(_) => lufs <= *range.end(),
            None => range.contains(&lufs),
        };
        if !in_range {
            failures.push(format!("{} measures {:.1} LUFS, against {:.1} ± {:.1}", example.name, lufs, HOUSE_LUFS, TOLERANCE_LU));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("
"));
}

#[test]
fn every_quieter_example_exists() {
    let files = QUIETER.iter().map(|(file, _)| file).chain(SONGS.iter().map(|(file, _, _)| file));
    for file in files {
        assert!(EXAMPLES.iter().any(|e| e.file_name == *file), "{} isn't an example", file);
    }
}

/// How an example is heard: for how long, how many times, and whether
/// the audition phrase is played into it.
struct Listen {
    seconds: f32,
    hearings: usize,
    played: bool,
}

fn listen_to(example: &Example) -> Listen {
    let patch = example.patch().unwrap();
    let played = patch
        .all_nodes()
        .iter()
        .any(|n| matches!(n.module_id.as_str(), "input.keyboard" | "input.midi_note" | "input.poly_midi"));
    match SONGS.iter().find(|(file, _, _)| *file == example.file_name) {
        Some(&(_, bars, bpm)) => Listen { seconds: bars * 4.0 * 60.0 / bpm, hearings: 1, played },
        None if played => Listen { seconds: PLAYED_SECONDS, hearings: 1, played },
        None => Listen { seconds: GENERATIVE_SECONDS, hearings: HEARINGS, played },
    }
}

/// Hears every example as [`listen_to`] says, on as many threads as there
/// are cores, one render each at a time. Returns each example's hearings,
/// in the order of [`EXAMPLES`].
fn hear_every_example() -> Vec<Vec<Hearing>> {
    let jobs: Vec<(usize, &Example, f32, bool)> = EXAMPLES
        .iter()
        .enumerate()
        .flat_map(|(index, example)| {
            let listen = listen_to(example);
            (0..listen.hearings).map(move |_| (index, example, listen.seconds, listen.played))
        })
        .collect();
    let next = AtomicUsize::new(0);
    let hearings: Mutex<Vec<Vec<Hearing>>> = Mutex::new(EXAMPLES.iter().map(|_| Vec::new()).collect());
    let threads = thread::available_parallelism().map_or(4, |n| n.get());
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while let Some(&(index, example, seconds, played)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let hearing = hear(example, seconds, played);
                    hearings.lock().unwrap()[index].push(hearing);
                }
            });
        }
    });
    hearings.into_inner().unwrap()
}

/// Renders an example once for `seconds`, and measures it.
fn hear(example: &Example, seconds: f32, played: bool) -> Hearing {
    let patch = example.patch().unwrap();
    let (mut renderer, compiled) = OfflineRenderer::from_patch(&patch, SAMPLE_RATE, 256).unwrap();
    let missing = renderer.load_samples(&compiled, SampleBase::Example);
    assert!(missing.is_empty(), "{}: {:?}", example.name, missing);

    if patch.all_nodes().iter().any(|n| n.module_id == "source.audio_input") {
        renderer.set_audio_input(plucked_audition(seconds));
    }
    let audio = if played {
        renderer.render_audition(&patch, &compiled, seconds)
    } else {
        renderer.render_seconds(seconds)
    };
    Hearing {
        seconds,
        lufs: loudness(&audio.left, &audio.right, SAMPLE_RATE).integrated,
        peak_in_db: amp_to_db(renderer.limiter().peak_in),
    }
}

/// The loudness of several hearings together: the mean of their energies,
/// in LUFS.
fn energy_mean(lufs: impl Iterator<Item = f64>) -> f64 {
    let (sum, count) = lufs.fold((0.0, 0), |(sum, count), l| (sum + 10f64.powf(l / 10.0), count + 1));
    10.0 * (sum / count as f64).log10()
}

/// The audition phrase plucked over and over, as a guitar into an Audio
/// Input might play it: each note a decaying tone with an octave above it,
/// peaking about 6 dB below full scale.
fn plucked_audition(seconds: f32) -> StereoBuffer {
    const PHRASE_SECONDS: f32 = 4.0;
    let mut left = vec![0.0_f32; (SAMPLE_RATE * seconds) as usize];
    let mut start_of_phrase = 0.0;
    while start_of_phrase + PHRASE_SECONDS <= seconds {
        for &(note, start, length) in AUDITION {
            let hz = 440.0 * 2f32.powf((note as f32 - 69.0) / 12.0);
            let from = ((start_of_phrase + start) * SAMPLE_RATE) as usize;
            for (n, sample) in left.iter_mut().skip(from).take((length * SAMPLE_RATE) as usize).enumerate() {
                let t = n as f32 / SAMPLE_RATE;
                let phase = std::f32::consts::TAU * hz * t;
                *sample += 0.15 * (-3.0 * t).exp() * (phase.sin() + 0.3 * (2.0 * phase).sin());
            }
        }
        start_of_phrase += PHRASE_SECONDS;
    }
    StereoBuffer { right: left.clone(), left }
}
