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
//! - A Kit's `"slot 1 track"` … (#468): the part a knob turns whatever the
//!   Track says, likewise (`{"name":"Track 3","note":2,"channel":3}`). A
//!   knob without one turns the Track's.
//! - An Instrument's `"button 1"` … `"button 8"` (#474): the button its
//!   jack presses, likewise (`{"name":"Target Trk Rec/Play","cc":14}`).
//!
//! - A Strike's `"part"` (#484): the part it strikes, as a lane's; a CC
//!   Out's `"parameter"`: the parameter it sends, as a knob's. Each keeps
//!   its device's `"profile"` too, copied over its MIDI cable.
//!
//! A knob plays its profile's parameter of that name, a lane its part and
//! a button its button, so a CC or a note corrected in the profile reaches
//! every patch. One
//! whose profile this computer doesn't know plays what the patch wrote
//! down: the patch carries enough to play the hardware on its own.

use std::collections::BTreeMap;

use super::{Button, ButtonMap, Kind, LaneMap, Parameter, Part, Profile, Profiles, SlotMap};
use crate::modules::instrument::{BUTTONS, SLOTS, SLOT_NAMES};
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

/// The knob (from 0) a node's control `param_name` is: "Slot 3" is 2. Its
/// other controls (Port, Channel, a Kit's Track) are none.
pub fn slot_named(param_name: &str) -> Option<usize> {
    SLOT_NAMES.iter().position(|&name| name == param_name)
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
    held_under(labels, &slot_key(slot), profiles)
}

/// The parameter the label `key` holds, from the profile where it's known.
fn held_under(labels: &BTreeMap<String, String>, key: &str, profiles: &Profiles) -> Option<Held> {
    let written: Parameter = serde_json::from_str(labels.get(key)?).ok()?;
    let profile = profile_name(labels).and_then(|name| profiles.find(name));
    match profile.and_then(|known| known.profile.parameter(&written.name)) {
        Some(parameter) => Some(Held { parameter: parameter.clone(), known: true }),
        None => Some(Held { parameter: written, known: false }),
    }
}

/// How many steps share a knob's travel when it holds a stepped
/// parameter: Mutate moves it by whole ones, and a morph flips it once
/// (#447). None for one that sweeps.
pub fn steps_of(parameter: &Parameter) -> Option<usize> {
    let count = SlotMap::of(parameter).steps().len();
    (count > 0).then_some(count)
}

/// What every knob sends. A Kit's knob on a track of its own goes out on
/// that track's channel, but for a parameter with a channel of its own
/// (the Digitakt's FX channel), which still wins.
pub fn maps(labels: &BTreeMap<String, String>, profiles: &Profiles) -> [SlotMap; SLOTS] {
    std::array::from_fn(|slot| {
        let Some(held) = held(labels, slot, profiles) else { return SlotMap::EMPTY };
        let mut map = SlotMap::of(&held.parameter);
        if map.channel.is_none() {
            map.channel = own_track(labels, slot, profiles).and_then(|part| part.channel);
        }
        map
    })
}

/// The label that has a knob hold `parameter`.
pub fn written(parameter: &Parameter) -> String {
    serde_json::to_string(parameter).expect("a parameter serializes")
}

/// The labels that have an Instrument play `profile`, each knob holding
/// one of its first twelve parameters and each button one of its first
/// eight buttons (an empty text takes a label away).
pub fn choose(profile: &Profile) -> Vec<(String, String)> {
    let mut labels = choose_knobs(profile);
    labels.extend((0..BUTTONS).map(|button| (button_key(button), profile.buttons.get(button).map(written_button).unwrap_or_default())));
    labels
}

/// The labels that have a node play `profile`'s knobs.
fn choose_knobs(profile: &Profile) -> Vec<(String, String)> {
    let mut labels = vec![(PROFILE.to_string(), profile.name.clone())];
    labels.extend((0..SLOTS).map(|slot| (slot_key(slot), profile.parameters.get(slot).map(written).unwrap_or_default())));
    // Every knob turns the Track's, to start with
    labels.extend((0..SLOTS).map(|slot| (slot_track_key(slot), String::new())));
    labels
}

/// The label that holds button `button`'s button (from 0): "button 1".
pub fn button_key(button: usize) -> String {
    format!("button {}", button + 1)
}

/// The button an Instrument's button `button` presses, if it holds one:
/// its profile's, where the profile is known and has it, or else as the
/// patch wrote it.
pub fn button(labels: &BTreeMap<String, String>, button: usize, profiles: &Profiles) -> Option<Button> {
    let written: Button = serde_json::from_str(labels.get(&button_key(button))?).ok()?;
    let profile = profile_name(labels).and_then(|name| profiles.find(name));
    Some(profile.and_then(|known| known.profile.buttons.iter().find(|b| b.name == written.name)).cloned().unwrap_or(written))
}

/// What every button presses.
pub fn button_maps(labels: &BTreeMap<String, String>, profiles: &Profiles) -> [ButtonMap; BUTTONS] {
    std::array::from_fn(|index| button(labels, index, profiles).map_or(ButtonMap::EMPTY, |b| ButtonMap::of(&b)))
}

/// The label that has a button press `button`.
pub fn written_button(button: &Button) -> String {
    serde_json::to_string(button).expect("a button serializes")
}

/// The label that holds lane `lane`'s part (from 0): "lane 1".
pub fn lane_key(lane: usize) -> String {
    format!("lane {}", lane + 1)
}

/// The part lane `lane` plays, if it plays one: its profile's, where the
/// profile is known and has it, or else as the patch wrote it.
pub fn part(labels: &BTreeMap<String, String>, lane: usize, profiles: &Profiles) -> Option<Part> {
    part_under(labels, &lane_key(lane), profiles)
}

/// The part the label `key` holds, from the profile where it's known.
fn part_under(labels: &BTreeMap<String, String>, key: &str, profiles: &Profiles) -> Option<Part> {
    let written: Part = serde_json::from_str(labels.get(key)?).ok()?;
    let profile = profile_name(labels).and_then(|name| profiles.find(name));
    Some(profile.and_then(|known| known.profile.parts.iter().find(|p| p.name == written.name)).cloned().unwrap_or(written))
}

/// What every lane plays. Knobs follow a lane's track only where the
/// profile, known, says its parts take the same CCs each on its channel.
pub fn lane_maps(labels: &BTreeMap<String, String>, profiles: &Profiles) -> [LaneMap; LANES] {
    let cc_per_part = profile_name(labels).and_then(|name| profiles.find(name)).is_some_and(|known| known.profile.cc_per_part);
    std::array::from_fn(|lane| part(labels, lane, profiles).map_or(LaneMap::EMPTY, |part| LaneMap::of(&part, cc_per_part)))
}

/// The label that holds the track knob `slot` turns (from 0), where it has
/// one of its own: "slot 3 track".
pub fn slot_track_key(slot: usize) -> String {
    format!("slot {} track", slot + 1)
}

/// Whether a Kit's knobs can each turn a track of their own: its profile
/// takes the same CCs on each part's channel, or this computer doesn't
/// know it, and the patch says.
pub fn tracks_per_knob(labels: &BTreeMap<String, String>, profiles: &Profiles) -> bool {
    profile_name(labels).and_then(|name| profiles.find(name)).is_none_or(|known| known.profile.cc_per_part)
}

/// The track knob `slot` turns whatever the Track says, if it has one of
/// its own: its profile's part of that name, or else as the patch wrote it.
/// None where the profile doesn't take CCs a part a channel.
pub fn own_track(labels: &BTreeMap<String, String>, slot: usize, profiles: &Profiles) -> Option<Part> {
    let written: Part = serde_json::from_str(labels.get(&slot_track_key(slot))?).ok()?;
    if !tracks_per_knob(labels, profiles) {
        return None;
    }
    let profile = profile_name(labels).and_then(|name| profiles.find(name));
    Some(profile.and_then(|known| known.profile.parts.iter().find(|p| p.name == written.name)).cloned().unwrap_or(written)).filter(|part| part.channel.is_some())
}

/// A part's name, short enough to tag a knob with: "Track 3" is "T3",
/// "Part 4" "P4", and one without a number its first five letters.
pub fn short_name(name: &str) -> String {
    let name = name.trim();
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (words, number) = name.split_at(name.len() - digits);
    match words.trim().chars().next() {
        Some(first) if digits > 0 => format!("{first}{number}"),
        _ if digits > 0 => number.to_string(),
        _ => name.chars().take(5).collect(),
    }
}

/// The label that has a lane play `part`.
pub fn written_part(part: &Part) -> String {
    serde_json::to_string(part).expect("a part serializes")
}

/// The labels that have a Kit play `profile`: as [`choose`], and each lane
/// playing one of its first eight parts.
pub fn choose_kit(profile: &Profile) -> Vec<(String, String)> {
    let mut labels = choose_knobs(profile);
    labels.extend((0..LANES).map(|lane| (lane_key(lane), profile.parts.get(lane).map(written_part).unwrap_or_default())));
    labels
}

/// Whether a Kit can play `profile`: a kit, with its parts.
pub fn is_kit(profile: &Profile) -> bool {
    profile.kind == Kind::Kit && !profile.parts.is_empty()
}

/// The label that holds the part a Strike strikes (#484).
pub const PART: &str = "part";

/// The label that holds the parameter a CC Out sends (#484).
pub const PARAMETER: &str = "parameter";

/// The part a Strike strikes, if it has one: its profile's, where the
/// profile is known and has it, or else as the patch wrote it.
pub fn strike_part(labels: &BTreeMap<String, String>, profiles: &Profiles) -> Option<Part> {
    part_under(labels, PART, profiles)
}

/// What a Strike strikes: its part, or nothing (its own Note).
pub fn strike_map(labels: &BTreeMap<String, String>, profiles: &Profiles) -> LaneMap {
    strike_part(labels, profiles).map_or(LaneMap::EMPTY, |part| LaneMap::of(&part, false))
}

/// The parameter a CC Out sends, if it holds one, and where that came from.
pub fn cc_out_held(labels: &BTreeMap<String, String>, profiles: &Profiles) -> Option<Held> {
    held_under(labels, PARAMETER, profiles)
}

/// What a CC Out sends: its parameter, or nothing (a plain CC).
pub fn cc_out_map(labels: &BTreeMap<String, String>, profiles: &Profiles) -> SlotMap {
    cc_out_held(labels, profiles).map_or(SlotMap::EMPTY, |held| SlotMap::of(&held.parameter))
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

    /// A Digitakt Kit's labels, its lanes on its eight tracks, and knob
    /// `slot` holding `parameter` on the track `track` of its own, if any.
    fn digitakt(knobs: &[(usize, &str, Option<usize>)], profiles: &Profiles) -> BTreeMap<String, String> {
        let digitakt = &profiles.find("Digitakt").unwrap().profile;
        let mut chosen = labels(choose_kit(digitakt));
        for &(slot, name, track) in knobs {
            chosen.insert(slot_key(slot), written(digitakt.parameter(name).unwrap()));
            if let Some(track) = track {
                chosen.insert(slot_track_key(slot), written_part(&digitakt.parts[track]));
            }
        }
        chosen
    }

    #[test]
    fn a_kit_knob_on_a_track_of_its_own_goes_out_on_its_channel() {
        let profiles = profiles();
        let chosen = digitakt(&[(0, "Decay", Some(0)), (1, "Decay", Some(2)), (2, "Decay", None), (3, "Delay Time", Some(4))], &profiles);
        let maps = maps(&chosen, &profiles);
        assert_eq!(maps[0].channel, Some(1), "Track 1's Decay on channel 1");
        assert_eq!(maps[1].channel, Some(3), "Track 3's on channel 3");
        assert_eq!(maps[2].channel, None, "the Track's: the Kit says where");
        assert_eq!(maps[3].channel, Some(9), "the FX channel still wins");
        assert_eq!(own_track(&chosen, 1, &profiles).unwrap().name, "Track 3");
        assert!(own_track(&chosen, 2, &profiles).is_none());
        // Its steps stay with it: a selector on a track of its own
        let mut chosen = digitakt(&[(4, "Filter Type", Some(5))], &profiles);
        let filter = SlotMap::of(profiles.find("Digitakt").unwrap().profile.parameter("Filter Type").unwrap());
        let on_track = maps_of(&chosen, &profiles, 4);
        assert_eq!((on_track.channel, on_track.steps(), on_track.address), (Some(6), filter.steps(), filter.address));
        // Choosing the kit again puts every knob back on the Track's
        chosen.extend(choose_kit(&profiles.find("Digitakt").unwrap().profile));
        let chosen = labels(chosen.into_iter().collect());
        assert!((0..SLOTS).all(|slot| own_track(&chosen, slot, &profiles).is_none()));
    }

    fn maps_of(labels: &BTreeMap<String, String>, profiles: &Profiles, slot: usize) -> SlotMap {
        maps(labels, profiles)[slot]
    }

    #[test]
    fn a_knobs_own_track_plays_as_the_patch_wrote_it_and_only_where_tracks_take_ccs() {
        let profiles = profiles();
        let mut chosen = digitakt(&[(0, "Decay", Some(2))], &profiles);
        // A profile this computer doesn't know: the track the patch wrote
        chosen.insert(PROFILE.to_string(), "Someone's Groovebox".to_string());
        assert_eq!(maps_of(&chosen, &profiles, 0).channel, Some(3));
        // The SP-404MKII's pads are notes: its knobs stay on the Kit's channel
        let sp = &profiles.find("SP-404MKII").unwrap().profile;
        let mut chosen = labels(choose_kit(sp));
        chosen.insert(slot_track_key(0), written_part(&Part { name: "Pad 3".into(), note: Some(50), channel: Some(4) }));
        assert!(own_track(&chosen, 0, &profiles).is_none());
        let held = held(&chosen, 0, &profiles).unwrap().parameter;
        assert_eq!(maps_of(&chosen, &profiles, 0), SlotMap::of(&held), "as it was");
    }

    #[test]
    fn an_instrument_presses_its_profiles_buttons_by_name() {
        let profiles = profiles();
        let rc = &profiles.find("RC-505").unwrap().profile;
        let chosen = labels(choose(rc));
        let rec = button(&chosen, 0, &profiles).unwrap();
        assert_eq!((rec.name.as_str(), rec.cc), ("Target Trk Rec/Play", 14));
        assert_eq!(button_maps(&chosen, &profiles)[0].cc, Some(14));
        assert!((rc.buttons.len()..BUTTONS).all(|b| button(&chosen, b, &profiles).is_none()));
        // The patch wrote Rec/Play down on another CC than the profile's now
        let mut stale = rec.clone();
        stale.cc = 1;
        let mut patch = BTreeMap::from([(PROFILE.to_string(), "rc-505".to_string()), (button_key(0), written_button(&stale))]);
        assert_eq!(button(&patch, 0, &profiles).unwrap().cc, 14, "the profile's, corrected");
        // A profile this computer doesn't know: the patch's own
        patch.insert(PROFILE.to_string(), "Someone's Looper".to_string());
        assert_eq!(button(&patch, 0, &profiles).unwrap(), stale);
        // A Kit has no buttons
        let digitakt = &profiles.find("Digitakt").unwrap().profile;
        assert!(choose_kit(digitakt).iter().all(|(key, _)| !key.starts_with("button")));
    }

    #[test]
    fn a_tracks_short_name() {
        assert_eq!(short_name("Track 3"), "T3");
        assert_eq!(short_name("Part 12"), "P12");
        assert_eq!(short_name("Kick"), "Kick");
        assert_eq!(short_name("Snare Drum"), "Snare");
        assert_eq!(short_name("7"), "7");
    }

    /// What a knob holding `parameter` sends once `typed` is typed into its
    /// Value field (#466).
    fn typed(parameter: &Parameter, typed: &str) -> u16 {
        let map = SlotMap::of(parameter);
        map.value(map.travel(parameter.parse(typed).unwrap()))
    }

    #[test]
    fn a_typed_value_is_exactly_what_the_knob_sends() {
        let profiles = profiles();
        let freak = &profiles.find("MicroFreak").unwrap().profile;
        let cutoff = freak.parameter("Cutoff").unwrap();
        assert_eq!(cutoff.address, super::super::Address::Cc(23));
        assert_eq!(typed(cutoff, "100"), 100);
        let fm = &profiles.find("volca fm").unwrap().profile;
        assert_eq!(typed(fm.parameter("Arp Type").unwrap(), "Fall 2"), 65);
        // Every built-in parameter takes back what its knob reads out
        for known in profiles.all() {
            for parameter in &known.profile.parameters {
                let (lo, hi) = parameter.range;
                let values: Vec<u16> = match parameter.steps() {
                    steps if steps.is_empty() => vec![lo, lo + (hi - lo) / 3, parameter.centre(), hi],
                    steps => steps.into_iter().take(super::super::slot::MAX_STEPS).map(|(_, value)| value).collect(),
                };
                for value in values {
                    let text = parameter.text(value);
                    assert_eq!(typed(parameter, &text), value, "{} {}: {text}", known.profile.name, parameter.name);
                }
            }
        }
    }
}
