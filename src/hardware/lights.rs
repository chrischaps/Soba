//! Pad lights (#430): how a controller with RGB pads is told what colour
//! each one is. The MiniLab 3's pads can be lit from the computer by SysEx,
//! one message a pad: which pad, and its red, green and blue.
//!
//! What to send is data, not code, a JSON file a controller like a
//! [`Profile`](super::Profile), so a message found wrong on the real device
//! is put right in the file:
//!
//! ```json
//! { "name": "MiniLab 3", "port": "Minilab3",
//!   "colour": "F0 00 20 6B 7F 42 02 02 16 {pad} {r} {g} {b} F7", "max": 127,
//!   "pads": [{ "id": 4, "note": 36, "channel": 10 }] }
//! ```
//!
//! The colour message is hex bytes with `{pad}`, `{r}`, `{g}` and `{b}`
//! where the pad's id and its colour go, each colour 0 to `max`. Each pad
//! is known by the note and channel it sends, which is how the patch knows
//! what it plays. `enter` and `leave` are messages sent before the first
//! colour and when the lights stop, should the device need a mode set.
//!
//! A file that says `"confirmed": false` was built from what others found
//! by testing, not from its maker's chart, and hasn't been checked on the
//! device yet: the Rig says so beside its lights.

use std::sync::OnceLock;

use serde::Deserialize;

/// The controllers with lights that ship with the app, compiled in.
const BUNDLED: &[(&str, &str)] = &[("minilab-3.json", include_str!("lights/minilab-3.json"))];

/// One byte of a colour message: as written, or where a pad's id or
/// colour goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Byte(u8),
    Pad,
    Red,
    Green,
    Blue,
}

/// One pad that lights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct LitPad {
    /// What the colour message calls it.
    pub id: u8,
    /// The note it sends.
    pub note: u8,
    /// The channel it sends on, 1-16.
    pub channel: u8,
}

/// A controller's pad lights, as its file has them.
#[derive(Debug, Clone, Deserialize)]
struct LightsFile {
    name: String,
    #[serde(default)]
    maker: String,
    port: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    confirmed: bool,
    #[serde(default)]
    enter: Vec<String>,
    #[serde(default)]
    leave: Vec<String>,
    colour: String,
    max: u8,
    pads: Vec<LitPad>,
    #[serde(default)]
    notes: String,
}

/// A controller whose pads Soba can light.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lights {
    pub name: String,
    pub maker: String,
    /// A piece of the name its MIDI ports show up as.
    pub port: String,
    pub source: String,
    /// Checked on the device, not only read about.
    pub confirmed: bool,
    pub enter: Vec<Vec<u8>>,
    pub leave: Vec<Vec<u8>>,
    colour: Vec<Token>,
    /// The brightest a colour's red, green or blue goes.
    pub max: u8,
    pub pads: Vec<LitPad>,
    pub notes: String,
}

/// Bytes written as hex, "F0 00 20 6B F7".
fn hex(text: &str) -> Result<Vec<u8>, String> {
    text.split_whitespace().map(|byte| u8::from_str_radix(byte, 16).map_err(|_| format!("“{byte}” isn't a hex byte"))).collect()
}

/// A colour message, as written.
fn template(text: &str) -> Result<Vec<Token>, String> {
    text.split_whitespace()
        .map(|word| match word {
            "{pad}" => Ok(Token::Pad),
            "{r}" => Ok(Token::Red),
            "{g}" => Ok(Token::Green),
            "{b}" => Ok(Token::Blue),
            byte => u8::from_str_radix(byte, 16).map(Token::Byte).map_err(|_| format!("“{byte}” isn't a hex byte or {{pad}}, {{r}}, {{g}} or {{b}}")),
        })
        .collect()
}

/// A SysEx message: F0 first, F7 last and none of either between.
fn is_sysex(message: &[u8]) -> bool {
    message.len() >= 2 && message[0] == 0xF0 && message[message.len() - 1] == 0xF7 && message[1..message.len() - 1].iter().all(|&b| b < 0x80)
}

impl Lights {
    /// Reads a controller's lights from its file.
    pub fn from_json(json: &str) -> Result<Lights, String> {
        let file: LightsFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let colour = template(&file.colour)?;
        for token in [Token::Pad, Token::Red, Token::Green, Token::Blue] {
            if !colour.contains(&token) {
                return Err(format!("the colour message has no {token:?}"));
            }
        }
        let messages = |list: &[String]| list.iter().map(|m| hex(m)).collect::<Result<Vec<_>, _>>();
        let lights = Lights {
            name: file.name,
            maker: file.maker,
            port: file.port,
            source: file.source,
            confirmed: file.confirmed,
            enter: messages(&file.enter)?,
            leave: messages(&file.leave)?,
            colour,
            max: file.max.min(127),
            pads: file.pads,
            notes: file.notes,
        };
        if !is_sysex(&lights.message(0, [0, 0, 0])) {
            return Err("the colour message isn't one SysEx message".to_string());
        }
        if let Some(bad) = lights.enter.iter().chain(&lights.leave).find(|m| !is_sysex(m)) {
            return Err(format!("{bad:02X?} isn't one SysEx message"));
        }
        if let Some(pad) = lights.pads.iter().find(|p| p.note > 127 || !(1..=16).contains(&p.channel) || p.id > 127) {
            return Err(format!("pad {} has a note, channel or id out of range", pad.id));
        }
        Ok(lights)
    }

    /// The message that lights pad `id` in `rgb`, each 0 to [`max`](Self::max).
    pub fn message(&self, id: u8, rgb: [u8; 3]) -> Vec<u8> {
        let [r, g, b] = rgb.map(|c| c.min(self.max));
        self.colour
            .iter()
            .map(|token| match *token {
                Token::Byte(byte) => byte,
                Token::Pad => id,
                Token::Red => r,
                Token::Green => g,
                Token::Blue => b,
            })
            .collect()
    }

    /// Is `device` this controller, by the name of one of its ports?
    pub fn is(&self, device: &str) -> bool {
        key(device).contains(&key(&self.port))
    }

    /// The pad that sends `note` on `channel` (0-15), if one does.
    pub fn pad_of(&self, note: u8, channel: u8) -> Option<usize> {
        self.pads.iter().position(|p| p.note == note && p.channel == channel + 1)
    }
}

/// A port's name as it's matched: its letters and digits, lower case, so
/// "Minilab3 MIDI" and "MIDIOUT2 (Minilab3 MIDI)" are both a MiniLab 3.
fn key(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Every controller Soba can light.
pub fn all() -> &'static [Lights] {
    static ALL: OnceLock<Vec<Lights>> = OnceLock::new();
    // One that won't load fails `every_bundled_controller_loads`
    ALL.get_or_init(|| BUNDLED.iter().filter_map(|(_, json)| Lights::from_json(json).ok()).collect())
}

/// The lights of the controller `device` is a port of, if Soba can light it.
pub fn of_device(device: &str) -> Option<&'static Lights> {
    all().iter().find(|lights| lights.is(device))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_controller_loads() {
        for (file, json) in BUNDLED {
            Lights::from_json(json).unwrap_or_else(|e| panic!("{file}: {e}"));
        }
        assert_eq!(all().len(), BUNDLED.len());
    }

    #[test]
    fn the_minilab_lights_pad_one_with_arturias_header() {
        let lights = of_device("Minilab3 MIDI").expect("a MiniLab 3");
        let pad = lights.pad_of(36, 9).expect("pad 1 sends C1 on channel 10");
        assert_eq!(pad, 0);
        assert_eq!(
            lights.message(lights.pads[pad].id, [127, 64, 200]),
            vec![0xF0, 0x00, 0x20, 0x6B, 0x7F, 0x42, 0x02, 0x02, 0x16, 0x04, 127, 64, 127, 0xF7],
            "a colour past max is held to it"
        );
        assert_eq!(lights.pads.len(), 16, "both banks");
        assert!(!lights.confirmed, "until it's checked on the device");
    }

    #[test]
    fn either_of_its_ports_is_the_minilab_and_other_devices_are_not() {
        assert!(of_device("MIDIOUT2 (Minilab3 MIDI)").is_some());
        assert!(of_device("2- MiniLab 3").is_some());
        assert!(of_device("Digitakt").is_none());
        assert!(of_device("MiniLab mkII").is_none());
    }

    #[test]
    fn a_file_that_says_no_pad_or_isnt_sysex_is_refused() {
        let file = |colour: &str| format!(r#"{{"name":"X","port":"x","colour":"{colour}","max":127,"pads":[]}}"#);
        assert!(Lights::from_json(&file("F0 {pad} {r} {g} {b} F7")).is_ok());
        assert!(Lights::from_json(&file("F0 {r} {g} {b} F7")).is_err(), "no pad");
        assert!(Lights::from_json(&file("90 {pad} {r} {g} {b}")).is_err(), "not SysEx");
        assert!(Lights::from_json(&file("F0 ZZ {pad} {r} {g} {b} F7")).is_err(), "not hex");
    }
}
