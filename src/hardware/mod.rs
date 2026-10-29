//! The hardware in the room (Phase 17, The Rig): what each device is
//! called, listens to and can do, as a [`Profile`].
//!
//! Profiles ship with the app, compiled in so the browser build has them
//! too (`src/hardware/profiles/`), each built from its maker's official
//! MIDI implementation chart. More live in a folder of your own,
//! `Documents/Soba/Instruments`, one JSON file each: made by hand, or by
//! listening to a device's knobs ([`listening`]). One of yours with a
//! shipped profile's name takes its place.

pub mod breeding;
pub mod button;
pub mod dx7;
pub mod garden;
pub mod hearing;
pub mod instrument;
pub mod lane;
pub mod listening;
#[cfg(test)]
mod pages;
pub mod profile;
pub mod setlist;
pub mod slot;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub use profile::{Address, Button, ButtonMode, Clock, Curve, Kind, Naming, Parameter, Part, Profile, Programs, Step, Voices};
pub use button::ButtonMap;
pub use lane::LaneMap;
pub use setlist::RowMap;
pub use slot::SlotMap;

/// A profile that ships with the app.
struct Bundled {
    /// Its manual page's name, and the one the tests name when it won't parse.
    #[cfg_attr(not(test), allow(dead_code))]
    file_name: &'static str,
    json: &'static str,
}

macro_rules! bundled {
    ($($file:literal),* $(,)?) => {
        [$(Bundled { file_name: $file, json: include_str!(concat!("profiles/", $file)) }),*]
    };
}

/// Every profile that ships, in menu order.
const BUNDLED: [Bundled; 10] = bundled![
    "digitakt.json",
    "sp-404mkii.json",
    "rc-505.json",
    "rc-505mkii.json",
    "microfreak.json",
    "polyend-tracker.json",
    "volca-fm.json",
    "volca-drum.json",
    "fantom-06.json",
    "zoia.json",
];

/// Where a profile came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// It ships with the app.
    Bundled,
    /// A file in the Instruments folder.
    File(PathBuf),
}

/// A profile, and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Known {
    pub profile: Profile,
    pub origin: Origin,
}

/// The profiles Soba knows: the shipped ones, yours in their place where
/// they share a name, and yours besides. Each file that isn't a profile
/// says why.
#[derive(Debug, Clone, Default)]
pub struct Profiles {
    known: Vec<Known>,
    /// Each file in the folder that wouldn't load, and why.
    pub problems: Vec<(PathBuf, String)>,
}

/// Where your profiles live.
pub fn folder() -> PathBuf {
    dirs::document_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Documents")))
        // The browser has neither, and its temp_dir panics: a path there
        // reads as no profiles, and saving to it fails with a message
        .unwrap_or_else(|| if crate::app::WEB { PathBuf::new() } else { std::env::temp_dir() })
        .join("Soba")
        .join("Instruments")
}

/// The profiles that ship, parsed. One that won't is a bug, which the
/// tests catch, and is passed over.
pub fn bundled() -> Vec<Profile> {
    BUNDLED.iter().filter_map(|b| Profile::from_json(b.json).ok()).collect()
}

impl Profiles {
    /// The shipped profiles and the ones in your folder.
    pub fn load() -> Self {
        Self::load_from(&folder())
    }

    /// The shipped profiles and the ones in `folder`.
    pub fn load_from(folder: &Path) -> Self {
        let mut profiles = Self { known: bundled().into_iter().map(|profile| Known { profile, origin: Origin::Bundled }).collect(), problems: Vec::new() };
        let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
            .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))).collect())
            .unwrap_or_default();
        files.sort();
        for path in files {
            let read = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|json| Profile::from_json(&json));
            match read {
                Ok(profile) => profiles.add(Known { profile, origin: Origin::File(path) }),
                Err(why) => profiles.problems.push((path, why)),
            }
        }
        profiles
    }

    /// Adds one, in the place of any of its name.
    fn add(&mut self, known: Known) {
        match self.known.iter_mut().find(|k| same_name(&k.profile.name, &known.profile.name)) {
            Some(place) => *place = known,
            None => self.known.push(known),
        }
    }

    /// Every one, shipped ones first in their order, then yours by name.
    pub fn all(&self) -> &[Known] {
        &self.known
    }

    /// The one called `name`.
    pub fn find(&self, name: &str) -> Option<&Known> {
        self.known.iter().find(|k| same_name(&k.profile.name, name))
    }

    /// The one a MIDI port called `port` most likely is: the longest hint
    /// it holds, so "RC-505mkII" beats "RC-505".
    pub fn suggest(&self, port: &str) -> Option<&Known> {
        let port = port.to_lowercase();
        self.known
            .iter()
            .filter(|k| !k.profile.port.is_empty() && port.contains(&k.profile.port.to_lowercase()))
            .max_by_key(|k| k.profile.port.len())
    }
}

/// The profiles this computer knows, read once and shared: the
/// Instruments' faces look theirs up every frame. [`reload`] reads them
/// again, after one is saved.
pub fn known() -> Arc<Profiles> {
    shelf().lock().map(|known| known.clone()).unwrap_or_default()
}

/// Reads the profiles again, for a folder that's changed.
pub fn reload() {
    if let Ok(mut known) = shelf().lock() {
        *known = Arc::new(Profiles::load());
    }
}

fn shelf() -> &'static Mutex<Arc<Profiles>> {
    static KNOWN: OnceLock<Mutex<Arc<Profiles>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(Arc::new(Profiles::load())))
}

/// Names that differ only in case or the spaces around them are one name.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Saves `profile` to `folder`, named after it. Returns where it went.
pub fn save_in(folder: &Path, profile: &Profile) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let path = folder.join(crate::app::library::file_name(&profile.name));
    std::fs::write(&path, profile.to_json())?;
    Ok(path)
}

/// The colour of a parameter's group: a module category's, as Soba's own
/// modules wear them, so an oscillator's knob on a MicroFreak is the blue
/// of an Oscillator's header.
pub fn group_color(group: &str) -> egui::Color32 {
    use crate::app::theme::module as category;
    match group.to_ascii_lowercase().as_str() {
        "oscillator" | "sample" | "voice" => category::SOURCE,
        "filter" => category::FILTER,
        "envelope" | "lfo" | "arp" => category::MODULATION,
        "fx" => category::EFFECT,
        _ => category::UTILITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn folder(test: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("soba-instruments-test").join(test);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn every_bundled_profile_parses_and_names_its_chart() {
        let mut names = HashSet::new();
        for bundled in &BUNDLED {
            let profile = Profile::from_json(bundled.json).unwrap_or_else(|why| panic!("{}: {why}", bundled.file_name));
            assert!(!profile.source.trim().is_empty(), "{} names the chart it follows", bundled.file_name);
            assert!(!profile.maker.is_empty() && !profile.port.is_empty(), "{} has its maker and port", bundled.file_name);
            assert!(!profile.parameters.is_empty() || !profile.parts.is_empty() || !profile.buttons.is_empty(), "{} has something to play", bundled.file_name);
            assert!(names.insert(profile.name.to_lowercase()), "{} is the only {}", bundled.file_name, profile.name);
        }
        assert_eq!(bundled().len(), BUNDLED.len());
    }

    #[test]
    fn bundled_parameters_keep_within_reach() {
        for profile in bundled() {
            for p in &profile.parameters {
                assert!(p.name.chars().count() <= 16, "{}: {} fits on a knob", profile.name, p.name);
                if let Address::Cc(cc) = p.address {
                    // Bank Select, the NRPN numbers and the channel modes aren't parameters
                    assert!(!matches!(cc, 0 | 32 | 98 | 99 | 120..=127), "{}: {} on CC {cc}", profile.name, p.name);
                }
            }
        }
    }

    #[test]
    fn a_user_profile_with_a_bundled_name_takes_its_place() {
        let folder = folder("override");
        let mine = r#"{ "name": "microfreak", "port": "MicroFreak", "parameters": [{ "name": "Mine", "cc": 20 }] }"#;
        std::fs::write(folder.join("my freak.json"), mine).unwrap();
        std::fs::write(folder.join("new.json"), r#"{ "name": "Box", "parameters": [{ "name": "A", "cc": 1 }] }"#).unwrap();
        std::fs::write(folder.join("broken.json"), "{ nope").unwrap();
        let profiles = Profiles::load_from(&folder);

        assert_eq!(profiles.all().len(), BUNDLED.len() + 1, "one replaced, one added");
        let freak = profiles.find("MicroFreak").unwrap();
        assert_eq!(freak.profile.parameters[0].name, "Mine");
        assert_eq!(freak.origin, Origin::File(folder.join("my freak.json")));
        assert!(profiles.find("Box").is_some());
        assert_eq!(profiles.problems.len(), 1);
        assert!(profiles.problems[0].0.ends_with("broken.json"));
    }

    #[test]
    fn a_port_suggests_the_profile_whose_hint_it_holds() {
        let profiles = Profiles::load_from(Path::new("no such folder"));
        assert_eq!(profiles.suggest("2- Digitakt").map(|k| k.profile.name.as_str()), Some("Digitakt"));
        assert_eq!(profiles.suggest("MicroFreak MIDI").map(|k| k.profile.name.as_str()), Some("MicroFreak"));
        assert!(profiles.suggest("MiniLab3 MIDI").is_none());
    }

    #[test]
    fn a_saved_profile_loads_back() {
        let folder = folder("save");
        let mut profile = bundled().remove(0);
        profile.name = "Saved: Copy".into();
        let path = save_in(&folder, &profile).unwrap();
        assert_eq!(path.file_name().unwrap(), "Saved  Copy.json");
        assert_eq!(Profiles::load_from(&folder).find("Saved: Copy").unwrap().profile, profile);
    }
}
