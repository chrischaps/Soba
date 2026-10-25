//! FM Voice: a DX7 voice for a volca fm, bred by ear (#448). The volca
//! speaks DX7, whose six operators, 32 algorithms and 155 bytes are
//! famously hard to program by hand. The node holds a voice and sends it
//! whole, by SysEx (#440), every time it changes; **Mutate** makes a child
//! of it and **Cross** a child of it and another, each sent to be heard at
//! once. The voices bred so far grow in the node's garden, a bank's worth,
//! each one click from playing again (see [`crate::hardware::garden`]).
//!
//! Nothing here runs on the audio thread: a voice goes from the editor as
//! a SysEx message (see `app::synth_app::fm_voices`), when it changes, on
//! opening the patch and with Send. The node's knobs say where it goes and
//! how far Mutate goes.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::SignalBuffer,
};
use crate::hardware::dx7::Target;
use crate::link::midi::PORT_LABELS;
use crate::modules::midi_out::CHANNEL_LABELS;

/// The module's ID, as patches name it.
pub const ID: &str = "output.fm_voice";

/// What Format offers, in [`Target`]'s order.
const FORMATS: &[&str] = &["volca fm", "DX7"];

/// A DX7 voice, sent to a volca fm (or a DX7, or Dexed) as it changes.
///
/// # Parameters
///
/// - **Port**, **Channel**: where the voice goes. The volca fm takes it on
///   any channel; a DX7 on its own.
/// - **Format**: volca fm or DX7, which differ in a voice's last byte.
/// - **Amount**: how far Mutate moves a child from its parent, 0 to 1.
pub struct FmVoice {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl FmVoice {
    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    pub const PARAM_FORMAT: usize = 2;
    pub const PARAM_AMOUNT: usize = 3;

    pub fn new() -> Self {
        let parameters = vec![
            ParameterDefinition::choice("port", "Port", PORT_LABELS, 0).describe("Which device the voice goes to: open the Rig (Ctrl+Shift+H) to give each port one"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0).describe("The channel the voice is addressed to. A volca fm takes it on any; a DX7 only on its own"),
            ParameterDefinition::choice("format", "Format", FORMATS, 0)
                .describe("What it's sent to. A volca fm reads a voice's last byte as which operators are on, where a DX7, a volca fm2 or Dexed reads a checksum"),
            ParameterDefinition::new("amount", "Amount", 0.0, 1.0, 0.35, ParameterDisplay::linear(""))
                .describe("How far Mutate moves a child from its parent: a little a nudge to a neighbour, all the way a wander"),
        ];
        Self { ports: Vec::new(), parameters }
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
            description: "A DX7 voice for a volca fm, sent by SysEx as it changes: mutate it, cross two, and keep a garden of the voices bred by ear",
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

    fn process(&mut self, _inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], _params: &[f32], _context: &ProcessContext) {}

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_and_parameters() {
        let module = FmVoice::new();
        assert_eq!(module.info().id, "output.fm_voice");
        assert_eq!(module.info().category, ModuleCategory::Output);
        assert!(module.ports().is_empty());
        assert_eq!(module.parameters()[FmVoice::PARAM_AMOUNT].name, "Amount");
        for word in ["DX7", "volca", "SysEx", "mutate"] {
            assert!(module.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
        assert_eq!(FmVoice::target(0.0), Target::Volca);
        assert_eq!(FmVoice::target(1.0), Target::Dx7);
    }
}
