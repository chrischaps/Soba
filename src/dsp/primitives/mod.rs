//! Small DSP building blocks shared between modules.
//!
//! Everything here is allocation-free and safe to call on the audio thread.

pub mod adaa;
pub mod blep;
pub mod delay;
pub mod glide;
pub mod interpolate;
pub mod noise;
pub mod one_pole;
pub mod oversample;
pub mod saturation;
pub mod seam;
pub mod sinc;
pub mod slices;
pub mod svf;
pub mod tape;
pub mod tape_stop;
pub mod tpt;

pub use blep::BlepDelay;
pub use delay::FracDelay;
pub use glide::{glide_parameters, Glide, GlideMode};
pub use interpolate::hermite;
pub use noise::NoiseFloor;
pub use one_pole::{decay_for_seconds, rate_for_seconds, Follower, OnePole};
pub use adaa::{Adaa1, BiasedTanh, Curve, HardClip, RoundedFolder, Tanh};
pub use oversample::{Downsampler2x, Downsampler4x, Upsampler2x, Upsampler4x};
pub use saturation::{fast_tanh, SoftSaturator};
pub use seam::seam_weight;
pub use sinc::SincKernel;
pub use slices::{Markers, Slices, MAX_MARKERS, MAX_SLICES, ONSETS};
pub use svf::{Svf, SvfCoeffs, SvfOut};
pub use tape::{RecordHead, TapeTransport};
pub use tape_stop::{StopHeads, TapeStop};
pub use tpt::{prewarp, CutoffGain, TptIntegrator, TptOnePole};
