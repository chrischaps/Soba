//! MIDI In (#479): which device, and which channel, the modules patched
//! from it hear.
//!
//! Note In, Pad, Controls, the Clock and the MIDI Monitor each pick their
//! own Device (#435). A purple cable from a MIDI In picks it for them: a
//! group of eight Pads gets one jack to plug a controller into, and moving
//! to another controller is one change, not eight.
//!
//! The cable is a routing, not a stream. The module makes no sound and
//! reads no events: the editor follows each MIDI cable back to the MIDI In
//! it comes from, through any groups between, and has the module at its
//! other end hear that MIDI In's Device and Channel (see
//! `crate::graph::midi_device::heard`). The engine filters each module's
//! MIDI as it always has. Unpatched, a module hears its own Device again.
//!
//! The Device is kept in the node's labels, as the listening modules keep
//! theirs; the Channel is a parameter.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{SignalBuffer, SignalType},
};

/// The module's ID, as patches name it.
pub const ID: &str = "input.midi";

/// The name of the MIDI output, and of the MIDI input of every module that
/// can hear one.
pub const PORT_NAME: &str = "MIDI";

/// The MIDI input a module that hears MIDI gains (#479): plugged into a MIDI
/// In, it hears that MIDI In's device and channel instead of its own Device.
pub fn input() -> PortDefinition {
    PortDefinition::input("midi", PORT_NAME, SignalType::Midi)
        .describe("Patch from a MIDI In to hear its device and channel; unpatched, it hears its own Device")
}

/// Index of the Channel parameter: 0 is Omni, 1-16 a channel.
pub const PARAM_CHANNEL: usize = 0;

/// The channel names, Omni first.
const CHANNELS: [&str; 17] = ["Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// The one MIDI channel (0-15, as on the wire) a Channel parameter's value
/// picks, or `None` for Omni.
pub fn channel_of(value: f32) -> Option<u8> {
    match value.round() as i32 {
        channel @ 1..=16 => Some(channel as u8 - 1),
        _ => None,
    }
}

/// A MIDI device, as a cable.
pub struct MidiIn {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl MidiIn {
    /// Creates a MIDI In on any device and every channel.
    pub fn new() -> Self {
        Self {
            ports: vec![PortDefinition::output("midi", PORT_NAME, SignalType::Midi)
                .describe("Patch into a module's MIDI input, or a group's, and it hears this device and channel")],
            parameters: vec![ParameterDefinition::choice("channel", "Channel", &CHANNELS, 0)
                .describe("Which channel the modules patched from it hear: Omni for every one")],
        }
    }
}

impl Default for MidiIn {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for MidiIn {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "MIDI In",
            category: ModuleCategory::Source,
            description: "A MIDI device and channel, as a cable: the modules patched from it hear them",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn process(&mut self, _inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], _params: &[f32], _context: &ProcessContext) {
        // A routing, not a stream: the editor reads the cable (see the module docs)
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_midi_in_is_one_purple_output_and_a_channel() {
        let midi_in = MidiIn::new();
        assert_eq!(midi_in.info().category, ModuleCategory::Source);
        let [port] = midi_in.ports() else { panic!("one port") };
        assert!(!port.is_input());
        assert_eq!(port.signal_type, SignalType::Midi);
        assert_eq!(midi_in.parameters()[PARAM_CHANNEL].name, "Channel");
    }

    #[test]
    fn omni_is_every_channel_and_the_rest_are_one_each() {
        assert_eq!(channel_of(0.0), None);
        assert_eq!(channel_of(1.0), Some(0));
        assert_eq!(channel_of(10.0), Some(9));
        assert_eq!(channel_of(16.0), Some(15));
        assert_eq!(channel_of(17.0), None, "past the last, every channel");
    }

    #[test]
    fn every_module_that_hears_midi_has_a_midi_input() {
        let registry = crate::engine::create_module_registry();
        for id in [super::super::note_in::ID, super::super::pad::ID, super::super::controls::ID, super::super::clock::ID, super::super::midi_monitor::ID] {
            let module = registry.create(id).unwrap();
            let port = module.ports().iter().find(|p| p.name == PORT_NAME).unwrap_or_else(|| panic!("{id} has no MIDI input"));
            assert!(port.is_input() && port.signal_type == SignalType::Midi, "{id}");
        }
    }
}
