//! A Setlist's rows (#445): for each of a song's sections, what the room
//! changes to as it starts. A row is one device's program: the Digitakt to
//! Pattern A05, the FANTOM to Scene 12, the RC-505 to Memory 7, or plain
//! numbers for a device Soba has no profile for.
//!
//! The node keeps its rows in its labels, as an Instrument keeps its knobs,
//! so they save, copy and undo with the rest of the node: `"row 3.2"` holds
//! section 3's second row, written out as JSON
//! (`{"port":0,"device":"Digitakt","program":4,"channel":1,"lead":250}`).
//!
//! A row naming a device goes as its profile says: the channel a bank
//! banked by channel is on (the SP-404MKII's D16 is program change 15 on
//! channel 4), and its Bank Select. One whose profile this computer doesn't
//! know goes as the patch wrote it down: the program as the program change,
//! on the row's channel, after the bank it wrote.
//!
//! What a row sends reaches the audio thread as a [`RowMap`], small and
//! `Copy`, by value (`EngineCommand::MapRow`), so the profile never does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Profile, Profiles, Programs};
use crate::link::midi::{Program, PORTS};

/// The most rows a section has.
pub const ROWS: usize = 8;

/// The most sections a Setlist has: an Arranger's.
pub const SECTIONS: usize = crate::modules::arranger::SECTIONS;

/// The longest a row's lead is, in milliseconds: a bar at 60 BPM.
pub const MAX_LEAD: u16 = 4000;

/// The label that holds section `section`'s row `row` (both from 0):
/// "row 3.2".
pub fn row_key(section: usize, row: usize) -> String {
    format!("row {}.{}", section + 1, row + 1)
}

/// One device's program, for a section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// The port it goes out of, 0 to 7 (A to H).
    pub port: u8,
    /// The device it names, by its profile's name: empty for plain numbers.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device: String,
    /// The device's program, from 0, in its own order (Pattern A05 is 4),
    /// or a plain row's program change, 0 to 127.
    pub program: u16,
    /// Which of the device's banks, from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank: Option<u8>,
    /// The device's channel, 1 to 16: for one banked by channel, its first
    /// bank's.
    pub channel: u8,
    /// Bank Select, as last written: what a plain row sends, and a row
    /// whose profile isn't known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msb: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsb: Option<u8>,
    /// How long before the bar line it goes, in milliseconds: 0 on the
    /// downbeat.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lead: u16,
}

fn is_zero(lead: &u16) -> bool {
    *lead == 0
}

/// What a row sends, as the audio thread has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowMap {
    /// The port, 0 to 7.
    pub port: u8,
    /// The channel, from 0.
    pub channel: u8,
    pub program: Program,
    /// How long before the bar line it goes, in seconds.
    pub lead: f32,
}

impl Row {
    /// A new row on `port`, for `profile` or plain numbers: its first
    /// program, on the channel it takes programs on.
    pub fn new(port: usize, profile: Option<&Profile>) -> Self {
        let mut row = Self { port: port.min(PORTS - 1) as u8, device: String::new(), program: 0, bank: None, channel: 1, msb: None, lsb: None, lead: 0 };
        if let Some(profile) = profile {
            row.choose(profile);
        }
        row
    }

    /// Names `profile`'s device instead: its first program, on the
    /// channel it takes programs on, in its first bank if it has banks.
    pub fn choose(&mut self, profile: &Profile) {
        self.device = profile.name.clone();
        self.program = 0;
        let programs = profile.programs.as_ref();
        self.channel = programs.and_then(|p| p.channel).unwrap_or_else(|| profile.channel()).clamp(1, 16);
        self.set_bank(programs.filter(|p| !p.banks.is_empty()).map(|_| 0), programs);
    }

    /// Picks bank `bank` of `programs` (None for none), writing its Bank
    /// Select down.
    pub fn set_bank(&mut self, bank: Option<u8>, programs: Option<&Programs>) {
        self.bank = bank;
        let chosen = bank.and_then(|b| programs?.banks.get(b as usize));
        self.msb = chosen.map(|b| b.msb);
        self.lsb = chosen.and_then(|b| b.lsb);
    }

    /// Its device's programs, if its profile is known and has them.
    pub fn programs<'a>(&self, profiles: &'a Profiles) -> Option<&'a Programs> {
        if self.device.is_empty() {
            return None;
        }
        profiles.find(&self.device)?.profile.programs.as_ref()
    }

    /// How many programs it can pick from.
    pub fn count(&self, profiles: &Profiles) -> u16 {
        self.programs(profiles).map_or(128, Programs::count)
    }

    /// What it sends.
    pub fn map(&self, profiles: &Profiles) -> RowMap {
        let (channel, change, bank_msb, bank_lsb) = match self.programs(profiles) {
            Some(programs) => {
                // The profile counts its banks by channel up from its own
                // channel, or else from the one it's given: from the row's
                let (first, change) = programs.message(self.program, self.channel);
                let channel = first.saturating_sub(programs.channel.unwrap_or(self.channel)).saturating_add(self.channel);
                let bank = self.bank.and_then(|b| programs.banks.get(b as usize));
                (channel, change, bank.map(|b| b.msb), bank.and_then(|b| b.lsb))
            }
            None => (self.channel, self.program.min(127) as u8, self.msb, self.lsb),
        };
        RowMap {
            port: self.port.min(PORTS as u8 - 1),
            channel: channel.clamp(1, 16) - 1,
            program: Program { bank_msb: bank_msb.map(|b| b.min(127)), bank_lsb: bank_lsb.map(|b| b.min(127)), program: change.min(127) },
            lead: self.lead.min(MAX_LEAD) as f32 / 1000.0,
        }
    }

    /// The program as the device shows it: "A05", "12", or a plain row's
    /// program change.
    pub fn label(&self, profiles: &Profiles) -> String {
        match self.programs(profiles) {
            Some(programs) => programs.label(self.program),
            None => self.program.min(127).to_string(),
        }
    }

    /// The program with its word and bank: "Pattern A05", "Scene A 12",
    /// or "Program 4" for a plain row.
    pub fn name(&self, profiles: &Profiles) -> String {
        let Some(programs) = self.programs(profiles) else { return format!("Program {}", self.label(profiles)) };
        match self.bank.and_then(|b| programs.banks.get(b as usize)) {
            // A bank that says what it holds needn't say it twice
            Some(bank) if bank.name.to_lowercase().starts_with(&programs.what.to_lowercase()) => format!("{} {}", bank.name, programs.label(self.program)),
            _ => programs.name(self.bank.map(usize::from), self.program),
        }
    }
}

/// Section `section`'s row `row`, if it has one.
pub fn row(labels: &BTreeMap<String, String>, section: usize, row: usize) -> Option<Row> {
    serde_json::from_str(labels.get(&row_key(section, row))?).ok()
}

/// Section `section`'s rows, in their places.
pub fn rows(labels: &BTreeMap<String, String>, section: usize) -> [Option<Row>; ROWS] {
    std::array::from_fn(|r| row(labels, section, r))
}

/// The label that holds `row`.
pub fn written(row: &Row) -> String {
    serde_json::to_string(row).expect("a row serializes")
}

/// What every row of every section sends.
pub fn maps(labels: &BTreeMap<String, String>, profiles: &Profiles) -> Box<[[Option<RowMap>; ROWS]; SECTIONS]> {
    let mut cues = Box::new([[None; ROWS]; SECTIONS]);
    for (key, text) in labels.range("row ".to_string()..) {
        let Some((section, row)) = parse_key(key) else { continue };
        if let Ok(row_of) = serde_json::from_str::<Row>(text) {
            cues[section][row] = Some(row_of.map(profiles));
        }
    }
    cues
}

/// The section and row a label holds, from 0, if it holds one.
fn parse_key(key: &str) -> Option<(usize, usize)> {
    let (section, row) = key.strip_prefix("row ")?.split_once('.')?;
    let (section, row) = (section.parse::<usize>().ok()?.checked_sub(1)?, row.parse::<usize>().ok()?.checked_sub(1)?);
    (section < SECTIONS && row < ROWS).then_some((section, row))
}

/// The highest section with a row, from 0.
pub fn last_section(labels: &BTreeMap<String, String>) -> Option<usize> {
    labels.keys().filter_map(|key| parse_key(key)).map(|(section, _)| section).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles() -> Profiles {
        Profiles::load_from(std::path::Path::new("no such folder"))
    }

    fn labels(rows: &[(usize, usize, &Row)]) -> BTreeMap<String, String> {
        rows.iter().map(|&(s, r, row)| (row_key(s, r), written(row))).collect()
    }

    #[test]
    fn a_digitakt_row_names_its_pattern_and_sends_its_number() {
        let profiles = profiles();
        let digitakt = &profiles.find("Digitakt").unwrap().profile;
        let mut row = Row::new(2, Some(digitakt));
        row.program = 4;
        assert_eq!(row.name(&profiles), "Pattern A05");
        row.program = 16 + 15;
        assert_eq!(row.label(&profiles), "B16");
        let cue = row.map(&profiles);
        assert_eq!((cue.port, cue.channel, cue.program), (2, digitakt.channel() - 1, Program { bank_msb: None, bank_lsb: None, program: 31 }));
    }

    #[test]
    fn a_device_banked_by_channel_sends_on_the_banks_channel() {
        let profiles = profiles();
        let sp = &profiles.find("SP-404MKII").unwrap().profile;
        let mut row = Row::new(0, Some(sp));
        row.program = 3 * 16 + 15;
        assert_eq!(row.label(&profiles), "D16");
        let cue = row.map(&profiles);
        assert_eq!((cue.channel + 1, cue.program.program), (row.channel + 3, 15), "D16 is program change 15 three channels up");
        // Set to another channel, the banks count up from there
        row.channel = 5;
        assert_eq!(row.map(&profiles).channel + 1, 8);
    }

    #[test]
    fn a_banked_device_sends_its_bank_select_first() {
        let profiles = profiles();
        let fantom = &profiles.find("FANTOM-06").unwrap().profile;
        let mut row = Row::new(1, Some(fantom));
        assert_eq!((row.bank, row.msb, row.lsb), (Some(0), Some(85), Some(0)), "its first bank, written down");
        row.set_bank(Some(1), fantom.programs.as_ref());
        row.program = 11;
        assert_eq!(row.name(&profiles), "Scene B 12", "the bank says what it holds");
        assert_eq!(row.map(&profiles).program, Program { bank_msb: Some(85), bank_lsb: Some(1), program: 11 });
    }

    #[test]
    fn a_row_without_its_profile_plays_as_written() {
        let profiles = profiles();
        let row = Row { port: 7, device: "Somebody's Synth".into(), program: 9, bank: Some(0), channel: 3, msb: Some(1), lsb: None, lead: 120 };
        assert_eq!(row.name(&profiles), "Program 9");
        let cue = row.map(&profiles);
        assert_eq!((cue.port, cue.channel, cue.program, cue.lead), (7, 2, Program { bank_msb: Some(1), bank_lsb: None, program: 9 }, 0.12));
        let plain = Row::new(0, None);
        assert_eq!(plain.map(&profiles).program, Program { bank_msb: None, bank_lsb: None, program: 0 });
    }

    #[test]
    fn rows_round_trip_through_their_labels() {
        let profiles = profiles();
        let digitakt = Row::new(0, Some(&profiles.find("Digitakt").unwrap().profile));
        let plain = Row { lead: 500, ..Row::new(4, None) };
        let labels = labels(&[(0, 0, &digitakt), (2, 7, &plain)]);
        assert_eq!(row(&labels, 0, 0), Some(digitakt.clone()));
        assert_eq!(rows(&labels, 2)[7], Some(plain.clone()));
        assert_eq!(labels[&row_key(2, 7)], r#"{"port":4,"program":0,"channel":1,"lead":500}"#, "plain and short");
        let cues = maps(&labels, &profiles);
        assert_eq!(cues[0][0], Some(digitakt.map(&profiles)));
        assert_eq!(cues[2][7].unwrap().lead, 0.5);
        assert_eq!(cues.iter().flatten().filter(|c| c.is_some()).count(), 2);
        assert_eq!(last_section(&labels), Some(2));
        assert_eq!(parse_key("row 33.1"), None);
        assert_eq!(parse_key("row 1.9"), None);
        assert_eq!(parse_key("row 0.1"), None);
    }
}
