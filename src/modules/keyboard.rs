//! Keyboard input module.
//!
//! A virtual keyboard that converts computer keyboard input into gate, pitch CV,
//! and velocity signals for playing the synthesizer.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{glide_parameters, Glide, GlideMode},
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

/// Key priority modes for handling multiple simultaneous keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPriority {
    /// Most recently pressed key takes priority.
    Last = 0,
    /// Lowest note playing takes priority.
    Lowest = 1,
    /// Highest note playing takes priority.
    Highest = 2,
}

impl KeyPriority {
    /// Convert from parameter value (0-2) to key priority.
    pub fn from_param(value: f32) -> Self {
        match value as usize {
            0 => KeyPriority::Last,
            1 => KeyPriority::Lowest,
            2 => KeyPriority::Highest,
            _ => KeyPriority::Last,
        }
    }

    /// The note that sounds from `held`, listed in the order the keys went down.
    ///
    /// Releasing the sounding key falls back to the next one under the same
    /// rule, because the choice is made afresh from whatever is still held.
    pub fn select_note(self, held: &[i32]) -> Option<i32> {
        match self {
            KeyPriority::Last => held.last().copied(),
            KeyPriority::Lowest => held.iter().copied().min(),
            KeyPriority::Highest => held.iter().copied().max(),
        }
    }
}

/// A virtual keyboard for triggering notes from computer keyboard input.
///
/// Converts QWERTY keyboard presses into musical notes, outputting gate,
/// pitch CV, and velocity signals that can drive oscillators and envelopes.
///
/// # Ports
///
/// **Outputs:**
/// - **Gate** (Gate): High (1.0) when a key is pressed, low (0.0) when released.
/// - **Pitch** (Control): V/Oct pitch CV. 0.0 = C4 (middle C), +1.0 = C5, -1.0 = C3.
/// - **Velocity** (Control): Note velocity (0.0-1.0).
///
/// # Parameters
///
/// - **Note** (0-127): Current MIDI note number (set by UI from keyboard events).
/// - **Gate** (0/1): Current gate state (set by UI from keyboard events).
/// - **Octave** (-2 to +2): Octave shift applied to keyboard input.
/// - **Velocity** (0-1): Fixed velocity value for all notes.
/// - **Priority** (0-2): Key priority mode (Last, Lowest, Highest).
/// - **Glide** (0-2 s): Time for the pitch to slide to a new note. 0 is off.
/// - **Glide Mode** (Always, Legato): Glide on every note, or only on notes
///   played while another key is held.
pub struct KeyboardInput {
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
    /// The note the pitch heads for, octave included, as a MIDI number.
    /// Only moves while the gate is high, so the release stays in tune.
    target: f32,
    /// The pitch as it slides toward `target`.
    glide: Glide,
    /// Gate state at the end of the last block.
    current_gate: f32,
}

impl KeyboardInput {
    /// Creates a new keyboard input module.
    pub fn new() -> Self {
        let mut keyboard = Self {
            sample_rate: 44100.0,
            ports: vec![
                // Output ports
                PortDefinition::output("gate", "Gate", SignalType::Gate).describe("High while a key is held; patch into an envelope's Gate"),
                PortDefinition::output("pitch", "Pitch", SignalType::Control).describe("Pitch of the held note as V/Oct"),
                PortDefinition::output("velocity", "Velocity", SignalType::Control).describe("Note strength, 0 to 1"),
            ],
            parameters: vec![
                // Note: MIDI note number (0-127), set by UI
                // Hidden from normal UI - controlled by keyboard events
                ParameterDefinition::new(
                    "note",
                    "Note",
                    0.0,
                    127.0,
                    60.0, // Default to middle C
                    ParameterDisplay::Linear { unit: "" },
                ).describe("Held note as a MIDI number; set by the keys you play"),
                // Gate: 0 or 1, set by UI when keys pressed/released
                ParameterDefinition::toggle("gate", "Gate", false).describe("On while a key is down; set by the keys you play"),
                // Octave: shift the keyboard up/down by octaves
                ParameterDefinition::new(
                    "octave",
                    "Octave",
                    -2.0,
                    2.0,
                    0.0,
                    ParameterDisplay::Linear { unit: "" },
                ).describe("Shifts the keyboard up or down by octaves"),
                // Velocity: fixed velocity for all notes
                ParameterDefinition::normalized("velocity", "Velocity", 1.0).describe("Fixed strength sent for every note"),
                // Priority: key priority mode
                ParameterDefinition::choice(
                    "priority",
                    "Priority",
                    &["Last", "Lowest", "Highest"],
                    0,
                ).describe("Which key sounds when several are held"),
            ],
            target: 60.0,
            glide: Glide::NEW,
            current_gate: 0.0,
        };
        // After the others, so saved patches load unchanged
        keyboard.parameters.extend(glide_parameters());
        keyboard
    }

    /// Port index constants.
    const PORT_GATE: usize = 0;
    const PORT_PITCH: usize = 1;
    const PORT_VELOCITY: usize = 2;

    /// Parameter index constants.
    const PARAM_NOTE: usize = 0;
    const PARAM_GATE: usize = 1;
    const PARAM_OCTAVE: usize = 2;
    const PARAM_VELOCITY: usize = 3;
    /// Read by the UI, which picks the sounding note from the held keys.
    pub const PARAM_PRIORITY: usize = 4;
    const PARAM_GLIDE: usize = 5;
    const PARAM_GLIDE_MODE: usize = 6;

    /// Convert MIDI note number to V/Oct pitch CV.
    ///
    /// Middle C (MIDI 60) = 0.0
    /// C5 (MIDI 72) = +1.0
    /// C3 (MIDI 48) = -1.0
    #[inline]
    fn midi_to_voct(midi_note: f32) -> f32 {
        (midi_note - 60.0) / 12.0
    }
}

impl Default for KeyboardInput {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for KeyboardInput {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "input.keyboard",
            name: "Keyboard",
            category: ModuleCategory::Source,
            description: "Virtual keyboard for playing notes from computer keyboard",
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
    }

    fn process(
        &mut self,
        _inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        // Get parameter values
        let note = params[Self::PARAM_NOTE];
        let gate = if params[Self::PARAM_GATE] > 0.5 { 1.0 } else { 0.0 };
        let octave = params[Self::PARAM_OCTAVE];
        let velocity = params[Self::PARAM_VELOCITY];
        let glide = Glide::coefficient(params[Self::PARAM_GLIDE], self.sample_rate);
        let glide_mode = GlideMode::from_param(params[Self::PARAM_GLIDE_MODE]);

        // The note moves only while a key is held. A gate that just rose is
        // a note from silence, which Legato mode starts on its own pitch;
        // a new note under a held gate is legato and glides
        if gate > 0.5 {
            let shifted_note = note + (octave * 12.0);
            if self.current_gate < 0.5 {
                self.glide.start(shifted_note, glide_mode.glides(false));
            }
            self.target = shifted_note;
        }

        // Fill output buffers
        for i in 0..context.block_size {
            // Gate output - instant transition
            outputs[Self::PORT_GATE].samples[i] = gate;

            outputs[Self::PORT_PITCH].samples[i] = Self::midi_to_voct(self.glide.next(self.target, glide));

            // Velocity output
            outputs[Self::PORT_VELOCITY].samples[i] = velocity;
        }

        self.current_gate = gate;
    }

    fn reset(&mut self) {
        self.target = 60.0;
        self.glide = Glide::NEW;
        self.current_gate = 0.0;
    }
}

/// Maps a computer keyboard key to a MIDI note number relative to C4.
///
/// Uses a tracker-style layout: two pianos, an octave apart.
/// - Bottom row (Z, X, C, V, B, N, M, comma, period, slash) = white keys from C4
/// - Middle row (S, D, G, H, J, L, semicolon) = their black keys
/// - Top row (Q, W, E, R, T, Y, U, I, O, P) = white keys from C5
/// - Number row (2, 3, 5, 6, 7, 9, 0) = their black keys
///
/// The two pianos overlap: `,` `.` `/` play the same C D E as Q W E.
///
/// Returns None if the key doesn't map to a note.
pub fn key_to_note(key: egui::Key) -> Option<i32> {
    use egui::Key;

    // Bottom row: white keys starting from C
    // Z=C, X=D, C=E, V=F, B=G, N=A, M=B, ,=C+, .=D+, /=E+
    match key {
        // Lower octave - white keys (Z X C V B N M , . /)
        Key::Z => Some(0),   // C
        Key::X => Some(2),   // D
        Key::C => Some(4),   // E
        Key::V => Some(5),   // F
        Key::B => Some(7),   // G
        Key::N => Some(9),   // A
        Key::M => Some(11),  // B
        Key::Comma => Some(12),  // C (next octave)
        Key::Period => Some(14), // D (next octave)
        Key::Slash => Some(16),  // E (next octave)

        // Lower octave - black keys (S D F G H J K L ; ')
        Key::S => Some(1),   // C#
        Key::D => Some(3),   // D#
        // F is skipped (no black key between E and F)
        Key::G => Some(6),   // F#
        Key::H => Some(8),   // G#
        Key::J => Some(10),  // A#
        // K is skipped (no black key between B and C)
        Key::L => Some(13),  // C# (next octave)
        Key::Semicolon => Some(15), // D# (next octave)

        // Upper octave - white keys (Q W E R T Y U I O P), from C5
        Key::Q => Some(12),  // C
        Key::W => Some(14),  // D
        Key::E => Some(16),  // E
        Key::R => Some(17),  // F
        Key::T => Some(19),  // G
        Key::Y => Some(21),  // A
        Key::U => Some(23),  // B
        Key::I => Some(24),  // C (next octave)
        Key::O => Some(26),  // D (next octave)
        Key::P => Some(28),  // E (next octave)

        // Upper octave - black keys (2 3 5 6 7 9 0), skipping 4 and 8 as
        // the middle row skips F and K
        Key::Num2 => Some(13), // C#
        Key::Num3 => Some(15), // D#
        Key::Num5 => Some(18), // F#
        Key::Num6 => Some(20), // G#
        Key::Num7 => Some(22), // A#
        Key::Num9 => Some(25), // C# (next octave)
        Key::Num0 => Some(27), // D# (next octave)

        _ => None,
    }
}

/// Converts a relative note (from key_to_note) to an absolute MIDI note number.
///
/// Base octave 0 means the keyboard starts at C4 (MIDI 60).
pub fn relative_to_midi(relative_note: i32, octave_shift: i32) -> u8 {
    let base_midi = 60; // C4
    let midi_note = base_midi + relative_note + (octave_shift * 12);
    midi_note.clamp(0, 127) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyboard_info() {
        let kbd = KeyboardInput::new();
        assert_eq!(kbd.info().id, "input.keyboard");
        assert_eq!(kbd.info().name, "Keyboard");
        assert_eq!(kbd.info().category, ModuleCategory::Source);
    }

    #[test]
    fn test_keyboard_ports() {
        let kbd = KeyboardInput::new();
        let ports = kbd.ports();

        assert_eq!(ports.len(), 3);

        // All are outputs
        assert!(ports[0].is_output());
        assert_eq!(ports[0].id, "gate");
        assert_eq!(ports[0].signal_type, SignalType::Gate);

        assert!(ports[1].is_output());
        assert_eq!(ports[1].id, "pitch");
        assert_eq!(ports[1].signal_type, SignalType::Control);

        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "velocity");
        assert_eq!(ports[2].signal_type, SignalType::Control);
    }

    #[test]
    fn test_keyboard_parameters() {
        let kbd = KeyboardInput::new();
        let params = kbd.parameters();

        assert_eq!(params.len(), 7);

        assert_eq!(params[0].id, "note");
        assert_eq!(params[1].id, "gate");
        assert_eq!(params[2].id, "octave");
        assert_eq!(params[3].id, "velocity");
        assert_eq!(params[4].id, "priority");
        assert_eq!(params[5].id, "glide");
        assert_eq!(params[6].id, "glide_mode");
    }

    #[test]
    fn test_midi_to_voct() {
        // Middle C (60) = 0.0
        assert!((KeyboardInput::midi_to_voct(60.0) - 0.0).abs() < f32::EPSILON);

        // C5 (72) = +1.0
        assert!((KeyboardInput::midi_to_voct(72.0) - 1.0).abs() < f32::EPSILON);

        // C3 (48) = -1.0
        assert!((KeyboardInput::midi_to_voct(48.0) - (-1.0)).abs() < f32::EPSILON);

        // A4 (69) = 0.75 (9 semitones above C4)
        assert!((KeyboardInput::midi_to_voct(69.0) - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_key_priority_conversion() {
        assert_eq!(KeyPriority::from_param(0.0), KeyPriority::Last);
        assert_eq!(KeyPriority::from_param(1.0), KeyPriority::Lowest);
        assert_eq!(KeyPriority::from_param(2.0), KeyPriority::Highest);
        assert_eq!(KeyPriority::from_param(99.0), KeyPriority::Last); // Out of range
    }

    #[test]
    fn test_select_note_by_priority() {
        // C, then G, then E held, in that order
        let held = [0, 7, 4];
        assert_eq!(KeyPriority::Last.select_note(&held), Some(4));
        assert_eq!(KeyPriority::Lowest.select_note(&held), Some(0));
        assert_eq!(KeyPriority::Highest.select_note(&held), Some(7));

        // Releasing the sounding E falls back to G under Last
        assert_eq!(KeyPriority::Last.select_note(&[0, 7]), Some(7));
        assert_eq!(KeyPriority::Lowest.select_note(&[]), None);
    }

    #[test]
    fn test_key_mapping() {
        use egui::Key;

        // White keys
        assert_eq!(key_to_note(Key::Z), Some(0));  // C
        assert_eq!(key_to_note(Key::X), Some(2));  // D
        assert_eq!(key_to_note(Key::C), Some(4));  // E
        assert_eq!(key_to_note(Key::V), Some(5));  // F
        assert_eq!(key_to_note(Key::B), Some(7));  // G
        assert_eq!(key_to_note(Key::N), Some(9));  // A
        assert_eq!(key_to_note(Key::M), Some(11)); // B

        // Black keys
        assert_eq!(key_to_note(Key::S), Some(1));  // C#
        assert_eq!(key_to_note(Key::D), Some(3));  // D#
        assert_eq!(key_to_note(Key::G), Some(6));  // F#
        assert_eq!(key_to_note(Key::H), Some(8));  // G#
        assert_eq!(key_to_note(Key::J), Some(10)); // A#

        // Non-note keys
        // Top row and numbers: the same piano, an octave up
        assert_eq!(key_to_note(Key::Q), Some(12)); // C5
        assert_eq!(key_to_note(Key::R), Some(17)); // F5
        assert_eq!(key_to_note(Key::P), Some(28)); // E6
        assert_eq!(key_to_note(Key::Num2), Some(13)); // C#5
        assert_eq!(key_to_note(Key::Num0), Some(27)); // D#6
        assert_eq!(key_to_note(Key::Num4), None);
        assert_eq!(key_to_note(Key::Num8), None);

        assert_eq!(key_to_note(Key::Space), None);
        assert_eq!(key_to_note(Key::Escape), None);
    }

    #[test]
    fn test_relative_to_midi() {
        // C at base octave = C4 = 60
        assert_eq!(relative_to_midi(0, 0), 60);

        // D at base octave = D4 = 62
        assert_eq!(relative_to_midi(2, 0), 62);

        // C one octave up = C5 = 72
        assert_eq!(relative_to_midi(0, 1), 72);

        // C one octave down = C3 = 48
        assert_eq!(relative_to_midi(0, -1), 48);

        // Clamping
        assert_eq!(relative_to_midi(0, -10), 0);  // Can't go below 0
        assert_eq!(relative_to_midi(12, 10), 127); // Can't go above 127
    }

    #[test]
    fn test_keyboard_generates_output() {
        let mut kbd = KeyboardInput::new();
        kbd.prepare(44100.0, 256);

        let mut outputs = vec![
            SignalBuffer::gate(256),
            SignalBuffer::control(256),
            SignalBuffer::control(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        // Test with gate on, note 60, octave 0, velocity 1.0
        kbd.process(&[], &mut outputs, &[60.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0], &ctx);

        // Gate should be 1.0
        assert!((outputs[0].samples[0] - 1.0).abs() < f32::EPSILON);

        // Pitch should be 0.0 (middle C)
        assert!((outputs[1].samples[0] - 0.0).abs() < f32::EPSILON);

        // Velocity should be 1.0
        assert!((outputs[2].samples[0] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_keyboard_octave_shift() {
        let mut kbd = KeyboardInput::new();
        kbd.prepare(44100.0, 256);

        let mut outputs = vec![
            SignalBuffer::gate(256),
            SignalBuffer::control(256),
            SignalBuffer::control(256),
        ];
        let ctx = ProcessContext::new(44100.0, 256);

        // Note 60 (C4) with octave +1 should output pitch +1.0 (C5)
        kbd.process(&[], &mut outputs, &[60.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0], &ctx);
        assert!((outputs[1].samples[0] - 1.0).abs() < f32::EPSILON);

        // Note 60 (C4) with octave -1 should output pitch -1.0 (C3)
        kbd.reset();
        kbd.process(&[], &mut outputs, &[60.0, 1.0, -1.0, 1.0, 0.0, 0.0, 1.0], &ctx);
        assert!((outputs[1].samples[0] - (-1.0)).abs() < f32::EPSILON);
    }

    /// Plays `blocks` blocks of `note` with the gate at `gate`, returning the
    /// pitch in semitones from C4.
    fn play(kbd: &mut KeyboardInput, note: f32, gate: f32, glide: f32, mode: GlideMode, blocks: usize) -> Vec<f32> {
        let mut outputs = vec![SignalBuffer::gate(256), SignalBuffer::control(256), SignalBuffer::control(256)];
        let ctx = ProcessContext::new(48000.0, 256);
        let params = [note, gate, 0.0, 1.0, 0.0, glide, mode as i32 as f32];
        let mut pitch = Vec::new();
        for _ in 0..blocks {
            kbd.process(&[], &mut outputs, &params, &ctx);
            pitch.extend(outputs[1].samples.iter().map(|p| p * 12.0));
        }
        pitch
    }

    fn keyboard() -> KeyboardInput {
        let mut kbd = KeyboardInput::new();
        kbd.prepare(48000.0, 256);
        kbd
    }

    #[test]
    fn test_keyboard_glides_legato() {
        // Hold C, then G under the same gate: a slide that arrives in 200 ms
        let mut kbd = keyboard();
        play(&mut kbd, 60.0, 1.0, 0.2, GlideMode::Legato, 1);
        let p = play(&mut kbd, 67.0, 1.0, 0.2, GlideMode::Legato, 50);
        assert!(p[0] < 0.1);
        let at99 = p.iter().position(|&s| s >= 7.0 * 0.99).unwrap() as f32 / 48000.0;
        assert!((at99 - 0.2).abs() < 0.001, "99% at {at99} s");
    }

    #[test]
    fn test_keyboard_legato_mode_starts_after_a_gap_on_pitch() {
        let mut kbd = keyboard();
        play(&mut kbd, 60.0, 1.0, 0.5, GlideMode::Legato, 1);
        play(&mut kbd, 60.0, 0.0, 0.5, GlideMode::Legato, 1);
        let p = play(&mut kbd, 67.0, 1.0, 0.5, GlideMode::Legato, 1);
        assert_eq!(p[0], 7.0);

        // Always mode slides from C even after the gap
        let mut kbd = keyboard();
        play(&mut kbd, 60.0, 1.0, 0.5, GlideMode::Always, 1);
        play(&mut kbd, 60.0, 0.0, 0.5, GlideMode::Always, 1);
        let p = play(&mut kbd, 67.0, 1.0, 0.5, GlideMode::Always, 1);
        assert!(p[0] < 0.1);
    }

    #[test]
    fn test_keyboard_zero_glide_is_instant() {
        let mut kbd = keyboard();
        play(&mut kbd, 60.0, 1.0, 0.0, GlideMode::Always, 1);
        let p = play(&mut kbd, 67.0, 1.0, 0.0, GlideMode::Always, 1);
        assert!(p.iter().all(|&s| s == 7.0));
        // Released, the pitch holds where it was
        let p = play(&mut kbd, 72.0, 0.0, 0.0, GlideMode::Always, 1);
        assert!(p.iter().all(|&s| s == 7.0));
    }

    #[test]
    fn test_keyboard_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<KeyboardInput>();
    }

    #[test]
    fn test_keyboard_default() {
        let kbd = KeyboardInput::default();
        assert_eq!(kbd.info().id, "input.keyboard");
    }
}
