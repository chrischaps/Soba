//! MIDI Note module.
//!
//! Turns live MIDI into CV signals: V/Oct pitch (with pitch bend), gate,
//! velocity and aftertouch. This provides hardware MIDI input as an
//! alternative to the Keyboard module.
//!
//! MIDI reaches the module on the audio thread, through
//! [`ProcessContext::midi`], with each event placed at its sample in the
//! block. Notes start and stop on that exact sample, however short they are.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{glide_parameters, rate_for_seconds, Glide, GlideMode},
    signal::SignalBuffer,
    MidiEvent, MidiMessage, ParameterDisplay, SignalType,
};

/// Voice priority modes for handling polyphonic input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoicePriority {
    /// Most recently pressed note takes priority.
    Last = 0,
    /// Lowest note takes priority.
    Low = 1,
    /// Highest note takes priority.
    High = 2,
}

impl VoicePriority {
    /// Convert from parameter value (0-2) to voice priority.
    pub fn from_param(value: f32) -> Self {
        match value as usize {
            0 => VoicePriority::Last,
            1 => VoicePriority::Low,
            2 => VoicePriority::High,
            _ => VoicePriority::Last,
        }
    }
}

/// CC 120, All Sound Off.
const ALL_SOUND_OFF: u8 = 120;
/// CC 123, All Notes Off.
const ALL_NOTES_OFF: u8 = 123;

/// How quickly pitch bend follows the wheel. Wheels send steps a few
/// milliseconds apart; gliding between them keeps bends from zippering.
const BEND_SMOOTHING_SECONDS: f32 = 0.005;

/// The module's ID, as patches name it.
pub const ID: &str = "input.midi_note";

/// A MIDI Note module that converts MIDI input to CV signals.
///
/// Monophonic: when several keys are held, the Priority parameter picks the
/// one that sounds.
///
/// # Ports
///
/// **Outputs:**
/// - **Pitch** (Control): V/Oct pitch CV including pitch bend. 0.0 = C4 (MIDI 60), +1.0 = C5, -1.0 = C3.
/// - **Gate** (Gate): High (1.0) while a key is held, low (0.0) when all are released.
/// - **Velocity** (Control): The sounding note's velocity (0.0-1.0).
/// - **Aftertouch** (Control): Channel pressure (0.0-1.0).
///
/// # Parameters
///
/// - **Channel** (Omni, 1-16): Which MIDI channel to listen to.
/// - **Octave** (-4 to +4): Octave shift applied to MIDI input.
/// - **Priority** (Last, Low, High): Which held key sounds.
/// - **Retrigger**: When on, moving between held keys (legato) drops the
///   gate for one sample, so envelopes start again.
/// - **Bend Range** (0-12 semitones): How far the pitch bend wheel bends.
/// - **Glide** (0-2 s): Time for the pitch to slide to a new note. 0 is off.
/// - **Glide Mode** (Always, Legato): Glide on every note, or only on notes
///   played while another key is held.
pub struct MidiNote {
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
    /// Keys held down as (note, velocity), oldest first. Its capacity covers
    /// every MIDI note, so it never reallocates.
    held: Vec<(u8, u8)>,
    /// The note the pitch output plays. Kept after release, so the release
    /// tail stays in tune.
    note: u8,
    /// The pitch as it slides toward `note`.
    glide: Glide,
    /// Whether any key is held.
    gate: bool,
    /// The sounding note's velocity (0.0-1.0).
    velocity: f32,
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
    /// Keep the gate high for this sample even if the note already ended,
    /// so the shortest note still makes a rising edge.
    hold_gate: bool,
    /// Drop the gate for this sample, so a retriggered legato note makes a
    /// fresh rising edge.
    retrigger_gap: bool,
}

impl MidiNote {
    /// Creates a new MIDI Note module.
    pub fn new() -> Self {
        let mut module = Self {
            ports: vec![
                // Output ports
                PortDefinition::output("pitch", "Pitch", SignalType::Control).describe("Pitch of the played note as V/Oct, with pitch bend"),
                PortDefinition::output("gate", "Gate", SignalType::Gate).describe("High while a key is held; patch into an envelope's Gate"),
                PortDefinition::output("velocity", "Velocity", SignalType::Control).describe("How hard the key was struck, 0 to 1"),
                PortDefinition::output("aftertouch", "Aftertouch", SignalType::Control).describe("Key pressure after the note starts, 0 to 1"),
            ],
            parameters: vec![
                // Channel: MIDI channel filter (0=Omni, 1-16=specific)
                ParameterDefinition::choice(
                    "channel",
                    "Channel",
                    &["Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"],
                    0,
                ).describe("Only respond to this MIDI channel; Omni hears all"),
                // Octave: shift the notes up/down by octaves
                ParameterDefinition::new("octave", "Octave", -4.0, 4.0, 0.0, ParameterDisplay::stepped("oct")).describe("Shifts incoming notes up or down by octaves"),
                // Priority: which held key sounds
                ParameterDefinition::choice("priority", "Priority", &["Last", "Low", "High"], 0).describe("Which key sounds when several are held"),
                // Retrigger: restart envelopes on legato notes
                ParameterDefinition::toggle("retrigger", "Retrigger", false).describe("Restarts the gate on notes played legato"),
                // Bend Range: semitones at full pitch bend
                ParameterDefinition::new("bend_range", "Bend Range", 0.0, 12.0, 2.0, ParameterDisplay::stepped("st")).describe("Semitones the pitch bend wheel moves at full travel"),
            ],
            held: Vec::with_capacity(128),
            note: 60,
            glide: Glide::NEW,
            gate: false,
            velocity: 0.0,
            aftertouch: 0.0,
            bend_target: 0.0,
            bend: 0.0,
            bend_coeff: 1.0,
            sample_rate: 44100.0,
            hold_gate: false,
            retrigger_gap: false,
        };
        // After the others, so saved patches load unchanged
        module.parameters.extend(glide_parameters());
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
    pub const PARAM_OCTAVE: usize = 1;
    pub const PARAM_PRIORITY: usize = 2;
    pub const PARAM_RETRIGGER: usize = 3;
    pub const PARAM_BEND_RANGE: usize = 4;
    pub const PARAM_GLIDE: usize = 5;
    pub const PARAM_GLIDE_MODE: usize = 6;

    /// Convert MIDI note number to V/Oct pitch CV.
    ///
    /// Middle C (MIDI 60) = 0.0
    /// C5 (MIDI 72) = +1.0
    /// C3 (MIDI 48) = -1.0
    #[inline]
    pub fn midi_to_voct(midi_note: f32) -> f32 {
        (midi_note - 60.0) / 12.0
    }

    /// Whether the Channel parameter lets `event` through.
    pub(crate) fn listens_to(channel_param: f32, event: &MidiEvent) -> bool {
        let channel = channel_param.round() as i32;
        channel == 0 || channel - 1 == event.channel as i32
    }

    /// Applies one MIDI event.
    fn handle(&mut self, event: &MidiEvent, play: Play) {
        match event.message {
            MidiMessage::NoteOn { note, velocity } if velocity > 0 => {
                self.held.retain(|&(held, _)| held != note);
                if self.held.len() < self.held.capacity() {
                    self.held.push((note, velocity));
                }
                self.choose_note(play);
            }
            // Note On at velocity 0 is a Note Off
            MidiMessage::NoteOn { note, .. } | MidiMessage::NoteOff { note, .. } => {
                self.held.retain(|&(held, _)| held != note);
                self.choose_note(play);
            }
            MidiMessage::PitchBend { value } => {
                self.bend_target = (value as f32 / 8192.0).clamp(-1.0, 1.0);
            }
            MidiMessage::Aftertouch { pressure } => {
                self.aftertouch = pressure as f32 / 127.0;
            }
            MidiMessage::ControlChange { controller: ALL_NOTES_OFF | ALL_SOUND_OFF, .. } => {
                self.held.clear();
                self.gate = false;
            }
            _ => {}
        }
    }

    /// The held key that sounds under `priority`, as (note, velocity).
    fn active(&self, priority: VoicePriority) -> Option<(u8, u8)> {
        match priority {
            VoicePriority::Last => self.held.last().copied(),
            VoicePriority::Low => self.held.iter().copied().min_by_key(|&(note, _)| note),
            VoicePriority::High => self.held.iter().copied().max_by_key(|&(note, _)| note),
        }
    }

    /// Updates the sounding note and gate after the held keys changed.
    fn choose_note(&mut self, play: Play) {
        let Some((note, velocity)) = self.active(play.priority) else {
            self.gate = false;
            return;
        };
        if !self.gate {
            self.gate = true;
            self.hold_gate = true;
            // No key was held: in Legato mode the note starts on its pitch
            self.glide.start(note as f32, play.glide_mode.glides(false));
        } else if note == self.note {
            // Still the same key sounding: nothing to change
            return;
        } else if play.retrigger {
            self.retrigger_gap = true;
        }
        self.note = note;
        self.velocity = velocity as f32 / 127.0;
    }
}

/// The parameters that decide how a note event plays.
#[derive(Clone, Copy)]
struct Play {
    priority: VoicePriority,
    retrigger: bool,
    glide_mode: GlideMode,
}

impl Default for MidiNote {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for MidiNote {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "MIDI Note",
            category: ModuleCategory::Source,
            description: "Convert MIDI note events to CV signals",
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
        let octave = params[Self::PARAM_OCTAVE].round();
        let play = Play {
            priority: VoicePriority::from_param(params[Self::PARAM_PRIORITY]),
            retrigger: params[Self::PARAM_RETRIGGER] > 0.5,
            glide_mode: GlideMode::from_param(params[Self::PARAM_GLIDE_MODE]),
        };
        let bend_range = params[Self::PARAM_BEND_RANGE];
        let glide = Glide::coefficient(params[Self::PARAM_GLIDE], self.sample_rate);

        let mut events = context
            .midi
            .iter()
            .filter(|event| Self::listens_to(channel, event))
            .peekable();

        for i in 0..context.block_size {
            // Everything that lands on this sample, in order
            while let Some(event) = events.next_if(|event| event.sample_offset as usize <= i) {
                self.handle(event, play);
            }

            self.bend += (self.bend_target - self.bend) * self.bend_coeff;
            // Bend rides on top of the glide, unslewed by it
            let semitones = self.glide.next(self.note as f32, glide) + octave * 12.0 + self.bend * bend_range;

            let gate = if self.hold_gate {
                true
            } else {
                self.gate && !self.retrigger_gap
            };
            self.hold_gate = false;
            self.retrigger_gap = false;

            outputs[Self::PORT_PITCH].samples[i] = Self::midi_to_voct(semitones);
            outputs[Self::PORT_GATE].samples[i] = if gate { 1.0 } else { 0.0 };
            outputs[Self::PORT_VELOCITY].samples[i] = self.velocity;
            outputs[Self::PORT_AFTERTOUCH].samples[i] = self.aftertouch;
        }

        // Anything placed past the block still counts, from the next one
        for event in events {
            self.handle(event, play);
        }
    }

    fn reset(&mut self) {
        self.held.clear();
        self.glide = Glide::NEW;
        self.gate = false;
        self.velocity = 0.0;
        self.aftertouch = 0.0;
        self.bend_target = 0.0;
        self.bend = 0.0;
        self.hold_gate = false;
        self.retrigger_gap = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;

    /// Default parameters: Omni, octave 0, Last priority, no retrigger, ±2 st,
    /// no glide (Legato).
    const DEFAULTS: [f32; 7] = [0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 1.0];

    fn module() -> MidiNote {
        let mut module = MidiNote::new();
        module.prepare(SR, BLOCK);
        module
    }

    fn outputs() -> Vec<SignalBuffer> {
        vec![
            SignalBuffer::control(BLOCK),
            SignalBuffer::gate(BLOCK),
            SignalBuffer::control(BLOCK),
            SignalBuffer::control(BLOCK),
        ]
    }

    /// Runs one block with `midi`, returning the outputs.
    fn run(module: &mut MidiNote, params: &[f32], midi: &[MidiEvent]) -> Vec<SignalBuffer> {
        let mut out = outputs();
        let context = ProcessContext::new(SR, BLOCK).with_midi(midi);
        module.process(&[], &mut out, params, &context);
        out
    }

    fn gate(out: &[SignalBuffer]) -> &[f32] {
        &out[MidiNote::PORT_GATE].samples
    }

    fn pitch(out: &[SignalBuffer]) -> &[f32] {
        &out[MidiNote::PORT_PITCH].samples
    }

    /// Indices where the gate rises.
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
    fn test_midi_note_info() {
        let module = MidiNote::new();
        assert_eq!(module.info().id, "input.midi_note");
        assert_eq!(module.info().name, "MIDI Note");
        assert_eq!(module.info().category, ModuleCategory::Source);
    }

    #[test]
    fn test_midi_note_ports() {
        let module = MidiNote::new();
        let ports = module.ports();

        assert_eq!(ports.len(), 4);
        assert!(ports.iter().all(|port| port.is_output()));
        assert_eq!(ports[0].id, "pitch");
        assert_eq!(ports[0].signal_type, SignalType::Control);
        assert_eq!(ports[1].id, "gate");
        assert_eq!(ports[1].signal_type, SignalType::Gate);
        assert_eq!(ports[2].id, "velocity");
        assert_eq!(ports[3].id, "aftertouch");
    }

    #[test]
    fn test_midi_note_parameters() {
        let module = MidiNote::new();
        let ids: Vec<&str> = module.parameters().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["channel", "octave", "priority", "retrigger", "bend_range", "glide", "glide_mode"]);
        let defaults: Vec<f32> = module.parameters().iter().map(|p| p.default).collect();
        assert_eq!(defaults, DEFAULTS);
    }

    #[test]
    fn test_midi_to_voct() {
        assert!((MidiNote::midi_to_voct(60.0) - 0.0).abs() < f32::EPSILON);
        assert!((MidiNote::midi_to_voct(72.0) - 1.0).abs() < f32::EPSILON);
        assert!((MidiNote::midi_to_voct(48.0) - (-1.0)).abs() < f32::EPSILON);
        assert!((MidiNote::midi_to_voct(69.0) - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_voice_priority_conversion() {
        assert_eq!(VoicePriority::from_param(0.0), VoicePriority::Last);
        assert_eq!(VoicePriority::from_param(1.0), VoicePriority::Low);
        assert_eq!(VoicePriority::from_param(2.0), VoicePriority::High);
        assert_eq!(VoicePriority::from_param(99.0), VoicePriority::Last);
    }

    #[test]
    fn test_silent_until_played() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[]);
        assert!(gate(&out).iter().all(|&g| g == 0.0));
        assert!(pitch(&out).iter().all(|&p| p == 0.0), "rests at C4");
    }

    #[test]
    fn test_note_lands_on_its_sample() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[MidiEvent::note_on(100, 0, 72, 127), MidiEvent::note_off(180, 0, 72, 0)]);

        let g = gate(&out);
        assert!(g[..100].iter().all(|&s| s == 0.0));
        assert!(g[100..180].iter().all(|&s| s == 1.0));
        assert!(g[180..].iter().all(|&s| s == 0.0));
        assert_eq!(pitch(&out)[100], 1.0, "C5 from the note's first sample");
        assert_eq!(out[MidiNote::PORT_VELOCITY].samples[100], 1.0);
        assert_eq!(pitch(&out)[200], 1.0, "pitch holds through the release");
    }

    #[test]
    fn test_shortest_note_still_triggers() {
        // On and off on the same sample: one sample of gate, not none
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[MidiEvent::note_on(10, 0, 60, 100), MidiEvent::note_off(10, 0, 60, 0)]);
        assert_eq!(rising_edges(gate(&out)), vec![10]);
        assert_eq!(gate(&out)[11], 0.0);
    }

    #[test]
    fn test_note_on_velocity_zero_is_note_off() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_on(50, 0, 60, 0)]);
        assert_eq!(gate(&out)[49], 1.0);
        assert_eq!(gate(&out)[50], 0.0);
    }

    #[test]
    fn test_notes_carry_across_blocks() {
        let mut m = module();
        run(&mut m, &DEFAULTS, &[MidiEvent::note_on(200, 0, 64, 100)]);
        let out = run(&mut m, &DEFAULTS, &[MidiEvent::note_off(30, 0, 64, 0)]);
        assert!(gate(&out)[..30].iter().all(|&s| s == 1.0));
        assert!(gate(&out)[30..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_channel_filter() {
        let on_ch2 = [MidiEvent::note_on(0, 1, 60, 100)];

        let mut omni = module();
        assert_eq!(gate(&run(&mut omni, &DEFAULTS, &on_ch2))[0], 1.0);

        // Channel parameter 2 is MIDI channel 2, which is wire channel 1
        let mut ch2 = module();
        let params = [2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 1.0];
        assert_eq!(gate(&run(&mut ch2, &params, &on_ch2))[0], 1.0);

        let mut ch1 = module();
        let params = [1.0, 0.0, 0.0, 0.0, 2.0, 0.0, 1.0];
        assert!(gate(&run(&mut ch1, &params, &on_ch2)).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_priority() {
        // Hold C4, then G4, then E4
        let chord = [
            MidiEvent::note_on(0, 0, 60, 100),
            MidiEvent::note_on(10, 0, 67, 100),
            MidiEvent::note_on(20, 0, 64, 100),
        ];
        let sounding = |priority: f32| {
            let mut m = module();
            let out = run(&mut m, &[0.0, 0.0, priority, 0.0, 2.0, 0.0, 1.0], &chord);
            (pitch(&out)[30] * 12.0 + 60.0).round() as u8
        };
        assert_eq!(sounding(0.0), 64, "Last");
        assert_eq!(sounding(1.0), 60, "Low");
        assert_eq!(sounding(2.0), 67, "High");
    }

    #[test]
    fn test_release_returns_to_held_note() {
        let mut m = module();
        let out = run(
            &mut m,
            &DEFAULTS,
            &[MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_on(10, 0, 67, 100), MidiEvent::note_off(20, 0, 67, 0)],
        );
        assert_eq!(pitch(&out)[15], 7.0 / 12.0);
        assert_eq!(pitch(&out)[20], 0.0, "back to C4");
        assert!(gate(&out).iter().all(|&g| g == 1.0), "gate held throughout");
    }

    #[test]
    fn test_legato_retrigger() {
        let legato = [MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_on(100, 0, 62, 100)];

        let mut plain = module();
        let out = run(&mut plain, &DEFAULTS, &legato);
        assert_eq!(rising_edges(gate(&out)), vec![0], "legato without retrigger keeps one gate");

        let mut retrig = module();
        let out = run(&mut retrig, &[0.0, 0.0, 0.0, 1.0, 2.0, 0.0, 1.0], &legato);
        assert_eq!(rising_edges(gate(&out)), vec![0, 101], "one-sample gap before the new note");
        assert_eq!(gate(&out)[100], 0.0);
        assert_eq!(pitch(&out)[100], 2.0 / 12.0, "the new pitch arrives with the gap");
    }

    #[test]
    fn test_retrigger_ignores_keys_that_dont_sound() {
        // Low priority: a higher key joining doesn't change the note, so
        // nothing retriggers
        let mut m = module();
        let out = run(
            &mut m,
            &[0.0, 0.0, 1.0, 1.0, 2.0, 0.0, 1.0],
            &[MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_on(100, 0, 72, 100)],
        );
        assert_eq!(rising_edges(gate(&out)), vec![0]);
    }

    #[test]
    fn test_pitch_bend() {
        let bend_up = MidiEvent::new(0, 0, MidiMessage::PitchBend { value: 8191 });
        let note = MidiEvent::note_on(0, 0, 60, 100);

        // Let the 5 ms glide settle: 20 blocks is over 100 ms
        let mut m = module();
        run(&mut m, &DEFAULTS, &[note, bend_up]);
        for _ in 0..20 {
            run(&mut m, &DEFAULTS, &[]);
        }
        let out = run(&mut m, &DEFAULTS, &[]);
        let semitones = pitch(&out)[BLOCK - 1] * 12.0;
        assert!((semitones - 2.0).abs() < 0.01, "full bend is +2 st by default, got {semitones}");

        // Bend Range 12: a full octave
        let params = [0.0, 0.0, 0.0, 0.0, 12.0, 0.0, 1.0];
        let out = run(&mut m, &params, &[]);
        assert!((pitch(&out)[BLOCK - 1] - 1.0).abs() < 0.01);

        // Centred again
        let centre = MidiEvent::new(0, 0, MidiMessage::PitchBend { value: 0 });
        run(&mut m, &DEFAULTS, &[centre]);
        for _ in 0..20 {
            run(&mut m, &DEFAULTS, &[]);
        }
        let out = run(&mut m, &DEFAULTS, &[]);
        assert!(pitch(&out)[BLOCK - 1].abs() < 0.001);
    }

    #[test]
    fn test_pitch_bend_glides() {
        // A jump in bend doesn't step the pitch in one sample
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[MidiEvent::new(0, 0, MidiMessage::PitchBend { value: 8191 })]);
        let p = pitch(&out);
        assert!(p[0] * 12.0 < 0.1, "first sample barely moves");
        assert!(p.windows(2).all(|w| w[1] >= w[0]), "rises smoothly");
    }

    #[test]
    fn test_octave_shift() {
        let mut m = module();
        let out = run(&mut m, &[0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 1.0], &[MidiEvent::note_on(0, 0, 60, 100)]);
        assert_eq!(pitch(&out)[0], 1.0);
        let out = run(&mut m, &[0.0, -2.0, 0.0, 0.0, 2.0, 0.0, 1.0], &[]);
        assert_eq!(pitch(&out)[0], -2.0);
    }

    #[test]
    fn test_aftertouch() {
        let mut m = module();
        let out = run(&mut m, &DEFAULTS, &[MidiEvent::new(64, 0, MidiMessage::Aftertouch { pressure: 127 })]);
        let at = &out[MidiNote::PORT_AFTERTOUCH].samples;
        assert_eq!(at[63], 0.0);
        assert_eq!(at[64], 1.0);
    }

    #[test]
    fn test_all_notes_off() {
        let mut m = module();
        let out = run(
            &mut m,
            &DEFAULTS,
            &[
                MidiEvent::note_on(0, 0, 60, 100),
                MidiEvent::note_on(0, 0, 64, 100),
                MidiEvent::new(50, 0, MidiMessage::ControlChange { controller: ALL_NOTES_OFF, value: 0 }),
            ],
        );
        assert!(gate(&out)[50..].iter().all(|&s| s == 0.0));
        // Nothing left held: a new note is a fresh gate
        let out = run(&mut m, &DEFAULTS, &[MidiEvent::note_on(5, 0, 67, 100)]);
        assert_eq!(rising_edges(gate(&out)), vec![5]);
    }

    #[test]
    fn test_reset_releases_everything() {
        let mut m = module();
        run(&mut m, &DEFAULTS, &[MidiEvent::note_on(0, 0, 60, 100)]);
        m.reset();
        let out = run(&mut m, &DEFAULTS, &[]);
        assert!(gate(&out).iter().all(|&g| g == 0.0));
    }

    /// Defaults with a glide of `seconds` in `mode`.
    fn glide_params(seconds: f32, mode: GlideMode) -> [f32; 7] {
        [0.0, 0.0, 0.0, 0.0, 2.0, seconds, mode as i32 as f32]
    }

    /// Runs `blocks` blocks, the first with `midi`, returning the pitch in
    /// semitones from C4.
    fn pitch_run(m: &mut MidiNote, params: &[f32], midi: &[MidiEvent], blocks: usize) -> Vec<f32> {
        let mut semitones = Vec::new();
        for block in 0..blocks {
            let out = run(m, params, if block == 0 { midi } else { &[] });
            semitones.extend(pitch(&out).iter().map(|p| p * 12.0));
        }
        semitones
    }

    #[test]
    fn test_glide_arrives_in_its_time() {
        // Hold C4, then G4 from a block boundary: a 7 semitone step
        let params = glide_params(0.2, GlideMode::Legato);
        let mut m = module();
        run(&mut m, &params, &[MidiEvent::note_on(0, 0, 60, 100)]);
        let p = pitch_run(&mut m, &params, &[MidiEvent::note_on(0, 0, 67, 100)], 50);

        let crossing = |fraction: f32| p.iter().position(|&s| s >= 7.0 * fraction).unwrap() as f32 / SR;
        let tau = 0.2 / 100f32.ln();
        assert!((crossing(1.0 - (-1.0f32).exp()) - tau).abs() < 0.001, "63% at τ = {tau} s");
        assert!((crossing(0.99) - 0.2).abs() < 0.001, "99% at the Glide time");
        assert!(p.windows(2).all(|w| w[1] >= w[0]), "a smooth rise");
    }

    #[test]
    fn test_legato_mode_doesnt_glide_from_a_released_key() {
        let params = glide_params(0.5, GlideMode::Legato);
        let mut m = module();
        let out = run(&mut m, &params, &[MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_off(50, 0, 60, 0), MidiEvent::note_on(100, 0, 67, 100)]);
        assert_eq!(pitch(&out)[100], 7.0 / 12.0, "G starts on G");

        // The same phrase in Always mode slides up from C
        let mut m = module();
        let params = glide_params(0.5, GlideMode::Always);
        let out = run(&mut m, &params, &[MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_off(50, 0, 60, 0), MidiEvent::note_on(100, 0, 67, 100)]);
        assert!(pitch(&out)[100] * 12.0 < 0.1);
    }

    #[test]
    fn test_first_note_never_glides() {
        let mut m = module();
        let out = run(&mut m, &glide_params(1.0, GlideMode::Always), &[MidiEvent::note_on(10, 0, 72, 100)]);
        assert_eq!(pitch(&out)[10], 1.0);
    }

    #[test]
    fn test_legato_glides_back_to_a_held_key() {
        // Trill: hold C, tap G, let go of G, and the pitch slides back
        let params = glide_params(0.1, GlideMode::Legato);
        let mut m = module();
        let midi = [MidiEvent::note_on(0, 0, 60, 100), MidiEvent::note_on(0, 0, 67, 100)];
        run(&mut m, &params, &midi);
        let p = pitch_run(&mut m, &params, &[MidiEvent::note_off(0, 0, 67, 0)], 40);
        assert!(p[0] > 0.0 && p[0] < 7.0, "on its way down, not jumped: {}", p[0]);
        assert!(p.last().unwrap().abs() < 0.01);
    }

    #[test]
    fn test_zero_glide_matches_no_glide() {
        // With Glide at 0, both modes play exactly the pitch the note, octave
        // and bend make
        let phrase = [
            MidiEvent::note_on(0, 0, 60, 100),
            MidiEvent::note_on(30, 0, 64, 100),
            MidiEvent::new(40, 0, MidiMessage::PitchBend { value: 4000 }),
            MidiEvent::note_off(90, 0, 64, 0),
            MidiEvent::note_off(150, 0, 60, 0),
            MidiEvent::note_on(200, 0, 55, 100),
        ];
        for mode in [GlideMode::Always, GlideMode::Legato] {
            let mut params = glide_params(0.0, mode);
            params[MidiNote::PARAM_OCTAVE] = 1.0;
            let mut glided = module();
            let out = run(&mut glided, &params, &phrase);

            // Today's formula, from the bend the module heard
            let mut bend = 0.0f32;
            let coeff = rate_for_seconds(BEND_SMOOTHING_SECONDS, SR);
            let notes = |i: usize| match i {
                0..=29 => 60.0,
                30..=89 => 64.0,
                90..=199 => 60.0,
                _ => 55.0,
            };
            for (i, &p) in pitch(&out).iter().enumerate() {
                let target = if i >= 40 { 4000.0 / 8192.0 } else { 0.0 };
                bend += (target - bend) * coeff;
                let expected = MidiNote::midi_to_voct(notes(i) + 12.0 + bend * 2.0);
                assert_eq!(p, expected, "{mode:?} at sample {i}");
            }
        }
    }

    #[test]
    fn test_bend_rides_on_top_of_the_glide() {
        // A long glide doesn't slow the bend: it settles in its usual 5 ms
        let params = glide_params(2.0, GlideMode::Always);
        let mut m = module();
        run(&mut m, &params, &[MidiEvent::note_on(0, 0, 60, 100)]);
        let p = pitch_run(&mut m, &params, &[MidiEvent::new(0, 0, MidiMessage::PitchBend { value: 8191 })], 10);
        assert!((p[(0.05 * SR) as usize] - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_midi_note_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MidiNote>();
    }

    #[test]
    fn test_midi_note_default() {
        let module = MidiNote::default();
        assert_eq!(module.info().id, "input.midi_note");
    }
}
