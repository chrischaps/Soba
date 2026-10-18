//! The plugin end to end, without a DAW: the app made as the plugin makes
//! it, its patch played by the plugin's player with the host's MIDI and
//! transport, its state kept and restored, and its macros followed.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use rtrb::RingBuffer;
use soba::app::SynthApp;
use soba::dsp::HostTransport;
use soba_plugin::player::Player;

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

fn note_allocation() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_allocation();
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Counts allocations and frees made by `f` on this thread.
fn count_allocations<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    COUNTING.with(|c| c.set(true));
    let result = f();
    COUNTING.with(|c| c.set(false));
    (result, ALLOCATIONS.load(Ordering::Relaxed) - before)
}

const RATE: f32 = 48000.0;
const MAX_FRAMES: usize = 512;

/// The plugin as a host first sets it up: the app, its player, and a
/// context to run the app's frames in, as the runner does.
fn plugin() -> (SynthApp, Player, egui::Context) {
    let (ui_midi, heard) = RingBuffer::new(1024);
    let (app, processor) = SynthApp::hosted(RATE, heard);
    let player = Player::new(processor, ui_midi, RATE, MAX_FRAMES);
    let ctx = egui::Context::default();
    soba_plugin::prepare_context(&ctx);
    (app, player, ctx)
}

/// One of the app's frames, as the runner runs them with the editor closed.
fn frame(app: &mut SynthApp, ctx: &egui::Context, time: f64) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Default::default(), egui::vec2(1280.0, 720.0))),
        time: Some(time),
        ..Default::default()
    };
    let _ = ctx.run_ui(input, |ui| app.draw_frame(ui));
}

/// The patch the plugin's state holds.
fn patch_of(state: &str) -> serde_json::Value {
    let state: serde_json::Value = serde_json::from_str(state).unwrap();
    serde_json::from_str(state["patch"].as_str().unwrap()).unwrap()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

#[test]
fn harness_counts_allocations() {
    let (_, counted) = count_allocations(|| {
        std::hint::black_box(Vec::<u8>::with_capacity(16));
    });
    assert_eq!(counted, 2, "one allocation and one free");
}

#[test]
fn clock_follows_the_host() {
    use soba::dsp::{DspModule, ProcessContext, SignalBuffer};
    // The plugin marks the process as hosted before it makes any module
    let _ = plugin();
    let mut clock = soba::modules::Clock::new();
    assert_eq!(clock.parameters()[4].name, "Source");
    clock.prepare(RATE, 256);
    // Tempo knob at 60, Source on the outside clock: the DAW's 120 wins
    let params = [60.0, 50.0, 2.0, 1.0, 2.0, 50.0];
    let mut outputs = vec![SignalBuffer::gate(256); 3];
    let run = |clock: &mut soba::modules::Clock, outputs: &mut Vec<SignalBuffer>, host: HostTransport| {
        let context = ProcessContext::new(RATE, 256).with_host(Some(host));
        clock.process(&[], outputs, &params, &context);
    };

    // Stopped: no beat, wherever the DAW is parked
    run(&mut clock, &mut outputs, HostTransport { playing: false, tempo_bpm: 120.0, beat: 7.5 });
    assert!(outputs[0].samples.iter().all(|&g| g == 0.0), "a gate while the DAW is stopped");

    // Playing from bar 3: Reset fires, and the beat is the DAW's
    run(&mut clock, &mut outputs, HostTransport { playing: true, tempo_bpm: 120.0, beat: 8.0 });
    assert_eq!(outputs[2].samples[0], 1.0, "no Reset as the DAW started");
    assert_eq!(outputs[0].samples[0], 1.0, "the downbeat's gate");
    let readout = clock.readout(&params).unwrap();
    assert_eq!(readout.values[soba::modules::Clock::READOUT_TEMPO], 120.0);
    let transport = clock.transport(&params).unwrap();
    assert!(transport.playing);
    // Half a beat (12000 samples at 120 BPM) after 8.0, with the gate low by then
    run(&mut clock, &mut outputs, HostTransport { playing: true, tempo_bpm: 120.0, beat: 8.5 });
    assert!((clock.transport(&params).unwrap().beat_position.unwrap() - (8.5 + 255.0 / 24000.0)).abs() < 1e-9);
    assert_eq!(outputs[0].samples[0], 0.0, "the gate after its half beat");
}

#[test]
fn plays_host_midi_without_allocating() {
    let (mut app, mut player, ctx) = plugin();
    // The app ships its patch to the audio thread as a frame ends
    frame(&mut app, &ctx, 0.0);

    let mut left = vec![0.0; MAX_FRAMES];
    let mut right = vec![0.0; MAX_FRAMES];
    let mut beat = 0.0;
    let (mut before_note, mut after_note) = (0.0f32, 0.0f32);
    let mut allocations = 0;
    // Hosts' buffers vary: odd sizes, and now and then one too long
    let sizes = [512, 333, 64, 1, 480, 700, 128];
    for buffer in 0..240 {
        let frames = sizes[buffer % sizes.len()];
        left.resize(frames.max(MAX_FRAMES), 0.0);
        right.resize(frames.max(MAX_FRAMES), 0.0);
        let host = HostTransport { playing: true, tempo_bpm: 120.0, beat };
        let (_, counted) = count_allocations(|| {
            // A chord from the DAW, held, then let go
            if buffer == 40 {
                for (i, note) in [60u8, 64, 67].into_iter().enumerate() {
                    player.queue(i as u32 * 7, &[0x90, note, 100]);
                }
                player.queue(3, &[0xB0, 74, 64]);
            }
            if buffer == 200 {
                for note in [60u8, 64, 67] {
                    player.queue(0, &[0x80, note, 0]);
                }
            }
            let mut channels: [&mut [f32]; 2] = [&mut left[..frames], &mut right[..frames]];
            player.play(&mut channels, frames, Some(host));
        });
        allocations += counted;
        let level = peak(&left[..frames]).max(peak(&right[..frames]));
        if buffer < 40 {
            before_note = before_note.max(level);
        } else if buffer < 200 {
            after_note = after_note.max(level);
        }
        beat += frames as f64 * 120.0 / 60.0 / RATE as f64;
        // The app's frames go on beside it, taking back what the audio thread is done with
        if buffer % 8 == 0 {
            frame(&mut app, &ctx, buffer as f64 / 100.0);
        }
    }
    assert_eq!(allocations, 0, "the plugin's audio thread allocated");
    assert!(before_note < 1e-4, "the pad sounded before any note: {before_note}");
    assert!(after_note > 0.01, "the pad didn't play the host's chord: {after_note}");
}

/// One of the app's frames with the editor open and focused, a computer
/// key going down or up in it.
fn key_frame(app: &mut SynthApp, ctx: &egui::Context, time: f64, key: egui::Key, pressed: bool) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Default::default(), egui::vec2(1280.0, 720.0))),
        time: Some(time),
        focused: true,
        events: vec![egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Default::default() }],
        ..Default::default()
    };
    let _ = ctx.run_ui(input, |ui| app.draw_frame(ui));
}

#[test]
fn computer_keys_play_note_in_with_no_midi_device() {
    // The pad's Note In hears the keys, though the plugin has no MIDI
    // engine: they take the UI's own ring to the audio thread
    let (mut app, mut player, ctx) = plugin();
    frame(&mut app, &ctx, 0.0);

    let mut left = vec![0.0; MAX_FRAMES];
    let mut right = vec![0.0; MAX_FRAMES];
    let mut play = |buffers: usize| {
        let mut level = 0.0f32;
        for _ in 0..buffers {
            let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
            player.play(&mut channels, MAX_FRAMES, None);
            level = level.max(peak(&left)).max(peak(&right));
        }
        level
    };
    assert!(play(20) < 1e-4, "the pad sounded before any key");
    key_frame(&mut app, &ctx, 0.1, egui::Key::Z, true);
    assert!(play(40) > 0.01, "the pad didn't play the Z key");
    key_frame(&mut app, &ctx, 0.6, egui::Key::Z, false);
    play(400);
    assert!(play(20) < 1e-3, "the pad kept sounding after the key let go");
}

#[test]
fn draws_in_a_new_context_each_time_the_editor_opens() {
    // The runner's context, then the editor's, closed and opened again: each
    // a context of its own, as the plugin makes them
    let (mut app, _player, runner) = plugin();
    frame(&mut app, &runner, 0.0);
    for opening in 0..3 {
        let editor = egui::Context::default();
        soba_plugin::prepare_context(&editor);
        for n in 0..3 {
            frame(&mut app, &editor, 1.0 + opening as f64 + n as f64 * 0.02);
        }
        frame(&mut app, &runner, 1.5 + opening as f64);
    }
}

#[test]
fn state_round_trips_with_macros() {
    let (mut app, _player, ctx) = plugin();
    frame(&mut app, &ctx, 0.0);
    let state = app.hosted_state();
    let mut patch = patch_of(&state);
    let nodes = patch["nodes"].as_array().unwrap().len();
    assert!(nodes > 3, "the first patch is a whole pad");

    // A project whose filter Cutoff follows Macro 1, over 100-1100 Hz
    let filter = patch["nodes"].as_array().unwrap().iter().find(|n| n["module_id"] == "filter.svf").unwrap()["id"].clone();
    patch["midi_mappings"] = serde_json::json!([{
        "cc_number": 0, "channel": 0, "node_id": filter, "param_index": 0, "param_name": "Cutoff",
        "min_value": 100.0, "max_value": 1100.0, "macro": 0
    }]);
    let mut saved: serde_json::Value = serde_json::from_str(&state).unwrap();
    saved["patch"] = serde_json::Value::String(patch.to_string());

    let (mut restored, _player, ctx) = plugin();
    restored.restore_hosted_state(&saved.to_string()).unwrap();
    frame(&mut restored, &ctx, 0.0);
    restored.follow_macros(&[0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    frame(&mut restored, &ctx, 0.1);

    let kept = patch_of(&restored.hosted_state());
    assert_eq!(kept["nodes"].as_array().unwrap().len(), nodes);
    let cutoff = kept["nodes"].as_array().unwrap().iter().find(|n| n["module_id"] == "filter.svf").unwrap()["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Cutoff")
        .unwrap()["value"]
        .as_f64()
        .unwrap();
    assert!((cutoff - 600.0).abs() < 1.0, "Macro 1 at half turned Cutoff to {cutoff}, not 600");
    // The mapping is kept with the project
    assert_eq!(kept["midi_mappings"][0]["macro"], 0);
}
