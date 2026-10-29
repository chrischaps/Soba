//! The audio callback must never allocate.
//!
//! A counting global allocator watches `AudioProcessor::process` while it
//! plays a patch of more than 30 modules (every built-in module, some
//! twice), through odd device buffer sizes, while the patch is edited and
//! its Clock changes tempo under a tempo-synced Delay and its effects are
//! bypassed and brought back, live MIDI plays its Note In modules, a live
//! audio input feeds its Audio Input module (the input device switched
//! midway to a twelve-channel one), the output is recorded (one take stopped and a new one
//! started midway), its Sampler is given new recordings while it plays
//! them, and its Looper is taken round its whole cycle again and again:
//! record, overdub, undo, redo, stop, restart, clear, while its loop is
//! copied out to be saved and a saved loop is loaded back into it. Edits are compiled on the "UI" side (outside the counted
//! region); installing them happens inside it. The input device's callback
//! is counted too, and so is SysEx sent from the audio thread while slots
//! of the pool are in flight (#440).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use soba::dsp::{InputAudio, ListenTo, MidiEvent as DspMidiEvent, SampleData, Snapshot, SnapshotOutcome};

use soba::engine::{
    Probe, ProbeScope, SCOPE_WINDOW,
    create_module_registry, input_channel, input_channel_converting, input_channel_same_clock, AudioProcessor, EngineChannels, EngineCommand, EngineEvent, MidiEvent,
    NodeId, Recording, TimestampedMidiEvent, UiHandle,
};
use soba::modules::looper::{LoopState, Looper};
use soba::dsp::primitives::{Markers, ONSETS};
use soba::hardware::{Address, Parameter, SlotMap, Step};
use soba::modules::{Chopper, Cloud, Controls, MidiOut, Mixer, Pads, Resonator};

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
fn count_allocations(f: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

/// Port layout of a registered module: (input port indices, output port indices).
fn ports(module_id: &str) -> (Vec<usize>, Vec<usize>) {
    let module = create_module_registry().create(module_id).unwrap();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for (index, port) in module.ports().iter().enumerate() {
        if port.is_input() {
            inputs.push(index);
        } else {
            outputs.push(index);
        }
    }
    (inputs, outputs)
}

/// The patch size the guard plays: every module, the first few twice.
const PATCH_SIZE: usize = 49;

/// Adds `count` modules, cycling through every built-in module in
/// registration order, chained output-to-input, with every port monitored.
fn build_big_patch(ui: &mut UiHandle, count: usize) -> Vec<(NodeId, &'static str)> {
    let registry = create_module_registry();
    let ids: Vec<&'static str> = registry.list_modules().iter().map(|info| info.id).collect();

    let mut nodes = Vec::new();
    let mut previous_output: Option<(NodeId, usize)> = None;
    for (n, module_id) in ids.into_iter().cycle().take(count).enumerate() {
        let node_id = n as NodeId + 1;
        ui.send_command(EngineCommand::AddModule { node_id, module_id });

        let (inputs, outputs) = ports(module_id);
        if let (Some((from_node, from_port)), Some(&to_port)) = (previous_output, inputs.first()) {
            ui.send_command(EngineCommand::Connect { from_node, from_port, to_node: node_id, to_port });
        }
        for input_index in 0..inputs.len() {
            ui.send_command(EngineCommand::MonitorInput { node_id, input_index });
        }
        for output_index in 0..outputs.len() {
            ui.send_command(EngineCommand::MonitorOutput { node_id, output_index });
        }
        if let Some(&port) = outputs.first() {
            previous_output = Some((node_id, port));
        }
        nodes.push((node_id, module_id));
    }
    nodes
}

/// The first node running `module_id`.
fn node_of(nodes: &[(NodeId, &'static str)], module_id: &str) -> NodeId {
    nodes.iter().find(|(_, id)| *id == module_id).map(|&(node_id, _)| node_id).unwrap()
}

#[test]
fn harness_counts_allocations() {
    let counted = count_allocations(|| {
        std::hint::black_box(Vec::<u8>::with_capacity(16));
    });
    assert_eq!(counted, 2, "one allocation and one free");
}

#[test]
fn audio_callback_never_allocates() {
    // MIDI Outs and the patch's MIDI clock get a ring to write to, as in
    // the app. Nothing drains them here, so they fill, and writing to a
    // full ring is tried too
    soba::link::midi::MIDI.start_draining();
    soba::link::midi::MIDI.set_clock(0, soba::link::midi::ClockMode::Transport);
    let (mut ui, engine) = EngineChannels::with_defaults().split();
    let mut processor = AudioProcessor::new(48000.0, 256, engine);
    let (mut midi, midi_input) = rtrb::RingBuffer::new(512);
    processor.set_midi_input(midi_input);
    let nodes = build_big_patch(&mut ui, PATCH_SIZE);
    assert!(nodes.len() >= 30);

    // Sync every Delay to the patch tempo, which the first Clock sets
    let clock = node_of(&nodes, "util.clock");
    for &(node_id, _) in nodes.iter().filter(|(_, id)| *id == "fx.delay") {
        ui.send_command(EngineCommand::SetParameter { node_id, param_index: 6, value: 2.0 });
    }
    ui.send_command(EngineCommand::SetPlaying(true));

    // A Sampler playing on the Clock's beat, given a new recording every so
    // often, mid-note or not. The test keeps no copy, so if the audio thread
    // dropped a recording it replaced, freeing it would count
    let sampler = node_of(&nodes, "source.sampler");
    ui.send_command(EngineCommand::Connect {
        from_node: clock,
        from_port: port("util.clock", "Gate"),
        to_node: sampler,
        to_port: port("source.sampler", "Gate"),
    });
    let tone = |hz: f32| {
        let samples: Vec<f32> = (0..24000).map(|n| (n as f32 * hz / 48000.0 * std::f32::consts::TAU).sin() * 0.5).collect();
        Some(Arc::new(SampleData::stereo(samples.clone(), samples, 44100.0)))
    };
    ui.send_command(EngineCommand::LoadSample { node_id: sampler, sample: tone(220.0) });
    let mut loads = 1;

    // A Dancer listening to the Sampler, so it hunts for a beat in it
    ui.send_command(EngineCommand::Connect {
        from_node: sampler,
        from_port: port("source.sampler", "L"),
        to_node: node_of(&nodes, "util.dancer"),
        to_port: port("util.dancer", "Audio"),
    });

    // A Link Out sending an LFO and the Clock's gate to Seoul
    let link = node_of(&nodes, "output.link");
    ui.send_command(EngineCommand::Connect {
        from_node: node_of(&nodes, "mod.lfo"),
        from_port: 0,
        to_node: link,
        to_port: port("output.link", "CV 1"),
    });
    ui.send_command(EngineCommand::Connect {
        from_node: clock,
        from_port: port("util.clock", "Gate"),
        to_node: link,
        to_port: port("output.link", "Gate 1"),
    });

    // A MIDI Out playing the Clock's gate and an LFO as a CC. Port A sends
    // the Clock as MIDI clock (set above)
    let midi_out = node_of(&nodes, "output.midi");
    ui.send_command(EngineCommand::Connect {
        from_node: clock,
        from_port: port("util.clock", "Gate"),
        to_node: midi_out,
        to_port: port("output.midi", "Gate"),
    });
    ui.send_command(EngineCommand::Connect {
        from_node: node_of(&nodes, "mod.lfo"),
        from_port: 0,
        to_node: midi_out,
        to_port: port("output.midi", "CC 1"),
    });
    // ...and the LFO as the bend, the pressure, a 14-bit CC and an NRPN,
    // with a program, a bank and a bend range to send (#439)
    for input in ["Bend", "Pressure", "CC 2", "CC 3"] {
        ui.send_command(EngineCommand::Connect {
            from_node: node_of(&nodes, "mod.lfo"),
            from_port: 0,
            to_node: midi_out,
            to_port: port("output.midi", input),
        });
    }
    for (param_index, value) in [
        (MidiOut::PARAM_CC_MODE + 1, 1.0),
        (MidiOut::PARAM_CC_MODE + 2, 2.0),
        (MidiOut::PARAM_NRPN + 2, 148.0),
        (MidiOut::PARAM_PROGRAM, 4.0),
        (MidiOut::PARAM_BANK_MSB, 87.0),
        (MidiOut::PARAM_BEND_RANGE, 12.0),
    ] {
        ui.send_command(EngineCommand::SetParameter { node_id: midi_out, param_index, value });
    }

    // An Instrument (#442) playing the Clock's gate, its first knob a CC
    // the LFO sweeps, its second a 14-bit pair of the device's own and its
    // third a stepped one. Below, its knobs are mapped again mid-run, as
    // Send all does
    let instrument = node_of(&nodes, "source.instrument");
    ui.send_command(EngineCommand::Connect {
        from_node: clock,
        from_port: port("util.clock", "Gate"),
        to_node: instrument,
        to_port: port("source.instrument", "Gate"),
    });
    ui.send_command(EngineCommand::Connect {
        from_node: node_of(&nodes, "mod.lfo"),
        from_port: 0,
        to_node: instrument,
        to_port: port("source.instrument", "Slot 1"),
    });
    let knobs = instrument_knobs();
    for (slot, map) in knobs.iter().enumerate() {
        ui.send_command(EngineCommand::MapSlot { node_id: instrument, slot, map: *map });
    }

    // A Controls (#427) hearing the mod wheel, the bend and the pressure
    // played below, a fader on CC 74 swinging -1 to 1, and an endless
    // encoder on CC 20 counting up and down. Its A cable runs on down the
    // chain, as every module's first output does
    let controls = node_of(&nodes, "input.controls");
    ui.send_command(EngineCommand::SetParameter { node_id: controls, param_index: Controls::cc_param(2), value: 20.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: controls, param_index: Controls::source_param(2), value: 0.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: controls, param_index: Controls::encoding_param(2), value: 1.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: controls, param_index: Controls::range_param(3), value: 2.0 });

    // Pads (#428) on channel 1 from note 48, so the notes played below hit
    // them, and claim them from every Note In on Omni; pad 8 on its own
    // note on channel 10. Its mode and Base move below, claims and all
    let pads = node_of(&nodes, "input.pads");
    ui.send_command(EngineCommand::SetParameter { node_id: pads, param_index: Pads::PARAM_BASE, value: 48.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: pads, param_index: Pads::PARAM_CHANNEL, value: 1.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: pads, param_index: Pads::note_param(7), value: 36.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: pads, param_index: Pads::channel_param(7), value: 10.0 });

    // Two devices play (#435): the Controls hears the first, the Pads the
    // second, picked out of the block's MIDI each callback
    ui.send_command(EngineCommand::Listen { node_id: controls, listen: ListenTo::Port(0), channel: None });
    ui.send_command(EngineCommand::Listen { node_id: pads, listen: ListenTo::Port(1), channel: None });

    // A Chopper on the same Clock, cut at markers, given a new loop (and
    // new markers) every so often, mid-slice or not
    let chopper = node_of(&nodes, "source.chopper");
    ui.send_command(EngineCommand::Connect {
        from_node: clock,
        from_port: port("util.clock", "Gate"),
        to_node: chopper,
        to_port: port("source.chopper", "Clock"),
    });
    ui.send_command(EngineCommand::SetParameter { node_id: chopper, param_index: Chopper::PARAM_SLICES, value: ONSETS });
    ui.send_command(EngineCommand::SetParameter { node_id: chopper, param_index: Chopper::step_param(1), value: -101_200.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: chopper, param_index: Chopper::step_param(2), value: 314_005.0 });
    ui.send_command(EngineCommand::SetParameter { node_id: chopper, param_index: Chopper::chance_param(3), value: 50.0 });
    ui.send_command(EngineCommand::LoadSample { node_id: chopper, sample: tone(110.0) });
    ui.send_command(EngineCommand::SetMarkers { node_id: chopper, markers: Markers::between(&[0.2, 0.45, 0.7], 0.0, 1.0) });

    // A Cloud at its densest, pitched up through its grains' lowpasses,
    // playing a file that's replaced mid-grain now and then, and the live
    // input (patched in and out), frozen and let go
    let cloud = node_of(&nodes, "source.cloud");
    let cloud_param = |param_index: usize, value: f32| EngineCommand::SetParameter { node_id: cloud, param_index, value };
    ui.send_command(cloud_param(Cloud::PARAM_DENSITY, 200.0));
    ui.send_command(cloud_param(Cloud::PARAM_SIZE, 60.0));
    ui.send_command(cloud_param(Cloud::PARAM_PITCH, 19.0));
    ui.send_command(cloud_param(Cloud::PARAM_SPRAY, 1.0));
    ui.send_command(EngineCommand::LoadSample { node_id: cloud, sample: tone(330.0) });
    let audio_input = node_of(&nodes, "source.audio_input");
    let cloud_in = || EngineCommand::Connect { from_node: audio_input, from_port: 0, to_node: cloud, to_port: port("source.cloud", "In L") };

    // A Resonator plucked by the Clock, rung by a Drum at the same time,
    // at its longest and brightest, its pitch on an LFO; it swaps between
    // String and Body and through the materials as it plays
    let resonator = node_of(&nodes, "source.resonator");
    let resonator_param = |param_index: usize, value: f32| EngineCommand::SetParameter { node_id: resonator, param_index, value };
    ui.send_command(EngineCommand::Connect { from_node: clock, from_port: port("util.clock", "Gate"), to_node: resonator, to_port: port("source.resonator", "Gate") });
    ui.send_command(EngineCommand::Connect { from_node: node_of(&nodes, "source.drum"), from_port: 0, to_node: resonator, to_port: port("source.resonator", "In") });
    ui.send_command(EngineCommand::Connect { from_node: node_of(&nodes, "mod.lfo"), from_port: 0, to_node: resonator, to_port: port("source.resonator", "V/Oct") });
    ui.send_command(resonator_param(Resonator::PARAM_DECAY, 1.0));
    ui.send_command(resonator_param(Resonator::PARAM_DAMPING, 0.0));
    ui.send_command(resonator_param(Resonator::PARAM_BRIGHTNESS, 1.0));

    // A Looper fed by the live input (with a round trip to take off its
    // overdubs), pressed through its cycle by its footswitches
    let looper = node_of(&nodes, "util.looper");
    ui.send_command(EngineCommand::Connect {
        from_node: node_of(&nodes, "source.audio_input"),
        from_port: 0,
        to_node: looper,
        to_port: port("util.looper", "In L"),
    });
    ui.set_input_latency(1500);
    let pedal = |pedal: usize, down: bool| EngineCommand::SetParameter {
        node_id: looper,
        param_index: Looper::PARAM_PEDALS + pedal,
        value: if down { 1.0 } else { 0.0 },
    };
    // (round within each 120, footswitch): Rec, Stop, Undo, Clear
    let presses = [(5, 0), (30, 0), (45, 0), (70, 0), (78, 2), (84, 2), (90, 1), (96, 1), (102, 0), (106, 2), (112, 3)];
    let mut looper_states = std::collections::HashSet::new();
    // Its loop copied out to save, into room set aside here, and a saved
    // loop loaded back into it while it runs
    let saved_loop = |hz: f32| {
        let samples: Vec<f32> = (0..30000).map(|n| (n as f32 * hz / 48000.0 * std::f32::consts::TAU).sin() * 0.3).collect();
        Some(Arc::new(SampleData::stereo(samples.clone(), samples, 48000.0)))
    };
    let mut snapshots_done = 0;
    let mut loops_loaded = 0;

    // A cable probed, moved from one module's output to the next (some of
    // them being swapped out meanwhile), and let go of now and then. The
    // scope is kept here, so the audio thread never frees one
    let scope = Arc::new(ProbeScope::new());

    // Record the whole run: the tap copies every callback into its ring
    let takes = std::env::temp_dir().join("soba-realtime-alloc");
    std::fs::create_dir_all(&takes).unwrap();
    let (mut recording, tap) = Recording::start(&takes.join("take1.wav"), 48000, 2).unwrap();
    ui.start_recording(tap);

    // A stereo input device delivering 480-frame buffers of a tone
    let (mut input, feed, mut monitor) = input_channel(48000);
    ui.connect_input(feed);
    let device_input: Vec<f32> = (0..480).flat_map(|n| [(n as f32 * 0.13).sin() * 0.5; 2]).collect();
    // ...and the twelve-channel interface it's switched to (#450)
    let wide_input: Vec<f32> = (0..480).flat_map(|n| [(n as f32 * 0.13).sin() * 0.5; 12]).collect();
    let mut device_channels = 2;
    let mut input_due = 0usize;
    assert!(ui.flush());

    // Typical WASAPI/CoreAudio sizes, including ones larger than a block
    let device_buffers = [256, 441, 480, 128, 1024, 64];
    let mut output = vec![0.0_f32; 1024 * 2];
    let mut blocks = 0;
    let mut allocations = 0;

    for round in 0..1000 {
        let frames = device_buffers[round % device_buffers.len()];

        // Midway, switch input devices, to a mixer that brings each of its
        // twelve channels: the old feed comes back to be dropped here, the
        // new one goes in
        if round == 300 {
            let (next, feed, next_monitor) = input_channel_converting(48000, 48000, 12);
            device_channels = 12;
            input = next;
            monitor = next_monitor;
            ui.connect_input(feed);
            ui.flush();
        }

        // Midway, end the take and start another: the old tap comes back to
        // be dropped here, the new one goes in
        if round == 500 {
            ui.stop_recording();
            let (next, tap) = Recording::start(&takes.join("take2.wav"), 48000, 2).unwrap();
            ui.start_recording(tap);
            let first = std::mem::replace(&mut recording, next);
            ui.flush();
            allocations += count_allocations(|| processor.process(&mut output[..frames * 2], 2));
            ui.flush();
            let summary = first.finish(std::time::Duration::from_secs(5));
            assert!(summary.frames > 0 && summary.error.is_none());
        }

        // Edit the patch while it plays: knob moves and tempo changes every
        // block, and a module swapped out every 50 blocks
        ui.send_command(EngineCommand::SetParameter {
            node_id: 1,
            param_index: 0,
            value: 220.0 + round as f32,
        });
        ui.send_command(EngineCommand::SetParameter {
            node_id: clock,
            param_index: 0,
            value: 60.0 + (round % 120) as f32,
        });
        // Every few blocks, bypass every filter and effect or bring them all
        // back: crossfades, fully bypassed rests, and resets on return
        if round % 8 == 3 {
            for &(node_id, _) in &nodes {
                ui.send_command(EngineCommand::SetBypass { node_id, bypassed: round % 16 == 3 });
            }
        }
        for &(at, which) in &presses {
            if round % 120 == at {
                ui.send_command(pedal(which, true));
            }
            if round % 120 == at + 2 {
                ui.send_command(pedal(which, false));
            }
        }
        if round % 37 == 11 {
            ui.send_command(EngineCommand::LoadSample { node_id: sampler, sample: tone(220.0 + round as f32) });
            loads += 1;
        }
        if round % 41 == 17 {
            ui.send_command(EngineCommand::LoadSample { node_id: chopper, sample: tone(110.0 + round as f32) });
            let place = (round % 9) as f32 / 10.0 + 0.05;
            ui.send_command(EngineCommand::SetMarkers { node_id: chopper, markers: Markers::between(&[place, 0.5, 0.96], 0.0, 1.0) });
        }
        if round % 43 == 13 {
            ui.send_command(EngineCommand::LoadSample { node_id: cloud, sample: tone(330.0 + round as f32) });
        }
        if round % 29 == 7 {
            for (slot, map) in knobs.iter().enumerate() {
                ui.send_command(EngineCommand::MapSlot { node_id: instrument, slot, map: *map });
            }
        }
        match round % 200 {
            50 => ui.send_command(cloud_in()),
            110 => ui.send_command(cloud_param(Cloud::PARAM_FREEZE, 1.0)),
            140 => ui.send_command(cloud_param(Cloud::PARAM_FREEZE, 0.0)),
            170 => ui.send_command(EngineCommand::Disconnect { node_id: cloud, port: port("source.cloud", "In L"), is_input: true }),
            _ => {}
        }
        if round % 37 == 5 {
            ui.send_command(resonator_param(Resonator::PARAM_MODE, ((round / 37) % 2) as f32));
            ui.send_command(resonator_param(Resonator::PARAM_MATERIAL, ((round / 74) % 4) as f32));
        }
        if round % 40 == 35 {
            ui.request_snapshot(looper, Box::new(Snapshot::with_capacity(48000 * 4)));
        }
        if round % 240 == 115 {
            ui.send_command(EngineCommand::LoadSample { node_id: looper, sample: saved_loop(110.0 + round as f32) });
            loops_loaded += 1;
        }
        if round % 50 == 25 {
            let (node_id, module_id) = nodes[round / 50 % nodes.len()];
            ui.send_command(EngineCommand::RemoveModule { node_id });
            ui.send_command(EngineCommand::AddModule { node_id, module_id });
        }
        if round % 40 == 13 {
            // A pad pressed on the face or let go, a new mode, a new Base
            let pad = round / 40 % 8;
            ui.send_command(EngineCommand::SetParameter { node_id: pads, param_index: Pads::press_param(pad), value: (round % 80 / 40) as f32 * 0.7 });
            ui.send_command(EngineCommand::SetParameter { node_id: pads, param_index: Pads::PARAM_MODE, value: (round / 40 % 3) as f32 });
            ui.send_command(EngineCommand::SetParameter { node_id: pads, param_index: Pads::PARAM_BASE, value: 44.0 + (round / 40 % 8) as f32 });
        }
        if round % 30 == 7 {
            let probe = (round % 90 != 7).then(|| {
                let (node_id, _) = nodes[round / 30 % nodes.len()];
                Probe { node_id, output_index: round / 7 % 2, scope: scope.clone(), duck: Default::default() }
            });
            ui.probe(probe);
        }
        // Play: notes on and off every block, with bends and pressure. The
        // queue would overflow (and the push panic) unless the callback drains it
        let note = 48 + (round % 24) as u8;
        let events = [
            MidiEvent::NoteOn { channel: 0, note, velocity: 100 },
            MidiEvent::PitchBend { channel: 0, value: (round as i16 % 64) * 128 - 4096 },
            MidiEvent::ChannelPressure { channel: 0, pressure: (round % 128) as u8 },
            MidiEvent::NoteOff { channel: 0, note: note.wrapping_sub(3), velocity: 0 },
            // The faders, the mod wheel and an encoder, for Controls
            MidiEvent::ControlChange { channel: 0, controller: 74, value: (round * 3 % 128) as u8 },
            MidiEvent::ControlChange { channel: 0, controller: 1, value: (round % 128) as u8 },
            MidiEvent::ControlChange { channel: 0, controller: 20, value: if round % 3 == 0 { 63 } else { 65 } },
            // How hard a held pad is pressed, and pad 8 on channel 10
            MidiEvent::PolyPressure { channel: 0, note: note.wrapping_sub(1), pressure: (round * 5 % 128) as u8 },
            if round % 2 == 0 { MidiEvent::NoteOn { channel: 9, note: 36, velocity: 90 } } else { MidiEvent::NoteOff { channel: 9, note: 36, velocity: 0 } },
        ];
        // From two devices, turn about
        for (n, event) in events.into_iter().enumerate() {
            midi.push(TimestampedMidiEvent::heard((n % 2) as u8, event)).unwrap();
        }
        // The Pads' device unplugged and plugged back in, every so often,
        // and patched from a MIDI In on channel 10, its claims narrowed to
        // that channel (#479)
        if round % 40 == 13 {
            let listen = if round % 80 == 13 { ListenTo::Unplugged } else { ListenTo::Port(1) };
            let channel = (round % 120 == 53).then_some(9);
            ui.send_command(EngineCommand::Listen { node_id: pads, listen, channel });
        }
        ui.flush();
        // Snapshots coming back are dropped here, off the audio thread
        while let Some((_, snapshot)) = ui.take_snapshot() {
            if snapshot.outcome() == SnapshotOutcome::Done {
                snapshots_done += 1;
            }
        }
        for event in ui.drain_events() {
            if let EngineEvent::Readout { node_id, readout } = event {
                if node_id == looper {
                    looper_states.insert(format!("{:?}", LoopState::from_code(readout.values[Looper::READOUT_STATE])));
                }
            }
        }

        // The input device keeps time with the output, 480 frames at a time
        input_due += frames;
        while input_due >= 480 {
            allocations += count_allocations(|| {
                input.record_latency(Some(std::time::Duration::from_micros(10_000 + round as u64)));
                match device_channels {
                    2 => input.push_f32(&device_input, 2),
                    _ => input.push_f32(&wide_input, 12),
                }
            });
            input_due -= 480;
        }

        allocations += count_allocations(|| processor.process(&mut output[..frames * 2], 2));
        blocks += 1;
    }

    assert_eq!(blocks, 1000);
    assert!(processor.is_playing());
    assert_eq!(processor.plan().len(), nodes.len());
    assert!(processor.plan().tempo_bpm().is_some(), "the Clock sets the patch tempo");
    assert!(processor.is_recording());
    assert!(recording.elapsed().as_secs_f32() > 1.0, "the second take heard half the run");
    assert!(processor.has_input());
    assert!(monitor.buffered_frames() > 0, "the second input was read");
    assert_eq!(monitor.overflow_frames(), 0);
    assert!(monitor.device_latency().is_some(), "the input's timestamps were taken in");
    assert!(output.iter().all(|s| s.is_finite()));
    assert!(loads > 25);
    assert!(snapshots_done > 5, "{snapshots_done} snapshots of a loop came back");
    assert!(loops_loaded >= 4);
    assert!(scope.read(&mut [0.0; SCOPE_WINDOW]) > 10_000, "the probe heard the patch");
    for state in ["Recording", "Playing", "Overdubbing", "Stopped", "Empty"] {
        assert!(looper_states.contains(state), "the Looper never reached {state}: {looper_states:?}");
    }
    assert_eq!(allocations, 0, "audio callback allocated {allocations} times over {blocks} callbacks");
}

/// The ASIO path: one driver runs the input's callback and then the
/// output's at each buffer switch, in small buffers, and the output takes
/// 32-bit integers, converted from the patch's floats through a scratch
/// buffer.
#[test]
fn asio_callback_never_allocates() {
    let (mut ui, engine) = EngineChannels::with_defaults().split();
    let mut processor = AudioProcessor::new(48000.0, 256, engine);
    let nodes = build_big_patch(&mut ui, PATCH_SIZE);
    ui.send_command(EngineCommand::SetPlaying(true));

    let (mut input, feed, mut monitor) = input_channel_same_clock(48000, true, 2);
    ui.connect_input(feed);
    assert!(ui.flush());

    // Made when the stream is built, as the engine does
    let mut scratch = vec![0.0_f32; 4096 * 2];
    let mut output = vec![0_i32; 128 * 2];
    let mut allocations = 0;

    for round in 0..2000 {
        let frames = if round < 1000 { 64 } else { 128 };

        // The buffer size changes from the driver's panel, and the input
        // opens again on the new one
        if round == 1000 {
            let (next, feed, next_monitor) = input_channel_same_clock(48000, true, 2);
            input = next;
            monitor = next_monitor;
            ui.connect_input(feed);
            ui.flush();
        }
        ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 0, value: 220.0 + round as f32 });
        if round % 50 == 25 {
            let (node_id, module_id) = nodes[round / 50 % nodes.len()];
            ui.send_command(EngineCommand::RemoveModule { node_id });
            ui.send_command(EngineCommand::AddModule { node_id, module_id });
        }
        ui.flush();
        ui.drain_events().for_each(drop);

        let device_input: Vec<f32> = (0..frames).flat_map(|n| [(n as f32 * 0.13).sin() * 0.5; 2]).collect();
        let output = &mut output[..frames * 2];
        allocations += count_allocations(|| {
            input.record_latency(Some(std::time::Duration::from_micros(2_700)));
            input.push_f32(&device_input, 2);
            if round % 100 == 0 {
                // The driver reports an overload now and then
                monitor.mark_xrun();
            }
            processor.process_into(output, &mut scratch, 2)
        });
    }

    assert!(processor.has_input());
    assert_eq!(monitor.underrun_frames(), 0, "a shared clock never runs dry");
    assert_eq!(monitor.overflow_frames(), 0);
    assert_eq!(monitor.target_frames(), 128, "one buffer");
    assert_eq!(monitor.buffered_frames(), 0, "read as it arrives");
    assert_eq!(monitor.device_xruns(), 10);
    assert_eq!(allocations, 0, "ASIO callback allocated {allocations} times");
}

/// Index of the port called `name` on a registered module.
/// An Instrument's knobs: a CC, a 14-bit pair on two CCs of the device's
/// own (the Digitakt's LFO depth), and a stepped one with values of its own.
fn instrument_knobs() -> [SlotMap; 3] {
    let cutoff = Parameter::cc("Cutoff", 23);
    let mut depth = Parameter::cc("LFO1 Depth", 109);
    depth.address = Address::Cc14 { msb: 109, lsb: 61 };
    depth.range = (0, 16383);
    let mut arp = Parameter::cc("Arp Type", 49);
    arp.values = vec![Step::At { label: "Off".into(), value: 6 }, Step::At { label: "Rise".into(), value: 19 }];
    [SlotMap::of(&cutoff), SlotMap::of(&depth), SlotMap::of(&arp)]
}

fn port(module_id: &str, name: &str) -> usize {
    let module = create_module_registry().create(module_id).unwrap();
    module.ports().iter().position(|p| p.name == name).unwrap_or_else(|| panic!("{module_id} has no {name}"))
}

#[test]
fn poly_patch_never_allocates_with_eight_voices() {
    let (mut ui, engine) = EngineChannels::with_defaults().split();
    let mut processor = AudioProcessor::new(48000.0, 256, engine);
    let (mut midi, midi_input) = rtrb::RingBuffer::new(512);
    processor.set_midi_input(midi_input);
    // The computer keys play along, on the UI's own ring
    let (mut keys, keys_input) = soba::engine::ui_notes();
    processor.set_ui_midi_input(keys_input);

    // Note In -> Slope (a glide per voice) -> osc -> SVF -> ladder -> VCA
    // (opened by an envelope) -> mixer -> chained mixer -> delay -> output.
    // Everything up to the VCA runs 8 voices; the mixer spreads them across
    // its stereo pair, passes its mix on a four-strand Bus to the next, and
    // the delay hears that
    let chain: [(NodeId, &'static str); 11] = [
        (1, "input.poly_midi"),
        (2, "osc.sine"),
        (3, "filter.svf"),
        (4, "filter.ladder"),
        (5, "mod.adsr"),
        (6, "util.vca"),
        (7, "util.mixer"),
        (8, "fx.delay"),
        (9, "output.audio"),
        (10, "util.mixer"),
        (11, "mod.slope"),
    ];
    for (node_id, module_id) in chain {
        ui.send_command(EngineCommand::AddModule { node_id, module_id });
    }
    fn connect(ui: &mut UiHandle, from: (NodeId, &str, &str), to: (NodeId, &str, &str)) {
        ui.send_command(EngineCommand::Connect {
            from_node: from.0,
            from_port: port(from.1, from.2),
            to_node: to.0,
            to_port: port(to.1, to.2),
        });
    }
    connect(&mut ui, (1, "input.poly_midi", "Pitch"), (11, "mod.slope", "In"));
    connect(&mut ui, (11, "mod.slope", "Out"), (2, "osc.sine", "V/Oct"));
    connect(&mut ui, (1, "input.poly_midi", "Gate"), (5, "mod.adsr", "Gate"));
    connect(&mut ui, (1, "input.poly_midi", "Velocity"), (5, "mod.adsr", "Velocity"));
    connect(&mut ui, (2, "osc.sine", "Out"), (3, "filter.svf", "In"));
    connect(&mut ui, (5, "mod.adsr", "Out"), (3, "filter.svf", "Cutoff"));
    connect(&mut ui, (3, "filter.svf", "LowPass"), (4, "filter.ladder", "In"));
    connect(&mut ui, (4, "filter.ladder", "LP24"), (6, "util.vca", "In"));
    connect(&mut ui, (5, "mod.adsr", "Out"), (6, "util.vca", "CV"));
    connect(&mut ui, (6, "util.vca", "Out"), (7, "util.mixer", "Ch 1"));
    connect(&mut ui, (7, "util.mixer", "Chain Out"), (10, "util.mixer", "Chain In"));
    connect(&mut ui, (10, "util.mixer", "Out L"), (8, "fx.delay", "In L"));
    connect(&mut ui, (10, "util.mixer", "Out R"), (8, "fx.delay", "In R"));
    // Full width, and the envelope sweeping channel 1's pan
    ui.send_command(EngineCommand::SetParameter { node_id: 7, param_index: Mixer::PARAM_WIDTH, value: 1.0 });
    connect(&mut ui, (5, "mod.adsr", "Out"), (7, "util.mixer", "Pan 1"));
    connect(&mut ui, (8, "fx.delay", "Out L"), (9, "output.audio", "Left"));
    connect(&mut ui, (8, "fx.delay", "Out R"), (9, "output.audio", "Right"));
    for node_id in 1..=11 {
        for index in 0..4 {
            ui.send_command(EngineCommand::MonitorInput { node_id, input_index: index });
            ui.send_command(EngineCommand::MonitorOutput { node_id, output_index: index });
        }
    }
    // The Bus between the mixers is watched too, as a cable drawing its strands
    ui.send_command(EngineCommand::MonitorOutput { node_id: 7, output_index: Mixer::CHAIN_OUT });
    ui.send_command(EngineCommand::SetPlaying(true));
    assert!(ui.flush());

    let device_buffers = [256, 441, 480, 128, 1024, 64];
    let mut output = vec![0.0_f32; 1024 * 2];
    let mut allocations = 0;

    for round in 0..1000 {
        let frames = device_buffers[round % device_buffers.len()];

        // Ten-note clusters on 8 voices: every round steals. The pedal comes
        // and goes, and the voice count drops and comes back
        let root = 36 + (round % 48) as u8;
        for k in 0..10 {
            let event = MidiEvent::NoteOn { channel: 0, note: root + k * 2, velocity: 60 + k * 6 };
            midi.push(TimestampedMidiEvent::now(event)).unwrap();
        }
        let pedal = if round % 20 < 10 { 127 } else { 0 };
        midi.push(TimestampedMidiEvent::now(MidiEvent::ControlChange { channel: 0, controller: 64, value: pedal }))
            .unwrap();
        for k in 0..10 {
            let event = MidiEvent::NoteOff { channel: 0, note: root.wrapping_sub(2) + k * 2, velocity: 0 };
            midi.push(TimestampedMidiEvent::now(event)).unwrap();
        }
        keys.note(72 + (round % 12) as u8, round % 3 != 2);
        if round % 100 == 50 {
            ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 1, value: 3.0 });
        }
        if round % 100 == 70 {
            ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 1, value: 8.0 });
        }
        if round % 8 == 3 {
            ui.send_command(EngineCommand::SetBypass { node_id: 3, bypassed: round % 16 == 3 });
        }
        if round % 250 == 125 {
            // Rebuild the filter mid-chord: a fresh 8-voice module joins
            ui.send_command(EngineCommand::RemoveModule { node_id: 4 });
            ui.send_command(EngineCommand::AddModule { node_id: 4, module_id: "filter.ladder" });
            connect(&mut ui, (3, "filter.svf", "LowPass"), (4, "filter.ladder", "In"));
            connect(&mut ui, (4, "filter.ladder", "LP24"), (6, "util.vca", "In"));
        }
        ui.flush();
        ui.drain_events().for_each(drop);

        if round % 2 == 0 {
            allocations += count_allocations(|| processor.process(&mut output[..frames * 2], 2));
        } else {
            // As the plugin plays: the host's MIDI, with the keys before it
            let mut host = [DspMidiEvent::note_on(10, 0, 48, 90), DspMidiEvent::note_off(frames as u32 - 1, 0, 48, 0)];
            allocations += count_allocations(|| {
                processor.process_offline(&mut output[..frames * 2], 2, &mut host, InputAudio::default())
            });
        }
    }

    assert!(processor.is_playing());
    assert_eq!(processor.plan().len(), chain.len());
    assert!(output.iter().all(|s| s.is_finite()));
    assert!(output.iter().any(|&s| s != 0.0), "the chords are heard");
    let plan = processor.plan();
    assert_eq!(plan.output_channels(11, 0), Some(8), "each voice glides on its own");
    assert_eq!(plan.output_channels(2, 0), Some(8), "the oscillator runs 8 voices");
    assert_eq!(plan.output_channels(6, 0), Some(8), "so does the VCA");
    assert_eq!(plan.output_channels(7, 0), Some(1), "the mixer sums them");
    assert_eq!(allocations, 0, "audio callback allocated {allocations} times playing 8 voices");
}

/// Where the SysEx test's sender sends: nowhere, counting what arrives.
struct Sink(Arc<AtomicUsize>);

struct SinkPort(Arc<AtomicUsize>);

impl soba::link::midi_sender::Outputs for Sink {
    fn scan(&mut self) -> Vec<String> {
        vec!["Sink".into()]
    }
    fn open(&mut self, _name: &str) -> Result<Box<dyn soba::link::midi_sender::Connection>, String> {
        Ok(Box::new(SinkPort(self.0.clone())))
    }
}

impl soba::link::midi_sender::Connection for SinkPort {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes[0] == 0xF0 {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// SysEx from the audio thread (#440) claims a slot of the pool and sends
/// its number down the module's ring: no allocation, while the sender
/// sends and frees the slots on its own thread, a DIN budget keeps many
/// of them waiting (so the pool runs dry and claims are refused), and the
/// UI sends dumps of its own.
#[test]
fn sysex_in_flight_never_allocates() {
    use soba::dsp::context::ProcessContext;
    use soba::link::midi::{Message, MidiBus, DIN_RATE};
    use soba::link::midi_sender::MidiSender;
    use soba::link::midi_writer::MidiWriter;
    use std::time::{Duration, Instant};

    let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
    let arrived = Arc::new(AtomicUsize::new(0));
    let mut sender = MidiSender::new(bus, Box::new(Sink(arrived.clone())));
    bus.choose(0, Some("Sink"));
    bus.set_budget(0, DIN_RATE);
    let mut writer = MidiWriter::on_bus(bus);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sending = {
        let done = done.clone();
        std::thread::spawn(move || {
            let voice: Vec<u8> = std::iter::once(0xF0).chain(std::iter::repeat_n(0x11, 161)).chain(std::iter::once(0xF7)).collect();
            let mut k = 0;
            while !done.load(Ordering::Relaxed) {
                if k % 50 == 0 {
                    bus.send_sysex(0, &voice).unwrap();
                }
                sender.tick(Instant::now());
                std::thread::sleep(Duration::from_millis(1));
                k += 1;
            }
        })
    };
    // Pad colours, as a controller's lights take them: four a block
    let lights: Vec<u8> = [0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x02, 0x00, 0x16, 0x01, 0x7F, 0x00, 0x40, 0xF7].to_vec();
    let start = Instant::now();
    let contexts: Vec<ProcessContext> = (0..300).map(|block| ProcessContext::new(48_000.0, 480).sent_at(start + Duration::from_millis(block * 10))).collect();
    let mut refused = 0;
    let allocations = count_allocations(|| {
        for context in &contexts {
            writer.begin(context, 0);
            for k in 0..4 {
                if writer.sysex(k * 100, &lights).is_err() {
                    refused += 1;
                }
            }
            writer.message(0, Message::note_on(0, 36, 100));
            writer.message(240, Message::note_off(0, 36));
            writer.end(480);
            // About as an audio callback runs, so the sender sends as the blocks come
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    done.store(true, Ordering::Relaxed);
    sending.join().unwrap();
    println!("{} SysEx messages arrived; {refused} claims refused while the pool was in use", arrived.load(Ordering::Relaxed));
    assert!(arrived.load(Ordering::Relaxed) > 50, "SysEx was in flight");
    assert!(refused > 0, "the DIN budget held enough back to run the pool dry");
    assert_eq!(allocations, 0, "sending SysEx from the audio thread allocated {allocations} times");
}
