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
pub mod compressor;
pub mod dancer;
pub mod delay;
pub mod divider;
pub mod distortion;
pub mod drum;
pub mod envelope;
pub mod eq;
pub mod filter;
pub mod keyboard;
pub mod ladder;
pub mod lfo;
pub mod link_out;
pub mod logic;
pub mod looper;
pub mod midi_monitor;
pub mod midi_out;
pub mod mix;
pub mod mixer;
pub mod midi_note;
pub mod noise;
pub mod oscillator;
pub mod oscilloscope;
pub mod output;
pub mod poly_midi;
pub mod quantizer;
pub mod reverb;
pub mod sample_hold;
pub mod scenes;
pub mod sampler;
pub mod sequencer;
pub mod slicer;
pub mod slope;
pub mod tape;
pub mod trigger_sequencer;
pub mod vca;
pub mod vocoder;

/// Whether a module plays a WAV file its node names: the Sampler, and the
/// Chopper. The editor loads, slices and drops files on both alike.
pub fn plays_a_sample(module_id: &str) -> bool {
    module_id == sampler::SAMPLER_ID || module_id == chopper::CHOPPER_ID
}

// Re-export commonly used types
pub use arranger::Arranger;
pub use attenuverter::Attenuverter;
pub use audio_input::AudioInput;
pub use chopper::Chopper;
pub use chord_sequencer::ChordSequencer;
pub use chorus::Chorus;
pub use clock::Clock;
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
pub use keyboard::KeyboardInput;
pub use ladder::LadderFilter;
pub use lfo::Lfo;
pub use link_out::LinkOut;
pub use midi_out::MidiOut;
pub use logic::Logic;
pub use looper::Looper;
pub use midi_monitor::MidiMonitor;
pub use mix::Mix;
pub use mixer::Mixer;
pub use midi_note::MidiNote;
pub use noise::Noise;
pub use oscillator::Oscillator;
pub use oscilloscope::Oscilloscope;
pub use output::AudioOutput;
pub use poly_midi::PolyMidi;
pub use quantizer::Quantizer;
pub use reverb::Reverb;
pub use sample_hold::SampleHold;
pub use scenes::Scenes;
pub use sequencer::StepSequencer;
pub use slicer::Slicer;
pub use slope::Slope;
pub use tape::Tape;
pub use trigger_sequencer::TriggerSequencer;
pub use vca::Vca;
pub use vocoder::Vocoder;
