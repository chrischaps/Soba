//! Scenes: the knobs of the patch, or of the group it sits in, kept as
//! scenes A to H, and a Morph knob to glide from one scene to another.
//!
//! The scenes and the gliding are the editor's (see `graph/scenes.rs` and
//! `app/synth_app/scenes.rs`): a scene sets other modules' knobs, which the
//! editor owns, and they turn on screen as it glides. The engine's part is
//! only to carry Morph, From and To, and the CV patched into Morph, which
//! the editor reads back as it does any knob's CV.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

/// The module's ID, as patches name it.
pub const ID: &str = "util.scenes";

/// The scenes' names, in slot order.
pub const SLOT_LABELS: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];

/// Keeps scenes of knobs and glides between two of them.
pub struct Scenes {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Scenes {
    pub fn new() -> Self {
        Self {
            ports: vec![PortDefinition::input("morph", "Morph", SignalType::Control)
                .describe("0 to 1 glides from the From scene to the To scene: an LFO, an envelope or an Arranger lane")],
            parameters: vec![
                ParameterDefinition::new("morph", "Morph", 0.0, 1.0, 0.0, ParameterDisplay::linear(""))
                    .describe("How far from the From scene to the To scene the knobs sit. Switches flip halfway"),
                ParameterDefinition::choice("from", "From", &SLOT_LABELS, 0)
                    .describe("The scene Morph starts from, at 0"),
                ParameterDefinition::choice("to", "To", &SLOT_LABELS, 1)
                    .describe("The scene Morph arrives at, at 1"),
            ],
        }
    }

    pub const PARAM_MORPH: usize = 0;
    pub const PARAM_FROM: usize = 1;
    pub const PARAM_TO: usize = 2;
}

impl Default for Scenes {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Scenes {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Scenes",
            category: ModuleCategory::Utility,
            description: "Keep the knobs as scenes, and glide between two",
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
        // Nothing to make: the editor turns the knobs
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morph_pairs_with_its_jack() {
        let scenes = Scenes::new();
        assert_eq!(scenes.info().id, ID);
        assert_eq!(scenes.ports()[0].name, scenes.parameters()[Scenes::PARAM_MORPH].name);
        assert_eq!(scenes.parameters()[Scenes::PARAM_TO].default, 1.0);
    }
}
