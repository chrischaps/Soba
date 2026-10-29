//! Engine Channels
//!
//! Lock-free communication between the UI thread and audio engine thread.
//! Uses rtrb ring buffers for SPSC (single-producer, single-consumer) queues:
//!
//! - **messages** (UI -> audio): compiled plans, parameter changes, play/stop
//! - **retired plans** (audio -> UI): replaced plans, to be dropped off the
//!   audio thread
//! - **retired taps** (audio -> UI): recording taps the audio thread is done
//!   with, for the same reason
//! - **retired inputs** (audio -> UI): audio input feeds, likewise
//! - **retired samples** (audio -> UI): recordings modules have let go of,
//!   so a long file is never freed in the audio callback
//! - **snapshots** (audio -> UI): copies of Loopers' loops, made into room
//!   the UI set aside, coming back to be saved
//! - **takes** (audio -> UI): takes Samplers have just recorded, shared,
//!   for the UI to keep with the patch
//! - **events** (audio -> UI): output metering, CPU load, status
//! - **monitor boards** (audio -> UI): the newest monitor values, module
//!   meters and readouts, one board per plan (see [`super::monitor_board`])
//! - **scope frames** (audio -> UI): oscilloscope captures, by value
//!
//! The UI side owns the [`AudioGraph`]. Structural commands edit it there,
//! and [`UiHandle::flush`] ships the result to the audio thread as a
//! compiled [`GraphPlan`], so nothing on the audio side ever allocates.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, PushError, RingBuffer};

use super::audio_graph::AudioGraph;
use super::audio_processor::create_module_registry;
use super::commands::{AudioMessage, EngineCommand, EngineEvent, NodeId, PlanMessage, ScopeFrame};
use crate::dsp::{FinishedTake, SampleData, Snapshot};
use super::graph_plan::GraphPlan;
use super::monitor_board::Monitors;
use super::audio_input::InputFeed;
use super::recorder::RecordTap;

/// Default buffer size for the message queue (UI -> Engine).
pub const DEFAULT_COMMAND_BUFFER_SIZE: usize = 1024;

/// Default buffer size for event queue (Engine -> UI).
pub const DEFAULT_EVENT_BUFFER_SIZE: usize = 256;

/// The most compiled plans that may be on their way to, or held by, the
/// audio thread at once. This bounds the queue that returns retired plans,
/// so the audio thread always has room to hand one back.
pub const MAX_PLANS_IN_FLIGHT: usize = 4;

/// Recording taps that can be on their way back at once. The UI starts a
/// recording only once the last one's tap is back, so one would do.
const RETIRED_TAP_BUFFER_SIZE: usize = 4;

/// Audio input feeds that can be on their way back at once: one for each
/// time the input device is changed between two UI frames.
const RETIRED_INPUT_BUFFER_SIZE: usize = 4;

/// Recordings that can be on their way back at once. Each load sends at
/// most one back, and a Sampler fading out of an old recording one more.
const RETIRED_SAMPLE_BUFFER_SIZE: usize = 64;

/// Loop snapshots that can be on their way to the audio thread, or back,
/// at once: more than a patch has Loopers. The audio thread holds as many
/// while it copies them.
pub const MAX_SNAPSHOTS: usize = 64;

/// Finished takes that can wait for the UI: more than a patch records
/// between two UI frames.
const TAKE_BUFFER_SIZE: usize = 16;

/// Oscilloscope captures that can wait for the UI: room for every
/// oscillator's trace (each sends up to 30 a second) between two UI frames,
/// with a Scope's alongside.
const SCOPE_FRAME_BUFFER_SIZE: usize = 32;

/// Sample rate and block size, shared so the UI prepares new modules for the
/// settings the audio thread is actually running at.
struct AudioConfig {
    sample_rate_bits: AtomicU32,
    block_size: AtomicUsize,
    /// The round trip live input takes from the input jack to the
    /// speakers, in frames, or 0 with no input open.
    input_latency: AtomicUsize,
}

impl AudioConfig {
    fn new(sample_rate: f32, block_size: usize) -> Self {
        Self {
            sample_rate_bits: AtomicU32::new(sample_rate.to_bits()),
            block_size: AtomicUsize::new(block_size),
            input_latency: AtomicUsize::new(0),
        }
    }

    fn load(&self) -> (f32, usize) {
        (
            f32::from_bits(self.sample_rate_bits.load(Ordering::Acquire)),
            self.block_size.load(Ordering::Acquire),
        )
    }

    fn store(&self, sample_rate: f32, block_size: usize) {
        self.sample_rate_bits.store(sample_rate.to_bits(), Ordering::Release);
        self.block_size.store(block_size, Ordering::Release);
    }
}

/// Play/stop changes waiting for room in the message queue, or a play
/// waiting for the graph edits made before it, collapsed to what the audio
/// thread still needs to see: at most a stop, then a play.
#[derive(Debug, Default)]
struct UnsentTransport {
    /// A stop to deliver before `playing`, so its reset of module state
    /// isn't skipped by a quick stop-then-play.
    stop_first: bool,
    playing: Option<bool>,
}

impl UnsentTransport {
    fn push(&mut self, playing: bool) {
        self.stop_first = playing && (self.stop_first || self.playing == Some(false));
        self.playing = Some(playing);
    }

    fn peek(&self) -> Option<bool> {
        if self.stop_first {
            Some(false)
        } else {
            self.playing
        }
    }

    fn pop(&mut self) {
        if self.stop_first {
            self.stop_first = false;
        } else {
            self.playing = None;
        }
    }
}

/// Both ends of the engine's communication channels.
pub struct EngineChannels {
    ui: UiHandle,
    engine: EngineHandle,
}

impl EngineChannels {
    /// Create new engine channels with the specified buffer sizes.
    ///
    /// # Arguments
    /// * `command_capacity` - Number of messages the UI -> engine queue can hold
    /// * `event_capacity` - Number of events the engine -> UI queue can hold
    pub fn new(command_capacity: usize, event_capacity: usize) -> Self {
        let (message_tx, message_rx) = RingBuffer::new(command_capacity);
        let (retired_tx, retired_rx) = RingBuffer::new(MAX_PLANS_IN_FLIGHT);
        let (event_tx, event_rx) = RingBuffer::new(event_capacity);
        let (scope_tx, scope_rx) = RingBuffer::new(SCOPE_FRAME_BUFFER_SIZE);
        let (retired_tap_tx, retired_tap_rx) = RingBuffer::new(RETIRED_TAP_BUFFER_SIZE);
        let (retired_input_tx, retired_input_rx) = RingBuffer::new(RETIRED_INPUT_BUFFER_SIZE);
        let (retired_sample_tx, retired_sample_rx) = RingBuffer::new(RETIRED_SAMPLE_BUFFER_SIZE);
        let (snapshot_tx, snapshot_rx) = RingBuffer::new(MAX_SNAPSHOTS);
        let (take_tx, take_rx) = RingBuffer::new(TAKE_BUFFER_SIZE);

        // Placeholder settings until an AudioProcessor reports the real ones
        let graph = AudioGraph::with_registry(44100.0, 256, create_module_registry());
        let (sample_rate, block_size) = (graph.sample_rate(), graph.block_size());
        let config = Arc::new(AudioConfig::new(sample_rate, block_size));

        Self {
            ui: UiHandle {
                graph,
                message_tx,
                retired_rx,
                retired_tap_rx,
                retired_input_rx,
                retired_sample_rx,
                snapshot_rx,
                snapshots_out: 0,
                take_rx,
                event_rx,
                scope_rx,
                monitors: Monitors::default(),
                config: Arc::clone(&config),
                unsent_plan: None,
                unsent_transport: UnsentTransport::default(),
                unsent_handoffs: Vec::new(),
                unsent_samples: Vec::new(),
                // The processor starts with an empty plan of its own, which
                // it retires to us like any other
                plans_in_flight: 1,
            },
            engine: EngineHandle {
                message_rx,
                retired_tx,
                retired_tap_tx,
                retired_input_tx,
                retired_sample_tx,
                snapshot_tx,
                take_tx,
                event_tx,
                scope_tx,
                config,
            },
        }
    }

    /// Create new channels with default buffer sizes.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_COMMAND_BUFFER_SIZE, DEFAULT_EVENT_BUFFER_SIZE)
    }

    /// Split the channels into UI-side and Engine-side handles.
    /// This consumes self and returns two handles that can be sent to different threads.
    pub fn split(self) -> (UiHandle, EngineHandle) {
        (self.ui, self.engine)
    }
}

/// UI-side handle for communicating with the audio engine.
///
/// Owns the [`AudioGraph`]. Graph edits are applied to it immediately and
/// reach the audio thread on the next [`flush`](Self::flush), batched into a
/// single compiled plan. Parameter changes and play/stop go straight through.
/// Nothing sent through it is dropped when the queue is full.
pub struct UiHandle {
    graph: AudioGraph,
    message_tx: Producer<AudioMessage>,
    retired_rx: Consumer<Box<GraphPlan>>,
    retired_tap_rx: Consumer<RecordTap>,
    retired_input_rx: Consumer<InputFeed>,
    retired_sample_rx: Consumer<Arc<SampleData>>,
    snapshot_rx: Consumer<(NodeId, Box<Snapshot>)>,
    /// Snapshots asked for and not yet back.
    snapshots_out: usize,
    take_rx: Consumer<(NodeId, FinishedTake)>,
    event_rx: Consumer<EngineEvent>,
    scope_rx: Consumer<ScopeFrame>,
    /// Where each plan's taps, meters and readouts are read
    monitors: Monitors,
    config: Arc<AudioConfig>,
    /// A compiled plan that didn't fit in the queue yet. Plans must arrive in
    /// order, so no newer plan is compiled until this one is sent.
    unsent_plan: Option<Box<GraphPlan>>,
    /// Play/stop changes that didn't fit in the queue yet.
    unsent_transport: UnsentTransport,
    /// Recording starts and stops, audio inputs connected and
    /// disconnected, and chords auditioned and cues played, that didn't
    /// fit in the queue yet, in order.
    unsent_handoffs: Vec<AudioMessage>,
    /// Recordings for running modules, and snapshots of them, waiting
    /// until the plans holding those modules are sent, and then for room in
    /// the queue, in order.
    unsent_samples: Vec<AudioMessage>,
    /// Plans sent (or held by the audio thread) and not yet returned.
    plans_in_flight: usize,
}

impl UiHandle {
    /// Send a command to the audio engine.
    ///
    /// No command is ever lost, and this never waits for space:
    /// - Graph edits are applied to the UI-side graph and delivered by
    ///   [`flush`](Self::flush) as a compiled plan, held back until the queue
    ///   has room.
    /// - Parameter and bypass changes are queued immediately. If the queue is
    ///   full the value is still recorded in the graph, and travels with the
    ///   next plan.
    /// - Play/stop is queued immediately, or kept until `flush` finds room.
    ///   A play also waits for the plan holding any graph edits sent before
    ///   it, so a patch opened while playing starts as it was saved, not
    ///   from its modules' defaults or with a moment of the old patch.
    pub fn send_command(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::SetParameter { node_id, param_index, value } => {
                self.graph.set_parameter(node_id, param_index, value);
                let message = AudioMessage::Plan(PlanMessage::SetParameter { node_id, param_index, value });
                // If the queue is full, the value travels with the next plan
                // instead. A held-back plan has the value from when it was
                // compiled and would undo this one when it's installed, so a
                // newer plan has to follow it
                if self.message_tx.push(message).is_err() || self.unsent_plan.is_some() {
                    self.graph.mark_dirty();
                }
            }
            EngineCommand::SetBypass { node_id, bypassed } => {
                self.graph.set_bypass(node_id, bypassed);
                if self.message_tx.push(AudioMessage::Plan(PlanMessage::SetBypass { node_id, bypassed })).is_err()
                    || self.unsent_plan.is_some()
                {
                    self.graph.mark_dirty();
                }
            }
            EngineCommand::Listen { node_id, listen, channel } => {
                self.graph.set_listen(node_id, listen, channel);
                if self.message_tx.push(AudioMessage::Plan(PlanMessage::Listen { node_id, listen, channel })).is_err()
                    || self.unsent_plan.is_some()
                {
                    self.graph.mark_dirty();
                }
            }
            EngineCommand::Audition { node_id, chord } => {
                // Kept until there's room, in order: a chord let go of mustn't
                // be lost, or it would go on sounding
                self.unsent_handoffs.push(AudioMessage::Plan(PlanMessage::Audition { node_id, chord }));
                self.send_handoffs();
            }
            EngineCommand::SetPlaying(playing) => {
                self.unsent_transport.push(playing);
                self.send_transport();
            }
            other => {
                self.graph.handle_command(other);
            }
        }
    }

    /// Hands a recording's tap to the audio thread, which starts copying its
    /// output into it from the next callback. Never dropped: if the queue is
    /// full it's delivered by a later [`flush`](Self::flush).
    pub fn start_recording(&mut self, tap: RecordTap) {
        self.unsent_handoffs.push(AudioMessage::StartRecording(tap));
        self.send_handoffs();
    }

    /// Asks the audio thread to hand the recording's tap back, ending the
    /// recording once a [`flush`](Self::flush) drops it.
    pub fn stop_recording(&mut self) {
        self.unsent_handoffs.push(AudioMessage::StopRecording);
        self.send_handoffs();
    }

    /// Tells the audio thread how late live input reaches the speakers, in
    /// frames: the round trip measured for the input that's open, or 0
    /// with none. Modules fed by an Audio Input hear it as
    /// [`ProcessContext::input_latency`](crate::dsp::ProcessContext::input_latency).
    pub fn set_input_latency(&self, frames: usize) {
        self.config.input_latency.store(frames, Ordering::Relaxed);
    }

    /// Plays a recording from a module's face, beside the patch (see
    /// [`Cue`](super::cue::Cue)), replacing any cue playing; or with
    /// `None`, stops it. Never dropped: a stop mustn't be lost, or the cue
    /// would play on.
    pub fn cue(&mut self, cue: Option<super::cue::Cue>) {
        self.unsent_handoffs.push(AudioMessage::Cue(cue));
        self.send_handoffs();
    }

    /// Hears what a cable carries, beside the patch (see
    /// [`Probe`](super::probe::Probe)), or with `None`, stops. Never
    /// dropped: a stop mustn't be lost, or the probe would play on.
    pub fn probe(&mut self, probe: Option<super::probe::Probe>) {
        self.unsent_handoffs.push(AudioMessage::Probe(probe));
        self.send_handoffs();
    }

    /// Measures a device's delay (see [`Ping`](crate::link::ping::Ping)),
    /// replacing any measurement running; or with `None`, stops it. Never
    /// dropped: a stop mustn't be lost, or the notes would play on.
    pub fn ping(&mut self, ping: Option<crate::link::ping::Ping>) {
        self.unsent_handoffs.push(AudioMessage::Ping(ping));
        self.send_handoffs();
    }

    /// Hands an audio input's feed to the audio thread, which reads from it
    /// from the next callback, replacing any input it had. Never dropped.
    pub fn connect_input(&mut self, feed: InputFeed) {
        self.unsent_handoffs.push(AudioMessage::ConnectInput(feed));
        self.send_handoffs();
    }

    /// Asks the audio thread to hand its audio input back, leaving the
    /// patch's Audio Input modules silent.
    pub fn disconnect_input(&mut self) {
        self.unsent_handoffs.push(AudioMessage::DisconnectInput);
        self.send_handoffs();
    }

    /// Sends any recording or input changes waiting for room in the queue.
    /// Returns true if none are left waiting.
    fn send_handoffs(&mut self) -> bool {
        while !self.unsent_handoffs.is_empty() {
            let message = self.unsent_handoffs.remove(0);
            if let Err(PushError::Full(message)) = self.message_tx.push(message) {
                self.unsent_handoffs.insert(0, message);
                return false;
            }
        }
        true
    }

    /// Sends any play/stop changes that are waiting for room in the queue,
    /// except a play while graph edits are still to go. Returns true if none
    /// are left waiting.
    fn send_transport(&mut self) -> bool {
        while let Some(playing) = self.unsent_transport.peek() {
            if playing && self.edits_unsent() {
                return false;
            }
            if self.message_tx.push(AudioMessage::SetPlaying(playing)).is_err() {
                return false;
            }
            self.unsent_transport.pop();
        }
        true
    }

    /// Whether graph edits have been made that no sent plan holds yet.
    fn edits_unsent(&self) -> bool {
        self.graph.is_dirty() || self.unsent_plan.is_some()
    }

    /// Delivers pending graph edits to the audio thread as a compiled plan,
    /// and drops plans the audio thread has retired.
    ///
    /// Call once per UI frame, after sending that frame's commands. Returns
    /// true if everything sent so far is on its way. If it returns false, the
    /// audio thread is behind (or not running): call again soon, and the
    /// held-back edits follow as soon as there is room.
    pub fn flush(&mut self) -> bool {
        while let Ok(retired) = self.retired_rx.pop() {
            drop(retired);
            self.plans_in_flight = self.plans_in_flight.saturating_sub(1);
        }
        // Dropping a tap here is what tells its recording to finish
        while let Ok(tap) = self.retired_tap_rx.pop() {
            drop(tap);
        }
        while let Ok(feed) = self.retired_input_rx.pop() {
            drop(feed);
        }
        while let Ok(sample) = self.retired_sample_rx.pop() {
            drop(sample);
        }

        let (sample_rate, block_size) = self.config.load();
        self.graph.set_audio_config(sample_rate, block_size);

        // A stop goes ahead of the edits; a play waits for them, below
        if !self.send_handoffs() || (!self.send_transport() && !self.edits_unsent()) {
            return false;
        }
        if self.unsent_transport.peek() == Some(false) {
            return false;
        }

        loop {
            if self.plans_in_flight >= MAX_PLANS_IN_FLIGHT {
                break;
            }
            let fresh = || self.graph.take_plan().map(|plan| self.monitors.watch(plan));
            let Some(plan) = self.unsent_plan.take().or_else(fresh) else {
                break;
            };
            match self.message_tx.push(AudioMessage::InstallPlan(plan)) {
                Ok(()) => self.plans_in_flight += 1,
                Err(PushError::Full(AudioMessage::InstallPlan(plan))) => {
                    self.unsent_plan = Some(plan);
                    break;
                }
                Err(PushError::Full(_)) => unreachable!("pushed a plan"),
            }
        }

        // A recording for a running module goes after the plan that has
        // the module, or the audio thread wouldn't find it
        if self.unsent_plan.is_some() {
            return false;
        }
        let loads = self.graph.take_sample_loads();
        let loads = loads.into_iter().map(|(node_id, sample)| PlanMessage::LoadSample { node_id, sample });
        self.unsent_samples.extend(loads.chain(self.graph.take_take_handoffs()).map(AudioMessage::Plan));
        if !self.send_samples() {
            return false;
        }

        // A play held back for the edits follows them, and the recordings
        self.send_transport() && !self.graph.is_dirty()
    }

    /// Asks the audio thread for a copy of a module's recording (a Looper's
    /// loop), into `snapshot`, made with room enough off the audio thread.
    /// It comes back through [`take_snapshot`](Self::take_snapshot), filled
    /// or saying why not, after the next [`flush`](Self::flush)es deliver
    /// it. Returns false, and keeps nothing, if too many are already out.
    pub fn request_snapshot(&mut self, node_id: NodeId, snapshot: Box<Snapshot>) -> bool {
        if self.snapshots_out >= MAX_SNAPSHOTS {
            return false;
        }
        self.snapshots_out += 1;
        self.unsent_samples.push(AudioMessage::SnapshotLoop { node_id, snapshot });
        true
    }

    /// A snapshot the audio thread has finished, if one is back.
    pub fn take_snapshot(&mut self) -> Option<(NodeId, Box<Snapshot>)> {
        let back = self.snapshot_rx.pop().ok()?;
        self.snapshots_out = self.snapshots_out.saturating_sub(1);
        Some(back)
    }

    /// Snapshots asked for and not yet taken back.
    pub fn snapshots_out(&self) -> usize {
        self.snapshots_out
    }

    /// A take a module (a Sampler, a Chopper) has just recorded, if one is
    /// waiting. The module plays it already: this is shared, for the UI to
    /// keep.
    pub fn take_finished_take(&mut self) -> Option<(NodeId, FinishedTake)> {
        self.take_rx.pop().ok()
    }

    /// Sends any recordings waiting for room in the queue. Returns true if
    /// none are left waiting.
    fn send_samples(&mut self) -> bool {
        while !self.unsent_samples.is_empty() {
            let message = self.unsent_samples.remove(0);
            if let Err(PushError::Full(message)) = self.message_tx.push(message) {
                self.unsent_samples.insert(0, message);
                return false;
            }
        }
        true
    }

    /// The UI-side graph: the patch as the engine will play it.
    pub fn graph(&self) -> &AudioGraph {
        &self.graph
    }

    /// Notes that a module already holds `sample`, without sending it: a
    /// Looper whose loop was just saved as that file. See
    /// [`AudioGraph::note_sample`].
    pub fn note_sample(&mut self, node_id: NodeId, sample: Option<Arc<SampleData>>) {
        self.graph.note_sample(node_id, sample);
    }

    /// Receive an event from the audio engine.
    /// Returns Some(event) if available, None if no events pending.
    ///
    /// Monitored values, meters and readouts come last, the newest of each
    /// once per drain (see [`Monitors::next`]), however many callbacks ran.
    ///
    /// This is a non-blocking operation.
    pub fn recv_event(&mut self) -> Option<EngineEvent> {
        self.event_rx
            .pop()
            .ok()
            .or_else(|| self.scope_rx.pop().ok().map(ScopeFrame::into_event))
            .or_else(|| self.monitors.next())
    }

    /// Drain all pending events from the engine.
    /// Returns an iterator over all available events.
    pub fn drain_events(&mut self) -> impl Iterator<Item = EngineEvent> + '_ {
        std::iter::from_fn(|| self.recv_event())
    }

    /// Check how many messages can still be queued.
    pub fn command_slots_available(&self) -> usize {
        self.message_tx.slots()
    }

    /// Check if the message buffer is full.
    pub fn is_command_buffer_full(&self) -> bool {
        self.message_tx.is_full()
    }
}

/// Engine-side handle for communicating with the UI.
///
/// IMPORTANT: All methods are designed to be real-time safe (non-blocking, no allocations).
pub struct EngineHandle {
    message_rx: Consumer<AudioMessage>,
    retired_tx: Producer<Box<GraphPlan>>,
    retired_tap_tx: Producer<RecordTap>,
    retired_input_tx: Producer<InputFeed>,
    retired_sample_tx: Producer<Arc<SampleData>>,
    snapshot_tx: Producer<(NodeId, Box<Snapshot>)>,
    take_tx: Producer<(NodeId, FinishedTake)>,
    event_tx: Producer<EngineEvent>,
    scope_tx: Producer<ScopeFrame>,
    config: Arc<AudioConfig>,
}

impl EngineHandle {
    /// Receive a message from the UI.
    /// Returns Some(message) if available, None if none are pending.
    ///
    /// REAL-TIME SAFE: Non-blocking operation.
    pub fn recv_message(&mut self) -> Option<AudioMessage> {
        self.message_rx.pop().ok()
    }

    /// Hands a replaced plan back to the UI thread to be dropped there.
    ///
    /// REAL-TIME SAFE: The UI never has more than [`MAX_PLANS_IN_FLIGHT`]
    /// plans out, so there is always room. Should that ever fail, the plan is
    /// dropped here rather than lost.
    pub fn retire_plan(&mut self, plan: Box<GraphPlan>) {
        if let Err(PushError::Full(plan)) = self.retired_tx.push(plan) {
            debug_assert!(false, "retired plan queue full");
            drop(plan);
        }
    }

    /// Hands a recording tap back to the UI thread, so its ring is freed
    /// there and its recording finishes.
    ///
    /// REAL-TIME SAFE: the UI has at most one tap out at a time. Should the
    /// queue ever be full, the tap is dropped here rather than lost.
    pub fn retire_tap(&mut self, tap: RecordTap) {
        if let Err(PushError::Full(tap)) = self.retired_tap_tx.push(tap) {
            debug_assert!(false, "retired tap queue full");
            drop(tap);
        }
    }

    /// Hands an audio input feed back to the UI thread, so its ring is freed
    /// there.
    ///
    /// REAL-TIME SAFE: should the queue ever be full, the feed is dropped
    /// here rather than lost.
    pub fn retire_input(&mut self, feed: InputFeed) {
        if let Err(PushError::Full(feed)) = self.retired_input_tx.push(feed) {
            debug_assert!(false, "retired input queue full");
            drop(feed);
        }
    }

    /// Hands a recording a module has let go of back to the UI thread, so
    /// its memory is freed there.
    ///
    /// REAL-TIME SAFE: should the queue ever be full, the recording is
    /// dropped here rather than lost.
    pub fn retire_sample(&mut self, sample: Arc<SampleData>) {
        if let Err(PushError::Full(sample)) = self.retired_sample_tx.push(sample) {
            debug_assert!(false, "retired sample queue full");
            drop(sample);
        }
    }

    /// Hands a finished snapshot back to the UI.
    ///
    /// REAL-TIME SAFE: the UI has at most [`MAX_SNAPSHOTS`] out, so there
    /// is always room. Should that ever fail, it's dropped here rather
    /// than lost.
    pub fn return_snapshot(&mut self, node_id: NodeId, snapshot: Box<Snapshot>) {
        if let Err(PushError::Full(back)) = self.snapshot_tx.push((node_id, snapshot)) {
            debug_assert!(false, "snapshot queue full");
            drop(back);
        }
    }

    /// Tells the UI of a take a module has just recorded.
    ///
    /// REAL-TIME SAFE: should the queue ever be full, the UI never hears
    /// of it, and the module plays it unkept; it's handed back to be
    /// dropped, rather than freed here.
    pub fn send_take(&mut self, node_id: NodeId, take: FinishedTake) {
        if let Err(PushError::Full((_, take))) = self.take_tx.push((node_id, take)) {
            debug_assert!(false, "take queue full");
            self.retire_sample(take.sample);
        }
    }

    /// How late live input reaches the speakers, in frames, as the UI last
    /// measured it.
    ///
    /// REAL-TIME SAFE: an atomic load.
    pub fn input_latency(&self) -> usize {
        self.config.input_latency.load(Ordering::Relaxed)
    }

    /// Records the settings the audio thread runs at, so the UI prepares new
    /// modules for them.
    pub fn set_audio_config(&self, sample_rate: f32, block_size: usize) {
        self.config.store(sample_rate, block_size);
    }

    /// Send an event to the UI.
    /// Returns Ok(()) if the event was queued, or Err(event) if the buffer is full.
    ///
    /// REAL-TIME SAFE: Non-blocking operation.
    // The rejected event comes back by value: boxing it would allocate on the
    // audio thread
    #[allow(clippy::result_large_err)]
    pub fn send_event(&mut self, event: EngineEvent) -> Result<(), EngineEvent> {
        self.event_tx
            .push(event)
            .map_err(|rtrb::PushError::Full(event)| event)
    }

    /// Try to send an event, dropping it silently if the buffer is full.
    /// Use this for metering data where dropping old values is acceptable.
    ///
    /// REAL-TIME SAFE: Non-blocking, no allocations.
    pub fn send_event_lossy(&mut self, event: EngineEvent) {
        let _ = self.event_tx.push(event);
    }

    /// Send an oscilloscope capture, dropping it if the UI is behind.
    ///
    /// REAL-TIME SAFE: Non-blocking, no allocations.
    pub fn send_scope_frame_lossy(&mut self, frame: ScopeFrame) {
        let _ = self.scope_tx.push(frame);
    }

    /// Check how many events can still be queued.
    pub fn event_slots_available(&self) -> usize {
        self.event_tx.slots()
    }

    /// Check how many messages are pending.
    pub fn messages_pending(&self) -> usize {
        self.message_rx.slots()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::OutputLevels;
    use crate::engine::CpuLoad;

    fn add(node_id: u64, module_id: &'static str) -> EngineCommand {
        EngineCommand::AddModule { node_id, module_id }
    }

    /// Plays the audio thread's part: installs each plan, retiring the old one.
    fn run_audio_side(engine: &mut EngineHandle, current: &mut Box<GraphPlan>) -> Vec<&'static str> {
        let mut seen = Vec::new();
        while let Some(message) = engine.recv_message() {
            match message {
                AudioMessage::InstallPlan(mut plan) => {
                    plan.take_over(current);
                    let old = std::mem::replace(current, plan);
                    engine.retire_plan(old);
                    seen.push("plan");
                }
                AudioMessage::Plan(message) => {
                    seen.push(match &message {
                        PlanMessage::SetParameter { .. } => "param",
                        PlanMessage::SetBypass { .. } => "bypass",
                        PlanMessage::Listen { .. } => "listen",
                        PlanMessage::Audition { chord, .. } => if chord.is_some() { "audition" } else { "audition off" },
                        PlanMessage::LoadSample { .. } => "sample",
                        PlanMessage::SetMarkers { .. } => "markers",
                        PlanMessage::MapSlot { .. } => "slot",
                        PlanMessage::SlotHeard { .. } => "heard",
                        PlanMessage::MapButton { .. } => "button",
                        PlanMessage::MapLane { .. } => "lane",
                        PlanMessage::MapRow { .. } => "row",
                        PlanMessage::SetFmVoice { .. } => "fm voice",
                        PlanMessage::GiveRoom { .. } => "room",
                        PlanMessage::SettleTake { .. } => "settle",
                    });
                    current.apply(message, |sample| engine.retire_sample(sample));
                }
                AudioMessage::SetPlaying(playing) => seen.push(if playing { "play" } else { "stop" }),
                AudioMessage::StartRecording(tap) => {
                    engine.retire_tap(tap);
                    seen.push("record");
                }
                AudioMessage::StopRecording => seen.push("stop recording"),
                AudioMessage::ConnectInput(feed) => {
                    engine.retire_input(feed);
                    seen.push("input");
                }
                AudioMessage::DisconnectInput => seen.push("no input"),
                AudioMessage::Cue(cue) => {
                    if let Some(cue) = cue {
                        engine.retire_sample(cue.sample);
                    }
                    seen.push("cue");
                }
                AudioMessage::Probe(_) => seen.push("probe"),
                AudioMessage::Ping(_) => seen.push("ping"),
                AudioMessage::SnapshotLoop { node_id, mut snapshot } => {
                    let mut budget = usize::MAX;
                    current.fill_snapshot(node_id, &mut snapshot, &mut budget);
                    engine.return_snapshot(node_id, snapshot);
                    seen.push("snapshot");
                }
            }
        }
        seen
    }

    #[test]
    fn test_default_channels() {
        let (ui, engine) = EngineChannels::with_defaults().split();
        assert_eq!(ui.command_slots_available(), DEFAULT_COMMAND_BUFFER_SIZE);
        assert_eq!(engine.event_slots_available(), DEFAULT_EVENT_BUFFER_SIZE);
    }

    #[test]
    fn test_graph_edits_are_batched_into_one_plan() {
        let (mut ui, mut engine) = EngineChannels::with_defaults().split();
        let mut plan = Box::new(GraphPlan::empty(256));

        ui.send_command(add(1, "osc.sine"));
        ui.send_command(add(2, "output.audio"));
        ui.send_command(EngineCommand::Connect { from_node: 1, from_port: 5, to_node: 2, to_port: 2 });
        assert_eq!(engine.messages_pending(), 0, "nothing sent before flush");

        assert!(ui.flush());
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["plan"]);
        assert_eq!(plan.len(), 2);

        // Nothing changed: nothing more to send
        assert!(ui.flush());
        assert_eq!(engine.messages_pending(), 0);
    }

    #[test]
    fn test_parameters_go_straight_through() {
        let (mut ui, mut engine) = EngineChannels::with_defaults().split();
        let mut plan = Box::new(GraphPlan::empty(256));
        ui.send_command(add(1, "osc.sine"));
        ui.flush();
        run_audio_side(&mut engine, &mut plan);

        ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 0, value: 220.0 });
        ui.send_command(EngineCommand::SetPlaying(true));
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["param", "play"]);
        assert_eq!(plan.nodes[0].params[0], 220.0);
        assert_eq!(ui.graph().parameters(1).unwrap()[0], 220.0);
    }

    #[test]
    fn test_parameter_for_new_node_arrives_with_its_plan() {
        let (mut ui, mut engine) = EngineChannels::with_defaults().split();
        let mut plan = Box::new(GraphPlan::empty(256));

        // The parameter message overtakes the (unflushed) plan; the plan
        // still carries the value
        ui.send_command(add(1, "osc.sine"));
        ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 0, value: 330.0 });
        ui.flush();
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["param", "plan"]);
        assert_eq!(plan.nodes[0].params[0], 330.0);
    }

    #[test]
    fn test_retired_plans_return_to_ui() {
        let (mut ui, mut engine) = EngineChannels::with_defaults().split();
        let mut plan = Box::new(GraphPlan::empty(256));

        // Many edits, each flushed and installed, never exhaust the plan budget
        for id in 0..(MAX_PLANS_IN_FLIGHT as u64 * 3) {
            ui.send_command(add(id, "osc.sine"));
            assert!(ui.flush(), "plan {id} was held back");
            run_audio_side(&mut engine, &mut plan);
        }
        assert_eq!(plan.len(), MAX_PLANS_IN_FLIGHT * 3);
        assert!(plan.nodes.iter().all(|n| n.module.is_some()));
    }

    #[test]
    fn test_plans_wait_while_audio_thread_is_away() {
        let (mut ui, mut engine) = EngineChannels::with_defaults().split();
        let mut plan = Box::new(GraphPlan::empty(256));

        // With no audio callbacks, at most MAX_PLANS_IN_FLIGHT plans go out
        // (one of which is the processor's initial plan)
        for id in 0..10 {
            ui.send_command(add(id, "osc.sine"));
            ui.flush();
        }
        assert_eq!(engine.messages_pending(), MAX_PLANS_IN_FLIGHT - 1);

        // Once the audio thread catches up, the held-back edits follow
        run_audio_side(&mut engine, &mut plan);
        assert!(ui.flush());
        run_audio_side(&mut engine, &mut plan);
        assert_eq!(plan.len(), 10);
        assert!(plan.nodes.iter().all(|n| n.module.is_some()));
    }

    #[test]
    fn test_full_queue_holds_plan_until_space() {
        let (mut ui, mut engine) = EngineChannels::new(2, 8).split();
        let mut plan = Box::new(GraphPlan::empty(256));

        ui.send_command(EngineCommand::SetPlaying(true));
        ui.send_command(EngineCommand::SetPlaying(false));
        assert!(ui.is_command_buffer_full());
        // Held, not dropped
        ui.send_command(EngineCommand::SetPlaying(true));

        ui.send_command(add(1, "osc.sine"));
        assert!(!ui.flush(), "no room for the plan yet");

        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["play", "stop"]);
        assert!(ui.flush());
        // The held play waits for the plan made before it
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["plan", "play"]);
        assert_eq!(plan.len(), 1);
    }

    /// Leaves a plan held back for want of room, then lets the queue drain
    /// so that a message sent now gets in ahead of it.
    fn hold_back_a_plan(ui: &mut UiHandle, engine: &mut EngineHandle, plan: &mut Box<GraphPlan>, node_id: NodeId) {
        ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 0, value: 100.0 });
        ui.send_command(add(node_id, "osc.sine"));
        assert!(!ui.flush(), "no room for the plan yet");
        assert_eq!(run_audio_side(engine, plan), vec!["param"]);
    }

    fn flush_everything(ui: &mut UiHandle, engine: &mut EngineHandle, plan: &mut Box<GraphPlan>) {
        let mut frames = 0;
        while !ui.flush() {
            run_audio_side(engine, plan);
            frames += 1;
            assert!(frames < 10, "edits still held back after {frames} frames");
        }
        run_audio_side(engine, plan);
    }

    #[test]
    fn test_held_back_plan_doesnt_undo_a_later_knob_or_bypass() {
        let (mut ui, mut engine) = EngineChannels::new(1, 8).split();
        let mut plan = Box::new(GraphPlan::empty(256));
        // A delay, since an oscillator can't be bypassed
        ui.send_command(add(1, "fx.delay"));
        assert!(ui.flush());
        run_audio_side(&mut engine, &mut plan);

        // The held-back plan still has 100
        hold_back_a_plan(&mut ui, &mut engine, &mut plan, 2);
        ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 0, value: 200.0 });
        flush_everything(&mut ui, &mut engine, &mut plan);
        assert_eq!(plan.len(), 2);
        let delay = plan.nodes.iter().find(|n| n.node_id == 1).unwrap();
        assert_eq!(delay.params[0], 200.0, "the held-back plan undid the knob");

        // The held-back plan still has the delay playing
        hold_back_a_plan(&mut ui, &mut engine, &mut plan, 3);
        ui.send_command(EngineCommand::SetBypass { node_id: 1, bypassed: true });
        flush_everything(&mut ui, &mut engine, &mut plan);
        assert_eq!(plan.len(), 3);
        assert_eq!(plan.is_bypassed(1), Some(true), "the held-back plan undid the bypass");
    }

    #[test]
    fn test_held_transport_collapses_but_keeps_the_stop() {
        let (mut ui, mut engine) = EngineChannels::new(1, 8).split();
        let mut plan = Box::new(GraphPlan::empty(256));

        ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 0, value: 1.0 });
        assert!(ui.is_command_buffer_full());

        // A burst of toggles while the queue is full: the stop still has to
        // reach the audio thread, since stopping clears tails
        for playing in [false, true, false, true] {
            ui.send_command(EngineCommand::SetPlaying(playing));
        }
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.extend(run_audio_side(&mut engine, &mut plan));
            ui.flush();
        }
        assert_eq!(seen, vec!["param", "stop", "play"]);

        // Ending on a stop needs only the stop
        ui.send_command(EngineCommand::SetParameter { node_id: 1, param_index: 0, value: 2.0 });
        for playing in [true, false, true, false] {
            ui.send_command(EngineCommand::SetPlaying(playing));
        }
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.extend(run_audio_side(&mut engine, &mut plan));
            ui.flush();
        }
        assert_eq!(seen, vec!["param", "stop"]);
    }

    #[test]
    fn test_play_waits_for_the_patch_opened_before_it() {
        let (mut ui, mut engine) = EngineChannels::new(64, 8).split();
        let mut plan = Box::new(GraphPlan::empty(256));
        ui.send_command(add(1, "osc.sine"));
        ui.send_command(EngineCommand::SetPlaying(true));
        assert!(ui.flush());
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["plan", "play"]);

        // Another patch opened while playing: stop, swap the graph, set its
        // knobs, play on. The audio thread mustn't play before the new patch
        // is there, or it plays the old one, or the new one's defaults
        ui.send_command(EngineCommand::SetPlaying(false));
        ui.send_command(EngineCommand::ClearGraph);
        ui.send_command(add(2, "osc.sine"));
        ui.send_command(EngineCommand::SetParameter { node_id: 2, param_index: 0, value: 1234.0 });
        ui.send_command(EngineCommand::SetPlaying(true));
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["stop", "param"], "the play waits");

        assert!(ui.flush());
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["plan", "play"]);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.nodes[0].params[0], 1234.0, "the knob came with the patch");
    }

    #[test]
    fn test_large_patch_load_never_loses_nodes() {
        // A tiny queue, an audio thread that only wakes every few frames, and
        // a stream of knob moves competing for room with the patch
        let (mut ui, mut engine) = EngineChannels::new(4, 8).split();
        let mut plan = Box::new(GraphPlan::empty(256));
        const NODES: u64 = 50;

        ui.send_command(EngineCommand::ClearGraph);
        for id in 1..=NODES {
            let module_id = if id % 2 == 0 { "osc.sine" } else { "fx.delay" };
            ui.send_command(add(id, module_id));
            if id > 1 {
                // Oscillator "Out" is port 5, delay "Out L" port 4
                let from_port = if (id - 1) % 2 == 0 { 5 } else { 4 };
                ui.send_command(EngineCommand::Connect { from_node: id - 1, from_port, to_node: id, to_port: 0 });
            }
            ui.send_command(EngineCommand::SetParameter { node_id: id, param_index: 0, value: id as f32 });
            ui.send_command(EngineCommand::SetPlaying(true));
            if id % 10 == 0 {
                ui.flush();
            }
            if id % 15 == 0 {
                run_audio_side(&mut engine, &mut plan);
            }
        }

        // Keep the UI's frames going until it has sent everything
        let mut frames = 0;
        while !ui.flush() {
            run_audio_side(&mut engine, &mut plan);
            frames += 1;
            assert!(frames < 100, "edits still held back after {frames} frames");
        }
        run_audio_side(&mut engine, &mut plan);

        assert_eq!(plan.len(), NODES as usize, "nodes were lost");
        assert!(plan.nodes.iter().all(|n| n.module.is_some()));
        for node in &plan.nodes {
            assert_eq!(node.params[0], node.node_id as f32, "node {} lost its parameter", node.node_id);
        }
        assert_eq!(plan.processing_order().collect::<Vec<_>>(), (1..=NODES).collect::<Vec<_>>());
    }

    #[test]
    fn test_event_send_receive() {
        let (mut ui, mut engine) = EngineChannels::new(64, 64).split();

        // Send event from engine
        let result = engine.send_event(EngineEvent::OutputLevel(OutputLevels {
            post: [0.5, 0.6],
            ..Default::default()
        }));
        assert!(result.is_ok());

        // Receive in UI
        let event = ui.recv_event();
        assert!(event.is_some());
        if let EngineEvent::OutputLevel(levels) = event.unwrap() {
            assert_eq!(levels.post, [0.5, 0.6]);
        } else {
            panic!("Wrong event type");
        }
    }

    #[test]
    fn test_scope_frames_arrive_as_events() {
        let (mut ui, mut engine) = EngineChannels::new(64, 64).split();
        engine.send_scope_frame_lossy(ScopeFrame::new(7, &[0.1, 0.2, 0.3], &[], true));

        match ui.recv_event() {
            Some(EngineEvent::ScopeBuffer { node_id, channel1, channel2, triggered }) => {
                assert_eq!(node_id, 7);
                assert_eq!(&*channel1, &[0.1, 0.2, 0.3]);
                assert!(channel2.is_empty());
                assert!(triggered);
            }
            other => panic!("expected a scope buffer, got {other:?}"),
        }
    }

    #[test]
    fn test_lossy_events() {
        let (mut ui, mut engine) = EngineChannels::new(1, 1).split();

        engine.send_event_lossy(EngineEvent::CpuLoad(CpuLoad { average: 0.5, peak: 0.5 }));
        engine.send_event_lossy(EngineEvent::CpuLoad(CpuLoad { average: 0.6, peak: 0.6 })); // Should be dropped

        assert!(ui.recv_event().is_some());
        assert!(ui.recv_event().is_none());
    }

    #[test]
    fn test_drain_events() {
        let (mut ui, mut engine) = EngineChannels::new(64, 64).split();

        // Send multiple events
        engine.send_event_lossy(EngineEvent::Started);
        engine.send_event_lossy(EngineEvent::CpuLoad(CpuLoad { average: 0.3, peak: 0.3 }));
        engine.send_event_lossy(EngineEvent::OutputLevel(OutputLevels::default()));

        // Drain all events
        let events: Vec<_> = ui.drain_events().collect();
        assert_eq!(events.len(), 3);

        // No more events
        assert!(ui.recv_event().is_none());
    }

    #[test]
    fn test_slots_available() {
        let (mut ui, mut engine) = EngineChannels::new(10, 10).split();

        assert_eq!(ui.command_slots_available(), 10);
        assert_eq!(engine.event_slots_available(), 10);

        ui.send_command(EngineCommand::SetPlaying(true));
        engine.send_event_lossy(EngineEvent::Started);

        assert_eq!(ui.command_slots_available(), 9);
        assert_eq!(engine.event_slots_available(), 9);
    }

    #[test]
    fn test_handles_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<UiHandle>();
        assert_send::<EngineHandle>();
    }

    #[test]
    fn test_audio_config_reaches_ui_graph() {
        let (mut ui, engine) = EngineChannels::with_defaults().split();
        engine.set_audio_config(48000.0, 128);
        ui.flush();
        assert_eq!(ui.graph().sample_rate(), 48000.0);
        assert_eq!(ui.graph().block_size(), 128);
    }

    fn recording(value: f32) -> Arc<SampleData> {
        Arc::new(SampleData::mono(vec![value; 4800], 48000.0))
    }

    /// The recording a module in `plan` holds, taken out and put back.
    fn sample_in(plan: &mut GraphPlan, node_id: u64) -> Option<Arc<SampleData>> {
        let held = plan.load_sample(node_id, None);
        assert!(plan.load_sample(node_id, held.clone()).is_none());
        held
    }

    #[test]
    fn test_a_loaded_sample_survives_plan_swaps() {
        let (mut ui, mut engine) = EngineChannels::with_defaults().split();
        let mut plan = Box::new(GraphPlan::empty(256));
        let first = recording(0.5);

        // Loaded before the module ever reached the audio thread: it goes with it
        ui.send_command(add(1, "source.sampler"));
        ui.send_command(EngineCommand::LoadSample { node_id: 1, sample: Some(first.clone()) });
        ui.flush();
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["plan"]);
        assert!(Arc::ptr_eq(&sample_in(&mut plan, 1).unwrap(), &first));

        // Other modules come and go around it
        ui.send_command(add(2, "osc.sine"));
        ui.flush();
        run_audio_side(&mut engine, &mut plan);
        ui.send_command(EngineCommand::RemoveModule { node_id: 2 });
        ui.flush();
        run_audio_side(&mut engine, &mut plan);
        assert!(Arc::ptr_eq(&sample_in(&mut plan, 1).unwrap(), &first));
        assert!(Arc::ptr_eq(ui.graph().sample(1).unwrap(), &first));
    }

    #[test]
    fn test_a_new_sample_follows_the_plan_and_the_old_one_comes_back() {
        let (mut ui, mut engine) = EngineChannels::with_defaults().split();
        let mut plan = Box::new(GraphPlan::empty(256));
        let first = recording(0.5);
        ui.send_command(add(1, "source.sampler"));
        ui.send_command(EngineCommand::LoadSample { node_id: 1, sample: Some(first.clone()) });
        ui.flush();
        run_audio_side(&mut engine, &mut plan);

        // A running module's new recording goes after the plan made the same frame
        let second = recording(-0.5);
        ui.send_command(add(2, "osc.sine"));
        ui.send_command(EngineCommand::LoadSample { node_id: 1, sample: Some(second.clone()) });
        assert_eq!(engine.messages_pending(), 0, "held until flush");
        ui.flush();
        assert_eq!(run_audio_side(&mut engine, &mut plan), vec!["plan", "sample"]);
        assert!(Arc::ptr_eq(&sample_in(&mut plan, 1).unwrap(), &second));

        // The first is dropped on the UI side, not the audio side
        assert_eq!(Arc::strong_count(&first), 2, "here and on its way back");
        ui.flush();
        assert_eq!(Arc::strong_count(&first), 1);
    }

    #[test]
    fn test_a_sample_waits_for_a_plan_that_did_not_fit() {
        // Room for one message: the plan takes it, the recording waits
        let (mut ui, mut engine) = EngineChannels::new(1, 16).split();
        let mut plan = Box::new(GraphPlan::empty(256));
        ui.send_command(add(1, "source.sampler"));
        ui.flush();
        run_audio_side(&mut engine, &mut plan);

        ui.send_command(add(2, "osc.sine"));
        ui.flush();
        ui.send_command(add(3, "osc.sine"));
        ui.send_command(EngineCommand::LoadSample { node_id: 1, sample: Some(recording(0.5)) });
        assert!(!ui.flush(), "the queue is full");
        let mut seen = run_audio_side(&mut engine, &mut plan);
        while !ui.flush() {
            seen.extend(run_audio_side(&mut engine, &mut plan));
        }
        seen.extend(run_audio_side(&mut engine, &mut plan));
        assert_eq!(seen, vec!["plan", "plan", "sample"]);
        assert!(sample_in(&mut plan, 1).is_some());
    }

    #[test]
    fn test_empty_receive() {
        let (mut ui, mut engine) = EngineChannels::new(64, 64).split();

        // Nothing sent yet
        assert!(engine.recv_message().is_none());
        assert!(ui.recv_event().is_none());
    }
}
