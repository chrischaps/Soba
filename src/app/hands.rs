//! Hands follow the selection (#429): the encoders and faders of a
//! controller Soba has been taught turn whichever module is selected, the
//! way a DAW's remote controls do. Click an oscillator and the eight
//! encoders are its first eight knobs; click a filter and they're the
//! filter's. The four faders take the next four.
//!
//! Soba is taught a controller once, from the MIDI In menu: turn encoder
//! 1, then 2, through 8, then move fader 1 through 4. What it heard is kept
//! in the app's storage under the device's name, not in the patch, since
//! it's the hardware's and goes with it from patch to patch. With several
//! devices open, the one whose control is turned first is the one taught
//! (#454), and the hands stay with it.
//!
//! While the hands follow, the patch's own mappings on the taught CCs
//! pause, and come back when they stop. The turning itself is
//! [`MidiMappings`]': a mapping is made for each knob the hands hold, so an
//! endless encoder steps and a fader picks its knob up exactly as a mapped
//! one does. Kept apart from the frame loop, with no egui in it, so it can
//! be tested.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::engine::device_names::same;
use crate::engine::NodeId as EngineNodeId;
use crate::persistence::{MidiEncoding, MidiInput, MidiMapping, MidiSource};
use super::midi_mappings::{MidiMappings, Standing};

/// How many encoders are taught, and turn the first knobs.
pub const ENCODERS: usize = 8;
/// How many faders are taught, and turn the knobs after.
pub const FADERS: usize = 4;
/// Every control the hands have, encoders first.
pub const SLOTS: usize = ENCODERS + FADERS;

/// How many values a control sends while it's taught before it's taken:
/// enough for an endless encoder to tell itself from a knob with ends.
const CLICKS: usize = 4;

/// The storage key listing the controllers taught, a name a line.
const TAUGHT_KEY: &str = "taught_controllers";
/// The storage key of whether the hands follow the selection.
const FOLLOW_KEY: &str = "hands_follow";

/// The storage key a controller's controls are kept under.
fn controller_key(name: &str) -> String {
    format!("controller:{name}")
}

/// One control taught: the CC it sends, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taught {
    pub cc: u8,
    /// 1-16.
    pub channel: u8,
    #[serde(default)]
    pub encoding: MidiEncoding,
}

impl Taught {
    /// Does it send `cc` on `channel` (0-15)?
    fn hears(self, cc: u8, channel: u8) -> bool {
        self.cc == cc && self.channel == channel + 1
    }

    /// As the status bar says it: "CC 74 · ch 1, endless".
    pub fn text(self) -> String {
        let endless = if self.encoding == MidiEncoding::Absolute { "" } else { ", endless" };
        format!("CC {} · ch {}{endless}", self.cc, self.channel)
    }
}

/// A controller as Soba was taught it: each encoder and fader, or `None`
/// for one skipped (it has fewer).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controller {
    pub encoders: Vec<Option<Taught>>,
    pub faders: Vec<Option<Taught>>,
}

impl Controller {
    /// The control in `slot`: encoders 0-7, faders 8-11.
    pub fn slot(&self, slot: usize) -> Option<Taught> {
        match slot.checked_sub(ENCODERS) {
            None => self.encoders.get(slot).copied().flatten(),
            Some(fader) => self.faders.get(fader).copied().flatten(),
        }
    }

    fn set(&mut self, slot: usize, taught: Option<Taught>) {
        let (list, at) = match slot.checked_sub(ENCODERS) {
            None => (&mut self.encoders, slot),
            Some(fader) => (&mut self.faders, fader),
        };
        if list.len() <= at {
            list.resize(at + 1, None);
        }
        list[at] = taught;
    }

    /// Each control taught, with its slot.
    pub fn slots(&self) -> impl Iterator<Item = (usize, Taught)> + '_ {
        (0..SLOTS).filter_map(|slot| Some((slot, self.slot(slot)?)))
    }

    /// The slot of the control that sends `cc` on `channel` (0-15).
    pub fn hears(&self, cc: u8, channel: u8) -> Option<usize> {
        self.slots().find(|(_, t)| t.hears(cc, channel)).map(|(slot, _)| slot)
    }

    pub fn is_empty(&self) -> bool {
        self.slots().next().is_none()
    }

    /// What it has: "8 encoders and 4 faders".
    pub fn summary(&self) -> String {
        let count = |range: std::ops::Range<usize>| range.filter(|&slot| self.slot(slot).is_some()).count();
        let (encoders, faders) = (count(0..ENCODERS), count(ENCODERS..SLOTS));
        let plural = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        match (encoders, faders) {
            (0, 0) => "nothing".to_string(),
            (e, 0) => plural(e, "encoder"),
            (0, f) => plural(f, "fader"),
            (e, f) => format!("{} and {}", plural(e, "encoder"), plural(f, "fader")),
        }
    }
}

/// What a slot's badge says: 1-8 for the encoders, F1-F4 for the faders.
pub fn slot_label(slot: usize) -> String {
    match slot.checked_sub(ENCODERS) {
        None => (slot + 1).to_string(),
        Some(fader) => format!("F{}", fader + 1),
    }
}

/// One step of teaching: the control to move next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The nth encoder, from 0.
    Encoder(usize),
    /// The nth fader, from 0.
    Fader(usize),
}

impl Step {
    fn of(slot: usize) -> Option<Self> {
        match slot.checked_sub(ENCODERS) {
            None => Some(Step::Encoder(slot)),
            Some(fader) if fader < FADERS => Some(Step::Fader(fader)),
            Some(_) => None,
        }
    }

    /// What it's called: "Encoder 3", "Fader 1".
    pub fn name(self) -> String {
        match self {
            Step::Encoder(n) => format!("Encoder {}", n + 1),
            Step::Fader(n) => format!("Fader {}", n + 1),
        }
    }

    /// What the player is asked to do.
    pub fn ask(self) -> String {
        match self {
            Step::Encoder(n) => format!("turn encoder {} a few clicks", n + 1),
            Step::Fader(n) => format!("move fader {}", n + 1),
        }
    }
}

/// A control heard while it's taught, not yet taken.
#[derive(Debug, Clone)]
struct Listening {
    /// The device it's on.
    device: String,
    cc: u8,
    channel: u8,
    values: Vec<u8>,
}

/// Soba being taught a controller, a control at a time.
#[derive(Debug, Clone)]
pub struct Teaching {
    /// The device's name, to keep it under: `None` until a control is
    /// taken, when it's that control's device (#454).
    device: Option<String>,
    /// The slot being taught.
    slot: usize,
    taught: Controller,
    listening: Option<Listening>,
    /// The step just taken, and what it heard, for the prompt to say.
    last: Option<(Step, Option<Taught>)>,
}

impl Teaching {
    /// Teaching the device called `device`.
    pub fn new(device: impl Into<String>) -> Self {
        Self { device: Some(device.into()), ..Self::any() }
    }

    /// Teaching whichever device's control is turned first (#454).
    pub fn any() -> Self {
        Self { device: None, slot: 0, taught: Controller::default(), listening: None, last: None }
    }

    /// The device taught, once a control of it has been taken.
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    /// The control to move next, or `None` once they've all been taught.
    pub fn step(&self) -> Option<Step> {
        Step::of(self.slot)
    }

    /// Hears a CC (channel 0-15) from the device being taught.
    pub fn hear(&mut self, cc: u8, channel: u8, value: u8) -> bool {
        let device = self.device.clone().unwrap_or_default();
        self.hear_from(&device, cc, channel, value)
    }

    /// Hears a CC (channel 0-15) from the device called `device`. A
    /// control already taught is still being turned or let go of, and
    /// isn't heard again. Another control than the one being heard takes
    /// its place: the player moved the one they meant. Until one is taken,
    /// every device's controls are heard, and the first taken names the
    /// device taught; from then on, other devices' aren't (#454). Returns
    /// whether the step was taken.
    pub fn hear_from(&mut self, device: &str, cc: u8, channel: u8, value: u8) -> bool {
        let Some(step) = self.step() else { return false };
        if self.device.as_deref().is_some_and(|taught| !same(taught, device)) {
            return false;
        }
        if self.taught.hears(cc, channel).is_some() {
            return false;
        }
        let listening = match self.listening.as_mut() {
            Some(l) if l.cc == cc && l.channel == channel && l.device == device => l,
            _ => self.listening.insert(Listening { device: device.to_string(), cc, channel, values: Vec::new() }),
        };
        listening.values.push(value);
        if listening.values.len() < CLICKS {
            return false;
        }
        if self.device.is_none() {
            self.device = Some(listening.device.clone());
        }
        // A fader has ends, whatever it sends
        let encoding = match step {
            Step::Encoder(_) => MidiEncoding::guess(&listening.values),
            Step::Fader(_) => MidiEncoding::Absolute,
        };
        let taught = Taught { cc, channel: channel + 1, encoding };
        self.taught.set(self.slot, Some(taught));
        self.next(step, Some(taught));
        true
    }

    /// Skips the control being taught: the controller hasn't one.
    pub fn skip(&mut self) {
        if let Some(step) = self.step() {
            self.taught.set(self.slot, None);
            self.next(step, None);
        }
    }

    fn next(&mut self, step: Step, taught: Option<Taught>) {
        self.slot += 1;
        self.listening = None;
        self.last = Some((step, taught));
    }

    /// Is it a control being heard, not yet taken?
    pub fn hearing(&self) -> Option<(u8, u8)> {
        self.listening.as_ref().map(|l| (l.cc, l.channel + 1))
    }

    /// What it has been taught so far.
    pub fn taught(&self) -> &Controller {
        &self.taught
    }

    /// What the status bar asks for: what was just heard, and what next.
    pub fn prompt(&self) -> String {
        let last = match self.last {
            Some((step, Some(taught))) => format!("{} is {} · ", step.name(), taught.text()),
            Some((step, None)) => format!("{} skipped · ", step.name()),
            None => String::new(),
        };
        let Some(step) = self.step() else { return format!("{last}done") };
        // Before a device is chosen, which one is moving
        let on = match (&self.device, &self.listening) {
            (None, Some(l)) if !l.device.is_empty() => format!(" on {}", l.device),
            _ => String::new(),
        };
        let hearing = self.hearing().map(|(cc, channel)| format!(" (hearing CC {cc} · ch {channel}{on})")).unwrap_or_default();
        let mut ask = step.ask();
        if last.is_empty() {
            ask = format!("{}{}", ask[..1].to_uppercase(), &ask[1..]);
        }
        format!("{last}{ask}{hearing}…")
    }
}

/// A knob the hands could hold: one of the selected module's, or of a
/// selected group's face.
#[derive(Debug, Clone, PartialEq)]
pub struct HandTarget {
    pub node_id: EngineNodeId,
    pub param_index: usize,
    pub param_name: String,
    pub min_value: f32,
    pub max_value: f32,
}

/// The hands, and the controllers they've been taught.
#[derive(Debug, Default)]
pub struct Hands {
    /// The player wants them to follow the selection.
    pub on: bool,
    /// Something else wants the controller for now (Map Mode, MIDI Learn):
    /// they let go, and the patch's mappings come back meanwhile.
    pub held_back: bool,
    /// Each controller taught, by its device's name.
    controllers: HashMap<String, Controller>,
    /// The name of the device open ("" for none).
    device: String,
    teaching: Option<Teaching>,
    /// What teaching ended with, for the status bar to say once.
    finished: Option<String>,
    /// The slot, node and parameter of each knob held.
    holding: Vec<(usize, EngineNodeId, usize)>,
    /// A mapping for each knob held, which turns it.
    turner: MidiMappings,
}

impl Hands {
    /// The device open, whose controller the hands are: its name, or none.
    pub fn set_device(&mut self, name: Option<&str>) {
        let name = name.unwrap_or_default();
        if self.device != name {
            self.device = name.to_string();
            self.let_go();
        }
    }

    /// The name of the device whose controller the hands are ("" for none).
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Puts the hands on one of the devices `open` (#435): the one being
    /// taught; else the one they're on, if it's open and taught (#454);
    /// else the first that has been taught, or else the first, to teach.
    /// While Soba is taught whichever device moves first, with several
    /// open, it's none of them yet.
    pub fn follow_devices(&mut self, open: &[String]) {
        let chosen = match self.teaching.as_ref().map(Teaching::device) {
            Some(Some(taught)) => Some(taught.to_string()),
            Some(None) if open.len() > 1 => None,
            _ => {
                let current = open.iter().find(|name| same(name, &self.device) && self.taught_as(name).is_some());
                let taught = || open.iter().find(|name| self.taught_as(name).is_some());
                current.or_else(taught).or(open.first()).cloned()
            }
        };
        self.set_device(chosen.as_deref());
    }

    /// Are the CCs of the device called `device` (`None` for Soba's own)
    /// for the hands? Those of the device they're on, and while Soba is
    /// taught whichever device moves first, every device's (#454).
    pub fn hears_device(&self, device: Option<&str>) -> bool {
        let any = self.teaching.as_ref().is_some_and(|t| t.device.is_none());
        device.is_none_or(|device| any || same(device, &self.device))
    }

    /// The controller of the device open, if it has been taught.
    pub fn controller(&self) -> Option<&Controller> {
        self.taught_as(&self.device)
    }

    /// The controller taught as the device called `name`, or as it was
    /// called before the system renumbered it (#436).
    fn taught_as(&self, name: &str) -> Option<&Controller> {
        let exact = self.controllers.get(name);
        let near = || self.controllers.iter().find(|(taught, _)| crate::engine::device_names::same(taught, name)).map(|(_, c)| c);
        exact.or_else(near).filter(|c| !c.is_empty())
    }

    /// Teaches `controller` as the open device's, as if it had been
    /// taught by hand.
    pub fn know(&mut self, name: &str, controller: Controller) {
        self.controllers.insert(name.to_string(), controller);
        self.let_go();
    }

    /// Do the hands follow the selection now? On, taught, and nothing else
    /// wanting the controller.
    pub fn active(&self) -> bool {
        self.on && !self.held_back && self.teaching.is_none() && self.controller().is_some()
    }

    /// Takes hold of `targets`, the selection's knobs in the order its face
    /// shows them: the encoders the first eight, the faders the next four.
    /// Returns whether what they hold changed, which is when the badges
    /// fade in again. A fader picks its new knob up afresh.
    pub fn follow(&mut self, targets: &[HandTarget]) -> bool {
        let controller = self.controller().filter(|_| self.active()).cloned().unwrap_or_default();
        let held: Vec<(usize, Taught, &HandTarget)> =
            targets.iter().take(SLOTS).enumerate().filter_map(|(slot, target)| Some((slot, controller.slot(slot)?, target))).collect();
        let holding: Vec<_> = held.iter().map(|(slot, _, t)| (*slot, t.node_id, t.param_index)).collect();
        if holding == self.holding {
            return false;
        }
        self.turner.clear();
        for (_, taught, t) in held {
            let mapping = MidiMapping::new(taught.cc, taught.channel, t.node_id, t.param_index, t.param_name.clone(), t.min_value, t.max_value);
            self.turner.learn(MidiMapping { encoding: taught.encoding, ..mapping });
        }
        self.holding = holding;
        true
    }

    /// Lets go of what they hold, to take hold afresh.
    fn let_go(&mut self) {
        self.holding.clear();
        self.turner.clear();
    }

    /// Hears a CC (channel 0-15): taught, while being taught; or turning
    /// the knob its control holds into `turns`, while the hands follow.
    /// Returns whether it was theirs, which the patch's mappings then
    /// don't hear: a taught control's mappings pause while the hands
    /// follow, even with nothing selected.
    pub fn hear(
        &mut self,
        cc: u8,
        channel: u8,
        value: u8,
        current: &impl Fn(u64, usize) -> Option<Standing>,
        turns: &mut Vec<(u64, usize, f32)>,
    ) -> bool {
        self.hear_from(None, cc, channel, value, current, turns)
    }

    /// [`hear`](Self::hear)s a CC from the device called `device`, or
    /// Soba's own (`None`, heard as the device they're on). The first
    /// control taught from a device makes that device the one taught, and
    /// the one the hands are on (#454).
    pub fn hear_from(
        &mut self,
        device: Option<&str>,
        cc: u8,
        channel: u8,
        value: u8,
        current: &impl Fn(u64, usize) -> Option<Standing>,
        turns: &mut Vec<(u64, usize, f32)>,
    ) -> bool {
        if let Some(teaching) = self.teaching.as_mut() {
            let taken = teaching.hear_from(device.unwrap_or(&self.device), cc, channel, value);
            if let Some(taught) = teaching.device().filter(|&taught| taught != self.device) {
                self.device = taught.to_string();
            }
            if taken && teaching.step().is_none() {
                self.finish_teaching();
            }
            return true;
        }
        if !self.active() || self.controller().and_then(|c| c.hears(cc, channel)).is_none() {
            return false;
        }
        let held = self.turner.iter().position(|m| m.hears(MidiSource::Cc, cc, channel));
        if let Some(index) = held {
            let (node_id, param_index) = {
                let m = &self.turner.as_slice()[index];
                (m.node_id, m.param_index)
            };
            if let Some(value) = self.turner.turn(index, MidiInput::Cc(value), current(node_id, param_index)) {
                turns.push((node_id, param_index, value));
            }
        }
        true
    }

    /// The mappings that turn the knobs held, for where they stand.
    pub fn mappings(&self) -> &MidiMappings {
        &self.turner
    }

    /// Each knob held, and the slot that holds it, for its badge.
    pub fn badges(&self) -> HashMap<(EngineNodeId, usize), u8> {
        self.holding.iter().map(|&(slot, node, param)| ((node, param), slot as u8)).collect()
    }

    /// The patch's knobs whose mappings are paused while the hands follow:
    /// mapped to a taught control.
    pub fn paused(&self, patch: &MidiMappings) -> HashSet<(EngineNodeId, usize)> {
        let Some(controller) = self.controller().filter(|_| self.active()) else { return HashSet::new() };
        patch
            .iter()
            .filter(|m| m.scene.is_none() && m.source == MidiSource::Cc)
            .filter(|m| controller.slots().any(|(_, t)| m.hears(MidiSource::Cc, t.cc, t.channel - 1)))
            .map(|m| (m.node_id, m.param_index))
            .collect()
    }

    /// Starts teaching a controller, from its first encoder: whichever
    /// device's encoder is turned first (#454).
    pub fn start_teaching(&mut self) {
        self.teaching = Some(Teaching::any());
        self.finished = None;
        self.let_go();
    }

    /// The teaching under way, if one is.
    pub fn teaching(&self) -> Option<&Teaching> {
        self.teaching.as_ref()
    }

    /// Skips the control being taught; past the last, it's done.
    pub fn skip(&mut self) {
        if let Some(teaching) = self.teaching.as_mut() {
            teaching.skip();
            if teaching.step().is_none() {
                self.finish_teaching();
            }
        }
    }

    /// Ends teaching, keeping what was taught: the controller has no more
    /// controls. Nothing taught keeps the one taught before. The hands
    /// follow from then on.
    pub fn finish_teaching(&mut self) {
        let Some(teaching) = self.teaching.take() else { return };
        self.finished = Some(if teaching.taught.is_empty() {
            "Nothing taught: the controller is as it was".to_string()
        } else {
            let summary = teaching.taught.summary();
            self.controllers.insert(teaching.device.unwrap_or_else(|| self.device.clone()), teaching.taught);
            self.on = true;
            format!("Taught {summary}. The encoders follow the selection")
        });
        self.let_go();
    }

    /// Gives teaching up, keeping the controller as it was.
    pub fn cancel_teaching(&mut self) {
        if self.teaching.take().is_some() {
            self.finished = Some("Teaching cancelled".to_string());
        }
    }

    /// What teaching ended with, once.
    pub fn take_finished(&mut self) -> Option<String> {
        self.finished.take()
    }

    /// Where the hardware stands for each knob still waiting to be picked up.
    pub fn waiting(&self) -> HashMap<(EngineNodeId, usize), f32> {
        self.turner.waiting()
    }

    /// The controllers taught, and whether the hands follow, from a past
    /// session.
    pub fn load(&mut self, storage: &dyn eframe::Storage) {
        self.on = storage.get_string(FOLLOW_KEY).is_some_and(|v| v == "on");
        let names = storage.get_string(TAUGHT_KEY).unwrap_or_default();
        for name in names.lines() {
            let controller = storage.get_string(&controller_key(name)).and_then(|json| serde_json::from_str::<Controller>(&json).ok());
            if let Some(controller) = controller {
                self.controllers.insert(name.to_string(), controller);
            }
        }
        self.let_go();
    }

    /// Keeps the controllers taught, each under its device's name, and
    /// whether the hands follow.
    pub fn store(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(FOLLOW_KEY, if self.on { "on" } else { "off" }.to_string());
        let mut names: Vec<&String> = self.controllers.keys().collect();
        names.sort();
        storage.set_string(TAUGHT_KEY, names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join("\n"));
        for (name, controller) in &self.controllers {
            if let Ok(json) = serde_json::to_string(controller) {
                storage.set_string(&controller_key(name), json);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::midi_mappings::ENCODER_STEP;

    /// Turns encoder `n` (CC 70 + n, channel 1) four clicks up, or
    /// moves fader `n` (CC 20 + n) a little.
    fn teach_encoder(teaching: &mut Teaching, n: u8, values: [u8; 4]) {
        for value in values {
            teaching.hear(70 + n, 0, value);
        }
    }

    #[test]
    fn teaching_takes_each_control_in_turn_and_guesses_the_encoders() {
        let mut teaching = Teaching::new("MiniLab3");
        assert_eq!(teaching.step(), Some(Step::Encoder(0)));
        assert!(teaching.prompt().starts_with("Turn encoder 1"));

        // Encoder 1 is endless (64 ± clicks), encoder 2 absolute
        teach_encoder(&mut teaching, 0, [65; 4]);
        assert_eq!(teaching.step(), Some(Step::Encoder(1)));
        assert!(teaching.prompt().starts_with("Encoder 1 is CC 70 · ch 1, endless · turn encoder 2"), "{}", teaching.prompt());
        // Encoder 1 still turning as the hand leaves it isn't encoder 2
        teaching.hear(70, 0, 65);
        assert_eq!(teaching.hearing(), None);
        teach_encoder(&mut teaching, 1, [40, 41, 42, 43]);

        let taught = teaching.taught();
        assert_eq!(taught.slot(0), Some(Taught { cc: 70, channel: 1, encoding: MidiEncoding::Relative64 }));
        assert_eq!(taught.slot(1), Some(Taught { cc: 71, channel: 1, encoding: MidiEncoding::Absolute }));
        assert_eq!(teaching.step(), Some(Step::Encoder(2)));
    }

    #[test]
    fn a_briskly_turned_encoder_is_still_taken_for_endless() {
        let mut teaching = Teaching::new("pads");
        teach_encoder(&mut teaching, 0, [65, 67, 70, 66]);
        assert_eq!(teaching.taught().slot(0).map(|t| t.encoding), Some(MidiEncoding::Relative64));
    }

    #[test]
    fn a_control_moved_by_mistake_gives_way_to_the_one_meant() {
        let mut teaching = Teaching::new("pads");
        teaching.hear(1, 0, 10);
        teaching.hear(1, 0, 11);
        assert_eq!(teaching.hearing(), Some((1, 1)));
        teach_encoder(&mut teaching, 0, [1, 1, 1, 1]);
        assert_eq!(teaching.taught().slot(0).map(|t| (t.cc, t.encoding)), Some((70, MidiEncoding::TwosComplement)));
    }

    #[test]
    fn steps_can_be_skipped_and_teaching_finished_early() {
        let mut hands = Hands::default();
        hands.set_device(Some("Little Keys"));
        hands.start_teaching();
        // Four encoders, then the rest skipped, then one fader
        for n in 0..4 {
            for value in [64, 65, 66, 67] {
                hands.hear(70 + n, 0, value, &|_, _| None, &mut Vec::new());
            }
        }
        for _ in 4..ENCODERS {
            hands.skip();
        }
        assert_eq!(hands.teaching().and_then(Teaching::step), Some(Step::Fader(0)));
        assert!(hands.teaching().unwrap().prompt().starts_with("Encoder 8 skipped · move fader 1"));
        for value in [0, 9, 20, 31] {
            hands.hear(20, 1, value, &|_, _| None, &mut Vec::new());
        }
        assert!(hands.teaching().is_some());
        hands.finish_teaching();

        let controller = hands.controller().expect("taught");
        assert_eq!(controller.summary(), "4 encoders and 1 fader");
        assert_eq!(controller.slot(4), None);
        assert_eq!(controller.slot(ENCODERS), Some(Taught { cc: 20, channel: 2, encoding: MidiEncoding::Absolute }));
        assert!(hands.on, "the hands follow once taught");
        assert!(hands.take_finished().unwrap().starts_with("Taught 4 encoders and 1 fader"));

        // Skipping past the last fader is done too; nothing taught keeps the old one
        hands.start_teaching();
        for _ in 0..SLOTS {
            hands.skip();
        }
        assert!(hands.teaching().is_none());
        assert_eq!(hands.controller().map(Controller::summary).as_deref(), Some("4 encoders and 1 fader"));
        assert!(hands.take_finished().unwrap().starts_with("Nothing taught"));
    }

    /// A controller of two endless encoders (CC 70, 71) and one fader (CC 20), on channel 1.
    fn minilab() -> Hands {
        let mut hands = Hands::default();
        hands.set_device(Some("MiniLab3"));
        let endless = |cc| Some(Taught { cc, channel: 1, encoding: MidiEncoding::Relative64 });
        let controller = Controller { encoders: vec![endless(70), endless(71)], faders: vec![Some(Taught { cc: 20, channel: 1, encoding: MidiEncoding::Absolute })] };
        hands.know("MiniLab3", controller);
        hands.on = true;
        hands
    }

    fn knobs(node_id: u64, count: usize) -> Vec<HandTarget> {
        (0..count).map(|i| HandTarget { node_id, param_index: i, param_name: format!("K{i}"), min_value: 0.0, max_value: 1.0 }).collect()
    }

    #[test]
    fn the_encoders_hold_the_first_knobs_and_the_faders_the_next() {
        let mut hands = minilab();
        assert!(hands.follow(&knobs(7, 10)));
        let badges = hands.badges();
        assert_eq!(badges.get(&(7, 0)), Some(&0));
        assert_eq!(badges.get(&(7, 1)), Some(&1));
        // Encoders 3-8 weren't taught, so knobs 3-8 go free; fader 1 is knob 9
        assert_eq!(badges.get(&(7, 8)), Some(&(ENCODERS as u8)));
        assert_eq!(badges.len(), 3);
        assert_eq!(slot_label(8), "F1");
        assert!(!hands.follow(&knobs(7, 10)), "the same selection holds on");
        assert!(hands.follow(&knobs(9, 2)), "another module takes the hands");
        assert!(hands.follow(&[]));
        assert!(hands.badges().is_empty());
    }

    #[test]
    fn encoders_step_and_faders_pick_up() {
        let mut hands = minilab();
        hands.follow(&knobs(7, 10));
        let knob = std::cell::Cell::new(0.5_f32);
        let current = |_: u64, _: usize| Some(Standing { value: knob.get(), logarithmic: false, stepped: false });
        let mut turn = |cc, value| {
            let mut turns = Vec::new();
            assert!(hands.hear(cc, 0, value, &current, &mut turns));
            if let Some(&(_, _, v)) = turns.last() {
                knob.set(v);
            }
            turns
        };
        // Encoder 1: two clicks up
        turn(70, 65);
        let turns = turn(70, 65);
        assert_eq!(turns[0].1, 0);
        assert!((knob.get() - (0.5 + 2.0 * ENCODER_STEP)).abs() < 1e-5);

        // The fader waits to pick knob 9 up, then follows
        knob.set(0.5);
        assert!(turn(20, 10).is_empty());
        assert!(turn(20, 40).is_empty());
        let turns = turn(20, 90);
        assert_eq!((turns[0].0, turns[0].1), (7, 8));
        assert!((knob.get() - 90.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn the_patch_mappings_on_taught_controls_pause_while_the_hands_follow() {
        let mut hands = minilab();
        let mut patch = MidiMappings::default();
        patch.learn(MidiMapping::new(70, 1, 3, 0, "Cutoff", 20.0, 20000.0));
        patch.learn(MidiMapping::new(74, 1, 3, 1, "Res", 0.0, 1.0));
        hands.follow(&[]);

        // With nothing selected, CC 70 is still the hands', and CC 74 the patch's
        let mut turns = Vec::new();
        assert!(hands.hear(70, 0, 65, &|_, _| None, &mut turns));
        assert!(turns.is_empty());
        assert!(!hands.hear(74, 0, 65, &|_, _| None, &mut turns));
        assert_eq!(hands.paused(&patch), HashSet::from([(3, 0)]));

        // Off, or held back by Map Mode, the patch has it back
        hands.held_back = true;
        assert!(!hands.hear(70, 0, 65, &|_, _| None, &mut turns));
        assert!(hands.paused(&patch).is_empty());
        hands.held_back = false;
        hands.on = false;
        assert!(!hands.hear(70, 0, 65, &|_, _| None, &mut turns));
        assert!(hands.paused(&patch).is_empty());
    }

    #[test]
    fn another_device_has_its_own_controller() {
        let mut hands = minilab();
        hands.follow(&knobs(7, 2));
        hands.set_device(Some("Launchkey"));
        assert!(!hands.active());
        hands.follow(&knobs(7, 2));
        assert!(hands.badges().is_empty(), "it lets go");
        hands.set_device(Some("MiniLab3"));
        assert!(hands.follow(&knobs(7, 2)), "and takes hold again when it's back");
    }

    /// With a taught MiniLab and a Fantom open, teaching takes whichever
    /// device's encoder is turned first, and keeps the controller under
    /// its name (#454).
    #[test]
    fn teaching_with_two_devices_open_takes_the_one_turned_first() {
        let open = ["MiniLab3".to_string(), "Fantom-06".to_string()];
        fn turn(hands: &mut Hands, device: &str, cc: u8, value: u8) {
            hands.hear_from(Some(device), cc, 0, value, &|_, _| None, &mut Vec::new());
        }
        let mut hands = minilab();
        hands.follow_devices(&open);
        assert_eq!(hands.device(), "MiniLab3", "the taught one");
        let minilab = hands.controller().cloned();

        hands.start_teaching();
        hands.follow_devices(&open);
        assert_eq!(hands.device(), "", "none of them yet");
        assert!(hands.hears_device(Some("MiniLab3")) && hands.hears_device(Some("Fantom-06")));
        // A nudge of the MiniLab gives way to the Fantom's encoder, turned
        turn(&mut hands, "MiniLab3", 70, 65);
        turn(&mut hands, "MiniLab3", 70, 65);
        assert!(hands.teaching().unwrap().prompt().contains("(hearing CC 70 · ch 1 on MiniLab3)"));
        for value in [1, 1, 1, 1] {
            turn(&mut hands, "Fantom-06", 16, value);
        }
        assert_eq!(hands.teaching().and_then(Teaching::device), Some("Fantom-06"));
        assert_eq!(hands.device(), "Fantom-06", "the hands are on the device taught");
        hands.follow_devices(&open);
        assert_eq!(hands.device(), "Fantom-06", "and stay there while it's taught");
        // The MiniLab's controls are its own again
        assert!(!hands.hears_device(Some("MiniLab3")));
        for value in [65; 4] {
            turn(&mut hands, "MiniLab3", 71, value);
        }
        assert_eq!(hands.teaching().unwrap().step(), Some(Step::Encoder(1)), "the MiniLab teaches nothing now");
        for value in [1; 4] {
            turn(&mut hands, "Fantom-06", 17, value);
        }
        hands.finish_teaching();

        let fantom = hands.controller().expect("taught");
        assert_eq!(fantom.summary(), "2 encoders");
        assert_eq!(fantom.slot(0).map(|t| t.cc), Some(16));
        // The hands stay with the Fantom, and the MiniLab keeps what it was taught
        hands.follow_devices(&open);
        assert_eq!(hands.device(), "Fantom-06");
        hands.set_device(Some("MiniLab3"));
        assert_eq!(hands.controller().cloned(), minilab);
        // Gone, the hands go back to the first taught
        hands.follow_devices(&open[..1]);
        assert_eq!(hands.device(), "MiniLab3");
    }

    #[test]
    fn with_one_device_open_teaching_is_that_ones() {
        let open = ["Little Keys".to_string()];
        let mut hands = Hands::default();
        hands.follow_devices(&open);
        hands.start_teaching();
        hands.follow_devices(&open);
        assert_eq!(hands.device(), "Little Keys", "the device open, to name it");
        for value in [64, 65, 66, 67] {
            // Soba's own CCs (a capture's) are the device's the hands are on
            hands.hear(70, 0, value, &|_, _| None, &mut Vec::new());
        }
        hands.finish_teaching();
        assert_eq!(hands.controller().map(Controller::summary).as_deref(), Some("1 encoder"));
        assert!(hands.controllers.contains_key("Little Keys"));
    }

    #[test]
    fn controllers_are_kept_by_device_name() {
        #[derive(Default)]
        struct Memory(HashMap<String, String>);
        impl eframe::Storage for Memory {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_string(), value);
            }
            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }
            fn flush(&mut self) {}
        }
        let hands = minilab();
        let mut storage = Memory::default();
        hands.store(&mut storage);
        assert!(storage.0.contains_key("controller:MiniLab3"));

        let mut loaded = Hands::default();
        loaded.load(&storage);
        loaded.set_device(Some("MiniLab3"));
        assert!(loaded.on);
        assert_eq!(loaded.controller(), hands.controller());
    }
}
