//! A profile: one piece of hardware in its own words (#441). What it's
//! called, which CC is its Cutoff and what the values of its Oscillator
//! Type mean, which note is its kick, how its programs are named, and
//! whether it keeps time. The Instrument module and the Setlist read it;
//! nothing here sends anything.
//!
//! A profile is a JSON file, like a patch:
//!
//! ```json
//! { "name": "MicroFreak", "maker": "Arturia", "kind": "synth", "channels": [1],
//!   "voices": { "paraphonic": 4 },
//!   "parameters": [{ "name": "Cutoff", "cc": 23, "group": "Filter" }] }
//! ```
//!
//! A parameter is a CC (`"cc": 74`), a 14-bit pair (`"cc": 1, "lsb": 33`)
//! or an NRPN (`"nrpn": 1234`), across its whole range unless it names
//! one. A stepped one lists its values: plain labels share the range
//! evenly, and labels with a `value` say exactly what to send. One on its
//! own channel says so (`"channel": 9`, the Digitakt's delay), and a CC
//! that nudges rather than sets is `"relative": true`, its middle meaning
//! no change (the FANTOM's Cutoff).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// The highest value a 7-bit message carries.
pub const MAX_7: u16 = 127;
/// The highest value a 14-bit pair or an NRPN carries.
pub const MAX_14: u16 = 16383;

/// A piece of hardware, as Soba knows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// As its maker writes it: "Digitakt", "SP-404MKII", "volca fm2".
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub maker: String,
    /// What it is, in a line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// The maker's chart it was built from, and its version.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// A piece of the name its MIDI port shows up as, to suggest its port.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub port: String,
    #[serde(default)]
    pub kind: Kind,
    /// The channels it listens on from the factory, 1-16.
    #[serde(default = "first_channel")]
    pub channels: Vec<u8>,
    /// How many notes it sounds at once, when its maker says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voices: Option<Voices>,
    /// In the order an Instrument's knobs take them: its first twelve by
    /// default.
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    /// CCs a gate presses or flips: a looper's record, a mute.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<Button>,
    /// A kit's drums, pads or tracks, or a multitimbral synth's parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
    /// Each part takes the same CCs on its own channel (a track a channel).
    #[serde(default, skip_serializing_if = "is_false")]
    pub cc_per_part: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub programs: Option<Programs>,
    #[serde(default)]
    pub clock: Clock,
    /// What to set on the device for the profile to work.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

fn first_channel() -> Vec<u8> {
    vec![1]
}

fn is_false(b: &bool) -> bool {
    !b
}

/// What it plays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// One instrument, played with notes.
    #[default]
    Synth,
    /// A drum machine, sampler or groovebox: its parts each have a note,
    /// a channel, or both.
    Kit,
}

/// How many notes it sounds at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Voices {
    Mono,
    Poly(u8),
    /// Several oscillators through one filter and envelope.
    Paraphonic(u8),
}

impl Voices {
    /// How many notes at once.
    pub fn count(self) -> u8 {
        match self {
            Voices::Mono => 1,
            Voices::Poly(n) | Voices::Paraphonic(n) => n,
        }
    }
}

/// Where a parameter's value goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address {
    Cc(u8),
    /// The value's top seven bits on `msb`, its bottom seven on `lsb`.
    Cc14 { msb: u8, lsb: u8 },
    /// A parameter number, 0-16383, set through CCs 99 and 98.
    Nrpn(u16),
}

impl Address {
    /// The highest value it carries.
    pub fn max(self) -> u16 {
        match self {
            Address::Cc(_) => MAX_7,
            Address::Cc14 { .. } | Address::Nrpn(_) => MAX_14,
        }
    }

    /// As a chart writes it: "CC 74", "CC 1/33", "NRPN 1234".
    pub fn text(self) -> String {
        match self {
            Address::Cc(cc) => format!("CC {cc}"),
            Address::Cc14 { msb, lsb } => format!("CC {msb}/{lsb}"),
            Address::Nrpn(n) => format!("NRPN {n}"),
        }
    }
}

/// How a knob's turn spreads over the values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Curve {
    #[default]
    Linear,
    /// Fine at the bottom, as a time or a frequency wants.
    Log,
}

/// One value of a stepped parameter: a label sharing the range evenly
/// with the others, or a label and exactly what to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Label(String),
    At { label: String, value: u16 },
}

impl Step {
    pub fn label(&self) -> &str {
        match self {
            Step::Label(label) | Step::At { label, .. } => label,
        }
    }
}

/// One thing the device lets a message set: its cutoff, its oscillator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawParameter", into = "RawParameter")]
pub struct Parameter {
    pub name: String,
    pub address: Address,
    /// Oscillator, Filter, Envelope, FX… which colours it.
    pub group: String,
    /// The lowest and highest values it takes.
    pub range: (u16, u16),
    pub curve: Curve,
    /// Its values' labels, for a stepped one.
    pub values: Vec<Step>,
    /// What it starts at, when the chart says.
    pub default: Option<u16>,
    /// Whether Mutate may move it: not a volume, a tuning or a MIDI setting.
    pub mutate: bool,
    /// Its own channel, 1-16, whatever the device's or a part's: the
    /// Digitakt's delay is on its FX channel, not a track's.
    pub channel: Option<u8>,
    /// It moves the device's value by an offset rather than setting it:
    /// the middle of its range is no change.
    pub relative: bool,
}

impl Parameter {
    /// A CC's parameter across its whole range, in no group.
    pub fn cc(name: impl Into<String>, cc: u8) -> Self {
        Self {
            name: name.into(),
            address: Address::Cc(cc),
            group: String::new(),
            range: (0, MAX_7),
            curve: Curve::Linear,
            values: Vec::new(),
            default: None,
            mutate: true,
            channel: None,
            relative: false,
        }
    }

    /// Whether it takes a few named values rather than a sweep.
    pub fn stepped(&self) -> bool {
        !self.values.is_empty()
    }

    /// Each value's label and what to send for it. Labels without a value
    /// share the range evenly, each sent at the middle of its share, so a
    /// device that splits the range at its own points still lands on it.
    pub fn steps(&self) -> Vec<(&str, u16)> {
        let (lo, hi) = self.range;
        let n = self.values.len() as f32;
        let share = (hi - lo + 1) as f32 / n;
        self.values
            .iter()
            .enumerate()
            .map(|(i, step)| match step {
                Step::At { label, value } => (label.as_str(), *value),
                Step::Label(label) => (label.as_str(), (lo as f32 + share * (i as f32 + 0.5)).floor().min(hi as f32) as u16),
            })
            .collect()
    }

    /// The label of the step `value` falls in: the nearest one sent.
    pub fn label_of(&self, value: u16) -> Option<&str> {
        self.steps().into_iter().min_by_key(|&(_, at)| at.abs_diff(value)).map(|(label, _)| label)
    }

    /// The middle of its range: no change, for a relative one. 64 for a
    /// whole CC, as the makers write it.
    pub fn centre(&self) -> u16 {
        let (lo, hi) = self.range;
        lo + (hi - lo).div_ceil(2)
    }

    /// `value` as a face shows it: a stepped one's label, a relative one's
    /// offset ("+12", "−5", "0"), anything else its number.
    pub fn text(&self, value: u16) -> String {
        if let Some(label) = self.label_of(value) {
            return label.to_string();
        }
        if !self.relative {
            return value.to_string();
        }
        match value as i32 - self.centre() as i32 {
            0 => "0".to_string(),
            offset if offset > 0 => format!("+{offset}"),
            offset => format!("−{}", -offset),
        }
    }

    /// What `typed` asks it to send, as [`text`](Self::text) writes it: a
    /// step's label (any case, or the start of just one), a relative one's
    /// offset ("+12", "-5", "−5"), or a number. A number past the range
    /// sends the end it's past; text that's none of these is `None`.
    pub fn parse(&self, typed: &str) -> Option<u16> {
        let typed = typed.trim();
        if self.stepped() {
            let steps = self.steps();
            let lower = typed.to_lowercase();
            if let Some(&(_, value)) = steps.iter().find(|(label, _)| label.to_lowercase() == lower) {
                return Some(value);
            }
            let starting: Vec<u16> = steps.iter().filter(|(label, _)| !lower.is_empty() && label.to_lowercase().starts_with(&lower)).map(|&(_, value)| value).collect();
            return match starting[..] {
                [value] => Some(value),
                _ => typed.parse::<u16>().ok().filter(|value| steps.iter().any(|&(_, at)| at == *value)),
            };
        }
        let (lo, hi) = self.range;
        let number: i64 = typed.replace('−', "-").trim_start_matches('+').trim().parse().ok()?;
        let value = if self.relative { self.centre() as i64 + number } else { number };
        Some(value.clamp(lo as i64, hi as i64) as u16)
    }

    /// What's wrong with it, if anything: each problem a line.
    fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let name = &self.name;
        if name.trim().is_empty() {
            problems.push("a parameter has no name".to_string());
        }
        match self.address {
            Address::Cc(cc) if cc > 127 => problems.push(format!("{name}: CC {cc} is past 127")),
            Address::Cc14 { msb, lsb } if msb > 127 || lsb > 127 || msb == lsb => {
                problems.push(format!("{name}: CC {msb}/{lsb} isn't a pair of CCs"))
            }
            Address::Nrpn(n) if n > MAX_14 => problems.push(format!("{name}: NRPN {n} is past {MAX_14}")),
            _ => {}
        }
        let (lo, hi) = self.range;
        let max = self.address.max();
        if lo >= hi || hi > max {
            problems.push(format!("{name}: its range {lo}-{hi} isn't within 0-{max}"));
        }
        if let Some(default) = self.default.filter(|d| !(lo..=hi).contains(d)) {
            problems.push(format!("{name}: its default {default} is outside {lo}-{hi}"));
        }
        let steps = self.steps();
        if steps.windows(2).any(|w| w[0].1 >= w[1].1) {
            problems.push(format!("{name}: its values don't climb"));
        }
        if let Some((label, value)) = steps.iter().find(|(_, v)| !(lo..=hi).contains(v)) {
            problems.push(format!("{name}: “{label}” at {value} is outside {lo}-{hi}"));
        }
        if let Some(channel) = self.channel.filter(|c| !(1..=16).contains(c)) {
            problems.push(format!("{name}: its channel {channel} isn't 1-16"));
        }
        if self.relative && self.stepped() {
            problems.push(format!("{name}: a relative parameter can't have named values"));
        }
        problems
    }
}

/// A parameter as it's written: its address in loose fields.
#[derive(Serialize, Deserialize)]
struct RawParameter {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cc: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lsb: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nrpn: Option<u16>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<[u16; 2]>,
    #[serde(default, skip_serializing_if = "is_linear")]
    curve: Curve,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    values: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<u16>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    mutate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel: Option<u8>,
    #[serde(default, skip_serializing_if = "is_false")]
    relative: bool,
}

fn is_linear(curve: &Curve) -> bool {
    *curve == Curve::Linear
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

impl TryFrom<RawParameter> for Parameter {
    type Error = String;

    fn try_from(raw: RawParameter) -> Result<Self, String> {
        let address = match (raw.cc, raw.lsb, raw.nrpn) {
            (Some(cc), None, None) => Address::Cc(cc),
            (Some(msb), Some(lsb), None) => Address::Cc14 { msb, lsb },
            (None, None, Some(n)) => Address::Nrpn(n),
            _ => return Err(format!("{}: give it a cc, a cc and an lsb, or an nrpn", raw.name)),
        };
        let range = raw.range.map_or((0, address.max()), |[lo, hi]| (lo, hi));
        Ok(Parameter {
            name: raw.name,
            address,
            group: raw.group,
            range,
            curve: raw.curve,
            values: raw.values,
            default: raw.default,
            mutate: raw.mutate,
            channel: raw.channel,
            relative: raw.relative,
        })
    }
}

impl From<Parameter> for RawParameter {
    fn from(p: Parameter) -> Self {
        let (cc, lsb, nrpn) = match p.address {
            Address::Cc(cc) => (Some(cc), None, None),
            Address::Cc14 { msb, lsb } => (Some(msb), Some(lsb), None),
            Address::Nrpn(n) => (None, None, Some(n)),
        };
        let range = (p.range != (0, p.address.max())).then_some([p.range.0, p.range.1]);
        RawParameter {
            name: p.name,
            cc,
            lsb,
            nrpn,
            group: p.group,
            range,
            curve: p.curve,
            values: p.values,
            default: p.default,
            mutate: p.mutate,
            channel: p.channel,
            relative: p.relative,
        }
    }
}

/// A CC a gate presses (while it's held) or flips (each time it opens).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub name: String,
    pub cc: u8,
    #[serde(default)]
    pub mode: ButtonMode,
    #[serde(default = "full")]
    pub on: u8,
    #[serde(default)]
    pub off: u8,
    /// Its own channel, when not the device's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
}

fn full() -> u8 {
    127
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ButtonMode {
    #[default]
    Momentary,
    Toggle,
}

/// One of a kit's drums, pads or tracks: the note that plays it, the
/// channel it's on, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
}

/// How its programs are chosen and named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Programs {
    /// What the device calls them: Pattern, Patch, Scene, Memory.
    #[serde(default = "program")]
    pub what: String,
    #[serde(flatten)]
    pub naming: Naming,
    /// The channel it takes program changes on, when not the device's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
    /// The banks Bank Select reaches, when it uses them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub banks: Vec<Bank>,
    /// Each lettered bank is on its own channel, counting up from the
    /// programs' first: the SP-404MKII's bank D is program changes on
    /// channel 4.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bank_by_channel: bool,
}

fn program() -> String {
    "Program".to_string()
}

/// How a program change's number reads on the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "style", rename_all = "lowercase")]
pub enum Naming {
    /// Plain numbers from `first`: 1-128, or 0-127.
    Numbered {
        #[serde(default = "one")]
        first: u16,
        #[serde(default = "all")]
        count: u16,
    },
    /// A letter a bank, then a number within it: the Digitakt's A01-H16.
    Lettered {
        letters: String,
        per_bank: u16,
        #[serde(default = "one")]
        first: u16,
    },
}

fn one() -> u16 {
    1
}

fn all() -> u16 {
    128
}

/// A bank of programs, chosen by Bank Select before the program change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bank {
    pub name: String,
    pub msb: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsb: Option<u8>,
}

impl Programs {
    /// How many there are in a bank, or across every channel's bank for
    /// one banked by channel.
    pub fn count(&self) -> u16 {
        match &self.naming {
            Naming::Numbered { count, .. } => (*count).min(128),
            Naming::Lettered { letters, per_bank, .. } => {
                let all = letters.chars().count() as u16 * per_bank;
                if self.bank_by_channel { all } else { all.min(128) }
            }
        }
    }

    /// Where the `index`th program goes, given the device's channel: the
    /// channel (the programs' own, or else the device's) and the program
    /// change. Banked by channel, each letter is the next channel along.
    pub fn message(&self, index: u16, device_channel: u8) -> (u8, u8) {
        let first = self.channel.unwrap_or(device_channel);
        match &self.naming {
            Naming::Lettered { per_bank, .. } if self.bank_by_channel => {
                let per_bank = (*per_bank).max(1);
                (first + (index / per_bank) as u8, (index % per_bank) as u8)
            }
            _ => (first, index.min(127) as u8),
        }
    }

    /// The `index`th program as the device shows it: "A05", "12". Past
    /// the letters, a plain number.
    pub fn label(&self, index: u16) -> String {
        let n = index;
        match &self.naming {
            Naming::Numbered { first, .. } => (n + first).to_string(),
            Naming::Lettered { letters, per_bank, first } => match letters.chars().nth((n / per_bank.max(&1)) as usize) {
                Some(letter) => format!("{letter}{:02}", n % per_bank + first),
                None => (n + 1).to_string(),
            },
        }
    }

    /// With its word and its bank: "Pattern A05", "Scene User 12".
    pub fn name(&self, bank: Option<usize>, index: u16) -> String {
        match bank.and_then(|b| self.banks.get(b)) {
            Some(bank) => format!("{} {} {}", self.what, bank.name, self.label(index)),
            None => format!("{} {}", self.what, self.label(index)),
        }
    }
}

/// Whether it keeps time with MIDI clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    #[serde(default)]
    pub sends: bool,
    #[serde(default)]
    pub receives: bool,
    /// It starts and stops with Start, Stop and Continue.
    #[serde(default)]
    pub transport: bool,
}

impl Profile {
    /// A profile from its JSON, or why it isn't one.
    pub fn from_json(json: &str) -> Result<Profile, String> {
        let profile: Profile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        match profile.problems().as_slice() {
            [] => Ok(profile),
            problems => Err(problems.join("; ")),
        }
    }

    /// As a file holds it.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a profile serializes")
    }

    /// The parameter called `name`.
    pub fn parameter(&self, name: &str) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.name == name)
    }

    /// Its first channel, 1-16.
    pub fn channel(&self) -> u8 {
        self.channels.first().copied().unwrap_or(1)
    }

    /// What's wrong with it, if anything: each problem a line. A profile
    /// with any isn't loaded.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.name.trim().is_empty() {
            problems.push("it has no name".to_string());
        }
        let channel = |c: u8| (1..=16).contains(&c);
        if self.channels.is_empty() || !self.channels.iter().copied().all(channel) {
            problems.push(format!("its channels {:?} aren't all 1-16", self.channels));
        }
        if matches!(self.voices, Some(Voices::Poly(0) | Voices::Paraphonic(0))) {
            problems.push("it has no voices".to_string());
        }
        for parameter in &self.parameters {
            problems.extend(parameter.problems());
        }
        unique(&mut problems, "parameter", self.parameters.iter().map(|p| p.name.as_str()));
        unique(&mut problems, "button", self.buttons.iter().map(|b| b.name.as_str()));
        unique(&mut problems, "part", self.parts.iter().map(|p| p.name.as_str()));
        for button in &self.buttons {
            if button.cc > 127 || button.on > 127 || button.off > 127 || button.channel.is_some_and(|c| !channel(c)) {
                problems.push(format!("{}: its CC, values or channel are out of range", button.name));
            }
        }
        for part in &self.parts {
            if part.note.is_some_and(|n| n > 127) || part.channel.is_some_and(|c| !channel(c)) {
                problems.push(format!("{}: its note or channel is out of range", part.name));
            }
            if part.note.is_none() && part.channel.is_none() {
                problems.push(format!("{}: give it a note, a channel or both", part.name));
            }
        }
        if self.kind == Kind::Kit && self.parts.is_empty() {
            problems.push("a kit needs its parts".to_string());
        }
        if let Some(programs) = &self.programs {
            if programs.channel.is_some_and(|c| !channel(c)) {
                problems.push("its programs' channel isn't 1-16".to_string());
            }
            if let Naming::Lettered { letters, per_bank, .. } = &programs.naming {
                if letters.is_empty() || *per_bank == 0 {
                    problems.push("its programs need letters and a count a bank".to_string());
                }
            }
            if programs.count() == 0 {
                problems.push("it has no programs".to_string());
            }
            if programs.banks.iter().any(|b| b.msb > 127 || b.lsb.is_some_and(|l| l > 127)) {
                problems.push("a bank's MSB or LSB is past 127".to_string());
            }
            if programs.bank_by_channel {
                match &programs.naming {
                    Naming::Lettered { letters, per_bank, .. } => {
                        let first = programs.channel.unwrap_or(self.channel()) as usize;
                        let last = first + letters.chars().count().max(1) - 1;
                        if last > 16 {
                            problems.push(format!("its banks run from channel {first} past 16, to {last}"));
                        }
                        if *per_bank > 128 {
                            problems.push("a bank on its own channel holds at most 128 programs".to_string());
                        }
                    }
                    Naming::Numbered { .. } => problems.push("programs banked by channel need a letter a bank".to_string()),
                }
            }
        }
        problems
    }
}

/// Adds a problem for each `names` used twice, as `what`s.
fn unique<'a>(problems: &mut Vec<String>, what: &str, names: impl Iterator<Item = &'a str>) {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            problems.push(format!("two {what}s are called {name}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(json: &str) -> Result<Profile, String> {
        Profile::from_json(json)
    }

    #[test]
    fn a_parameter_is_a_cc_a_pair_or_an_nrpn() {
        let p = profile(
            r#"{ "name": "Test", "parameters": [
                { "name": "Cutoff", "cc": 74 },
                { "name": "Fine", "cc": 1, "lsb": 33 },
                { "name": "Deep", "nrpn": 1234, "range": [0, 100] }
            ] }"#,
        )
        .unwrap();
        assert_eq!(p.parameters[0].address, Address::Cc(74));
        assert_eq!(p.parameters[0].range, (0, 127));
        assert_eq!(p.parameters[1].address, Address::Cc14 { msb: 1, lsb: 33 });
        assert_eq!(p.parameters[1].range, (0, MAX_14));
        assert_eq!(p.parameters[2].address, Address::Nrpn(1234));
        assert_eq!(p.parameters[2].range, (0, 100));
        assert!(profile(r#"{ "name": "Test", "parameters": [{ "name": "Both", "cc": 1, "nrpn": 2 }] }"#).is_err());
        assert!(profile(r#"{ "name": "Test", "parameters": [{ "name": "None" }] }"#).is_err());
    }

    #[test]
    fn it_round_trips_as_it_was_written() {
        let json = r#"{ "name": "Test", "maker": "Soba", "kind": "kit", "channels": [10], "voices": { "poly": 8 },
            "parameters": [
                { "name": "Wave", "cc": 9, "group": "Oscillator", "values": ["Saw", { "label": "Square", "value": 100 }], "mutate": false },
                { "name": "Delay", "cc": 85, "group": "FX", "channel": 9 },
                { "name": "Cutoff", "cc": 74, "group": "Filter", "relative": true }
            ],
            "parts": [{ "name": "Kick", "note": 36 }],
            "programs": { "what": "Pattern", "style": "lettered", "letters": "AB", "per_bank": 16, "channel": 10, "bank_by_channel": true },
            "clock": { "sends": true, "receives": true, "transport": true } }"#;
        let p = profile(json).unwrap();
        assert_eq!(profile(&p.to_json()).unwrap(), p);
        assert!(!p.to_json().contains("range"), "a whole range isn't written");
        assert_eq!(p.parameters[1].channel, Some(9));
        assert!(p.parameters[2].relative);
        assert!(p.programs.as_ref().unwrap().bank_by_channel);
        let plain = profile(r#"{ "name": "T", "parameters": [{ "name": "A", "cc": 1 }] }"#).unwrap().to_json();
        assert!(!plain.contains("null") && !plain.contains("relative") && !plain.contains("bank_by_channel"), "fields left out stay out: {plain}");
    }

    #[test]
    fn a_parameter_on_its_own_channel_names_a_real_one() {
        let fx = profile(r#"{ "name": "T", "parameters": [{ "name": "Delay", "cc": 85, "channel": 16 }] }"#).unwrap();
        assert_eq!(fx.parameters[0].channel, Some(16));
        assert_eq!(fx.parameters[0].address, Address::Cc(85));
        let err = profile(r#"{ "name": "T", "parameters": [{ "name": "Delay", "cc": 85, "channel": 0 }] }"#).unwrap_err();
        assert!(err.contains("channel 0 isn't 1-16"), "{err}");
        assert!(profile(r#"{ "name": "T", "parameters": [{ "name": "Delay", "cc": 85, "channel": 17 }] }"#).is_err());
    }

    #[test]
    fn a_relative_parameter_centres_on_no_change_and_reads_as_an_offset() {
        let p = profile(r#"{ "name": "T", "parameters": [{ "name": "Cutoff", "cc": 74, "relative": true }, { "name": "Level", "cc": 7 }] }"#).unwrap();
        let cutoff = &p.parameters[0];
        assert_eq!(cutoff.centre(), 64, "as Roland writes it: 00H - 40H - 7FH is -64 - 0 - +63");
        assert_eq!(cutoff.text(64), "0");
        assert_eq!(cutoff.text(76), "+12");
        assert_eq!(cutoff.text(0), "−64");
        assert_eq!(cutoff.text(127), "+63");
        assert_eq!(p.parameters[1].text(64), "64", "an absolute one is its number");
        let wide = profile(r#"{ "name": "T", "parameters": [{ "name": "Fine", "nrpn": 5, "relative": true }] }"#).unwrap();
        assert_eq!(wide.parameters[0].centre(), 8192);
        let stepped = r#"{ "name": "T", "parameters": [{ "name": "M", "cc": 1, "relative": true, "values": ["A", "B"] }] }"#;
        assert!(profile(stepped).unwrap_err().contains("relative"));
    }

    #[test]
    fn typed_values_read_as_the_face_writes_them() {
        let p = profile(
            r#"{ "name": "T", "parameters": [
                { "name": "Cutoff", "cc": 23 },
                { "name": "Shift", "cc": 74, "relative": true },
                { "name": "Arp", "cc": 49, "values": [{ "label": "Off", "value": 6 }, { "label": "Rise 1", "value": 19 }, { "label": "Fall 2", "value": 65 }] },
                { "name": "Fine", "nrpn": 5, "range": [100, 9000] }
            ] }"#,
        )
        .unwrap();
        let [cutoff, shift, arp, fine] = &p.parameters[..] else { panic!("four parameters") };
        assert_eq!([cutoff.parse("100"), cutoff.parse(" 7 "), cutoff.parse("200"), cutoff.parse("-3")], [Some(100), Some(7), Some(127), Some(0)]);
        assert_eq!(cutoff.parse("loud"), None);
        assert_eq!(fine.parse("50"), Some(100), "below the range is its bottom");
        // An offset from the middle, however the minus is written
        assert_eq!([shift.parse("+12"), shift.parse("12"), shift.parse("-5"), shift.parse("−5"), shift.parse("0")], [Some(76), Some(76), Some(59), Some(59), Some(64)]);
        assert_eq!(shift.text(shift.parse("+12").unwrap()), "+12");
        assert_eq!(shift.parse("+99"), Some(127));
        // A step by name, in any case or by the start of only one
        assert_eq!([arp.parse("Fall 2"), arp.parse("fall 2"), arp.parse("f"), arp.parse("o")], [Some(65), Some(65), Some(65), Some(6)]);
        assert_eq!(arp.parse("ri"), Some(19), "only Rise 1 starts so");
        assert_eq!(arp.parse("65"), Some(65), "a step's own value");
        assert_eq!([arp.parse("64"), arp.parse("Rise 9"), arp.parse("")], [None, None, None]);
    }

    #[test]
    fn programs_banked_by_channel_go_to_their_banks_channel() {
        // The SP-404MKII's chart: PC 15 on channel 4 is bank D, pattern 16.
        let sp = profile(r#"{ "name": "T", "programs": { "what": "Pattern", "style": "lettered", "letters": "ABCDEFGHIJ", "per_bank": 16, "bank_by_channel": true } }"#).unwrap();
        let programs = sp.programs.as_ref().unwrap();
        assert_eq!(programs.count(), 160, "every bank, past 128");
        let d16 = 3 * 16 + 15;
        assert_eq!(programs.label(d16), "D16");
        assert_eq!(programs.message(d16, sp.channel()), (4, 15));
        assert_eq!(programs.message(0, sp.channel()), (1, 0));
        assert_eq!(programs.label(159), "J16");
        assert_eq!(programs.message(159, sp.channel()), (10, 15));

        let flat = Programs {
            what: "Pattern".into(),
            naming: Naming::Lettered { letters: "ABCDEFGH".into(), per_bank: 16, first: 1 },
            channel: Some(10),
            banks: Vec::new(),
            bank_by_channel: false,
        };
        assert_eq!(flat.message(20, 1), (10, 20), "one channel, its own over the device's");
        assert_eq!(flat.count(), 128);

        let numbered = r#"{ "name": "T", "programs": { "style": "numbered", "bank_by_channel": true } }"#;
        assert!(profile(numbered).unwrap_err().contains("letter a bank"));
        let past = r#"{ "name": "T", "channels": [10], "programs": { "style": "lettered", "letters": "ABCDEFGHIJ", "per_bank": 16, "bank_by_channel": true } }"#;
        assert!(profile(past).unwrap_err().contains("past 16"), "banks from channel 10 run out at G");
    }

    #[test]
    fn plain_labels_share_the_range_and_are_sent_at_the_middle_of_their_share() {
        let p = profile(r#"{ "name": "T", "parameters": [{ "name": "Mode", "cc": 1, "values": ["A", "B", "C", "D"] }] }"#).unwrap();
        let mode = &p.parameters[0];
        assert_eq!(mode.steps(), [("A", 16), ("B", 48), ("C", 80), ("D", 112)]);
        assert_eq!(mode.label_of(0), Some("A"));
        assert_eq!(mode.label_of(64), Some("B"));
        assert_eq!(mode.label_of(127), Some("D"));
        let two = profile(r#"{ "name": "T", "parameters": [{ "name": "On", "cc": 1, "values": ["Off", "On"] }] }"#).unwrap();
        assert_eq!(two.parameters[0].steps(), [("Off", 32), ("On", 96)]);
    }

    #[test]
    fn values_that_dont_climb_or_fall_outside_the_range_are_refused() {
        assert!(profile(r#"{ "name": "T", "parameters": [{ "name": "M", "cc": 1, "values": [{ "label": "A", "value": 50 }, { "label": "B", "value": 10 }] }] }"#).is_err());
        assert!(profile(r#"{ "name": "T", "parameters": [{ "name": "M", "cc": 1, "range": [0, 10], "values": [{ "label": "A", "value": 50 }] }] }"#).is_err());
        assert!(profile(r#"{ "name": "T", "parameters": [{ "name": "M", "cc": 200 }] }"#).is_err());
        assert!(profile(r#"{ "name": "T", "parameters": [{ "name": "M", "cc": 1, "range": [0, 200] }] }"#).is_err());
        assert!(profile(r#"{ "name": "T", "channels": [17] }"#).is_err());
    }

    #[test]
    fn names_are_unique_and_a_kit_has_its_parts() {
        let twice = r#"{ "name": "T", "parameters": [{ "name": "A", "cc": 1 }, { "name": "A", "cc": 2 }] }"#;
        assert!(profile(twice).unwrap_err().contains("two parameters are called A"));
        assert!(profile(r#"{ "name": "T", "kind": "kit" }"#).is_err());
        assert!(profile(r#"{ "name": "T", "kind": "kit", "parts": [{ "name": "Kick" }] }"#).is_err(), "a part needs a note or a channel");
    }

    #[test]
    fn programs_read_as_the_device_shows_them() {
        let lettered = Programs {
            what: "Pattern".into(),
            naming: Naming::Lettered { letters: "ABCDEFGH".into(), per_bank: 16, first: 1 },
            channel: None,
            banks: Vec::new(),
            bank_by_channel: false,
        };
        assert_eq!(lettered.label(0), "A01");
        assert_eq!(lettered.label(4), "A05");
        assert_eq!(lettered.label(16), "B01");
        assert_eq!(lettered.label(127), "H16");
        assert_eq!(lettered.count(), 128);
        assert_eq!(lettered.name(None, 4), "Pattern A05");

        let numbered = Programs {
            what: "Scene".into(),
            naming: Naming::Numbered { first: 1, count: 128 },
            channel: None,
            banks: vec![Bank { name: "User".into(), msb: 85, lsb: Some(0) }],
            bank_by_channel: false,
        };
        assert_eq!(numbered.label(11), "12");
        assert_eq!(numbered.name(Some(0), 11), "Scene User 12");
        assert_eq!(numbered.name(Some(5), 11), "Scene 12", "a bank it hasn't is left out");
    }
}
