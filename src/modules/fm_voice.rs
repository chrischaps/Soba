//! FM Voice: a DX7 voice for a volca fm, bred by ear (#448). The volca
//! speaks DX7, whose six operators, 32 algorithms and 155 bytes are
//! famously hard to program by hand. The node holds a voice and sends it
//! whole, by SysEx (#440), every time it changes; **Mutate** makes a child
//! of it and **Cross** a child of it and another, each sent to be heard at
//! once. The voices bred so far grow in the node's garden, a bank's worth,
//! each one click from playing again (see [`crate::hardware::garden`]).
//!
//! A voice goes from the editor as a SysEx message (see
//! `app::synth_app::fm_voices`), when it changes, on opening the patch and
//! with Send. The node's knobs say where it goes and how far Mutate goes.
//!
//! **It sounds in Soba too** (#472), for the browser, a train, or anyone
//! without a volca: patch a Pitch and a Gate in and Out plays the voice on
//! six operators of Soba's own (see [`engine`]), a note per channel of the
//! cable. It does both at once, so a volca and Soba play the same voice,
//! and a voice bred, recalled or undone is heard on each as it changes, in
//! the notes held too. The voice reaches the audio thread by value, as
//! `EngineCommand::SetFmVoice`.

pub mod engine;

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer, SignalType},
};
use crate::hardware::dx7::{Target, Voice};
use crate::link::midi::PORT_LABELS;
use crate::modules::midi_out::CHANNEL_LABELS;

/// The module's ID, as patches name it.
pub const ID: &str = "output.fm_voice";

/// What Format offers, in [`Target`]'s order.
const FORMATS: &[&str] = &["volca fm", "DX7"];

/// A DX7 voice, sent to a volca fm (or a DX7, or Dexed) as it changes, and
/// played on six operators of Soba's own.
///
/// # Parameters
///
/// - **Port**, **Channel**: where the voice goes. The volca fm takes it on
///   any channel; a DX7 on its own.
/// - **Format**: volca fm or DX7, which differ in a voice's last byte.
/// - **Amount**: how far Mutate moves a child from its parent, 0 to 1.
/// - **Level**: how loud Out plays the voice.
///
/// # Ports
///
/// - **Pitch** (Control, poly), **Gate** (Gate, poly), **Velocity**
///   (Control, poly): the notes, one per channel.
/// - **Out** (Audio, poly): each note, on its channel.
pub struct FmVoice {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    engine: engine::Engine,
    /// The channels played last block.
    active: usize,
    /// Level as the last block ended, to glide from.
    level: f32,
}

impl FmVoice {
    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    pub const PARAM_FORMAT: usize = 2;
    pub const PARAM_AMOUNT: usize = 3;
    pub const PARAM_LEVEL: usize = 4;

    pub const PORT_PITCH: usize = 0;
    pub const PORT_GATE: usize = 1;
    pub const PORT_VELOCITY: usize = 2;

    pub fn new() -> Self {
        let parameters = vec![
            ParameterDefinition::choice("port", "Port", PORT_LABELS, 0).describe("Which device the voice goes to: open the Rig (Ctrl+Shift+H) to give each port one"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0).describe("The channel the voice is addressed to. A volca fm takes it on any; a DX7 only on its own"),
            ParameterDefinition::choice("format", "Format", FORMATS, 0)
                .describe("What it's sent to. A volca fm reads a voice's last byte as which operators are on, where a DX7, a volca fm2 or Dexed reads a checksum"),
            ParameterDefinition::new("amount", "Amount", 0.0, 1.0, 0.35, ParameterDisplay::linear(""))
                .describe("How far Mutate moves a child from its parent: a little a nudge to a neighbour, all the way a wander"),
            ParameterDefinition::new("level", "Level", 0.0, 1.0, 0.25, ParameterDisplay::linear("")).describe("How loud Out plays the voice: at 1, a carrier at its loudest is full scale"),
        ];
        let ports = vec![
            PortDefinition::input_with_default("pitch", "Pitch", SignalType::Control, 0.0).describe("V/Oct, a note per channel: Soba plays the voice itself"),
            PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0).describe("High while each note is held"),
            // About neutral: a DX7's velocity scaling barely moves at 100
            PortDefinition::input_with_default("velocity", "Velocity", SignalType::Control, 100.0 / 127.0).describe("How hard each note is struck, 0 to 1, as the voice's operators feel it"),
            PortDefinition::output("out", "Out", SignalType::Audio).describe("The voice played on six operators of Soba's own, a note per channel"),
        ];
        Self { ports, parameters, engine: engine::Engine::new(), active: 1, level: 0.25 }
    }

    /// What Format's value `value` sends to.
    pub fn target(value: f32) -> Target {
        if value.round() as usize == 1 {
            Target::Dx7
        } else {
            Target::Volca
        }
    }
}

impl Default for FmVoice {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for FmVoice {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "FM Voice",
            category: ModuleCategory::Output,
            description: "A DX7 voice for a volca fm, sent by SysEx as it changes and played on six FM operators of its own: mutate it, cross two, and keep a garden of the voices bred by ear",
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
        self.engine.prepare(sample_rate);
    }

    fn process(&mut self, inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], params: &[f32], _context: &ProcessContext) {
        let Some(out) = outputs.first_mut() else { return };
        let (Some(pitch), Some(gate), Some(velocity)) = (inputs.get(Self::PORT_PITCH), inputs.get(Self::PORT_GATE), inputs.get(Self::PORT_VELOCITY)) else {
            return;
        };
        // A note per channel of the widest cable in
        let wanted = [Self::PORT_PITCH, Self::PORT_GATE, Self::PORT_VELOCITY].iter().filter_map(|&port| connected_input(inputs, port)).map(|b| b.channels()).max().unwrap_or(1);
        let channels = out.set_channels(wanted);
        if channels < self.active {
            self.engine.drop_from(channels);
        }
        self.active = channels;

        let len = out.samples.len();
        let level = params.get(Self::PARAM_LEVEL).copied().unwrap_or(0.25).clamp(0.0, 1.0);
        let (from, step) = (self.level, (level - self.level) / len.max(1) as f32);
        self.level = level;
        for channel in 0..channels {
            let played = engine::Played { pitch: &pitch.voice(channel).samples, gate: &gate.voice(channel).samples, velocity: &velocity.voice(channel).samples };
            let samples = out.channel_mut(channel);
            self.engine.play(channel, &played, samples);
            for (i, sample) in samples.iter_mut().enumerate() {
                *sample *= from + step * i as f32;
            }
        }
        self.engine.run_lfo(len);
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn polyphonic(&self) -> bool {
        true
    }

    fn set_fm_voice(&mut self, voice: Voice) {
        self.engine.set_voice(&voice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_and_parameters() {
        let module = FmVoice::new();
        assert_eq!(module.info().id, "output.fm_voice");
        assert_eq!(module.info().category, ModuleCategory::Output);
        let ids: Vec<&str> = module.ports().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["pitch", "gate", "velocity", "out"]);
        assert!(module.polyphonic());
        assert_eq!(module.parameters()[FmVoice::PARAM_AMOUNT].name, "Amount");
        assert_eq!(module.parameters()[FmVoice::PARAM_LEVEL].name, "Level");
        for word in ["DX7", "volca", "SysEx", "mutate", "FM"] {
            assert!(module.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
        assert_eq!(FmVoice::target(0.0), Target::Volca);
        assert_eq!(FmVoice::target(1.0), Target::Dx7);
    }

    /// Runs the module a block of `len` at full Level, with a poly Pitch
    /// and Gate of `notes` (MIDI), every one held.
    fn chord(module: &mut FmVoice, notes: &[f32], len: usize) -> SignalBuffer {
        let mut pitch = SignalBuffer::polyphonic(len, SignalType::Control);
        let mut gate = SignalBuffer::polyphonic(len, SignalType::Gate);
        pitch.set_channels(notes.len());
        gate.set_channels(notes.len());
        for (k, &note) in notes.iter().enumerate() {
            pitch.channel_mut(k).fill((note - 60.0) / 12.0);
            gate.channel_mut(k).fill(1.0);
        }
        let velocity = SignalBuffer::unconnected(len, SignalType::Control);
        let mut outputs = vec![SignalBuffer::polyphonic(len, SignalType::Audio)];
        let mut params: Vec<f32> = module.parameters().iter().map(|p| p.default).collect();
        params[FmVoice::PARAM_LEVEL] = 1.0;
        module.level = 1.0;
        module.process(&[&pitch, &gate, &velocity], &mut outputs, &params, &ProcessContext::default());
        outputs.remove(0)
    }

    #[test]
    fn a_chord_plays_a_note_per_channel() {
        let mut module = FmVoice::new();
        module.prepare(48_000.0, 4800);
        module.set_fm_voice(Voice::init());
        let out = chord(&mut module, &[60.0, 64.0, 67.0], 4800);
        assert_eq!(out.channels(), 3);
        for k in 0..3 {
            let peak = out.voice(k).samples[2400..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.5, "channel {k}: {peak}");
        }
        assert_ne!(out.voice(0).samples[3000], out.voice(1).samples[3000], "each at its own pitch");
    }

    #[test]
    fn unpatched_it_is_silent() {
        let mut module = FmVoice::new();
        module.prepare(48_000.0, 256);
        let quiet = SignalBuffer::unconnected(256, SignalType::Control);
        let mut outputs = vec![SignalBuffer::polyphonic(256, SignalType::Audio)];
        let params: Vec<f32> = module.parameters().iter().map(|p| p.default).collect();
        module.process(&[&quiet, &quiet, &quiet], &mut outputs, &params, &ProcessContext::default());
        assert!(outputs[0].samples.iter().all(|&s| s == 0.0));
    }
}
