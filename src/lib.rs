//! Soba Library
//!
//! Core library for the modular audio synthesizer.

#[cfg(feature = "ableton-link")]
pub mod ableton_link;
pub mod app;
pub mod dsp;
pub mod engine;
pub mod graph;
pub mod link;
pub mod modules;
pub mod persistence;
pub mod widgets;
