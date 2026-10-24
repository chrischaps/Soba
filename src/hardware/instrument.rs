//! An Instrument node's setup (#442): which profile it plays, and which of
//! the profile's parameters each of its knobs holds. A Kit's (#443) too,
//! and which of the profile's parts each of its lanes plays.
//!
//! The node keeps it in its labels, as an Arranger keeps its sections' names,
//! so it saves, copies and undoes with the rest of the node:
//!
//! - `"profile"`: the profile's name, as the Sampler keeps its file.
//! - `"slot 1"` … `"slot 12"`: the parameter the knob holds, written out as
//!   the profile writes it (`{"name":"Cutoff","cc":23,"group":"Filter"}`).
//! - A Kit's `"lane 1"` … `"lane 8"`: the part the lane plays, likewise
//!   (`{"name":"Pad 13","note":36,"channel":1}`).
//!
//! A knob plays its profile's parameter of that name, and a lane its part,
//! so a CC or a note corrected in the profile reaches every patch. One
//! whose profile this computer doesn't know plays what the patch wrote
//! down: the patch carries enough to play the hardware on its own.

use std::collections::BTreeMap;

use super::{Kind, LaneMap, Parameter, Part, Profile, Profiles, SlotMap};
use crate::modules::instrument::SLOTS;
use crate::modules::kit::LANES;

/// Whether a module plays hardware from a profile: an Instrument or a Kit.
pub fn plays_a_profile(module_id: &str) -> bool {
    module_id == crate::modules::instrument::ID || module_id == crate::modules::kit::ID
}

/// The label that names the profile.
pub const PROFILE: &str = "profile";

/// The label that holds knob `slot`'s parameter (from 0): "slot 1".
pub fn slot_key(slot: usize) -> String {
    format!("slot {}", slot + 1)
}

/// The profile a node plays, by name.
pub fn profile_name(labels: &BTreeMap<String, String>) -> Option<&str> {
    labels.get(PROFILE).map(String::as_str).filter(|name| !name.trim().is_empty())
}

/// What a knob holds, and where that came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub parameter: Parameter,
    /// The profile has it: false when it's played from what the patch wrote.
    pub known: bool,
}

/// The parameter knob `slot` holds, if it holds one: its profile's, where
/// the profile is known and has it, or else as the patch wrote it.
pub fn held(labels: &BTreeMap<String, String>, slot: usize, profiles: &Profiles) -> Option<Held> {
    let written: Parameter = serde_json::from_str(labels.get(&slot_key(slot))?).ok()?;
    let profile = profile_name(labels).and_then(|name| profiles.find(name));
    match profile.and_then(|known| known.profile.parameter(&written.name)) {
        Some(parameter) => Some(Held { parameter: parameter.clone(), known: true }),
        None => Some(Held { parameter: written, known: false }),
    }
}

/// What every knob sends.
pub fn maps(labels: &BTreeMap<String, String>, profiles: &Profiles) -> [SlotMap; SLOTS] {
    std::array::from_fn(|slot| held(labels, slot, profiles).map_or(SlotMap::EMPTY, |held| SlotMap::of(&held.parameter)))
}

/// The label that has a knob hold `parameter`.
pub fn written(parameter: &Parameter) -> String {
    serde_json::to_string(parameter).expect("a parameter serializes")
}

/// The labels that have a node play `profile`, each knob holding one of
/// its first twelve parameters (an empty text takes a label away).
pub fn choose(profile: &Profile) -> Vec<(String, String)> {
    let mut labels = vec![(PROFILE.to_string(), profile.name.clone())];
    labels.extend((0..SLOTS).map(|slot| (slot_key(slot), profile.parameters.get(slot).map(written).unwrap_or_default())));
    labels
}

/// The label that holds lane `lane`'s part (from 0): "lane 1".
pub fn lane_key(lane: usize) -> String {
    format!("lane {}", lane + 1)
}

/// The part lane `lane` plays, if it plays one: its profile's, where the
/// profile is known and has it, or else as the patch wrote it.
pub fn part(labels: &BTreeMap<String, String>, lane: usize, profiles: &Profiles) -> Option<Part> {
    let written: Part = serde_json::from_str(labels.get(&lane_key(lane))?).ok()?;
    let profile = profile_name(labels).and_then(|name| profiles.find(name));
    Some(profile.and_then(|known| known.profile.parts.iter().find(|p| p.name == written.name)).cloned().unwrap_or(written))
}

/// What every lane plays. Knobs follow a lane's track only where the
/// profile, known, says its parts take the same CCs each on its channel.
pub fn lane_maps(labels: &BTreeMap<String, String>, profiles: &Profiles) -> [LaneMap; LANES] {
    let cc_per_part = profile_name(labels).and_then(|name| profiles.find(name)).is_some_and(|known| known.profile.cc_per_part);
    std::array::from_fn(|lane| part(labels, lane, profiles).map_or(LaneMap::EMPTY, |part| LaneMap::of(&part, cc_per_part)))
}

/// The label that has a lane play `part`.
pub fn written_part(part: &Part) -> String {
    serde_json::to_string(part).expect("a part serializes")
}

/// The labels that have a Kit play `profile`: as [`choose`], and each lane
/// playing one of its first eight parts.
pub fn choose_kit(profile: &Profile) -> Vec<(String, String)> {
    let mut labels = choose(profile);
    labels.extend((0..LANES).map(|lane| (lane_key(lane), profile.parts.get(lane).map(written_part).unwrap_or_default())));
    labels
}

/// Whether a Kit can play `profile`: a kit, with its parts.
pub fn is_kit(profile: &Profile) -> bool {
    profile.kind == Kind::Kit && !profile.parts.is_empty()
}

/// Where a knob sits to start `parameter` from: its default, if the chart
/// gives one, or else the middle (no change, for a relative one).
pub fn start(parameter: &Parameter) -> f32 {
    let map = SlotMap::of(parameter);
    match parameter.default {
        Some(value) => map.travel(value),
        None if parameter.stepped() => map.travel(map.steps()[0]),
        None => map.travel(parameter.centre()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn profiles() -> Profiles {
        Profiles::load_from(Path::new("no such folder"))
    }

    fn labels(pairs: Vec<(String, String)>) -> BTreeMap<String, String> {
        pairs.into_iter().filter(|(_, text)| !text.is_empty()).collect()
    }

    #[test]
    fn choosing_a_profile_holds_its_first_twelve() {
        let profiles = profiles();
        let freak = &profiles.find("MicroFreak").unwrap().profile;
        let chosen = labels(choose(freak));
        assert_eq!(profile_name(&chosen), Some("MicroFreak"));
        for slot in 0..SLOTS {
            let held = held(&chosen, slot, &profiles).unwrap();
            assert_eq!(held.parameter, freak.parameters[slot]);
            assert!(held.known);
        }
        // A profile with fewer leaves the rest empty
        let fm = &profiles.find("volca fm").unwrap().profile;
        assert!(fm.parameters.len() < SLOTS);
        let chosen = labels(choose(fm));
        assert!(held(&chosen, SLOTS - 1, &profiles).is_none());
        assert!(maps(&chosen, &profiles)[SLOTS - 1].is_empty());
    }

    #[test]
    fn a_knob_plays_its_profile_and_without_one_what_the_patch_wrote() {
        let profiles = profiles();
        let freak = &profiles.find("MicroFreak").unwrap().profile;
        // The patch wrote Cutoff down on another CC than the profile's now
        let mut stale = freak.parameters[0].clone();
        let right = stale.address;
        stale.address = super::super::Address::Cc(1);
        let mut patch = BTreeMap::from([(PROFILE.to_string(), "microfreak".to_string()), (slot_key(0), written(&stale))]);
        assert_eq!(held(&patch, 0, &profiles).unwrap().parameter.address, right, "the profile's, corrected");
        // A profile this computer doesn't know: the patch's own
        patch.insert(PROFILE.to_string(), "Someone's Synth".to_string());
        let alone = held(&patch, 0, &profiles).unwrap();
        assert_eq!(alone.parameter, stale);
        assert!(!alone.known);
        assert_eq!(maps(&patch, &profiles)[0], SlotMap::of(&stale));
        // Nonsense holds nothing
        patch.insert(slot_key(1), "{ nope".to_string());
        assert!(held(&patch, 1, &profiles).is_none());
    }

    #[test]
    fn a_knob_starts_at_its_default_or_the_middle() {
        let mut p = Parameter::cc("Cutoff", 23);
        assert!((start(&p) - 64.0 / 127.0).abs() < 1e-6);
        p.default = Some(127);
        assert_eq!(start(&p), 1.0);
        let profiles = profiles();
        let fm = &profiles.find("volca fm").unwrap().profile;
        let transpose = fm.parameter("Transpose").unwrap();
        // Its "0" step is sent at 64, and a knob there sends it
        assert_eq!(SlotMap::of(transpose).value(start(transpose)), transpose.default.unwrap_or(transpose.steps()[0].1));
    }
}
