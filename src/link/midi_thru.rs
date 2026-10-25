//! Thru routes (#446): Soba as the room's MIDI hub.
//!
//! With every device plugged into the computer, the computer is the patch
//! bay. A **route** passes what one MIDI In hears straight to one MIDI Out
//! port: the Fantom's keys playing the MicroFreak, or the Tracker driving
//! the Volcas, with no cables in the graph and while the graph does
//! something else. Each route can pass one channel or all, move them to
//! another channel, and pass only some kinds of message (notes, CCs, bend
//! and pressure, clock, program changes).
//!
//! **The quick way through.** Routing doesn't wait for the audio thread or
//! a block: the MIDI engine's input callback runs each message down the
//! routes ([`ThruInput::forward`]) and pushes what passes onto a ring to the
//! link's sender thread, then wakes it. The sender sends it at once, ahead
//! of anything due, skipping the output's delay and the port's offset that
//! line a MIDI Out's notes up with the sound: thru is for playing.
//!
//! The callback reads the routes from atomics, a word each ([`Route`]),
//! which the UI keeps up to date from the routes as the Rig edits them
//! ([`ThruRoute`], by device name) and the inputs open now. So the
//! callback never locks, allocates or waits on account of a route.
//!
//! **Loops are refused.** A route whose port plays the device it comes from
//! would play itself, and with a device that echoes what it hears, feed
//! back forever. Such a route passes nothing (see [`is_loop`]).
//!
//! **Thru only** (#463). An input can be open for its routes alone: a
//! keyboard that's only there to play a piece of hardware. Modules set to
//! any device, MIDI Learn, the mappings from any device and the hands
//! taught by whichever moves first all pass it by; one that names its
//! device still hears it, since naming it is asking for it. The mark is
//! the UI's: the input callback passes everything on as before, and what
//! listens leaves it be (see [`ThruBus::thru_only_slots`]).
//!
//! Routes are this computer's, like the ports: the settings keep them a
//! line each, and each Thru-only input a line too ([`ThruBus::setting`]).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

use rtrb::{Consumer, Producer, RingBuffer};

use super::midi::{Message, PORTS, PORT_LABELS};
use crate::engine::device_names;

/// Routes there can be at once: more than a room of gear needs.
pub const ROUTES: usize = 16;
/// What begins a Thru-only input's line in the settings. A route's begins
/// "on" or "off", so a Soba from before #463 leaves it out.
const ONLY: &str = "only";
/// Messages the ring to the sender holds: the sender empties it as soon as
/// it's woken, so a full ring is a stalled sender, not a busy player.
const RING: usize = 512;

/// The kinds of message a route passes, a bit each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Kinds(u8);

impl Kinds {
    pub const NONE: Self = Self(0);
    /// Note On and Note Off.
    pub const NOTES: Self = Self(1);
    /// Control changes.
    pub const CONTROL: Self = Self(2);
    /// Pitch bend, channel pressure and poly pressure: the hand's expression.
    pub const EXPRESSION: Self = Self(4);
    /// Clock ticks, Start, Stop, Continue and Song Position.
    pub const CLOCK: Self = Self(8);
    /// Program changes.
    pub const PROGRAM: Self = Self(16);
    pub const ALL: Self = Self(31);

    /// Each kind, as the Rig names it and the settings keep it.
    pub const EACH: [(Self, &'static str, &'static str); 5] = [
        (Self::NOTES, "Notes", "notes"),
        (Self::CONTROL, "CC", "cc"),
        (Self::EXPRESSION, "Bend", "bend"),
        (Self::CLOCK, "Clock", "clock"),
        (Self::PROGRAM, "Prog", "program"),
    ];

    /// Does this pass every kind in `kind`, and is there one?
    pub fn has(self, kind: Self) -> bool {
        kind.0 != 0 && self.0 & kind.0 == kind.0
    }

    /// This with `kind` passed, or not.
    pub fn with(self, kind: Self, on: bool) -> Self {
        if on {
            Self(self.0 | kind.0)
        } else {
            Self(self.0 & !kind.0)
        }
    }

    /// The kind of message `status` begins: none for one no route passes
    /// (SysEx, Active Sensing, Song Select and the like).
    pub fn of(status: u8) -> Self {
        match status {
            0x80..=0x9F => Self::NOTES,
            0xB0..=0xBF => Self::CONTROL,
            0xA0..=0xAF | 0xD0..=0xEF => Self::EXPRESSION,
            0xC0..=0xCF => Self::PROGRAM,
            0xF2 | 0xF8 | 0xFA | 0xFB | 0xFC => Self::CLOCK,
            _ => Self::NONE,
        }
    }
}

impl Default for Kinds {
    /// Everything but clock: a keyboard's clock beside the patch's own on
    /// the same port would run its device at double time.
    fn default() -> Self {
        Self(Self::ALL.0 & !Self::CLOCK.0)
    }
}

/// How many bytes a message that begins with `status` has.
fn length(status: u8) -> usize {
    match status {
        0xC0..=0xDF => 2,
        0x80..=0xEF | 0xF2 => 3,
        _ => 1,
    }
}

/// A route as the Rig edits it and the settings keep it: by device name,
/// so it survives a renumber (see [`device_names`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThruRoute {
    /// Passing, or set aside.
    pub on: bool,
    /// The MIDI In it hears, by name.
    pub from: String,
    /// The MIDI Out port it plays, A to H.
    pub to: usize,
    /// The channel it passes (0-15), or all of them.
    pub channel: Option<u8>,
    /// The channel it moves them to (0-15), or the one each came on.
    pub remap: Option<u8>,
    /// What it passes.
    pub kinds: Kinds,
}

impl ThruRoute {
    /// A route from the input `from` to port `to`, passing everything but
    /// clock on every channel as it is.
    pub fn new(from: impl Into<String>, to: usize) -> Self {
        Self { on: true, from: from.into(), to: to.min(PORTS - 1), channel: None, remap: None, kinds: Kinds::default() }
    }

    /// As the settings keep it: on or off, the input, the port, the
    /// channel, the channel it becomes and the kinds, between tabs.
    fn line(&self) -> String {
        let channel = |c: Option<u8>, none: &str| c.map_or(none.to_string(), |c| (c + 1).to_string());
        let kinds: Vec<&str> = Kinds::EACH.iter().filter(|(kind, ..)| self.kinds.has(*kind)).map(|(_, _, key)| *key).collect();
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            if self.on { "on" } else { "off" },
            self.from,
            PORT_LABELS[self.to.min(PORTS - 1)],
            channel(self.channel, "all"),
            channel(self.remap, "same"),
            kinds.join(",")
        )
    }

    /// The route `line` keeps, if it reads as one.
    fn from_line(line: &str) -> Option<Self> {
        let mut fields = line.split('\t');
        let on = match fields.next()? {
            "on" => true,
            "off" => false,
            _ => return None,
        };
        let from = fields.next().filter(|from| !from.is_empty())?.to_string();
        let port = fields.next()?;
        let to = PORT_LABELS.iter().position(|label| *label == port)?;
        let channel = |field: Option<&str>| -> Option<Option<u8>> {
            match field?.parse::<u8>() {
                Ok(n @ 1..=16) => Some(Some(n - 1)),
                Ok(_) => None,
                Err(_) => Some(None),
            }
        };
        let channel_of = channel(fields.next())?;
        let remap = channel(fields.next())?;
        let kinds = fields
            .next()
            .unwrap_or_default()
            .split(',')
            .filter_map(|key| Kinds::EACH.iter().find(|(.., k)| *k == key).map(|(kind, ..)| *kind))
            .fold(Kinds::NONE, |kinds, kind| kinds.with(kind, true));
        Some(Self { on, from, to, channel: channel_of, remap, kinds })
    }
}

/// What a route is doing, for the Rig.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteState {
    /// Set aside by hand.
    Off,
    /// Its port plays the device it comes from: refused.
    Loop,
    /// Its port has no device.
    NoDevice,
    /// Its input isn't open, so Soba hears nothing to pass.
    Closed,
    /// Passing, though its port's device isn't plugged in.
    Unplugged,
    /// Passing.
    Passing,
}

/// A route as the input callback reads it: the input's slot rather than
/// its name, packed into a word (see [`pack`](Self::pack)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    /// The slot of the input it hears (see [`crate::engine::MidiEngine`]).
    pub slot: u8,
    /// The port it plays.
    pub port: u8,
    pub channel: Option<u8>,
    pub remap: Option<u8>,
    pub kinds: Kinds,
}

impl Route {
    /// Set in every packed route, so a word of 0 is none.
    const ON: u64 = 1 << 63;
    /// A channel of none, packed.
    const ANY: u64 = 0xFF;

    fn pack(self) -> u64 {
        let channel = |c: Option<u8>| c.map_or(Self::ANY, |c| (c & 0x0F) as u64);
        Self::ON
            | self.slot as u64
            | (self.port as u64) << 8
            | channel(self.channel) << 16
            | channel(self.remap) << 24
            | (self.kinds.0 as u64) << 32
    }

    fn unpack(word: u64) -> Option<Self> {
        if word & Self::ON == 0 {
            return None;
        }
        let byte = |shift: u32| (word >> shift) as u8;
        let channel = |shift: u32| (byte(shift) != Self::ANY as u8).then_some(byte(shift) & 0x0F);
        Some(Self { slot: byte(0), port: byte(8).min(PORTS as u8 - 1), channel: channel(16), remap: channel(24), kinds: Kinds(byte(32)) })
    }

    /// What this route sends of `bytes`, heard on input `slot`: the
    /// message, moved to its new channel, or `None` if it doesn't pass.
    /// REAL-TIME SAFE.
    pub fn pass(&self, slot: u8, bytes: &[u8]) -> Option<Message> {
        let &status = bytes.first()?;
        if slot != self.slot || !self.kinds.has(Kinds::of(status)) {
            return None;
        }
        let len = length(status);
        let mut out = [0u8; 3];
        out[..len].copy_from_slice(bytes.get(..len)?);
        if status < 0xF0 {
            if self.channel.is_some_and(|channel| channel != status & 0x0F) {
                return None;
            }
            if let Some(channel) = self.remap {
                out[0] = (status & 0xF0) | channel;
            }
        }
        Message::from_bytes(&out[..len])
    }
}

/// Is a route from the input called `input` to the output called `output`
/// a loop: are they the same device? Windows names a device's extra ports
/// "MIDIIN2 (Fantom-06)" and "MIDIOUT2 (Fantom-06)", so the device is
/// what's in the brackets.
pub fn is_loop(input: &str, output: &str) -> bool {
    device_names::same(input, output) || device_of(input) == device_of(output)
}

/// The device a port's name names: its [`key`](device_names::key), without
/// Windows' "midiin (...)" or "midiout (...)" around it.
fn device_of(name: &str) -> String {
    let key = device_names::key(name);
    for word in ["midiin (", "midiout ("] {
        if let Some(inner) = key.strip_prefix(word).and_then(|rest| rest.strip_suffix(')')) {
            return inner.to_string();
        }
    }
    key
}

/// A message passed down route `route` to port `port`, on its way to the sender.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Forward {
    pub route: u8,
    pub port: u8,
    pub message: Message,
}

/// Where the routes, the MIDI engine's callbacks and the sender meet.
///
/// The callbacks read only the packed routes and push to their ring. The
/// mutexes are the UI's and the sender's.
pub struct ThruBus {
    /// Whether a sender takes the ring. Without one, no input gets one.
    draining: AtomicBool,
    /// The routes as the Rig edits them.
    routes: Mutex<Vec<ThruRoute>>,
    /// The inputs open for their routes alone (#463), by name.
    only: Mutex<Vec<String>>,
    /// Each route as the callbacks read it: a [`Route`] packed, or 0.
    packed: [AtomicU64; ROUTES],
    /// How many messages each route has passed, for the Rig's lamps.
    forwarded: [AtomicU32; ROUTES],
    /// The ring made since the sender last looked.
    new_ring: Mutex<Option<Consumer<Forward>>>,
    /// The sender's thread, to wake when there's something to pass.
    #[cfg(not(target_arch = "wasm32"))]
    waker: std::sync::OnceLock<std::thread::Thread>,
}

impl ThruBus {
    pub const fn new() -> Self {
        Self {
            draining: AtomicBool::new(false),
            routes: Mutex::new(Vec::new()),
            only: Mutex::new(Vec::new()),
            packed: [const { AtomicU64::new(0) }; ROUTES],
            forwarded: [const { AtomicU32::new(0) }; ROUTES],
            new_ring: Mutex::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            waker: std::sync::OnceLock::new(),
        }
    }

    // --- For the MIDI engine, when it starts ---

    /// The input side of a new ring to the sender, or `None` while no
    /// sender takes it. One made later takes the place of the last.
    pub fn open_input(&'static self) -> Option<ThruInput> {
        if !self.draining.load(Relaxed) {
            return None;
        }
        let (ring, consumer) = RingBuffer::new(RING);
        *self.new_ring.lock().ok()? = Some(consumer);
        Some(ThruInput { bus: self, ring })
    }

    // --- For the UI ---

    /// Are there no routes?
    pub fn is_empty(&self) -> bool {
        self.routes.lock().map_or(true, |r| r.is_empty())
    }

    /// The routes, as the Rig edits them.
    pub fn routes(&self) -> Vec<ThruRoute> {
        self.routes.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// Changes the routes: no more than [`ROUTES`] are kept.
    pub fn edit(&self, edit: impl FnOnce(&mut Vec<ThruRoute>)) {
        if let Ok(mut routes) = self.routes.lock() {
            edit(&mut routes);
            routes.truncate(ROUTES);
        }
    }

    /// Hands the callbacks each route as it stands: from the slot its input
    /// is open in (`slot_of`), to its port, which plays `chosen` and finds
    /// it as `found` (see [`MidiBus::found`](super::midi::MidiBus::found)).
    /// Returns what each route is doing.
    pub fn follow(&self, slot_of: impl Fn(&str) -> Option<u8>, chosen: &[String; PORTS], found: &[Option<String>; PORTS]) -> Vec<RouteState> {
        let routes = self.routes();
        let mut states = Vec::with_capacity(routes.len());
        for (index, packed) in self.packed.iter().enumerate() {
            let (route, state) = match routes.get(index) {
                Some(route) => resolve(route, &slot_of, chosen, found),
                None => (None, RouteState::Off),
            };
            let word = route.map_or(0, Route::pack);
            if packed.load(Relaxed) != word {
                packed.store(word, Relaxed);
            }
            if index < routes.len() {
                states.push(state);
            }
        }
        states
    }

    /// How many messages route `index` has passed.
    pub fn forwarded(&self, index: usize) -> u32 {
        self.forwarded.get(index).map_or(0, |f| f.load(Relaxed))
    }

    /// Is the input called `name` open for its routes alone? Found again
    /// under a new number, like a route's input.
    pub fn is_thru_only(&self, name: &str) -> bool {
        self.only.lock().is_ok_and(|only| only.iter().any(|kept| device_names::same(kept, name)))
    }

    /// Marks the input called `name` Thru only, or not.
    pub fn set_thru_only(&self, name: &str, on: bool) {
        if let Ok(mut only) = self.only.lock() {
            only.retain(|kept| !device_names::same(kept, name));
            if on {
                only.push(name.to_string());
            }
        }
    }

    /// The slots of the inputs `open` (each slot and name) that are Thru
    /// only, a bit a slot: what "any device" leaves out.
    pub fn thru_only_slots<'a>(&self, open: impl IntoIterator<Item = (u8, &'a str)>) -> u8 {
        let Ok(only) = self.only.lock() else { return 0 };
        if only.is_empty() {
            return 0;
        }
        open.into_iter()
            .filter(|(_, name)| only.iter().any(|kept| device_names::same(kept, name)))
            .fold(0, |slots, (slot, _)| slots | 1u8.checked_shl(slot as u32).unwrap_or(0))
    }

    /// The routes as the settings keep them, a line each, then a line for
    /// each Thru-only input: "only" and its name.
    pub fn setting(&self) -> String {
        let only = self.only.lock().map(|only| only.clone()).unwrap_or_default();
        let routes = self.routes().iter().map(ThruRoute::line).collect::<Vec<_>>();
        routes.into_iter().chain(only.iter().map(|name| format!("{ONLY}\t{name}"))).collect::<Vec<_>>().join("\n")
    }

    /// Takes up the routes and Thru-only inputs [`setting`](Self::setting)
    /// kept, leaving out a line it can't read.
    pub fn restore(&self, setting: &str) {
        let routes: Vec<ThruRoute> = setting.lines().filter_map(ThruRoute::from_line).collect();
        self.edit(|kept| *kept = routes);
        let only = setting.lines().filter_map(|line| line.strip_prefix(ONLY)?.strip_prefix('\t')).filter(|name| !name.is_empty());
        if let Ok(mut kept) = self.only.lock() {
            *kept = only.map(str::to_string).collect();
        }
    }

    // --- For the sender ---

    /// Says a sender takes the ring from now on.
    pub fn start_draining(&self) {
        self.draining.store(true, Relaxed);
    }

    /// The ring, if an input has made one since the last look.
    pub fn take_ring(&self) -> Option<Consumer<Forward>> {
        self.new_ring.try_lock().ok()?.take()
    }

    /// Route `index` as the callbacks read it.
    pub fn route(&self, index: usize) -> Option<Route> {
        Route::unpack(self.packed.get(index)?.load(Relaxed))
    }

    pub fn set_forwarded(&self, index: usize, count: u32) {
        if let Some(slot) = self.forwarded.get(index) {
            slot.store(count, Relaxed);
        }
    }

    /// Has the input side wake `thread` when there's something to pass.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_waker(&self, thread: std::thread::Thread) {
        let _ = self.waker.set(thread);
    }

    /// Wakes the sender. REAL-TIME SAFE: an unpark doesn't wait.
    fn wake(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(thread) = self.waker.get() {
            thread.unpark();
        }
        // The browser's sender has no thread: it passes them now
        #[cfg(target_arch = "wasm32")]
        super::web_sender::forward_now();
    }

    /// Stops every route from input `slot`, which has closed, until the UI
    /// finds its input open again: another device opened in the slot
    /// mustn't be passed down them.
    fn forget(&self, slot: u8) {
        for packed in &self.packed {
            if Route::unpack(packed.load(Relaxed)).is_some_and(|route| route.slot == slot) {
                packed.store(0, Relaxed);
            }
        }
    }
}

impl Default for ThruBus {
    fn default() -> Self {
        Self::new()
    }
}

/// What a route does now, packed, and what it says in the Rig.
fn resolve(route: &ThruRoute, slot_of: &impl Fn(&str) -> Option<u8>, chosen: &[String; PORTS], found: &[Option<String>; PORTS]) -> (Option<Route>, RouteState) {
    let to = route.to.min(PORTS - 1);
    if !route.on {
        return (None, RouteState::Off);
    }
    let Some(device) = found[to].as_deref().or(Some(chosen[to].as_str()).filter(|d| !d.is_empty())) else {
        return (None, RouteState::NoDevice);
    };
    if is_loop(&route.from, device) || is_loop(&route.from, &chosen[to]) {
        return (None, RouteState::Loop);
    }
    let Some(slot) = slot_of(&route.from) else {
        return (None, RouteState::Closed);
    };
    let packed = Route { slot, port: to as u8, channel: route.channel, remap: route.remap, kinds: route.kinds };
    (Some(packed), if found[to].is_some() { RouteState::Passing } else { RouteState::Unplugged })
}

/// The MIDI engine's end: what its devices' callbacks pass down the routes.
pub struct ThruInput {
    bus: &'static ThruBus,
    ring: Producer<Forward>,
}

impl ThruInput {
    /// Passes what input `slot` just heard down every route from it, and
    /// wakes the sender if any took it. Returns whether any did.
    /// REAL-TIME SAFE: no locks, no allocation, never waits. A full ring
    /// drops the message rather than hold up the input.
    pub fn forward(&mut self, slot: u8, bytes: &[u8]) -> bool {
        let mut passed = false;
        for (index, packed) in self.bus.packed.iter().enumerate() {
            let Some(route) = Route::unpack(packed.load(Relaxed)) else { continue };
            if let Some(message) = route.pass(slot, bytes) {
                passed |= self.ring.push(Forward { route: index as u8, port: route.port, message }).is_ok();
            }
        }
        if passed {
            self.bus.wake();
        }
        passed
    }

    /// Input `slot` closed: its routes pass nothing until it's open again.
    pub fn forget(&self, slot: u8) {
        self.bus.forget(slot);
    }
}

/// The routes, and the ring every MIDI In passes them down.
pub static THRU: ThruBus = ThruBus::new();

#[cfg(test)]
mod tests {
    use super::*;

    fn route(kinds: Kinds) -> Route {
        Route { slot: 2, port: 1, channel: None, remap: None, kinds }
    }

    fn bytes(message: Option<Message>) -> Option<Vec<u8>> {
        message.map(|m| m.bytes().to_vec())
    }

    #[test]
    fn a_route_passes_only_its_input() {
        let route = route(Kinds::ALL);
        assert_eq!(bytes(route.pass(2, &[0x90, 60, 100])), Some(vec![0x90, 60, 100]));
        assert_eq!(route.pass(3, &[0x90, 60, 100]), None, "another input");
    }

    #[test]
    fn a_route_passes_only_its_kinds() {
        let notes = route(Kinds::NOTES);
        assert!(notes.pass(2, &[0x93, 60, 100]).is_some());
        assert!(notes.pass(2, &[0x83, 60, 40]).is_some(), "the Note Off with its note");
        assert!(notes.pass(2, &[0xB3, 1, 64]).is_none());
        let expression = route(Kinds::EXPRESSION);
        assert!(expression.pass(2, &[0xE0, 0, 64]).is_some(), "bend");
        assert!(expression.pass(2, &[0xD0, 90]).is_some(), "pressure");
        assert!(expression.pass(2, &[0xA0, 60, 90]).is_some(), "poly pressure");
        assert!(expression.pass(2, &[0x90, 60, 90]).is_none());
        let clock = route(Kinds::CLOCK);
        for message in [&[0xF8][..], &[0xFA], &[0xFB], &[0xFC], &[0xF2, 4, 0]] {
            assert!(clock.pass(2, message).is_some(), "{message:?}");
        }
        assert!(clock.pass(2, &[0xC0, 5]).is_none());
        assert_eq!(bytes(route(Kinds::PROGRAM).pass(2, &[0xC4, 5])), Some(vec![0xC4, 5]));
        // Clock is off unless asked for
        assert!(route(Kinds::default()).pass(2, &[0xF8]).is_none());
        assert!(route(Kinds::default()).pass(2, &[0xB0, 74, 3]).is_some());
    }

    #[test]
    fn what_no_route_passes() {
        let all = route(Kinds::ALL);
        assert!(all.pass(2, &[0xFE]).is_none(), "Active Sensing");
        assert!(all.pass(2, &[0xF0, 0x41, 0x10, 0xF7]).is_none(), "SysEx");
        assert!(all.pass(2, &[0x90, 60]).is_none(), "cut short");
        assert!(all.pass(2, &[]).is_none());
        assert_eq!(bytes(all.pass(2, &[0xC0, 5, 9])), Some(vec![0xC0, 5]), "only its own bytes");
    }

    #[test]
    fn a_channel_filter_passes_one_channel() {
        let route = Route { channel: Some(9), ..route(Kinds::ALL) };
        assert!(route.pass(2, &[0x99, 36, 100]).is_some());
        assert!(route.pass(2, &[0x90, 36, 100]).is_none());
        assert!(route.pass(2, &[0xF8]).is_some(), "clock has no channel");
    }

    #[test]
    fn a_remap_moves_every_channel_to_one() {
        let route = Route { remap: Some(3), ..route(Kinds::ALL) };
        assert_eq!(bytes(route.pass(2, &[0x90, 60, 100])), Some(vec![0x93, 60, 100]));
        assert_eq!(bytes(route.pass(2, &[0x8F, 60, 0])), Some(vec![0x83, 60, 0]));
        assert_eq!(bytes(route.pass(2, &[0xE5, 0x10, 0x50])), Some(vec![0xE3, 0x10, 0x50]));
        assert_eq!(bytes(route.pass(2, &[0xFA])), Some(vec![0xFA]), "Start stays as it is");
        // Filtered and moved: channel 1 to channel 10
        let drums = Route { channel: Some(0), remap: Some(9), ..route };
        assert_eq!(bytes(drums.pass(2, &[0x90, 36, 90])), Some(vec![0x99, 36, 90]));
        assert_eq!(drums.pass(2, &[0x91, 36, 90]), None);
    }

    #[test]
    fn a_route_packs_into_a_word() {
        for route in [
            route(Kinds::default()),
            Route { slot: 7, port: 7, channel: Some(15), remap: Some(0), kinds: Kinds::ALL },
            Route { slot: 0, port: 0, channel: Some(0), remap: None, kinds: Kinds::NONE },
        ] {
            assert_eq!(Route::unpack(route.pack()), Some(route));
        }
        assert_eq!(Route::unpack(0), None);
    }

    #[test]
    fn routes_survive_the_settings() {
        let bus = ThruBus::new();
        let mut moved = ThruRoute::new("2- Fantom-06", 1);
        moved.channel = Some(0);
        moved.remap = Some(15);
        moved.kinds = Kinds::NOTES.with(Kinds::CLOCK, true);
        let mut off = ThruRoute::new("Polyend Tracker", 7);
        off.on = false;
        bus.edit(|routes| *routes = vec![moved.clone(), off.clone(), ThruRoute::new("Digitakt", 0)]);
        let setting = bus.setting();
        assert_eq!(setting.lines().next(), Some("on\t2- Fantom-06\tB\t1\t16\tnotes,clock"));
        let back = ThruBus::new();
        back.restore(&format!("{setting}\nnot a route\non\t\tA\tall\tsame\tnotes"));
        assert_eq!(back.routes(), [moved, off, ThruRoute::new("Digitakt", 0)], "and lines it can't read are left out");
    }

    #[test]
    fn thru_only_inputs_are_found_again_and_kept() {
        let bus = ThruBus::new();
        bus.edit(|routes| routes.push(ThruRoute::new("2- Fantom-06", 1)));
        assert!(!bus.is_thru_only("2- Fantom-06"));
        assert_eq!(bus.thru_only_slots([(0, "MiniLab3"), (3, "2- Fantom-06")]), 0);
        bus.set_thru_only("2- Fantom-06", true);
        assert!(bus.is_thru_only("3- Fantom-06"), "renumbered after a reboot");
        assert!(!bus.is_thru_only("MiniLab3"));
        assert_eq!(bus.thru_only_slots([(0, "MiniLab3"), (3, "3- Fantom-06")]), 0b1000, "any device leaves slot 3 out");
        assert_eq!(bus.thru_only_slots([(0, "MiniLab3")]), 0, "not open, nothing to leave out");
        // Kept a line of its own, after the routes
        let setting = bus.setting();
        assert_eq!(setting, "on\t2- Fantom-06\tB\tall\tsame\tnotes,cc,bend,program\nonly\t2- Fantom-06");
        let back = ThruBus::new();
        back.restore(&setting);
        assert!(back.is_thru_only("2- Fantom-06"));
        assert_eq!(back.routes(), bus.routes(), "the line isn't read as a route");
        // Marked again, it isn't kept twice; unmarked, it's gone
        bus.set_thru_only("3- Fantom-06", true);
        assert_eq!(bus.setting().matches("only").count(), 1);
        bus.set_thru_only("2- Fantom-06", false);
        assert!(!bus.is_thru_only("2- Fantom-06") && !bus.setting().contains("only"));
    }

    fn ports(chosen: &[(usize, &str)]) -> ([String; PORTS], [Option<String>; PORTS]) {
        let mut names: [String; PORTS] = Default::default();
        for &(port, name) in chosen {
            names[port] = name.to_string();
        }
        let found = std::array::from_fn(|port| Some(names[port].clone()).filter(|n| !n.is_empty() && !n.contains("unplugged")));
        (names, found)
    }

    #[test]
    fn following_says_what_each_route_does() {
        let bus = ThruBus::new();
        let (chosen, found) = ports(&[(0, "MicroFreak"), (1, "Fantom-06"), (2, "Volca unplugged")]);
        bus.edit(|routes| {
            routes.push(ThruRoute::new("Fantom-06", 0));
            routes.push(ThruRoute::new("Fantom-06", 1));
            routes.push(ThruRoute::new("Fantom-06", 3));
            routes.push(ThruRoute::new("Digitakt", 0));
            routes.push(ThruRoute::new("Fantom-06", 2));
            routes.push(ThruRoute { on: false, ..ThruRoute::new("Fantom-06", 0) });
        });
        let slot_of = |name: &str| (name == "Fantom-06").then_some(4);
        let states = bus.follow(slot_of, &chosen, &found);
        use RouteState::*;
        assert_eq!(states, [Passing, Loop, NoDevice, Closed, Unplugged, Off]);
        assert_eq!(bus.route(0), Some(Route { slot: 4, port: 0, channel: None, remap: None, kinds: Kinds::default() }));
        for refused in 1..ROUTES {
            if refused != 4 {
                assert_eq!(bus.route(refused), None, "route {refused}");
            }
        }
        // A route taken away stops
        bus.edit(|routes| routes.clear());
        assert!(bus.follow(slot_of, &chosen, &found).is_empty());
        assert_eq!(bus.route(0), None);
    }

    #[test]
    fn loops_are_refused_however_the_system_names_them() {
        assert!(is_loop("MicroFreak", "MicroFreak"));
        assert!(is_loop("2- Digitakt", "3- Digitakt"), "renumbered");
        assert!(is_loop("MIDIIN2 (Fantom-06)", "MIDIOUT2 (Fantom-06)"));
        assert!(is_loop("Fantom-06", "MIDIOUT2 (Fantom-06)"), "another port of the same device");
        assert!(is_loop("Digitakt:Digitakt MIDI 1 24:0", "Digitakt:Digitakt MIDI 1 24:0"));
        assert!(!is_loop("Fantom-06", "MicroFreak"));
        assert!(!is_loop("Digitakt", "Digitone"));
    }

    #[test]
    fn the_input_passes_down_every_route_from_it() {
        let bus: &'static ThruBus = Box::leak(Box::new(ThruBus::new()));
        assert!(bus.open_input().is_none(), "no sender, no ring");
        bus.start_draining();
        let mut input = bus.open_input().unwrap();
        let mut ring = bus.take_ring().unwrap();
        let (chosen, found) = ports(&[(0, "MicroFreak"), (5, "Volca Keys")]);
        let mut to_volca = ThruRoute::new("Fantom-06", 5);
        to_volca.remap = Some(2);
        bus.edit(|routes| *routes = vec![ThruRoute::new("Fantom-06", 0), to_volca, ThruRoute::new("Digitakt", 0)]);
        bus.follow(|name| (name == "Fantom-06").then_some(1), &chosen, &found);
        assert!(input.forward(1, &[0x90, 60, 100]));
        assert!(!input.forward(0, &[0x90, 60, 100]), "an input no route hears");
        assert!(!input.forward(1, &[0xFE]));
        let passed: Vec<Forward> = std::iter::from_fn(|| ring.pop().ok()).collect();
        assert_eq!(passed, [
            Forward { route: 0, port: 0, message: Message::note_on(0, 60, 100) },
            Forward { route: 1, port: 5, message: Message::note_on(2, 60, 100) },
        ]);
        // The input closes: its routes stop before the UI looks again
        input.forget(1);
        assert!(!input.forward(1, &[0x90, 60, 100]));
    }
}
