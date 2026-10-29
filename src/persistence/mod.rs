//! Persistence module
//!
//! Patch save/load functionality using serde and JSON.

pub mod compile;
pub mod examples;
#[cfg(target_arch = "wasm32")]
mod example_fetch;
pub mod graph_io;
pub mod library;
mod opus;
pub mod pads_group;
pub mod controls_group;
pub mod patch;
pub mod patch_link;
pub mod sample_files;
pub mod short_link;
pub mod stored_zip;
#[cfg(target_arch = "wasm32")]
pub mod tab_store;

pub use compile::{compile_patch, CompiledPatch};
pub use examples::{Example, Shelf, EXAMPLES};
pub use library::{LibraryGroup, Section, LIBRARY};
pub use graph_io::{
    capture_level, capture_patch, merge_patch, renumber_groups, stage_patch, CapturedLevel, Merged, StagedNode, StagedPart,
    StagedPatch,
};

pub use patch::{
    ConnectionData, FrameData, GroupData, JackData, MidiControl, MidiEncoding, MidiInput, MidiMapping, MidiSource,
    NamedParameter, NodeData, NoteData, ParameterValue,
    Patch, PatchError,
    load_from_file, migrate_v2_to_v3, patch_from_json, patch_to_json, save_to_file, PATCH_VERSION,
};
