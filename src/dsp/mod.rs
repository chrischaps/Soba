//! DSP module
//!
//! Core DSP traits and types.
//! Defines the DspModule trait, ports, parameters, and signal types.

pub mod analysis;
pub mod bypass;
pub mod context;
pub mod denormal;
pub mod dynamics;
pub mod module_trait;
pub mod parameter;
pub mod poly;
pub mod port;
pub mod primitives;
pub mod registry;
pub mod sample;
pub mod signal;
pub mod smoothed_value;

// Re-export commonly used types
pub use context::{InputAudio, ProcessContext, TransportState};
pub use module_trait::{Audition, DspModule, MeterLevels, ModuleCategory, ModuleError, ModuleInfo, OutputLevels, Readout, MAX_METERS, MAX_READOUT};
pub use parameter::{ParameterDefinition, ParameterDisplay};
pub use poly::Poly;
pub use port::{PortDefinition, PortDirection};
pub use registry::{ModuleFactory, ModuleRegistry};
pub use sample::{SampleData, Snapshot, SnapshotOutcome, MAX_SAMPLE_SECONDS};
pub use signal::{connected_input, MidiEvent, MidiMessage, SignalBuffer, SignalType, MAX_CHANNELS};
pub use smoothed_value::SmoothedValue;
