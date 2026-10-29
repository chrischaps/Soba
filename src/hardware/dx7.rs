//! A DX7 voice (#448): the six operators, their envelopes and ratios, the
//! algorithm that wires them, as the volca fm takes it by SysEx.
//!
//! A voice is the DX7's 155 bytes, in the order Yamaha's data sheet lists
//! them: operator 6's 21 bytes first, down to operator 1's, then the pitch
//! envelope, the algorithm, the LFO, the transpose and a ten-letter name.
//! A bank packs 32 voices into 128 bytes each. The layouts follow the
//! "DX7 Data Format" sheet (the DX7 MIDI Data Format Sheet as Ewan
//! Macpherson compiled it, 1993), which Dexed keeps as
//! `doc/sysex-format.txt`. The algorithms are the table in
//! music-synthesizer-for-android's `fm_core.cc`, which Dexed plays.
//!
//! **The volca fm differs in one byte.** Where a DX7 single voice ends
//! with its checksum, the volca reads which operators are on (bit 0
//! operator 1, up to bit 5 operator 6): a true checksum mutes some of
//! them. So a voice sent to a volca ends with `0x3F`, all six on, and one
//! sent to a DX7 (or Dexed, or a volca fm2) with the checksum. Banks carry
//! a checksum on both. That's from Volcako, an editor written against the
//! volca fm (github.com/Luomu/volcako, `source/sysex.cpp`), whose "DX7
//! compatibility mode" sends the checksum and "will sound wrong on Volca
//! FM". Korg's chart says only that the volca takes DX7 bulk data, and
//! Volcako finds it takes it on any channel.

use crate::link::sysex::{self, SysexError};

/// The bytes of one voice.
pub const VOICE_BYTES: usize = 155;
/// The bytes of one voice packed into a bank.
pub const PACKED_BYTES: usize = 128;
/// The voices in a bank.
pub const BANK_VOICES: usize = 32;
/// The operators in a voice.
pub const OPERATORS: usize = 6;
/// The bytes of one operator.
const OP_BYTES: usize = 21;

/// A single voice dump: F0, Yamaha, channel, format 0, 155 bytes, the
/// checksum, F7.
pub const SINGLE_BYTES: usize = 6 + VOICE_BYTES + 2;
/// A 32-voice bank: F0, Yamaha, channel, format 9, 4,096 bytes, the
/// checksum, F7.
pub const BANK_BYTES: usize = 6 + BANK_VOICES * PACKED_BYTES + 2;

const YAMAHA: u8 = 0x43;

/// What each operator's bytes are, from its first (see [`Voice::op_at`]).
pub mod op {
    /// The envelope's four rates, R1 to R4: 0 to 99, faster as they rise.
    pub const RATES: usize = 0;
    /// Its four levels, L1 to L4: 0 to 99.
    pub const LEVELS: usize = 4;
    /// Keyboard level scaling: the break point (39 is C3), how deep each side
    /// goes, and each side's curve (0 -LIN, 1 -EXP, 2 +EXP, 3 +LIN).
    pub const BREAK_POINT: usize = 8;
    pub const LEFT_DEPTH: usize = 9;
    pub const RIGHT_DEPTH: usize = 10;
    pub const LEFT_CURVE: usize = 11;
    pub const RIGHT_CURVE: usize = 12;
    /// How much faster its envelope runs up the keyboard: 0 to 7.
    pub const RATE_SCALING: usize = 13;
    /// How far the LFO moves its level: 0 to 3.
    pub const AMP_MOD: usize = 14;
    /// How far velocity moves its level: 0 to 7.
    pub const VELOCITY: usize = 15;
    /// Its output level: 0 to 99. A carrier's is how loud it is, a
    /// modulator's how bright it makes what it modulates.
    pub const LEVEL: usize = 16;
    /// 0 for a ratio of the note, 1 for a fixed frequency.
    pub const MODE: usize = 17;
    /// The ratio's whole part (0 is a half), 0 to 31, and its fine part, 0
    /// to 99: the ratio is coarse × (1 + fine / 100).
    pub const COARSE: usize = 18;
    pub const FINE: usize = 19;
    /// 0 to 14, 7 in tune.
    pub const DETUNE: usize = 20;
}

/// The voice's bytes after its operators.
pub mod global {
    /// The pitch envelope's rates and levels (50 is the note itself).
    pub const PITCH_RATES: usize = 126;
    pub const PITCH_LEVELS: usize = 130;
    /// 0 to 31, the algorithm numbered from 1 less one.
    pub const ALGORITHM: usize = 134;
    /// 0 to 7.
    pub const FEEDBACK: usize = 135;
    pub const OSC_SYNC: usize = 136;
    pub const LFO_SPEED: usize = 137;
    pub const LFO_DELAY: usize = 138;
    pub const LFO_PITCH_DEPTH: usize = 139;
    pub const LFO_AMP_DEPTH: usize = 140;
    pub const LFO_SYNC: usize = 141;
    /// 0 triangle, 1 saw down, 2 saw up, 3 square, 4 sine, 5 sample and hold.
    pub const LFO_WAVE: usize = 142;
    pub const PITCH_MOD: usize = 143;
    /// 0 to 48, 24 as played.
    pub const TRANSPOSE: usize = 144;
    /// Ten ASCII letters.
    pub const NAME: usize = 145;
    pub const NAME_LETTERS: usize = 10;
}

/// The highest value each of an operator's bytes takes.
const OP_MAX: [u8; OP_BYTES] = [99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 3, 3, 7, 3, 7, 99, 1, 31, 99, 14];
/// The highest value each byte after the operators takes, to the name.
const GLOBAL_MAX: [u8; 19] = [99, 99, 99, 99, 99, 99, 99, 99, 31, 7, 1, 99, 99, 99, 99, 1, 5, 7, 48];

/// The highest value byte `at` of a voice takes.
pub fn max(at: usize) -> u8 {
    match at {
        _ if at < OPERATORS * OP_BYTES => OP_MAX[at % OP_BYTES],
        _ if at < global::NAME => GLOBAL_MAX[at - OPERATORS * OP_BYTES],
        // A letter: printable ASCII
        _ => 126,
    }
}

/// The lowest: a letter's is a space.
pub fn min(at: usize) -> u8 {
    if at >= global::NAME {
        b' '
    } else {
        0
    }
}

/// One DX7 voice, its 155 bytes as the data sheet orders them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Voice(pub [u8; VOICE_BYTES]);

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Voice({:?}, algorithm {})", self.name(), self.algorithm() + 1)
    }
}

impl Default for Voice {
    fn default() -> Self {
        Self::init()
    }
}

impl Voice {
    /// The DX7's INIT VOICE: operator 1 alone, a plain sine at the note.
    pub fn init() -> Self {
        let mut bytes = [0; VOICE_BYTES];
        for n in 1..=OPERATORS {
            let at = Self::op_at(n);
            bytes[at..at + 4].copy_from_slice(&[99; 4]);
            bytes[at + op::LEVELS..at + op::LEVELS + 4].copy_from_slice(&[99, 99, 99, 0]);
            bytes[at + op::BREAK_POINT] = 39;
            bytes[at + op::LEVEL] = if n == 1 { 99 } else { 0 };
            bytes[at + op::COARSE] = 1;
            bytes[at + op::DETUNE] = 7;
        }
        bytes[global::PITCH_RATES..global::PITCH_RATES + 4].copy_from_slice(&[99; 4]);
        bytes[global::PITCH_LEVELS..global::PITCH_LEVELS + 4].copy_from_slice(&[50; 4]);
        bytes[global::OSC_SYNC] = 1;
        bytes[global::LFO_SPEED] = 35;
        bytes[global::LFO_SYNC] = 1;
        bytes[global::PITCH_MOD] = 3;
        bytes[global::TRANSPOSE] = 24;
        let mut voice = Self(bytes);
        voice.set_name("INIT VOICE");
        voice
    }

    /// Where operator `n`'s bytes start (1 to 6): operator 6 comes first.
    pub const fn op_at(n: usize) -> usize {
        (OPERATORS - n) * OP_BYTES
    }

    /// Operator `n`'s byte `field` (one of [`op`]'s).
    pub fn op(&self, n: usize, field: usize) -> u8 {
        self.0[Self::op_at(n) + field]
    }

    pub fn set_op(&mut self, n: usize, field: usize, value: u8) {
        self.0[Self::op_at(n) + field] = value;
    }

    /// The algorithm, from 0.
    pub fn algorithm(&self) -> usize {
        (self.0[global::ALGORITHM] as usize).min(ALGORITHMS - 1)
    }

    pub fn feedback(&self) -> u8 {
        self.0[global::FEEDBACK]
    }

    /// The name, its trailing spaces gone, anything unprintable a space.
    pub fn name(&self) -> String {
        let letters = &self.0[global::NAME..global::NAME + global::NAME_LETTERS];
        let name: String = letters.iter().map(|&b| if (32..127).contains(&b) { b as char } else { ' ' }).collect();
        name.trim_end().to_string()
    }

    /// Names it: ten letters at most, padded with spaces, in printable ASCII.
    pub fn set_name(&mut self, name: &str) {
        let mut letters = name.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).map(|c| c as u8);
        for at in global::NAME..global::NAME + global::NAME_LETTERS {
            self.0[at] = letters.next().unwrap_or(b' ');
        }
    }

    /// Operator `n`'s frequency: a ratio of the note, or Hz when fixed.
    pub fn frequency(&self, n: usize) -> Frequency {
        let (coarse, fine) = (self.op(n, op::COARSE) as f32, self.op(n, op::FINE) as f32);
        if self.op(n, op::MODE) == 1 {
            // Fixed: 1, 10, 100 or 1000 Hz from the coarse's last two bits,
            // times 10^(fine / 100)
            let decade = 10f32.powi((self.op(n, op::COARSE) & 3) as i32);
            Frequency::Fixed(decade * 10f32.powf(fine / 100.0))
        } else {
            let whole = if coarse == 0.0 { 0.5 } else { coarse };
            Frequency::Ratio(whole * (1.0 + fine / 100.0))
        }
    }

    /// Whether every byte is in its range.
    pub fn in_range(&self) -> bool {
        self.0.iter().enumerate().all(|(at, &b)| (min(at)..=max(at)).contains(&b))
    }

    /// The voice with every byte brought into its range.
    pub fn clamped(mut self) -> Self {
        for (at, b) in self.0.iter_mut().enumerate() {
            *b = (*b).clamp(min(at), max(at));
        }
        self
    }

    /// The operators that sound (1 to 6), as the algorithm has them.
    pub fn carriers(&self) -> [bool; OPERATORS] {
        wiring(self.algorithm()).carriers
    }

    /// As a single voice dump on `channel` (0 to 15), for `target`.
    pub fn single(&self, channel: u8, target: Target) -> Vec<u8> {
        let mut message = Vec::with_capacity(SINGLE_BYTES);
        message.extend_from_slice(&[sysex::START, YAMAHA, channel & 0x0F, 0x00, 0x01, 0x1B]);
        message.extend(self.0.iter().map(|b| b & 0x7F));
        message.push(match target {
            Target::Dx7 => checksum(&self.0),
            Target::Volca => 0x3F,
        });
        message.push(sysex::END);
        message
    }

    /// Packed into a bank's 128 bytes.
    pub fn pack(&self) -> [u8; PACKED_BYTES] {
        let v = &self.0;
        let mut p = [0; PACKED_BYTES];
        for k in 0..OPERATORS {
            let (from, to) = (k * OP_BYTES, k * 17);
            let o = &v[from..from + OP_BYTES];
            p[to..to + 11].copy_from_slice(&o[..11]);
            p[to + 11] = (o[op::RIGHT_CURVE] & 3) << 2 | (o[op::LEFT_CURVE] & 3);
            p[to + 12] = (o[op::DETUNE] & 15) << 3 | (o[op::RATE_SCALING] & 7);
            p[to + 13] = (o[op::VELOCITY] & 7) << 2 | (o[op::AMP_MOD] & 3);
            p[to + 14] = o[op::LEVEL] & 0x7F;
            p[to + 15] = (o[op::COARSE] & 31) << 1 | (o[op::MODE] & 1);
            p[to + 16] = o[op::FINE] & 0x7F;
        }
        p[102..110].copy_from_slice(&v[global::PITCH_RATES..global::PITCH_RATES + 8]);
        p[110] = v[global::ALGORITHM] & 31;
        p[111] = (v[global::OSC_SYNC] & 1) << 3 | (v[global::FEEDBACK] & 7);
        p[112..116].copy_from_slice(&v[global::LFO_SPEED..global::LFO_SPEED + 4]);
        p[116] = (v[global::PITCH_MOD] & 7) << 4 | (v[global::LFO_WAVE] & 7) << 1 | (v[global::LFO_SYNC] & 1);
        p[117] = v[global::TRANSPOSE] & 0x7F;
        p[118..128].copy_from_slice(&v[global::NAME..global::NAME + 10]);
        for b in &mut p {
            *b &= 0x7F;
        }
        p
    }

    /// A voice from a bank's 128 bytes.
    pub fn unpack(p: &[u8; PACKED_BYTES]) -> Self {
        let mut v = [0; VOICE_BYTES];
        for k in 0..OPERATORS {
            let (to, from) = (k * OP_BYTES, k * 17);
            let o = &mut v[to..to + OP_BYTES];
            o[..11].copy_from_slice(&p[from..from + 11]);
            o[op::LEFT_CURVE] = p[from + 11] & 3;
            o[op::RIGHT_CURVE] = (p[from + 11] >> 2) & 3;
            o[op::RATE_SCALING] = p[from + 12] & 7;
            o[op::DETUNE] = (p[from + 12] >> 3) & 15;
            o[op::AMP_MOD] = p[from + 13] & 3;
            o[op::VELOCITY] = (p[from + 13] >> 2) & 7;
            o[op::LEVEL] = p[from + 14];
            o[op::MODE] = p[from + 15] & 1;
            o[op::COARSE] = (p[from + 15] >> 1) & 31;
            o[op::FINE] = p[from + 16];
        }
        v[global::PITCH_RATES..global::PITCH_RATES + 8].copy_from_slice(&p[102..110]);
        v[global::ALGORITHM] = p[110] & 31;
        v[global::FEEDBACK] = p[111] & 7;
        v[global::OSC_SYNC] = (p[111] >> 3) & 1;
        v[global::LFO_SPEED..global::LFO_SPEED + 4].copy_from_slice(&p[112..116]);
        v[global::LFO_SYNC] = p[116] & 1;
        v[global::LFO_WAVE] = (p[116] >> 1) & 7;
        v[global::PITCH_MOD] = (p[116] >> 4) & 7;
        v[global::TRANSPOSE] = p[117];
        v[global::NAME..global::NAME + 10].copy_from_slice(&p[118..128]);
        Self(v)
    }

    /// The voice as hex, two letters a byte: how a node keeps it.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != VOICE_BYTES * 2 || !text.is_ascii() {
            return None;
        }
        let mut bytes = [0; VOICE_BYTES];
        for (k, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&text[k * 2..k * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }
}

/// Where a voice goes, for the one byte they read differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A volca fm: the operators on, all six.
    Volca,
    /// A DX7, Dexed or anything else that reads the data sheet: the checksum.
    Dx7,
}

/// An operator's frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frequency {
    /// Times the note.
    Ratio(f32),
    /// Hz, whatever the note.
    Fixed(f32),
}

impl Frequency {
    /// "×1.00", "×3.50", "440 Hz".
    pub fn label(&self) -> String {
        match *self {
            Self::Ratio(r) => format!("×{r:.2}"),
            Self::Fixed(hz) if hz >= 1000.0 => format!("{:.1}k", hz / 1000.0),
            Self::Fixed(hz) if hz >= 100.0 => format!("{hz:.0}Hz"),
            Self::Fixed(hz) => format!("{hz:.1}Hz"),
        }
    }
}

/// The masked two's complement of the sum of `data`: what follows a dump.
pub fn checksum(data: &[u8]) -> u8 {
    let sum = data.iter().fold(0u8, |sum, &b| sum.wrapping_add(b & 0x7F));
    sum.wrapping_neg() & 0x7F
}

/// A bank of `voices` on `channel` (0 to 15): up to 32, and any left over
/// INIT VOICE, as a DX7 fills an empty bank.
pub fn bank(voices: &[Voice], channel: u8) -> Vec<u8> {
    let mut data = Vec::with_capacity(BANK_VOICES * PACKED_BYTES);
    for k in 0..BANK_VOICES {
        data.extend_from_slice(&voices.get(k).copied().unwrap_or_else(Voice::init).pack());
    }
    let mut message = Vec::with_capacity(BANK_BYTES);
    message.extend_from_slice(&[sysex::START, YAMAHA, channel & 0x0F, 0x09, 0x20, 0x00]);
    message.extend_from_slice(&data);
    message.push(checksum(&data));
    message.push(sysex::END);
    message
}

/// Why a file gave no voices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// It isn't SysEx at all.
    NotSysex,
    /// It's SysEx, but no message in it is a DX7 voice or bank.
    NoVoices,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotSysex => "it isn't a SysEx file",
            Self::NoVoices => "it holds no DX7 voice or bank",
        })
    }
}

impl From<SysexError> for ReadError {
    fn from(_: SysexError) -> Self {
        Self::NotSysex
    }
}

/// Every voice in a `.syx` file, in order: a single voice, a bank of 32,
/// or several of either. Other messages are passed over, and a checksum
/// that doesn't add up is forgiven, as a volca's voice files don't.
pub fn read(file: &[u8]) -> Result<Vec<Voice>, ReadError> {
    let mut voices = Vec::new();
    for message in sysex::split(file)? {
        match message {
            [sysex::START, YAMAHA, channel, 0x00, 0x01, 0x1B, data @ .., _, sysex::END] if channel & 0xF0 == 0 && data.len() == VOICE_BYTES => {
                let mut bytes = [0; VOICE_BYTES];
                bytes.copy_from_slice(data);
                voices.push(Voice(bytes));
            }
            [sysex::START, YAMAHA, channel, 0x09, 0x20, 0x00, data @ .., _, sysex::END] if channel & 0xF0 == 0 && data.len() == BANK_VOICES * PACKED_BYTES => {
                voices.extend(data.as_chunks::<PACKED_BYTES>().0.iter().map(Voice::unpack));
            }
            _ => {}
        }
    }
    if voices.is_empty() {
        Err(ReadError::NoVoices)
    } else {
        Ok(voices)
    }
}

/// How many algorithms there are.
pub const ALGORITHMS: usize = 32;

// Each operator's buses, from music-synthesizer-for-android's fm_core.cc
// (FmCore::algorithms), operator 6 first, as it computes them
pub const OUT_BUS_ONE: u8 = 1 << 0;
pub const OUT_BUS_TWO: u8 = 1 << 1;
pub const OUT_BUS_ADD: u8 = 1 << 2;
pub const IN_BUS_ONE: u8 = 1 << 4;
pub const IN_BUS_TWO: u8 = 1 << 5;
pub const FB_IN: u8 = 1 << 6;
pub const FB_OUT: u8 = 1 << 7;

#[rustfmt::skip]
const BUSES: [[u8; OPERATORS]; ALGORITHMS] = [
    [0xc1, 0x11, 0x11, 0x14, 0x01, 0x14], // 1
    [0x01, 0x11, 0x11, 0x14, 0xc1, 0x14], // 2
    [0xc1, 0x11, 0x14, 0x01, 0x11, 0x14], // 3
    [0xc1, 0x11, 0x94, 0x01, 0x11, 0x14], // 4
    [0xc1, 0x14, 0x01, 0x14, 0x01, 0x14], // 5
    [0xc1, 0x94, 0x01, 0x14, 0x01, 0x14], // 6
    [0xc1, 0x11, 0x05, 0x14, 0x01, 0x14], // 7
    [0x01, 0x11, 0xc5, 0x14, 0x01, 0x14], // 8
    [0x01, 0x11, 0x05, 0x14, 0xc1, 0x14], // 9
    [0x01, 0x05, 0x14, 0xc1, 0x11, 0x14], // 10
    [0xc1, 0x05, 0x14, 0x01, 0x11, 0x14], // 11
    [0x01, 0x05, 0x05, 0x14, 0xc1, 0x14], // 12
    [0xc1, 0x05, 0x05, 0x14, 0x01, 0x14], // 13
    [0xc1, 0x05, 0x11, 0x14, 0x01, 0x14], // 14
    [0x01, 0x05, 0x11, 0x14, 0xc1, 0x14], // 15
    [0xc1, 0x11, 0x02, 0x25, 0x05, 0x14], // 16
    [0x01, 0x11, 0x02, 0x25, 0xc5, 0x14], // 17
    [0x01, 0x11, 0x11, 0xc5, 0x05, 0x14], // 18
    [0xc1, 0x14, 0x14, 0x01, 0x11, 0x14], // 19
    [0x01, 0x05, 0x14, 0xc1, 0x14, 0x14], // 20
    [0x01, 0x14, 0x14, 0xc1, 0x14, 0x14], // 21
    [0xc1, 0x14, 0x14, 0x14, 0x01, 0x14], // 22
    [0xc1, 0x14, 0x14, 0x01, 0x14, 0x04], // 23
    [0xc1, 0x14, 0x14, 0x14, 0x04, 0x04], // 24
    [0xc1, 0x14, 0x14, 0x04, 0x04, 0x04], // 25
    [0xc1, 0x05, 0x14, 0x01, 0x14, 0x04], // 26
    [0x01, 0x05, 0x14, 0xc1, 0x14, 0x04], // 27
    [0x04, 0xc1, 0x11, 0x14, 0x01, 0x14], // 28
    [0xc1, 0x14, 0x01, 0x14, 0x04, 0x04], // 29
    [0x04, 0xc1, 0x11, 0x14, 0x04, 0x04], // 30
    [0xc1, 0x14, 0x04, 0x04, 0x04, 0x04], // 31
    [0xc4, 0x04, 0x04, 0x04, 0x04, 0x04], // 32
];

/// The buses algorithm `algorithm` (from 0) has each operator read and
/// write, operator 6 first, in the order a player computes them (#472):
/// the flags above.
pub fn buses(algorithm: usize) -> [u8; OPERATORS] {
    BUSES[algorithm.min(ALGORITHMS - 1)]
}

/// How an algorithm wires the operators, numbered 1 to 6 as the DX7 does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wiring {
    /// Which operators sound, by index (operator 1 at 0).
    pub carriers: [bool; OPERATORS],
    /// Each operator modulating another: (from, to).
    pub modulations: Vec<(usize, usize)>,
    /// The feedback loop: from the operator whose output feeds back, to the
    /// one it feeds (the same, for most).
    pub feedback: (usize, usize),
}

/// How algorithm `algorithm` (from 0) wires the operators, worked out from
/// the buses each operator reads and writes, in the order they're computed.
pub fn wiring(algorithm: usize) -> Wiring {
    let buses = buses(algorithm);
    let mut carriers = [false; OPERATORS];
    let mut modulations = Vec::new();
    let mut on_bus: [Vec<usize>; 3] = Default::default();
    let (mut into, mut out) = (OPERATORS, OPERATORS);
    for (k, &flags) in buses.iter().enumerate() {
        let n = OPERATORS - k;
        let read = match flags & (IN_BUS_ONE | IN_BUS_TWO) {
            IN_BUS_ONE => Some(1),
            IN_BUS_TWO => Some(2),
            _ => None,
        };
        if let Some(bus) = read {
            modulations.extend(on_bus[bus].iter().map(|&from| (from, n)));
        }
        let write = match flags & (OUT_BUS_ONE | OUT_BUS_TWO) {
            OUT_BUS_ONE => 1,
            OUT_BUS_TWO => 2,
            _ => 0,
        };
        if write == 0 {
            carriers[n - 1] = true;
        } else if flags & OUT_BUS_ADD != 0 {
            on_bus[write].push(n);
        } else {
            on_bus[write] = vec![n];
        }
        if flags & FB_IN != 0 {
            into = n;
        }
        if flags & FB_OUT != 0 {
            out = n;
        }
    }
    Wiring { carriers, modulations, feedback: (out, into) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice with every byte different where its range allows.
    fn busy() -> Voice {
        let mut voice = Voice::init();
        for at in 0..global::NAME {
            voice.0[at] = ((at * 7 + 3) % (max(at) as usize + 1)) as u8;
        }
        voice.set_name("BUSY  VOX");
        voice
    }

    #[test]
    fn init_voice_is_operator_one_alone() {
        let init = Voice::init();
        assert!(init.in_range());
        assert_eq!(init.name(), "INIT VOICE");
        assert_eq!(init.algorithm(), 0);
        assert_eq!(init.op(1, op::LEVEL), 99);
        assert!((2..=6).all(|n| init.op(n, op::LEVEL) == 0));
        assert_eq!(init.frequency(1), Frequency::Ratio(1.0));
    }

    #[test]
    fn a_single_voice_round_trips_byte_for_byte() {
        let voice = busy();
        let file = voice.single(0, Target::Dx7);
        assert_eq!(file.len(), SINGLE_BYTES);
        assert_eq!(&file[..6], &[0xF0, 0x43, 0x00, 0x00, 0x01, 0x1B]);
        let sum: u32 = file[6..6 + VOICE_BYTES].iter().map(|&b| b as u32).sum::<u32>() + file[6 + VOICE_BYTES] as u32;
        assert_eq!(sum % 128, 0, "the checksum makes the data add up to a multiple of 128");
        let read = read(&file).unwrap();
        assert_eq!(read, vec![voice]);
        assert_eq!(read[0].single(0, Target::Dx7), file, "written again, the same bytes");
    }

    #[test]
    fn a_volca_voice_ends_with_its_operators_on() {
        let file = busy().single(4, Target::Volca);
        assert_eq!(file[2], 4, "the channel");
        assert_eq!(file[SINGLE_BYTES - 2], 0x3F, "all six on, in place of the checksum");
        assert_eq!(read(&file).unwrap(), vec![busy()], "and it's read back all the same");
    }

    #[test]
    fn a_bank_packs_and_unpacks_every_field() {
        let voices: Vec<Voice> = (0..BANK_VOICES as u8)
            .map(|k| {
                let mut voice = Voice::init();
                for at in 0..global::NAME {
                    voice.0[at] = ((at * (k as usize + 1) + k as usize) % (max(at) as usize + 1)) as u8;
                }
                voice.set_name(&format!("VOICE {k}"));
                voice
            })
            .collect();
        for voice in &voices {
            assert_eq!(Voice::unpack(&voice.pack()), *voice);
        }
        let file = bank(&voices, 0);
        assert_eq!(file.len(), BANK_BYTES);
        assert_eq!(read(&file).unwrap(), voices);
        assert_eq!(bank(&read(&file).unwrap(), 0), file, "a bank round-trips byte for byte");
    }

    #[test]
    fn a_short_bank_is_filled_with_init_voices() {
        let file = bank(&[busy()], 2);
        let voices = read(&file).unwrap();
        assert_eq!(voices.len(), BANK_VOICES);
        assert_eq!(voices[0], busy());
        assert!(voices[1..].iter().all(|v| *v == Voice::init()));
    }

    #[test]
    fn files_that_hold_no_voice() {
        assert_eq!(read(b"not sysex"), Err(ReadError::NotSysex));
        assert_eq!(read(&[0xF0, 0x41, 0x10, 0x42, 0xF7]), Err(ReadError::NoVoices), "a Roland message");
        let mut short = busy().single(0, Target::Dx7);
        short.remove(10);
        assert_eq!(read(&short), Err(ReadError::NoVoices), "a byte short");
    }

    #[test]
    fn several_messages_in_one_file() {
        let mut file = busy().single(0, Target::Dx7);
        file.extend(Voice::init().single(0, Target::Volca));
        assert_eq!(read(&file).unwrap(), vec![busy(), Voice::init()]);
    }

    #[test]
    fn hex_round_trips() {
        let voice = busy();
        assert_eq!(Voice::from_hex(&voice.to_hex()), Some(voice));
        assert_eq!(Voice::from_hex("00ff"), None);
    }

    #[test]
    fn clamping_brings_every_byte_into_range() {
        let wild = Voice([127; VOICE_BYTES]);
        assert!(!wild.in_range());
        let tame = wild.clamped();
        assert!(tame.in_range());
        assert_eq!(tame.algorithm(), 31);
        assert_eq!(tame.op(1, op::DETUNE), 14);
        assert_eq!(tame.name(), "~~~~~~~~~~");
    }

    #[test]
    fn names_are_ten_printable_letters() {
        let mut voice = Voice::init();
        voice.set_name("A VERY LONG NAME");
        assert_eq!(voice.name(), "A VERY LON");
        voice.set_name("bell\u{7}é");
        assert_eq!(voice.name(), "bell");
    }

    #[test]
    fn frequencies_read_as_the_dx7_plays_them() {
        let mut voice = Voice::init();
        voice.set_op(2, op::COARSE, 0);
        assert_eq!(voice.frequency(2), Frequency::Ratio(0.5), "coarse 0 is a half");
        voice.set_op(2, op::COARSE, 2);
        voice.set_op(2, op::FINE, 75);
        assert_eq!(voice.frequency(2).label(), "×3.50");
        voice.set_op(2, op::MODE, 1);
        voice.set_op(2, op::COARSE, 2);
        voice.set_op(2, op::FINE, 0);
        assert_eq!(voice.frequency(2), Frequency::Fixed(100.0));
    }

    #[test]
    fn algorithms_wire_as_the_dx7_draws_them() {
        // 1: 2 into 1, and 6 into 5 into 4 into 3, 6 feeding itself
        let one = wiring(0);
        assert_eq!(one.carriers, [true, false, true, false, false, false]);
        let mut m = one.modulations.clone();
        m.sort_unstable();
        assert_eq!(m, vec![(2, 1), (4, 3), (5, 4), (6, 5)]);
        assert_eq!(one.feedback, (6, 6));
        // 4: the loop runs from 4 back to 6
        assert_eq!(wiring(3).feedback, (4, 6));
        // 7: 5 and 4 both into 3
        assert!(wiring(6).modulations.contains(&(5, 3)) && wiring(6).modulations.contains(&(4, 3)));
        // 16: 2, 3 and 5 into 1, the only carrier
        let sixteen = wiring(15);
        assert_eq!(sixteen.carriers, [true, false, false, false, false, false]);
        assert!([(2, 1), (3, 1), (5, 1), (4, 3), (6, 5)].iter().all(|e| sixteen.modulations.contains(e)));
        // 22: 6 into 5, 4 and 3
        assert!([(6, 3), (6, 4), (6, 5), (2, 1)].iter().all(|e| wiring(21).modulations.contains(e)));
        // 32: all six sound, 6 feeding itself
        assert_eq!(wiring(31).carriers, [true; 6]);
        assert!(wiring(31).modulations.is_empty());
        assert_eq!(wiring(31).feedback, (6, 6));
        // Every algorithm: every modulator reaches a carrier, and how many
        // sound matches the DX7's panel
        let counts: Vec<usize> = (0..ALGORITHMS).map(|a| wiring(a).carriers.iter().filter(|&&c| c).count()).collect();
        assert_eq!(counts, vec![2, 2, 2, 2, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 3, 3, 4, 4, 4, 5, 5, 3, 3, 3, 4, 4, 5, 6]);
        for a in 0..ALGORITHMS {
            let w = wiring(a);
            for n in 1..=OPERATORS {
                assert!(w.carriers[n - 1] || w.modulations.iter().any(|&(from, _)| from == n), "algorithm {}: operator {n} goes nowhere", a + 1);
            }
        }
    }
}
