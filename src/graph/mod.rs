//! Graph module
//!
//! Node graph integration with egui_node_graph2.
//! Handles data types, node templates, connection validation, and custom rendering.

pub mod annotation_ui;
pub mod annotations;
pub mod catalog;
pub mod groups;
mod group_face;
mod clock_display;
mod data_types;
pub mod hints;
pub mod input_display;
mod dancer_display;
mod dancer_moves;
mod divider_display;
mod display_frame;
mod drum_display;
mod envelope_cv;
mod envelope_display;
mod filter_display;
pub mod knob_cv;
mod lfo_display;
mod logic_display;
mod oscillator_display;
mod looper_display;
mod slope_display;
mod tape_display;
mod vocoder_display;
mod trigger_display;
mod arranger_display;
pub mod arranger_follow;
mod chord_chime;
mod chord_ear;
mod chord_grid;
mod step_ear;
mod step_grid;
mod mixer_strips;
mod module_ui;
mod node_data;
pub mod port_mapping;
mod responses;
pub mod hovered_file;
pub mod resample;
pub mod sample_shelf;
mod sampler_display;
mod chopper_display;
mod sampler_slices;
pub mod sampler_take;
mod sampler_tidy;
pub mod signal_history;
mod state;
mod templates;
mod validation;
mod value_types;

pub use arranger_display::claim_wheel as claim_arranger_wheel;
pub use chord_ear::{letter_keys_held, letter_keys_taken};
pub use data_types::SynthDataType;
pub use groups::{GroupId, NodeKind};
pub use port_mapping::SynthGraph;
pub use node_data::{KnobParam, LedIndicator, NodeDisplay, SynthNodeData};
pub use responses::{SynthResponse, Tidy};
pub use state::{create_editor_state, DisplayMidiEvent, MidiMappingInfo, SamplerCue, SynthGraphEditorState, SynthGraphState};
pub use templates::{AllNodeTemplates, SynthNodeTemplate};
pub use validation::{validate_connection, types_compatible, ConnectionError, ValidationResult};
pub use value_types::{NumberSpec, SynthValueType};

// Re-export useful types from egui_node_graph2
pub use egui_node_graph2::{NodeId, InputId, OutputId, AnyParameterId};
