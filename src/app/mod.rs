//! Application module
//!
//! Contains the main egui application, theme definitions, and UI state management.

#[cfg(feature = "asio")]
mod asio_badge;
pub mod capture;
mod capture_aim;
pub(crate) mod editing;
pub mod engine_sync;
mod input_device;
pub mod lesson;
pub mod library;
mod long_press;
pub mod midi_mappings;
mod midi_session;
mod mutate;
mod notice;
mod palette;
pub mod pace;
mod qwerty_keys;
mod recording;
mod session;
pub mod shortcuts;
mod status_line;
pub mod synth_app;
pub mod theme;
pub mod undo;
/// New versions: the check, the install and the restart. Not in the browser,
/// which always loads the latest
#[cfg(not(target_arch = "wasm32"))]
pub mod update;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// Whether this is the browser build, where there are no files, threads
/// or MIDI devices: see [`web`].
pub const WEB: bool = cfg!(target_arch = "wasm32");

pub use synth_app::SynthApp;
