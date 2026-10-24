//! A profile made by listening (#441): for a device that sends its knobs
//! as CCs (the Digitakt, the MicroFreak), turn each knob on it and type
//! its name. Each CC heard becomes a parameter, in the order its knobs
//! were first turned, on the channel it came on. What comes out is a
//! profile of your own, saved to the Instruments folder.

use super::profile::{Kind, Parameter, Profile};

/// One knob heard: the CC it sends, and the name typed for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeardKnob {
    pub cc: u8,
    /// 1-16.
    pub channel: u8,
    /// Its last value, so the row can show it moving.
    pub value: u8,
    /// How many values it has sent, so a row can flash as it moves.
    pub moves: u32,
    /// The name typed for it; empty until one is.
    pub name: String,
}

impl HeardKnob {
    /// Its name, or "CC 74" until it has one.
    pub fn shown_name(&self) -> String {
        if self.name.trim().is_empty() { format!("CC {}", self.cc) } else { self.name.trim().to_string() }
    }
}

/// Listening to one device's knobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listening {
    /// The input's name, which becomes the profile's port hint.
    pub device: String,
    /// What the profile will be called.
    pub name: String,
    pub knobs: Vec<HeardKnob>,
}

/// CCs that are a device's MIDI housekeeping, not one of its knobs: Bank
/// Select, the NRPN and RPN numbers and data, and the channel modes.
fn housekeeping(cc: u8) -> bool {
    matches!(cc, 0 | 6 | 32 | 38 | 96..=101 | 120..=127)
}

impl Listening {
    /// Listening to the device called `device`, the profile named after it.
    pub fn new(device: impl Into<String>) -> Self {
        let device = device.into();
        Self { name: device.clone(), device, knobs: Vec::new() }
    }

    /// Hears a CC (channel 0-15) from the device. Returns true when it's
    /// a knob not heard before.
    pub fn hear(&mut self, channel: u8, cc: u8, value: u8) -> bool {
        if housekeeping(cc) {
            return false;
        }
        let channel = channel + 1;
        if let Some(knob) = self.knobs.iter_mut().find(|k| k.cc == cc && k.channel == channel) {
            knob.value = value;
            knob.moves = knob.moves.wrapping_add(1);
            return false;
        }
        self.knobs.push(HeardKnob { cc, channel, value, moves: 1, name: String::new() });
        true
    }

    /// Forgets the knob in row `index`: turned by mistake.
    pub fn forget(&mut self, index: usize) {
        if index < self.knobs.len() {
            self.knobs.remove(index);
        }
    }

    /// The channel most of the knobs came on.
    fn channel(&self) -> u8 {
        let mut counts = [0usize; 17];
        for knob in &self.knobs {
            counts[knob.channel as usize] += 1;
        }
        (1..=16u8).max_by_key(|&c| (counts[c as usize], std::cmp::Reverse(c))).filter(|&c| counts[c as usize] > 0).unwrap_or(1)
    }

    /// Why it can't be saved yet, if it can't.
    pub fn not_ready(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("Name the instrument".to_string());
        }
        if self.knobs.is_empty() {
            return Some("Turn a knob on the device".to_string());
        }
        let profile = self.profile();
        profile.problems().into_iter().next()
    }

    /// The profile it has heard: a parameter a knob, on the channel most
    /// came on. A knob heard on another keeps it as its own, as the
    /// Digitakt's delay keeps its FX channel.
    pub fn profile(&self) -> Profile {
        let channel = self.channel();
        let parameters = self
            .knobs
            .iter()
            .map(|k| match k.channel == channel {
                true => Parameter::cc(k.shown_name(), k.cc),
                false => {
                    let name = if k.name.trim().is_empty() { format!("CC {} ch {}", k.cc, k.channel) } else { k.shown_name() };
                    Parameter { channel: Some(k.channel), ..Parameter::cc(name, k.cc) }
                }
            })
            .collect();
        Profile {
            name: self.name.trim().to_string(),
            maker: String::new(),
            about: String::new(),
            source: format!("Heard from {} by Soba", self.device),
            port: self.device.clone(),
            kind: Kind::Synth,
            channels: vec![channel],
            voices: None,
            parameters,
            buttons: Vec::new(),
            parts: Vec::new(),
            cc_per_part: false,
            programs: None,
            clock: Default::default(),
            notes: String::new(),
        }
    }

    /// The knobs heard on another channel than the profile's, which keep
    /// their own.
    pub fn off_channel(&self) -> usize {
        let channel = self.channel();
        self.knobs.iter().filter(|k| k.channel != channel).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{save_in, Profiles};

    #[test]
    fn each_knob_is_heard_once_in_the_order_it_was_turned() {
        let mut listening = Listening::new("MicroFreak");
        assert!(listening.hear(0, 23, 10));
        assert!(listening.hear(0, 83, 0));
        assert!(!listening.hear(0, 23, 11), "still turning");
        assert!(!listening.hear(0, 0, 1), "Bank Select isn't a knob");
        assert!(!listening.hear(0, 123, 0), "nor All Notes Off");
        assert_eq!(listening.knobs.iter().map(|k| (k.cc, k.value)).collect::<Vec<_>>(), [(23, 11), (83, 0)]);
    }

    #[test]
    fn named_knobs_become_parameters_and_the_rest_keep_their_numbers() {
        let mut listening = Listening::new("2- MicroFreak");
        listening.name = "My Freak".into();
        listening.hear(2, 23, 0);
        listening.hear(2, 83, 0);
        listening.hear(5, 40, 0);
        listening.knobs[0].name = " Cutoff ".into();
        let profile = listening.profile();
        assert_eq!(profile.channels, [3]);
        assert_eq!(profile.port, "2- MicroFreak");
        assert_eq!(profile.parameters.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Cutoff", "CC 83", "CC 40 ch 6"]);
        assert_eq!(profile.parameters.iter().map(|p| p.channel).collect::<Vec<_>>(), [None, None, Some(6)], "the odd one out keeps its channel");
        assert_eq!(listening.off_channel(), 1);
        assert_eq!(listening.not_ready(), None);
    }

    #[test]
    fn one_cc_on_two_channels_is_two_parameters() {
        // A Digitakt's track filter and its FX channel's delay time both turn up as CCs from one port
        let mut listening = Listening::new("Elektron Digitakt");
        listening.name = "Heard Digitakt".into();
        listening.hear(0, 74, 10);
        listening.hear(0, 75, 10);
        listening.hear(8, 85, 10);
        listening.hear(8, 74, 10);
        let profile = listening.profile();
        assert_eq!(profile.channels, [1]);
        assert_eq!(profile.parameters.len(), 4);
        assert_eq!(profile.parameters[3].name, "CC 74 ch 9");
        assert_eq!(profile.parameters[3].channel, Some(9));
        assert_eq!(listening.not_ready(), None, "no two names alike");
    }

    #[test]
    fn two_knobs_of_one_name_or_none_heard_cant_be_saved() {
        let mut listening = Listening::new("Box");
        assert!(listening.not_ready().is_some());
        listening.hear(0, 20, 0);
        listening.hear(0, 21, 0);
        listening.knobs[0].name = "Same".into();
        listening.knobs[1].name = "Same".into();
        assert!(listening.not_ready().unwrap().contains("Same"));
    }

    #[test]
    fn a_profile_heard_saves_and_loads_back() {
        let folder = std::env::temp_dir().join("soba-instruments-test").join("listening");
        let _ = std::fs::remove_dir_all(&folder);
        let mut listening = Listening::new("Digitakt");
        listening.name = "Heard Digitakt".into();
        listening.hear(0, 74, 64);
        listening.knobs[0].name = "Filter".into();
        save_in(&folder, &listening.profile()).unwrap();
        let profiles = Profiles::load_from(&folder);
        assert!(profiles.problems.is_empty());
        assert_eq!(profiles.find("Heard Digitakt").unwrap().profile, listening.profile());
    }
}
