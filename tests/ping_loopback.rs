//! Measuring a device's delay (#449), on a loopback simulated end to end.
//!
//! The pings go out through the real MIDI sender, ticking every
//! millisecond as its thread does, to a pretend device that answers each
//! Note On with a kick a known time after it was sent. The kick comes back
//! through a pretend audio interface (its input's delay and the jitter
//! buffer's), into the samples the engine hears beside the pings. What the
//! measurement makes of it must match the device's delay within a
//! millisecond (the sender's tick is part of what a real device waits, and
//! is counted with it), leave out a ping that landed far off, and give up
//! on an input too noisy to hear the pings in.
//!
//! And the audio thread's side, pinging while the transport plays and
//! while it's stopped, must never allocate.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use soba::dsp::arrival::{agree, Disagreement};
use soba::dsp::InputAudio;
use soba::engine::{input_channel, AudioProcessor, EngineChannels, EngineCommand, RoundTrip};
use soba::link::midi::{MidiBus, MIDI};
use soba::link::midi_sender::{Connection, MidiSender, Outputs};
use soba::link::ping::{Ping, PingPlan, Pinger, Progress, Trip};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;
/// The pretend interface: 12 ms out, 4 ms in, 8 ms of jitter buffer.
const OUTPUT_MS: u64 = 12;
const INPUT_MS: u64 = 4;
const BUFFER_FRAMES: usize = 384;
/// The port sends 5 ms early.
const OFFSET: f32 = -5.0;

/// Each message sent, and when.
type Log = Arc<Mutex<Vec<(Vec<u8>, Instant)>>>;

struct Loopback {
    log: Log,
}

struct Wire {
    log: Log,
}

impl Outputs for Loopback {
    fn scan(&mut self) -> Vec<String> {
        vec!["Loopback".into()]
    }
    fn open(&mut self, _: &str) -> Result<Box<dyn Connection>, String> {
        Ok(Box::new(Wire { log: self.log.clone() }))
    }
}

impl Connection for Wire {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.send_at(bytes, Instant::now())
    }
    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), String> {
        self.log.lock().unwrap().push((bytes.to_vec(), at));
        Ok(())
    }
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// A deterministic hiss, -1 to 1 times `level`.
fn hiss(frames: usize, level: f32) -> Vec<f32> {
    let mut state = 0x9E37_79B9u32;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            level * (state as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

/// A kick into `out` from `at`: a sine falling from 150 Hz to 50, dying away.
fn add_kick(out: &mut [f32], at: usize, level: f32) {
    let mut phase = 0.0f32;
    for (n, s) in out.iter_mut().skip(at).take((0.4 * RATE) as usize).enumerate() {
        let t = n as f32 / RATE;
        let hz = 50.0 + 100.0 * (-t / 0.03).exp();
        *s += level * phase.sin() * (-t / 0.08).exp();
        phase += std::f32::consts::TAU * hz / RATE;
    }
}

/// Measures a device that sounds `device_ms` after each note reaches it
/// (ping `late.0` another `late.1` ms later), over an input hissing at
/// `noise`: the trip besides the device, and where each ping was heard.
fn measure(device_ms: f32, noise: f32, late: Option<(usize, f32)>) -> (Trip, Vec<Option<usize>>) {
    let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
    let log = Log::default();
    let mut sender = MidiSender::new(bus, Box::new(Loopback { log: log.clone() }));
    bus.choose(0, Some("Loopback"));
    bus.set_latency(Some(ms(OUTPUT_MS)));
    bus.set_offset(0, OFFSET);

    let plan = PingPlan::new(0, 0, 36, RATE, PingPlan::LEAD);
    let (ping, mut listening) = Ping::on_bus(bus, plan).expect("the sender drains the bus");
    let mut pinger = Pinger::default();
    pinger.start(Some(ping));

    // What the input hears, filled in as the device answers
    let mut heard = hiss(plan.frames() + BLOCK, noise);
    let origin = Instant::now();
    let frame_of = |at: Instant| ((at - origin).as_secs_f64() * RATE as f64).round() as usize;
    // From the device's jack to the engine reading it
    let back = ms(INPUT_MS) + Duration::from_secs_f64(BUFFER_FRAMES as f64 / RATE as f64);
    // The sender's thread wakes every millisecond, out of step with the blocks
    let mut tick = origin + Duration::from_micros(370);
    let mut answered = 0;
    let mut start = 0;
    while pinger.is_pinging() {
        let at = origin + Duration::from_secs_f64(start as f64 / RATE as f64);
        let block = heard[start..start + BLOCK].to_vec();
        pinger.run(&InputAudio::interleaved(&block, 1), BLOCK, RATE, Some(at));
        start += BLOCK;
        let end = origin + Duration::from_secs_f64(start as f64 / RATE as f64);
        while tick < end {
            sender.tick(tick);
            tick += ms(1);
        }
        let notes: Vec<Instant> = log.lock().unwrap().iter().filter(|(bytes, _)| bytes[0] & 0xF0 == 0x90 && bytes[2] > 0).map(|&(_, at)| at).collect();
        for &sent in &notes[answered..] {
            let extra = late.filter(|&(k, _)| k == answered).map_or(0.0, |(_, ms)| ms);
            let sounds = sent + Duration::from_secs_f32((device_ms + extra) / 1000.0);
            let frame = frame_of(sounds + back);
            assert!(frame >= start, "the answer comes after the block that sent it");
            add_kick(&mut heard, frame, 0.4);
            answered += 1;
        }
    }
    assert_eq!(answered, plan.pings, "every ping went out");
    let offs = log.lock().unwrap().iter().filter(|(bytes, _)| bytes[0] & 0xF0 == 0x80).count();
    assert_eq!(offs, plan.pings, "and was let go");
    assert_eq!(listening.poll(), Progress::Heard);
    assert!(listening.released());

    let round_trip = RoundTrip::new(Some(ms(INPUT_MS)), 480, BUFFER_FRAMES, Some(ms(OUTPUT_MS)), RATE as u32);
    (Trip::new(round_trip, OFFSET), plan.arrivals(listening.captured()))
}

#[test]
fn a_loopback_measures_the_known_delay() {
    for device in [0.0, 3.5, 9.0, 27.25] {
        let (trip, arrivals) = measure(device, 0.0005, None);
        let agreement = agree(&trip.device_delays(&arrivals, RATE)).expect("every ping heard");
        println!("device {device} ms: measured {:.2} ms, within {:.2} ms", agreement.median, agreement.spread);
        assert_eq!((agreement.heard, agreement.kept), (8, 8));
        // The sender's tick lands each note up to a millisecond after it's due
        let late = agreement.median - device;
        assert!((0.0..1.0).contains(&late), "measured {:.2} ms for {device} ms", agreement.median);
        assert!(agreement.spread < 0.2, "{agreement:?}");

        // The offset it offers lands the device with Soba
        let (offset, out_of_reach) = trip.offset_for(agreement.median);
        assert!(!out_of_reach || device > OUTPUT_MS as f32);
        if !out_of_reach {
            assert!(Trip::new(trip.round_trip, offset).lands(agreement.median).abs() < 1e-3);
        }
    }
}

#[test]
fn a_ping_landing_far_off_is_left_out() {
    let (trip, arrivals) = measure(6.0, 0.0005, Some((3, 22.0)));
    let agreement = agree(&trip.device_delays(&arrivals, RATE)).expect("seven agree");
    assert_eq!((agreement.heard, agreement.kept), (8, 7));
    assert!((0.0..1.0).contains(&(agreement.median - 6.0)), "{agreement:?}");
}

#[test]
fn a_noisy_input_gives_up_rather_than_guess() {
    let (trip, arrivals) = measure(6.0, 0.15, None);
    match agree(&trip.device_delays(&arrivals, RATE)) {
        Err(Disagreement::TooFewHeard { heard, of: 8 }) => assert!(heard < 5, "{heard}"),
        other => panic!("guessed: {other:?}"),
    }
}

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

#[test]
fn pinging_never_allocates_on_the_audio_thread() {
    MIDI.start_draining();
    let (mut ui, engine) = EngineChannels::with_defaults().split();
    let mut processor = AudioProcessor::new(RATE, BLOCK, engine);
    let (mut input, feed, _monitor) = input_channel(RATE as u32);
    ui.connect_input(feed);
    assert!(ui.flush());
    let device_input: Vec<f32> = (0..480).flat_map(|n| [(n as f32 * 0.13).sin() * 0.5; 2]).collect();
    let mut output = vec![0.0f32; 1024 * 2];
    let buffers = [256, 441, 480, 128, 1024, 64];
    let mut allocations = 0;
    let mut input_due = 0;

    // Two measurements: the first begun stopped and carried on playing,
    // the second stopped partway (and its end kept until it's let go)
    for (round, stop_at) in [(0, None), (1, Some(40_000))] {
        let plan = PingPlan::new(0, 0, 36, RATE, 0.3);
        let (ping, mut listening) = Ping::on_bus(&MIDI, plan).expect("draining");
        ui.ping(Some(ping));
        assert!(ui.flush());
        let mut done = 0usize;
        let mut index = 0;
        let progress = loop {
            let frames = buffers[index % buffers.len()];
            index += 1;
            // The input device keeps up with the output
            input_due += frames;
            while input_due >= 480 {
                allocations += count_allocations(|| input.push_f32(&device_input, 2));
                input_due -= 480;
            }
            if round == 0 && done > plan.frames() / 2 && done < plan.frames() / 2 + 1024 {
                ui.send_command(EngineCommand::SetPlaying(true));
            }
            if stop_at.is_some_and(|at| done >= at) {
                ui.ping(None);
            }
            ui.flush();
            allocations += count_allocations(|| processor.process(&mut output[..frames * 2], 2));
            done += frames;
            match listening.poll() {
                Progress::Listening(_) => assert!(done < plan.frames() * 2, "never finished"),
                progress => break progress,
            }
        };
        match round {
            0 => assert_eq!(progress, Progress::Heard),
            _ => assert!(matches!(progress, Progress::Cut(heard) if heard < plan.frames())),
        }
        assert!(listening.released(), "the audio thread let go before the UI did");
        ui.send_command(EngineCommand::SetPlaying(false));
        ui.flush();
    }
    assert_eq!(allocations, 0, "the audio thread allocated or freed while pinging");
}
