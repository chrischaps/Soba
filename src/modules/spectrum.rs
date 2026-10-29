//! Spectrum module: a sound's harmonics, as lines standing up from the floor.
//!
//! It does nothing to the sound and passes nothing on. The engine copies
//! both inputs into the module's spectrum tap after every block (see
//! [`SpectrumPorts`]), and the face (`graph::spectrum_display`) reads the
//! tap on the UI thread, FFT and all, so the audio thread only copies.
//!
//! In 1 is drawn blue and In 2 orange, as the Oscilloscope draws them:
//! the scope shows a wave's shape, the Spectrum what it's made of.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, SpectrumPorts},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

/// The module's ID, as patches name it.
pub const ID: &str = "util.spectrum";

/// The biggest the face can be dragged, times its smallest.
pub const MAX_SIZE: f32 = 3.0;

/// A display of what's in a sound: one line per partial, on a keyboard's
/// axis.
///
/// # Ports
///
/// **Inputs:**
/// - **In 1** (Audio): The first spectrum, drawn blue
/// - **In 2** (Audio): The second, drawn orange behind it
///
/// **Outputs:** None (display only)
///
/// # Parameters
///
/// - **Hold** (0-5 s): How long each peak stays before it falls away
/// - **Range** (24-120 dB): How far below full scale the floor is
/// - **Harmonics**: Number the partials of the note heard
/// - **Size** (1-3): How big the face is drawn, set by dragging its corner
pub struct Spectrum {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Spectrum {
    pub fn new() -> Self {
        Self {
            ports: vec![
                PortDefinition::input_with_default("in1", "In 1", SignalType::Audio, 0.0)
                    .describe("The sound to look into, drawn blue; its note's harmonics are numbered"),
                PortDefinition::input_with_default("in2", "In 2", SignalType::Audio, 0.0)
                    .describe("A second sound to compare, drawn orange behind the first"),
            ],
            parameters: vec![
                ParameterDefinition::new("hold", "Hold", 0.0, 5.0, 1.5, ParameterDisplay::linear("s"))
                    .describe("How long each peak is held before it falls away, so a pluck leaves something to read"),
                ParameterDefinition::new("range", "Range", 24.0, 120.0, 72.0, ParameterDisplay::linear("dB"))
                    .describe("How far below full scale the floor of the display is"),
                ParameterDefinition::toggle("harmonics", "Harmonics", true)
                    .describe("Number the harmonics of the note heard, and draw how a saw's would fall"),
                ParameterDefinition::new("size", "Size", 1.0, MAX_SIZE, 1.0, ParameterDisplay::linear("×"))
                    .describe("How big the face is drawn; drag its corner"),
            ],
        }
    }
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Spectrum {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Spectrum",
            category: ModuleCategory::Utility,
            description: "The harmonics in a sound, as lines on a keyboard's axis",
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

    /// Nothing to do: the engine copies the inputs to the tap.
    fn process(&mut self, _inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], _params: &[f32], _context: &ProcessContext) {}

    fn reset(&mut self) {}

    fn spectrum_ports(&self) -> Option<SpectrumPorts> {
        // No outputs of its own, so the tap hears In 2 beside In 1
        Some(SpectrumPorts { input: "in1", outputs: &["in2"] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_audio_inputs_and_nothing_out() {
        let spectrum = Spectrum::new();
        let ids: Vec<&str> = spectrum.ports().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["in1", "in2"]);
        assert!(spectrum.ports().iter().all(|p| p.is_input() && p.signal_type == SignalType::Audio));
        assert_eq!(spectrum.info().category, ModuleCategory::Utility);
    }
}
