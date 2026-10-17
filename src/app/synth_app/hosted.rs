//! The app inside a DAW, as the CLAP plugin's editor (`plugin/`).
//!
//! The plugin makes the app with [`SynthApp::hosted`], which hands back the
//! [`AudioProcessor`] for the plugin's audio thread to drive with the host's
//! buffers, MIDI and transport. The app keeps no devices of its own: the DAW
//! plays it, so Play/Stop, the device menus and updates are left out.
//!
//! The patch is the plugin's state, saved inside the DAW's project
//! ([`SynthApp::hosted_state`]), and the host's eight macros turn the knobs
//! mapped to them ([`SynthApp::follow_macros`]), as MIDI CCs do theirs.

use rtrb::Consumer;
use serde::{Deserialize, Serialize};

use super::*;
use crate::engine::{AudioProcessor, EngineChannels, TimestampedMidiEvent};
use crate::graph::MACRO_COUNT;
use crate::persistence::{examples, patch_to_json};

/// The patch the plugin starts with: a pad played from the DAW's notes.
const FIRST_PATCH: &str = "Lush Pad";

/// What the plugin keeps for the app.
pub(super) struct Hosted {
    /// The macros' values as the knobs last followed them (NaN: not yet).
    macros: [f32; MACRO_COUNT as usize],
}

/// The plugin's state, as the DAW's project keeps it.
#[derive(Serialize, Deserialize)]
struct HostedState {
    /// The patch, as a patch file holds it.
    patch: String,
    /// The file it was opened from or saved to, which samples saved
    /// beside it are found from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    /// The example it was opened from, whose samples ship inside Soba.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    example: Option<String>,
}

impl SynthApp {
    /// The app as a plugin's editor, at the host's `sample_rate`, hearing
    /// the host's MIDI from `midi` (for the MIDI Monitor, the pianos and
    /// MIDI Learn). Returns the processor the plugin's audio thread runs:
    /// it plays whatever the app builds, with
    /// [`process_offline`](AudioProcessor::process_offline).
    pub fn hosted(sample_rate: f32, midi: Consumer<TimestampedMidiEvent>) -> (Self, AudioProcessor) {
        crate::hosted::set_hosted();
        let (ui_handle, engine_handle) = EngineChannels::with_defaults().split();
        let processor = AudioProcessor::new(sample_rate, crate::engine::BLOCK_SIZE, engine_handle);

        let mut app = Self::with_devices(Err(AudioError::NoOutputDevice), None);
        app.audio_error_message = None;
        app.ui_handle = Some(ui_handle);
        app.midi_event_consumer = Some(midi);
        app.user_state.hosted = true;
        app.hosted = Some(Hosted { macros: [f32::NAN; MACRO_COUNT as usize] });
        // The DAW decides when it plays; the patch always runs
        app.set_playing(true);
        if let Some(example) = examples::EXAMPLES.iter().find(|e| e.name == FIRST_PATCH) {
            app.open_example(example);
        }
        (app, processor)
    }

    /// Whether the app is a plugin's editor.
    pub fn is_hosted(&self) -> bool {
        self.hosted.is_some()
    }

    /// The patch as the DAW's project keeps it.
    pub fn hosted_state(&self) -> String {
        let patch = self.create_patch(&self.patch_title());
        let state = HostedState {
            patch: patch_to_json(&patch).unwrap_or_default(),
            path: self.current_patch_path.clone(),
            example: self.current_example.map(|e| e.name.to_string()),
        };
        serde_json::to_string(&state).unwrap_or_default()
    }

    /// Opens the patch a DAW's project kept (see [`hosted_state`](Self::hosted_state)).
    pub fn restore_hosted_state(&mut self, state: &str) -> Result<(), String> {
        let state: HostedState = serde_json::from_str(state).map_err(|e| e.to_string())?;
        let patch = crate::persistence::patch_from_json(&state.patch).map_err(|e| e.to_string())?;
        let example = state.example.as_deref().and_then(|name| examples::EXAMPLES.iter().find(|e| e.name == name));
        let base = match (&state.path, example) {
            (Some(path), _) => SampleBase::Folder(path.parent().unwrap_or(Path::new("."))),
            (None, Some(_)) => SampleBase::Example,
            (None, None) => SampleBase::Keys,
        };
        let warnings = self.load_patch(&patch, base).map_err(|e| e.to_string())?;
        self.current_patch_path = state.path;
        self.current_example = example;
        self.show_load_warnings(warnings);
        // The knobs on macros take the macros' values again
        if let Some(hosted) = self.hosted.as_mut() {
            hosted.macros = [f32::NAN; MACRO_COUNT as usize];
        }
        Ok(())
    }

    /// Turns the knobs mapped to the host's macros to `values` (0-1 each),
    /// where a macro has moved since the knobs last followed it.
    pub fn follow_macros(&mut self, values: &[f32]) {
        let Some(hosted) = self.hosted.as_mut() else { return };
        let mut moved = [false; MACRO_COUNT as usize];
        for (slot, (&value, last)) in values.iter().zip(hosted.macros.iter_mut()).enumerate() {
            if value != *last {
                moved[slot] = true;
                *last = value;
            }
        }
        let updates: Vec<(u64, usize, f32)> = self
            .midi_mappings
            .iter()
            .filter_map(|m| {
                let slot = m.macro_slot? as usize;
                moved.get(slot).copied().filter(|&moved| moved)?;
                Some((m.node_id, m.param_index, m.macro_to_value(values[slot])))
            })
            .collect();
        for (node_id, param_index, value) in updates {
            self.send_command(EngineCommand::SetParameter { node_id, param_index, value });
            self.cached_params.insert((node_id, param_index), value);
            self.update_graph_param_from_cc(node_id, param_index, value);
        }
    }

    /// Maps the host's macro `slot` to a parameter, in place of whatever
    /// CC or macro turned it, and turns the knob to the macro at once.
    pub(super) fn map_to_macro(&mut self, mapping: MidiMapping) {
        let Some(slot) = mapping.macro_slot else { return };
        self.midi_mappings.retain(|m| !(m.node_id == mapping.node_id && m.param_index == mapping.param_index));
        self.user_state.set_macro_mapping(mapping.node_id, mapping.param_index, slot);
        self.status_message = Some(format!("{} follows Macro {}", mapping.param_name, slot + 1));
        self.midi_mappings.push(mapping);
        if let Some(hosted) = self.hosted.as_mut() {
            hosted.macros[slot as usize] = f32::NAN;
        }
    }
}
