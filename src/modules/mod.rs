//! Modules module
//!
//! Built-in synthesizer modules.
//! Includes oscillators, filters, envelopes, LFOs, utilities, and output modules.

pub mod arranger;
pub mod attenuverter;
pub mod audio_input;
pub mod chopper;
pub mod chord_sequencer;
pub mod chorus;
pub mod clock;
pub mod cloud;
pub mod controls;
pub mod compressor;
pub mod dancer;
pub mod delay;
pub mod divider;
pub mod distortion;
pub mod drum;
pub mod envelope;
pub mod eq;
pub mod instrument;
pub mod kit;
pub mod filter;
pub mod fm_voice;
pub mod ladder;
pub mod lfo;
pub mod link_out;
pub mod logic;
pub mod looper;
pub mod midi_monitor;
pub mod midi_out;
pub mod mix;
pub mod mixer;
pub mod noise;
pub mod note_in;
pub mod pads;
pub mod oscillator;
pub mod oscilloscope;
pub mod output;
pub mod quantizer;
pub mod resonator;
pub mod reverb;
pub mod sample_hold;
pub mod scenes;
pub mod sampler;
pub mod sequencer;
pub mod setlist;
pub mod slicer;
pub mod slope;
pub mod tape;
pub mod trigger_sequencer;
pub mod vca;
pub mod vocoder;

use crate::dsp::{ModuleRegistry, Poly};

/// Whether a module plays a WAV file its node names: the Sampler, the
/// Chopper and the Cloud. The editor loads, slices and drops files on them
/// alike (the Cloud has no Slices, so it's never sliced).
pub fn plays_a_sample(module_id: &str) -> bool {
    module_id == sampler::ID || module_id == chopper::ID || module_id == cloud::ID
}

/// Every built-in module, in add-node menu order: the one list a new module
/// joins. Modules wrapped in [`Poly`] are polyphonic: they run one voice per
/// channel of a polyphonic cable.
macro_rules! builtin_modules {
    ($($module:ty),* $(,)?) => {
        /// How many modules [`register_builtin`] registers.
        pub const BUILTIN_COUNT: usize = [$(stringify!($module)),*].len();

        /// Registers every built-in module, in add-node menu order.
        pub fn register_builtin(registry: &mut ModuleRegistry) {
            $(registry.register::<$module>();)*
        }
    };
}

builtin_modules! {
    Poly<Oscillator>,
    Poly<Noise>,
    Poly<Drum>,
    Poly<Resonator>,
    Sampler,
    Chopper,
    Cloud,
    AudioInput,
    NoteIn,
    Controls,
    Pads,
    Poly<SvfFilter>,
    Poly<LadderFilter>,
    Poly<AdsrEnvelope>,
    Lfo,
    Poly<Slope>,
    Clock,
    Poly<Vca>,
    Poly<Attenuverter>,
    Mix,
    Mixer,
    Poly<SampleHold>,
    Poly<Quantizer>,
    ClockDivider,
    Logic,
    Looper,
    Oscilloscope,
    Dancer,
    StepSequencer,
    ChordSequencer,
    TriggerSequencer,
    Arranger,
    Scenes,
    StereoDelay,
    Reverb,
    ParametricEq,
    Distortion,
    Chorus,
    Tape,
    Slicer,
    Vocoder,
    Compressor,
    MidiMonitor,
    AudioOutput,
    LinkOut,
    MidiOut,
    Instrument,
    Kit,
    Setlist,
    FmVoice,
}

// Re-export commonly used types
pub use arranger::Arranger;
pub use attenuverter::Attenuverter;
pub use audio_input::AudioInput;
pub use chopper::Chopper;
pub use chord_sequencer::ChordSequencer;
pub use chorus::Chorus;
pub use clock::Clock;
pub use cloud::Cloud;
pub use controls::Controls;
pub use pads::Pads;
pub use compressor::Compressor;
pub use dancer::Dancer;
pub use delay::StereoDelay;
pub use divider::ClockDivider;
pub use distortion::Distortion;
pub use drum::Drum;
pub use sampler::Sampler;
pub use envelope::AdsrEnvelope;
pub use eq::ParametricEq;
pub use filter::SvfFilter;
pub use instrument::Instrument;
pub use kit::Kit;
pub use ladder::LadderFilter;
pub use lfo::Lfo;
pub use link_out::LinkOut;
pub use midi_out::MidiOut;
pub use logic::Logic;
pub use looper::Looper;
pub use midi_monitor::MidiMonitor;
pub use mix::Mix;
pub use mixer::Mixer;
pub use noise::Noise;
pub use note_in::NoteIn;
pub use oscillator::Oscillator;
pub use oscilloscope::Oscilloscope;
pub use output::AudioOutput;
pub use quantizer::Quantizer;
pub use resonator::Resonator;
pub use reverb::Reverb;
pub use sample_hold::SampleHold;
pub use scenes::Scenes;
pub use sequencer::StepSequencer;
pub use setlist::Setlist;
pub use fm_voice::FmVoice;
pub use slicer::Slicer;
pub use slope::Slope;
pub use tape::Tape;
pub use trigger_sequencer::TriggerSequencer;
pub use vca::Vca;
pub use vocoder::Vocoder;
