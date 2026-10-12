//! Engine module
//!
//! Audio engine and processing graph.
//! Handles cpal integration, graph compilation and processing, and MIDI input.

pub mod audio_engine;
pub mod audio_graph;
pub mod audio_input;
pub mod audio_processor;
pub mod channels;
pub mod commands;
pub mod cue;
pub mod graph_plan;
pub mod latency;
pub mod midi_engine;
pub mod midi_scheduler;
pub mod offline;
pub mod recorder;

pub use audio_engine::{AudioEngine, AudioError, AudioSystem, DeviceInfo, BUFFER_CHOICES};
pub use audio_graph::{AudioGraph, Connection};
pub use audio_input::{input_channel, input_channel_converting, input_channel_same_clock, InputFeed, InputMonitor, InputSender};
pub use audio_processor::{AudioProcessor, create_module_registry};
pub use channels::{
    EngineChannels, EngineHandle, UiHandle, DEFAULT_COMMAND_BUFFER_SIZE, DEFAULT_EVENT_BUFFER_SIZE,
    MAX_PLANS_IN_FLIGHT,
};
pub use commands::{AudioMessage, ChannelPeaks, EngineCommand, EngineEvent, NodeId, PortIndex, ScopeFrame};
pub use cue::{Cue, CUE_LEVEL};
pub use graph_plan::GraphPlan;
pub use latency::{LatencyGauge, RoundTrip};
pub use recorder::{RecordTap, Recording, RecordingSummary};
pub use offline::{read_wav, LimiterReport, OfflineRenderer, StereoBuffer, AUDITION};
pub use midi_engine::{
    MidiDeviceInfo, MidiEngine, MidiError, MidiEvent, MidiReceivers, TimestampedMidiEvent,
};
pub use midi_scheduler::{MidiScheduler, MAX_MIDI_EVENTS_PER_CALLBACK};
