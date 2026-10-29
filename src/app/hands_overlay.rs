//! The Hands overlay (#431): a sketch of the controller along the foot of
//! the canvas, each encoder, fader and pad labelled with what it does right
//! now. This is the part that works out what that is, with no egui in it so
//! it can be tested; the strip itself is drawn by the app
//! (`synth_app::hands_overlay`).
//!
//! What a control does comes from, in turn:
//! - Soba being taught the controller: the controls taught so far, and the
//!   one it waits for.
//! - The hands following the selection (#429): the knob each encoder and
//!   fader holds.
//! - The patch's own mappings on the taught encoders' and faders' CCs.
//! - With nothing taught, the patch's CC mappings, in the order they were
//!   learned, on eight encoders and four faders drawn as a guess.
//!
//! A taught controller is drawn with as many encoders and faders as it
//! was taught (#485), and one being taught grows a control at a time.
//!
//! The pads are the patch's mappings to notes, footswitches and Scenes
//! pads, whatever else the hands are doing: Soba isn't taught pads, so
//! they're the mapped notes in order.

use crate::engine::NodeId as EngineNodeId;
use crate::persistence::{MidiControl, MidiEncoding, MidiMapping, MidiSource};
use super::hands::{Controller, Step, Taught};

/// How many pads the sketch has room for.
pub const PADS: usize = 8;

/// The encoders and faders drawn while nothing is taught: a guess, the
/// MiniLab's.
pub const GUESSED_ENCODERS: usize = 8;
pub const GUESSED_FADERS: usize = 4;

/// What a control on the controller sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hardware {
    pub source: MidiSource,
    /// The CC's number, or the note's.
    pub number: u8,
    /// 1-16, or 0 for any.
    pub channel: u8,
    /// An endless encoder, which sends steps.
    pub endless: bool,
}

impl Hardware {
    fn taught(taught: Taught) -> Self {
        Self { source: MidiSource::Cc, number: taught.cc, channel: taught.channel, endless: taught.encoding != MidiEncoding::Absolute }
    }

    fn of(mapping: &MidiMapping) -> Self {
        Self { source: mapping.source, number: mapping.cc_number, channel: mapping.channel, endless: mapping.encoding != MidiEncoding::Absolute }
    }

    /// Does `mapping` listen to it?
    fn turns(self, mapping: &MidiMapping) -> bool {
        match self.channel {
            0 => mapping.macro_slot.is_none() && mapping.source == self.source && mapping.cc_number == self.number,
            channel => mapping.hears(self.source, self.number, channel - 1),
        }
    }

    /// Does a mapping on `other` listen to it?
    fn turns_like(self, other: Hardware) -> bool {
        self.source == other.source && self.number == other.number && (other.channel == 0 || self.channel == 0 || self.channel == other.channel)
    }

    /// The same control as `other`: what it sends, on what channel.
    fn same(self, other: Hardware) -> bool {
        (self.source, self.number, self.channel) == (other.source, other.number, other.channel)
    }

    /// As a mapping's badge says it: "CC 74 · ch 1", "C2 · ch 10".
    pub fn text(self) -> String {
        let info = crate::graph::MidiMappingInfo {
            cc_number: self.number,
            channel: self.channel,
            macro_slot: None,
            source: self.source,
            control: MidiControl::Knob,
            encoding: MidiEncoding::Absolute,
            jump: false,
            invert: false,
        };
        info.text()
    }
}

/// One control on the sketch: what it sends, if that's known, and what it
/// does, if anything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Control {
    pub hardware: Option<Hardware>,
    /// The mapping it turns, the hands' or the patch's.
    pub does: Option<MidiMapping>,
    /// How many more of the patch's mappings it turns besides.
    pub also: usize,
    /// The Pad module it is (#480), by its engine node.
    pub pad: Option<EngineNodeId>,
}

impl Control {
    fn of(hardware: Hardware, mappings: Vec<&MidiMapping>) -> Self {
        let also = mappings.len().saturating_sub(1);
        Self { hardware: Some(hardware), does: mappings.first().map(|m| (*m).clone()), also, pad: None }
    }
}

/// A Pad module in the patch (#480), whose pad one of the sketch's is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadModule {
    pub node_id: EngineNodeId,
    pub note: u8,
    /// 1-16, or 0 for any.
    pub channel: u8,
}

/// Where the labels came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Soba is being taught the controller, and waits for this slot.
    Teaching(Option<usize>),
    /// The encoders and faders hold the selection's knobs.
    Following,
    /// The patch's mappings on the taught controls.
    Patch,
    /// Nothing taught: the patch's mappings, on a controller guessed at.
    Untaught,
}

/// The controller as the strip draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sketch {
    pub source: Source,
    /// Each encoder, then each fader.
    pub slots: Vec<Control>,
    /// How many of the slots are encoders: the rest are faders.
    pub encoders: usize,
    /// The mapped pads, at most [`PADS`].
    pub pads: Vec<Control>,
    /// How many of the patch's mapped controls aren't on the sketch.
    pub unshown: usize,
}

/// What the hands are doing, for [`sketch`].
#[derive(Debug, Clone, Copy, Default)]
pub struct HandsNow<'a> {
    /// The controller taught for the device open.
    pub controller: Option<&'a Controller>,
    /// While the hands follow the selection, the mappings that turn the
    /// knobs they hold (none, with nothing selected).
    pub holding: Option<&'a [MidiMapping]>,
    /// While Soba is taught, what it has been taught so far and the step
    /// it waits for.
    pub teaching: Option<(&'a Controller, Option<Step>)>,
    /// The patch's Pad modules, whose pads come first, lowest note first.
    pub pads: &'a [PadModule],
}

/// Does `mapping` belong on a pad: a note, a footswitch or a Scenes pad?
fn on_a_pad(mapping: &MidiMapping) -> bool {
    mapping.source == MidiSource::Note || mapping.control == MidiControl::Button || mapping.scene.is_some()
}

/// The slot a teaching step teaches, on a sketch of `encoders` encoders.
fn slot_of(step: Step, encoders: usize) -> usize {
    match step {
        Step::Encoder(n) => n,
        Step::Fader(n) => encoders + n,
    }
}

/// The patch's mappings with the same hardware gathered, in the order the
/// first of each was learned.
fn gathered<'a>(mappings: impl Iterator<Item = &'a MidiMapping>) -> Vec<(Hardware, Vec<&'a MidiMapping>)> {
    let mut gathered: Vec<(Hardware, Vec<&MidiMapping>)> = Vec::new();
    for mapping in mappings {
        let hardware = Hardware::of(mapping);
        match gathered.iter_mut().find(|(h, _)| h.same(hardware)) {
            Some((_, list)) => list.push(mapping),
            None => gathered.push((hardware, vec![mapping])),
        }
    }
    gathered
}

/// What each control on the controller does right now, from the hands and
/// the patch's `mappings`.
pub fn sketch(hands: HandsNow, mappings: &[MidiMapping]) -> Sketch {
    // A DAW's macros aren't on the controller
    let patch: Vec<&MidiMapping> = mappings.iter().filter(|m| m.macro_slot.is_none()).collect();
    let taught = hands.teaching.map(|(taught, _)| taught).or(hands.controller);
    // As many as were taught; while taught, those so far and the one waited for
    let (encoders, faders) = match (hands.teaching, taught) {
        (Some((so_far, step)), _) => (
            so_far.encoders.len() + usize::from(matches!(step, Some(Step::Encoder(_)))),
            so_far.faders.len() + usize::from(matches!(step, Some(Step::Fader(_)))),
        ),
        (None, Some(controller)) => (controller.encoders.len(), controller.faders.len()),
        (None, None) => (GUESSED_ENCODERS, GUESSED_FADERS),
    };
    let source = match (hands.teaching, hands.holding, taught) {
        (Some((_, step)), ..) => Source::Teaching(step.map(|step| slot_of(step, encoders))),
        (None, Some(_), Some(_)) => Source::Following,
        (None, _, Some(_)) => Source::Patch,
        (None, _, None) => Source::Untaught,
    };

    let mut slots = vec![Control::default(); encoders + faders];
    // The patch's mappings that are somewhere on the sketch, or paused
    let mut placed = vec![false; patch.len()];
    match taught {
        Some(controller) => {
            for (slot, taught) in controller.slots() {
                let hardware = Hardware::taught(taught);
                let on_it: Vec<usize> = (0..patch.len()).filter(|&i| hardware.turns(patch[i])).collect();
                for &i in &on_it {
                    placed[i] = true;
                }
                slots[slot] = match (source, hands.holding) {
                    (Source::Teaching(_), _) => Control { hardware: Some(hardware), ..Default::default() },
                    // The patch's own mappings on it pause meanwhile
                    (Source::Following, Some(holding)) => Control::of(hardware, holding.iter().filter(|m| hardware.turns(m)).collect()),
                    _ => Control::of(hardware, on_it.iter().map(|&i| patch[i]).collect()),
                };
            }
        }
        None => {
            // In the order they were learned. With endless encoders among
            // them the controller has encoders, so the knobs with ends are
            // more likely its faders: they go there first, and the rest on
            // whatever's left over
            let knobs = gathered(patch.iter().copied().filter(|m| m.source == MidiSource::Cc && !on_a_pad(m)));
            let mixed = knobs.iter().any(|(h, _)| h.endless) && knobs.iter().any(|(h, _)| !h.endless);
            let mut free = vec![true; slots.len()];
            let mut take = |faders: Option<bool>| {
                let slot = (0..free.len()).find(|&slot| free[slot] && faders.is_none_or(|f| (slot >= encoders) == f))?;
                free[slot] = false;
                Some(slot)
            };
            let mut over = Vec::new();
            let mut laid = Vec::new();
            for (hardware, list) in knobs {
                match take(Some(mixed && !hardware.endless)) {
                    Some(slot) => laid.push((slot, hardware, list)),
                    None => over.push((hardware, list)),
                }
            }
            for (hardware, list) in over {
                let Some(slot) = take(None) else { break };
                laid.push((slot, hardware, list));
            }
            for (slot, hardware, list) in laid {
                for (i, m) in patch.iter().enumerate() {
                    if hardware.same(Hardware::of(m)) {
                        placed[i] = true;
                    }
                }
                slots[slot] = Control::of(hardware, list);
            }
        }
    }

    // The pads: the Pad modules' first, lowest note first, each with any
    // mapping on its note; then the mapped notes, footswitches and Scenes
    // pads not on an encoder or fader, lowest first
    let mut modules = hands.pads.to_vec();
    modules.sort_by_key(|pad| (pad.note, pad.channel, pad.node_id));
    let mut pads: Vec<Control> = Vec::new();
    for module in modules.into_iter().take(PADS) {
        let hardware = Hardware { source: MidiSource::Note, number: module.note, channel: module.channel, endless: false };
        let on_it: Vec<&MidiMapping> = patch.iter().copied().filter(|m| hardware.turns(m)).collect();
        pads.push(Control { pad: Some(module.node_id), ..Control::of(hardware, on_it) });
    }
    let mut mapped = gathered(patch.iter().enumerate().filter(|(i, m)| !placed[*i] && on_a_pad(m)).map(|(_, m)| *m));
    mapped.retain(|(h, _)| !pads.iter().any(|p| p.hardware.is_some_and(|pad| pad.turns_like(*h))));
    mapped.sort_by_key(|(h, _)| (h.source != MidiSource::Note, h.number, h.channel));
    let room = PADS.saturating_sub(pads.len());
    pads.extend(mapped.into_iter().take(room).map(|(hardware, list)| Control::of(hardware, list)));
    for pad in &pads {
        for (i, m) in patch.iter().enumerate() {
            if pad.hardware.is_some_and(|h| h.same(Hardware::of(m)) || (pad.pad.is_some() && h.turns(m))) {
                placed[i] = true;
            }
        }
    }

    let unshown = gathered(patch.iter().enumerate().filter(|(i, _)| !placed[*i]).map(|(_, m)| *m)).len();
    Sketch { source, slots, encoders, pads, unshown }

}

/// A control's name as the strip says it: a Looper's "Pedal Rec" is its
/// "Rec".
pub fn short_name(param_name: &str) -> &str {
    param_name.strip_prefix("Pedal ").unwrap_or(param_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MiniLab's, which is also what's guessed with nothing taught.
    const ENCODERS: usize = GUESSED_ENCODERS;
    const FADERS: usize = GUESSED_FADERS;
    const SLOTS: usize = ENCODERS + FADERS;

    fn endless(cc: u8) -> Option<Taught> {
        Some(Taught { cc, channel: 1, encoding: MidiEncoding::Relative64 })
    }

    /// A MiniLab: encoders on CC 70-77, faders on CC 20-23, channel 1.
    fn minilab() -> Controller {
        let fader = |cc| Some(Taught { cc, channel: 1, encoding: MidiEncoding::Absolute });
        Controller { encoders: (70..78).map(endless).collect(), faders: (20..24).map(fader).collect() }
    }

    fn knob(cc: u8, node: u64, param: usize, name: &str) -> MidiMapping {
        MidiMapping::new(cc, 1, node, param, name, 0.0, 1.0)
    }

    fn pad(note: u8, node: u64, name: &str, scene: Option<u8>) -> MidiMapping {
        MidiMapping { source: MidiSource::Note, control: MidiControl::Button, scene, ..MidiMapping::new(note, 10, node, 0, name, 0.0, 1.0) }
    }

    fn named(control: &Control) -> Option<&str> {
        control.does.as_ref().map(|m| m.param_name.as_str())
    }

    #[test]
    fn the_patch_mappings_label_the_taught_controls_they_listen_to() {
        let controller = minilab();
        let patch = vec![knob(72, 1, 0, "Cutoff"), knob(21, 2, 3, "Level"), knob(99, 3, 0, "Drive")];
        let sketch = sketch(HandsNow { controller: Some(&controller), ..Default::default() }, &patch);
        assert_eq!(sketch.source, Source::Patch);
        assert_eq!(sketch.slots.len(), SLOTS);
        // Encoder 3 is CC 72; fader 2 is CC 21
        assert_eq!(named(&sketch.slots[2]), Some("Cutoff"));
        assert_eq!(named(&sketch.slots[ENCODERS + 1]), Some("Level"));
        assert!(sketch.slots[2].hardware.unwrap().endless);
        assert!(!sketch.slots[ENCODERS + 1].hardware.unwrap().endless);
        // The rest are taught but do nothing, and CC 99 isn't on the controller
        assert_eq!(sketch.slots.iter().filter(|c| c.does.is_none()).count(), SLOTS - 2);
        assert!(sketch.slots.iter().all(|c| c.hardware.is_some()));
        assert_eq!(sketch.unshown, 1);
        assert!(sketch.pads.is_empty());
    }

    #[test]
    fn the_hands_label_the_encoders_and_faders_while_they_follow() {
        let controller = minilab();
        let patch = vec![knob(70, 1, 0, "Cutoff"), knob(99, 3, 0, "Drive")];
        let holding = vec![knob(70, 7, 0, "Freq"), knob(71, 7, 1, "Detune"), knob(20, 7, 8, "Level")];
        let hands = HandsNow { controller: Some(&controller), holding: Some(&holding), ..Default::default() };
        let sketch = sketch(hands, &patch);
        assert_eq!(sketch.source, Source::Following);
        assert_eq!(named(&sketch.slots[0]), Some("Freq"), "the hands' knob, not the paused Cutoff");
        assert_eq!(sketch.slots[0].also, 0);
        assert_eq!(named(&sketch.slots[1]), Some("Detune"));
        assert_eq!(named(&sketch.slots[ENCODERS]), Some("Level"));
        assert_eq!(named(&sketch.slots[2]), None);
        // Cutoff is paused, not missing; Drive's CC isn't the controller's
        assert_eq!(sketch.unshown, 1);

        // With nothing selected they hold nothing, and the paused mappings stay paused
        let hands = HandsNow { controller: Some(&controller), holding: Some(&[]), ..Default::default() };
        let idle = super::sketch(hands, &patch);
        assert_eq!(idle.source, Source::Following);
        assert!(idle.slots.iter().all(|c| c.does.is_none()));
    }

    #[test]
    fn with_nothing_taught_the_mappings_fill_the_encoders_then_the_faders() {
        let mut patch: Vec<MidiMapping> = (0..14).map(|i| knob(30 + i, 1, i as usize, &format!("K{i}"))).collect();
        // The same CC on two knobs is one control; a pad goes on the pads
        patch.insert(1, knob(30, 2, 0, "Twin"));
        patch.push(pad(36, 5, "Scene A", Some(0)));
        let sketch = sketch(HandsNow::default(), &patch);
        assert_eq!(sketch.source, Source::Untaught);
        assert_eq!(named(&sketch.slots[0]), Some("K0"));
        assert_eq!(sketch.slots[0].also, 1);
        assert_eq!(named(&sketch.slots[1]), Some("K1"));
        assert_eq!(named(&sketch.slots[ENCODERS + FADERS - 1]), Some("K11"));
        assert_eq!(sketch.slots[ENCODERS].hardware.map(|h| h.text()), Some("CC 38 · ch 1".to_string()));
        assert_eq!(sketch.unshown, 2, "K12 and K13 don't fit");
        assert_eq!(sketch.pads.len(), 1);
        assert_eq!(named(&sketch.pads[0]), Some("Scene A"));
    }

    #[test]
    fn with_nothing_taught_knobs_with_ends_go_on_the_faders_beside_endless_encoders() {
        let encoder = |cc: u8, i: usize| MidiMapping { encoding: MidiEncoding::Relative64, ..knob(cc, i as u64, 0, "E") };
        let mut patch: Vec<MidiMapping> = (0..3).map(|i| knob(20 + i as u8, 1, i, "F")).collect();
        patch.extend((0..10).map(|i| encoder(70 + i as u8, 10 + i)));
        let sketch = sketch(HandsNow::default(), &patch);
        let number = |slot: usize| sketch.slots[slot].hardware.map(|h| h.number);
        assert_eq!(number(0), Some(70));
        assert_eq!(number(ENCODERS - 1), Some(77));
        assert_eq!(number(ENCODERS), Some(20), "the first knob with ends is fader 1");
        assert_eq!(number(ENCODERS + 2), Some(22));
        assert_eq!(number(ENCODERS + 3), Some(78), "an encoder past the eighth takes a fader left over");
        assert_eq!(sketch.unshown, 1);

        // All with ends: in the order learned, encoders first
        let plain: Vec<MidiMapping> = (0..3).map(|i| knob(20 + i as u8, 1, i, "K")).collect();
        assert_eq!(super::sketch(HandsNow::default(), &plain).slots[0].hardware.map(|h| h.number), Some(20));
    }

    #[test]
    fn the_pads_are_the_mapped_notes_footswitches_and_scenes_lowest_first() {
        let controller = minilab();
        let footswitch = MidiMapping { control: MidiControl::Button, ..knob(108, 4, 9, "Pedal Rec") };
        let mut patch = vec![pad(38, 5, "Scene B", Some(1)), footswitch, pad(36, 5, "Scene A", Some(0))];
        // A host macro is the DAW's, not the controller's
        patch.push(MidiMapping { macro_slot: Some(2), ..knob(0, 1, 1, "Res") });
        // Nine more pads than there's room for
        patch.extend((0..9).map(|i| pad(60 + i, 6, &format!("Note {i}"), None)));
        let sketch = sketch(HandsNow { controller: Some(&controller), ..Default::default() }, &patch);
        assert_eq!(sketch.pads.len(), PADS);
        assert_eq!(named(&sketch.pads[0]), Some("Scene A"));
        assert_eq!(named(&sketch.pads[1]), Some("Scene B"));
        assert!(sketch.pads[0].hardware.unwrap().text().ends_with(" · ch 10"));
        // Notes before CCs: the footswitch's CC 108 is past the eight
        assert!(sketch.pads.iter().all(|p| p.hardware.unwrap().source == MidiSource::Note));
        assert_eq!(sketch.unshown, 4, "three notes and the footswitch");
        assert_eq!(short_name("Pedal Rec"), "Rec");
        assert_eq!(short_name("Cutoff"), "Cutoff");
    }

    #[test]
    fn while_taught_the_controls_show_what_was_heard_and_which_comes_next() {
        let so_far = Controller { encoders: vec![endless(70), None], faders: Vec::new() };
        let controller = minilab();
        let hands = HandsNow { controller: Some(&controller), teaching: Some((&so_far, Some(Step::Encoder(2)))), ..Default::default() };
        let patch = vec![knob(70, 1, 0, "Cutoff")];
        let sketch = sketch(hands, &patch);
        assert_eq!(sketch.source, Source::Teaching(Some(2)));
        assert_eq!(sketch.slots[0].hardware.map(|h| h.number), Some(70));
        assert_eq!(sketch.slots[0].does, None, "no labels while it's taught");
        assert_eq!(sketch.slots[1].hardware, None, "skipped");
        // The controller grows as it's taught: the two so far and the one waited for
        assert_eq!((sketch.slots.len(), sketch.encoders), (3, 3));

        // On to the faders: all the encoders, and the fader waited for
        let so_far = Controller { encoders: vec![endless(70), endless(71)], faders: vec![endless(20)] };
        let hands = HandsNow { teaching: Some((&so_far, Some(Step::Fader(1)))), ..Default::default() };
        let sketch = super::sketch(hands, &patch);
        assert_eq!((sketch.slots.len(), sketch.encoders), (4, 2));
        assert_eq!(sketch.source, Source::Teaching(Some(3)));
        assert_eq!(slot_of(Step::Fader(1), 2), 3);
    }

    #[test]
    fn a_taught_controller_is_drawn_with_as_many_controls_as_it_has() {
        // A BeatStep: sixteen encoders, no faders
        let beatstep = Controller { encoders: (70..86).map(endless).collect(), faders: Vec::new() };
        let patch = vec![knob(85, 1, 0, "Cutoff")];
        let sketch = sketch(HandsNow { controller: Some(&beatstep), ..Default::default() }, &patch);
        assert_eq!((sketch.slots.len(), sketch.encoders), (16, 16));
        assert_eq!(named(&sketch.slots[15]), Some("Cutoff"));
        assert_eq!(sketch.unshown, 0);
    }

    #[test]
    fn the_pad_modules_come_first_lowest_note_first_with_any_mapping_on_their_notes() {
        // Seven Pads on 36-42, channel 10, added in no order
        let modules: Vec<PadModule> = [40u8, 36, 42, 37, 39, 38, 41].iter().map(|&note| PadModule { node_id: note as u64 + 100, note, channel: 10 }).collect();
        let patch = vec![pad(37, 4, "Pedal Rec", None), pad(50, 5, "Scene A", Some(0)), pad(51, 5, "Scene B", Some(1))];
        let hands = HandsNow { pads: &modules, ..Default::default() };
        let sketch = sketch(hands, &patch);
        assert_eq!(sketch.pads.len(), PADS);
        assert_eq!(sketch.pads[0].pad, Some(136), "the kick's Pad first");
        assert_eq!(sketch.pads[0].does, None);
        assert_eq!(named(&sketch.pads[1]), Some("Pedal Rec"), "a mapping on the pad's note");
        assert_eq!(sketch.pads[6].pad, Some(142));
        // One place left: Scene A, and Scene B doesn't fit
        assert_eq!(sketch.pads[7].pad, None);
        assert_eq!(named(&sketch.pads[7]), Some("Scene A"));
        assert_eq!(sketch.unshown, 1);
    }

    #[test]
    fn an_omni_mapping_listens_to_the_taught_control_on_any_channel() {
        let controller = minilab();
        let patch = vec![knob(73, 0, 0, "Mix")];
        let sketch = sketch(HandsNow { controller: Some(&controller), ..Default::default() }, &patch);
        assert_eq!(named(&sketch.slots[3]), Some("Mix"));
    }
}
