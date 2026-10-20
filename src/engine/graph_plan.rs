//! Compiled, real-time-ready form of the audio graph.
//!
//! A [`GraphPlan`] is built off the audio thread by
//! [`AudioGraph::compile`](super::AudioGraph::compile). Everything the audio
//! callback needs is resolved up front: the processing order, which buffer
//! feeds each input, every signal buffer at full block capacity, and the
//! monitor taps. Running a block is then plain slice indexing, with no
//! allocation, hashing or searching.
//!
//! Modules already running in the previous plan are not rebuilt. A node that
//! existed before is compiled without a module, and [`GraphPlan::take_over`]
//! moves the live module (with its phase, envelope and delay-line state) from
//! the outgoing plan into the new one. That is a pointer move, so swapping
//! plans on the audio thread is real-time safe. The outgoing plan, holding
//! any removed modules, is handed back to be dropped on another thread.
//!
//! A bypassed filter or effect passes its audio input straight to its output.
//! Switching crossfades over [`BYPASS_FADE_SECONDS`] so nothing clicks, and
//! once fully bypassed the module isn't run at all.
//!
//! Buffers carry up to [`MAX_CHANNELS`](crate::dsp::MAX_CHANNELS) channels.
//! Polyphonic modules set how many each block, and the count flows
//! downstream with the signal. A module that isn't polyphonic hears a
//! polyphonic cable on an audio input as all its channels summed.
//!
//! A cable into a late input that closes a loop (a mixer's effect return,
//! fed from the same mixer's send) is heard through a [`LateLine`]: one
//! block behind, so its source can run after the module it feeds.

use std::ops::Range;
use std::sync::Arc;

use web_time::Instant;

use crate::dsp::{Audition, DspModule, FinishedTake, OutputLevels, ProcessContext, SampleData, SignalBuffer, Snapshot, SnapshotOutcome, TransportState};
use crate::dsp::finite::all_finite;
use crate::dsp::primitives::Markers;
use crate::engine::commands::{ChannelPeaks, NodeId, PlanMessage, PortIndex};
use crate::engine::monitor_board::MonitorWriter;
use crate::engine::spectrum_tap::SpectrumTap;
use crate::modules::scenes::SceneKnob;

pub use crate::dsp::module_trait::MAX_INPUTS;

/// How long bypassing a module, or bringing it back, crossfades for.
pub const BYPASS_FADE_SECONDS: f32 = 0.02;

/// Filler for unused entries of the per-module input array.
static EMPTY_BUFFER: SignalBuffer = SignalBuffer::EMPTY;

/// Where a module input reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputSource {
    /// An upstream module's output buffer (index into `GraphPlan::outputs`).
    Output(usize),
    /// An unpatched input's stand-in buffer (index into `GraphPlan::defaults`).
    Default(usize),
    /// A polyphonic upstream output (index into `GraphPlan::outputs`) heard
    /// by a mono audio input as the sum of its channels, written into a
    /// buffer of its own (index into `GraphPlan::mixdowns`).
    Mixdown { source: usize, mix: usize },
    /// A cable closing a loop into a late input, heard one block behind
    /// (index into `GraphPlan::late`).
    Late(usize),
}

/// What a monitored input reports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MonitorSource {
    /// The first sample of an upstream output buffer.
    Output(usize),
    /// A fixed value: the port's default, as nothing is patched in.
    Constant(f32),
}

/// One module in the plan.
pub(crate) struct PlanNode {
    pub(crate) node_id: NodeId,
    /// `None` until a freshly created module is supplied, or until
    /// [`GraphPlan::take_over`] moves the running module in.
    pub(crate) module: Option<Box<dyn DspModule>>,
    pub(crate) params: Vec<f32>,
    /// One entry per input port, in port order.
    pub(crate) inputs: Vec<InputSource>,
    /// This node's output buffers within `GraphPlan::outputs`.
    pub(crate) outputs: Range<usize>,
    /// Which of them anything hears, a bit per output (see
    /// [`ProcessContext::heard_outputs`]).
    pub(crate) heard: u64,
    /// Whether the module is bypassed: where the crossfade is heading.
    pub(crate) bypassed: bool,
    /// How much of the module's own output is heard: 1 in the signal path,
    /// 0 fully bypassed, in between while crossfading.
    pub(crate) wet: f32,
    /// For each output, the input (index into `inputs`) it passes while
    /// bypassed. Empty for modules that can't be bypassed.
    pub(crate) dry: Vec<Option<usize>>,
    /// Whether live input reaches the module, through any chain of modules,
    /// so it's told how late (see [`ProcessContext::input_latency`]).
    pub(crate) hears_live: bool,
    /// Where its display's spectra are kept, for a filter.
    pub(crate) spectrum: Option<SpectrumWatch>,
    /// Whether it made a NaN or infinity since [`GraphPlan::take_faults`]
    /// last asked, and was reset for it.
    pub(crate) faulted: bool,
}

#[cfg(test)]
impl PlanNode {
    /// A node with no module, owning `outputs`: enough to find its buffers.
    pub(crate) fn bare(node_id: NodeId, outputs: Range<usize>) -> Self {
        Self {
            node_id,
            module: None,
            params: Vec::new(),
            inputs: Vec::new(),
            outputs,
            heard: u64::MAX,
            bypassed: false,
            wet: 1.0,
            dry: Vec::new(),
            hears_live: false,
            spectrum: None,
            faulted: false,
        }
    }
}

/// A node whose input and output are copied to its display's spectrum tap
/// after every block.
pub(crate) struct SpectrumWatch {
    /// The input heard (index into the node's `inputs`).
    pub(crate) input: usize,
    /// The output heard, counted among the node's outputs.
    pub(crate) output: usize,
    pub(crate) tap: Arc<SpectrumTap>,
}

/// A cable that closes a loop, heard a constant one block behind.
///
/// Its source runs after the module it feeds, so the module hears what the
/// source played a block ago. The delay is the plan's largest block rather
/// than the last block's length, so it holds steady when blocks vary.
pub(crate) struct LateLine {
    /// The output buffer it hears (index into `GraphPlan::outputs`), once
    /// the plan has laid it out. Silent until then.
    pub(crate) source: Option<usize>,
    /// The source's recent samples, its channels summed, oldest overwritten.
    ring: Vec<f32>,
    /// Where in `ring` the next block is written.
    write: usize,
    /// What the input hears this block.
    heard: SignalBuffer,
}

impl LateLine {
    pub(crate) fn new(max_block_size: usize, signal_type: crate::dsp::SignalType) -> Self {
        Self { source: None, ring: vec![0.0; 2 * max_block_size.max(1)], write: 0, heard: SignalBuffer::new(max_block_size, signal_type) }
    }

    /// Fills `heard` with the samples from one largest block ago.
    fn read(&mut self, max_block_size: usize) {
        let len = self.ring.len();
        let start = self.write + len - max_block_size;
        for (i, sample) in self.heard.samples.iter_mut().enumerate() {
            *sample = self.ring[(start + i) % len];
        }
    }

    /// Records the block `source` just played.
    fn write(&mut self, source: &SignalBuffer) {
        let len = self.ring.len();
        for i in 0..self.heard.samples.len() {
            let sum: f32 = (0..source.channels()).map(|c| source.voice(c).samples[i]).sum();
            self.ring[(self.write + i) % len] = sum;
        }
        self.write = (self.write + self.heard.samples.len()) % len;
    }
}

/// A monitored input port, reported to the UI for knob animation.
pub(crate) struct InputTap {
    pub(crate) node_id: NodeId,
    pub(crate) input_index: PortIndex,
    pub(crate) source: MonitorSource,
}

/// A monitored output port, reported to the UI for LEDs and cable animation.
pub(crate) struct OutputTap {
    pub(crate) node_id: NodeId,
    pub(crate) output_index: PortIndex,
    pub(crate) buffer: usize,
    /// The loudest of every block since the UI was last given a reading, so
    /// a hit early in a callback of several blocks isn't lost to the last.
    pub(crate) peaks: Option<ChannelPeaks>,
}

/// A Scenes module's knobs, which the plan turns as its Morph moves.
pub(crate) struct PlanScenes {
    /// The Scenes module (index into `GraphPlan::nodes`).
    pub(crate) node: usize,
    pub(crate) knobs: Vec<PlanKnob>,
}

/// A knob a scene holds, found in the plan.
pub(crate) struct PlanKnob {
    /// Its module (index into `GraphPlan::nodes`).
    pub(crate) node: usize,
    pub(crate) knob: SceneKnob,
}

/// A compiled audio graph, ready to run on the audio thread.
pub struct GraphPlan {
    /// Modules in processing (topological) order.
    pub(crate) nodes: Vec<PlanNode>,
    /// Every output port's buffer, grouped by node in processing order, so a
    /// node's inputs always lie before its own outputs.
    pub(crate) outputs: Vec<SignalBuffer>,
    /// Stand-ins for unpatched inputs, holding the port's default value.
    pub(crate) defaults: Vec<SignalBuffer>,
    /// The value each `defaults` buffer is filled with.
    pub(crate) default_values: Vec<f32>,
    /// Sums of polyphonic cables, for mono audio inputs.
    pub(crate) mixdowns: Vec<SignalBuffer>,
    /// Cables closing loops, heard a block behind.
    pub(crate) late: Vec<LateLine>,
    pub(crate) input_taps: Vec<InputTap>,
    pub(crate) output_taps: Vec<OutputTap>,
    /// Scenes modules' knobs, turned as each one's Morph moves.
    pub(crate) scenes: Vec<PlanScenes>,
    /// Where the UI reads the taps, meters and readouts, once the UI side
    /// has given the plan a board (see [`super::monitor_board`]).
    pub(crate) monitor: Option<MonitorWriter>,
    /// The number of samples every buffer currently holds.
    pub(crate) block_len: usize,
    /// The capacity every buffer was allocated with.
    pub(crate) max_block_size: usize,
}

impl GraphPlan {
    /// A plan with no modules.
    pub fn empty(max_block_size: usize) -> Self {
        Self {
            nodes: Vec::new(),
            outputs: Vec::new(),
            defaults: Vec::new(),
            default_values: Vec::new(),
            mixdowns: Vec::new(),
            late: Vec::new(),
            input_taps: Vec::new(),
            output_taps: Vec::new(),
            scenes: Vec::new(),
            monitor: None,
            block_len: max_block_size,
            max_block_size,
        }
    }

    /// The largest block [`process`](Self::process) accepts.
    pub fn max_block_size(&self) -> usize {
        self.max_block_size
    }

    /// The number of modules in the plan.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns true if the plan has no modules.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Node IDs in processing order.
    pub fn processing_order(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().map(|node| node.node_id)
    }

    /// Moves the running module of every node this plan shares with
    /// `previous` into this plan. Modules this plan doesn't use stay in
    /// `previous`, to be dropped with it.
    ///
    /// REAL-TIME SAFE: moves boxes, never allocates.
    pub fn take_over(&mut self, previous: &mut GraphPlan) {
        for node in self.nodes.iter_mut().filter(|node| node.module.is_none()) {
            if let Some(old) = previous.nodes.iter_mut().find(|old| old.node_id == node.node_id) {
                node.module = old.module.take();
                // A crossfade in progress carries on where it was
                node.wet = old.wet;
            }
        }
        // Knobs a scene turned stand where it left them, not where the
        // editor last set them
        for scenes in &self.scenes {
            for PlanKnob { node, knob } in &scenes.knobs {
                let Some(old) = previous.nodes.iter().find(|old| old.node_id == knob.node) else { continue };
                if let (Some(&value), Some(param)) = (old.params.get(knob.param), self.nodes[*node].params.get_mut(knob.param)) {
                    *param = value;
                }
            }
        }
    }

    /// Sets a parameter on a module. Returns false if the node or parameter
    /// doesn't exist in this plan.
    ///
    /// REAL-TIME SAFE.
    pub fn set_parameter(&mut self, node_id: NodeId, param_index: usize, value: f32) -> bool {
        self.nodes
            .iter_mut()
            .find(|node| node.node_id == node_id)
            .and_then(|node| node.params.get_mut(param_index))
            .map(|param| *param = value)
            .is_some()
    }

    /// Gives a module a recording to play, or takes its away. Returns the
    /// recording to drop off the audio thread: the one the module had, or
    /// `sample` itself if the node isn't in this plan or doesn't play
    /// recordings.
    ///
    /// REAL-TIME SAFE: moves pointers, never allocates or frees.
    pub fn load_sample(&mut self, node_id: NodeId, sample: Option<Arc<SampleData>>) -> Option<Arc<SampleData>> {
        match self.nodes.iter_mut().find(|node| node.node_id == node_id).and_then(|node| node.module.as_mut()) {
            Some(module) => module.load_sample(sample),
            None => sample,
        }
    }

    /// Gives a module the markers its recording is cut at.
    ///
    /// REAL-TIME SAFE: the markers come by value.
    pub fn set_markers(&mut self, node_id: NodeId, markers: Markers) {
        if let Some(module) = self.nodes.iter_mut().find(|node| node.node_id == node_id).and_then(|node| node.module.as_mut()) {
            module.set_markers(markers);
        }
    }

    /// Gives a module room to record a take into. Returns room to drop off
    /// the audio thread: the module's old room, or this if the node isn't
    /// in this plan or doesn't record.
    ///
    /// REAL-TIME SAFE.
    pub fn give_room(&mut self, node_id: NodeId, room: Arc<SampleData>) -> Option<Arc<SampleData>> {
        match self.nodes.iter_mut().find(|node| node.node_id == node_id).and_then(|node| node.module.as_mut()) {
            Some(module) => module.give_room(room),
            None => Some(room),
        }
    }

    /// Swaps a module's take for the same recording made to measure.
    /// Returns what to drop off the audio thread.
    ///
    /// REAL-TIME SAFE.
    pub fn settle_take(&mut self, node_id: NodeId, take: u32, sample: Arc<SampleData>) -> Option<Arc<SampleData>> {
        match self.nodes.iter_mut().find(|node| node.node_id == node_id).and_then(|node| node.module.as_mut()) {
            Some(module) => module.settle_take(take, sample),
            None => Some(sample),
        }
    }

    /// Calls `f` with every take a module finished since the last call,
    /// and its node.
    ///
    /// REAL-TIME SAFE.
    pub fn take_finished_takes(&mut self, mut f: impl FnMut(NodeId, FinishedTake)) {
        for node in &mut self.nodes {
            if let Some(take) = node.module.as_mut().and_then(|module| module.take_finished_take()) {
                f(node.node_id, take);
            }
        }
    }

    /// Calls `f` with every recording a module has finished with, to be
    /// dropped off the audio thread.
    ///
    /// REAL-TIME SAFE.
    pub fn take_retired_samples(&mut self, mut f: impl FnMut(Arc<SampleData>)) {
        for module in self.nodes.iter_mut().filter_map(|node| node.module.as_mut()) {
            if let Some(sample) = module.take_retired_sample() {
                f(sample);
            }
        }
    }

    /// Lets every module do its piece of background work (a Looper copying
    /// in a loop), sharing `budget` frames of copying between them.
    ///
    /// REAL-TIME SAFE.
    pub fn background(&mut self, budget: &mut usize) {
        for module in self.nodes.iter_mut().filter_map(|node| node.module.as_mut()) {
            if *budget == 0 {
                break;
            }
            module.background(budget);
        }
    }

    /// Copies up to `budget` frames of a module's recording into
    /// `snapshot`. Returns true once the snapshot is finished, as
    /// [`Unsupported`](SnapshotOutcome::Unsupported) if the node isn't in
    /// this plan.
    ///
    /// REAL-TIME SAFE.
    pub fn fill_snapshot(&mut self, node_id: NodeId, snapshot: &mut Snapshot, budget: &mut usize) -> bool {
        match self.nodes.iter_mut().find(|node| node.node_id == node_id).and_then(|node| node.module.as_mut()) {
            Some(module) => module.fill_snapshot(snapshot, budget),
            None => {
                snapshot.finish(SnapshotOutcome::Unsupported, 0);
                true
            }
        }
    }

    /// Bypasses a module or brings it back, crossfading over the next
    /// [`BYPASS_FADE_SECONDS`]. Returns false if the node doesn't exist in
    /// this plan or can't be bypassed.
    ///
    /// REAL-TIME SAFE.
    pub fn set_bypass(&mut self, node_id: NodeId, bypassed: bool) -> bool {
        self.nodes
            .iter_mut()
            .find(|node| node.node_id == node_id && !node.dry.is_empty())
            .map(|node| node.bypassed = bypassed)
            .is_some()
    }

    /// Plays a chord on a module's outputs from its face, or stops (see
    /// [`DspModule::audition`]). Ignored if the node isn't in this plan.
    ///
    /// REAL-TIME SAFE.
    pub fn audition(&mut self, node_id: NodeId, chord: Option<Audition>) {
        if let Some(module) = self.nodes.iter_mut().find(|node| node.node_id == node_id).and_then(|node| node.module.as_mut()) {
            module.audition(chord);
        }
    }

    /// Applies a change to a running module. Recordings the plan lets go
    /// of go to `retire`, to be dropped off the audio thread. The live
    /// engine, the offline renderer and the tests all come through here,
    /// so a new kind of change can't reach one and not the others.
    ///
    /// REAL-TIME SAFE: as the methods it calls.
    pub fn apply(&mut self, message: PlanMessage, mut retire: impl FnMut(Arc<SampleData>)) {
        let let_go = match message {
            PlanMessage::SetParameter { node_id, param_index, value } => {
                self.set_parameter(node_id, param_index, value);
                None
            }
            PlanMessage::SetBypass { node_id, bypassed } => {
                self.set_bypass(node_id, bypassed);
                None
            }
            PlanMessage::Audition { node_id, chord } => {
                self.audition(node_id, chord);
                None
            }
            PlanMessage::LoadSample { node_id, sample } => self.load_sample(node_id, sample),
            PlanMessage::SetMarkers { node_id, markers } => {
                self.set_markers(node_id, markers);
                None
            }
            PlanMessage::GiveRoom { node_id, room } => self.give_room(node_id, room),
            PlanMessage::SettleTake { node_id, take, sample } => self.settle_take(node_id, take, sample),
        };
        if let Some(sample) = let_go {
            retire(sample);
        }
    }

    /// Resets the internal state of every module (oscillator phases, filter
    /// memory, delay and reverb tails) so playback restarts from silence.
    pub fn reset_modules(&mut self) {
        for module in self.nodes.iter_mut().filter_map(|node| node.module.as_mut()) {
            module.reset();
        }
        // A loop's return restarts silent too, not with the old tail
        for line in &mut self.late {
            line.ring.fill(0.0);
        }
    }

    /// Re-prepares every module for a new sample rate.
    ///
    /// Not real-time safe: modules may reallocate. Call with the stream stopped.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        for module in self.nodes.iter_mut().filter_map(|node| node.module.as_mut()) {
            module.prepare(sample_rate, self.max_block_size);
            module.reset();
        }
    }

    /// Processes one block through every module in order.
    ///
    /// `context.block_size` must not exceed [`max_block_size`](Self::max_block_size);
    /// larger blocks are clamped. Callers with bigger device buffers process
    /// them in chunks.
    ///
    /// REAL-TIME SAFE: no allocation, locking or searching.
    pub fn process(&mut self, context: &ProcessContext) {
        self.run(context, None);
    }

    /// [`process`](Self::process), adding how long each node took, in
    /// nanoseconds, to its entry in `nanos` (one per node, in processing
    /// order: see [`processing_order`](Self::processing_order)). For
    /// `render --bench`: the clock reads cost more than some modules do.
    pub fn process_timed(&mut self, context: &ProcessContext, nanos: &mut [u64]) {
        debug_assert_eq!(nanos.len(), self.nodes.len());
        self.run(context, Some(nanos));
    }

    fn run(&mut self, context: &ProcessContext, mut nanos: Option<&mut [u64]>) {
        debug_assert!(context.block_size <= self.max_block_size);
        self.set_block_len(context.block_size.min(self.max_block_size));

        // Every module in the block sees the same tempo and beat, whatever
        // its place in the processing order
        let mut context = *context;
        if let Some(transport) = self.transport() {
            context.transport = transport;
        }
        // Only modules live input reaches hear how late it is
        let live_context = context;
        let context = &context.with_input_latency(0);
        let fade_step = 1.0 / (BYPASS_FADE_SECONDS * context.sample_rate).max(1.0);

        let Self { nodes, outputs, defaults, mixdowns, late, max_block_size, .. } = self;
        for line in late.iter_mut() {
            line.read(*max_block_size);
        }
        for (index, node) in nodes.iter_mut().enumerate() {
            let started = nanos.is_some().then(Instant::now);
            // Inputs come from earlier nodes, whose buffers all precede ours
            let (upstream, rest) = outputs.split_at_mut(node.outputs.start);
            let own = &mut rest[..node.outputs.len()];
            for buffer in own.iter_mut() {
                buffer.clear();
            }

            let Some(module) = node.module.as_mut() else {
                continue;
            };

            for source in &node.inputs {
                if let InputSource::Mixdown { source, mix } = *source {
                    if upstream[source].channels() > 1 {
                        mixdowns[mix].mix_down(&upstream[source]);
                    }
                }
            }

            let mut inputs = [&EMPTY_BUFFER; MAX_INPUTS];
            for (slot, source) in inputs.iter_mut().zip(&node.inputs) {
                *slot = match *source {
                    InputSource::Output(index) => &upstream[index],
                    InputSource::Default(index) => &defaults[index],
                    // One channel needs no summing: read it where it is
                    InputSource::Mixdown { source, .. } if upstream[source].channels() == 1 => {
                        &upstream[source]
                    }
                    InputSource::Mixdown { mix, .. } => &mixdowns[mix],
                    InputSource::Late(index) => &late[index].heard,
                };
            }

            let inputs = &inputs[..node.inputs.len()];

            if node.bypassed && node.wet == 0.0 {
                // Fully bypassed: the module rests while its input passes by
                pass_dry(own, &node.dry, inputs);
                watch_spectrum(&node.spectrum, inputs, own, context.sample_rate);
                continue;
            }
            if !node.bypassed && node.wet == 0.0 {
                // Coming back from a full bypass. Start from silence rather
                // than replay whatever the delay lines held when it left
                module.reset();
            }

            let context = if node.hears_live { &live_context } else { context };
            module.process(inputs, own, &node.params, &context.hearing(node.heard));

            let target = if node.bypassed { 0.0 } else { 1.0 };
            if node.wet != target {
                node.wet = crossfade(own, &node.dry, inputs, node.wet, target, fade_step);
            }
            if !outputs_finite(own) {
                // A NaN would latch in every filter and delay downstream, and
                // in this module's own state: silence the block and start over
                for buffer in own.iter_mut() {
                    buffer.clear();
                }
                module.reset();
                node.faulted = true;
            }
            watch_spectrum(&node.spectrum, inputs, own, context.sample_rate);
            if let (Some(nanos), Some(started)) = (nanos.as_deref_mut(), started) {
                nanos[index] += started.elapsed().as_nanos() as u64;
            }
        }

        // Every source has played: keep their blocks for next time
        for line in late.iter_mut() {
            if let Some(source) = line.source {
                line.write(&outputs[source]);
            }
        }

        self.turn_scene_knobs();

        // Each monitored output's loudest since it was last read
        for tap in self.output_taps.iter_mut() {
            let now = ChannelPeaks::of(&self.outputs[tap.buffer]);
            tap.peaks = Some(tap.peaks.map_or(now, |earlier| earlier.louder(&now)));
        }
    }

    /// Turns the knobs of each Scenes module whose Morph moved in the
    /// block, for the next. A knob a scene doesn't hold at both ends stays.
    fn turn_scene_knobs(&mut self) {
        for scenes in &self.scenes {
            let Some(module) = self.nodes[scenes.node].module.as_mut() else { continue };
            let Some((morph, from, to)) = module.take_morph() else { continue };
            for PlanKnob { node, knob } in &scenes.knobs {
                if let (Some(value), Some(param)) = (knob.at(morph, from, to), self.nodes[*node].params.get_mut(knob.param)) {
                    *param = value;
                }
            }
        }
    }

    /// How many channels a node's output (counted among its outputs) carried
    /// in the last block, or `None` if there is no such output.
    pub fn output_channels(&self, node_id: NodeId, output_index: usize) -> Option<usize> {
        let node = self.nodes.iter().find(|node| node.node_id == node_id)?;
        let buffer = node.outputs.clone().nth(output_index)?;
        Some(self.outputs[buffer].channels())
    }

    /// What a node's output (counted among its outputs) carried in the last
    /// block, or `None` if there is no such output.
    pub fn output(&self, node_id: NodeId, output_index: usize) -> Option<&SignalBuffer> {
        let node = self.nodes.iter().find(|node| node.node_id == node_id)?;
        let buffer = node.outputs.clone().nth(output_index)?;
        Some(&self.outputs[buffer])
    }

    /// Has every module that mixes several parts keep them apart from now
    /// on (see [`DspModule::keep_stems`]). Allocates: for offline renders.
    pub fn keep_stems(&mut self) {
        let max_block_size = self.max_block_size;
        for module in self.nodes.iter_mut().filter_map(|node| node.module.as_deref_mut()) {
            module.keep_stems(max_block_size);
        }
    }

    /// Calls `f` with each part a module kept apart in the last block: the
    /// node, the part's name, then its left and right.
    pub fn stems(&self, mut f: impl FnMut(NodeId, &'static str, &[f32], &[f32])) {
        for node in &self.nodes {
            if let Some(module) = node.module.as_deref() {
                module.stems(&mut |name, left, right| f(node.node_id, name, left, right));
            }
        }
    }

    /// A running module's parameters, or `None` if it isn't in this plan.
    pub fn parameters(&self, node_id: NodeId) -> Option<&[f32]> {
        self.nodes.iter().find(|node| node.node_id == node_id).map(|node| node.params.as_slice())
    }

    /// Whether a node is bypassed, or `None` if it isn't in this plan.
    pub fn is_bypassed(&self, node_id: NodeId) -> Option<bool> {
        self.nodes.iter().find(|node| node.node_id == node_id).map(|node| node.bypassed)
    }

    /// The patch transport: its tempo and beat, set by the first tempo source
    /// (a Clock) in processing order, or `None` if the patch has none.
    ///
    /// REAL-TIME SAFE.
    pub fn transport(&self) -> Option<TransportState> {
        self.nodes.iter().find_map(|node| {
            node.module.as_ref().and_then(|module| module.transport(&node.params))
        })
    }

    /// The patch tempo, from its [`transport`](Self::transport).
    pub fn tempo_bpm(&self) -> Option<f32> {
        self.transport().and_then(|transport| transport.tempo_bpm)
    }

    /// Sets every buffer's length to `len`, within its allocated capacity.
    fn set_block_len(&mut self, len: usize) {
        if len == self.block_len {
            return;
        }
        let late = self.late.iter_mut().map(|line| &mut line.heard);
        for buffer in self.outputs.iter_mut().chain(&mut self.mixdowns).chain(late) {
            buffer.set_len(len);
        }
        for (buffer, &value) in self.defaults.iter_mut().zip(&self.default_values) {
            buffer.samples.resize(len, value);
        }
        self.block_len = len;
    }

    /// The values at monitored inputs after the last block: the first sample
    /// of the patched signal, or the port default when nothing is patched.
    pub fn input_values(&self) -> impl Iterator<Item = (NodeId, PortIndex, f32)> + '_ {
        self.input_taps.iter().map(|tap| {
            let value = match tap.source {
                MonitorSource::Output(index) => {
                    self.outputs[index].samples.first().copied().unwrap_or(0.0)
                }
                MonitorSource::Constant(value) => value,
            };
            (tap.node_id, tap.input_index, value)
        })
    }

    /// The values at monitored outputs over the blocks since they were last
    /// read (see [`restart_output_peaks`](Self::restart_output_peaks)): the
    /// peak of each channel, and across all of them, as the sample with the
    /// largest magnitude, sign preserved, so cables can draw bipolar signals
    /// such as LFOs swinging either side of zero.
    pub fn output_values(&self) -> impl Iterator<Item = (NodeId, PortIndex, f32, ChannelPeaks)> + '_ {
        self.output_taps.iter().map(|tap| {
            let peaks = tap.peaks.unwrap_or_else(|| ChannelPeaks::of(&self.outputs[tap.buffer]));
            (tap.node_id, tap.output_index, peaks.overall(), peaks)
        })
    }

    /// Starts each monitored output's peaks over, once they've been read.
    pub fn restart_output_peaks(&mut self) {
        for tap in self.output_taps.iter_mut() {
            tap.peaks = None;
        }
    }

    /// The first module in processing order that produces the final audio
    /// output, if any.
    pub fn output_module(&self) -> Option<&dyn DspModule> {
        self.nodes
            .iter()
            .filter_map(|node| node.module.as_deref())
            .find(|module| module.get_audio_output().is_some())
    }

    /// Takes the output module's meter readings since the last call.
    pub fn take_output_levels(&mut self) -> Option<OutputLevels> {
        self.nodes
            .iter_mut()
            .filter_map(|node| node.module.as_deref_mut())
            .find(|module| module.get_audio_output().is_some())
            .and_then(|module| module.take_output_levels())
    }

    /// The final stereo output, as (left, right). The slices span the
    /// module's full buffer; the first `block_size` samples are this block.
    pub fn audio_output(&self) -> Option<(&[f32], &[f32])> {
        self.output_module().and_then(|module| module.get_audio_output())
    }

    /// Hands the UI this callback's monitored readings, the modules' meters
    /// and readouts among them, if the plan has a board to put them on.
    ///
    /// REAL-TIME SAFE: the board was made with the plan.
    pub fn publish_monitors(&mut self) {
        if let Some(mut board) = self.monitor.take() {
            board.publish(self);
            self.monitor = Some(board);
        }
    }

    /// Calls `f` for each oscilloscope with a new capture ready.
    ///
    /// REAL-TIME SAFE: captures are lent, not copied.
    /// Hands over each module that made a NaN or infinity since the last
    /// call (and was silenced and reset for it), clearing the record.
    ///
    /// REAL-TIME SAFE.
    pub fn take_faults(&mut self, mut f: impl FnMut(NodeId)) {
        for node in &mut self.nodes {
            if std::mem::take(&mut node.faulted) {
                f(node.node_id);
            }
        }
    }

    pub fn take_scope_captures(&mut self, mut f: impl FnMut(NodeId, &[f32], &[f32], bool)) {
        for node in &mut self.nodes {
            if let Some(module) = node.module.as_mut() {
                if let Some((channel1, channel2, triggered)) = module.take_scope_data() {
                    f(node.node_id, channel1, channel2, triggered);
                }
            }
        }
    }
}

/// Whether every channel a module's outputs carried this block is finite.
///
/// REAL-TIME SAFE.
fn outputs_finite(outputs: &[SignalBuffer]) -> bool {
    outputs.iter().all(|buffer| (0..buffer.channels()).all(|channel| all_finite(&buffer.voice(channel).samples)))
}

/// Copies a watched node's input and heard output to its spectrum tap.
///
/// REAL-TIME SAFE: stores into the tap's ring.
fn watch_spectrum(watch: &Option<SpectrumWatch>, inputs: &[&SignalBuffer], outputs: &[SignalBuffer], sample_rate: f32) {
    if let Some(watch) = watch {
        if let (Some(input), Some(output)) = (inputs.get(watch.input), outputs.get(watch.output)) {
            watch.tap.write(input, output, sample_rate);
        }
    }
}

/// Copies each bypassed output's input straight through, every channel of
/// it. Outputs with nothing to pass stay silent.
fn pass_dry(outputs: &mut [SignalBuffer], dry: &[Option<usize>], inputs: &[&SignalBuffer]) {
    for (output, source) in outputs.iter_mut().zip(dry) {
        if let Some(input) = source.and_then(|index| inputs.get(index)) {
            let channels = output.set_channels(input.channels());
            for channel in 0..channels {
                let input = &input.voice(channel).samples;
                for (out, &sample) in output.channel_mut(channel).iter_mut().zip(input) {
                    *out = sample;
                }
            }
        }
    }
}

/// Blends the module's outputs with its dry inputs, moving the wet amount
/// from `from` toward `to` by `step` per sample. Returns where it got to.
fn crossfade(
    outputs: &mut [SignalBuffer],
    dry: &[Option<usize>],
    inputs: &[&SignalBuffer],
    from: f32,
    to: f32,
    step: f32,
) -> f32 {
    let advance = |wet: f32| if to > wet { (wet + step).min(to) } else { (wet - step).max(to) };
    let mut reached = from;
    for (output, source) in outputs.iter_mut().zip(dry) {
        let input = source.and_then(|index| inputs.get(index));
        // As many channels as either side carries
        let wanted = output.channels().max(input.map_or(1, |buffer| buffer.channels()));
        let channels = output.set_channels(wanted);
        for channel in 0..channels {
            let input = input.map(|buffer| &buffer.voice(channel).samples);
            let mut wet = from;
            for (i, sample) in output.channel_mut(channel).iter_mut().enumerate() {
                wet = advance(wet);
                let dry = input.and_then(|samples| samples.get(i)).copied().unwrap_or(0.0);
                *sample = dry + wet * (*sample - dry);
            }
            reached = wet;
        }
    }
    reached
}
