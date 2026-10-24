//! Pads: a pad controller's pads as gates, for finger drums and triggers
//! (#428).
//!
//! Pads send notes, and played into a Note In they're pitches: pad 1 is C2.
//! Pads hears eight of them as eight gates instead, to strike a Drum each,
//! open an envelope or reset a sequencer, with how hard each was hit and
//! how hard it's pressed.
//!
//! The pads are eight notes in a row from a Base note, on one Channel: 36
//! to 43 on channel 10 when the module is added, the General MIDI drum
//! notes most pad controllers send. Learn takes both from pad 1, and pads
//! 2 to 8 follow on the next seven notes. A pad can learn a note and
//! channel of its own, which it keeps when the rest are learned again. All
//! of it lives in hidden parameters, written by a learn from the face (see
//! [`crate::app::module_learn`]), so the module reads MIDI on the audio
//! thread, sample-accurately, and the patch keeps what it learned.
//!
//! The notes it listens to are claimed: a Note In on Omni skips them (see
//! [`crate::dsp::note_claims`]), so a hit never also plays the synth.
//!
//! Hits that land on the same sample are struck a sample apart, so **Hit**
//! (the latest hit's velocity) is each one's own as it rises: patch Hit
//! into every drum's Accent and each hears how hard it was struck.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::rate_for_seconds,
    signal::SignalBuffer,
    MidiEvent, MidiMessage, NoteClaims, ParameterDisplay, SignalType, KEYS_CHANNEL, MAX_CHANNELS,
};

/// The module's ID, as patches name it.
pub const ID: &str = "input.pads";

/// How many pads it has.
pub const PADS: usize = 8;

/// Pad 1's note when the module is added: C1, the General MIDI kick.
pub const DEFAULT_BASE: u8 = 36;

/// The channel the pads are heard on when the module is added: 10, the
/// General MIDI drum channel.
pub const DEFAULT_CHANNEL: u8 = 10;

/// How long a pad's gate lasts in Trigger mode, in seconds.
pub const TRIGGER_SECONDS: f32 = 0.005;

/// How long Pressure takes to follow the pads, so a 7-bit pressure's steps
/// don't zipper whatever it opens.
const PRESSURE_SLEW: f32 = 0.005;

/// The longest a pad is ever said to have been quiet, in seconds: the
/// readout's "never hit".
const QUIET: f32 = 1.0e6;

/// The Channel choices: 0 is Omni.
const CHANNEL_LABELS: &[&str] = &["Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// How a pad's gate follows its hits, as the Mode parameter holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// High while the pad is held.
    Momentary,
    /// Each hit flips it: hit once to open it, again to close it.
    Latch,
    /// A short gate per hit, however long the pad is held.
    Trigger,
}

impl Mode {
    /// The Mode choices, in parameter order.
    pub const LABELS: &'static [&'static str] = &["Momentary", "Latch", "Trigger"];
    const ALL: [Mode; 3] = [Mode::Momentary, Mode::Latch, Mode::Trigger];

    /// The mode a Mode parameter's value names.
    pub fn from_param(value: f32) -> Self {
        Self::ALL[(value.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

/// The Gate outputs' ids and names, pad by pad.
const GATE_IDS: [&str; PADS] = ["gate_1", "gate_2", "gate_3", "gate_4", "gate_5", "gate_6", "gate_7", "gate_8"];
const GATE_NAMES: [&str; PADS] = ["Gate 1", "Gate 2", "Gate 3", "Gate 4", "Gate 5", "Gate 6", "Gate 7", "Gate 8"];

/// Pads 2 to 8's own notes and channels, and every pad's press from the
/// face, by name. Pad 1's note and channel are Base and Channel.
const NOTE_NAMES: [(&str, &str); PADS - 1] =
    [("note_2", "Note 2"), ("note_3", "Note 3"), ("note_4", "Note 4"), ("note_5", "Note 5"), ("note_6", "Note 6"), ("note_7", "Note 7"), ("note_8", "Note 8")];
const CHANNEL_NAMES: [(&str, &str); PADS - 1] = [
    ("channel_2", "Channel 2"),
    ("channel_3", "Channel 3"),
    ("channel_4", "Channel 4"),
    ("channel_5", "Channel 5"),
    ("channel_6", "Channel 6"),
    ("channel_7", "Channel 7"),
    ("channel_8", "Channel 8"),
];
const PRESS_NAMES: [(&str, &str); PADS] =
    [("press_1", "Press 1"), ("press_2", "Press 2"), ("press_3", "Press 3"), ("press_4", "Press 4"), ("press_5", "Press 5"), ("press_6", "Press 6"), ("press_7", "Press 7"), ("press_8", "Press 8")];

/// What a pad's own note or channel holds while it follows the rest.
pub const FOLLOW: f32 = -1.0;

/// What one pad listens to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listen {
    /// The note, or `None` past the top of MIDI's (a Base of 125 leaves
    /// pads 4 to 8 with none).
    pub note: Option<u8>,
    /// 0 is Omni, 1-16 a MIDI channel.
    pub channel: u8,
    /// Whether it has a note or a channel of its own, rather than
    /// following pad 1.
    pub own: bool,
}

impl Listen {
    /// Whether an event on wire channel `channel` (0-15, or the keys') of
    /// note `note` is this pad's.
    #[inline]
    fn hears(&self, channel: u8, note: u8) -> bool {
        self.note == Some(note) && channel != KEYS_CHANNEL && (self.channel == 0 || self.channel - 1 == channel)
    }

    /// Whether channel pressure on wire channel `channel` reaches it.
    #[inline]
    fn hears_channel(&self, channel: u8) -> bool {
        channel != KEYS_CHANNEL && (self.channel == 0 || self.channel - 1 == channel)
    }
}

/// What each pad listens to, read from the parameters.
pub fn listens(params: &[f32]) -> [Listen; PADS] {
    let at = |index: usize, default: f32| params.get(index).copied().unwrap_or(default);
    let base = at(Pads::PARAM_BASE, DEFAULT_BASE as f32).round().clamp(0.0, 127.0) as u8;
    let channel = at(Pads::PARAM_CHANNEL, DEFAULT_CHANNEL as f32).round().clamp(0.0, 16.0) as u8;
    std::array::from_fn(|pad| {
        if pad == 0 {
            return Listen { note: Some(base), channel, own: false };
        }
        let own_note = at(Pads::note_param(pad), FOLLOW).round();
        let own_channel = at(Pads::channel_param(pad), FOLLOW).round();
        let note = if own_note >= 0.0 { Some(own_note.min(127.0) as u8) } else { base.checked_add(pad as u8).filter(|&note| note <= 127) };
        let pad_channel = if own_channel >= 0.0 { own_channel.min(16.0) as u8 } else { channel };
        Listen { note, channel: pad_channel, own: own_note >= 0.0 || own_channel >= 0.0 }
    })
}

/// Claims the notes a Pads module with `params` listens to, so an Omni
/// Note In skips them. It's how the engine and the UI both work them out.
///
/// REAL-TIME SAFE.
pub fn claim_notes(params: &[f32], claims: &mut NoteClaims) {
    for listen in listens(params) {
        if let Some(note) = listen.note {
            claims.claim(listen.channel.checked_sub(1), note);
        }
    }
}

/// One pad as it plays.
#[derive(Clone, Copy, Debug)]
struct Pad {
    /// Held down on the controller.
    held: bool,
    /// Held down on the face.
    pressed: bool,
    /// Open, in Latch mode.
    latched: bool,
    /// Samples of a Trigger's gate still to come.
    trigger_left: u32,
    /// Samples its gate drops for before rising again, so a hit while it's
    /// high is a new edge.
    rest: u32,
    /// How hard it was last hit, 0 to 1.
    velocity: f32,
    /// How hard it's pressed, from polyphonic aftertouch, 0 to 1.
    pressure: f32,
    /// Seconds since it was last hit.
    quiet: f32,
    /// Its gate at the end of the last block, for the face.
    gate: bool,
}

impl Pad {
    const IDLE: Pad =
        Pad { held: false, pressed: false, latched: false, trigger_left: 0, rest: 0, velocity: 0.0, pressure: 0.0, quiet: QUIET, gate: false };

    fn down(&self) -> bool {
        self.held || self.pressed
    }

    /// Whether its gate is high, in `mode`, before any rest.
    fn open(&self, mode: Mode) -> bool {
        match mode {
            Mode::Momentary => self.down(),
            Mode::Latch => self.latched,
            Mode::Trigger => self.trigger_left > 0,
        }
    }
}

/// Eight pads from a pad controller, each a gate.
///
/// # Ports
///
/// **Outputs:**
/// - **Gate 1** to **Gate 8** (Gate): each pad, as its Mode says.
/// - **Velocity** (Control): how hard each pad was last hit, 0 to 1, one
///   channel per pad on a polyphonic cable.
/// - **Hit** (Control): how hard the latest hit was, held until the next.
///   It changes on the sample its gate rises, so one cable into every
///   drum's Accent gives each its own hit.
/// - **Pressure** (Control): how hard the held pads are pressed, 0 to 1,
///   from polyphonic or channel aftertouch: the hardest of them.
///
/// # Parameters
///
/// - **Mode**: Momentary (high while held), Latch (each hit flips it) or
///   Trigger (a 5 ms gate per hit).
/// - Hidden, set from the face: **Base** and **Channel** (pad 1's note and
///   channel, which the rest follow), each other pad's own **Note** and
///   **Channel** (-1 follows), and each pad's **Press** from the face.
pub struct Pads {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    pads: [Pad; PADS],
    /// The face's presses as the last block saw them.
    pressed: [f32; PADS],
    /// The mode the last block played, so a new one starts clean.
    mode: Mode,
    /// Channel pressure, while any pad is held.
    channel_pressure: f32,
    /// The Hit output.
    hit: f32,
    /// The Pressure output, as slewed.
    pressure: f32,
    sample_rate: f32,
}

impl Pads {
    /// Creates a new Pads module.
    pub fn new() -> Self {
        let mut ports: Vec<PortDefinition> = (0..PADS)
            .map(|pad| {
                let description = if pad == 0 {
                    "Pad 1: high while it's held, flipped by each hit, or a short trigger, as Mode says"
                } else {
                    "This pad, as Mode says"
                };
                PortDefinition::output(GATE_IDS[pad], GATE_NAMES[pad], SignalType::Gate).describe(description)
            })
            .collect();
        ports.extend([
            PortDefinition::output("velocity", "Velocity", SignalType::Control)
                .describe("How hard each pad was last hit, 0 to 1: one channel per pad, on a polyphonic cable"),
            PortDefinition::output("hit", "Hit", SignalType::Control)
                .describe("How hard the latest hit was, held until the next. Patch it into every drum's Accent: each hears its own hit"),
            PortDefinition::output("pressure", "Pressure", SignalType::Control)
                .describe("How hard the held pads are pressed (aftertouch), 0 to 1"),
        ]);

        let mut parameters = vec![
            ParameterDefinition::choice("mode", "Mode", Mode::LABELS, 0)
                .describe("Momentary is high while a pad is held; Latch flips with each hit; Trigger is a short gate per hit"),
            ParameterDefinition::new("base", "Base", 0.0, 127.0, DEFAULT_BASE as f32, ParameterDisplay::stepped(""))
                .describe("Pad 1's note; pads 2 to 8 follow on the next seven"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, DEFAULT_CHANNEL as usize)
                .describe("The MIDI channel the pads are heard on; Omni hears all"),
        ];
        for &(id, name) in &NOTE_NAMES {
            parameters.push(
                ParameterDefinition::new(id, name, FOLLOW, 127.0, FOLLOW, ParameterDisplay::stepped(""))
                    .describe("This pad's own note, or -1 to follow pad 1"),
            );
        }
        for &(id, name) in &CHANNEL_NAMES {
            parameters.push(
                ParameterDefinition::new(id, name, FOLLOW, 16.0, FOLLOW, ParameterDisplay::stepped(""))
                    .describe("This pad's own channel (0 Omni, 1-16), or -1 to follow pad 1"),
            );
        }
        for &(id, name) in &PRESS_NAMES {
            parameters.push(
                ParameterDefinition::new(id, name, 0.0, 1.0, 0.0, ParameterDisplay::linear(""))
                    .describe("The pad pressed on the face, as hard as this"),
            );
        }
        Self {
            ports,
            parameters,
            pads: [Pad::IDLE; PADS],
            pressed: [0.0; PADS],
            mode: Mode::Momentary,
            channel_pressure: 0.0,
            hit: 0.0,
            pressure: 0.0,
            sample_rate: 44100.0,
        }
    }

    /// The Mode parameter's index.
    pub const PARAM_MODE: usize = 0;
    /// The Base parameter's index: pad 1's note.
    pub const PARAM_BASE: usize = 1;
    /// The Channel parameter's index: pad 1's channel.
    pub const PARAM_CHANNEL: usize = 2;
    const PARAM_NOTES: usize = 3;
    const PARAM_CHANNELS: usize = Self::PARAM_NOTES + PADS - 1;
    const PARAM_PRESSES: usize = Self::PARAM_CHANNELS + PADS - 1;

    /// The index of the parameter holding pad `pad`'s note (from 0): Base
    /// for pad 1, its own Note for the rest. A learn writes the note heard
    /// here.
    pub const fn note_param(pad: usize) -> usize {
        if pad == 0 { Self::PARAM_BASE } else { Self::PARAM_NOTES + pad - 1 }
    }

    /// The index of the parameter holding pad `pad`'s channel: Channel for
    /// pad 1, its own for the rest.
    pub const fn channel_param(pad: usize) -> usize {
        if pad == 0 { Self::PARAM_CHANNEL } else { Self::PARAM_CHANNELS + pad - 1 }
    }

    /// The index of pad `pad`'s Press parameter, which the face plays.
    pub const fn press_param(pad: usize) -> usize {
        Self::PARAM_PRESSES + pad
    }

    /// The name of pad `pad`'s Press parameter.
    pub fn press_name(pad: usize) -> &'static str {
        PRESS_NAMES[pad].1
    }

    /// The output index of each pad's gate, then Velocity, Hit and Pressure.
    pub const PORT_VELOCITY: usize = PADS;
    pub const PORT_HIT: usize = PADS + 1;
    pub const PORT_PRESSURE: usize = PADS + 2;

    /// The readout's places: each pad's gate (0 or 1), how hard it was
    /// last hit, the seconds since, and how hard it's pressed.
    pub const READOUT_GATE: usize = 0;
    pub const READOUT_VELOCITY: usize = PADS;
    pub const READOUT_QUIET: usize = 2 * PADS;
    pub const READOUT_PRESSURE: usize = 3 * PADS;

    /// Strikes pad `pad` at `velocity` (0 to 1), held down on the face or
    /// on the controller.
    fn hit(&mut self, pad: usize, velocity: f32, mode: Mode, trigger: u32, on_face: bool) {
        let p = &mut self.pads[pad];
        let was_open = p.open(mode) && p.rest == 0;
        if on_face {
            p.pressed = true;
        } else {
            p.held = true;
        }
        match mode {
            Mode::Momentary => {
                if was_open {
                    p.rest = 1;
                }
            }
            Mode::Latch => p.latched = !p.latched,
            Mode::Trigger => {
                if was_open {
                    p.rest = 1;
                }
                p.trigger_left = trigger;
            }
        }
        p.velocity = velocity;
        p.pressure = 0.0;
        p.quiet = 0.0;
        self.hit = velocity;
    }

    /// What one event does, other than a hit (see [`Self::hit_of`]).
    fn hear(&mut self, event: &MidiEvent, listens: &[Listen; PADS]) {
        match event.message {
            MidiMessage::NoteOff { note, .. } | MidiMessage::NoteOn { note, velocity: 0 } => {
                for (pad, listen) in self.pads.iter_mut().zip(listens) {
                    if listen.hears(event.channel, note) {
                        pad.held = false;
                        pad.pressure = 0.0;
                    }
                }
            }
            MidiMessage::PolyPressure { note, pressure } => {
                for (pad, listen) in self.pads.iter_mut().zip(listens) {
                    if listen.hears(event.channel, note) {
                        pad.pressure = pressure as f32 / 127.0;
                    }
                }
            }
            MidiMessage::Aftertouch { pressure } => {
                if listens.iter().any(|listen| listen.hears_channel(event.channel)) {
                    self.channel_pressure = pressure as f32 / 127.0;
                }
            }
            MidiMessage::ControlChange { controller: 120 | 123, .. } => {
                // All Sound Off, All Notes Off: every pad lets go
                for pad in &mut self.pads {
                    pad.held = false;
                    pad.pressure = 0.0;
                }
            }
            _ => {}
        }
    }

    /// The velocity `event` strikes a pad of `listens` at, if it's a hit.
    fn hit_of(event: &MidiEvent) -> Option<(u8, f32)> {
        match event.message {
            MidiMessage::NoteOn { note, velocity } if velocity > 0 && event.channel != KEYS_CHANNEL => Some((note, velocity as f32 / 127.0)),
            _ => None,
        }
    }

    /// Hears `event`: strikes every pad it's the note of, or does what else
    /// it says.
    fn take(&mut self, event: &MidiEvent, listens: &[Listen; PADS], mode: Mode, trigger: u32) {
        match Self::hit_of(event) {
            Some((note, velocity)) => {
                for pad in 0..PADS {
                    if listens[pad].hears(event.channel, note) {
                        self.hit(pad, velocity, mode, trigger, false);
                    }
                }
            }
            None => self.hear(event, listens),
        }
    }
}

impl Default for Pads {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Pads {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Pads",
            category: ModuleCategory::Source,
            description: "A MIDI pad controller's pads as eight gates, with velocity and pressure: finger drums, triggers and latches",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    /// Its outputs get room for a channel per pad, for Velocity.
    fn polyphonic(&self) -> bool {
        true
    }

    fn note_claims(&self) -> Option<fn(&[f32], &mut NoteClaims)> {
        Some(claim_notes)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
    }

    fn process(&mut self, _inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let mode = Mode::from_param(params.get(Self::PARAM_MODE).copied().unwrap_or(0.0));
        if mode != self.mode {
            // A new mode starts with every gate shut
            self.mode = mode;
            for pad in &mut self.pads {
                pad.latched = false;
                pad.trigger_left = 0;
                pad.rest = 0;
            }
        }
        let listens = listens(params);
        let trigger = ((TRIGGER_SECONDS * self.sample_rate).round() as u32).max(1);
        let slew = rate_for_seconds(PRESSURE_SLEW, self.sample_rate);
        let block_seconds = context.block_size as f32 / self.sample_rate;
        for pad in &mut self.pads {
            pad.quiet = (pad.quiet + block_seconds).min(QUIET);
        }

        // Pressed on the face, or let go, from the block's first sample
        for pad in 0..PADS {
            let press = params.get(Self::press_param(pad)).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            let was = self.pressed[pad];
            self.pressed[pad] = press;
            if press > 0.0 && was == 0.0 {
                self.hit(pad, press, mode, trigger, true);
            } else if press == 0.0 && was > 0.0 {
                self.pads[pad].pressed = false;
            }
        }

        for output in outputs.iter_mut() {
            output.set_channels(1);
        }
        let velocity_channels = if outputs.len() > Self::PORT_VELOCITY {
            outputs[Self::PORT_VELOCITY].set_channels(PADS.min(MAX_CHANNELS))
        } else {
            0
        };

        let mut events = context.midi.iter().peekable();
        for i in 0..context.block_size {
            // What lands on this sample, in order, but only one hit: the
            // next waits a sample, so each rises with its own Hit
            let mut struck = false;
            while let Some(event) = events.peek() {
                if event.sample_offset as usize > i || (struck && Self::hit_of(event).is_some()) {
                    break;
                }
                struck |= Self::hit_of(event).is_some_and(|(note, _)| listens.iter().any(|l| l.hears(event.channel, note)));
                self.take(event, &listens, mode, trigger);
                events.next();
            }

            let mut pressure = 0.0f32;
            for (index, pad) in self.pads.iter_mut().enumerate() {
                let gate = pad.rest == 0 && pad.open(mode);
                pad.rest = pad.rest.saturating_sub(1);
                pad.trigger_left = pad.trigger_left.saturating_sub(1);
                if pad.down() {
                    pressure = pressure.max(pad.pressure.max(self.channel_pressure));
                }
                if let Some(output) = outputs.get_mut(index) {
                    output.samples[i] = if gate { 1.0 } else { 0.0 };
                }
                pad.gate = gate;
            }
            if self.pads.iter().all(|pad| !pad.down()) {
                self.channel_pressure = 0.0;
            }
            self.pressure += (pressure - self.pressure) * slew;
            for pad in 0..velocity_channels {
                outputs[Self::PORT_VELOCITY].channel_mut(pad)[i] = self.pads[pad].velocity;
            }
            if let Some(output) = outputs.get_mut(Self::PORT_HIT) {
                output.samples[i] = self.hit;
            }
            if let Some(output) = outputs.get_mut(Self::PORT_PRESSURE) {
                output.samples[i] = self.pressure;
            }
        }
        // Anything placed past the block still counts, from the next one
        for event in events {
            self.take(event, &listens, mode, trigger);
        }
    }

    fn reset(&mut self) {
        for pad in &mut self.pads {
            *pad = Pad { held: pad.held, pressed: pad.pressed, ..Pad::IDLE };
        }
        self.channel_pressure = 0.0;
        self.hit = 0.0;
        self.pressure = 0.0;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        for (index, pad) in self.pads.iter().enumerate() {
            readout.values[Self::READOUT_GATE + index] = if pad.gate { 1.0 } else { 0.0 };
            readout.values[Self::READOUT_VELOCITY + index] = pad.velocity;
            readout.values[Self::READOUT_QUIET + index] = pad.quiet;
            readout.values[Self::READOUT_PRESSURE + index] = if pad.down() { pad.pressure.max(self.channel_pressure) } else { 0.0 };
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;
    /// Channel 10, as the wire has it.
    const TEN: u8 = 9;

    fn defaults() -> Vec<f32> {
        Pads::new().parameters().iter().map(|p| p.default).collect()
    }

    fn module() -> Pads {
        let mut module = Pads::new();
        module.prepare(SR, BLOCK);
        module
    }

    fn outputs() -> Vec<SignalBuffer> {
        let mut outputs: Vec<SignalBuffer> = (0..PADS).map(|_| SignalBuffer::polyphonic(BLOCK, SignalType::Gate)).collect();
        outputs.extend((0..3).map(|_| SignalBuffer::polyphonic(BLOCK, SignalType::Control)));
        outputs
    }

    fn run(module: &mut Pads, params: &[f32], midi: &[MidiEvent]) -> Vec<SignalBuffer> {
        let mut out = outputs();
        let context = ProcessContext::new(SR, BLOCK).with_midi(midi);
        module.process(&[], &mut out, params, &context);
        out
    }

    fn on(at: u32, channel: u8, note: u8, velocity: u8) -> MidiEvent {
        MidiEvent::new(at, channel, MidiMessage::NoteOn { note, velocity })
    }

    fn off(at: u32, channel: u8, note: u8) -> MidiEvent {
        MidiEvent::new(at, channel, MidiMessage::NoteOff { note, velocity: 0 })
    }

    /// The samples at which `samples` rises.
    fn rises(samples: &[f32]) -> Vec<usize> {
        let mut last = 0.0;
        let mut at = Vec::new();
        for (i, &s) in samples.iter().enumerate() {
            if s > 0.5 && last <= 0.5 {
                at.push(i);
            }
            last = s;
        }
        at
    }

    #[test]
    fn info_ports_and_parameters() {
        let pads = Pads::new();
        assert_eq!(pads.info().id, "input.pads");
        let names: Vec<&str> = pads.ports().iter().map(|p| p.name).collect();
        assert_eq!(&names[..2], ["Gate 1", "Gate 2"]);
        assert_eq!(&names[PADS..], ["Velocity", "Hit", "Pressure"]);
        assert!(pads.ports()[..PADS].iter().all(|p| p.is_output() && p.signal_type == SignalType::Gate));
        let params: Vec<&str> = pads.parameters().iter().map(|p| p.name).collect();
        assert_eq!(params[Pads::note_param(0)], "Base");
        assert_eq!(params[Pads::channel_param(0)], "Channel");
        assert_eq!(params[Pads::note_param(2)], "Note 3");
        assert_eq!(params[Pads::channel_param(7)], "Channel 8");
        assert_eq!(params[Pads::press_param(7)], "Press 8");
        assert_eq!(params.len(), Pads::press_param(7) + 1);
        for word in ["pad", "MIDI", "drums", "trigger"] {
            assert!(pads.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn the_default_pads_are_the_gm_drum_notes_on_channel_ten() {
        let listens = listens(&defaults());
        let notes: Vec<Option<u8>> = listens.iter().map(|l| l.note).collect();
        assert_eq!(notes, (36..44).map(Some).collect::<Vec<_>>());
        assert!(listens.iter().all(|l| l.channel == 10 && !l.own));

        let mut module = module();
        let out = run(&mut module, &defaults(), &[on(10, TEN, 38, 100), on(20, 0, 38, 100), on(30, TEN, 44, 100)]);
        assert_eq!(rises(&out[2].samples), [10], "pad 3 is the snare, on channel 10 only");
        assert!((0..PADS).filter(|&p| p != 2).all(|p| rises(&out[p].samples).is_empty()));
    }

    #[test]
    fn a_learned_base_and_channel_move_the_pads_and_an_own_note_stays() {
        let mut params = defaults();
        // As a learn from pad 1 writes them: note 60 on channel 1
        params[Pads::note_param(0)] = 60.0;
        params[Pads::channel_param(0)] = 1.0;
        // Pad 4 re-learned on its own: note 50 on channel 2
        params[Pads::note_param(3)] = 50.0;
        params[Pads::channel_param(3)] = 2.0;
        let listens = listens(&params);
        assert_eq!(listens[1], Listen { note: Some(61), channel: 1, own: false });
        assert_eq!(listens[3], Listen { note: Some(50), channel: 2, own: true });
        assert_eq!(listens[7].note, Some(67));

        let mut module = module();
        let out = run(&mut module, &params, &[on(0, 0, 61, 90), on(5, 1, 50, 90), on(9, 0, 63, 90)]);
        assert_eq!(rises(&out[1].samples), [0]);
        assert_eq!(rises(&out[3].samples), [5], "pad 4 on its own note and channel");
        assert!(rises(&out[2].samples).is_empty() && rises(&out[4].samples).is_empty(), "63 is no one's now");

        // Past the top of MIDI's notes, pads hear nothing
        params[Pads::note_param(0)] = 125.0;
        assert_eq!(super::listens(&params)[2].note, Some(127));
        assert_eq!(super::listens(&params)[4].note, None);
    }

    #[test]
    fn momentary_holds_while_the_pad_is_held_and_a_rehit_is_a_new_edge() {
        let mut module = module();
        let params = defaults();
        let out = run(&mut module, &params, &[on(10, TEN, 36, 100), off(100, TEN, 36), on(150, TEN, 36, 100), on(200, TEN, 36, 60)]);
        let gate = &out[0].samples;
        assert_eq!(gate[9], 0.0);
        assert!(gate[10..100].iter().all(|&s| s == 1.0), "high while held");
        assert!(gate[100..150].iter().all(|&s| s == 0.0), "low once let go");
        assert_eq!(rises(gate), [10, 150, 201], "struck again while held, it drops for a sample and rises");
        assert_eq!(gate[BLOCK - 1], 1.0);
    }

    #[test]
    fn latch_flips_with_each_hit_whatever_the_hold() {
        let mut module = module();
        let mut params = defaults();
        params[Pads::PARAM_MODE] = 1.0;
        let out = run(&mut module, &params, &[on(10, TEN, 37, 100), off(20, TEN, 37), on(100, TEN, 37, 100)]);
        let gate = &out[1].samples;
        assert!(gate[10..100].iter().all(|&s| s == 1.0), "open past its release");
        assert!(gate[100..].iter().all(|&s| s == 0.0), "the next hit closes it");
        // Held down through the next block, it stays as the hit left it
        let out = run(&mut module, &params, &[on(0, TEN, 37, 100)]);
        assert!(out[1].samples.iter().all(|&s| s == 1.0));
        let out = run(&mut module, &params, &[]);
        assert!(out[1].samples.iter().all(|&s| s == 1.0));
        assert_eq!(module.readout(&params).unwrap().values[Pads::READOUT_GATE + 1], 1.0, "the face shows it open");
    }

    #[test]
    fn trigger_is_a_short_gate_per_hit_however_long_it_is_held() {
        let mut module = module();
        let mut params = defaults();
        params[Pads::PARAM_MODE] = 2.0;
        let long = (TRIGGER_SECONDS * SR) as usize;
        let mut gate = Vec::new();
        gate.extend(run(&mut module, &params, &[on(10, TEN, 38, 100)])[2].samples.iter());
        gate.extend(run(&mut module, &params, &[])[2].samples.iter());
        assert_eq!(rises(&gate), [10]);
        assert_eq!(gate.iter().filter(|&&s| s == 1.0).count(), long, "5 ms, held or not");
        // Struck again inside its gate, it starts over on a new edge
        let out = run(&mut module, &params, &[on(0, TEN, 38, 100), on(100, TEN, 38, 100)]);
        assert_eq!(rises(&out[2].samples), [0, 101]);
    }

    #[test]
    fn velocity_is_a_channel_per_pad_and_hit_follows_each_hit() {
        let mut module = module();
        let params = defaults();
        let out = run(&mut module, &params, &[on(0, TEN, 36, 127), on(50, TEN, 39, 64)]);
        let velocity = &out[Pads::PORT_VELOCITY];
        assert_eq!(velocity.channels(), PADS);
        assert_eq!(velocity.voice(0).samples[BLOCK - 1], 1.0);
        assert!((velocity.voice(3).samples[BLOCK - 1] - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(velocity.voice(1).samples[BLOCK - 1], 0.0);
        let hit = &out[Pads::PORT_HIT].samples;
        assert_eq!(hit[49], 1.0);
        assert!((hit[50] - 64.0 / 127.0).abs() < 1e-6, "on the sample pad 4 rises");
    }

    #[test]
    fn hits_on_one_sample_are_struck_a_sample_apart_each_with_its_own_hit() {
        let mut module = module();
        let params = defaults();
        // Kick and snare together, as a two-finger hit sends them
        let out = run(&mut module, &params, &[on(20, TEN, 36, 127), on(20, TEN, 38, 40), off(20, TEN, 42)]);
        assert_eq!(rises(&out[0].samples), [20]);
        assert_eq!(rises(&out[2].samples), [21]);
        let hit = &out[Pads::PORT_HIT].samples;
        assert_eq!(hit[20], 1.0, "the kick's Accent reads its own hit");
        assert!((hit[21] - 40.0 / 127.0).abs() < 1e-6, "and the snare's its own");
    }

    #[test]
    fn pressure_is_the_hardest_held_pad_and_falls_when_let_go() {
        let mut module = module();
        let params = defaults();
        let poly = |at, note, pressure| MidiEvent::new(at, TEN, MidiMessage::PolyPressure { note, pressure });
        run(&mut module, &params, &[on(0, TEN, 36, 100), on(1, TEN, 37, 100), poly(2, 36, 127), poly(3, 37, 30)]);
        let mut last = 0.0;
        for _ in 0..20 {
            last = run(&mut module, &params, &[])[Pads::PORT_PRESSURE].samples[BLOCK - 1];
        }
        assert!((last - 1.0).abs() < 1e-3, "{last}");
        assert_eq!(module.readout(&params).unwrap().values[Pads::READOUT_PRESSURE], 1.0);
        // Pressure on another pad's note, or channel, is someone else's
        run(&mut module, &params, &[off(0, TEN, 36), poly(1, 60, 127), MidiEvent::new(2, 0, MidiMessage::Aftertouch { pressure: 127 })]);
        for _ in 0..20 {
            last = run(&mut module, &params, &[])[Pads::PORT_PRESSURE].samples[BLOCK - 1];
        }
        assert!((last - 30.0 / 127.0).abs() < 1e-3, "{last}");
        // Channel pressure on the pads' channel presses the held ones
        run(&mut module, &params, &[MidiEvent::new(0, TEN, MidiMessage::Aftertouch { pressure: 100 })]);
        for _ in 0..20 {
            last = run(&mut module, &params, &[])[Pads::PORT_PRESSURE].samples[BLOCK - 1];
        }
        assert!((last - 100.0 / 127.0).abs() < 1e-3, "{last}");
        run(&mut module, &params, &[off(0, TEN, 37)]);
        for _ in 0..20 {
            last = run(&mut module, &params, &[])[Pads::PORT_PRESSURE].samples[BLOCK - 1];
        }
        assert!(last < 1e-3, "nothing held, nothing pressed: {last}");
    }

    #[test]
    fn a_press_on_the_face_plays_the_pad() {
        let mut module = module();
        let mut params = defaults();
        params[Pads::press_param(5)] = 0.8;
        let out = run(&mut module, &params, &[]);
        assert_eq!(rises(&out[5].samples), [0]);
        assert_eq!(out[Pads::PORT_HIT].samples[0], 0.8);
        assert_eq!(module.readout(&params).unwrap().values[Pads::READOUT_QUIET + 5], 0.0, "just hit");
        let out = run(&mut module, &params, &[]);
        assert!(out[5].samples.iter().all(|&s| s == 1.0), "held while it's down");
        params[Pads::press_param(5)] = 0.0;
        let out = run(&mut module, &params, &[]);
        assert!(out[5].samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn the_keys_never_play_the_pads() {
        let mut module = module();
        let mut params = defaults();
        params[Pads::PARAM_CHANNEL] = 0.0;
        let out = run(&mut module, &params, &[on(0, KEYS_CHANNEL, 36, 100), on(1, 3, 37, 100)]);
        assert!(rises(&out[0].samples).is_empty(), "the computer keys play Note In");
        assert_eq!(rises(&out[1].samples), [1], "Omni hears every MIDI channel");
    }

    #[test]
    fn the_claims_are_the_pads_notes_on_their_channels() {
        let mut params = defaults();
        params[Pads::note_param(7)] = 70.0;
        params[Pads::channel_param(7)] = 0.0;
        let mut claims = NoteClaims::EMPTY;
        claim_notes(&params, &mut claims);
        assert!((36..43).all(|note| claims.claims(TEN, note)));
        assert!(!claims.claims(TEN, 43) && !claims.claims(0, 36));
        assert!((0..16).all(|channel| claims.claims(channel, 70)), "pad 8 on Omni claims its note everywhere");
    }
}
