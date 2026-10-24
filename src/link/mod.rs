//! The link to Seoul: a patch's control signals, sent over OSC.
//!
//! Seoul is a music visualizer. It hears Soba through loopback already, but
//! it can only guess at what a patch means from its sound. A **Link Out**
//! module ([`crate::modules::link_out`]) sends it the patch's intent
//! instead: eight control lanes, four gates, and the Clock's tempo and
//! position. The contract is Seoul's `docs/LINK.md`. In short, every 5 ms
//! one OSC bundle goes to `127.0.0.1:<port>` holding
//!
//! | Address | Arguments |
//! | --- | --- |
//! | `/soba/cv/1` … `/soba/cv/8` | `f` value |
//! | `/soba/gate/1` … `/soba/gate/4` | `i` 0 or 1 |
//! | `/soba/clock` | `f` bpm, `f` beat_phase, `i` beat_count, `i` bar |
//! | `/soba/transport` | `i` playing |
//! | `/soba/hello` | `s` version, once a second |
//!
//! Two threads meet on a [`LinkBus`]: a fixed set of atomics, one
//! [`PortSlot`] per port a Link Out can pick. The audio thread stores each
//! lane's newest value there once a block, which never allocates, locks or
//! touches a socket. On the desktop a sender thread ([`sender`]) reads the
//! bus every 5 ms and sends what it finds. While no Link Out runs on a port,
//! nothing is sent to it, and Seoul sees the link drop.
//!
//! The same thread carries MIDI out ([`midi`]): a second protocol on the
//! one transport, for MIDI Out modules playing hardware and other apps. In
//! the browser, which has no OSC, a timer sends MIDI Out ([`web_sender`]).

pub mod midi;
pub mod midi_clock;
pub mod midi_sender;
pub mod midi_thru;
pub mod osc;
pub mod ping;
#[cfg(not(target_arch = "wasm32"))]
pub mod sender;
#[cfg(target_arch = "wasm32")]
pub mod web_sender;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};

/// Control lanes a Link Out sends: `/soba/cv/1` to `/soba/cv/8`.
pub const CV_LANES: usize = 8;
/// Gates a Link Out sends: `/soba/gate/1` to `/soba/gate/4`.
pub const GATE_LANES: usize = 4;

/// The ports a Link Out can send to, in the order of its Port choice. Seoul
/// listens on the first unless its `seoul.toml` says otherwise; a second
/// Seoul takes another.
pub const PORTS: [u16; 4] = [9137, 9138, 9139, 9140];
/// [`PORTS`] as the Port choice shows them.
pub const PORT_LABELS: &[&str] = &["9137", "9138", "9139", "9140"];

/// The version `/soba/hello` sends.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use crate::dsp::gate::GATE_THRESHOLD;

/// What the sender knows about a port, for a Link Out's face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    /// Nothing is sending: no sender thread (the browser, the offline
    /// renderer), or no Link Out on this port.
    Idle = 0,
    /// Bundles are going out, and nothing has refused them.
    Sending = 1,
    /// Bundles are going out, but the port is closed: Seoul isn't running,
    /// or listens on another port.
    NoListener = 2,
    /// The socket failed.
    Failed = 3,
}

impl LinkStatus {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Sending,
            2 => Self::NoListener,
            3 => Self::Failed,
            _ => Self::Idle,
        }
    }
}

/// One port's lanes: what every Link Out on that port last stored.
///
/// Each value has a counter beside it that its writer bumps whenever it
/// stores, so the sender can tell a lane still being written from one left
/// behind (its cable pulled, its module deleted) and send 0 for that. Two
/// Link Outs on one port share it: each writes only the lanes it has
/// cables in.
pub struct PortSlot {
    /// Bumped once a block by every Link Out on this port.
    pulse: AtomicU32,
    /// f32 bits.
    cv: [AtomicU32; CV_LANES],
    cv_written: [AtomicU32; CV_LANES],
    gate: [AtomicBool; GATE_LANES],
    /// Rising edges so far, so a trigger shorter than a send isn't lost.
    gate_edges: [AtomicU32; GATE_LANES],
    gate_written: [AtomicU32; GATE_LANES],
    /// The patch's transport: whether it has a Clock, whether it plays,
    /// its tempo (f32 bits), its beat (f64 bits) and beats in a bar.
    has_clock: AtomicBool,
    playing: AtomicBool,
    bpm: AtomicU32,
    beat: AtomicU64,
    beats_per_bar: AtomicU8,
    /// A [`LinkStatus`], written by the sender.
    status: AtomicU8,
}

impl PortSlot {
    const fn new() -> Self {
        Self {
            pulse: AtomicU32::new(0),
            cv: [const { AtomicU32::new(0) }; CV_LANES],
            cv_written: [const { AtomicU32::new(0) }; CV_LANES],
            gate: [const { AtomicBool::new(false) }; GATE_LANES],
            gate_edges: [const { AtomicU32::new(0) }; GATE_LANES],
            gate_written: [const { AtomicU32::new(0) }; GATE_LANES],
            has_clock: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            bpm: AtomicU32::new(0),
            beat: AtomicU64::new(0),
            beats_per_bar: AtomicU8::new(4),
            status: AtomicU8::new(LinkStatus::Idle as u8),
        }
    }

    // --- The audio thread's side. REAL-TIME SAFE: stores to atomics ---

    /// Says a Link Out on this port ran a block.
    #[inline]
    pub fn pulse(&self) {
        self.pulse.fetch_add(1, Relaxed);
    }

    /// Stores control lane `lane`'s newest value.
    #[inline]
    pub fn set_cv(&self, lane: usize, value: f32) {
        self.cv[lane].store(value.to_bits(), Relaxed);
        self.cv_written[lane].fetch_add(1, Relaxed);
    }

    /// Stores gate `lane`'s state at the end of a block, and the rising
    /// edges it made in the block.
    #[inline]
    pub fn set_gate(&self, lane: usize, high: bool, rising_edges: u32) {
        self.gate[lane].store(high, Relaxed);
        if rising_edges > 0 {
            self.gate_edges[lane].fetch_add(rising_edges, Relaxed);
        }
        self.gate_written[lane].fetch_add(1, Relaxed);
    }

    /// Stores the patch's transport: `None` when it has no Clock.
    #[inline]
    pub fn set_clock(&self, clock: Option<ClockState>) {
        match clock {
            Some(clock) => {
                self.bpm.store(clock.bpm.to_bits(), Relaxed);
                self.beat.store(clock.beat.to_bits(), Relaxed);
                self.beats_per_bar.store(clock.beats_per_bar.max(1), Relaxed);
                self.playing.store(clock.playing, Relaxed);
                self.has_clock.store(true, Relaxed);
            }
            None => self.has_clock.store(false, Relaxed),
        }
    }

    // --- The sender's side ---

    /// Everything stored, as one reading.
    pub fn read(&self) -> SlotReading {
        SlotReading {
            pulse: self.pulse.load(Relaxed),
            cv: std::array::from_fn(|i| f32::from_bits(self.cv[i].load(Relaxed))),
            cv_written: std::array::from_fn(|i| self.cv_written[i].load(Relaxed)),
            gate: std::array::from_fn(|i| self.gate[i].load(Relaxed)),
            gate_edges: std::array::from_fn(|i| self.gate_edges[i].load(Relaxed)),
            gate_written: std::array::from_fn(|i| self.gate_written[i].load(Relaxed)),
            clock: self.has_clock.load(Relaxed).then(|| ClockState {
                playing: self.playing.load(Relaxed),
                bpm: f32::from_bits(self.bpm.load(Relaxed)),
                beat: f64::from_bits(self.beat.load(Relaxed)),
                beats_per_bar: self.beats_per_bar.load(Relaxed),
            }),
        }
    }

    pub fn set_status(&self, status: LinkStatus) {
        self.status.store(status as u8, Relaxed);
    }

    /// What the sender last knew about this port.
    pub fn status(&self) -> LinkStatus {
        LinkStatus::from_code(self.status.load(Relaxed))
    }
}

/// The patch's Clock, as a Link Out sends it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockState {
    pub playing: bool,
    pub bpm: f32,
    /// Beats since the transport started; beat 0 is the first downbeat.
    pub beat: f64,
    pub beats_per_bar: u8,
}

impl ClockState {
    /// `/soba/clock`'s arguments: bpm, beat_phase, beat_count and bar.
    pub fn message(&self) -> (f32, f32, i32, i32) {
        let beat_count = self.beat.floor();
        let phase = (self.beat - beat_count) as f32;
        let bar = (beat_count / self.beats_per_bar.max(1) as f64).floor();
        (self.bpm, phase.clamp(0.0, 1.0), beat_count as i32, bar as i32)
    }
}

/// One port's lanes as the sender read them.
#[derive(Clone, Copy, Debug)]
pub struct SlotReading {
    pub pulse: u32,
    pub cv: [f32; CV_LANES],
    pub cv_written: [u32; CV_LANES],
    pub gate: [bool; GATE_LANES],
    pub gate_edges: [u32; GATE_LANES],
    pub gate_written: [u32; GATE_LANES],
    pub clock: Option<ClockState>,
}

/// Every port's lanes.
pub struct LinkBus {
    slots: [PortSlot; PORTS.len()],
}

impl LinkBus {
    pub const fn new() -> Self {
        Self { slots: [const { PortSlot::new() }; PORTS.len()] }
    }

    /// The slot for the `index`th of [`PORTS`] (clamped, so a stray Port
    /// value still lands somewhere).
    pub fn slot(&self, index: usize) -> &PortSlot {
        &self.slots[index.min(PORTS.len() - 1)]
    }

    pub fn slots(&self) -> &[PortSlot] {
        &self.slots
    }
}

impl Default for LinkBus {
    fn default() -> Self {
        Self::new()
    }
}

/// The bus every Link Out writes to and the sender reads.
pub static BUS: LinkBus = LinkBus::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_message_splits_the_beat() {
        let clock = ClockState { playing: true, bpm: 120.0, beat: 9.25, beats_per_bar: 4 };
        assert_eq!(clock.message(), (120.0, 0.25, 9, 2));
        let three = ClockState { beats_per_bar: 3, ..clock };
        assert_eq!(three.message().3, 3);
    }

    #[test]
    fn a_count_in_before_beat_zero_counts_down() {
        let clock = ClockState { playing: true, bpm: 90.0, beat: -0.75, beats_per_bar: 4 };
        let (_, phase, beat, bar) = clock.message();
        assert!((phase - 0.25).abs() < 1e-6);
        assert_eq!((beat, bar), (-1, -1));
    }

    #[test]
    fn the_slot_reads_back_what_was_stored() {
        let bus = LinkBus::new();
        let slot = bus.slot(1);
        slot.pulse();
        slot.set_cv(2, -0.5);
        slot.set_gate(3, true, 2);
        slot.set_clock(Some(ClockState { playing: true, bpm: 128.0, beat: 4.5, beats_per_bar: 4 }));
        let reading = slot.read();
        assert_eq!(reading.pulse, 1);
        assert_eq!(reading.cv[2], -0.5);
        assert_eq!(reading.cv_written[2], 1);
        assert_eq!(reading.cv_written[0], 0);
        assert!(reading.gate[3]);
        assert_eq!(reading.gate_edges[3], 2);
        assert_eq!(reading.clock.unwrap().beat, 4.5);
        slot.set_clock(None);
        assert!(slot.read().clock.is_none());
        // Other ports are untouched
        assert_eq!(bus.slot(0).read().pulse, 0);
        assert_eq!(bus.slot(99).read().pulse, 0, "clamped to the last port");
    }
}
