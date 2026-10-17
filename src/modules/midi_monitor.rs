//! MIDI Monitor module.
//!
//! A utility module that displays incoming MIDI events in real-time.
//! This is primarily a debugging/learning tool that shows what MIDI data
//! is being received.
//!
//! Note: This module is display-only - it doesn't process any audio signals.
//! The MIDI events are displayed via custom UI rendering in the node graph,
//! not through the DSP pipeline.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
};

/// The module's ID, as patches name it.
pub const ID: &str = "util.midi_monitor";

/// A MIDI monitor module that displays incoming MIDI events.
///
/// This module is display-only - it reads from the global MIDI input
/// that's managed by the application. The actual event display happens
/// in the node graph UI, not in the DSP process method.
///
/// # Parameters
///
/// - **Channel Filter** (0-16): Filter by MIDI channel (0 = all channels).
/// - **Show Notes** (toggle): Show note on/off events.
/// - **Show CC** (toggle): Show control change events.
/// - **Show Pitch Bend** (toggle): Show pitch bend events.
pub struct MidiMonitor {
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl MidiMonitor {
    /// Creates a new MIDI Monitor module.
    pub fn new() -> Self {
        Self {
            parameters: vec![
                // Channel filter (0 = all, 1-16 = specific channel)
                ParameterDefinition::choice(
                    "channel",
                    "Channel",
                    &["All", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"],
                    0, // Default to all channels
                ).describe("Only show events from this MIDI channel"),
                // Toggle filters for event types
                ParameterDefinition::toggle("show_notes", "Notes", true).describe("Show note on and off events"),
                ParameterDefinition::toggle("show_cc", "CC", true).describe("Show controller messages such as knobs and pedals"),
                ParameterDefinition::toggle("show_pitch_bend", "Pitch Bend", true).describe("Show pitch bend wheel movements"),
            ],
        }
    }
}

impl Default for MidiMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for MidiMonitor {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "MIDI Monitor",
            category: ModuleCategory::Utility,
            description: "Display incoming MIDI events",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        // No ports - this is a display-only module
        &[]
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {
        // Nothing to prepare - this is a display-only module
    }

    fn process(
        &mut self,
        _inputs: &[&SignalBuffer],
        _outputs: &mut [SignalBuffer],
        _params: &[f32],
        _context: &ProcessContext,
    ) {
        // No processing - MIDI display is handled in the UI
    }

    fn reset(&mut self) {
        // Nothing to reset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_midi_monitor_info() {
        let monitor = MidiMonitor::new();
        assert_eq!(monitor.info().id, "util.midi_monitor");
        assert_eq!(monitor.info().name, "MIDI Monitor");
        assert_eq!(monitor.info().category, ModuleCategory::Utility);
    }

    #[test]
    fn test_midi_monitor_no_ports() {
        let monitor = MidiMonitor::new();
        assert!(monitor.ports().is_empty());
    }

    #[test]
    fn test_midi_monitor_parameters() {
        let monitor = MidiMonitor::new();
        let params = monitor.parameters();

        assert_eq!(params.len(), 4);
        assert_eq!(params[0].id, "channel");
        assert_eq!(params[1].id, "show_notes");
        assert_eq!(params[2].id, "show_cc");
        assert_eq!(params[3].id, "show_pitch_bend");
    }

    #[test]
    fn test_midi_monitor_default() {
        let monitor = MidiMonitor::default();
        assert_eq!(monitor.info().id, "util.midi_monitor");
    }

    #[test]
    fn test_midi_monitor_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MidiMonitor>();
    }
}
