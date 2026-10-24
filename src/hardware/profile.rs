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
//! evenly, and labels with a `value` say exactly what to send.

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
        RawParameter { name: p.name, cc, lsb, nrpn, group: p.group, range, curve: p.curve, values: p.values, default: p.default, mutate: p.mutate }
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
    /// How many there are in a bank.
    pub fn count(&self) -> u16 {
        match &self.naming {
            Naming::Numbered { count, .. } => (*count).min(128),
            Naming::Lettered { letters, per_bank, .. } => (letters.chars().count() as u16 * per_bank).min(128),
        }
    }

    /// Program change `program` (0-127) as the device shows it: "A05",
    /// "12". Past the letters, a plain number.
    pub fn label(&self, program: u8) -> String {
        let n = program as u16;
        match &self.naming {
            Naming::Numbered { first, .. } => (n + first).to_string(),
            Naming::Lettered { letters, per_bank, first } => match letters.chars().nth((n / per_bank.max(&1)) as usize) {
                Some(letter) => format!("{letter}{:02}", n % per_bank + first),
                None => (n + 1).to_string(),
            },
        }
    }

    /// With its word and its bank: "Pattern A05", "Scene User 12".
    pub fn name(&self, bank: Option<usize>, program: u8) -> String {
        match bank.and_then(|b| self.banks.get(b)) {
            Some(bank) => format!("{} {} {}", self.what, bank.name, self.label(program)),
            None => format!("{} {}", self.what, self.label(program)),
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
            "parameters": [{ "name": "Wave", "cc": 9, "group": "Oscillator", "values": ["Saw", { "label": "Square", "value": 100 }], "mutate": false }],
            "parts": [{ "name": "Kick", "note": 36 }],
            "programs": { "what": "Pattern", "style": "lettered", "letters": "AB", "per_bank": 16, "channel": 10 },
            "clock": { "sends": true, "receives": true, "transport": true } }"#;
        let p = profile(json).unwrap();
        assert_eq!(profile(&p.to_json()).unwrap(), p);
        assert!(!p.to_json().contains("range"), "a whole range isn't written");
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
        };
        assert_eq!(numbered.label(11), "12");
        assert_eq!(numbered.name(Some(0), 11), "Scene User 12");
        assert_eq!(numbered.name(Some(5), 11), "Scene 12", "a bank it hasn't is left out");
    }
}
