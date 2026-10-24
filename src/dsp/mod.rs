//! DSP module
//!
//! Core DSP traits and types.
//! Defines the DspModule trait, ports, parameters, and signal types.

pub mod analysis;
pub mod beat_tracker;
pub mod bypass;
pub mod context;
pub mod decibels;
pub mod denormal;
pub mod dynamics;
pub mod finite;
pub mod gate;
pub mod loudness;
pub mod module_trait;
pub mod note_claims;
pub mod parameter;
pub mod poly;
pub mod port;
pub mod primitives;
pub mod registry;
pub mod sample;
pub mod signal;
pub mod smoothed_value;
pub mod tempo;
pub mod onsets;
pub mod tidy;

// Re-export commonly used types
pub use decibels::{db_to_gain, gain_to_db};
pub use context::{HostTransport, InputAudio, ProcessContext, TransportState};
pub use module_trait::{Audition, DspModule, FinishedTake, MeterLevels, ModuleCategory, ModuleError, ModuleInfo, OutputLevels, Readout, SpectrumPorts, MAX_METERS, MAX_READOUT};
pub use note_claims::{NoteClaims, NO_CLAIMS};
pub use parameter::{ParameterDefinition, ParameterDisplay};
pub use poly::Poly;
pub use port::{PortDefinition, PortDirection};
pub use registry::{ModuleFactory, ModuleRegistry};
pub use sample::{SampleData, Snapshot, SnapshotOutcome, MAX_SAMPLE_SECONDS};
pub use signal::{connected_input, MidiEvent, MidiMessage, SignalBuffer, SignalType, KEYS_CHANNEL, MAX_CHANNELS};
pub use smoothed_value::SmoothedValue;
