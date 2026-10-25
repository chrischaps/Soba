//! The Setlist (#445) on a loopback simulated end to end: an Arranger plays
//! a three-section song, clocked as a Clock's quarter notes would clock it,
//! into a Setlist, whose program changes go out through the real MIDI
//! sender, ticking every millisecond as its thread does, to two pretend
//! devices that write down what reached them and when.
//!
//! The Digitakt's pattern must reach it its lead before each bar line it
//! changes on, and the FANTOM's scene, its bank first, on the downbeat,
//! each within the sender's millisecond. Started at the top, the first
//! section's rows go at once.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use soba::dsp::context::TransportState;
use soba::dsp::{DspModule, ProcessContext, SignalBuffer, SignalType};
use soba::hardware::setlist::{self, Row};
use soba::hardware::Profiles;
use soba::link::midi::MidiBus;
use soba::link::midi_sender::{Connection, MidiSender, Outputs};
use soba::modules::{Arranger, Setlist};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 128;
/// 120 BPM: a quarter note is half a second, and a bar of four is two.
const BEAT: usize = 24_000;
/// The output's delay, which every message waits for.
const LATENCY: Duration = Duration::from_millis(10);

/// Each message a device was sent, and when.
type Log = Arc<Mutex<Vec<(String, Vec<u8>, Instant)>>>;

struct Room {
    log: Log,
}

struct Wire {
    device: String,
    log: Log,
}

impl Outputs for Room {
    fn scan(&mut self) -> Vec<String> {
        vec!["Digitakt".into(), "FANTOM".into()]
    }
    fn open(&mut self, name: &str) -> Result<Box<dyn Connection>, String> {
        Ok(Box::new(Wire { device: name.into(), log: self.log.clone() }))
    }
}

impl Connection for Wire {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.send_at(bytes, Instant::now())
    }
    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), String> {
        self.log.lock().unwrap().push((self.device.clone(), bytes.to_vec(), at));
        Ok(())
    }
}

/// Intro, Verse and Chorus: the Digitakt on port A with a 500 ms lead, the
/// FANTOM on port B on the downbeat, in Scene A and then Scene B.
fn rows(profiles: &Profiles) -> std::collections::BTreeMap<String, String> {
    let digitakt = &profiles.find("Digitakt").unwrap().profile;
    let fantom = &profiles.find("FANTOM-06").unwrap().profile;
    let mut labels = std::collections::BTreeMap::new();
    for (section, pattern, scene, bank) in [(0, 0, 0, 0), (1, 4, 11, 0), (2, 16, 2, 1)] {
        let mut row = Row::new(0, Some(digitakt));
        row.program = pattern;
        row.lead = 500;
        labels.insert(setlist::row_key(section, 0), setlist::written(&row));
        let mut row = Row::new(1, Some(fantom));
        row.set_bank(Some(bank), fantom.programs.as_ref());
        row.program = scene;
        labels.insert(setlist::row_key(section, 1), setlist::written(&row));
    }
    labels
}

#[test]
fn each_section_changes_the_room_its_lead_ahead_of_the_bar() {
    let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
    let log = Log::default();
    let mut sender = MidiSender::new(bus, Box::new(Room { log: log.clone() }));
    bus.choose(0, Some("Digitakt"));
    bus.choose(1, Some("FANTOM"));
    bus.set_latency(Some(LATENCY));

    // The song: Steps 4 (a Clock's quarter notes), two bars, two, then one
    let mut arranger = Arranger::new();
    arranger.prepare(RATE, BLOCK);
    let mut params: Vec<f32> = arranger.parameters().iter().map(|p| p.default).collect();
    params[Arranger::PARAM_STEPS] = 4.0;
    params[Arranger::PARAM_SECTIONS] = 3.0;
    for (section, bars) in [2.0, 2.0, 1.0].into_iter().enumerate() {
        params[Arranger::PARAM_LENGTH + section] = bars;
    }

    let mut setlist = Setlist::on_bus(bus);
    setlist.prepare(RATE, BLOCK);
    let profiles = Profiles::load_from(std::path::Path::new("no such folder"));
    for (section, rows) in setlist::maps(&rows(&profiles), &profiles).iter().enumerate() {
        for (row, &map) in rows.iter().enumerate() {
            setlist.map_row(section, row, map);
        }
    }

    let origin = Instant::now();
    let at = |sample: usize| origin + Duration::from_secs_f64(sample as f64 / RATE as f64);
    let transport = TransportState { playing: true, beat_position: Some(0.0), ..TransportState::playing_at(120.0) };
    let mut outputs: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(BLOCK)).collect();
    let send = SignalBuffer::unconnected(BLOCK, SignalType::Gate);
    let mut tick = origin + Duration::from_micros(370);
    let mut downbeats = Vec::new();
    let mut section = None;
    // Six bars: round the song once, and into the Intro again
    for start in (0..6 * 4 * BEAT).step_by(BLOCK) {
        let mut clock = SignalBuffer::gate(BLOCK);
        for (i, sample) in clock.samples.iter_mut().enumerate() {
            *sample = if (start + i) % BEAT < 100 { 1.0 } else { 0.0 };
        }
        let mut context = ProcessContext::with_transport(RATE, BLOCK, transport);
        context.sent_at = Some(at(start));
        arranger.process(&[&clock], &mut outputs, &params, &context);
        for (i, &cv) in outputs[Arranger::OUT_SECTION].samples.iter().enumerate() {
            let now = Arranger::section_for_cv(cv, 3);
            if section.is_some_and(|s| s != now) {
                downbeats.push((now, start + i));
            }
            section = Some(now);
        }
        let (section_cv, next, bar) = (&outputs[Arranger::OUT_SECTION], &outputs[Arranger::OUT_NEXT], &outputs[Arranger::OUT_BAR]);
        setlist.process(&[section_cv, next, bar, &send], &mut [], &[], &context);
        while tick < at(start + BLOCK) + LATENCY {
            sender.tick(tick);
            tick += Duration::from_millis(1);
        }
    }
    assert_eq!(downbeats.iter().map(|&(s, _)| s).collect::<Vec<_>>(), [1, 2, 0], "Verse, Chorus, then the Intro again");

    let log = log.lock().unwrap();
    let sent = |device: &str| -> Vec<(Vec<u8>, Instant)> { log.iter().filter(|(d, _, _)| d == device).map(|(_, bytes, when)| (bytes.clone(), *when)).collect() };
    let near = |when: Instant, expected: Instant| {
        let off = if when > expected { when - expected } else { expected - when };
        off <= Duration::from_millis(1)
    };

    // The Digitakt: A01 at the top, then each pattern 500 ms ahead of its bar
    let digitakt = sent("Digitakt");
    let changes: Vec<_> = digitakt.iter().filter(|(bytes, _)| bytes[0] & 0xF0 == 0xC0).collect();
    assert_eq!(changes.iter().map(|(bytes, _)| bytes[1]).collect::<Vec<_>>(), [0, 4, 16, 0], "A01, A05, B01, A01");
    assert!(near(changes[0].1, at(0) + LATENCY), "the top: at once");
    let lead = Duration::from_millis(500);
    for (change, &(_, downbeat)) in changes[1..].iter().zip(&downbeats) {
        assert!(near(change.1, at(downbeat) + LATENCY - lead), "{:?} early, wanted 500 ms", (at(downbeat) + LATENCY).saturating_duration_since(change.1));
    }

    // The FANTOM: Bank Select, then the scene, on each downbeat
    let fantom = sent("FANTOM");
    let scenes: Vec<_> = fantom.iter().filter(|(bytes, _)| bytes[0] & 0xF0 == 0xC0).collect();
    assert_eq!(scenes.iter().map(|(bytes, _)| bytes[1]).collect::<Vec<_>>(), [0, 11, 2, 0]);
    for (scene, &(_, downbeat)) in scenes[1..].iter().zip(&downbeats) {
        assert!(near(scene.1, at(downbeat) + LATENCY), "the scene on the downbeat");
    }
    let chorus: Vec<_> = fantom.iter().filter(|(_, when)| near(*when, at(downbeats[1].1) + LATENCY)).map(|(bytes, _)| bytes.clone()).collect();
    let channel = fantom_channel(&profiles) - 1;
    assert_eq!(chorus, [vec![0xB0 | channel, 0, 85], vec![0xB0 | channel, 32, 1], vec![0xC0 | channel, 2]], "the chorus's Scene B 3, its bank first");
}

/// The channel the FANTOM's profile takes programs on, 1-16.
fn fantom_channel(profiles: &Profiles) -> u8 {
    let fantom = &profiles.find("FANTOM-06").unwrap().profile;
    fantom.programs.as_ref().and_then(|p| p.channel).unwrap_or(fantom.channel())
}
