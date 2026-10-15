//! A recording is exactly what the device was given: a sine played through
//! a recording session reads back from the WAV sample for sample, with no
//! frame dropped or doubled, through odd device buffer sizes and a stop.

use std::time::{Duration, Instant};

use soba::engine::{AudioProcessor, EngineChannels, EngineCommand, Recording, UiHandle};

const SAMPLE_RATE: u32 = 48000;

/// A sine oscillator into the output module's Mono input, playing.
fn sine_patch(ui: &mut UiHandle) {
    ui.send_command(EngineCommand::AddModule { node_id: 1, module_id: "osc.sine" });
    ui.send_command(EngineCommand::AddModule { node_id: 2, module_id: "output.audio" });
    ui.send_command(EngineCommand::Connect { from_node: 1, from_port: 5, to_node: 2, to_port: 2 });
    ui.send_command(EngineCommand::SetPlaying(true));
}

#[test]
fn recording_reads_back_sample_identical() {
    let (mut ui, engine) = EngineChannels::with_defaults().split();
    let mut processor = AudioProcessor::new(SAMPLE_RATE as f32, 256, engine);
    sine_patch(&mut ui);
    assert!(ui.flush());
    // Let the patch settle in before the take starts
    let mut output = vec![0.0_f32; 1024 * 2];
    processor.process(&mut output[..512 * 2], 2);

    let path = std::env::temp_dir().join("soba-recording-test.wav");
    let (recording, tap) = Recording::start(&path, SAMPLE_RATE, 2).unwrap();
    ui.start_recording(tap);
    assert!(ui.flush());

    // About five seconds (more than the ring holds) in uneven buffers,
    // keeping everything the "device" was given
    let device_buffers = [256, 441, 480, 128, 1024, 64, 1];
    let mut heard = Vec::new();
    let mut round = 0;
    let started = Instant::now();
    while heard.len() < SAMPLE_RATE as usize * 5 * 2 {
        let frames = device_buffers[round % device_buffers.len()];
        processor.process(&mut output[..frames * 2], 2);
        heard.extend_from_slice(&output[..frames * 2]);
        round += 1;
        // Twice real time by the clock, so the writer must keep up. A fixed
        // sleep paced it by how fast process runs, which an optimised build
        // outran until the ring overflowed
        if round % 32 == 0 {
            let played = Duration::from_secs_f64(heard.len() as f64 / 2.0 / SAMPLE_RATE as f64);
            if let Some(wait) = (played / 2).checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
    }

    // Stop: the next callback hands the tap back, and its buffer isn't kept
    ui.stop_recording();
    ui.flush();
    processor.process(&mut output[..256 * 2], 2);
    assert!(!processor.is_recording());
    ui.flush();
    let summary = recording.finish(Duration::from_secs(5));

    assert!(summary.error.is_none(), "{:?}", summary.error);
    assert_eq!(summary.dropped_frames, 0);
    assert_eq!(summary.frames as usize, heard.len() / 2);
    assert!(heard.iter().any(|s| s.abs() > 0.1), "the sine is audible");

    let mut reader = hound::WavReader::open(&path).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, SAMPLE_RATE);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples.len(), heard.len(), "no frame dropped or doubled");
    let first_difference = samples.iter().zip(&heard).position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first_difference, None, "the file matches the output bit for bit");
    std::fs::remove_file(&path).ok();
}

/// A long take at the device's pace loses nothing. Ten minutes by default
/// (`SOBA_RECORD_SECONDS` to change it); run it in a release build:
/// `cargo test --release --test recording -- --ignored`
#[test]
#[ignore = "runs in real time"]
fn long_take_in_real_time_drops_nothing() {
    let seconds: u64 = std::env::var("SOBA_RECORD_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    let (mut ui, engine) = EngineChannels::with_defaults().split();
    let mut processor = AudioProcessor::new(SAMPLE_RATE as f32, 256, engine);
    sine_patch(&mut ui);
    assert!(ui.flush());

    let path = std::env::temp_dir().join("soba-long-take.wav");
    let (recording, tap) = Recording::start(&path, SAMPLE_RATE, 2).unwrap();
    ui.start_recording(tap);
    assert!(ui.flush());

    // 480-frame buffers every 10 ms, on the clock like a device would ask
    const FRAMES: usize = 480;
    let mut output = vec![0.0_f32; FRAMES * 2];
    let start = std::time::Instant::now();
    let callbacks = seconds * SAMPLE_RATE as u64 / FRAMES as u64;
    for n in 0..callbacks {
        let due = start + Duration::from_secs_f64(n as f64 * FRAMES as f64 / SAMPLE_RATE as f64);
        if let Some(wait) = due.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(wait);
        }
        processor.process(&mut output, 2);
        if n % 100 == 0 {
            ui.flush();
        }
    }

    ui.stop_recording();
    ui.flush();
    processor.process(&mut output, 2);
    ui.flush();
    let summary = recording.finish(Duration::from_secs(5));
    eprintln!("{} s recorded, {} frames dropped", summary.duration().as_secs_f64(), summary.dropped_frames);
    assert!(summary.error.is_none(), "{:?}", summary.error);
    assert_eq!(summary.dropped_frames, 0);
    assert_eq!(summary.frames, callbacks * FRAMES as u64);
    std::fs::remove_file(&path).ok();
}
