//! Engine Commands and Events
//!
//! Defines the messages that flow between the UI thread and the audio engine thread.
//! All types here must be Send + 'static for safe cross-thread communication.

use std::sync::Arc;

use crate::dsp::primitives::Markers;
use crate::dsp::{Audition, MeterLevels, OutputLevels, Readout, SampleData, SignalBuffer, Snapshot, MAX_CHANNELS};
use crate::modules::oscilloscope::SCOPE_BUFFER_SIZE;
use crate::modules::scenes::SceneKnob;

use super::audio_input::InputFeed;
use super::cue::Cue;
use super::probe::Probe;
use super::graph_plan::GraphPlan;
use super::recorder::RecordTap;

/// Unique identifier for a node in the audio graph.
/// Maps to the node ID from egui_node_graph2.
pub type NodeId = u64;

/// Index of a port on a module.
pub type PortIndex = usize;

/// Commands sent from the UI thread to the audio engine.
/// These are processed non-blocking in the audio callback.
// Not boxed: a box would be freed on the audio thread
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum EngineCommand {
    /// Add a new module instance to the audio graph.
    AddModule {
        /// Unique identifier for this node instance.
        node_id: NodeId,
        /// Static string ID of the module type (from ModuleRegistry).
        module_id: &'static str,
    },

    /// Start a module's random numbers from `seed`, before it first plays.
    /// A patch seeds each module by its ID in the file, so it plays the same
    /// noise in the editor, in a capture and in the render tool; a module
    /// that isn't seeded draws from its node ID.
    SeedModule {
        /// The module to seed, just added.
        node_id: NodeId,
        /// Where its random numbers start.
        seed: u64,
    },

    /// Remove a module from the audio graph.
    RemoveModule {
        /// The node to remove.
        node_id: NodeId,
    },

    /// Connect two ports in the audio graph.
    Connect {
        /// Source node.
        from_node: NodeId,
        /// Output port index on source node.
        from_port: PortIndex,
        /// Destination node.
        to_node: NodeId,
        /// Input port index on destination node.
        to_port: PortIndex,
    },

    /// Disconnect a specific connection.
    Disconnect {
        /// Node with the connection to remove.
        node_id: NodeId,
        /// Port index to disconnect.
        port: PortIndex,
        /// Whether this is an input port (true) or output port (false).
        is_input: bool,
    },

    /// Set a parameter value on a module.
    SetParameter {
        /// Target node.
        node_id: NodeId,
        /// Parameter index.
        param_index: usize,
        /// New value in the parameter's real units (Hz, seconds, dB, ...),
        /// within the range its `ParameterDefinition` declares.
        value: f32,
    },

    /// Take a filter or effect out of the signal path, or put it back.
    /// Ignored for modules that can't be bypassed.
    SetBypass {
        /// Target node.
        node_id: NodeId,
        /// True to pass the module's audio input straight to its output.
        bypassed: bool,
    },

    /// Start or stop audio processing.
    SetPlaying(bool),

    /// Clear the entire audio graph.
    ClearGraph,

    /// Start monitoring an input port for UI feedback.
    /// The engine will send InputValue events with the signal values.
    MonitorInput {
        /// The node containing the input.
        node_id: NodeId,
        /// The input port index to monitor.
        input_index: PortIndex,
    },

    /// Stop monitoring an input port.
    UnmonitorInput {
        /// The node containing the input.
        node_id: NodeId,
        /// The input port index to stop monitoring.
        input_index: PortIndex,
    },

    /// Start monitoring an output port for UI feedback (e.g., LED indicators).
    /// The engine will send OutputValue events with the signal values.
    MonitorOutput {
        /// The node containing the output.
        node_id: NodeId,
        /// The output port index to monitor.
        output_index: PortIndex,
    },

    /// Stop monitoring an output port.
    UnmonitorOutput {
        /// The node containing the output.
        node_id: NodeId,
        /// The output port index to stop monitoring.
        output_index: PortIndex,
    },

    /// Give a module a recording to play (a Sampler's file), or take it
    /// away with `None`. Ignored by modules that don't play recordings.
    LoadSample {
        /// Target node.
        node_id: NodeId,
        /// The recording, at the engine's sample rate.
        sample: Option<Arc<SampleData>>,
    },

    /// Give a module the markers its recording is cut at (a Sampler's,
    /// sliced by Onsets). Ignored by modules that don't slice.
    SetMarkers {
        /// Target node.
        node_id: NodeId,
        markers: Markers,
    },

    /// Give a module room to record a take into (a Sampler's Rec): an
    /// empty recording with room set aside, at the engine's sample rate,
    /// that nothing else holds. Ignored by modules that don't record.
    GiveRoom {
        /// Target node.
        node_id: NodeId,
        room: Arc<SampleData>,
    },

    /// Swap the room a module recorded take `take` into for the same
    /// recording made to measure (see
    /// [`DspModule::settle_take`](crate::dsp::DspModule::settle_take)).
    SettleTake {
        /// Target node.
        node_id: NodeId,
        take: u32,
        sample: Arc<SampleData>,
    },

    /// Give a Scenes module the knobs its scenes hold, to turn as its
    /// Morph moves, replacing those it had. Ignored by other modules.
    SetScenes {
        /// The Scenes module.
        node_id: NodeId,
        knobs: Vec<SceneKnob>,
    },

    /// Play a chord on a module's outputs from its face, or stop with
    /// `None` (see [`DspModule::audition`](crate::dsp::DspModule::audition)).
    /// Played, not edited: nothing of it is kept in the graph.
    Audition {
        /// Target node.
        node_id: NodeId,
        chord: Option<Audition>,
    },
}

/// Messages delivered to the audio thread.
///
/// The UI sends [`EngineCommand`]s; structural ones are applied to the
/// [`AudioGraph`](super::AudioGraph) on the UI side and arrive here as a whole
/// compiled plan. Plans and parameter changes share one queue so they apply
/// in the order they were sent.
pub enum AudioMessage {
    /// Swap in a newly compiled graph.
    InstallPlan(Box<GraphPlan>),
    /// Change something in the running plan (see [`GraphPlan::apply`]).
    Plan(PlanMessage),
    /// Start or stop audio processing.
    SetPlaying(bool),
    /// Start copying the output into a recording's ring. A tap already
    /// running is handed back first.
    StartRecording(RecordTap),
    /// Hand the recording's tap back to the UI, which ends the recording.
    StopRecording,
    /// Start reading the patch's audio input from this feed. A feed already
    /// connected is handed back first.
    ConnectInput(InputFeed),
    /// Hand the audio input's feed back to the UI.
    DisconnectInput,
    /// Copy a module's recording (a Looper's loop) into room the UI set
    /// aside, a piece each callback, and hand it back when it's done.
    SnapshotLoop { node_id: NodeId, snapshot: Box<Snapshot> },
    /// Hear a recording from a module's face, beside the patch, playing or
    /// stopped; or with `None`, stop the one playing.
    Cue(Option<Cue>),
    /// Hear what a cable carries, beside the patch, or with `None`, stop.
    Probe(Option<Probe>),
}

/// Changes to a module in the running plan: the part of an
/// [`AudioMessage`] the live engine, the offline renderer and the tests
/// all apply alike, through [`GraphPlan::apply`].
// Not boxed: a box would be freed on the audio thread
#[allow(clippy::large_enum_variant)]
pub enum PlanMessage {
    /// Set a parameter on a running module.
    SetParameter {
        node_id: NodeId,
        param_index: usize,
        value: f32,
    },
    /// Bypass a running module, or bring it back.
    SetBypass { node_id: NodeId, bypassed: bool },
    /// Play a chord on a running module's outputs, or stop.
    Audition { node_id: NodeId, chord: Option<Audition> },
    /// Give a running module a recording to play, or take its away. The
    /// recording it had goes back to the UI to be dropped.
    LoadSample { node_id: NodeId, sample: Option<Arc<SampleData>> },
    /// Give a running module the markers its recording is cut at.
    SetMarkers { node_id: NodeId, markers: Markers },
    /// Give a running module room to record a take into. Room it's done
    /// with goes back to the UI to be dropped.
    GiveRoom { node_id: NodeId, room: Arc<SampleData> },
    /// Swap a running module's take for the same recording made to
    /// measure. What it's done with goes back to the UI to be dropped.
    SettleTake { node_id: NodeId, take: u32, sample: Arc<SampleData> },
}

impl PlanMessage {
    /// The module the message is for.
    pub fn node_id(&self) -> NodeId {
        match *self {
            Self::SetParameter { node_id, .. }
            | Self::SetBypass { node_id, .. }
            | Self::Audition { node_id, .. }
            | Self::LoadSample { node_id, .. }
            | Self::SetMarkers { node_id, .. }
            | Self::GiveRoom { node_id, .. }
            | Self::SettleTake { node_id, .. } => node_id,
        }
    }
}

/// One oscilloscope capture, sent from the audio thread by value so that
/// sending it allocates nothing.
pub struct ScopeFrame {
    pub node_id: NodeId,
    pub channel1: [f32; SCOPE_BUFFER_SIZE],
    pub channel2: [f32; SCOPE_BUFFER_SIZE],
    pub channel1_len: usize,
    pub channel2_len: usize,
    pub triggered: bool,
}

impl ScopeFrame {
    /// Copies a capture into a frame, truncating channels that don't fit.
    pub fn new(node_id: NodeId, channel1: &[f32], channel2: &[f32], triggered: bool) -> Self {
        let mut frame = Self {
            node_id,
            channel1: [0.0; SCOPE_BUFFER_SIZE],
            channel2: [0.0; SCOPE_BUFFER_SIZE],
            channel1_len: channel1.len().min(SCOPE_BUFFER_SIZE),
            channel2_len: channel2.len().min(SCOPE_BUFFER_SIZE),
            triggered,
        };
        frame.channel1[..frame.channel1_len].copy_from_slice(&channel1[..frame.channel1_len]);
        frame.channel2[..frame.channel2_len].copy_from_slice(&channel2[..frame.channel2_len]);
        frame
    }

    /// Converts the frame into the event the UI consumes.
    pub fn into_event(self) -> EngineEvent {
        EngineEvent::ScopeBuffer {
            node_id: self.node_id,
            channel1: self.channel1[..self.channel1_len].into(),
            channel2: self.channel2[..self.channel2_len].into(),
            triggered: self.triggered,
        }
    }
}

/// How much of its time budget the audio callback used, in percent (100 is
/// the whole buffer's worth of time: the device would have run dry).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuLoad {
    /// The smoothed load, for the meter's bar.
    pub average: f32,
    /// The slowest single callback since the last reading. One slow callback
    /// is what drops out, and the average hides it.
    pub peak: f32,
}

/// Events sent from the audio engine to the UI thread.
/// These provide feedback for metering and status display.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// Output stage meter readings (pre/post-limiter peaks and gain
    /// reduction) for the most recent audio callback.
    OutputLevel(OutputLevels),

    /// How much of each callback's time the audio processing took.
    CpuLoad(CpuLoad),

    /// Audio processing started.
    Started,

    /// Audio processing stopped.
    Stopped,

    /// An error occurred in the audio engine.
    Error,

    /// Reports the current value at a monitored input port.
    /// Used for animating knobs when their input is connected.
    InputValue {
        /// The node containing the input.
        node_id: NodeId,
        /// The input port index.
        input_index: PortIndex,
        /// The sampled value (typically first sample or average of block).
        value: f32,
    },

    /// Reports the current value at a monitored output port.
    /// Used for LED indicators and other output visualizations.
    OutputValue {
        /// The node containing the output.
        node_id: NodeId,
        /// The output port index.
        output_index: PortIndex,
        /// The sample with the largest magnitude in the block, across every
        /// channel, sign preserved.
        value: f32,
        /// The same reading for each channel, so the UI can draw a
        /// polyphonic cable strand by strand.
        channels: ChannelPeaks,
    },

    /// A module's own meter readings (a mixer's channel strips), for the
    /// most recent audio callback.
    MeterLevels {
        /// The module metering.
        node_id: NodeId,
        /// Its peaks, in the module's own order.
        levels: MeterLevels,
    },

    /// A module's live readout (a Clock's tempo and beat), for its node
    /// display, as of the most recent audio callback.
    Readout {
        /// The module reporting.
        node_id: NodeId,
        /// Its values, in the module's own order.
        readout: Readout,
    },

    /// Where the cue heard from a module's face has got to, 0 to 1 through
    /// its recording: after each callback while it plays, and once with
    /// `None` when it ends or is stopped.
    Cue {
        /// The node it's heard from.
        node_id: NodeId,
        place: Option<f32>,
    },

    /// A module made a NaN or infinity, and was silenced and reset for the
    /// block so it couldn't latch downstream. Sent once per callback it
    /// happens in, so the editor can show which module it was.
    ModuleFault {
        /// The module that made it.
        node_id: NodeId,
    },

    /// Oscilloscope buffer data for waveform display.
    /// Sent when an oscilloscope module captures a triggered waveform.
    ScopeBuffer {
        /// The oscilloscope node that captured this data.
        node_id: NodeId,
        /// Channel 1 waveform samples.
        channel1: Box<[f32]>,
        /// Channel 2 waveform samples (empty if not connected).
        channel2: Box<[f32]>,
        /// Whether this capture was triggered (true) or free-running (false).
        triggered: bool,
    },
}

/// The peak of each channel of an output over one block: the sample with the
/// largest magnitude, sign preserved, so a cable can draw a bipolar signal
/// such as an LFO swinging either side of zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelPeaks {
    count: u8,
    peaks: [f32; MAX_CHANNELS],
}

impl Default for ChannelPeaks {
    /// One silent channel.
    fn default() -> Self {
        Self { count: 1, peaks: [0.0; MAX_CHANNELS] }
    }
}

impl ChannelPeaks {
    /// Reads the peaks of every channel `buffer` carries.
    ///
    /// REAL-TIME SAFE.
    pub fn of(buffer: &SignalBuffer) -> Self {
        let mut peaks = [0.0; MAX_CHANNELS];
        let count = buffer.channels().min(MAX_CHANNELS);
        for (channel, peak) in peaks[..count].iter_mut().enumerate() {
            *peak = largest_magnitude(buffer.voice(channel).samples.iter().copied());
        }
        Self { count: count as u8, peaks }
    }

    /// How many channels the output carried.
    pub fn count(&self) -> usize {
        self.count as usize
    }

    /// The peak of channel `channel` (counting from 0), or silence past
    /// [`count`](Self::count).
    pub fn peak(&self, channel: usize) -> f32 {
        if channel < self.count() {
            self.peaks[channel]
        } else {
            0.0
        }
    }

    /// The peak across every channel.
    pub fn overall(&self) -> f32 {
        largest_magnitude(self.peaks[..self.count()].iter().copied())
    }

    /// Each channel's larger peak of this reading and `other`, over the
    /// channels of the later one, `other`.
    pub fn louder(&self, other: &Self) -> Self {
        let mut peaks = other.peaks;
        for (channel, peak) in peaks[..other.count()].iter_mut().enumerate() {
            *peak = largest_magnitude([self.peak(channel), *peak].into_iter());
        }
        Self { count: other.count, peaks }
    }
}

/// The value with the largest magnitude, sign preserved, or 0 when empty.
fn largest_magnitude(values: impl Iterator<Item = f32>) -> f32 {
    values.fold(0.0_f32, |acc, value| if value.abs() > acc.abs() { value } else { acc })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_peaks_mono() {
        let mut buffer = SignalBuffer::new(4, crate::dsp::SignalType::Audio);
        buffer.samples.copy_from_slice(&[0.1, -0.7, 0.5, 0.0]);
        let peaks = ChannelPeaks::of(&buffer);
        assert_eq!(peaks.count(), 1);
        assert_eq!(peaks.peak(0), -0.7);
        assert_eq!(peaks.peak(1), 0.0, "silent past the channel count");
        assert_eq!(peaks.overall(), -0.7);
    }

    #[test]
    fn test_channel_peaks_poly() {
        let mut buffer = SignalBuffer::polyphonic(4, crate::dsp::SignalType::Audio);
        buffer.set_channels(3);
        buffer.samples.fill(0.25);
        buffer.channel_mut(1).fill(-0.5);
        buffer.channel_mut(2).copy_from_slice(&[0.0, 0.9, 0.0, -0.1]);
        let peaks = ChannelPeaks::of(&buffer);
        assert_eq!(peaks.count(), 3);
        assert_eq!([peaks.peak(0), peaks.peak(1), peaks.peak(2)], [0.25, -0.5, 0.9]);
        assert_eq!(peaks.peak(3), 0.0);
        assert_eq!(peaks.overall(), 0.9);
    }

    #[test]
    fn test_command_debug() {
        let cmd = EngineCommand::SetPlaying(true);
        assert!(format!("{:?}", cmd).contains("SetPlaying"));
    }

    #[test]
    fn test_command_clone() {
        let cmd = EngineCommand::AddModule {
            node_id: 42,
            module_id: "sine_osc",
        };
        let cloned = cmd.clone();
        if let EngineCommand::AddModule { node_id, module_id } = cloned {
            assert_eq!(node_id, 42);
            assert_eq!(module_id, "sine_osc");
        } else {
            panic!("Clone failed");
        }
    }

    #[test]
    fn test_event_clone() {
        let levels = OutputLevels { pre: [0.9, 1.4], post: [0.5, 0.7], limiter_gain: 0.6 };
        let event = EngineEvent::OutputLevel(levels);
        let cloned = event.clone();
        if let EngineEvent::OutputLevel(cloned) = cloned {
            assert_eq!(cloned, levels);
        } else {
            panic!("Clone failed");
        }
    }

    #[test]
    fn test_scope_buffer_event() {
        let event = EngineEvent::ScopeBuffer {
            node_id: 1,
            channel1: vec![0.0, 0.5, 1.0, 0.5, 0.0].into_boxed_slice(),
            channel2: vec![].into_boxed_slice(),
            triggered: true,
        };
        if let EngineEvent::ScopeBuffer { node_id, channel1, channel2, triggered } = event {
            assert_eq!(node_id, 1);
            assert_eq!(channel1.len(), 5);
            assert!(channel2.is_empty());
            assert!(triggered);
        } else {
            panic!("Wrong event type");
        }
    }

    #[test]
    fn test_connect_command() {
        let cmd = EngineCommand::Connect {
            from_node: 1,
            from_port: 0,
            to_node: 2,
            to_port: 1,
        };
        if let EngineCommand::Connect {
            from_node,
            from_port,
            to_node,
            to_port,
        } = cmd
        {
            assert_eq!(from_node, 1);
            assert_eq!(from_port, 0);
            assert_eq!(to_node, 2);
            assert_eq!(to_port, 1);
        }
    }

    #[test]
    fn test_set_parameter_command() {
        let cmd = EngineCommand::SetParameter {
            node_id: 5,
            param_index: 2,
            value: 0.75,
        };
        if let EngineCommand::SetParameter {
            node_id,
            param_index,
            value,
        } = cmd
        {
            assert_eq!(node_id, 5);
            assert_eq!(param_index, 2);
            assert!((value - 0.75).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_command_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<EngineCommand>();
    }

    #[test]
    fn test_event_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<EngineEvent>();
    }
}
