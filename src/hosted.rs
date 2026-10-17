//! Whether Soba runs inside a DAW, as the CLAP plugin (`plugin/`).
//!
//! The plugin says so once, before it makes any module, and a few things
//! change with it: the Clock's outside source is the host instead of
//! Ableton Link, knobs offer the host's macros, and the app leaves the
//! audio and MIDI devices, updates and Play/Stop to the DAW.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

static HOSTED: AtomicBool = AtomicBool::new(false);

/// Marks this process as a plugin's. Once set, it stays set.
pub fn set_hosted() {
    HOSTED.store(true, Relaxed);
}

/// Whether Soba is running as a plugin inside a DAW.
///
/// REAL-TIME SAFE: an atomic load.
#[inline]
pub fn is_hosted() -> bool {
    HOSTED.load(Relaxed)
}
