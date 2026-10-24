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
//! It names a **port**, A to H, and this machine's settings say which
//! device each port plays (see [`MidiBus::choose`]), as the Rig panel
//! shows them. A patch that plays a synth on port A plays whatever synth is
//! on port A wherever it opens. A port finds its device again when the
//! system renumbers it (see [`device_names`](crate::engine::device_names)).
//!
//! **Clock** is the port's, not a module's (#437): the engine sends one
//! clock stream, from the Clock that sets the patch's tempo
//! ([`midi_clock`](super::midi_clock)), and the sender passes it to every
//! port whose [`ClockMode`] wants it, so two MIDI Outs on a port can't
//! double it and a port nothing plays can still keep a drum machine in
//! time. Each port also has an **offset**, to line a slow device up with
//! the rest (see [`MidiBus::set_offset`]).
//!
//! A device the Clock follows (#438) is sent none of it while it leads: a
//! drum machine set to send and receive clock would otherwise hear its own
//! back (see [`MidiBus::set_leader`]).
//!
//! Unlike the Seoul link's newest-value atomics, MIDI needs every message,
//! in order and on time: a lost Note Off is a stuck note. So the ring
//! carries events, and the sender keeps track of which notes each module
//! is holding, to let them go if the module disappears (deleted, or a new
//! patch loaded) or stops running.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Duration;

use web_time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::MIDI_INPUTS;
use crate::engine::device_names;

/// The [`MidiBus`]'s leader while the patch keeps its own time.
const NO_LEADER: u8 = u8::MAX;

/// Ports a MIDI Out can play, each mapped to a device in the settings:
/// enough for a room of USB gear and a DIN interface's ports (#436).
pub const PORTS: usize = 8;
/// The ports as the Port choice shows them.
pub const PORT_LABELS: &[&str] = &["A", "B", "C", "D", "E", "F", "G", "H"];

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

/// How far a port's offset reaches either way, in milliseconds.
pub const OFFSET_RANGE: f32 = 50.0;

/// What a port sends of the patch's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClockMode {
    /// None of it.
    #[default]
    Off = 0,
    /// Clock ticks, and Start, Stop, Continue and Song Position: the device
    /// follows the tempo and starts and stops with Soba.
    Transport = 1,
    /// Clock ticks alone: the device follows the tempo, and starts and
    /// stops on its own (an RC-505's loops, say).
    ClockOnly = 2,
}

impl ClockMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Transport, Self::ClockOnly];

    /// As the Rig names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Transport => "Clock + Transport",
            Self::ClockOnly => "Clock only",
        }
    }

    /// As the settings keep it.
    fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Transport => "transport",
            Self::ClockOnly => "clock",
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Transport,
            2 => Self::ClockOnly,
            _ => Self::Off,
        }
    }

    /// Whether a port set this way sends `message` of the patch's clock.
    pub fn passes(self, message: &Message) -> bool {
        match self {
            Self::Off => false,
            Self::Transport => true,
            Self::ClockOnly => message.bytes()[0] == status::CLOCK,
        }
    }
}

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

    /// Is this clock, start, stop or another real-time byte?
    pub fn is_real_time(&self) -> bool {
        self.bytes[0] >= status::CLOCK
    }

    /// The message in one word, for an atomic: its length, then its bytes.
    fn packed(&self) -> u32 {
        u32::from_be_bytes([self.len, self.bytes[0], self.bytes[1], self.bytes[2]])
    }

    fn unpacked(word: u32) -> Option<Self> {
        let [len, a, b, c] = word.to_be_bytes();
        (1..=3).contains(&len).then_some(Self { bytes: [a, b, c], len })
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
    /// Nothing plays it: no sender (the offline renderer), or
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

/// What a port has sent, for the Rig panel's lamps: counts that move as
/// messages go, to flash by, and the last of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    /// Messages sent, but for real-time ones.
    pub messages: u32,
    /// Clock ticks sent.
    pub clocks: u32,
    /// The last message sent, but for real-time ones.
    pub last: Option<Message>,
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
    /// What each port has sent: messages, clock ticks, and the last
    /// message packed (see [`Message::packed`]).
    sent: [[AtomicU32; 3]; PORTS],
    /// What each port sends of the patch's clock (a [`ClockMode`]).
    clock: [AtomicU8; PORTS],
    /// Each port's offset in milliseconds, an f32's bits.
    offsets: [AtomicU32; PORTS],
    /// The patch's clock ring, made since the sender last looked.
    new_clock: Mutex<Option<Consumer<Outgoing>>>,
    /// The MIDI input whose clock the patch follows now, or [`NO_LEADER`].
    leader: AtomicU8,
    /// For each MIDI input, the ports playing the same device, a bit each.
    same_device: [AtomicU8; MIDI_INPUTS],
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
            sent: [const { [const { AtomicU32::new(0) }; 3] }; PORTS],
            clock: [const { AtomicU8::new(ClockMode::Off as u8) }; PORTS],
            offsets: [const { AtomicU32::new(0) }; PORTS],
            new_clock: Mutex::new(None),
            leader: AtomicU8::new(NO_LEADER),
            same_device: [const { AtomicU8::new(0) }; MIDI_INPUTS],
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

    /// The ring the engine sends the patch's clock on, or `None` while no
    /// sender drains the bus. One made later takes the place of the last.
    pub fn open_clock_stream(&self) -> Option<Producer<Outgoing>> {
        if !self.draining.load(Relaxed) {
            return None;
        }
        let (producer, consumer) = RingBuffer::new(RING);
        *self.new_clock.lock().ok()? = Some(consumer);
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

    /// The device each port plays, of those plugged in: the one chosen, or
    /// the one it became when the system renumbered it. `None` for a port
    /// with none, or whose device isn't plugged in.
    pub fn found(&self) -> [Option<String>; PORTS] {
        let devices = self.devices();
        let found = device_names::find_all(&self.all_chosen(), &devices);
        std::array::from_fn(|port| found.get(port).copied().flatten().map(str::to_string))
    }

    /// Every port's device as the settings keep it: a line each.
    pub fn setting(&self) -> String {
        self.all_chosen().join("\n")
    }

    /// Takes up the devices [`setting`](Self::setting) kept. One kept
    /// before there were eight ports fills the first four.
    pub fn restore(&self, setting: &str) {
        for (port, device) in setting.lines().take(PORTS).enumerate() {
            self.choose(port, Some(device).filter(|d| !d.is_empty()));
        }
    }

    /// What port `port` sends of the patch's clock.
    pub fn clock(&self, port: usize) -> ClockMode {
        ClockMode::from_code(self.clock.get(port).map_or(0, |c| c.load(Relaxed)))
    }

    /// Every port's clock setting.
    pub fn clock_modes(&self) -> [ClockMode; PORTS] {
        std::array::from_fn(|port| self.clock(port))
    }

    /// Whether any port sends the patch's clock. REAL-TIME SAFE.
    pub fn wants_clock(&self) -> bool {
        self.clock.iter().any(|c| c.load(Relaxed) != ClockMode::Off as u8)
    }

    pub fn set_clock(&self, port: usize, mode: ClockMode) {
        if let Some(slot) = self.clock.get(port) {
            slot.store(mode as u8, Relaxed);
        }
    }

    /// Says which MIDI input's clock the patch follows, if any (#438): the
    /// ports playing that device are sent none of the clock while it
    /// leads, so it never hears its own back. REAL-TIME SAFE.
    pub fn set_leader(&self, input: Option<u8>) {
        self.leader.store(input.filter(|&input| (input as usize) < MIDI_INPUTS).unwrap_or(NO_LEADER), Relaxed);
    }

    /// Says which ports play the same device as MIDI input `input`, a bit
    /// each: the app's side, as devices open and ports are chosen.
    pub fn set_same_device(&self, input: usize, ports: u8) {
        if let Some(slot) = self.same_device.get(input) {
            slot.store(ports, Relaxed);
        }
    }

    /// The ports whose device leads the clock, a bit each: sent none of it.
    pub fn led_ports(&self) -> u8 {
        let leader = self.leader.load(Relaxed) as usize;
        self.same_device.get(leader).map_or(0, |ports| ports.load(Relaxed))
    }

    /// How much later than the audio port `port`'s messages go out, in
    /// milliseconds: negative for earlier.
    pub fn offset(&self, port: usize) -> f32 {
        self.offsets.get(port).map_or(0.0, |o| f32::from_bits(o.load(Relaxed)))
    }

    /// Every port's offset, in milliseconds.
    pub fn offsets(&self) -> [f32; PORTS] {
        std::array::from_fn(|port| self.offset(port))
    }

    /// Moves port `port`'s notes, CCs and clock `ms` later (or earlier,
    /// negative), within [`OFFSET_RANGE`]. Later goes as far as it says;
    /// earlier goes no further than the output's delay, which the sender
    /// already waits out (see [`earliest`](Self::earliest)).
    pub fn set_offset(&self, port: usize, ms: f32) {
        if let Some(slot) = self.offsets.get(port) {
            let ms = if ms.is_finite() { ms.clamp(-OFFSET_RANGE, OFFSET_RANGE) } else { 0.0 };
            slot.store(ms.to_bits(), Relaxed);
        }
    }

    /// The earliest offset that takes effect, in milliseconds: minus the
    /// output's delay, since a message can't go before its block is made.
    pub fn earliest(&self) -> f32 {
        -(self.latency().as_secs_f32() * 1000.0)
    }

    /// Every port's clock and offset as the settings keep them: a line
    /// each, as `transport +12.5`.
    pub fn clock_setting(&self) -> String {
        (0..PORTS).map(|port| format!("{} {:+}", self.clock(port).key(), self.offset(port))).collect::<Vec<_>>().join("\n")
    }

    /// Takes up what [`clock_setting`](Self::clock_setting) kept. A line
    /// it can't read leaves its port off, on time.
    pub fn restore_clock(&self, setting: &str) {
        for (port, line) in setting.lines().take(PORTS).enumerate() {
            let mut words = line.split_whitespace();
            let mode = words.next().and_then(|key| ClockMode::ALL.into_iter().find(|m| m.key() == key));
            let offset = mode.and(words.next()).and_then(|ms| ms.parse().ok());
            self.set_clock(port, mode.unwrap_or_default());
            self.set_offset(port, offset.unwrap_or(0.0));
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

    /// What port `port` has sent.
    pub fn traffic(&self, port: usize) -> Traffic {
        let Some([messages, clocks, last]) = self.sent.get(port) else { return Traffic::default() };
        Traffic { messages: messages.load(Relaxed), clocks: clocks.load(Relaxed), last: Message::unpacked(last.load(Relaxed)) }
    }

    // --- For the sender ---

    /// Says a sender drains this bus from now on.
    pub fn start_draining(&self) {
        self.draining.store(true, Relaxed);
    }

    pub fn take_new_streams(&self) -> Vec<Consumer<Outgoing>> {
        self.new_streams.try_lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default()
    }

    /// The patch's clock ring, if the engine has made one since the last look.
    pub fn take_clock_stream(&self) -> Option<Consumer<Outgoing>> {
        self.new_clock.try_lock().ok()?.take()
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

    pub fn set_traffic(&self, port: usize, traffic: Traffic) {
        if let Some([messages, clocks, last]) = self.sent.get(port) {
            messages.store(traffic.messages, Relaxed);
            clocks.store(traffic.clocks, Relaxed);
            last.store(traffic.last.map_or(0, |m| m.packed()), Relaxed);
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
        assert_eq!(other.all_chosen()[..4], ["", "", "DAW Bus 1", ""]);
        assert_eq!(bus.status(9), PortStatus::Idle, "a stray port reads idle");
    }

    #[test]
    fn eight_ports_and_the_old_four_line_setting() {
        let bus = MidiBus::new();
        bus.restore("Volca FM\n\nDigitakt\nMicroFreak");
        assert_eq!(bus.all_chosen(), ["Volca FM", "", "Digitakt", "MicroFreak", "", "", "", ""]);
        bus.choose(7, Some("Zoia"));
        let other = MidiBus::new();
        other.restore(&bus.setting());
        assert_eq!(other.chosen(7).as_deref(), Some("Zoia"), "port H is kept");
        assert_eq!(PORT_LABELS.len(), PORTS);
    }

    #[test]
    fn ports_find_their_renumbered_devices() {
        let bus = MidiBus::new();
        bus.restore("2- Digitakt\nMicroFreak\nRC-505");
        bus.set_devices(vec!["MicroFreak".into(), "3- Digitakt".into()]);
        let found = bus.found();
        assert_eq!(found[0].as_deref(), Some("3- Digitakt"));
        assert_eq!(found[1].as_deref(), Some("MicroFreak"));
        assert_eq!(found[2], None, "unplugged");
        assert_eq!(found[3], None, "none chosen");
    }

    #[test]
    fn clock_settings_are_kept_a_line_a_port() {
        let bus = MidiBus::new();
        assert!(!bus.wants_clock());
        bus.set_clock(0, ClockMode::Transport);
        bus.set_clock(3, ClockMode::ClockOnly);
        bus.set_offset(3, -7.5);
        bus.set_offset(5, 400.0);
        assert!(bus.wants_clock());
        assert_eq!(bus.offset(5), OFFSET_RANGE, "within reach");
        let other = MidiBus::new();
        other.restore_clock(&bus.clock_setting());
        assert_eq!(other.clock_modes(), bus.clock_modes());
        assert_eq!(other.offsets(), bus.offsets());
        other.restore_clock("sideways 3\ntransport nonsense");
        assert_eq!((other.clock(0), other.offset(0)), (ClockMode::Off, 0.0), "unreadable: off, on time");
        assert_eq!((other.clock(1), other.offset(1)), (ClockMode::Transport, 0.0));
    }

    #[test]
    fn clock_modes_pass_what_they_say() {
        let tick = Message::real_time(status::CLOCK);
        let start = Message::real_time(status::START);
        let place = Message::song_position(8);
        assert!(ClockMode::Transport.passes(&tick) && ClockMode::Transport.passes(&start) && ClockMode::Transport.passes(&place));
        assert!(ClockMode::ClockOnly.passes(&tick) && !ClockMode::ClockOnly.passes(&start) && !ClockMode::ClockOnly.passes(&place));
        assert!(!ClockMode::Off.passes(&tick));
    }

    #[test]
    fn traffic_survives_the_trip_through_atomics() {
        let bus = MidiBus::new();
        assert_eq!(bus.traffic(0), Traffic::default());
        let traffic = Traffic { messages: 3, clocks: 96, last: Some(Message::control(2, 74, 64)) };
        bus.set_traffic(5, traffic);
        assert_eq!(bus.traffic(5), traffic);
        bus.set_traffic(5, Traffic { last: Some(Message::real_time(status::START)), ..traffic });
        assert_eq!(bus.traffic(5).last.unwrap().bytes(), &[status::START]);
        assert!(Message::real_time(status::STOP).is_real_time());
        assert!(!Message::song_position(4).is_real_time());
    }
}
