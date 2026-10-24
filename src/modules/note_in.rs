//! Note In module.
//!
//! The one way to play notes into a patch: from a MIDI device, the computer
//! keys or the piano on its face, as pitch, gate and velocity. With one voice
//! it plays like a monosynth's keyboard: the Priority picks which held key
//! sounds, and letting go goes back to one still held. With more, each held
//! key plays on its own channel of cables that carry every voice at once, so
//! a chord through polyphonic modules is several independent voices.
//!
//! Notes reach it on the audio thread, through [`ProcessContext::midi`], so
//! every note starts on its sample, from a device or from the keys alike
//! (they come on [`KEYS_CHANNEL`], see [`crate::engine::ui_notes`]).
//!
//! On Omni it skips the notes a Pads module claims (#428), so hitting a pad
//! never also plays the synth. Set to a channel, it hears all of that
//! channel's notes, claimed or not.
//!
//! It was Poly MIDI, and keeps its ID, so Poly MIDI patches open as they
//! were. Keyboard and MIDI Note patches open as Note In with one voice (see
//! `persistence::patch`).

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{glide_parameters, rate_for_seconds, Glide, GlideMode},
    signal::SignalBuffer,
    MidiEvent, MidiMessage, ParameterDisplay, SignalType, KEYS_CHANNEL, MAX_CHANNELS,
};

/// CC 64, the sustain pedal.
const SUSTAIN_PEDAL: u8 = 64;
/// CC 120, All Sound Off.
const ALL_SOUND_OFF: u8 = 120;
/// CC 123, All Notes Off.
const ALL_NOTES_OFF: u8 = 123;

/// How quickly pitch bend follows the wheel. Wheels send steps a few
/// milliseconds apart; gliding between them keeps bends from zippering.
const BEND_SMOOTHING_SECONDS: f32 = 0.005;

/// Notes this soon after a note played from silence are one chord, struck
/// together: in Legato mode none of them glide.
const CHORD_SECONDS: f32 = 0.03;

/// How a new note picks its voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocation {
    /// Each note takes the next free voice in turn, so release tails ring
    /// on while new notes sound elsewhere.
    Rotate = 0,
    /// A note played again goes back to the voice that last played it, as a
    /// piano string is struck again; other notes take the voice that has
    /// been free longest.
    Reuse = 1,
}

impl Allocation {
    /// Converts from the parameter value.
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Allocation::Reuse,
            _ => Allocation::Rotate,
        }
    }
}

/// Which held key sounds, with one voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// The key pressed last.
    Last = 0,
    /// The lowest key.
    Low = 1,
    /// The highest key.
    High = 2,
}

impl Priority {
    /// Converts from the parameter value.
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Priority::Low,
            2 => Priority::High,
            _ => Priority::Last,
        }
    }
}

/// Where the notes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The keys and MIDI, on the Channel chosen.
    Both = 0,
    /// The computer keys and the on-screen pianos only.
    Keys = 1,
    /// MIDI only, on the Channel chosen.
    Midi = 2,
}

impl Source {
    /// Converts from the parameter value.
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Source::Keys,
            2 => Source::Midi,
            _ => Source::Both,
        }
    }

    /// Whether a module set to this source and `channel` (the Channel
    /// parameter: 0 is Omni, 1-16 a MIDI channel) hears `event`.
    pub fn hears(self, channel: f32, event: &MidiEvent) -> bool {
        let keys = event.channel == KEYS_CHANNEL;
        let channel = channel.round() as i32;
        let midi = !keys && (channel == 0 || channel - 1 == event.channel as i32);
        match self {
            Source::Both => keys || midi,
            Source::Keys => keys,
            Source::Midi => midi,
        }
    }
}

/// One voice: the note it plays and where its key is.
#[derive(Clone, Copy, Debug)]
struct Voice {
    /// The note the voice plays. Kept after release, so the release tail
    /// stays in tune.
    note: u8,
    /// The voice's pitch as it slides toward `note`, from the voice's own
    /// previous note.
    glide: Glide,
    /// The note's velocity (0.0-1.0).
    velocity: f32,
    /// The key is down.
    held: bool,
    /// The key is up but the sustain pedal holds the note.
    sustained: bool,
    /// When the note started, in note events: the oldest is stolen first.
    started: u64,
    /// When the voice last fell free, in note events.
    freed: u64,
    /// Keep the gate high for this sample even if the note already ended,
    /// so the shortest note still makes a rising edge.
    hold_gate: bool,
    /// Drop the gate for this sample, so a stolen or restruck voice makes a
    /// fresh rising edge.
    gap: bool,
}

impl Voice {
    const IDLE: Voice = Voice {
        note: 60,
        glide: Glide::NEW,
        velocity: 0.0,
        held: false,
        sustained: false,
        started: 0,
        freed: 0,
        hold_gate: false,
        gap: false,
    };

    /// Whether the voice is sounding a note (its gate is up).
    fn busy(&self) -> bool {
        self.held || self.sustained
    }

    /// The gate for the current sample. Clears the one-sample flags.
    fn take_gate(&mut self) -> bool {
        let gate = self.hold_gate || (self.busy() && !self.gap);
        self.hold_gate = false;
        self.gap = false;
        gate
    }

    /// The voice let go of its note, keeping its pitch for the release.
    fn released(self) -> Voice {
        Voice { note: self.note, glide: self.glide, ..Voice::IDLE }
    }
}

/// A key down, or let go while the pedal holds it, with one voice.
#[derive(Clone, Copy, Debug)]
struct Key {
    note: u8,
    velocity: f32,
    /// Let go, but held by the sustain pedal.
    sustained: bool,
}

/// The held keys with one voice, which plays the one the Priority picks.
/// The voice itself (its note, glide and velocity) is the first of
/// [`NoteIn::voices`], so the pitch carries on across a change of mode.
#[derive(Debug)]
struct Mono {
    /// Oldest first. Its capacity covers every MIDI note, so it never
    /// reallocates.
    keys: Vec<Key>,
    /// Whether any key is held (or sustained).
    gate: bool,
    /// Keep the gate high for this sample, so the shortest note still
    /// makes a rising edge.
    hold_gate: bool,
    /// Drop the gate for this sample, so a retriggered legato note makes a
    /// fresh rising edge.
    gap: bool,
}

impl Mono {
    fn new() -> Self {
        Self { keys: Vec::with_capacity(128), gate: false, hold_gate: false, gap: false }
    }

    /// The key that sounds under `priority`.
    fn active(&self, priority: Priority) -> Option<Key> {
        match priority {
            Priority::Last => self.keys.last().copied(),
            Priority::Low => self.keys.iter().copied().min_by_key(|key| key.note),
            Priority::High => self.keys.iter().copied().max_by_key(|key| key.note),
        }
    }

    /// The gate for the current sample. Clears the one-sample flags.
    fn take_gate(&mut self) -> bool {
        let gate = self.hold_gate || (self.gate && !self.gap);
        self.hold_gate = false;
        self.gap = false;
        gate
    }

    /// Lets go of every key at once.
    fn release_all(&mut self) {
        self.keys.clear();
        self.gate = false;
    }
}

/// The parameters that decide how a note event plays.
#[derive(Clone, Copy)]
struct Play {
    /// How many voices play: one is the mono mode.
    count: usize,
    allocation: Allocation,
    priority: Priority,
    retrigger: bool,
    glide_mode: GlideMode,
}

/// The module's ID, as patches name it: the ID Poly MIDI had.
pub const ID: &str = "input.poly_midi";

/// Notes, from a MIDI device or the computer keys, as pitch, gate and
/// velocity: mono, or polyphonic with voice allocation and stealing.
///
/// # Ports
///
/// **Outputs:**
/// - **Pitch** (Control, poly): V/Oct per voice, with pitch bend. 0.0 = C4.
/// - **Gate** (Gate, poly): High while the voice's key (or the sustain
///   pedal) holds its note.
/// - **Velocity** (Control, poly): Each voice's note velocity (0.0-1.0).
/// - **Aftertouch** (Control): Channel pressure (0.0-1.0), one channel
///   shared by every voice.
///
/// # Parameters
///
/// - **Channel** (Omni, 1-16): Which MIDI channel to listen to.
/// - **Voices** (1-8): How many channels the cables carry. One is mono.
/// - **Allocation** (Rotate, Reuse): How a new note picks its voice.
/// - **Octave** (-4 to +4): Octave shift.
/// - **Bend Range** (0-12 semitones): How far the pitch bend wheel bends.
/// - **Glide** (0-2 s): Time for each voice to slide from its own previous
///   note to its new one. 0 is off.
/// - **Glide Mode** (Always, Legato): Glide on every note, or only on notes
///   played while another key is held.
/// - **Priority** (Last, Low, High): With one voice, which held key sounds.
/// - **Retrigger**: With one voice, moving between held keys drops the gate
///   for one sample, so envelopes start again.
/// - **Source** (Both, Keys, MIDI): Where the notes come from.
/// - **Key Velocity** (0-1): How hard the computer keys and the piano play.
///
/// When every voice is busy, a new note steals one: a voice the sustain
/// pedal is holding if there is one, otherwise the oldest note.
pub struct NoteIn {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    voices: [Voice; MAX_CHANNELS],
    /// The held keys while there's one voice.
    mono: Mono,
    /// Whether the last block played one voice, so a change of mode lets
    /// go of what the other mode held.
    was_mono: bool,
    /// Where Rotate looks for a free voice next.
    next: usize,
    /// Counts note events, to order voices by age.
    clock: u64,
    /// Whether the sustain pedal is down.
    pedal: bool,
    /// Channel pressure (0.0-1.0).
    aftertouch: f32,
    /// Pitch bend as last received (-1.0 to 1.0).
    bend_target: f32,
    /// Pitch bend as heard, gliding toward `bend_target`.
    bend: f32,
    /// One-pole coefficient for the bend glide.
    bend_coeff: f32,
    /// Sample rate from the last prepare() call.
    sample_rate: f32,
    /// Samples left in which a new note joins the chord begun from silence.
    chord_window: u32,
}

impl NoteIn {
    /// Creates a new Note In module.
    pub fn new() -> Self {
        let mut module = Self {
            ports: vec![
                PortDefinition::output("pitch", "Pitch", SignalType::Control).describe("V/Oct, one pitch per voice, with pitch bend"),
                PortDefinition::output("gate", "Gate", SignalType::Gate).describe("High for each voice while its key is held; patch into an envelope's Gate"),
                PortDefinition::output("velocity", "Velocity", SignalType::Control).describe("How hard each voice's key was struck, 0 to 1"),
                PortDefinition::output("aftertouch", "Aftertouch", SignalType::Control).describe("Key pressure after the note starts, 0 to 1"),
            ],
            parameters: vec![
                ParameterDefinition::choice(
                    "channel",
                    "Channel",
                    &["Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"],
                    0,
                ).describe("Only respond to this MIDI channel; Omni hears all"),
                ParameterDefinition::new(
                    "voices",
                    "Voices",
                    1.0,
                    MAX_CHANNELS as f32,
                    MAX_CHANNELS as f32,
                    ParameterDisplay::stepped(""),
                ).describe("How many notes can sound at once; 1 plays one at a time, as a monosynth"),
                ParameterDefinition::choice("allocation", "Allocation", &["Rotate", "Reuse"], 0).describe("Which voice a new note takes: next in turn, or the longest free"),
                ParameterDefinition::new("octave", "Octave", -4.0, 4.0, 0.0, ParameterDisplay::stepped("oct")).describe("Shifts the notes up or down by octaves"),
                ParameterDefinition::new("bend_range", "Bend Range", 0.0, 12.0, 2.0, ParameterDisplay::stepped("st")).describe("Semitones the pitch bend wheel moves at full travel"),
            ],
            voices: [Voice::IDLE; MAX_CHANNELS],
            mono: Mono::new(),
            was_mono: false,
            next: 0,
            clock: 0,
            pedal: false,
            aftertouch: 0.0,
            bend_target: 0.0,
            bend: 0.0,
            bend_coeff: 1.0,
            sample_rate: 44100.0,
            chord_window: 0,
        };
        // After the others, so saved patches load unchanged
        module.parameters.extend(glide_parameters());
        module.parameters.extend([
            ParameterDefinition::choice("priority", "Priority", &["Last", "Low", "High"], 0).describe("With one voice, which key sounds when several are held"),
            ParameterDefinition::toggle("retrigger", "Retrigger", false).describe("With one voice, restarts the gate on notes played legato"),
            ParameterDefinition::choice("source", "Source", &["Both", "Keys", "MIDI"], 0).describe("Play from the computer keys and the piano, from MIDI, or both"),
            ParameterDefinition::normalized("key_velocity", "Key Velocity", crate::engine::UI_VELOCITY as f32 / 127.0)
                .describe("How hard the computer keys and the piano play, 0 to 1"),
        ]);
        module.prepare(44100.0, 256);
        module
    }

    /// Port index constants.
    const PORT_PITCH: usize = 0;
    const PORT_GATE: usize = 1;
    const PORT_VELOCITY: usize = 2;
    const PORT_AFTERTOUCH: usize = 3;

    /// Parameter index constants.
    pub const PARAM_CHANNEL: usize = 0;
    pub const PARAM_VOICES: usize = 1;
    pub const PARAM_ALLOCATION: usize = 2;
    pub const PARAM_OCTAVE: usize = 3;
    pub const PARAM_BEND_RANGE: usize = 4;
    pub const PARAM_GLIDE: usize = 5;
    pub const PARAM_GLIDE_MODE: usize = 6;
    pub const PARAM_PRIORITY: usize = 7;
    pub const PARAM_RETRIGGER: usize = 8;
    pub const PARAM_SOURCE: usize = 9;
    pub const PARAM_KEY_VELOCITY: usize = 10;

    /// Converts a MIDI note number to V/Oct pitch: middle C (60) is 0.0,
    /// C5 (72) +1.0 and C3 (48) -1.0.
    #[inline]
    pub fn midi_to_voct(midi_note: f32) -> f32 {
        (midi_note - 60.0) / 12.0
    }

    /// The next tick of the note-event clock.
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Applies one event. A key's note plays at `key_velocity`.
    fn handle(&mut self, event: &MidiEvent, play: Play, key_velocity: f32) {
        match event.message {
            MidiMessage::NoteOn { note, velocity } if velocity > 0 => {
                let velocity = if event.channel == KEYS_CHANNEL { key_velocity } else { velocity as f32 / 127.0 };
                if play.count == 1 {
                    self.mono_note_on(note, velocity, play);
                } else {
                    self.note_on(note, velocity, play);
                }
            }
            // Note On at velocity 0 is a Note Off
            MidiMessage::NoteOn { note, .. } | MidiMessage::NoteOff { note, .. } => {
                if play.count == 1 {
                    self.mono_note_off(note, play);
                } else {
                    self.note_off(note, play.count);
                }
            }
            MidiMessage::PitchBend { value } => {
                self.bend_target = (value as f32 / 8192.0).clamp(-1.0, 1.0);
            }
            MidiMessage::Aftertouch { pressure } => {
                self.aftertouch = pressure as f32 / 127.0;
            }
            MidiMessage::ControlChange { controller: SUSTAIN_PEDAL, value } => {
                self.pedal = value >= 64;
                if !self.pedal {
                    let now = self.tick();
                    for voice in self.voices.iter_mut().filter(|voice| voice.sustained) {
                        voice.sustained = false;
                        voice.freed = now;
                    }
                    self.mono.keys.retain(|key| !key.sustained);
                    if play.count == 1 {
                        self.choose_mono_note(play);
                    }
                }
            }
            MidiMessage::ControlChange { controller: ALL_NOTES_OFF | ALL_SOUND_OFF, .. } => {
                self.pedal = false;
                for voice in &mut self.voices {
                    voice.held = false;
                    voice.sustained = false;
                }
                self.mono.release_all();
            }
            _ => {}
        }
    }

    fn note_on(&mut self, note: u8, velocity: f32, play: Play) {
        let count = play.count;
        // A key held from before this chord: Legato mode glides
        let key_held = self.voices[..count].iter().any(|voice| voice.held);
        if !key_held {
            self.chord_window = (CHORD_SECONDS * self.sample_rate) as u32;
        }
        let legato = key_held && self.chord_window == 0;
        let index = self.allocate(note, count, play.allocation);
        let started = self.tick();
        let voice = &mut self.voices[index];
        if voice.busy() {
            // Stolen or struck again: a gap first, so envelopes restart
            voice.gap = true;
        } else {
            voice.hold_gate = true;
        }
        // The voice glides from wherever it was, stolen or not
        voice.glide.start(note as f32, play.glide_mode.glides(legato));
        voice.note = note;
        voice.velocity = velocity;
        voice.held = true;
        voice.sustained = false;
        voice.started = started;
    }

    /// Picks the voice (among the first `count`) to play `note`.
    fn allocate(&mut self, note: u8, count: usize, allocation: Allocation) -> usize {
        let voices = &self.voices[..count];

        // A note still sounding is struck again on its own voice
        if let Some(index) = voices.iter().position(|voice| voice.busy() && voice.note == note) {
            return index;
        }

        let free = match allocation {
            Allocation::Rotate => (0..count).map(|k| (self.next + k) % count).find(|&i| !voices[i].busy()),
            Allocation::Reuse => voices
                .iter()
                .position(|voice| !voice.busy() && voice.note == note)
                .or_else(|| {
                    (0..count).filter(|&i| !voices[i].busy()).min_by_key(|&i| voices[i].freed)
                }),
        };
        if let Some(index) = free {
            self.next = (index + 1) % count;
            return index;
        }

        // Every voice is busy: steal one the pedal is holding, else the
        // oldest note
        (0..count)
            .min_by_key(|&i| (voices[i].held, voices[i].started))
            .unwrap_or(0)
    }

    fn note_off(&mut self, note: u8, count: usize) {
        let now = self.tick();
        let pedal = self.pedal;
        for voice in self.voices[..count].iter_mut().filter(|voice| voice.held && voice.note == note) {
            voice.held = false;
            if pedal {
                voice.sustained = true;
            } else {
                voice.freed = now;
            }
        }
    }

    fn mono_note_on(&mut self, note: u8, velocity: f32, play: Play) {
        self.mono.keys.retain(|key| key.note != note);
        if self.mono.keys.len() < self.mono.keys.capacity() {
            self.mono.keys.push(Key { note, velocity, sustained: false });
        }
        self.choose_mono_note(play);
    }

    fn mono_note_off(&mut self, note: u8, play: Play) {
        if self.pedal {
            for key in self.mono.keys.iter_mut().filter(|key| key.note == note) {
                key.sustained = true;
            }
        } else {
            self.mono.keys.retain(|key| key.note != note);
            self.choose_mono_note(play);
        }
    }

    /// Updates the sounding note and gate after the held keys changed.
    fn choose_mono_note(&mut self, play: Play) {
        let Some(key) = self.mono.active(play.priority) else {
            self.mono.gate = false;
            return;
        };
        let voice = &mut self.voices[0];
        if !self.mono.gate {
            self.mono.gate = true;
            self.mono.hold_gate = true;
            // No key was held: in Legato mode the note starts on its pitch
            voice.glide.start(key.note as f32, play.glide_mode.glides(false));
        } else if key.note == voice.note {
            // Still the same key sounding: nothing to change
            return;
        } else if play.retrigger {
            self.mono.gap = true;
        }
        voice.note = key.note;
        voice.velocity = key.velocity;
    }
}

impl Default for NoteIn {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for NoteIn {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Note In",
            category: ModuleCategory::Source,
            description: "Play notes, mono or polyphonic, from the computer keyboard, its piano or a MIDI keyboard",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.bend_coeff = rate_for_seconds(BEND_SMOOTHING_SECONDS, sample_rate);
    }

    fn process(
        &mut self,
        _inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let channel = params[Self::PARAM_CHANNEL];
        let source = Source::from_param(params[Self::PARAM_SOURCE]);
        let key_velocity = params[Self::PARAM_KEY_VELOCITY].clamp(0.0, 1.0);
        let octave = params[Self::PARAM_OCTAVE].round();
        let bend_range = params[Self::PARAM_BEND_RANGE];
        let glide = Glide::coefficient(params[Self::PARAM_GLIDE], self.sample_rate);

        // As many voices as the knob asks for and the cables have room for
        let room = outputs[..Self::PORT_AFTERTOUCH].iter().map(|output| output.max_channels()).min().unwrap_or(1);
        let count = (params[Self::PARAM_VOICES].round() as usize).clamp(1, room);
        let play = Play {
            count,
            allocation: Allocation::from_param(params[Self::PARAM_ALLOCATION]),
            priority: Priority::from_param(params[Self::PARAM_PRIORITY]),
            retrigger: params[Self::PARAM_RETRIGGER] > 0.5,
            glide_mode: GlideMode::from_param(params[Self::PARAM_GLIDE_MODE]),
        };
        for output in &mut outputs[..Self::PORT_AFTERTOUCH] {
            output.set_channels(count);
        }
        // A change of mode lets go of what the other mode held
        let mono = count == 1;
        if mono != self.was_mono {
            self.was_mono = mono;
            self.mono.release_all();
            for voice in &mut self.voices {
                *voice = voice.released();
            }
        }
        // Voices turned off let go of their notes
        for voice in &mut self.voices[count..] {
            *voice = voice.released();
        }
        self.next %= count;

        // On Omni, the pads' notes are theirs. Only a strike is skipped: a
        // release still lets go, so a note held when a pad claimed it ends
        let omni = channel.round() == 0.0;
        let claimed = |event: &MidiEvent| match event.message {
            MidiMessage::NoteOn { note, velocity } if velocity > 0 => omni && context.claimed_notes.claims(event.channel, note),
            _ => false,
        };
        let mut events = context
            .midi
            .iter()
            .filter(|event| source.hears(channel, event) && !claimed(event))
            .peekable();

        for i in 0..context.block_size {
            // Everything that lands on this sample, in order
            while let Some(event) = events.next_if(|event| event.sample_offset as usize <= i) {
                self.handle(event, play, key_velocity);
            }

            self.chord_window = self.chord_window.saturating_sub(1);
            self.bend += (self.bend_target - self.bend) * self.bend_coeff;
            // Bend rides on top of each voice's glide, unslewed by it
            let shift = octave * 12.0 + self.bend * bend_range;

            if mono {
                let gate = self.mono.take_gate();
                let voice = &mut self.voices[0];
                outputs[Self::PORT_PITCH].samples[i] = Self::midi_to_voct(voice.glide.next(voice.note as f32, glide) + shift);
                outputs[Self::PORT_GATE].samples[i] = if gate { 1.0 } else { 0.0 };
                outputs[Self::PORT_VELOCITY].samples[i] = voice.velocity;
            } else {
                for (index, voice) in self.voices[..count].iter_mut().enumerate() {
                    let gate = voice.take_gate();
                    outputs[Self::PORT_PITCH].channel_mut(index)[i] = Self::midi_to_voct(voice.glide.next(voice.note as f32, glide) + shift);
                    outputs[Self::PORT_GATE].channel_mut(index)[i] = if gate { 1.0 } else { 0.0 };
                    outputs[Self::PORT_VELOCITY].channel_mut(index)[i] = voice.velocity;
                }
            }
            outputs[Self::PORT_AFTERTOUCH].samples[i] = self.aftertouch;
        }

        // Anything placed past the block still counts, from the next one
        for event in events {
            self.handle(event, play, key_velocity);
        }
    }

    fn reset(&mut self) {
        self.voices = [Voice::IDLE; MAX_CHANNELS];
        self.mono.release_all();
        self.mono.hold_gate = false;
        self.mono.gap = false;
        self.next = 0;
        self.clock = 0;
        self.pedal = false;
        self.aftertouch = 0.0;
        self.bend_target = 0.0;
        self.bend = 0.0;
        self.chord_window = 0;
    }

    fn polyphonic(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;
    const KEY_VELOCITY: f32 = 100.0 / 127.0;

    /// Default parameters: Omni, 8 voices, Rotate, octave 0, ±2 st, no glide
    /// (Legato), Last, no retrigger, both sources, key velocity 100.
    const DEFAULTS: [f32; 11] = [0.0, 8.0, 0.0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 0.0, KEY_VELOCITY];

    fn params(voices: usize, allocation: Allocation) -> [f32; 11] {
        let mut p = DEFAULTS;
        p[NoteIn::PARAM_VOICES] = voices as f32;
        p[NoteIn::PARAM_ALLOCATION] = allocation as i32 as f32;
        p
    }

    fn module() -> NoteIn {
        let mut module = NoteIn::new();
        module.prepare(SR, BLOCK);
        module
    }

    fn run(module: &mut NoteIn, params: &[f32], midi: &[MidiEvent]) -> Vec<SignalBuffer> {
        let mut out = vec![
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
            SignalBuffer::polyphonic(BLOCK, SignalType::Gate),
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
        ];
        let context = ProcessContext::new(SR, BLOCK).with_midi(midi);
        module.process(&[], &mut out, params, &context);
        out
    }

    fn on(at: u32, note: u8) -> MidiEvent {
        MidiEvent::note_on(at, 0, note, 100)
    }

    fn off(at: u32, note: u8) -> MidiEvent {
        MidiEvent::note_off(at, 0, note, 0)
    }

    fn cc(at: u32, controller: u8, value: u8) -> MidiEvent {
        MidiEvent::new(at, 0, MidiMessage::ControlChange { controller, value })
    }

    /// The note each voice plays at sample `i`, or `None` if its gate is low.
    fn sounding(out: &[SignalBuffer], i: usize) -> Vec<Option<u8>> {
        (0..out[NoteIn::PORT_GATE].channels())
            .map(|v| {
                let gate = out[NoteIn::PORT_GATE].voice(v).samples[i] > 0.5;
                let note = (out[NoteIn::PORT_PITCH].voice(v).samples[i] * 12.0 + 60.0).round() as u8;
                gate.then_some(note)
            })
            .collect()
    }

    fn rising_edges(gate: &[f32]) -> Vec<usize> {
        let mut previous = 0.0;
        let mut edges = Vec::new();
        for (i, &g) in gate.iter().enumerate() {
            if g > 0.5 && previous <= 0.5 {
                edges.push(i);
            }
            previous = g;
        }
        edges
    }

    #[test]
    fn test_note_in_ports_and_parameters() {
        let module = NoteIn::new();
        assert_eq!(module.info().id, "input.poly_midi");
        assert_eq!(module.info().name, "Note In");
        assert_eq!(module.info().category, ModuleCategory::Source);
        assert!(module.polyphonic());
        let ports: Vec<&str> = module.ports().iter().map(|p| p.id).collect();
        assert_eq!(ports, ["pitch", "gate", "velocity", "aftertouch"]);
        let ids: Vec<&str> = module.parameters().iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            ["channel", "voices", "allocation", "octave", "bend_range", "glide", "glide_mode", "priority", "retrigger", "source", "key_velocity"]
        );
        let defaults: Vec<f32> = module.parameters().iter().map(|p| p.default).collect();
        assert_eq!(defaults, DEFAULTS);
    }

    #[test]
    fn test_midi_to_voct() {
        assert_eq!(NoteIn::midi_to_voct(60.0), 0.0);
        assert_eq!(NoteIn::midi_to_voct(72.0), 1.0);
        assert_eq!(NoteIn::midi_to_voct(48.0), -1.0);
        assert!((NoteIn::midi_to_voct(69.0) - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_chord_spreads_across_voices() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[on(0, 60), on(0, 64), on(0, 67), on(0, 72)]);
        assert_eq!(out[NoteIn::PORT_PITCH].channels(), 8);
        assert_eq!(out[NoteIn::PORT_AFTERTOUCH].channels(), 1);
        assert_eq!(
            sounding(&out, 0),
            [Some(60), Some(64), Some(67), Some(72), None, None, None, None]
        );
    }

    #[test]
    fn test_releasing_one_note_leaves_the_others() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[on(0, 60), on(0, 64), on(0, 67), off(100, 64)]);
        assert_eq!(sounding(&out, 99)[..3], [Some(60), Some(64), Some(67)]);
        assert_eq!(sounding(&out, 100)[..3], [Some(60), None, Some(67)]);
        // The released voice keeps its pitch for the release tail
        assert_eq!(out[NoteIn::PORT_PITCH].voice(1).samples[200], 4.0 / 12.0);
    }

    #[test]
    fn test_rotate_moves_on_to_the_next_voice() {
        let mut m = module();
        let p = params(4, Allocation::Rotate);
        let out = run(&mut m, &p, &[on(0, 60), off(10, 60), on(20, 60), off(30, 60), on(40, 62)]);
        assert_eq!(sounding(&out, 5), [Some(60), None, None, None]);
        assert_eq!(sounding(&out, 25), [None, Some(60), None, None], "same note, next voice");
        assert_eq!(sounding(&out, 45), [None, None, Some(62), None]);
    }

    #[test]
    fn test_reuse_returns_a_note_to_its_voice() {
        let mut m = module();
        let p = params(4, Allocation::Reuse);
        let out = run(
            &mut m,
            &p,
            &[on(0, 60), on(0, 62), off(10, 60), off(10, 62), on(20, 62), on(30, 65)],
        );
        assert_eq!(sounding(&out, 5), [Some(60), Some(62), None, None]);
        assert_eq!(sounding(&out, 25), [None, Some(62), None, None], "62 back on its voice");
        // A new note takes the voice free longest: 3 and 4 never played
        assert_eq!(sounding(&out, 35)[2..], [Some(65), None]);
    }

    #[test]
    fn test_full_voices_steal_the_oldest_note() {
        let mut m = module();
        let p = params(2, Allocation::Rotate);
        let out = run(&mut m, &p, &[on(0, 60), on(10, 64), on(20, 67)]);
        assert_eq!(sounding(&out, 15), [Some(60), Some(64)]);
        // The stolen voice drops for one sample, then plays the new note
        assert_eq!(sounding(&out, 20), [None, Some(64)]);
        assert_eq!(sounding(&out, 21), [Some(67), Some(64)]);
        assert_eq!(rising_edges(&out[NoteIn::PORT_GATE].voice(0).samples), vec![0, 21]);
    }

    #[test]
    fn test_sustain_pedal_holds_notes_and_is_stolen_first() {
        let mut m = module();
        let p = params(2, Allocation::Rotate);
        let out = run(
            &mut m,
            &p,
            &[cc(0, SUSTAIN_PEDAL, 127), on(0, 60), off(10, 60), on(20, 64), on(30, 67)],
        );
        assert_eq!(sounding(&out, 15), [Some(60), None], "the pedal holds 60");
        // Both voices busy: 67 takes the pedalled 60 rather than the held 64
        assert_eq!(sounding(&out, 31), [Some(67), Some(64)]);

        let out = run(&mut m, &p, &[off(0, 67), cc(50, SUSTAIN_PEDAL, 0)]);
        assert_eq!(sounding(&out, 25), [Some(67), Some(64)]);
        assert_eq!(sounding(&out, 50), [None, Some(64)], "pedal up lets 67 go");
    }

    #[test]
    fn test_restriking_a_held_note_retriggers_its_voice() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[cc(0, SUSTAIN_PEDAL, 127), on(0, 60), off(10, 60), on(20, 60)]);
        assert_eq!(rising_edges(&out[NoteIn::PORT_GATE].voice(0).samples), vec![0, 21]);
        assert!(sounding(&out, 30)[1..].iter().all(Option::is_none), "no second voice");
    }

    #[test]
    fn test_velocity_per_voice() {
        let mut m = module();
        let out = run(
            &mut m,
            &DEFAULTS,
            &[MidiEvent::note_on(0, 0, 60, 127), MidiEvent::note_on(0, 0, 64, 0x40)],
        );
        let velocity = &out[NoteIn::PORT_VELOCITY];
        assert_eq!(velocity.voice(0).samples[0], 1.0);
        assert!((velocity.voice(1).samples[0] - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn test_shortest_note_still_triggers() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[on(10, 60), off(10, 60)]);
        assert_eq!(rising_edges(&out[NoteIn::PORT_GATE].samples), vec![10]);
        assert_eq!(out[NoteIn::PORT_GATE].samples[11], 0.0);
    }

    #[test]
    fn test_pitch_bend_bends_every_voice() {
        let mut m = module();
        run(&mut m, &DEFAULTS, &[on(0, 60), on(0, 67), MidiEvent::new(0, 0, MidiMessage::PitchBend { value: 8191 })]);
        for _ in 0..20 {
            run(&mut m, &DEFAULTS, &[]);
        }
        let out = run(&mut m, &DEFAULTS, &[]);
        let pitch = &out[NoteIn::PORT_PITCH];
        assert!((pitch.voice(0).samples[BLOCK - 1] * 12.0 - 2.0).abs() < 0.01);
        assert!((pitch.voice(1).samples[BLOCK - 1] * 12.0 - 9.0).abs() < 0.01);
    }

    #[test]
    fn test_fewer_voices_release_the_rest() {
        let mut m = module();
        run(&mut m, &DEFAULTS, &[on(0, 60), on(0, 64), on(0, 67)]);
        let out = run(&mut m, &params(2, Allocation::Rotate), &[]);
        assert_eq!(out[NoteIn::PORT_GATE].channels(), 2);

        // Back to 8: voice 3's note was let go, not left hanging
        let out = run(&mut m, &DEFAULTS, &[]);
        assert_eq!(sounding(&out, 0)[..3], [Some(60), Some(64), None]);
    }

    #[test]
    fn test_channel_filter_and_all_notes_off() {
        let mut m = module();
        let mut ch1_only = DEFAULTS;
        ch1_only[NoteIn::PARAM_CHANNEL] = 1.0;
        let out = run(&mut m, &ch1_only, &[MidiEvent::note_on(0, 1, 60, 100), on(0, 64)]);
        assert_eq!(sounding(&out, 0)[..2], [Some(64), None]);

        let out = run(&mut m, &ch1_only, &[cc(10, ALL_NOTES_OFF, 0)]);
        assert!(sounding(&out, 10).iter().all(Option::is_none));
    }

    fn glide_params(voices: usize, seconds: f32, mode: GlideMode) -> [f32; 11] {
        let mut p = params(voices, Allocation::Rotate);
        p[NoteIn::PARAM_GLIDE] = seconds;
        p[NoteIn::PARAM_GLIDE_MODE] = mode as i32 as f32;
        p
    }

    /// Voice `v`'s pitch at sample `i`, in semitones from C4.
    fn semitones(out: &[SignalBuffer], v: usize, i: usize) -> f32 {
        out[NoteIn::PORT_PITCH].voice(v).samples[i] * 12.0
    }

    #[test]
    fn test_each_voice_glides_from_its_own_note() {
        let p = glide_params(2, 0.1, GlideMode::Always);
        let mut m = module();
        // C and E on voices 1 and 2, released; then G and A take them in turn
        run(&mut m, &p, &[on(0, 60), on(0, 64), off(10, 60), off(10, 64)]);
        let out = run(&mut m, &p, &[on(0, 67), on(0, 69)]);
        assert!(semitones(&out, 0, 0) < 0.1, "G slides up from C");
        assert!((semitones(&out, 1, 0) - 4.0).abs() < 0.1, "A slides up from E");
        for _ in 0..30 {
            run(&mut m, &p, &[]);
        }
        let out = run(&mut m, &p, &[]);
        assert!((semitones(&out, 0, 0) - 7.0).abs() < 0.01);
        assert!((semitones(&out, 1, 0) - 9.0).abs() < 0.01);
    }

    #[test]
    fn test_stolen_voice_glides_from_where_it_was() {
        let p = glide_params(2, 0.5, GlideMode::Legato);
        let mut m = module();
        // Both voices held; G, played after the chord, steals C's voice and
        // slides up from C
        run(&mut m, &p, &[on(0, 60), on(0, 64)]);
        for _ in 0..10 {
            run(&mut m, &p, &[]);
        }
        let out = run(&mut m, &p, &[on(100, 67)]);
        assert!(semitones(&out, 0, 101) < 0.1);
        assert!(semitones(&out, 0, BLOCK - 1) > semitones(&out, 0, 101));
        assert_eq!(semitones(&out, 1, BLOCK - 1), 4.0, "E is left alone");
    }

    #[test]
    fn test_legato_chord_from_silence_doesnt_glide() {
        let p = glide_params(3, 0.5, GlideMode::Legato);
        let mut m = module();
        run(&mut m, &p, &[on(0, 60), on(0, 64), off(10, 60), off(10, 64)]);
        // G takes voice 3 and C5 voice 1, 4 ms apart: one chord, no slides
        let out = run(&mut m, &p, &[on(5, 67), on(200, 72)]);
        assert_eq!(semitones(&out, 2, 5), 7.0);
        assert_eq!(semitones(&out, 0, 200), 12.0);

        // A note added once the chord is down glides, from its voice's last
        // note (E4)
        for _ in 0..10 {
            run(&mut m, &p, &[]);
        }
        let out = run(&mut m, &p, &[on(10, 76)]);
        assert!((semitones(&out, 1, 10) - 4.0).abs() < 0.1, "E5 slides up from E4");
    }

    #[test]
    fn test_zero_glide_matches_no_glide() {
        let mut m = module();
        let mut p = glide_params(8, 0.0, GlideMode::Always);
        p[NoteIn::PARAM_OCTAVE] = -1.0;
        let bend = MidiEvent::new(0, 0, MidiMessage::PitchBend { value: -3000 });
        let out = run(&mut m, &p, &[on(0, 60), on(0, 67), bend, off(20, 60), on(40, 62)]);
        let mut b = 0.0f32;
        let coeff = rate_for_seconds(BEND_SMOOTHING_SECONDS, SR);
        for i in 0..BLOCK {
            b += (-3000.0 / 8192.0 - b) * coeff;
            let shift = -12.0 + b * 2.0;
            let pitch = &out[NoteIn::PORT_PITCH];
            assert_eq!(pitch.voice(1).samples[i], NoteIn::midi_to_voct(67.0 + shift));
            if i >= 40 {
                assert_eq!(pitch.voice(2).samples[i], NoteIn::midi_to_voct(62.0 + shift));
            }
        }
    }

    #[test]
    fn test_reset_releases_everything() {
        let mut m = module();
        run(&mut m, &DEFAULTS, &[on(0, 60), on(0, 64)]);
        m.reset();
        let out = run(&mut m, &DEFAULTS, &[]);
        assert!(sounding(&out, 0).iter().all(Option::is_none));
    }

    // Where the notes come from: the keys, MIDI or both

    fn key_on(at: u32, note: u8) -> MidiEvent {
        MidiEvent::note_on(at, KEYS_CHANNEL, note, 100)
    }

    fn key_off(at: u32, note: u8) -> MidiEvent {
        MidiEvent::note_off(at, KEYS_CHANNEL, note, 0)
    }

    fn with_source(source: Source) -> [f32; 11] {
        let mut p = DEFAULTS;
        p[NoteIn::PARAM_SOURCE] = source as i32 as f32;
        p
    }

    #[test]
    fn test_source_picks_the_keys_midi_or_both() {
        let notes = [key_on(0, 60), on(0, 64)];
        let heard = |source| {
            let mut m = module();
            let out = run(&mut m, &with_source(source), &notes);
            sounding(&out, 0).into_iter().flatten().collect::<Vec<_>>()
        };
        assert_eq!(heard(Source::Both), [60, 64]);
        assert_eq!(heard(Source::Keys), [60]);
        assert_eq!(heard(Source::Midi), [64]);
    }

    #[test]
    fn test_the_channel_filters_midi_not_the_keys() {
        let mut p = DEFAULTS;
        p[NoteIn::PARAM_CHANNEL] = 2.0;
        let mut m = module();
        let out = run(&mut m, &p, &[key_on(0, 60), on(0, 64), MidiEvent::note_on(0, 1, 67, 100)]);
        assert_eq!(sounding(&out, 0).into_iter().flatten().collect::<Vec<_>>(), [60, 67]);

        // Omni hears every MIDI channel, and only the keys come on theirs
        assert!(Source::Midi.hears(0.0, &MidiEvent::note_on(0, 15, 60, 100)));
        assert!(!Source::Midi.hears(0.0, &key_on(0, 60)));
    }

    #[test]
    fn test_omni_skips_the_notes_pads_claim_and_a_channel_hears_them() {
        let mut claims = crate::dsp::NoteClaims::EMPTY;
        claims.claim(Some(9), 36);
        let play = |channel: f32, midi: &[MidiEvent]| {
            let mut p = DEFAULTS;
            p[NoteIn::PARAM_CHANNEL] = channel;
            let mut m = module();
            let mut out = vec![
                SignalBuffer::polyphonic(BLOCK, SignalType::Control),
                SignalBuffer::polyphonic(BLOCK, SignalType::Gate),
                SignalBuffer::polyphonic(BLOCK, SignalType::Control),
                SignalBuffer::polyphonic(BLOCK, SignalType::Control),
            ];
            let context = ProcessContext::new(SR, BLOCK).with_midi(midi).with_claims(&claims);
            m.process(&[], &mut out, &p, &context);
            sounding(&out, BLOCK - 1).into_iter().flatten().collect::<Vec<_>>()
        };
        // A pad on channel 10, a key on 1, the same note on channel 2
        let midi = [MidiEvent::note_on(0, 9, 36, 100), MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_on(0, 1, 36, 100)];
        assert_eq!(play(0.0, &midi), [60, 36], "Omni skips the pad, and hears 36 where no pad claims it");
        assert_eq!(play(10.0, &midi), [36], "set to channel 10, it hears the pad's note");
        // The computer keys are never claimed
        assert_eq!(play(0.0, &[key_on(0, 36)]), [36]);
        // A release isn't skipped, so a note held before the claim ends
        let mut p = DEFAULTS;
        p[NoteIn::PARAM_CHANNEL] = 0.0;
        let mut m = module();
        run(&mut m, &p, &[MidiEvent::note_on(0, 9, 36, 100)]);
        let mut out = vec![
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
            SignalBuffer::polyphonic(BLOCK, SignalType::Gate),
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
        ];
        let off = [MidiEvent::new(0, 9, MidiMessage::NoteOff { note: 36, velocity: 0 })];
        m.process(&[], &mut out, &p, &ProcessContext::new(SR, BLOCK).with_midi(&off).with_claims(&claims));
        assert!(sounding(&out, BLOCK - 1).into_iter().flatten().next().is_none());
    }

    #[test]
    fn test_keys_play_at_the_key_velocity() {
        let mut p = DEFAULTS;
        p[NoteIn::PARAM_KEY_VELOCITY] = 0.5;
        let mut m = module();
        let out = run(&mut m, &p, &[key_on(0, 60), MidiEvent::note_on(0, 0, 64, 127)]);
        let velocity = &out[NoteIn::PORT_VELOCITY];
        assert_eq!(velocity.voice(0).samples[0], 0.5, "a key, at Key Velocity");
        assert_eq!(velocity.voice(1).samples[0], 1.0, "MIDI, at its own");
    }

    // One voice: MIDI Note's held keys, Priority and Retrigger

    /// One voice, with `priority` and `retrigger`.
    fn mono(priority: Priority, retrigger: bool) -> [f32; 11] {
        let mut p = DEFAULTS;
        p[NoteIn::PARAM_VOICES] = 1.0;
        p[NoteIn::PARAM_PRIORITY] = priority as i32 as f32;
        p[NoteIn::PARAM_RETRIGGER] = if retrigger { 1.0 } else { 0.0 };
        p
    }

    /// One voice, Last priority, no retrigger.
    fn one_voice() -> [f32; 11] {
        mono(Priority::Last, false)
    }

    /// One voice with a glide of `seconds` in `mode`.
    fn mono_glide(seconds: f32, mode: GlideMode) -> [f32; 11] {
        let mut p = one_voice();
        p[NoteIn::PARAM_GLIDE] = seconds;
        p[NoteIn::PARAM_GLIDE_MODE] = mode as i32 as f32;
        p
    }

    /// Runs one block into mono cables, as a monophonic patch has.
    fn run_mono(module: &mut NoteIn, params: &[f32], midi: &[MidiEvent]) -> Vec<SignalBuffer> {
        let mut out = vec![
            SignalBuffer::control(BLOCK),
            SignalBuffer::gate(BLOCK),
            SignalBuffer::control(BLOCK),
            SignalBuffer::control(BLOCK),
        ];
        let context = ProcessContext::new(SR, BLOCK).with_midi(midi);
        module.process(&[], &mut out, params, &context);
        out
    }

    /// Runs `blocks` blocks of one voice, the first with `midi`, returning
    /// the pitch in semitones from C4.
    fn pitch_run(m: &mut NoteIn, params: &[f32], midi: &[MidiEvent], blocks: usize) -> Vec<f32> {
        let mut semitones = Vec::new();
        for block in 0..blocks {
            let out = run_mono(m, params, if block == 0 { midi } else { &[] });
            semitones.extend(pitch(&out).iter().map(|p| p * 12.0));
        }
        semitones
    }

    fn gate(out: &[SignalBuffer]) -> &[f32] {
        &out[NoteIn::PORT_GATE].samples
    }

    fn pitch(out: &[SignalBuffer]) -> &[f32] {
        &out[NoteIn::PORT_PITCH].samples
    }

    #[test]
    fn test_mono_cables_play_one_voice() {
        // Without room for more channels, it plays one voice whatever the knob
        let mut m = module();
        let out = run_mono(&mut m, &DEFAULTS, &[on(0, 60), on(10, 64)]);
        assert_eq!(sounding(&out, 20), [Some(64)]);
    }

    #[test]
    fn test_one_voice_is_silent_until_played() {
        let mut m = module();
        let out = run_mono(&mut m, &one_voice(), &[]);
        assert!(gate(&out).iter().all(|&g| g == 0.0));
        assert!(pitch(&out).iter().all(|&p| p == 0.0), "rests at C4");
    }

    #[test]
    fn test_one_voice_lands_on_its_sample() {
        let mut m = module();
        let out = run_mono(&mut m, &one_voice(), &[MidiEvent::note_on(100, 0, 72, 127), MidiEvent::note_off(180, 0, 72, 0)]);
        let g = gate(&out);
        assert!(g[..100].iter().all(|&s| s == 0.0));
        assert!(g[100..180].iter().all(|&s| s == 1.0));
        assert!(g[180..].iter().all(|&s| s == 0.0));
        assert_eq!(pitch(&out)[100], 1.0, "C5 from the note's first sample");
        assert_eq!(out[NoteIn::PORT_VELOCITY].samples[100], 1.0);
        assert_eq!(pitch(&out)[200], 1.0, "pitch holds through the release");
    }

    #[test]
    fn test_one_voice_shortest_note_still_triggers() {
        let mut m = module();
        let out = run_mono(&mut m, &one_voice(), &[on(10, 60), off(10, 60)]);
        assert_eq!(rising_edges(gate(&out)), vec![10]);
        assert_eq!(gate(&out)[11], 0.0);
    }

    #[test]
    fn test_one_voice_note_on_velocity_zero_is_note_off() {
        let mut m = module();
        let out = run_mono(&mut m, &one_voice(), &[on(0, 60), MidiEvent::note_on(50, 0, 60, 0)]);
        assert_eq!(gate(&out)[49], 1.0);
        assert_eq!(gate(&out)[50], 0.0);
    }

    #[test]
    fn test_one_voice_notes_carry_across_blocks() {
        let mut m = module();
        run_mono(&mut m, &one_voice(), &[on(200, 64)]);
        let out = run_mono(&mut m, &one_voice(), &[off(30, 64)]);
        assert!(gate(&out)[..30].iter().all(|&s| s == 1.0));
        assert!(gate(&out)[30..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_priority() {
        // Hold C4, then G4, then E4
        let chord = [on(0, 60), on(10, 67), on(20, 64)];
        let sounding = |priority| {
            let mut m = module();
            let out = run_mono(&mut m, &mono(priority, false), &chord);
            (pitch(&out)[30] * 12.0 + 60.0).round() as u8
        };
        assert_eq!(sounding(Priority::Last), 64, "Last");
        assert_eq!(sounding(Priority::Low), 60, "Low");
        assert_eq!(sounding(Priority::High), 67, "High");
    }

    #[test]
    fn test_priority_conversion() {
        assert_eq!(Priority::from_param(0.0), Priority::Last);
        assert_eq!(Priority::from_param(1.0), Priority::Low);
        assert_eq!(Priority::from_param(2.0), Priority::High);
        assert_eq!(Priority::from_param(99.0), Priority::Last);
    }

    #[test]
    fn test_release_returns_to_held_note() {
        let mut m = module();
        let out = run_mono(&mut m, &one_voice(), &[on(0, 60), on(10, 67), off(20, 67)]);
        assert_eq!(pitch(&out)[15], 7.0 / 12.0);
        assert_eq!(pitch(&out)[20], 0.0, "back to C4");
        assert!(gate(&out).iter().all(|&g| g == 1.0), "gate held throughout");
    }

    #[test]
    fn test_legato_retrigger() {
        let legato = [on(0, 60), on(100, 62)];

        let mut plain = module();
        let out = run_mono(&mut plain, &one_voice(), &legato);
        assert_eq!(rising_edges(gate(&out)), vec![0], "legato without retrigger keeps one gate");

        let mut retrig = module();
        let out = run_mono(&mut retrig, &mono(Priority::Last, true), &legato);
        assert_eq!(rising_edges(gate(&out)), vec![0, 101], "one-sample gap before the new note");
        assert_eq!(gate(&out)[100], 0.0);
        assert_eq!(pitch(&out)[100], 2.0 / 12.0, "the new pitch arrives with the gap");
    }

    #[test]
    fn test_retrigger_ignores_keys_that_dont_sound() {
        // Low priority: a higher key joining doesn't change the note, so
        // nothing retriggers
        let mut m = module();
        let out = run_mono(&mut m, &mono(Priority::Low, true), &[on(0, 60), on(100, 72)]);
        assert_eq!(rising_edges(gate(&out)), vec![0]);
    }

    #[test]
    fn test_one_voice_pitch_bend_and_octave() {
        let mut m = module();
        run_mono(&mut m, &one_voice(), &[on(0, 60), MidiEvent::new(0, 0, MidiMessage::PitchBend { value: 8191 })]);
        for _ in 0..20 {
            run_mono(&mut m, &one_voice(), &[]);
        }
        let out = run_mono(&mut m, &one_voice(), &[]);
        assert!((pitch(&out)[BLOCK - 1] * 12.0 - 2.0).abs() < 0.01, "full bend is +2 st by default");

        let mut m = module();
        let mut p = one_voice();
        p[NoteIn::PARAM_OCTAVE] = -2.0;
        let out = run_mono(&mut m, &p, &[on(0, 60)]);
        assert_eq!(pitch(&out)[0], -2.0);
    }

    #[test]
    fn test_one_voice_aftertouch_and_all_notes_off() {
        let mut m = module();
        let out = run_mono(
            &mut m,
            &one_voice(),
            &[on(0, 60), on(0, 64), MidiEvent::new(32, 0, MidiMessage::Aftertouch { pressure: 127 }), cc(50, ALL_NOTES_OFF, 0)],
        );
        assert_eq!(out[NoteIn::PORT_AFTERTOUCH].samples[31], 0.0);
        assert_eq!(out[NoteIn::PORT_AFTERTOUCH].samples[32], 1.0);
        assert!(gate(&out)[50..].iter().all(|&s| s == 0.0));
        // Nothing left held: a new note is a fresh gate
        let out = run_mono(&mut m, &one_voice(), &[on(5, 67)]);
        assert_eq!(rising_edges(gate(&out)), vec![5]);
    }

    #[test]
    fn test_one_voice_sustain_pedal() {
        let mut m = module();
        let out = run_mono(&mut m, &one_voice(), &[cc(0, SUSTAIN_PEDAL, 127), on(0, 60), off(10, 60), cc(100, SUSTAIN_PEDAL, 0)]);
        assert!(gate(&out)[..100].iter().all(|&g| g == 1.0), "the pedal holds the note");
        assert!(gate(&out)[100..].iter().all(|&g| g == 0.0), "pedal up lets it go");
    }

    #[test]
    fn test_one_voice_keys_play_at_the_key_velocity() {
        let mut p = one_voice();
        p[NoteIn::PARAM_KEY_VELOCITY] = 1.0;
        let mut m = module();
        let out = run_mono(&mut m, &p, &[key_on(0, 60), key_off(100, 60)]);
        assert_eq!(out[NoteIn::PORT_VELOCITY].samples[0], 1.0);
        assert_eq!(rising_edges(gate(&out)), vec![0]);
        assert_eq!(gate(&out)[100], 0.0);
    }

    #[test]
    fn test_changing_mode_lets_go() {
        // Poly to mono and back: nothing the other mode held sounds on
        let mut m = module();
        run(&mut m, &DEFAULTS, &[on(0, 60), on(0, 64)]);
        let out = run(&mut m, &one_voice(), &[]);
        assert!(gate(&out).iter().all(|&g| g == 0.0));
        run(&mut m, &one_voice(), &[on(0, 67)]);
        let out = run(&mut m, &DEFAULTS, &[]);
        assert!(sounding(&out, 0).iter().all(Option::is_none));
    }

    #[test]
    fn test_one_voice_glide_arrives_in_its_time() {
        // Hold C4, then G4 from a block boundary: a 7 semitone step
        let params = mono_glide(0.2, GlideMode::Legato);
        let mut m = module();
        run_mono(&mut m, &params, &[on(0, 60)]);
        let p = pitch_run(&mut m, &params, &[on(0, 67)], 50);

        let crossing = |fraction: f32| p.iter().position(|&s| s >= 7.0 * fraction).unwrap() as f32 / SR;
        let tau = 0.2 / 100f32.ln();
        assert!((crossing(1.0 - (-1.0f32).exp()) - tau).abs() < 0.001, "63% at τ = {tau} s");
        assert!((crossing(0.99) - 0.2).abs() < 0.001, "99% at the Glide time");
        assert!(p.windows(2).all(|w| w[1] >= w[0]), "a smooth rise");
    }

    #[test]
    fn test_one_voice_legato_mode_doesnt_glide_from_a_released_key() {
        let phrase = [on(0, 60), off(50, 60), on(100, 67)];
        let mut m = module();
        let out = run_mono(&mut m, &mono_glide(0.5, GlideMode::Legato), &phrase);
        assert_eq!(pitch(&out)[100], 7.0 / 12.0, "G starts on G");

        // The same phrase in Always mode slides up from C
        let mut m = module();
        let out = run_mono(&mut m, &mono_glide(0.5, GlideMode::Always), &phrase);
        assert!(pitch(&out)[100] * 12.0 < 0.1);
    }

    #[test]
    fn test_one_voice_first_note_never_glides() {
        let mut m = module();
        let out = run_mono(&mut m, &mono_glide(1.0, GlideMode::Always), &[on(10, 72)]);
        assert_eq!(pitch(&out)[10], 1.0);
    }

    #[test]
    fn test_one_voice_legato_glides_back_to_a_held_key() {
        // Trill: hold C, tap G, let go of G, and the pitch slides back
        let params = mono_glide(0.1, GlideMode::Legato);
        let mut m = module();
        run_mono(&mut m, &params, &[on(0, 60), on(0, 67)]);
        let p = pitch_run(&mut m, &params, &[off(0, 67)], 40);
        assert!(p[0] > 0.0 && p[0] < 7.0, "on its way down, not jumped: {}", p[0]);
        assert!(p.last().unwrap().abs() < 0.01);
    }

    #[test]
    fn test_note_in_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<NoteIn>();
    }
}
