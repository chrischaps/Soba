//! MIDI out: a patch's notes, controllers and clock, sent to a device.
//!
//! A **MIDI Out** module ([`crate::modules::midi_out`]) turns cables into
//! MIDI messages on the audio thread, each stamped with the sample it
//! belongs on. They can't be sent from there: opening and writing a device
//! may block. So each MIDI Out owns one end of an rtrb ring, and the
//! link's sender thread (`link::midi_sender`) holds the other, sending each
//! message when its sample reaches the speakers.
//!
//! A module doesn't name a device, which would tie a patch to one machine.
//! It names a **port**, A to D, and this machine's settings say which
//! device each port plays (see [`MidiBus::choose`]). A patch that plays a
//! synth on port A plays whatever synth is on port A wherever it opens.
//!
//! Unlike the Seoul link's newest-value atomics, MIDI needs every message,
//! in order and on time: a lost Note Off is a stuck note. So the ring
//! carries events, and the sender keeps track of which notes each module
//! is holding, to let them go if the module disappears (deleted, or a new
//! patch loaded) or stops running.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Duration;

use web_time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

/// Ports a MIDI Out can play, each mapped to a device in the settings.
pub const PORTS: usize = 4;
/// The ports as the Port choice shows them.
pub const PORT_LABELS: &[&str] = &["A", "B", "C", "D"];

/// Events a module's ring holds: a busy block of chords, CCs and clock is
/// a few dozen, and the sender drains it every millisecond.
pub const RING: usize = 1024;

/// Status bytes this module sends.
pub mod status {
    pub const NOTE_OFF: u8 = 0x80;
    pub const NOTE_ON: u8 = 0x90;
    pub const CONTROL_CHANGE: u8 = 0xB0;
    pub const SONG_POSITION: u8 = 0xF2;
    pub const CLOCK: u8 = 0xF8;
    pub const START: u8 = 0xFA;
    pub const CONTINUE: u8 = 0xFB;
    pub const STOP: u8 = 0xFC;
}

/// CC 123, All Notes Off.
pub const ALL_NOTES_OFF: u8 = 123;

/// What a MIDI Out puts in its ring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outgoing {
    /// A block began. Its first sample is `sample` in the module's count,
    /// and leaves for the output device at `at` (see
    /// [`ProcessContext::sent_at`](crate::dsp::context::ProcessContext::sent_at)).
    /// The sender learns from these when each sample will be heard.
    Block { sample: u64, at: Instant, rate: f32, port: u8 },
    /// A message to send when sample `sample` is heard.
    Message { sample: u64, message: Message },
}

/// One MIDI message of up to three bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    bytes: [u8; 3],
    len: u8,
}

impl Message {
    pub const fn note_on(channel: u8, note: u8, velocity: u8) -> Self {
        Self::three(status::NOTE_ON | (channel & 0x0F), note & 0x7F, velocity & 0x7F)
    }

    pub const fn note_off(channel: u8, note: u8) -> Self {
        Self::three(status::NOTE_OFF | (channel & 0x0F), note & 0x7F, 0)
    }

    pub const fn control(channel: u8, controller: u8, value: u8) -> Self {
        Self::three(status::CONTROL_CHANGE | (channel & 0x0F), controller & 0x7F, value & 0x7F)
    }

    /// Song Position Pointer, in sixteenths from the top.
    pub const fn song_position(sixteenths: u16) -> Self {
        Self::three(status::SONG_POSITION, (sixteenths & 0x7F) as u8, ((sixteenths >> 7) & 0x7F) as u8)
    }

    /// A one-byte real-time message: clock, start, continue or stop.
    pub const fn real_time(byte: u8) -> Self {
        Self { bytes: [byte, 0, 0], len: 1 }
    }

    const fn three(a: u8, b: u8, c: u8) -> Self {
        Self { bytes: [a, b, c], len: 3 }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The note this starts, as (channel, note): a Note On with a velocity.
    pub fn starts(&self) -> Option<(u8, u8)> {
        (self.len == 3 && self.bytes[0] & 0xF0 == status::NOTE_ON && self.bytes[2] > 0)
            .then_some((self.bytes[0] & 0x0F, self.bytes[1]))
    }

    /// The note this ends, as (channel, note): a Note Off, or a Note On at
    /// velocity 0.
    pub fn ends(&self) -> Option<(u8, u8)> {
        let kind = self.bytes[0] & 0xF0;
        (self.len == 3 && (kind == status::NOTE_OFF || (kind == status::NOTE_ON && self.bytes[2] == 0)))
            .then_some((self.bytes[0] & 0x0F, self.bytes[1]))
    }
}

/// What the sender knows about a port, for a MIDI Out's face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortStatus {
    /// Nothing plays it: no sender (the browser, the offline renderer), or
    /// no MIDI Out on this port.
    Idle = 0,
    /// Messages are going to its device.
    Sending = 1,
    /// A MIDI Out plays it, but the settings give it no device.
    NoDevice = 2,
    /// Its device isn't plugged in.
    Missing = 3,
    /// Its device wouldn't open: another app may have it.
    Failed = 4,
}

impl PortStatus {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Sending,
            2 => Self::NoDevice,
            3 => Self::Missing,
            4 => Self::Failed,
            _ => Self::Idle,
        }
    }
}

/// Where the UI, the MIDI Outs and the sender meet.
///
/// The audio thread touches only its own ring. The mutexes are shared by
/// the UI and the sender, never the audio thread.
pub struct MidiBus {
    /// Whether a sender drains this bus. Without one, a MIDI Out gets no
    /// ring, so nothing fills up unread.
    draining: AtomicBool,
    /// Rings made since the sender last looked, for it to take.
    new_streams: Mutex<Vec<Consumer<Outgoing>>>,
    /// The output devices found, by name, newest scan.
    devices: Mutex<Vec<String>>,
    /// Each port's device by name; empty for none.
    chosen: Mutex<[String; PORTS]>,
    status: [AtomicU8; PORTS],
    /// The output device's delay, so MIDI goes out as the audio beside it
    /// is heard: in microseconds.
    latency_micros: AtomicU64,
    /// Asks the sender to look for devices now.
    rescan: AtomicBool,
}

impl MidiBus {
    pub const fn new() -> Self {
        Self {
            draining: AtomicBool::new(false),
            new_streams: Mutex::new(Vec::new()),
            devices: Mutex::new(Vec::new()),
            chosen: Mutex::new([const { String::new() }; PORTS]),
            status: [const { AtomicU8::new(PortStatus::Idle as u8) }; PORTS],
            latency_micros: AtomicU64::new(0),
            rescan: AtomicBool::new(true),
        }
    }

    // --- For a MIDI Out, when it's made (on the UI thread) ---

    /// A ring for a new MIDI Out to write to, or `None` while no sender
    /// drains the bus.
    pub fn open_stream(&self) -> Option<Producer<Outgoing>> {
        if !self.draining.load(Relaxed) {
            return None;
        }
        let (producer, consumer) = RingBuffer::new(RING);
        self.new_streams.lock().ok()?.push(consumer);
        Some(producer)
    }

    // --- For the UI ---

    /// The output devices found, by name.
    pub fn devices(&self) -> Vec<String> {
        self.devices.lock().map(|d| d.clone()).unwrap_or_default()
    }

    /// The device port `port` plays, if it has one.
    pub fn chosen(&self, port: usize) -> Option<String> {
        let chosen = self.chosen.lock().ok()?;
        chosen.get(port).filter(|name| !name.is_empty()).cloned()
    }

    /// Every port's device, empty for none: for the settings to keep.
    pub fn all_chosen(&self) -> [String; PORTS] {
        self.chosen.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Plays port `port` on the device called `device`, or on none.
    pub fn choose(&self, port: usize, device: Option<&str>) {
        if let Ok(mut chosen) = self.chosen.lock() {
            if let Some(slot) = chosen.get_mut(port) {
                *slot = device.unwrap_or_default().to_string();
            }
        }
    }

    /// Every port's device as the settings keep it: a line each.
    pub fn setting(&self) -> String {
        self.all_chosen().join("
")
    }

    /// Takes up the devices [`setting`](Self::setting) kept.
    pub fn restore(&self, setting: &str) {
        for (port, device) in setting.lines().take(PORTS).enumerate() {
            self.choose(port, Some(device).filter(|d| !d.is_empty()));
        }
    }

    /// Asks the sender to look for devices again, soon.
    pub fn rescan(&self) {
        self.rescan.store(true, Relaxed);
    }

    /// Tells the sender how long the output device takes to play a sample.
    pub fn set_latency(&self, latency: Option<Duration>) {
        self.latency_micros.store(latency.map_or(0, |l| l.as_micros() as u64), Relaxed);
    }

    /// What the sender last knew about port `port`.
    pub fn status(&self, port: usize) -> PortStatus {
        PortStatus::from_code(self.status.get(port).map_or(0, |s| s.load(Relaxed)))
    }

    // --- For the sender ---

    /// Says a sender drains this bus from now on.
    pub fn start_draining(&self) {
        self.draining.store(true, Relaxed);
    }

    pub fn take_new_streams(&self) -> Vec<Consumer<Outgoing>> {
        self.new_streams.try_lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default()
    }

    pub fn set_devices(&self, devices: Vec<String>) {
        if let Ok(mut found) = self.devices.lock() {
            *found = devices;
        }
    }

    pub fn take_rescan(&self) -> bool {
        self.rescan.swap(false, Relaxed)
    }

    pub fn latency(&self) -> Duration {
        Duration::from_micros(self.latency_micros.load(Relaxed))
    }

    pub fn set_status(&self, port: usize, status: PortStatus) {
        if let Some(slot) = self.status.get(port) {
            slot.store(status as u8, Relaxed);
        }
    }
}

impl Default for MidiBus {
    fn default() -> Self {
        Self::new()
    }
}

/// The bus every MIDI Out writes to and the sender reads.
pub static MIDI: MidiBus = MidiBus::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_know_their_notes() {
        assert_eq!(Message::note_on(2, 60, 100).bytes(), &[0x92, 60, 100]);
        assert_eq!(Message::note_on(2, 60, 100).starts(), Some((2, 60)));
        assert_eq!(Message::note_off(15, 61).ends(), Some((15, 61)));
        assert_eq!(Message::note_on(0, 62, 0).ends(), Some((0, 62)), "velocity 0 is a Note Off");
        assert_eq!(Message::note_on(0, 62, 0).starts(), None);
        assert_eq!(Message::control(0, 74, 200).bytes(), &[0xB0, 74, 72], "seven bits");
        assert_eq!(Message::real_time(status::CLOCK).bytes(), &[0xF8]);
        assert_eq!(Message::song_position(300).bytes(), &[0xF2, 44, 2], "300 = 2 × 128 + 44");
    }

    #[test]
    fn no_ring_until_a_sender_drains() {
        let bus = MidiBus::new();
        assert!(bus.open_stream().is_none());
        bus.start_draining();
        let mut producer = bus.open_stream().unwrap();
        producer.push(Outgoing::Message { sample: 4, message: Message::note_off(0, 60) }).unwrap();
        let mut streams = bus.take_new_streams();
        assert_eq!(streams.len(), 1);
        assert!(matches!(streams[0].pop(), Ok(Outgoing::Message { sample: 4, .. })));
        assert!(bus.take_new_streams().is_empty(), "taken once");
    }

    #[test]
    fn ports_choose_devices() {
        let bus = MidiBus::new();
        assert_eq!(bus.chosen(0), None);
        bus.choose(1, Some("Minilogue"));
        assert_eq!(bus.chosen(1).as_deref(), Some("Minilogue"));
        bus.choose(1, None);
        assert_eq!(bus.chosen(1), None);
        bus.choose(2, Some("DAW Bus 1"));
        let kept = bus.setting();
        let other = MidiBus::new();
        other.restore(&kept);
        assert_eq!(other.all_chosen(), ["", "", "DAW Bus 1", ""]);
        assert_eq!(bus.status(9), PortStatus::Idle, "a stray port reads idle");
    }
}
