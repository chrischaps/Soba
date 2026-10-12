//! Small DSP building blocks shared between modules.
//!
//! Everything here is allocation-free and safe to call on the audio thread.

pub mod adaa;
pub mod blep;
pub mod delay;
pub mod glide;
pub mod interpolate;
pub mod noise;
pub mod oversample;
pub mod saturation;
pub mod slices;
pub mod tape;
pub mod tape_stop;
pub mod tpt;

pub use blep::BlepDelay;
pub use delay::FracDelay;
pub use glide::{glide_parameters, Glide, GlideMode};
pub use interpolate::hermite;
pub use noise::NoiseFloor;
pub use adaa::{Adaa1, BiasedTanh, Curve, HardClip, RoundedFolder, Tanh};
pub use oversample::{Downsampler2x, Downsampler4x, Upsampler2x, Upsampler4x};
pub use saturation::{fast_tanh, SoftSaturator};
pub use slices::{Slices, MAX_SLICES};
pub use tape::{RecordHead, TapeTransport};
pub use tape_stop::{StopHeads, TapeStop};
pub use tpt::{prewarp, TptIntegrator};
