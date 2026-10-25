//! The UI's side of MIDI in (#343): the inputs to choose from and those
//! open, the notes held on them, which CC or pad turns which control, and
//! MIDI Learn.
//! Kept apart from the frame loop, with no egui in it, so what a note or a
//! CC does can be tested.
//!
//! Notes reach Note In modules on the audio thread, sample-accurately;
//! nothing here changes what they play. The UI's copy of each event lights
//! the pianos and the MIDI Monitor, and turns mapped knobs.
//!
//! Several inputs can be open at once (#435). Each event is heard with the
//! name of the device it came from, so a mapping learned from one device
//! isn't turned by another, and the hands follow one controller.
//!
//! A device is kept by name, and found again when the system renumbers it
//! (#436): "2- Digitakt" remembered opens "3- Digitakt", and a module or
//! mapping set to the one hears the other (see [`device_names`]).
//!
//! Notes played on the UI itself (the computer keys, the piano) go to the
//! audio thread on a ring of their own ([`UiNotes`]), with or without a
//! MIDI device, and are heard here as MIDI in is.

use rtrb::Consumer;

use crate::dsp::{MIDI_INPUTS, SOBA_PORT};
use crate::engine::device_names;
use crate::engine::{MidiDeviceInfo, MidiEngine, MidiEvent, TimestampedMidiEvent, UiNotes};
use crate::persistence::{MidiControl, MidiEncoding, MidiInput, MidiMapping, MidiSource};
use super::midi_mappings::{MidiMappings, Standing};
use super::module_learn::ModuleLearn;
use super::hands::Hands;
use crate::hardware::listening::Listening;

/// How many CCs a newly learned knob listens to before saying whether
/// it's an endless encoder (#424).
const GUESS_FROM: usize = 4;

/// How often the inputs are looked at again.
const FOLLOW_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The inputs to open of those remembered (#425): each one's place in
/// `devices`, if it's listed and not open already.
pub fn devices_to_connect(remembered: &[String], devices: &[MidiDeviceInfo], open: &[String]) -> Vec<usize> {
    devices
        .iter()
        .filter(|d| remembered.contains(&d.name) && !open.contains(&d.name))
        .map(|d| d.index)
        .collect()
}

/// Has each input remembered by a name the system has since renumbered
/// ("2- Digitakt", now "3- Digitakt") take its new name, so it's opened,
/// and kept, as it's called now.
pub fn follow_renames(remembered: &mut [String], devices: &[MidiDeviceInfo]) {
    let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
    let found: Vec<Option<String>> = device_names::find_all(remembered, &names).into_iter().map(|f| f.map(str::to_string)).collect();
    for (name, found) in remembered.iter_mut().zip(found) {
        if let Some(found) = found {
            *name = found;
        }
    }
}

/// What an input has been heard to send, for the Rig panel's lamps.
#[derive(Debug, Default, Clone, Copy)]
pub struct InputTraffic {
    /// Events heard, which moves as each comes: clock ticks aren't heard here.
    pub count: u32,
    /// The last of them.
    pub last: Option<MidiEvent>,
}

/// The names of the inputs remembered, from the app's storage: one a line.
/// A value from before #435 is a single name.
pub fn remembered_from(setting: &str) -> Vec<String> {
    setting.lines().map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect()
}

/// The control MIDI Learn is waiting for a CC or a note for.
#[derive(Debug, Clone)]
pub struct MidiLearnTarget {
    /// Engine node ID of the target parameter.
    pub node_id: u64,
    /// Parameter index within the node.
    pub param_index: usize,
    /// Parameter name for display.
    pub param_name: String,
    /// Minimum value of the parameter range.
    pub min_value: f32,
    /// Maximum value of the parameter range.
    pub max_value: f32,
    /// What kind of control it is: what a CC or a note will do to it.
    pub control: MidiControl,
    /// The Scenes pad (0-7) to recall, in place of a parameter.
    pub scene: Option<u8>,
    /// Where in the module to write what's heard, in place of mapping it:
    /// a Controls slot learning its fader (see [`super::module_learn`]).
    pub into: Option<ModuleLearn>,
}

impl MidiLearnTarget {
    /// What the status bar asks for while the learn waits.
    pub fn prompt(&self) -> String {
        match &self.into {
            Some(into) => format!("{}, for {}... (Esc to cancel)", into.prompt(), self.param_name),
            None => format!("Move a knob or fader, or hit a pad, to map {}... (Esc to cancel)", self.param_name),
        }
    }
}

/// What the events heard in one go asked of the UI.
#[derive(Debug, Default, PartialEq)]
pub struct Heard {
    /// A note started or stopped: the pianos show [`MidiSession::held`].
    pub notes_changed: bool,
    /// MIDI Learn took a CC or a note, and has stopped listening; or what
    /// a learned CC is (an endless encoder) has come clear since.
    pub learned: bool,
    /// Controls turned by mapped CCs and notes: engine node, parameter
    /// index and value.
    pub turns: Vec<(u64, usize, f32)>,
    /// Footswitches pressed or let go by mapped CCs and notes: engine
    /// node, parameter index, value, and whether it's a press. A quick tap
    /// brings both at once, so the app holds the release back a moment.
    pub footswitches: Vec<(u64, usize, f32, bool)>,
    /// Scenes pads recalled by mapped CCs and notes: engine node and slot.
    pub recalls: Vec<(u64, u8)>,
    /// Parameters a learn into a module wrote: engine node, parameter
    /// index and value. Unlike `turns`, they're edits, which undo takes back.
    pub learned_into: Vec<(u64, usize, f32)>,
    /// A presenter's footswitches (#489), which nothing else heard:
    /// channel (0-15), CC and value.
    pub pedals: Vec<(u8, u8, u8)>,
}

/// A CC just learned, listened to a moment longer.
struct Guess {
    node_id: u64,
    /// The mapping's parameter, or for a learn into a module, its
    /// encoding parameter.
    param_index: usize,
    /// Whether it was learned into a module, which is told by its encoding
    /// parameter rather than a mapping's style.
    into: bool,
    controller: u8,
    channel: u8,
    values: Vec<u8>,
}

/// MIDI in, as the UI holds it.
#[derive(Default)]
pub struct MidiSession {
    /// The MIDI inputs, or `None` where there are none to have (a DAW
    /// hands the plugin its MIDI, and a failed start leaves none).
    pub engine: Option<MidiEngine>,
    /// The UI's copy of the incoming events.
    pub events: Option<Consumer<TimestampedMidiEvent>>,
    /// The inputs to choose from, as last listed.
    pub devices: Vec<MidiDeviceInfo>,
    /// Why MIDI can't be had, or an input chosen wouldn't open.
    pub error: Option<String>,
    /// The names of the inputs to have open, kept between launches (#425):
    /// those chosen, less those closed by hand.
    pub remembered: Vec<String>,
    /// The names of the inputs as last followed, to see one come or go.
    listed: Vec<String>,
    /// When the inputs were last followed.
    followed: Option<web_time::Instant>,
    /// The notes held, in order of press, each with its velocity.
    held: Vec<(u8, u8)>,
    /// The input each held note was played on, beside `held`.
    held_on: Vec<u8>,
    /// The notes Pads modules claim (#428), which aren't held for the
    /// pianos, as an Omni Note In doesn't play them.
    pub claims: crate::dsp::NoteClaims,
    /// The slots of the inputs open for their thru routes alone (#463), a
    /// bit a slot: only what names one's device hears it.
    pub thru_only: u8,
    /// Which CC, note or macro turns which control.
    pub mappings: MidiMappings,
    /// The mappings as last opened or saved. Undo history doesn't hold
    /// them, so they say for themselves whether the patch has changed.
    saved_mappings: MidiMappings,
    learning: Option<MidiLearnTarget>,
    /// The CC just learned, and the values it has sent since: enough of
    /// them tell an endless encoder from a fader.
    guessing: Option<Guess>,
    /// The ring the UI's own notes take to the audio thread.
    notes: Option<UiNotes>,
    /// Notes played on the UI since the last [`drain`](Self::drain), for
    /// it to hear with the MIDI that came in.
    played: Vec<MidiEvent>,
    /// The controller's encoders and faders, which follow the selection
    /// (#429), and teaching Soba them.
    pub hands: Hands,
    /// What each input open has sent, by slot (#436).
    pub traffic: [InputTraffic; MIDI_INPUTS],
    /// A device's knobs being heard, to make its profile (#441).
    pub listening: Option<Listening>,
    /// A presenter's footswitches, held back for the presentation alone
    /// (#489): CCs, each on one channel (0-15) or any.
    pedals: Vec<(u8, Option<u8>)>,
}

impl MidiSession {
    /// The session of `engine`, its inputs listed, or of none (`error`
    /// says why, when MIDI wouldn't start).
    pub fn new(engine: Option<MidiEngine>, events: Option<Consumer<TimestampedMidiEvent>>, error: Option<String>) -> Self {
        let mut session = Self { engine, events, error, ..Self::default() };
        session.refresh_devices();
        session
    }

    /// Lists the inputs again.
    pub fn refresh_devices(&mut self) {
        if let Some(engine) = self.engine.as_mut() {
            self.devices = engine.enumerate_devices();
        }
    }

    /// Opens input `index` if it's closed, or closes it if it's open, and
    /// remembers the inputs open for next time.
    pub fn toggle(&mut self, index: usize) {
        let Some(name) = self.devices.get(index).map(|d| d.name.clone()) else { return };
        if self.is_open(&name) {
            self.remembered.retain(|n| *n != name);
            self.disconnect(&name);
        } else {
            self.connect(index);
            if self.is_open(&name) && !self.remembered.contains(&name) {
                self.remembered.push(name);
            }
        }
    }

    /// Opens input `index` beside those open, and remembers it. The browser
    /// has no handover to resume them from.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn choose(&mut self, index: usize) {
        let Some(name) = self.devices.get(index).map(|d| d.name.clone()) else { return };
        if !self.is_open(&name) {
            self.toggle(index);
        }
    }

    /// None chosen by hand: every input closed, and forgotten.
    pub fn choose_none(&mut self) {
        self.remembered.clear();
        self.disconnect_all();
    }

    /// Tells of the inputs remembered from a past session. They're looked
    /// for at the next [`follow_devices`](Self::follow_devices).
    pub fn remember(&mut self, names: Vec<String>) {
        self.remembered = names.into_iter().filter(|n| !n.is_empty()).collect();
        self.listed.clear();
    }

    /// The inputs remembered, for the app's storage: one a line.
    pub fn remembered_setting(&self) -> String {
        self.remembered.join("\n")
    }

    /// Follows the inputs, if a second has gone since the last time (the
    /// scanner thread looks every 2 s). Returns whether the inputs open
    /// changed.
    pub fn follow_devices_soon(&mut self) -> bool {
        let now = web_time::Instant::now();
        if self.followed.is_some_and(|t| now.duration_since(t) < FOLLOW_EVERY) {
            return false;
        }
        self.followed = Some(now);
        self.follow_devices()
    }

    /// Lists the inputs again and follows them. An input open that was
    /// unplugged is let go, and those remembered open as they appear.
    /// Returns whether the inputs open changed.
    pub fn follow_devices(&mut self) -> bool {
        if self.engine.is_none() {
            return false;
        }
        self.refresh_devices();
        let names: Vec<String> = self.devices.iter().map(|d| d.name.clone()).collect();
        if names == self.listed {
            return false;
        }
        self.listed = names;
        follow_renames(&mut self.remembered, &self.devices);
        let mut changed = false;
        for name in self.open_names() {
            if !self.devices.iter().any(|d| d.name == name) {
                self.disconnect(&name);
                changed = true;
            }
        }
        for index in devices_to_connect(&self.remembered, &self.devices, &self.open_names()) {
            self.connect(index);
            changed = true;
        }
        changed
    }

    /// The names of the inputs open, in the order they were opened into
    /// their slots.
    pub fn open_names(&self) -> Vec<String> {
        self.engine.as_ref().map_or_else(Vec::new, |e| e.open_inputs().map(|(_, name)| name.to_string()).collect())
    }

    /// The inputs open, each beside its slot: the port its events come on.
    pub fn open_slots(&self) -> Vec<(u8, String)> {
        self.engine.as_ref().map_or_else(Vec::new, |e| e.open_inputs().map(|(slot, name)| (slot, name.to_string())).collect())
    }

    /// Is the input called `name` open?
    pub fn is_open(&self, name: &str) -> bool {
        self.engine.as_ref().is_some_and(|e| e.slot_of(name).is_some())
    }

    /// The slot of the input called `name`, if it's open: the port its
    /// events come on. A name the system has since renumbered finds it.
    pub fn slot_of(&self, name: &str) -> Option<u8> {
        let engine = self.engine.as_ref()?;
        engine.slot_of(name).or_else(|| engine.slot_of(device_names::find(name, &self.open_names())?))
    }

    /// Is any input open?
    pub fn is_connected(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.is_connected())
    }

    /// Opens input `index` beside those open.
    fn connect(&mut self, index: usize) {
        let Some(engine) = self.engine.as_mut() else { return };
        match engine.connect(index) {
            Ok(_) => self.error = None,
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Closes the input called `name`, letting go of its notes.
    fn disconnect(&mut self, name: &str) {
        let Some(engine) = self.engine.as_mut() else { return };
        let Some(slot) = engine.slot_of(name) else { return };
        engine.disconnect(name);
        self.error = None;
        self.let_go(slot);
        self.traffic[slot as usize] = InputTraffic::default();
    }

    /// Closes every input open, letting go of their notes.
    fn disconnect_all(&mut self) {
        for name in self.open_names() {
            self.disconnect(&name);
        }
    }

    /// Lets go of the notes held on input `port`, which was closed.
    fn let_go(&mut self, port: u8) {
        let mut at = 0;
        while at < self.held.len() {
            if self.held_on[at] == port {
                self.held.remove(at);
                self.held_on.remove(at);
            } else {
                at += 1;
            }
        }
    }

    /// The notes held, in order of press, each with its velocity.
    pub fn held(&self) -> &[(u8, u8)] {
        &self.held
    }

    /// Has the UI's own notes ([`play_note`](Self::play_note)) sent on
    /// `notes`, whose other end the audio processor has.
    pub fn set_ui_notes(&mut self, notes: UiNotes) {
        self.notes = Some(notes);
    }

    /// Plays `note` (0-127), or lets it go, from the UI: a computer key or
    /// the piano. It goes to the audio thread on the UI's ring, and is
    /// heard here with the next [`drain`](Self::drain), as MIDI in is.
    pub fn play_note(&mut self, note: u8, on: bool) {
        if let Some(notes) = self.notes.as_mut() {
            self.played.push(notes.note(note, on));
        }
    }

    /// Sends `event` from the UI, as [`play_note`](Self::play_note) sends a
    /// key's, but on any channel: a `?listen` audition plays MIDI Note Ins
    /// too, and so does a presentation's beat.
    pub fn send_ui(&mut self, event: MidiEvent) {
        if let Some(notes) = self.notes.as_mut() {
            notes.send(event);
            self.played.push(event);
        }
    }

    /// Keeps these CCs from everything but a presentation, which hears
    /// them in [`Heard::pedals`]: no module, route, mapping or learn does
    /// (#489). Empty lets them through again.
    pub fn set_pedals(&mut self, pedals: Vec<(u8, Option<u8>)>) {
        if let Some(engine) = &self.engine {
            engine.set_pedals(pedals.clone());
        }
        self.pedals = pedals;
    }

    /// Whether a device's `event` is a presenter's footswitch.
    fn pedal(&self, event: &MidiEvent) -> Option<(u8, u8, u8)> {
        match *event {
            MidiEvent::ControlChange { channel, controller, value }
                if self.pedals.iter().any(|&(cc, on)| cc == controller && on.is_none_or(|on| on == channel)) =>
            {
                Some((channel, controller, value))
            }
            _ => None,
        }
    }

    /// Takes every event waiting, and the notes the UI played, handing
    /// each to `each` (the MIDI Monitor's log) as it's heard, with the
    /// input it came in on and the name of its device (`SOBA_PORT` and
    /// `None` for Soba's own). `current`
    /// gives where a parameter stands, which a toggle flips, a dropdown
    /// steps on from, an encoder turns from and a fader picks up.
    pub fn drain(&mut self, mut each: impl FnMut(MidiEvent, u8, Option<&str>), current: impl Fn(u64, usize) -> Option<Standing>) -> Heard {
        let mut heard = Heard::default();
        // Named once: the inputs don't open or close while this drains
        let names: [Option<String>; MIDI_INPUTS] =
            std::array::from_fn(|slot| self.engine.as_ref().and_then(|e| e.name_of(slot as u8)).map(str::to_string));
        if let Some(mut events) = self.events.take() {
            while let Ok(timestamped) = events.pop() {
                let device = names.get(timestamped.port as usize).and_then(Option::as_deref);
                if let Some(traffic) = self.traffic.get_mut(timestamped.port as usize) {
                    traffic.count = traffic.count.wrapping_add(1);
                    traffic.last = Some(timestamped.event);
                }
                // The presenter's alone: not even the MIDI Monitor logs it
                if let Some(pedal) = self.pedal(&timestamped.event) {
                    heard.pedals.push(pedal);
                    continue;
                }
                each(timestamped.event, timestamped.port, device);
                self.hear_from(timestamped.port, device, timestamped.event, &mut heard, &current);
            }
            self.events = Some(events);
        }
        for event in std::mem::take(&mut self.played) {
            each(event, SOBA_PORT, None);
            self.hear(event, &mut heard, &current);
        }
        heard
    }

    /// What one event Soba played itself does (see [`hear_from`](Self::hear_from)).
    pub fn hear(&mut self, event: MidiEvent, heard: &mut Heard, current: &impl Fn(u64, usize) -> Option<Standing>) {
        self.hear_from(SOBA_PORT, None, event, heard, current);
    }

    /// What one event from input `port`, the device called `device`, does:
    /// a note is held or let go, and a CC or a note is learned, or turns
    /// the controls mapped to it.
    ///
    /// An input open for its thru routes alone (#463) is heard only by what
    /// names its device: a mapping learned from it, the hands on it, or its
    /// profile being heard. Nothing that hears any device holds its notes,
    /// learns from it or is turned by it.
    pub fn hear_from(&mut self, port: u8, device: Option<&str>, event: MidiEvent, heard: &mut Heard, current: &impl Fn(u64, usize) -> Option<Standing>) {
        let thru_only = port != SOBA_PORT && 1u8.checked_shl(port as u32).is_some_and(|bit| self.thru_only & bit != 0);
        let (source, number, channel, input) = match event {
            MidiEvent::NoteOn { channel, note, velocity } => {
                if !thru_only && !self.held.iter().any(|&(n, _)| n == note) && !self.claims.claims(port, channel, note) {
                    self.held.push((note, velocity));
                    self.held_on.push(port);
                    heard.notes_changed = true;
                }
                (MidiSource::Note, note, channel, MidiInput::NoteOn)
            }
            MidiEvent::NoteOff { channel, note, .. } => {
                if let Some(at) = self.held.iter().position(|&(n, _)| n == note) {
                    self.held.remove(at);
                    self.held_on.remove(at);
                    heard.notes_changed = true;
                }
                (MidiSource::Note, note, channel, MidiInput::NoteOff)
            }
            MidiEvent::ControlChange { channel, controller, value } => (MidiSource::Cc, controller, channel, MidiInput::Cc(value)),
            _ => {
                if !thru_only {
                    self.learn_into(&event, heard);
                }
                return;
            }
        };
        // A device being listened to for its profile hears its own CCs,
        // which go on to do what they'd do anyway (#441)
        // (with no device open, a capture's cues stand in for one)
        if let (MidiInput::Cc(value), Some(listening)) = (input, self.listening.as_mut()) {
            if device.map_or(listening.device.is_empty(), |device| device_names::same(device, &listening.device)) {
                listening.hear(channel, number, value);
            }
        }
        // The hands are one controller's: another device's CCs pass them by,
        // unless Soba is taught whichever moves first (#454)
        let taught_any = self.hands.teaching().is_some_and(|teaching| teaching.device().is_none());
        let for_hands = self.hands.hears_device(device) && !(thru_only && taught_any);
        // While Soba is taught the controller, its CCs are for that alone (#429)
        if let (MidiInput::Cc(value), Some(_), true) = (input, self.hands.teaching(), for_hands) {
            self.hands.hear_from(device, number, channel, value, current, &mut heard.turns);
            return;
        }
        if !thru_only && self.learn_into(&event, heard) {
            return;
        }
        // A note let go isn't one to learn: its press came first
        if !thru_only && input != MidiInput::NoteOff && self.learning.as_ref().is_some_and(|target| target.into.is_none()) {
            if let Some(target) = self.learning.take() {
                // On the channel it came on, so the keys and the pads can
                // share note numbers. It leaves whatever it turned before
                let mut mapping = MidiMapping::new(
                    number,
                    channel + 1,
                    target.node_id,
                    target.param_index,
                    target.param_name,
                    target.min_value,
                    target.max_value,
                )
                .of(target.control);
                if source == MidiSource::Note {
                    mapping = mapping.note();
                }
                if let Some(slot) = target.scene {
                    mapping = mapping.recalling(slot);
                }
                // With several devices open, from the one it came from, so
                // the same CC on another turns something else (#435)
                if self.engine.as_ref().is_some_and(|e| e.open_inputs().nth(1).is_some()) {
                    mapping.device = device.map(str::to_string);
                }
                // A knob's or dropdown's CC may be an endless encoder: it
                // says so over the next few values
                self.guessing = (source == MidiSource::Cc && matches!(target.control, MidiControl::Knob | MidiControl::Select))
                    .then(|| Guess { node_id: target.node_id, param_index: target.param_index, into: false, controller: number, channel, values: vec![value_of(input)] });
                self.mappings.learn(mapping);
                heard.learned = true;
                return;
            }
        }
        if !thru_only && source == MidiSource::Cc && self.guess(number, channel, value_of(input), heard) {
            heard.learned = true;
            // The control holds still while it's told apart
            return;
        }
        // A taught control turns the selection's knob while the hands follow,
        // and the patch's mappings on it pause
        if let (MidiInput::Cc(value), true) = (input, for_hands) {
            if self.hands.hear_from(device, number, channel, value, current, &mut heard.turns) {
                return;
            }
        }
        for index in 0..self.mappings.len() {
            let mapping = &self.mappings.as_slice()[index];
            if !mapping.hears(source, number, channel) || !mapping.hears_device(device) || (thru_only && mapping.device.is_none()) {
                continue;
            }
            let (node_id, param_index, control) = (mapping.node_id, mapping.param_index, mapping.control);
            match mapping.scene {
                Some(slot) => {
                    if input.presses() {
                        heard.recalls.push((node_id, slot));
                    }
                }
                None => {
                    let Some(value) = self.mappings.turn(index, input, current(node_id, param_index)) else { continue };
                    if control == MidiControl::Button {
                        heard.footswitches.push((node_id, param_index, value, input.presses()));
                    } else {
                        heard.turns.push((node_id, param_index, value));
                    }
                }
            }
        }
    }

    /// Listens to the CC just learned while it says what it is: true while
    /// it does, and the mapped control holds still. Its last value names
    /// it an endless encoder or not. A module that learned it hears it all
    /// along, so it's never held.
    fn guess(&mut self, controller: u8, channel: u8, value: u8, heard: &mut Heard) -> bool {
        let Some(guess) = self.guessing.as_mut().filter(|g| g.controller == controller && g.channel == channel) else {
            return false;
        };
        let hold = !guess.into;
        guess.values.push(value);
        if guess.values.len() < GUESS_FROM {
            return hold;
        }
        let encoding = MidiEncoding::guess(&guess.values);
        let (node_id, param_index, into) = (guess.node_id, guess.param_index, guess.into);
        self.guessing = None;
        if into {
            // Only news if it's an encoder: the learn wrote Absolute
            if encoding != MidiEncoding::Absolute {
                heard.learned_into.push((node_id, param_index, super::module_learn::encoding_value(encoding)));
                heard.learned = true;
            }
        } else {
            self.mappings.restyle(node_id, param_index, None, |m| m.encoding = encoding);
            heard.learned = true;
        }
        hold
    }

    /// Ends a learn into a module with `event`, if it's one the learn
    /// takes, writing what it heard into the module's parameters: true if
    /// it did. A CC is listened to a moment longer, to tell an endless
    /// encoder from a fader.
    fn learn_into(&mut self, event: &MidiEvent, heard: &mut Heard) -> bool {
        let Some((node_id, into)) = self.learning.as_ref().and_then(|target| Some((target.node_id, target.into?))) else {
            return false;
        };
        let Some(writes) = into.hear(event) else { return false };
        self.learning = None;
        heard.learned = true;
        heard.learned_into.extend(writes.into_iter().map(|(param, value)| (node_id, param, value)));
        self.guessing = match (*event, into.encoding) {
            (MidiEvent::ControlChange { channel, controller, value }, Some(param_index)) => {
                Some(Guess { node_id, param_index, into: true, controller, channel, values: vec![value] })
            }
            _ => None,
        };
        true
    }

    /// Waits for a CC or a note for `target`, in place of any control it
    /// waited for.
    pub fn start_learn(&mut self, target: MidiLearnTarget) {
        // Learning a control wants the controller more than teaching it does
        self.hands.cancel_teaching();
        self.guessing = None;
        self.learning = Some(target);
    }

    /// Stops waiting for a CC or a note.
    pub fn cancel_learn(&mut self) {
        self.learning = None;
    }

    /// The control MIDI Learn waits for a CC or a note for.
    pub fn learning(&self) -> Option<&MidiLearnTarget> {
        self.learning.as_ref()
    }

    /// The patch's mappings, opened in place of these.
    pub fn open(&mut self, mappings: Vec<MidiMapping>) {
        self.mappings.load(mappings);
        self.learning = None;
        self.guessing = None;
    }

    /// No mappings, and nothing being learned: a new patch's.
    pub fn clear(&mut self) {
        self.mappings.clear();
        self.learning = None;
        self.guessing = None;
    }

    /// The mappings as they stand are the patch's file's.
    pub fn mark_saved(&mut self) {
        self.saved_mappings = self.mappings.clone();
    }

    /// Have the mappings changed since the patch was opened or saved?
    pub fn unsaved(&self) -> bool {
        self.mappings != self.saved_mappings
    }
}

/// A CC's value; a note has none to guess from.
fn value_of(input: MidiInput) -> u8 {
    match input {
        MidiInput::Cc(value) => value,
        MidiInput::NoteOn | MidiInput::NoteOff => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(node_id: u64) -> MidiLearnTarget {
        MidiLearnTarget {
            node_id,
            param_index: 0,
            param_name: "Cutoff".into(),
            min_value: 0.0,
            max_value: 100.0,
            control: MidiControl::Knob,
            scene: None,
            into: None,
        }
    }

    fn on(channel: u8, note: u8) -> MidiEvent {
        MidiEvent::NoteOn { channel, note, velocity: 100 }
    }

    fn off(channel: u8, note: u8) -> MidiEvent {
        MidiEvent::NoteOff { channel, note, velocity: 0 }
    }

    fn cc(channel: u8, controller: u8, value: u8) -> MidiEvent {
        MidiEvent::ControlChange { channel, controller, value }
    }

    #[test]
    fn notes_played_here_reach_the_audio_ring_and_are_heard_with_no_device() {
        let mut session = MidiSession::default();
        let (notes, mut audio) = crate::engine::ui_notes();
        session.set_ui_notes(notes);
        session.play_note(60, true);
        session.play_note(64, true);
        session.play_note(60, false);

        let mut logged = Vec::new();
        let heard = session.drain(|event, _, _| logged.push(event), |_, _| None);
        assert!(heard.notes_changed);
        assert_eq!(logged.len(), 3, "the MIDI Monitor logs them");
        assert_eq!(session.held(), [(64, crate::engine::UI_VELOCITY)]);
        assert_eq!(session.drain(|_, _, _| {}, |_, _| None), Heard::default(), "heard once");

        let sent: Vec<_> = std::iter::from_fn(|| audio.pop().ok()).map(|e| e.event).collect();
        assert!(matches!(sent[..], [
            MidiEvent::NoteOn { channel: crate::dsp::KEYS_CHANNEL, note: 60, .. },
            MidiEvent::NoteOn { note: 64, .. },
            MidiEvent::NoteOff { note: 60, .. },
        ]));
    }

    fn hear(session: &mut MidiSession, events: &[MidiEvent]) -> Heard {
        hear_at(session, events, None)
    }

    /// What `events` do with every parameter standing at `current`.
    fn hear_at(session: &mut MidiSession, events: &[MidiEvent], current: Option<f32>) -> Heard {
        let mut heard = Heard::default();
        for &event in events {
            session.hear(event, &mut heard, &|_, _| current.map(|value| Standing { value, logarithmic: false, stepped: false }));
        }
        heard
    }

    #[test]
    fn notes_are_held_in_order_once_each() {
        let mut session = MidiSession::default();
        let on = |note, velocity| MidiEvent::NoteOn { channel: 0, note, velocity };
        let heard = hear(&mut session, &[on(60, 100), on(64, 90), on(60, 50)]);
        assert!(heard.notes_changed);
        assert_eq!(session.held(), [(60, 100), (64, 90)]);
        let heard = hear(&mut session, &[MidiEvent::NoteOff { channel: 0, note: 60, velocity: 0 }]);
        assert!(heard.notes_changed);
        assert_eq!(session.held(), [(64, 90)]);
        // Letting go of a note not held changes nothing
        assert!(!hear(&mut session, &[MidiEvent::NoteOff { channel: 0, note: 1, velocity: 0 }]).notes_changed);
    }

    #[test]
    fn a_pads_notes_arent_held_for_the_pianos_but_still_learn() {
        let mut session = MidiSession::default();
        session.claims.claim(crate::dsp::ListenTo::Any, Some(9), 36);
        hear(&mut session, &[MidiEvent::NoteOn { channel: 9, note: 36, velocity: 100 }, MidiEvent::NoteOn { channel: 0, note: 36, velocity: 90 }]);
        assert_eq!(session.held(), [(36, 90)], "the same note on another channel is a key");
        // A learn still hears the pad
        session.start_learn(target(3));
        assert!(hear(&mut session, &[MidiEvent::NoteOn { channel: 9, note: 36, velocity: 100 }]).learned);
    }

    #[test]
    fn a_pads_on_one_device_leaves_another_devices_notes_for_the_pianos() {
        let mut session = MidiSession::default();
        session.claims.claim(crate::dsp::ListenTo::Port(0), Some(9), 36);
        let (mut heard, none) = (Heard::default(), |_: u64, _: usize| None);
        session.hear_from(0, Some("MiniLab3 MIDI"), MidiEvent::NoteOn { channel: 9, note: 36, velocity: 100 }, &mut heard, &none);
        assert!(session.held().is_empty(), "the MiniLab's pad is the Pads'");
        session.hear_from(1, Some("Fantom-06"), MidiEvent::NoteOn { channel: 9, note: 36, velocity: 80 }, &mut heard, &none);
        assert_eq!(session.held(), [(36, 80)], "the Fantom's same note is a key");
    }

    #[test]
    fn learning_takes_the_next_cc_and_then_it_turns_the_knob() {
        let mut session = MidiSession::default();
        session.start_learn(target(7));
        let heard = hear(&mut session, &[cc(3, 74, 0)]);
        assert!(heard.learned);
        assert!(heard.turns.is_empty());
        assert!(session.learning().is_none());
        assert!(session.unsaved());

        // It listens to a few more, to tell a fader from an encoder: a
        // fader walks through its values
        assert!(hear(&mut session, &[cc(3, 74, 1), cc(3, 74, 2), cc(3, 74, 3)]).turns.is_empty());
        assert_eq!(session.mappings.as_slice()[0].encoding, MidiEncoding::Absolute);

        // Learned on its channel, it turns across the parameter's range
        let heard = hear(&mut session, &[cc(3, 74, 127), cc(9, 74, 0), cc(3, 75, 64)]);
        assert_eq!(heard.turns, [(7, 0, 100.0)]);

        session.mark_saved();
        assert!(!session.unsaved());
    }

    #[test]
    fn an_endless_encoder_is_told_apart_and_then_steps_the_knob() {
        let mut session = MidiSession::default();
        session.start_learn(target(7));
        hear(&mut session, &[cc(0, 20, 65)]);
        let heard = hear(&mut session, &[cc(0, 20, 65), cc(0, 20, 65), cc(0, 20, 65)]);
        assert!(heard.learned, "the badge has news");
        assert!(heard.turns.is_empty(), "the knob held still meanwhile");
        assert_eq!(session.mappings.as_slice()[0].encoding, MidiEncoding::Relative64);

        let heard = hear_at(&mut session, &[cc(0, 20, 66)], Some(50.0));
        assert_eq!(heard.turns, [(7, 0, 52.0)]);
    }

    #[test]
    fn a_mapping_from_before_channels_were_learned_hears_every_channel() {
        let mut session = MidiSession::default();
        session.open(vec![MidiMapping::new(74, 0, 7, 0, "Cutoff", 0.0, 100.0)]);
        let heard = hear(&mut session, &[cc(0, 74, 127), cc(9, 74, 0)]);
        assert_eq!(heard.turns, [(7, 0, 100.0), (7, 0, 0.0)]);
        // Where a knob stands, a fader waits to pick it up
        assert!(hear_at(&mut session, &[cc(0, 74, 0)], Some(60.0)).turns.is_empty());
    }

    #[test]
    fn a_pad_learned_onto_a_footswitch_presses_it_while_held() {
        let mut session = MidiSession::default();
        let pedal = MidiLearnTarget { max_value: 1.0, control: MidiControl::Button, ..target(3) };
        session.start_learn(pedal);
        // Learn waits for a press; a stray release isn't one
        assert!(!hear(&mut session, &[off(9, 36)]).learned);
        assert!(hear(&mut session, &[on(9, 36)]).learned);

        assert_eq!(hear(&mut session, &[on(9, 36)]).footswitches, [(3, 0, 1.0, true)]);
        assert_eq!(hear(&mut session, &[off(9, 36)]).footswitches, [(3, 0, 0.0, false)]);
        // The same note from the keys, on channel 1, is the keys'
        assert!(hear(&mut session, &[on(0, 36)]).footswitches.is_empty());
    }

    #[test]
    fn a_cc_presses_a_footswitch_at_64_or_more() {
        let mut session = MidiSession::default();
        session.start_learn(MidiLearnTarget { max_value: 1.0, control: MidiControl::Button, ..target(3) });
        hear(&mut session, &[cc(0, 20, 127)]);
        let heard = hear(&mut session, &[cc(0, 20, 127), cc(0, 20, 0), cc(0, 20, 64), cc(0, 20, 63)]);
        let values: Vec<_> = heard.footswitches.iter().map(|&(_, _, value, pressed)| (value, pressed)).collect();
        assert_eq!(values, [(1.0, true), (0.0, false), (1.0, true), (0.0, false)]);
    }

    #[test]
    fn a_pad_flips_a_toggle_and_a_cc_sets_it() {
        let mut session = MidiSession::default();
        session.start_learn(MidiLearnTarget { max_value: 1.0, control: MidiControl::Toggle, ..target(4) });
        hear(&mut session, &[on(9, 40)]);
        assert_eq!(hear_at(&mut session, &[on(9, 40), off(9, 40)], Some(0.0)).turns, [(4, 0, 1.0)]);
        assert_eq!(hear_at(&mut session, &[on(9, 40), off(9, 40)], Some(1.0)).turns, [(4, 0, 0.0)]);

        session.start_learn(MidiLearnTarget { max_value: 1.0, control: MidiControl::Toggle, ..target(4) });
        hear(&mut session, &[cc(0, 21, 127)]);
        assert_eq!(hear_at(&mut session, &[cc(0, 21, 127), cc(0, 21, 10)], Some(1.0)).turns, [(4, 0, 1.0), (4, 0, 0.0)]);
    }

    #[test]
    fn a_pad_steps_a_dropdown_round_and_a_cc_sweeps_it() {
        let mut session = MidiSession::default();
        let select = MidiLearnTarget { max_value: 3.0, control: MidiControl::Select, ..target(5) };
        session.start_learn(select.clone());
        hear(&mut session, &[on(9, 41)]);
        assert_eq!(hear_at(&mut session, &[on(9, 41), off(9, 41)], Some(1.0)).turns, [(5, 0, 2.0)]);
        assert_eq!(hear_at(&mut session, &[on(9, 41)], Some(3.0)).turns, [(5, 0, 0.0)], "past the last it goes round");

        session.start_learn(select);
        hear(&mut session, &[cc(0, 22, 0), cc(0, 22, 20), cc(0, 22, 40), cc(0, 22, 60)]);
        let heard = hear(&mut session, &[cc(0, 22, 0), cc(0, 22, 50), cc(0, 22, 127)]);
        assert_eq!(heard.turns, [(5, 0, 0.0), (5, 0, 1.0), (5, 0, 3.0)]);
    }

    #[test]
    fn a_pad_on_a_knob_holds_it_at_the_top() {
        let mut session = MidiSession::default();
        session.start_learn(target(6));
        hear(&mut session, &[on(9, 42)]);
        assert_eq!(hear(&mut session, &[on(9, 42), off(9, 42)]).turns, [(6, 0, 100.0), (6, 0, 0.0)]);
    }

    #[test]
    fn a_pad_recalls_a_scene() {
        let mut session = MidiSession::default();
        session.start_learn(MidiLearnTarget { scene: Some(2), ..target(8) });
        hear(&mut session, &[on(9, 43)]);
        let heard = hear(&mut session, &[on(9, 43), off(9, 43)]);
        assert_eq!(heard.recalls, [(8, 2)]);
        assert!(heard.turns.is_empty());
    }

    /// A learn into Controls slot B (#427): its source, CC, channel and
    /// encoding parameters.
    fn into_controls() -> MidiLearnTarget {
        use crate::modules::Controls;
        MidiLearnTarget {
            param_index: Controls::source_param(1),
            param_name: "Controls B".into(),
            into: Some(ModuleLearn {
                kinds: super::super::module_learn::LearnKinds { cc: Some(0.0), bend: Some(1.0), pressure: Some(3.0), note: None },
                kind: Some(Controls::source_param(1)),
                number: Some(Controls::cc_param(1)),
                channel: Some(Controls::channel_param(1)),
                encoding: Some(Controls::encoding_param(1)),
            }),
            ..target(9)
        }
    }

    #[test]
    fn a_learn_into_a_module_writes_its_params_and_maps_nothing() {
        use crate::modules::Controls;
        let mut session = MidiSession::default();
        session.start_learn(into_controls());
        assert!(session.learning().unwrap().prompt().starts_with("Move a knob or fader, bend, or press a key down, for Controls B"));
        // A note passes by, still played; the CC is learned
        let heard = hear(&mut session, &[on(0, 60), cc(4, 74, 10)]);
        assert!(heard.learned && heard.notes_changed);
        assert_eq!(
            heard.learned_into,
            [(9, Controls::source_param(1), 0.0), (9, Controls::cc_param(1), 74.0), (9, Controls::channel_param(1), 5.0), (9, Controls::encoding_param(1), 0.0)],
        );
        assert!(session.learning().is_none());
        assert!(session.mappings.is_empty(), "nothing is mapped");
        assert!(!session.unsaved());

        // A fader walks on: nothing more to say
        assert_eq!(hear(&mut session, &[cc(4, 74, 11), cc(4, 74, 12), cc(4, 74, 13)]), Heard::default());

        // An encoder says so, and the module is told
        session.start_learn(into_controls());
        hear(&mut session, &[cc(0, 20, 65)]);
        let heard = hear(&mut session, &[cc(0, 20, 65), cc(0, 20, 63), cc(0, 20, 65)]);
        assert_eq!(heard.learned_into, [(9, Controls::encoding_param(1), 1.0)]);

        // The pitch bend has no number
        session.start_learn(into_controls());
        let heard = hear(&mut session, &[MidiEvent::PitchBend { channel: 0, value: 0 }]);
        assert_eq!(heard.learned_into, [(9, Controls::source_param(1), 1.0), (9, Controls::channel_param(1), 1.0)]);
    }

    #[test]
    fn a_cancelled_learn_learns_nothing_and_a_new_patch_forgets() {
        let mut session = MidiSession::default();
        session.start_learn(target(1));
        session.cancel_learn();
        assert_eq!(hear(&mut session, &[cc(0, 10, 64)]), Heard::default());

        session.start_learn(target(1));
        hear(&mut session, &[cc(0, 10, 64)]);
        session.start_learn(target(2));
        session.clear();
        assert!(session.learning().is_none());
        assert!(hear(&mut session, &[cc(0, 10, 64)]).turns.is_empty());
    }

    #[test]
    fn a_mapping_from_one_device_isnt_turned_by_another() {
        let mut session = MidiSession::default();
        let fantom = MidiMapping { device: Some("Fantom-06".into()), ..MidiMapping::new(74, 0, 7, 0, "Cutoff", 0.0, 100.0) };
        session.open(vec![fantom, MidiMapping { jump: true, ..MidiMapping::new(74, 0, 8, 0, "Res", 0.0, 1.0) }]);
        let mut heard = Heard::default();
        let none = |_: u64, _: usize| None;
        session.hear_from(1, Some("MiniLab3 MIDI"), cc(0, 74, 127), &mut heard, &none);
        assert_eq!(heard.turns, [(8, 0, 1.0)], "the MiniLab turns only the mapping from any device");
        let mut heard = Heard::default();
        session.hear_from(0, Some("Fantom-06"), cc(0, 74, 127), &mut heard, &none);
        assert_eq!(heard.turns, [(7, 0, 100.0), (8, 0, 1.0)]);
    }

    /// Teaching with two devices open hears both, until one's encoder is
    /// taken; then the other's CCs are the patch's again (#454).
    #[test]
    fn teaching_takes_the_device_whose_encoder_is_turned_first() {
        let mut session = MidiSession::default();
        session.open(vec![MidiMapping::new(74, 0, 8, 0, "Res", 0.0, 1.0)]);
        session.hands.start_teaching();
        let none = |_: u64, _: usize| None;
        let mut heard = Heard::default();
        for _ in 0..4 {
            session.hear_from(1, Some("Fantom-06"), cc(0, 16, 1), &mut heard, &none);
        }
        assert!(heard.turns.is_empty(), "the Fantom's encoder is taught");
        assert_eq!(session.hands.teaching().and_then(|t| t.device()), Some("Fantom-06"));
        assert_eq!(session.hands.device(), "Fantom-06");
        // The MiniLab's CC 74 turns the patch's mapping again
        session.hear_from(0, Some("MiniLab3 MIDI"), cc(0, 74, 127), &mut heard, &none);
        assert_eq!(heard.turns, [(8, 0, 1.0)]);
    }

    /// A Thru-only input (#463) is heard by what names it, and nothing that
    /// hears any device: no held notes, no learning, no mapping from any.
    #[test]
    fn a_thru_only_input_is_heard_only_by_what_names_it() {
        let mut session = MidiSession::default();
        let fantom = MidiMapping { device: Some("Fantom-06".into()), ..MidiMapping::new(74, 0, 7, 0, "Cutoff", 0.0, 100.0) };
        session.open(vec![fantom, MidiMapping { jump: true, ..MidiMapping::new(74, 0, 8, 0, "Res", 0.0, 1.0) }]);
        session.thru_only = 0b10;
        let none = |_: u64, _: usize| None;
        let mut heard = Heard::default();
        session.hear_from(1, Some("2- Fantom-06"), cc(0, 74, 127), &mut heard, &none);
        assert_eq!(heard.turns, [(7, 0, 100.0)], "only the mapping that names it");
        session.hear_from(1, Some("2- Fantom-06"), on(0, 60), &mut heard, &none);
        assert!(session.held().is_empty() && !heard.notes_changed, "its notes play the room, not the pianos");
        // MIDI Learn waits for another device
        session.start_learn(target(1));
        let mut heard = Heard::default();
        session.hear_from(1, Some("2- Fantom-06"), cc(0, 10, 64), &mut heard, &none);
        assert!(!heard.learned && session.learning().is_some());
        // The MiniLab, in slot 0, plays as ever
        let mut heard = Heard::default();
        session.hear_from(0, Some("MiniLab3 MIDI"), cc(0, 10, 64), &mut heard, &none);
        assert!(heard.learned);
    }

    #[test]
    fn a_device_listened_to_for_its_profile_is_heard_and_its_cc_still_turns() {
        let mut session = MidiSession::default();
        session.open(vec![MidiMapping::new(74, 0, 7, 0, "Cutoff", 0.0, 1.0)]);
        session.listening = Some(Listening::new("MicroFreak"));
        let mut heard = Heard::default();
        let none = |_: u64, _: usize| None;
        session.hear_from(1, Some("MiniLab3 MIDI"), cc(0, 20, 5), &mut heard, &none);
        session.hear_from(0, Some("2- MicroFreak"), cc(0, 74, 127), &mut heard, &none);
        session.hear(cc(0, 21, 5), &mut heard, &none);
        let knobs: Vec<u8> = session.listening.as_ref().unwrap().knobs.iter().map(|k| k.cc).collect();
        assert_eq!(knobs, [74], "only the MicroFreak, renumbered or not");
        assert_eq!(heard.turns, [(7, 0, 1.0)]);
    }

    #[test]
    fn a_note_held_on_a_device_is_let_go_when_its_closed_and_the_others_stay() {
        let mut session = MidiSession::default();
        let mut heard = Heard::default();
        let none = |_: u64, _: usize| None;
        session.hear_from(0, Some("Fantom-06"), on(0, 60), &mut heard, &none);
        session.hear_from(1, Some("MiniLab3 MIDI"), on(0, 64), &mut heard, &none);
        session.hear(on(0, 67), &mut heard, &none);
        session.let_go(0);
        assert_eq!(session.held(), [(64, 100), (67, 100)]);
        session.hear_from(1, Some("MiniLab3 MIDI"), off(0, 64), &mut heard, &none);
        assert_eq!(session.held(), [(67, 100)]);
    }

    #[test]
    fn drain_hands_every_event_to_the_monitor() {
        let (mut producer, consumer) = rtrb::RingBuffer::new(8);
        let mut session = MidiSession::new(None, Some(consumer), None);
        for event in [MidiEvent::NoteOn { channel: 0, note: 60, velocity: 1 }, cc(0, 1, 2)] {
            producer.push(TimestampedMidiEvent::heard(0, event)).unwrap();
        }
        let mut logged = Vec::new();
        let heard = session.drain(|event, _, _| logged.push(event), |_, _| None);
        assert_eq!(logged.len(), 2);
        assert!(heard.notes_changed);
        assert!(session.events.is_some());
    }

    #[test]
    fn a_presenters_footswitch_is_heard_by_the_presentation_alone() {
        let (mut producer, consumer) = rtrb::RingBuffer::new(8);
        let mut session = MidiSession::new(None, Some(consumer), None);
        session.set_pedals(vec![(64, None), (65, Some(1))]);
        session.start_learn(target(7));
        // CC 64 on any channel, CC 65 on channel 2 only
        for event in [cc(0, 64, 127), cc(3, 64, 0), cc(0, 65, 127), cc(1, 65, 127)] {
            producer.push(TimestampedMidiEvent::heard(0, event)).unwrap();
        }
        let mut logged = Vec::new();
        let heard = session.drain(|event, _, _| logged.push(event), |_, _| None);
        assert_eq!(heard.pedals, [(0, 64, 127), (3, 64, 0), (1, 65, 127)]);
        // CC 65 on channel 1 is anyone's: the monitor logs it and the learn takes it
        assert_eq!(logged.len(), 1);
        assert!(heard.learned);
        assert_eq!(session.traffic[0].count, 4, "the input's light still shows the pedal");
        session.set_pedals(Vec::new());
        producer.push(TimestampedMidiEvent::heard(0, cc(0, 64, 127))).unwrap();
        assert!(session.drain(|_, _, _| {}, |_, _| None).pedals.is_empty());
    }
}

#[cfg(test)]
mod remember_tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<MidiDeviceInfo> {
        names.iter().enumerate().map(|(index, n)| MidiDeviceInfo { name: n.to_string(), index }).collect()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_remembered_devices_are_found_by_name_wherever_they_are_listed() {
        let remembered = names(&["MiniLab3 MIDI", "Fantom-06"]);
        let devices = list(&["Loopback", "MiniLab3 MIDI", "Fantom-06"]);
        assert_eq!(devices_to_connect(&remembered, &devices, &[]), [1, 2]);
        let shuffled = list(&["Fantom-06", "MiniLab3 MIDI", "Loopback"]);
        assert_eq!(devices_to_connect(&remembered, &shuffled, &[]), [0, 1]);
    }

    #[test]
    fn nothing_is_opened_while_unplugged_or_nothing_is_remembered() {
        let devices = list(&["Loopback"]);
        assert!(devices_to_connect(&names(&["MiniLab3 MIDI"]), &devices, &[]).is_empty());
        assert!(devices_to_connect(&[], &devices, &[]).is_empty());
        assert!(devices_to_connect(&names(&["MiniLab3 MIDI"]), &[], &[]).is_empty());
    }

    #[test]
    fn an_input_open_already_isnt_opened_again() {
        let devices = list(&["Loopback", "MiniLab3 MIDI", "Fantom-06"]);
        let remembered = names(&["MiniLab3 MIDI", "Fantom-06"]);
        assert_eq!(devices_to_connect(&remembered, &devices, &names(&["MiniLab3 MIDI"])), [2]);
    }

    #[test]
    fn a_renumbered_input_is_opened_and_remembered_as_its_called_now() {
        let mut remembered = names(&["2- Digitakt", "MIDIIN3 (Fantom-06)", "MicroFreak"]);
        let devices = list(&["MicroFreak", "MIDIIN2 (FANTOM-06)", "3- Digitakt"]);
        follow_renames(&mut remembered, &devices);
        assert_eq!(remembered, ["3- Digitakt", "MIDIIN2 (FANTOM-06)", "MicroFreak"]);
        assert_eq!(devices_to_connect(&remembered, &devices, &[]), [0, 1, 2]);
        // Unplugged, it keeps the name it had
        let mut remembered = names(&["2- Digitakt"]);
        follow_renames(&mut remembered, &list(&["MicroFreak"]));
        assert_eq!(remembered, ["2- Digitakt"]);
    }

    #[test]
    fn choosing_none_leaves_nothing_to_open() {
        let mut session = MidiSession::default();
        session.remember(names(&["MiniLab3 MIDI"]));
        session.choose_none();
        assert!(session.remembered.is_empty());
        assert!(devices_to_connect(&session.remembered, &list(&["MiniLab3 MIDI"]), &[]).is_empty());
    }

    #[test]
    fn the_setting_keeps_a_name_a_line_and_reads_the_single_name_kept_before() {
        let mut session = MidiSession::default();
        session.remember(remembered_from("MiniLab3 MIDI"));
        assert_eq!(session.remembered, ["MiniLab3 MIDI"]);
        session.remember(names(&["MiniLab3 MIDI", "Fantom-06"]));
        assert_eq!(remembered_from(&session.remembered_setting()), ["MiniLab3 MIDI", "Fantom-06"]);
        assert!(remembered_from("").is_empty(), "a blank setting is nothing remembered");
    }
}
