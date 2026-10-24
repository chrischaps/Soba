//! Plays this machine's real MIDI output from a MIDI Out, through the
//! link's real sender thread.
//!
//! Ignored by default: it needs a MIDI output. Run with
//!
//! ```text
//! cargo test --release --test midi_out_device -- --ignored --nocapture
//! ```
//!
//! It plays an arpeggio, then a chord it stops mid-way, on the first
//! output found (on Windows, the Microsoft GS Wavetable Synth, which you
//! hear) or the one `SOBA_MIDI_OUT` names, with MIDI clock at 120 BPM from
//! the port (#437), as the engine sends it.
//!
//! **Clock steadiness.** Name a loopback port in `SOBA_MIDI_LOOPBACK` (a
//! loopMIDI port, say, as both output and input) and the test listens to
//! what it sends: every message arrives, no note is left hanging, and the
//! clock's ticks keep their 20.8 ms. `MIDI_OUT_SECONDS` sets how long the
//! clock runs (8 s by default); 300 makes it the five-minute soak a
//! following DAW needs, and the drift it reports is what a DAW would see.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use soba::dsp::context::{ProcessContext, TransportState};
use soba::dsp::module_trait::DspModule;
use soba::dsp::signal::SignalBuffer;
use soba::dsp::SignalType;
use soba::link::midi::{ClockMode, PortStatus, MIDI};
use soba::link::midi_clock::MidiClock;
use soba::modules::MidiOut;

const RATE: f32 = 48_000.0;
const BLOCK: usize = 480;
const BPM: f32 = 120.0;

#[test]
#[ignore = "needs a MIDI output"]
fn a_midi_out_plays_a_real_device() {
    soba::link::sender::start();
    let wanted = std::env::var("SOBA_MIDI_OUT").ok();
    let loopback = std::env::var("SOBA_MIDI_LOOPBACK").ok();
    let seconds: f64 = std::env::var("MIDI_OUT_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(8.0);

    let deadline = Instant::now() + Duration::from_secs(5);
    let devices = loop {
        let devices = MIDI.devices();
        if !devices.is_empty() || Instant::now() > deadline {
            break devices;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    println!("MIDI outputs: {devices:?}");
    let device = wanted
        .clone()
        .or_else(|| loopback.clone())
        .or_else(|| devices.first().cloned())
        .expect("no MIDI output on this machine");
    assert!(devices.contains(&device), "{device} isn't among them");
    MIDI.choose(0, Some(&device));
    MIDI.set_clock(0, ClockMode::Transport);
    println!("Playing {device}");

    // What comes back through the loopback, stamped as it arrives
    let heard: Arc<Mutex<Vec<(Instant, Vec<u8>)>>> = Arc::default();
    let _listening = loopback.as_ref().map(|name| {
        let input = midir::MidiInput::new("Soba loopback").unwrap();
        let mut input = input;
        input.ignore(midir::Ignore::None);
        let port = input.ports().into_iter().find(|p| input.port_name(p).is_ok_and(|n| &n == name)).expect("no such loopback input");
        let heard = heard.clone();
        input
            .connect(&port, "Soba loopback", move |_, bytes, _| heard.lock().unwrap().push((Instant::now(), bytes.to_vec())), ())
            .unwrap()
    });

    let mut module = MidiOut::new();
    let mut clock = MidiClock::new();
    let params: Vec<f32> = module.parameters().iter().map(|p| p.default).collect();
    let mut gate = SignalBuffer::new(BLOCK, SignalType::Gate);
    let mut pitch = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
    let empty = SignalBuffer::unconnected(BLOCK, SignalType::Control);

    // In real time, a block every 10 ms, as an audio callback would
    let blocks = (seconds * RATE as f64 / BLOCK as f64) as usize;
    let beats_per_block = BLOCK as f64 * BPM as f64 / 60.0 / RATE as f64;
    let started = Instant::now();
    let mut statuses = Vec::new();
    for block in 0..blocks {
        let beat = block as f64 * beats_per_block;
        let due = started + Duration::from_secs_f64(block as f64 * BLOCK as f64 / RATE as f64);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        // An eighth-note arpeggio for the first two bars, then a held
        // chord that the transport stops in the middle of
        let chord = beat >= 8.0;
        let channels = if chord { 3 } else { 1 };
        pitch.set_channels(channels);
        gate.set_channels(1);
        let step = (beat * 2.0) as usize;
        for c in 0..channels {
            let semis = if chord { [0.0, 4.0, 7.0][c] } else { [0.0, 4.0, 7.0, 12.0][step % 4] };
            pitch.channel_mut(c).fill(semis / 12.0);
        }
        let high = chord || (beat * 2.0).fract() < 0.5;
        gate.samples.fill(if high { 1.0 } else { 0.0 });
        let playing = beat < seconds * BPM as f64 / 60.0 - 1.0;
        let transport = TransportState { playing, beat_position: Some(beat), ..TransportState::playing_at(BPM) };
        let mut context = ProcessContext::new(RATE, BLOCK);
        context.transport = transport;
        let context = context.sent_at(due);
        let mut inputs: Vec<&SignalBuffer> = vec![&empty; module.ports().len()];
        inputs[MidiOut::PORT_PITCH] = &pitch;
        inputs[MidiOut::PORT_GATE] = &gate;
        clock.run(|| Some(transport), BLOCK, RATE, Some(due));
        module.process(&inputs, &mut [], &params, &context);
        statuses.push(MIDI.status(0));
    }
    std::thread::sleep(Duration::from_millis(300));
    let sending = statuses.iter().filter(|&&s| s == PortStatus::Sending).count();
    println!("Sending for {sending} of {blocks} blocks; last status {:?}", statuses.last());
    assert!(sending > blocks / 2, "the port should be sending: {:?}", statuses.last());
    let dropped = module.readout(&params).unwrap().values[MidiOut::READOUT_DROPPED];
    assert_eq!(dropped, 0.0, "the ring overflowed");

    let Some(_) = loopback else { return };
    let heard = heard.lock().unwrap().clone();
    let ticks: Vec<Instant> = heard.iter().filter(|(_, b)| b == &[0xF8]).map(|(at, _)| *at).collect();
    let starts = heard.iter().filter(|(_, b)| b == &[0xFA]).count();
    let stops = heard.iter().filter(|(_, b)| b == &[0xFC]).count();
    let mut held = std::collections::BTreeSet::new();
    for (_, bytes) in &heard {
        match (bytes[0] & 0xF0, bytes.get(2)) {
            (0x90, Some(&v)) if v > 0 => {
                held.insert(bytes[1]);
            }
            (0x80, _) | (0x90, _) => {
                held.remove(&bytes[1]);
            }
            _ => {}
        }
    }
    println!("Heard {} messages: {} clock ticks, {starts} Start, {stops} Stop; notes left on: {held:?}", heard.len(), ticks.len());
    assert_eq!((starts, stops), (1, 1));
    assert!(held.is_empty(), "stuck notes: {held:?}");
    // Each tick against where it should be: a straight line from the first
    let tick = Duration::from_secs_f64(60.0 / BPM as f64 / 24.0);
    let errors: Vec<f64> = ticks
        .iter()
        .enumerate()
        .map(|(k, at)| at.duration_since(ticks[0]).as_secs_f64() - k as f64 * tick.as_secs_f64())
        .collect();
    let worst = errors.iter().fold(0.0f64, |m, e| m.max(e.abs()));
    let drift = errors.last().copied().unwrap_or(0.0);
    let gaps: Vec<f64> = ticks.windows(2).map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.0).collect();
    let mean = gaps.iter().sum::<f64>() / gaps.len().max(1) as f64;
    let jitter = (gaps.iter().map(|g| (g - mean).powi(2)).sum::<f64>() / gaps.len().max(1) as f64).sqrt();
    println!(
        "Ticks every {mean:.3} ms (want {:.3}), jitter {jitter:.3} ms, worst {:.2} ms off the line, {:.2} ms drift by the end",
        tick.as_secs_f64() * 1000.0,
        worst * 1000.0,
        drift * 1000.0
    );
    let expected = ((seconds - 0.5) * BPM as f64 / 60.0 - 1.0) * 24.0;
    assert!(ticks.len() as f64 >= expected, "{} ticks, wanted about {expected}", ticks.len());
    assert!(worst < 0.004, "a tick strayed {:.2} ms", worst * 1000.0);
}

/// The machine's outputs for [`clock_keeps_time`]: one, which notes when
/// each message would have left.
struct Stopwatch(Arc<Mutex<Vec<(Instant, Vec<u8>)>>>);

struct StopwatchPort(Arc<Mutex<Vec<(Instant, Vec<u8>)>>>);

impl soba::link::midi_sender::Outputs for Stopwatch {
    fn scan(&mut self) -> Vec<String> {
        vec!["Stopwatch".into()]
    }
    fn open(&mut self, _name: &str) -> Result<Box<dyn soba::link::midi_sender::Connection>, String> {
        Ok(Box::new(StopwatchPort(self.0.clone())))
    }
}

impl soba::link::midi_sender::Connection for StopwatchPort {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().push((Instant::now(), bytes.to_vec()));
        Ok(())
    }
}

/// Does MIDI clock leave on time, block after block, as the sender thread
/// sleeps and wakes? No device needed: the sender plays a stopwatch.
/// `MIDI_OUT_SECONDS=300` makes it the five-minute soak.
#[test]
#[ignore = "runs in real time"]
fn clock_keeps_time() {
    use soba::link::midi::MidiBus;
    use soba::link::midi_sender::MidiSender;

    let seconds: f64 = std::env::var("MIDI_OUT_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(8.0);
    let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
    let sent: Arc<Mutex<Vec<(Instant, Vec<u8>)>>> = Arc::default();
    let mut sender = MidiSender::new(bus, Box::new(Stopwatch(sent.clone())));
    bus.choose(0, Some("Stopwatch"));
    bus.set_clock(0, ClockMode::Transport);
    let latency = Duration::from_millis(20);
    bus.set_latency(Some(latency));
    // The link thread's loop, noting how late it wakes at worst
    let woke_late = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let late = woke_late.clone();
    std::thread::spawn(move || {
        soba::link::midi_sender::raise_priority();
        loop {
            let next = sender.tick(Instant::now());
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
            let over = Instant::now().saturating_duration_since(next).as_micros() as u64;
            late.fetch_max(over, std::sync::atomic::Ordering::Relaxed);
        }
    });

    // The engine's clock, as the audio callback runs it
    let mut clock = MidiClock::on_bus(bus);
    let blocks = (seconds * RATE as f64 / BLOCK as f64) as usize;
    let beats_per_block = BLOCK as f64 * BPM as f64 / 60.0 / RATE as f64;
    let started = Instant::now() + Duration::from_millis(50);
    let mut callback_late = Duration::ZERO;
    for block in 0..blocks {
        let due = started + Duration::from_secs_f64(block as f64 * BLOCK as f64 / RATE as f64);
        // A callback runs when it runs: up to a couple of ms late
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        callback_late = callback_late.max(Instant::now().saturating_duration_since(due));
        let transport = TransportState { beat_position: Some(block as f64 * beats_per_block), ..TransportState::playing_at(BPM) };
        clock.run(|| Some(transport), BLOCK, RATE, Some(Instant::now()));
    }
    std::thread::sleep(Duration::from_millis(100));

    let sent = sent.lock().unwrap().clone();
    println!(
        "At worst the 'callbacks' ran {:.2} ms late, and the sender woke {:.2} ms late",
        callback_late.as_secs_f64() * 1000.0,
        woke_late.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0
    );
    let ticks: Vec<Instant> = sent.iter().filter(|(_, b)| b == &[0xF8]).map(|(at, _)| *at).collect();
    let tick = 60.0 / BPM as f64 / 24.0;
    // Against where each should leave: its place on the beat grid, heard
    // `latency` after the block carrying it was sent
    let errors: Vec<f64> = ticks
        .iter()
        .enumerate()
        .map(|(k, at)| {
            let ideal = started + latency + Duration::from_secs_f64(k as f64 * tick);
            at.duration_since(ideal).as_secs_f64() - ideal.saturating_duration_since(*at).as_secs_f64()
        })
        .collect();
    let late = errors.iter().fold(0.0f64, |m, &e| m.max(e));
    let worst = errors.iter().position(|&e| e == late).unwrap_or(0);
    let early = errors.iter().fold(0.0f64, |m, &e| m.min(e));
    let mean = errors.iter().sum::<f64>() / errors.len().max(1) as f64;
    let tail = &errors[errors.len().saturating_sub(24 * 4)..];
    let end = tail.iter().sum::<f64>() / tail.len().max(1) as f64;
    println!(
        "{} ticks over {seconds} s: on average {:.2} ms late, at worst {:.2} late (tick {worst}) and {:.2} early; {:.2} ms late over the last bar",
        ticks.len(),
        mean * 1000.0,
        late * 1000.0,
        -early * 1000.0,
        end * 1000.0
    );
    assert!(ticks.len() as f64 >= (seconds * BPM as f64 / 60.0 - 1.0) * 24.0);
    assert!(early > -0.001, "a tick left early");
    assert!(late < 0.004, "a tick left {:.2} ms late", late * 1000.0);
    assert!((end - errors[..96.min(errors.len())].iter().sum::<f64>() / 96.0).abs() < 0.001, "the clock drifted");
}
