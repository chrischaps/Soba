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
//! **Controllers** (#439) travel as a [`Control`]: a CC lane's value, a
//! 14-bit pair, an NRPN, the bend or the pressure, each one thing however
//! many messages it takes. A port with a **budget** (a DIN cable carries
//! about a thousand messages a second, see [`MidiBus::set_budget`]) keeps
//! only the newest value of each while it's full, so notes, programs and
//! clock, which travel as plain [`Message`]s, are never held up behind them.
//!
//! **SysEx** (#440) goes whole, any length, from the UI
//! ([`MidiBus::send_sysex`]) or, timed to a sample, from the audio thread
//! through a pool of slots made before it starts ([`sysex`](super::sysex)).
//!
//! Unlike the Seoul link's newest-value atomics, MIDI needs every message,
//! in order and on time: a lost Note Off is a stuck note. So the ring
//! carries events, and the sender keeps track of which notes each module
//! is holding, to let them go if the module disappears (deleted, or a new
//! patch loaded) or stops running.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use web_time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

use super::sysex::{self, SysexError, SysexPool};
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
/// a few dozen, and the sender drains it every millisecond. CCs sent where
/// they change (#439), as often as every millisecond a lane, fill more: a
/// 40 ms buffer of four lanes, the bend, and pressure on every voice is
/// about a thousand.
pub const RING: usize = 4096;

/// Status bytes this module sends.
pub mod status {
    pub const NOTE_OFF: u8 = 0x80;
    pub const NOTE_ON: u8 = 0x90;
    pub const POLY_PRESSURE: u8 = 0xA0;
    pub const CONTROL_CHANGE: u8 = 0xB0;
    pub const PROGRAM_CHANGE: u8 = 0xC0;
    pub const CHANNEL_PRESSURE: u8 = 0xD0;
    pub const PITCH_BEND: u8 = 0xE0;
    pub const SONG_POSITION: u8 = 0xF2;
    pub const CLOCK: u8 = 0xF8;
    pub const START: u8 = 0xFA;
    pub const CONTINUE: u8 = 0xFB;
    pub const STOP: u8 = 0xFC;
}

/// CC 123, All Notes Off.
pub const ALL_NOTES_OFF: u8 = 123;

/// The controller numbers MIDI 1.0 itself gives a meaning, as the MIDI
/// Association's Control Change table lists them
/// (midi.org/midi-1-0-control-change-messages).
pub mod cc {
    /// Bank Select MSB.
    pub const BANK_MSB: u8 = 0;
    /// Data Entry MSB: the value of the RPN or NRPN chosen.
    pub const DATA_ENTRY: u8 = 6;
    /// Bank Select LSB.
    pub const BANK_LSB: u8 = 32;
    /// Data Entry LSB.
    pub const DATA_ENTRY_LSB: u8 = 38;
    /// Which NRPN: its LSB and its MSB.
    pub const NRPN_LSB: u8 = 98;
    pub const NRPN_MSB: u8 = 99;
    /// Which RPN: its LSB and its MSB.
    pub const RPN_LSB: u8 = 100;
    pub const RPN_MSB: u8 = 101;
    /// The controllers 0 to 31 each have their fine half this many higher.
    pub const LSB_OFFSET: u8 = 32;
}

/// RPN 0, Pitch Bend Sensitivity: Data Entry MSB in semitones, LSB in cents.
pub const RPN_BEND_RANGE: u16 = 0;
/// RPN 127/127, the Null RPN: closes the one chosen, so a stray Data Entry
/// changes nothing.
pub const RPN_NULL: u16 = 0x3FFF;

/// The highest 14-bit value, as bends, 14-bit CCs and NRPNs carry.
pub const MAX_14: u16 = 0x3FFF;
/// A bend at rest: the middle of 14 bits.
pub const BEND_CENTER: u16 = 0x2000;

/// How many messages a second a DIN cable carries: 31,250 baud is 3,125
/// bytes a second, about a thousand three-byte messages. A port's budget
/// set to this keeps a DIN interface from falling behind (#439).
pub const DIN_RATE: u32 = 1000;

/// The most messages a second a budget can allow: more than any cable needs.
pub const MAX_BUDGET: u32 = 100_000;

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
    /// A message to send when sample `sample` is heard. A port's budget
    /// never holds these back: notes, programs and clock go this way.
    Message { sample: u64, message: Message },
    /// A controller's value to send when sample `sample` is heard. While
    /// its port is over budget, only the newest of each goes (#439).
    Control { sample: u64, control: Control },
    /// A SysEx message to send when sample `sample` is heard, in slot
    /// `slot` of the bus's [`SysexPool`] (#440). The sender frees the slot
    /// once it's sent. It goes whole, as the port's budget allows.
    Sysex { sample: u64, slot: u8 },
}

/// A few messages that go out together and in order: a controller's value
/// (an NRPN is four), or a Bank Select and its Program Change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Burst {
    messages: [Message; Burst::MAX],
    len: u8,
}

impl Burst {
    /// An RPN is the longest: which, its value, and the Null RPN.
    pub const MAX: usize = 6;

    const EMPTY: Self = Self { messages: [Message::real_time(0); Self::MAX], len: 0 };

    fn push(&mut self, message: Message) {
        if (self.len as usize) < Self::MAX {
            self.messages[self.len as usize] = message;
            self.len += 1;
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages[..self.len as usize]
    }

    /// How many bytes it is on the wire.
    pub fn bytes(&self) -> usize {
        self.messages().iter().map(|m| m.bytes().len()).sum()
    }
}

/// One controller's value, however many messages it takes to send (#439).
/// Values are as sent: 7 bits (0 to 127) or 14 bits (0 to [`MAX_14`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// A 7-bit CC.
    Cc { channel: u8, number: u8, value: u8 },
    /// A 14-bit CC: its coarse half on `number` (0 to 31), then its fine
    /// half on `number + 32`.
    Cc14 { channel: u8, number: u8, value: u16 },
    /// A Non-Registered Parameter: CC 99 and 98 choose it, then Data
    /// Entry (CC 6 and 38) sets it.
    Nrpn { channel: u8, parameter: u16, value: u16 },
    /// A Registered Parameter (the bend range is RPN 0): CC 101 and 100
    /// choose it, Data Entry sets it, and the Null RPN closes it.
    Rpn { channel: u8, parameter: u16, value: u16 },
    /// Pitch Bend, [`BEND_CENTER`] at rest.
    Bend { channel: u8, value: u16 },
    /// Channel Pressure.
    Pressure { channel: u8, value: u8 },
    /// Polyphonic Key Pressure on one note.
    PolyPressure { channel: u8, note: u8, value: u8 },
}

impl Control {
    /// A CC lane's value, 0 to 1, as `mode` sends it: on CC `number`, or
    /// on NRPN `parameter`.
    pub fn lane(mode: CcMode, channel: u8, number: u8, parameter: u16, value: f32) -> Self {
        let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        let fine = (value * MAX_14 as f32).round() as u16;
        match mode {
            CcMode::Bits7 => Self::Cc { channel, number: number & 0x7F, value: (value * 127.0).round() as u8 },
            CcMode::Bits14 => Self::Cc14 { channel, number: number.min(cc::LSB_OFFSET - 1), value: fine },
            CcMode::Nrpn => Self::Nrpn { channel, parameter: parameter & MAX_14, value: fine },
        }
    }

    /// A bend of -1 to 1 (full throw down to full throw up), in 14 bits.
    pub fn bend(channel: u8, bend: f32) -> Self {
        let bend = if bend.is_finite() { bend.clamp(-1.0, 1.0) } else { 0.0 };
        let value = (BEND_CENTER as f32 + bend * BEND_CENTER as f32).round().min(MAX_14 as f32) as u16;
        Self::Bend { channel, value }
    }

    /// 0 to 1 as 7 bits.
    pub fn seven(value: f32) -> u8 {
        if value.is_finite() {
            (value.clamp(0.0, 1.0) * 127.0).round() as u8
        } else {
            0
        }
    }

    /// The channel it goes out on, from 0.
    pub fn channel(&self) -> u8 {
        match *self {
            Self::Cc { channel, .. }
            | Self::Cc14 { channel, .. }
            | Self::Nrpn { channel, .. }
            | Self::Rpn { channel, .. }
            | Self::Bend { channel, .. }
            | Self::Pressure { channel, .. }
            | Self::PolyPressure { channel, .. } => channel & 0x0F,
        }
    }

    /// What it sets, whatever the value: while a port is over budget, a
    /// newer value with the same key takes an older one's place.
    pub fn key(&self) -> u32 {
        let (kind, number) = match *self {
            Self::Cc { number, .. } => (0, number as u32),
            Self::Cc14 { number, .. } => (1, number as u32),
            Self::Nrpn { parameter, .. } => (2, parameter as u32),
            Self::Rpn { parameter, .. } => (3, parameter as u32),
            Self::Bend { .. } => (4, 0),
            Self::Pressure { .. } => (5, 0),
            Self::PolyPressure { note, .. } => (6, note as u32),
        };
        kind << 24 | (self.channel() as u32) << 16 | number
    }

    /// The messages that send it, in order: a 14-bit value's coarse half
    /// first, then its fine half.
    pub fn burst(&self) -> Burst {
        let mut burst = Burst::EMPTY;
        let split = |value: u16| (((value >> 7) & 0x7F) as u8, (value & 0x7F) as u8);
        match *self {
            Self::Cc { channel, number, value } => burst.push(Message::control(channel, number, value)),
            Self::Cc14 { channel, number, value } => {
                let (msb, lsb) = split(value);
                burst.push(Message::control(channel, number, msb));
                burst.push(Message::control(channel, number + cc::LSB_OFFSET, lsb));
            }
            Self::Nrpn { channel, parameter, value } | Self::Rpn { channel, parameter, value } => {
                let nrpn = matches!(self, Self::Nrpn { .. });
                let (which_msb, which_lsb) = if nrpn { (cc::NRPN_MSB, cc::NRPN_LSB) } else { (cc::RPN_MSB, cc::RPN_LSB) };
                let (parameter_msb, parameter_lsb) = split(parameter);
                let (msb, lsb) = split(value);
                burst.push(Message::control(channel, which_msb, parameter_msb));
                burst.push(Message::control(channel, which_lsb, parameter_lsb));
                burst.push(Message::control(channel, cc::DATA_ENTRY, msb));
                burst.push(Message::control(channel, cc::DATA_ENTRY_LSB, lsb));
                if !nrpn {
                    let (null_msb, null_lsb) = split(RPN_NULL);
                    burst.push(Message::control(channel, cc::RPN_MSB, null_msb));
                    burst.push(Message::control(channel, cc::RPN_LSB, null_lsb));
                }
            }
            Self::Bend { channel, value } => burst.push(Message::pitch_bend(channel, value)),
            Self::Pressure { channel, value } => burst.push(Message::channel_pressure(channel, value)),
            Self::PolyPressure { channel, note, value } => burst.push(Message::poly_pressure(channel, note, value)),
        }
        burst
    }
}

/// How a CC lane sends its value (#439).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CcMode {
    /// One CC, 0 to 127.
    #[default]
    Bits7 = 0,
    /// Two CCs, `n` and `n + 32`, for 16,384 steps: the controllers 0 to 31.
    Bits14 = 1,
    /// An NRPN with 14 bits of value.
    Nrpn = 2,
}

impl CcMode {
    pub const ALL: [Self; 3] = [Self::Bits7, Self::Bits14, Self::Nrpn];
    /// As a lane's menu names them, in [`ALL`](Self::ALL)'s order.
    pub const LABELS: &'static [&'static str] = &["7-bit", "14-bit", "NRPN"];

    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Self::Bits14,
            2 => Self::Nrpn,
            _ => Self::Bits7,
        }
    }
}

/// A Program Change, and the bank to choose first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program {
    /// Bank Select MSB (CC 0), if the device wants one.
    pub bank_msb: Option<u8>,
    /// Bank Select LSB (CC 32), if the device wants one.
    pub bank_lsb: Option<u8>,
    /// The program, 0 to 127 as sent.
    pub program: u8,
}

impl Program {
    /// The Bank Select, MSB then LSB, and then the Program Change, which
    /// is what makes a device take the bank up.
    pub fn burst(&self, channel: u8) -> Burst {
        let mut burst = Burst::EMPTY;
        if let Some(msb) = self.bank_msb {
            burst.push(Message::control(channel, cc::BANK_MSB, msb));
        }
        if let Some(lsb) = self.bank_lsb {
            burst.push(Message::control(channel, cc::BANK_LSB, lsb));
        }
        burst.push(Message::program(channel, self.program));
        burst
    }
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

    /// Program Change: `program` 0 to 127, as sent (a manual that counts
    /// its programs from 1 calls this one `program + 1`).
    pub const fn program(channel: u8, program: u8) -> Self {
        Self::two(status::PROGRAM_CHANGE | (channel & 0x0F), program & 0x7F)
    }

    /// Pitch Bend: 14 bits, [`BEND_CENTER`] at rest, sent LSB first.
    pub const fn pitch_bend(channel: u8, value: u16) -> Self {
        Self::three(status::PITCH_BEND | (channel & 0x0F), (value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8)
    }

    /// Channel Pressure: aftertouch for the whole channel.
    pub const fn channel_pressure(channel: u8, value: u8) -> Self {
        Self::two(status::CHANNEL_PRESSURE | (channel & 0x0F), value & 0x7F)
    }

    /// Polyphonic Key Pressure: aftertouch for one note.
    pub const fn poly_pressure(channel: u8, note: u8, value: u8) -> Self {
        Self::three(status::POLY_PRESSURE | (channel & 0x0F), note & 0x7F, value & 0x7F)
    }

    /// Song Position Pointer, in sixteenths from the top.
    pub const fn song_position(sixteenths: u16) -> Self {
        Self::three(status::SONG_POSITION, (sixteenths & 0x7F) as u8, ((sixteenths >> 7) & 0x7F) as u8)
    }

    /// A one-byte real-time message: clock, start, continue or stop.
    pub const fn real_time(byte: u8) -> Self {
        Self { bytes: [byte, 0, 0], len: 1 }
    }

    const fn two(a: u8, b: u8) -> Self {
        Self { bytes: [a, b, 0], len: 2 }
    }

    const fn three(a: u8, b: u8, c: u8) -> Self {
        Self { bytes: [a, b, c], len: 3 }
    }

    /// The message in `bytes` as it came in, if it's one to three bytes:
    /// for passing MIDI through (#446).
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let len = bytes.len();
        if !(1..=3).contains(&len) {
            return None;
        }
        let mut message = Self { bytes: [0; 3], len: len as u8 };
        message.bytes[..len].copy_from_slice(bytes);
        Some(message)
    }

    /// The channel of a channel message (a note, CC, bend, pressure or
    /// program), from 0.
    pub fn channel(&self) -> Option<u8> {
        (self.bytes[0] < 0xF0).then_some(self.bytes[0] & 0x0F)
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
    /// Controller values the port's budget let go of, or folded into newer
    /// ones, while it was full (#439).
    pub thinned: u32,
    /// SysEx messages sent (#440).
    pub sysex: u32,
    /// How long the last SysEx sent was, in bytes.
    pub sysex_bytes: u32,
    /// SysEx bytes still waiting to go: a long dump pacing itself.
    pub sysex_waiting: u32,
    /// Whether the last message sent, but for real-time ones, was SysEx.
    pub last_sysex: bool,
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
    /// What each port has sent: messages, clock ticks, the last message
    /// packed (see [`Message::packed`]), values thinned, and the SysEx
    /// sent, the last one's length, the bytes waiting and whether it was
    /// the last thing sent (see [`Traffic`]).
    sent: [[AtomicU32; 8]; PORTS],
    /// Each port's budget, in messages a second: 0 for none (#439).
    budgets: [AtomicU32; PORTS],
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
    /// Asks the sender to open every device afresh: the browser has
    /// allowed SysEx, which the outputs it opened before can't send.
    reconnect: AtomicBool,
    /// The audio thread's SysEx slots, made when a sender starts.
    sysex_pool: OnceLock<Arc<SysexPool>>,
    /// SysEx the UI has sent since the sender last looked, by port.
    ui_sysex: Mutex<Vec<(u8, Box<[u8]>)>>,
    /// The sender's thread, to wake when the UI sends SysEx.
    #[cfg(not(target_arch = "wasm32"))]
    waker: OnceLock<std::thread::Thread>,
}

impl MidiBus {
    pub const fn new() -> Self {
        Self {
            draining: AtomicBool::new(false),
            new_streams: Mutex::new(Vec::new()),
            devices: Mutex::new(Vec::new()),
            chosen: Mutex::new([const { String::new() }; PORTS]),
            status: [const { AtomicU8::new(PortStatus::Idle as u8) }; PORTS],
            sent: [const { [const { AtomicU32::new(0) }; 8] }; PORTS],
            budgets: [const { AtomicU32::new(0) }; PORTS],
            clock: [const { AtomicU8::new(ClockMode::Off as u8) }; PORTS],
            offsets: [const { AtomicU32::new(0) }; PORTS],
            new_clock: Mutex::new(None),
            leader: AtomicU8::new(NO_LEADER),
            same_device: [const { AtomicU8::new(0) }; MIDI_INPUTS],
            latency_micros: AtomicU64::new(0),
            rescan: AtomicBool::new(true),
            reconnect: AtomicBool::new(false),
            sysex_pool: OnceLock::new(),
            ui_sysex: Mutex::new(Vec::new()),
            #[cfg(not(target_arch = "wasm32"))]
            waker: OnceLock::new(),
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

    /// The pool a module claims SysEx slots from on the audio thread, or
    /// `None` while no sender drains the bus.
    pub fn sysex_pool(&self) -> Option<Arc<SysexPool>> {
        self.sysex_pool.get().cloned()
    }

    /// The pool, for the sender to take up the slots modules send.
    pub fn pool(&self) -> Option<&SysexPool> {
        self.sysex_pool.get().map(|pool| &**pool)
    }

    // --- For the UI ---

    /// Sends one SysEx message on port `port`, whole, as soon as the
    /// port's budget has room for it, after any SysEx sent before it.
    pub fn send_sysex(&self, port: usize, message: &[u8]) -> Result<(), SysexError> {
        sysex::check(message)?;
        if message.len() > sysex::MAX_UI {
            return Err(SysexError::TooLong);
        }
        if !self.draining.load(Relaxed) {
            return Err(SysexError::NoSender);
        }
        self.ui_sysex.lock().map_err(|_| SysexError::NoSender)?.push((port.min(PORTS - 1) as u8, message.into()));
        // Sent at once, not on the sender's next idle look
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(thread) = self.waker.get() {
            thread.unpark();
        }
        #[cfg(target_arch = "wasm32")]
        super::web_sender::send_now();
        Ok(())
    }

    /// Sends every message in a `.syx` file on port `port`, in order (see
    /// [`sysex::split`]). Returns how many, and how many bytes.
    pub fn send_sysex_file(&self, port: usize, file: &[u8]) -> Result<(usize, usize), SysexError> {
        let messages = sysex::split(file)?;
        if messages.iter().any(|message| message.len() > sysex::MAX_UI) {
            return Err(SysexError::TooLong);
        }
        for message in &messages {
            self.send_sysex(port, message)?;
        }
        Ok((messages.len(), messages.iter().map(|message| message.len()).sum()))
    }

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

    /// Port `port`'s budget in messages a second, or 0 for none: a USB
    /// device takes all Soba sends, a DIN cable about [`DIN_RATE`].
    pub fn budget(&self, port: usize) -> u32 {
        self.budgets.get(port).map_or(0, |b| b.load(Relaxed))
    }

    /// Every port's budget.
    pub fn budgets(&self) -> [u32; PORTS] {
        std::array::from_fn(|port| self.budget(port))
    }

    /// Holds port `port` to `per_second` messages a second, or to none
    /// with 0. While it's full, controllers send only their newest values;
    /// notes, programs and clock always go, on time.
    pub fn set_budget(&self, port: usize, per_second: u32) {
        if let Some(slot) = self.budgets.get(port) {
            slot.store(per_second.min(MAX_BUDGET), Relaxed);
        }
    }

    /// Every port's budget as the settings keep it: a line each.
    pub fn budget_setting(&self) -> String {
        self.budgets().map(|b| b.to_string()).join("\n")
    }

    /// Takes up what [`budget_setting`](Self::budget_setting) kept. A line
    /// it can't read leaves its port without one.
    pub fn restore_budget(&self, setting: &str) {
        for (port, line) in setting.lines().take(PORTS).enumerate() {
            self.set_budget(port, line.trim().parse().unwrap_or(0));
        }
    }

    /// Asks the sender to look for devices again, soon.
    pub fn rescan(&self) {
        self.rescan.store(true, Relaxed);
    }

    /// Asks the sender to open every device afresh, soon.
    pub fn reconnect(&self) {
        self.reconnect.store(true, Relaxed);
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
        let Some([messages, clocks, last, thinned, sysex, sysex_bytes, sysex_waiting, last_sysex]) = self.sent.get(port) else { return Traffic::default() };
        Traffic {
            messages: messages.load(Relaxed),
            clocks: clocks.load(Relaxed),
            last: Message::unpacked(last.load(Relaxed)),
            thinned: thinned.load(Relaxed),
            sysex: sysex.load(Relaxed),
            sysex_bytes: sysex_bytes.load(Relaxed),
            sysex_waiting: sysex_waiting.load(Relaxed),
            last_sysex: last_sysex.load(Relaxed) != 0,
        }
    }

    // --- For the sender ---

    /// Says a sender drains this bus from now on, and makes the SysEx
    /// pool its modules claim from.
    pub fn start_draining(&self) {
        self.sysex_pool.get_or_init(|| Arc::new(SysexPool::new()));
        self.draining.store(true, Relaxed);
    }

    /// The SysEx the UI has sent since the last look, by port, in order.
    pub fn take_sysex(&self) -> Vec<(u8, Box<[u8]>)> {
        self.ui_sysex.try_lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default()
    }

    /// Has the UI's SysEx wake `thread`, the sender's.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_waker(&self, thread: std::thread::Thread) {
        let _ = self.waker.set(thread);
    }

    pub fn take_reconnect(&self) -> bool {
        self.reconnect.swap(false, Relaxed)
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
        if let Some([messages, clocks, last, thinned, sysex, sysex_bytes, sysex_waiting, last_sysex]) = self.sent.get(port) {
            messages.store(traffic.messages, Relaxed);
            clocks.store(traffic.clocks, Relaxed);
            last.store(traffic.last.map_or(0, |m| m.packed()), Relaxed);
            thinned.store(traffic.thinned, Relaxed);
            sysex.store(traffic.sysex, Relaxed);
            sysex_bytes.store(traffic.sysex_bytes, Relaxed);
            sysex_waiting.store(traffic.sysex_waiting, Relaxed);
            last_sysex.store(traffic.last_sysex as u32, Relaxed);
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
        let traffic = Traffic { messages: 3, clocks: 96, last: Some(Message::control(2, 74, 64)), thinned: 7, sysex: 2, sysex_bytes: 163, sysex_waiting: 4104, last_sysex: true };
        bus.set_traffic(5, traffic);
        assert_eq!(bus.traffic(5), traffic);
        bus.set_traffic(5, Traffic { last: Some(Message::real_time(status::START)), ..traffic });
        assert_eq!(bus.traffic(5).last.unwrap().bytes(), &[status::START]);
        assert!(Message::real_time(status::STOP).is_real_time());
        assert!(!Message::song_position(4).is_real_time());
    }

    #[test]
    fn controllers_bend_pressure_and_programs_make_the_right_bytes() {
        let bytes = |burst: Burst| burst.messages().iter().map(|m| m.bytes().to_vec()).collect::<Vec<_>>();
        // Pitch Bend: LSB first, the middle 8192
        assert_eq!(bytes(Control::bend(0, 0.0).burst()), [vec![0xE0, 0x00, 0x40]]);
        assert_eq!(bytes(Control::bend(3, 1.0).burst()), [vec![0xE3, 0x7F, 0x7F]]);
        assert_eq!(bytes(Control::bend(3, -1.0).burst()), [vec![0xE3, 0, 0]]);
        assert_eq!(Control::bend(0, f32::NAN), Control::bend(0, 0.0), "nonsense rests");
        // Pressure, the channel's and a note's
        assert_eq!(bytes(Control::Pressure { channel: 1, value: 90 }.burst()), [vec![0xD1, 90]]);
        assert_eq!(bytes(Control::PolyPressure { channel: 1, note: 60, value: 90 }.burst()), [vec![0xA1, 60, 90]]);
        // Bank Select MSB and LSB, then the Program Change
        let program = Program { bank_msb: Some(85), bank_lsb: Some(1), program: 11 };
        assert_eq!(bytes(program.burst(15)), [vec![0xBF, 0, 85], vec![0xBF, 32, 1], vec![0xCF, 11]]);
        assert_eq!(bytes(Program { bank_msb: None, bank_lsb: None, program: 200 }.burst(0)), [vec![0xC0, 72]], "seven bits");
        // 14-bit: coarse on n, fine on n + 32
        let fine = Control::lane(CcMode::Bits14, 0, 1, 0, 1.0);
        assert_eq!(bytes(fine.burst()), [vec![0xB0, 1, 127], vec![0xB0, 33, 127]]);
        assert_eq!(Control::lane(CcMode::Bits14, 0, 74, 0, 0.0), Control::Cc14 { channel: 0, number: 31, value: 0 }, "pairs stop at 31");
        assert_eq!(bytes(Control::lane(CcMode::Bits7, 0, 74, 0, 0.5).burst()), [vec![0xB0, 74, 64]]);
        // NRPN 3:5 (389) at 1000: which, then the value
        let nrpn = Control::Nrpn { channel: 2, parameter: 389, value: 1000 };
        assert_eq!(bytes(nrpn.burst()), [vec![0xB2, 99, 3], vec![0xB2, 98, 5], vec![0xB2, 6, 7], vec![0xB2, 38, 104]]);
        // RPN 0, the bend range: 2 semitones, then the Null RPN
        let range = Control::Rpn { channel: 0, parameter: RPN_BEND_RANGE, value: 2 << 7 };
        assert_eq!(
            bytes(range.burst()),
            [vec![0xB0, 101, 0], vec![0xB0, 100, 0], vec![0xB0, 6, 2], vec![0xB0, 38, 0], vec![0xB0, 101, 127], vec![0xB0, 100, 127]]
        );
        assert_eq!(range.burst().bytes(), 18);
        // Keys: the controller, not its value
        assert_eq!(Control::lane(CcMode::Bits7, 0, 74, 0, 0.1).key(), Control::lane(CcMode::Bits7, 0, 74, 0, 0.9).key());
        assert_ne!(Control::lane(CcMode::Bits7, 0, 74, 0, 0.1).key(), Control::lane(CcMode::Bits7, 1, 74, 0, 0.1).key());
        assert_ne!(Control::PolyPressure { channel: 0, note: 60, value: 1 }.key(), Control::PolyPressure { channel: 0, note: 61, value: 1 }.key());
        assert_eq!(Message::program(4, 2).channel(), Some(4));
        assert_eq!(Message::real_time(status::CLOCK).channel(), None);
    }

    #[test]
    fn budgets_are_kept_a_line_a_port() {
        let bus = MidiBus::new();
        assert_eq!(bus.budgets(), [0; PORTS], "none: USB takes it all");
        bus.set_budget(2, DIN_RATE);
        bus.set_budget(7, u32::MAX);
        assert_eq!(bus.budget(7), MAX_BUDGET);
        let other = MidiBus::new();
        other.restore_budget(&bus.budget_setting());
        assert_eq!(other.budgets(), bus.budgets());
        other.restore_budget("fast\n500");
        assert_eq!(other.budget(0), 0);
        assert_eq!(other.budget(1), 500);
    }

    #[test]
    fn the_ui_sends_whole_sysex_once_a_sender_drains() {
        let bus = MidiBus::new();
        assert_eq!(bus.send_sysex(0, &[0xF0, 1, 0xF7]), Err(SysexError::NoSender), "no sender, nothing to send it");
        assert!(bus.sysex_pool().is_none());
        bus.start_draining();
        assert!(bus.sysex_pool().is_some(), "the pool is made as the sender starts");
        assert_eq!(bus.send_sysex(0, &[0x90, 60, 100]), Err(SysexError::NotSysex));
        bus.send_sysex(2, &[0xF0, 0x43, 0x00, 0xF7]).unwrap();
        assert_eq!(bus.send_sysex_file(9, &[0xF0, 1, 0xF7, 0xF0, 2, 3, 0xF7]), Ok((2, 7)));
        let taken = bus.take_sysex();
        assert_eq!(taken.iter().map(|(port, bytes)| (*port, bytes.to_vec())).collect::<Vec<_>>(), [
            (2, vec![0xF0, 0x43, 0x00, 0xF7]),
            (7, vec![0xF0, 1, 0xF7]),
            (7, vec![0xF0, 2, 3, 0xF7]),
        ]);
        assert!(bus.take_sysex().is_empty(), "taken once");
    }
}
