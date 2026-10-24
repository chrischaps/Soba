//! The UI's side of MIDI in (#343): the inputs to choose from and the one
//! chosen, the notes held on it, which CC or pad turns which control, and
//! MIDI Learn.
//! Kept apart from the frame loop, with no egui in it, so what a note or a
//! CC does can be tested.
//!
//! Notes reach Note In modules on the audio thread, sample-accurately;
//! nothing here changes what they play. The UI's copy of each event lights
//! the pianos and the MIDI Monitor, and turns mapped knobs.
//!
//! Notes played on the UI itself (the computer keys, the piano) go to the
//! audio thread on a ring of their own ([`UiNotes`]), with or without a
//! MIDI device, and are heard here as MIDI in is.

use rtrb::Consumer;

use crate::engine::{MidiDeviceInfo, MidiEngine, MidiEvent, TimestampedMidiEvent, UiNotes};
use crate::persistence::{MidiControl, MidiEncoding, MidiInput, MidiMapping, MidiSource};
use super::midi_mappings::{MidiMappings, Standing};
use super::module_learn::ModuleLearn;
use super::hands::Hands;

/// How many CCs a newly learned knob listens to before saying whether
/// it's an endless encoder (#424).
const GUESS_FROM: usize = 4;

/// How often the inputs are looked at again.
const FOLLOW_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The input to open for the one remembered (#425): its place in `devices`,
/// if it's listed. Nothing is opened over an input already open, or once
/// the player has chosen None by hand (which forgets the name).
pub fn device_to_connect(remembered: Option<&str>, devices: &[MidiDeviceInfo], selected: Option<usize>, by_hand: bool) -> Option<usize> {
    if selected.is_some() {
        return None;
    }
    let name = remembered?;
    // A hand-picked input is remembered the moment it is picked, so what
    // is remembered is always the last choice; this only guards a blank name
    if by_hand && name.is_empty() {
        return None;
    }
    devices.iter().find(|d| d.name == name).map(|d| d.index)
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
    /// The input chosen, by its index in `devices`.
    pub selected: Option<usize>,
    /// Why MIDI can't be had, or the chosen input wouldn't open.
    pub error: Option<String>,
    /// The name of the input to have, kept between launches (#425): the
    /// one last chosen, or none once None was.
    pub remembered: Option<String>,
    /// Has the input been chosen by hand this session?
    by_hand: bool,
    /// The names of the inputs as last followed, to see one come or go.
    listed: Vec<String>,
    /// When the inputs were last followed.
    followed: Option<web_time::Instant>,
    /// The notes held, in order of press, each with its velocity.
    held: Vec<(u8, u8)>,
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

    /// The input chosen by hand: opened, and remembered for next time.
    pub fn choose(&mut self, index: usize) {
        self.by_hand = true;
        self.connect(index);
        if let Some(device) = self.selected_device() {
            self.remembered = Some(device.name.clone());
        }
    }

    /// None chosen by hand: closed, and forgotten.
    pub fn choose_none(&mut self) {
        self.by_hand = true;
        self.remembered = None;
        self.disconnect();
    }

    /// Tells of the input remembered from a past session. It's looked for
    /// at the next [`follow_devices`](Self::follow_devices).
    pub fn remember(&mut self, name: Option<String>) {
        self.remembered = name.filter(|n| !n.is_empty());
        self.listed.clear();
    }

    /// Follows the inputs, if a second has gone since the last time (the
    /// scanner thread looks every 2 s). Returns whether the input open
    /// changed.
    pub fn follow_devices_soon(&mut self) -> bool {
        let now = web_time::Instant::now();
        if self.followed.is_some_and(|t| now.duration_since(t) < FOLLOW_EVERY) {
            return false;
        }
        self.followed = Some(now);
        self.follow_devices()
    }

    /// Lists the inputs again and follows them. The input open keeps its
    /// name as the list is numbered afresh, and is let go if it was
    /// unplugged. The one remembered is opened when it appears, unless
    /// something is open or None was chosen. Returns whether the input
    /// open changed.
    pub fn follow_devices(&mut self) -> bool {
        if self.engine.is_none() {
            return false;
        }
        let open = self.selected_device().map(|d| d.name.clone());
        self.refresh_devices();
        let names: Vec<String> = self.devices.iter().map(|d| d.name.clone()).collect();
        if names == self.listed {
            return false;
        }
        self.listed = names;
        let mut changed = false;
        if let Some(name) = open {
            match self.devices.iter().position(|d| d.name == name) {
                Some(index) => self.selected = Some(index),
                None => {
                    self.disconnect();
                    changed = true;
                }
            }
        }
        if let Some(index) = device_to_connect(self.remembered.as_deref(), &self.devices, self.selected, self.by_hand) {
            self.connect(index);
            changed = true;
        }
        changed
    }

    /// The chosen input, if one is.
    pub fn selected_device(&self) -> Option<&MidiDeviceInfo> {
        self.selected.and_then(|i| self.devices.get(i))
    }

    /// Is the chosen input open?
    pub fn is_connected(&self) -> bool {
        self.selected.is_some() && self.engine.as_ref().is_some_and(|e| e.is_connected())
    }

    /// Opens input `index` in place of the one open. The notes held on the
    /// last were let go (All Notes Off), so none are held after.
    pub fn connect(&mut self, index: usize) {
        let Some(engine) = self.engine.as_mut() else { return };
        match engine.connect(index) {
            Ok(()) => {
                self.selected = Some(index);
                self.error = None;
            }
            Err(e) => {
                // connect() drops the previous connection before trying
                self.selected = None;
                self.error = Some(e.to_string());
            }
        }
        self.held.clear();
    }

    /// Closes the input open, letting go of its notes.
    pub fn disconnect(&mut self) {
        let Some(engine) = self.engine.as_mut() else { return };
        engine.disconnect();
        self.selected = None;
        self.error = None;
        self.held.clear();
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
    /// key's, but on any channel: a `?listen` audition plays MIDI Note Ins too.
    #[cfg(target_arch = "wasm32")]
    pub fn send_ui(&mut self, event: MidiEvent) {
        if let Some(notes) = self.notes.as_mut() {
            notes.send(event);
            self.played.push(event);
        }
    }

    /// Takes every event waiting, and the notes the UI played, handing
    /// each to `each` (the MIDI Monitor's log) as it's heard. `current`
    /// gives where a parameter stands, which a toggle flips, a dropdown
    /// steps on from, an encoder turns from and a fader picks up.
    pub fn drain(&mut self, mut each: impl FnMut(MidiEvent), current: impl Fn(u64, usize) -> Option<Standing>) -> Heard {
        let mut heard = Heard::default();
        if let Some(mut events) = self.events.take() {
            while let Ok(timestamped) = events.pop() {
                each(timestamped.event);
                self.hear(timestamped.event, &mut heard, &current);
            }
            self.events = Some(events);
        }
        for event in std::mem::take(&mut self.played) {
            each(event);
            self.hear(event, &mut heard, &current);
        }
        heard
    }

    /// What one event does: a note is held or let go, and a CC or a note is
    /// learned, or turns the controls mapped to it.
    pub fn hear(&mut self, event: MidiEvent, heard: &mut Heard, current: &impl Fn(u64, usize) -> Option<Standing>) {
        let (source, number, channel, input) = match event {
            MidiEvent::NoteOn { channel, note, velocity } => {
                if !self.held.iter().any(|&(n, _)| n == note) {
                    self.held.push((note, velocity));
                    heard.notes_changed = true;
                }
                (MidiSource::Note, note, channel, MidiInput::NoteOn)
            }
            MidiEvent::NoteOff { channel, note, .. } => {
                if let Some(at) = self.held.iter().position(|&(n, _)| n == note) {
                    self.held.remove(at);
                    heard.notes_changed = true;
                }
                (MidiSource::Note, note, channel, MidiInput::NoteOff)
            }
            MidiEvent::ControlChange { channel, controller, value } => (MidiSource::Cc, controller, channel, MidiInput::Cc(value)),
            _ => {
                self.learn_into(&event, heard);
                return;
            }
        };
        // While Soba is taught the controller, its CCs are for that alone (#429)
        if let (MidiInput::Cc(value), Some(_)) = (input, self.hands.teaching()) {
            self.hands.hear(number, channel, value, current, &mut heard.turns);
            return;
        }
        if self.learn_into(&event, heard) {
            return;
        }
        // A note let go isn't one to learn: its press came first
        if input != MidiInput::NoteOff && self.learning.as_ref().is_some_and(|target| target.into.is_none()) {
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
                // A knob's or dropdown's CC may be an endless encoder: it
                // says so over the next few values
                self.guessing = (source == MidiSource::Cc && matches!(target.control, MidiControl::Knob | MidiControl::Select))
                    .then(|| Guess { node_id: target.node_id, param_index: target.param_index, into: false, controller: number, channel, values: vec![value_of(input)] });
                self.mappings.learn(mapping);
                heard.learned = true;
                return;
            }
        }
        if source == MidiSource::Cc && self.guess(number, channel, value_of(input), heard) {
            heard.learned = true;
            // The control holds still while it's told apart
            return;
        }
        // A taught control turns the selection's knob while the hands follow,
        // and the patch's mappings on it pause
        if let MidiInput::Cc(value) = input {
            if self.hands.hear(number, channel, value, current, &mut heard.turns) {
                return;
            }
        }
        for index in 0..self.mappings.len() {
            let mapping = &self.mappings.as_slice()[index];
            if !mapping.hears(source, number, channel) {
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
        let heard = session.drain(|event| logged.push(event), |_, _| None);
        assert!(heard.notes_changed);
        assert_eq!(logged.len(), 3, "the MIDI Monitor logs them");
        assert_eq!(session.held(), [(64, crate::engine::UI_VELOCITY)]);
        assert_eq!(session.drain(|_| {}, |_, _| None), Heard::default(), "heard once");

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
    fn drain_hands_every_event_to_the_monitor() {
        let (mut producer, consumer) = rtrb::RingBuffer::new(8);
        let mut session = MidiSession::new(None, Some(consumer), None);
        for event in [MidiEvent::NoteOn { channel: 0, note: 60, velocity: 1 }, cc(0, 1, 2)] {
            producer.push(TimestampedMidiEvent { event, received: web_time::Instant::now() }).unwrap();
        }
        let mut logged = Vec::new();
        let heard = session.drain(|event| logged.push(event), |_, _| None);
        assert_eq!(logged.len(), 2);
        assert!(heard.notes_changed);
        assert!(session.events.is_some());
    }
}

#[cfg(test)]
mod remember_tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<MidiDeviceInfo> {
        names.iter().enumerate().map(|(index, n)| MidiDeviceInfo { name: n.to_string(), index }).collect()
    }

    #[test]
    fn the_remembered_device_is_found_by_name_wherever_it_is_listed() {
        let devices = list(&["Loopback", "MiniLab3 MIDI"]);
        assert_eq!(device_to_connect(Some("MiniLab3 MIDI"), &devices, None, false), Some(1));
        let shuffled = list(&["MiniLab3 MIDI", "Loopback"]);
        assert_eq!(device_to_connect(Some("MiniLab3 MIDI"), &shuffled, None, false), Some(0));
    }

    #[test]
    fn nothing_is_opened_while_unplugged_or_nothing_is_remembered() {
        let devices = list(&["Loopback"]);
        assert_eq!(device_to_connect(Some("MiniLab3 MIDI"), &devices, None, false), None);
        assert_eq!(device_to_connect(None, &devices, None, false), None);
        assert_eq!(device_to_connect(Some("MiniLab3 MIDI"), &[], None, false), None);
    }

    #[test]
    fn nothing_is_opened_over_an_input_already_open() {
        let devices = list(&["Loopback", "MiniLab3 MIDI"]);
        assert_eq!(device_to_connect(Some("MiniLab3 MIDI"), &devices, Some(0), true), None);
        assert_eq!(device_to_connect(Some("MiniLab3 MIDI"), &devices, Some(0), false), None);
    }

    #[test]
    fn choosing_none_leaves_nothing_to_open() {
        let mut session = MidiSession::default();
        session.remember(Some("MiniLab3 MIDI".into()));
        session.choose_none();
        assert_eq!(session.remembered, None);
        let devices = list(&["MiniLab3 MIDI"]);
        assert_eq!(device_to_connect(session.remembered.as_deref(), &devices, session.selected, session.by_hand), None);
    }

    #[test]
    fn a_blank_saved_name_is_nothing_remembered() {
        let mut session = MidiSession::default();
        session.remember(Some(String::new()));
        assert_eq!(session.remembered, None);
    }
}
