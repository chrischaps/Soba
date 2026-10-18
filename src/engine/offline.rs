//! Offline (non-real-time) rendering.
//!
//! Runs an [`AudioGraph`] directly, with no audio device, so patches and
//! modules can be rendered to sample buffers for tests, measurements, and the
//! `render` tool. Uses the same module registry, patch compilation and
//! [`GraphPlan`] swapping as the live app.

use crate::dsp::denormal::DenormalGuard;
use crate::dsp::{InputAudio, MidiEvent, MidiMessage, ProcessContext, KEYS_CHANNEL};
use crate::modules::note_in::{self, Source};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use crate::persistence::sample_files::{self, SampleBase};
use crate::persistence::{compile_patch, CompiledPatch, Patch, PatchError};

use super::commands::NodeId;
use super::{create_module_registry, AudioGraph, EngineCommand, GraphPlan, PlanMessage};

/// Rendered stereo audio.
#[derive(Debug, Default, Clone)]
pub struct StereoBuffer {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

/// Drives an [`AudioGraph`] block by block without an audio device.
pub struct OfflineRenderer {
    graph: AudioGraph,
    plan: Box<GraphPlan>,
    context: ProcessContext<'static>,
    /// Frames processed so far: the renderer's sample clock.
    position: u64,
    /// Queued MIDI as (frame, event), in frame order.
    midi: Vec<(u64, MidiEvent)>,
    /// The current block's MIDI, re-based to the block start.
    block_midi: Vec<MidiEvent>,
    /// What Audio Input modules hear, from frame 0 of the sample clock.
    /// Empty (silence) unless set.
    input: StereoBuffer,
    /// The current block of `input`.
    block_input: StereoBuffer,
    /// What the output limiter has done so far.
    limiter: LimiterReport,
    /// Every module that made a NaN or infinity (and was reset), in the
    /// order they first did.
    faults: Vec<NodeId>,
    /// How long processing has taken, once [`time`](Self::time) is on.
    bench: Option<Bench>,
}

/// How long a render spent processing the graph: the whole of it and each
/// node's share. Loading, analysis and copying the output out aren't in it.
#[derive(Debug, Default, Clone)]
pub struct Bench {
    /// Time spent in [`GraphPlan::process`].
    pub process: Duration,
    /// Frames processed while timing.
    pub frames: u64,
    /// Nanoseconds each node spent processing, for nodes in an earlier plan.
    by_node: HashMap<NodeId, u64>,
    /// The same for the current plan's nodes, in its processing order.
    current: Vec<u64>,
}

impl Bench {
    /// Each node's processing time in nanoseconds, the heaviest first.
    pub fn nodes(&self) -> Vec<(NodeId, u64)> {
        let mut nodes: Vec<_> = self.by_node.iter().map(|(&id, &ns)| (id, ns)).collect();
        nodes.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        nodes
    }

    /// Folds the current plan's times into `by_node`.
    fn settle(&mut self, plan: &GraphPlan) {
        for (node_id, ns) in plan.processing_order().zip(self.current.drain(..)) {
            *self.by_node.entry(node_id).or_default() += ns;
        }
        self.current.resize(plan.len(), 0);
    }
}

/// What the Audio Output's limiter has done over a render: how hard the
/// patch drove it, and how often it had to act. The limiter is there to
/// catch accidents, so a well-staged patch keeps it idle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterReport {
    /// Highest peak going into the limiter on either channel. It can exceed
    /// 1.0: that's how far over full scale the patch drives the output.
    pub peak_in: f32,
    /// The most the limiter turned the output down, as a gain (1.0 = never).
    pub deepest: f32,
    /// Blocks in which it turned the output down by more than
    /// [`LIMITING_DB`](Self::LIMITING_DB), out of `blocks`.
    pub blocks_limited: u64,
    pub blocks: u64,
}

impl LimiterReport {
    /// Gain reduction in a block below which the limiter counts as idle:
    /// a 1 dB dip on a lone transient goes unheard.
    pub const LIMITING_DB: f32 = 1.0;

    /// The share of the render (0-1) the limiter spent turning it down.
    pub fn time_limiting(&self) -> f32 {
        if self.blocks == 0 {
            0.0
        } else {
            self.blocks_limited as f32 / self.blocks as f32
        }
    }
}

impl Default for LimiterReport {
    fn default() -> Self {
        Self { peak_in: 0.0, deepest: 1.0, blocks_limited: 0, blocks: 0 }
    }
}

impl OfflineRenderer {
    /// Creates a renderer with an empty graph.
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        Self {
            graph: AudioGraph::with_registry(sample_rate, block_size, create_module_registry()),
            plan: Box::new(GraphPlan::empty(block_size)),
            context: ProcessContext::new(sample_rate, block_size),
            position: 0,
            midi: Vec::new(),
            block_midi: Vec::new(),
            input: StereoBuffer::default(),
            block_input: StereoBuffer { left: vec![0.0; block_size], right: vec![0.0; block_size] },
            limiter: LimiterReport::default(),
            faults: Vec::new(),
            bench: None,
        }
    }

    /// Starts timing processing, from nothing: see [`bench`](Self::bench).
    /// Each node is timed too, which costs a little itself.
    pub fn time(&mut self) {
        self.bench = Some(Bench { current: vec![0; self.plan.len()], ..Bench::default() });
    }

    /// How long processing took since [`time`](Self::time) was called, or
    /// `None` if it wasn't.
    pub fn bench(&self) -> Option<Bench> {
        let mut bench = self.bench.clone()?;
        bench.settle(&self.plan);
        Some(bench)
    }

    /// Creates a renderer and builds `patch` in it.
    ///
    /// Returns the compiled patch too, for its node-ID map and any warnings.
    pub fn from_patch(
        patch: &Patch,
        sample_rate: f32,
        block_size: usize,
    ) -> Result<(Self, CompiledPatch), PatchError> {
        let compiled = compile_patch(patch)?;
        let mut renderer = Self::new(sample_rate, block_size);
        for command in &compiled.commands {
            renderer.apply(command.clone());
        }
        Ok((renderer, compiled))
    }

    /// Loads the files a compiled patch's modules play (a Sampler's),
    /// resolving paths saved relative to the patch against `base`, and
    /// resampling each to the renderer's rate. Returns a warning for each
    /// file that couldn't be loaded or was cut short.
    pub fn load_samples(&mut self, compiled: &CompiledPatch, base: SampleBase) -> Vec<String> {
        let mut warnings = Vec::new();
        for (node_id, file) in &compiled.files {
            let key = sample_files::resolve(file, base);
            match sample_files::load(&key) {
                Ok(decoded) => {
                    if decoded.truncated {
                        warnings.push(format!("{file} is longer than the 5 minutes a Sampler keeps, and was cut"));
                    }
                    // Sliced by Onsets: found on the file as it was recorded, as the editor finds them
                    if let Some((_, slicing)) = compiled.slicing.iter().find(|(id, _)| id == node_id) {
                        self.apply(EngineCommand::SetMarkers { node_id: *node_id, markers: slicing.markers(&decoded.sample) });
                    }
                    let sample = Arc::new(decoded.sample.playable_at(self.context.sample_rate));
                    self.apply(EngineCommand::LoadSample { node_id: *node_id, sample: Some(sample) });
                }
                Err(e) => warnings.push(format!("Couldn't load sample {file}: {e}")),
            }
        }
        warnings
    }

    /// Applies an engine command (add module, connect, set parameter, ...).
    pub fn apply(&mut self, command: EngineCommand) -> bool {
        // Keep the running plan in step, as the live engine does
        match command {
            EngineCommand::SetParameter { node_id, param_index, value } => {
                self.plan.set_parameter(node_id, param_index, value);
            }
            EngineCommand::SetBypass { node_id, bypassed } => {
                self.plan.set_bypass(node_id, bypassed);
            }
            EngineCommand::Audition { node_id, chord } => {
                self.plan.audition(node_id, chord);
                return true;
            }
            _ => {}
        }
        self.graph.handle_command(command)
    }

    /// Mutable access to the underlying graph. Changes take effect from the
    /// next render.
    pub fn graph_mut(&mut self) -> &mut AudioGraph {
        &mut self.graph
    }

    /// Installs a new plan if the graph changed, carrying running modules over.
    fn sync_plan(&mut self) {
        if let Some(mut plan) = self.graph.take_plan() {
            if let Some(bench) = &mut self.bench {
                bench.settle(&self.plan);
                bench.current = vec![0; plan.len()];
            }
            plan.take_over(&mut self.plan);
            self.plan = plan;
        }
        // Recordings for modules already running, after the plan that has them
        for (node_id, sample) in self.graph.take_sample_loads() {
            self.plan.apply(PlanMessage::LoadSample { node_id, sample }, drop);
        }
        for handoff in self.graph.take_take_handoffs() {
            self.plan.apply(handoff, drop);
        }
        // Nothing here is real time, so loops loaded are copied in at once
        self.plan.background(&mut { usize::MAX });
        self.plan.take_retired_samples(drop);
    }

    /// Queues a MIDI message to arrive at `frame` on the renderer's sample
    /// clock (see [`position`](Self::position)), as live MIDI would be
    /// placed by the audio thread.
    pub fn queue_midi(&mut self, frame: u64, channel: u8, message: MidiMessage) {
        let at = self.midi.partition_point(|&(queued, _)| queued <= frame);
        self.midi.insert(at, (frame, MidiEvent::new(0, channel, message)));
    }

    /// Sets what Audio Input modules hear, as the input device would bring
    /// it, starting at frame 0 of the sample clock (see
    /// [`position`](Self::position)). Past its end they hear silence.
    pub fn set_audio_input(&mut self, input: StereoBuffer) {
        self.input = input;
    }

    /// Copies the input that falls in the next block into `block_input`.
    fn take_block_input(&mut self) {
        let start = (self.position as usize).min(self.input.left.len());
        let end = (start + self.context.block_size).min(self.input.left.len());
        let count = end - start;
        let block = &mut self.block_input;
        block.left[..count].copy_from_slice(&self.input.left[start..end]);
        block.right[..count].copy_from_slice(&self.input.right[start..end]);
        block.left[count..].fill(0.0);
        block.right[count..].fill(0.0);
    }

    /// Frames processed so far. Renders run in whole blocks, so this can be
    /// ahead of the frames returned when a render isn't a whole number of
    /// blocks.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Moves the queued MIDI that falls in the next block into `block_midi`.
    fn take_block_midi(&mut self) {
        let end = self.position + self.context.block_size as u64;
        let count = self.midi.partition_point(|&(frame, _)| frame < end);
        self.block_midi.clear();
        for (frame, mut event) in self.midi.drain(..count) {
            event.sample_offset = frame.saturating_sub(self.position) as u32;
            self.block_midi.push(event);
        }
    }

    /// What the output limiter has done since the renderer was made.
    pub fn limiter(&self) -> LimiterReport {
        self.limiter
    }

    /// The modules that made a NaN or infinity so far, each once, in the
    /// order they first did. The engine silenced and reset each for the
    /// block it happened in.
    pub fn faults(&self) -> &[NodeId] {
        &self.faults
    }

    /// Adds the block just processed to the limiter report.
    fn note_limiter(&mut self) {
        let Some(levels) = self.plan.take_output_levels() else { return };
        let report = &mut self.limiter;
        report.peak_in = report.peak_in.max(levels.pre[0]).max(levels.pre[1]);
        report.deepest = report.deepest.min(levels.limiter_gain);
        report.blocks += 1;
        if levels.limiter_gain < crate::dsp::dynamics::db_to_gain(-LimiterReport::LIMITING_DB) {
            report.blocks_limited += 1;
        }
    }

    /// The sample rate being rendered at.
    pub fn sample_rate(&self) -> f32 {
        self.context.sample_rate
    }

    /// Renders `frames` frames of the graph's audio output. A graph without an
    /// output module renders silence.
    pub fn render(&mut self, frames: usize) -> StereoBuffer {
        let mut out = StereoBuffer {
            left: Vec::with_capacity(frames),
            right: Vec::with_capacity(frames),
        };

        // Same floating-point mode as the live audio callback
        let _denormals = DenormalGuard::new();

        self.sync_plan();
        while out.left.len() < frames {
            self.take_block_midi();
            self.take_block_input();
            let input = InputAudio { left: &self.block_input.left, right: &self.block_input.right };
            let context = self.context.with_midi(&self.block_midi).with_input(input);
            match &mut self.bench {
                Some(bench) => {
                    let started = Instant::now();
                    self.plan.process_timed(&context, &mut bench.current);
                    bench.process += started.elapsed();
                    bench.frames += self.context.block_size as u64;
                }
                None => self.plan.process(&context),
            }
            self.position += self.context.block_size as u64;
            self.note_limiter();
            let faults = &mut self.faults;
            self.plan.take_faults(|node_id| {
                if !faults.contains(&node_id) {
                    faults.push(node_id);
                }
            });

            let wanted = (frames - out.left.len()).min(self.context.block_size);
            match self.plan.audio_output() {
                Some((left, right)) => {
                    out.left.extend_from_slice(&left[..wanted]);
                    out.right.extend_from_slice(&right[..wanted]);
                }
                None => {
                    out.left.resize(out.left.len() + wanted, 0.0);
                    out.right.resize(out.right.len() + wanted, 0.0);
                }
            }
        }
        out
    }

    /// Renders `seconds` of audio.
    pub fn render_seconds(&mut self, seconds: f32) -> StereoBuffer {
        let frames = (seconds * self.context.sample_rate).round() as usize;
        self.render(frames)
    }

    /// Renders `seconds` of audio, applying each command at its frame on
    /// the renderer's sample clock, as a player turning knobs and pressing
    /// footswitches would. A command lands at the start of the block its
    /// frame falls in, as one sent to the live engine does.
    pub fn render_with_commands(&mut self, seconds: f32, mut commands: Vec<(u64, EngineCommand)>) -> StereoBuffer {
        commands.sort_by_key(|&(frame, _)| frame);
        let start = self.position;
        let end = start + (seconds * self.context.sample_rate).round() as u64;
        let block = self.context.block_size as u64;
        let mut out = StereoBuffer::default();
        for (at, command) in commands {
            // Whole blocks only, so no rendered frames are dropped between calls
            let frames = at.min(end).saturating_sub(self.position) / block * block;
            out.append(self.render(frames as usize));
            self.apply(command);
        }
        out.append(self.render(end.saturating_sub(self.position) as usize));
        let frames = (end - start) as usize;
        out.left.truncate(frames);
        out.right.truncate(frames);
        out
    }

    /// Renders `seconds` of audio while [`AUDITION`] is played into every
    /// Note In of `patch`, so patches that wait for a player make sound
    /// offline too.
    ///
    /// It plays as the computer keys do when every Note In hears them, at
    /// each one's Key Velocity, and as MIDI at velocity 100 when every one
    /// hears MIDI; a patch with Note Ins of each kind gets both. Each note
    /// lands on its sample.
    pub fn render_audition(&mut self, patch: &Patch, seconds: f32) -> StereoBuffer {
        let sources: Vec<Source> = patch
            .all_nodes()
            .into_iter()
            .filter(|n| n.module_id == note_in::ID)
            .map(|n| {
                let source = n.parameters.iter().find(|p| p.name == "Source").map_or(0.0, |p| p.value.as_f32());
                Source::from_param(source)
            })
            .collect();
        let as_keys = sources.iter().any(|&s| s != Source::Midi);
        let as_midi = sources.contains(&Source::Midi) || !as_keys && !sources.is_empty();
        let channels = [(as_keys, KEYS_CHANNEL), (as_midi, 0)];

        let (start, sample_rate) = (self.position, self.context.sample_rate);
        let frame = |seconds: f32| start + (seconds * sample_rate).round() as u64;
        for &(note, at, length) in AUDITION {
            for &(_, channel) in channels.iter().filter(|(plays, _)| *plays) {
                self.queue_midi(frame(at), channel, MidiMessage::NoteOn { note, velocity: 100 });
                self.queue_midi(frame(at + length), channel, MidiMessage::NoteOff { note, velocity: 0 });
            }
        }
        self.render_seconds(seconds)
    }
}

/// The phrase [`OfflineRenderer::render_audition`] plays, as (MIDI note,
/// start, length) with times in seconds: a rising C major arpeggio, then the
/// chord held.
pub const AUDITION: &[(u8, f32, f32)] = &[
    (60, 0.0, 0.4),
    (64, 0.5, 0.4),
    (67, 1.0, 0.4),
    (72, 1.5, 0.8),
    (60, 2.5, 1.5),
    (64, 2.5, 1.5),
    (67, 2.5, 1.5),
];

/// Reads a mono or stereo WAV, MP3, FLAC or Ogg file (a bigger one gives
/// its first two channels), returning it with its sample rate.
pub fn read_wav(path: &std::path::Path) -> Result<(StereoBuffer, u32), String> {
    let sample = crate::persistence::sample_files::read_file(path)?.sample;
    let input = StereoBuffer { left: sample.left().to_vec(), right: sample.right().to_vec() };
    Ok((input, sample.sample_rate() as u32))
}

/// Reads a WAV as [`read_wav`] does, resampled to `sample_rate` if it was
/// recorded at another, returning it with the rate it was recorded at.
pub fn read_wav_at(path: &std::path::Path, sample_rate: u32) -> Result<(StereoBuffer, u32), String> {
    let mut sample = crate::persistence::sample_files::read_file(path)?.sample;
    let recorded = sample.sample_rate() as u32;
    if recorded != sample_rate {
        sample = sample.resampled(sample_rate as f32);
    }
    let input = StereoBuffer { left: sample.left().to_vec(), right: sample.right().to_vec() };
    Ok((input, recorded))
}

impl StereoBuffer {
    /// Adds `other` to the end of this buffer.
    pub fn append(&mut self, other: StereoBuffer) {
        self.left.extend(other.left);
        self.right.extend(other.right);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{peak, rms, Spectrum};
    use crate::modules::Oscillator;
    use crate::persistence::{ConnectionData, NamedParameter, NodeData, ParameterValue};

    /// Oscillator (default C4, chosen waveform) into the output's Mono input.
    fn osc_patch(waveform: usize) -> Patch {
        let mut patch = Patch::new("osc");
        let mut osc = NodeData::new(1, "osc.sine", (0.0, 0.0));
        osc.parameters = vec![
            NamedParameter::new("Octave", ParameterValue::Number(0.0)),
            NamedParameter::new("FM Depth", ParameterValue::Number(0.0)),
            NamedParameter::new("Waveform", ParameterValue::Select(waveform)),
            NamedParameter::new("Pulse Width", ParameterValue::LinearRange(0.5)),
        ];
        patch.nodes.push(osc);
        patch.nodes.push(NodeData::new(2, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 2, "Mono"));
        patch
    }

    #[test]
    fn a_timed_render_times_every_node_and_sounds_the_same() {
        let patch = osc_patch(0);
        let (mut plain, compiled) = OfflineRenderer::from_patch(&patch, 48_000.0, 256).unwrap();
        let (mut timed, _) = OfflineRenderer::from_patch(&patch, 48_000.0, 256).unwrap();
        assert!(timed.bench().is_none());
        timed.time();
        let heard = timed.render(1000);
        assert_eq!(heard.left, plain.render(1000).left);

        let bench = timed.bench().unwrap();
        // Whole blocks: 1000 frames is four of 256
        assert_eq!(bench.frames, 1024);
        assert!(bench.process > Duration::ZERO);
        let nodes = bench.nodes();
        assert_eq!(nodes.len(), compiled.node_ids.len());
        assert!(nodes.windows(2).all(|pair| pair[0].1 >= pair[1].1), "heaviest first");
        // The oscillator does more than nothing
        let osc = compiled.node_ids[&1];
        assert!(nodes.iter().any(|&(id, ns)| id == osc && ns > 0));
    }

    /// A Note In with one voice, playing from `source`.
    fn note_in(id: u64, source: Source) -> NodeData {
        let mut node = NodeData::new(id, note_in::ID, (-200.0, 0.0));
        node.parameters = vec![
            NamedParameter::new("Voices", ParameterValue::Number(1.0)),
            NamedParameter::new("Source", ParameterValue::Select(source as usize)),
        ];
        node
    }

    /// A one-voice Note In playing from `source`, playing a sine gated
    /// straight into a VCA.
    fn played_sine(source: Source) -> Patch {
        let mut patch = Patch::new("played");
        patch.nodes.push(note_in(1, source));
        patch.nodes.push(NodeData::new(2, "osc.sine", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(3, "util.vca", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(4, "output.audio", (0.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Pitch", 2, "V/Oct"));
        patch.connections.push(ConnectionData::new(1, "Gate", 3, "CV"));
        patch.connections.push(ConnectionData::new(2, "Out", 3, "In"));
        patch.connections.push(ConnectionData::new(3, "Out", 4, "Mono"));
        patch
    }

    #[test]
    fn test_audition_plays_note_ins_from_the_keys_or_midi() {
        let sr = 48000.0;
        for player in [Source::Keys, Source::Midi, Source::Both] {
            let patch = played_sine(player);
            let (mut r, _) = OfflineRenderer::from_patch(&patch, sr, 256).unwrap();
            // Not a whole number of blocks, and ending mid-phrase
            assert_eq!(r.render_audition(&patch, 0.7).left.len(), (0.7 * sr) as usize, "{:?}", player);

            let (mut r, _) = OfflineRenderer::from_patch(&patch, sr, 256).unwrap();
            let out = r.render_audition(&patch, 2.0);
            assert_eq!(out.left.len(), 2 * sr as usize, "{:?}", player);

            let window = |from: f32, to: f32| &out.left[(from * sr) as usize..(to * sr) as usize];
            // The gap between the first two notes is silent, once the VCA's
            // declick has faded the first one out
            assert!(rms(window(0.44, 0.49)) < 0.01, "{:?}: gate should be closed between notes", player);
            // The second note is E4
            let f = Spectrum::of(window(0.55, 0.85), sr).dominant_frequency();
            let cents = 1200.0 * (f / (Oscillator::C4_HZ as f64 * 2f64.powf(4.0 / 12.0))).log2();
            assert!(cents.abs() < 5.0, "{:?}: expected E4, measured {:.2} Hz", player, f);
        }
    }

    #[test]
    fn test_audition_plays_the_keys_at_their_velocity() {
        // A Note In played from the keys, its Velocity opening the VCA:
        // the keys play as hard as its Key Velocity says
        let level = |key_velocity: f32| {
            let mut patch = played_sine(Source::Keys);
            patch.nodes[0].parameters.push(NamedParameter::new("Key Velocity", ParameterValue::Scalar(key_velocity)));
            patch.connections.retain(|c| c.from_port != "Gate");
            patch.connections.push(ConnectionData::new(1, "Velocity", 3, "CV"));
            let (mut r, _) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();
            let out = r.render_audition(&patch, 0.4);
            rms(&out.left[4800..])
        };
        let (soft, hard) = (level(0.25), level(1.0));
        assert!((hard / soft - 4.0).abs() < 0.2, "{soft} and {hard}");
    }

    #[test]
    fn test_renders_requested_length() {
        let (mut r, _) = OfflineRenderer::from_patch(&osc_patch(0), 48000.0, 256).unwrap();
        // Not a multiple of the block size
        let out = r.render(1000);
        assert_eq!(out.left.len(), 1000);
        assert_eq!(out.right.len(), 1000);
    }

    #[test]
    fn test_patch_plays_middle_c() {
        let sr = 44100.0;
        let (mut r, compiled) = OfflineRenderer::from_patch(&osc_patch(0), sr, 512).unwrap();
        assert!(compiled.warnings.is_empty());

        // Skip the first 0.1 s (parameter smoothing), then measure ~1.5 s
        r.render_seconds(0.1);
        let out = r.render(65536);

        assert!(out.left.iter().all(|s| s.is_finite()));
        assert!(rms(&out.left) > 0.1, "patch should be audible");
        assert_eq!(out.left, out.right, "mono input feeds both channels");

        let f = Spectrum::of(&out.left, sr).dominant_frequency();
        let cents = 1200.0 * (f / Oscillator::C4_HZ as f64).log2();
        assert!(cents.abs() < 0.5, "expected C4, measured {:.3} Hz ({:+.2} cents)", f, cents);
    }

    #[test]
    fn test_output_stage_keeps_peaks_in_range() {
        // A full-scale saw at volume 0.8 stays within ±1 after the output stage
        let (mut r, _) = OfflineRenderer::from_patch(&osc_patch(1), 48000.0, 256).unwrap();
        let out = r.render_seconds(0.5);
        assert!(peak(&out.left) <= 1.0);
    }

    #[test]
    fn test_unpatched_right_input_is_normalled_end_to_end() {
        // osc -> Delay "In L" only; both delay outputs feed the output module.
        // With nothing in "In R", the delay must output the same on both sides.
        let mut patch = osc_patch(1);
        patch.connections.clear();
        patch.nodes.push(NodeData::new(3, "fx.delay", (100.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 3, "In L"));
        patch.connections.push(ConnectionData::new(3, "Out L", 2, "Left"));
        patch.connections.push(ConnectionData::new(3, "Out R", 2, "Right"));

        let (mut r, compiled) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        let out = r.render_seconds(0.5);
        assert!(rms(&out.right) > 0.05, "right channel should carry the mono source");
        assert_eq!(out.left, out.right);
    }

    /// A Clock playing a Sampler that plays `file`, into the output.
    fn sampler_patch(file: &str) -> Patch {
        let mut patch = Patch::new("sampled");
        patch.nodes.push(NodeData::new(1, "util.clock", (0.0, 0.0)));
        let mut sampler = NodeData::new(2, "source.sampler", (100.0, 0.0));
        sampler.file = Some(file.to_string());
        patch.nodes.push(sampler);
        patch.nodes.push(NodeData::new(3, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Gate", 2, "Gate"));
        patch.connections.push(ConnectionData::new(2, "L", 3, "Left"));
        patch.connections.push(ConnectionData::new(2, "R", 3, "Right"));
        patch
    }

    #[test]
    fn test_a_sampler_plays_its_file_from_beside_the_patch() {
        use crate::dsp::SampleData;
        let folder = std::env::temp_dir().join(format!("soba-sampler-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("kit")).unwrap();
        // A 44.1 kHz tone, played at 48 kHz: still 440 Hz
        let tone: Vec<f32> = (0..44100).map(|n| (std::f32::consts::TAU * 440.0 * n as f32 / 44100.0).sin() * 0.5).collect();
        let wav = crate::persistence::sample_files::encode_wav(&SampleData::mono(tone, 44100.0));
        std::fs::write(folder.join("kit").join("tone.wav"), wav).unwrap();

        let (mut r, compiled) = OfflineRenderer::from_patch(&sampler_patch("kit/tone.wav"), 48000.0, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        assert!(r.load_samples(&compiled, SampleBase::Folder(&folder)).is_empty());
        let out = r.render_seconds(0.4);
        assert!(rms(&out.left) > 0.1, "rms {}", rms(&out.left));
        assert_eq!(out.left, out.right, "a mono file plays on both sides");
        let hz = Spectrum::of(&out.left[4800..], 48000.0).dominant_frequency();
        assert!((hz - 440.0).abs() < 3.0, "{hz}");
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn test_an_input_wav_at_another_rate_is_resampled() {
        use crate::dsp::SampleData;
        let path = std::env::temp_dir().join(format!("soba-input-{}.wav", std::process::id()));
        // One second of a 44.1 kHz tone, heard at 48 kHz: still one second of 440 Hz
        let tone: Vec<f32> = (0..44100).map(|n| (std::f32::consts::TAU * 440.0 * n as f32 / 44100.0).sin() * 0.5).collect();
        std::fs::write(&path, crate::persistence::sample_files::encode_wav(&SampleData::mono(tone, 44100.0))).unwrap();

        let (audio, recorded) = read_wav_at(&path, 48000).unwrap();
        assert_eq!(recorded, 44100);
        assert!((audio.left.len() as i64 - 48000).abs() <= 1, "{} frames", audio.left.len());
        assert_eq!(audio.left, audio.right, "a mono file is heard on both sides");
        let hz = Spectrum::of(&audio.left, 48000.0).dominant_frequency();
        assert!((hz - 440.0).abs() < 3.0, "{hz}");
        // At its own rate it's read as it is
        let (same, _) = read_wav_at(&path, 44100).unwrap();
        assert_eq!(same.left, read_wav(&path).unwrap().0.left);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_a_sampler_sliced_by_onsets_is_cut_where_the_editor_would() {
        use crate::dsp::onsets::{HandMarkers, DEFAULT_SENSITIVITY};
        use crate::dsp::primitives::ONSETS;
        use crate::dsp::SampleData;
        let folder = std::env::temp_dir().join(format!("soba-onsets-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        // Half a second of silence, then a tone that starts at once: one hit
        let file: Vec<f32> = (0..48000)
            .map(|n| if n < 24000 { 0.0 } else { (std::f32::consts::TAU * 440.0 * n as f32 / 48000.0).sin() * 0.5 })
            .collect();
        std::fs::write(folder.join("hit.wav"), crate::persistence::sample_files::encode_wav(&SampleData::mono(file, 48000.0))).unwrap();

        // The Clock strikes Root, slice 1, every half second
        let render = |slices: f32, hand: Option<HandMarkers>| {
            let mut patch = sampler_patch("hit.wav");
            let sampler = &mut patch.nodes[1];
            sampler.parameters.push(NamedParameter::new("Slices", ParameterValue::Number(slices)));
            sampler.slice_markers = hand;
            let (mut r, compiled) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();
            assert!(r.load_samples(&compiled, SampleBase::Folder(&folder)).is_empty());
            peak(&r.render_seconds(1.0).left)
        };
        assert!(render(1.0, None) > 0.2, "unsliced, it reaches the tone");
        // The hit is found: slice 1 is the silence before it
        let found = render(ONSETS, None);
        assert!(found < 1e-3, "{found}");
        // A marker moved by hand to 75% puts half the tone in slice 1...
        assert!(render(ONSETS, Some(HandMarkers::new(DEFAULT_SENSITIVITY, vec![0.75]))) > 0.2);
        // ...unless Sensitivity has moved on since, when they're found afresh
        assert!(render(ONSETS, Some(HandMarkers::new(0.9, vec![0.75]))) < 1e-3);
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn test_a_missing_sample_is_a_warning_and_a_silent_sampler() {
        let (mut r, compiled) = OfflineRenderer::from_patch(&sampler_patch("gone/nowhere.wav"), 48000.0, 256).unwrap();
        let warnings = r.load_samples(&compiled, SampleBase::Folder(&std::env::temp_dir()));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("gone/nowhere.wav"), "{warnings:?}");
        assert_eq!(peak(&r.render_seconds(0.2).left), 0.0);
    }

    #[test]
    fn test_editing_between_renders_keeps_module_state() {
        // Rendering in two halves, with an unrelated edit in between, must
        // match rendering in one go: the oscillator keeps its phase
        let whole = OfflineRenderer::from_patch(&osc_patch(0), 48000.0, 256).unwrap().0.render(2048);

        let (mut r, _) = OfflineRenderer::from_patch(&osc_patch(0), 48000.0, 256).unwrap();
        let mut split = r.render(1024);
        r.apply(EngineCommand::AddModule { node_id: 99, module_id: "mod.lfo" });
        let second = r.render(1024);
        split.left.extend(second.left);

        assert_eq!(split.left, whole.left);
    }

    #[test]
    fn test_a_loaded_patch_starts_at_its_saved_knobs() {
        // Noise at Level 0 (for an envelope to play) and a Mix input faded
        // down: neither glides in from its default when the patch opens
        let mut patch = Patch::new("faded");
        let mut noise = NodeData::new(1, "source.noise", (0.0, 0.0));
        noise.parameters = vec![NamedParameter::new("Level", ParameterValue::LinearRange(0.0))];
        let mut mixer = NodeData::new(3, "util.mix", (100.0, 0.0));
        mixer.parameters = vec![NamedParameter::new("Level 1", ParameterValue::LinearRange(0.0))];
        patch.nodes.extend([noise, NodeData::new(2, "osc.sine", (0.0, 100.0)), mixer]);
        patch.nodes.push(NodeData::new(4, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "White", 4, "Left"));
        patch.connections.push(ConnectionData::new(2, "Out", 3, "In 1"));
        patch.connections.push(ConnectionData::new(3, "Out", 4, "Right"));

        let (mut r, compiled) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        let out = r.render_seconds(0.05);
        assert_eq!(peak(&out.left), 0.0, "Noise at Level 0 hissed on load");
        assert_eq!(peak(&out.right), 0.0, "a faded Mix input leaked on load");
    }

    #[test]
    fn test_a_modules_noise_follows_its_id_in_the_patch() {
        // A Noise and a snare (hit by a Clock) in a patch, with `extra`
        // modules ahead of them in the file to move their engine IDs along
        let render = |noise_id: u64, extra: u64| {
            let mut patch = Patch::new("hiss");
            patch.nodes.extend((0..extra).map(|n| NodeData::new(100 + n, "osc.sine", (0.0, 0.0))));
            patch.nodes.push(NodeData::new(noise_id, "source.noise", (0.0, 0.0)));
            patch.nodes.push(NodeData::new(200, "util.clock", (0.0, 0.0)));
            let mut drum = NodeData::new(201, "source.drum", (0.0, 0.0));
            drum.parameters = vec![NamedParameter::new("Type", ParameterValue::Select(1))];
            patch.nodes.extend([drum, NodeData::new(202, "output.audio", (0.0, 0.0))]);
            patch.connections.push(ConnectionData::new(noise_id, "White", 202, "Left"));
            patch.connections.push(ConnectionData::new(200, "Gate", 201, "Trig"));
            patch.connections.push(ConnectionData::new(201, "Out", 202, "Right"));
            let (mut r, compiled) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();
            assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
            r.apply(EngineCommand::SetPlaying(true));
            r.render_seconds(0.5)
        };
        let first = render(7, 0);
        assert!(peak(&first.left) > 0.1 && peak(&first.right) > 0.1, "both make noise");

        // However many modules come before them, they play the same noise...
        let numbered_later = render(7, 3);
        assert_eq!(first.left, numbered_later.left);
        assert_eq!(first.right, numbered_later.right);
        // ...and a Noise with another ID plays other noise
        assert_ne!(first.left, render(8, 0).left);
    }

    /// Rising edges through `threshold`, in seconds.
    fn onsets(samples: &[f32], threshold: f32, sample_rate: f32) -> Vec<f32> {
        samples
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < threshold && w[1] >= threshold)
            .map(|(i, _)| (i + 1) as f32 / sample_rate)
            .collect()
    }

    #[test]
    fn test_delay_sync_follows_clock_tempo() {
        // Clock (one short pulse per whole note) -> Delay synced to 1/4,
        // half dry and half wet. Each pulse is followed by its echo exactly
        // one beat later, at whatever tempo the Clock is set to.
        const CLOCK: u64 = 1;
        const DELAY: u64 = 2;
        const OUT: u64 = 3;
        let sr = 48000.0;
        let mut r = OfflineRenderer::new(sr, 256);
        for command in [
            EngineCommand::AddModule { node_id: CLOCK, module_id: "util.clock" },
            EngineCommand::AddModule { node_id: DELAY, module_id: "fx.delay" },
            EngineCommand::AddModule { node_id: OUT, module_id: "output.audio" },
            EngineCommand::Connect { from_node: CLOCK, from_port: 2, to_node: DELAY, to_port: 0 },
            EngineCommand::Connect { from_node: DELAY, from_port: 4, to_node: OUT, to_port: 2 },
            EngineCommand::SetParameter { node_id: CLOCK, param_index: 0, value: 120.0 },
            EngineCommand::SetParameter { node_id: CLOCK, param_index: 1, value: 2.0 }, // gate %
            EngineCommand::SetParameter { node_id: CLOCK, param_index: 2, value: 0.0 }, // whole
            EngineCommand::SetParameter { node_id: DELAY, param_index: 1, value: 0.0 }, // feedback
            EngineCommand::SetParameter { node_id: DELAY, param_index: 6, value: 1.0 }, // 1/4
        ] {
            r.apply(command);
        }

        // 120 BPM: pulses at 0 s and 2 s, echoes half a second after each
        let at_120 = onsets(&r.render_seconds(3.0).left, 0.15, sr);
        assert_eq!(at_120.len(), 4, "pulses and echoes at 120 BPM: {at_120:?}");
        for pair in at_120.chunks(2) {
            let beat = pair[1] - pair[0];
            assert!((beat - 0.5).abs() < 0.002, "echo after {beat} s at 120 BPM");
        }

        // Halfway through the clock's cycle, slow it to 80 BPM: the next pulse
        // comes 1.5 s later, and its echo a beat (0.75 s) after that
        r.apply(EngineCommand::SetParameter { node_id: CLOCK, param_index: 0, value: 80.0 });
        let at_80 = onsets(&r.render_seconds(2.5).left, 0.15, sr);
        assert_eq!(at_80.len(), 2, "pulse and echo at 80 BPM: {at_80:?}");
        let beat = at_80[1] - at_80[0];
        assert!((beat - 0.75).abs() < 0.002, "echo after {beat} s at 80 BPM");
    }

    /// A Clock and an LFO (square, synced to 1/4) playing into the output,
    /// so each beat is a rising edge. The Clock is unpatched: the LFO
    /// follows it through the patch's transport alone.
    fn synced_lfo_patch(r: &mut OfflineRenderer, clock_source: f32) {
        const CLOCK: u64 = 1;
        const LFO: u64 = 2;
        const OUT: u64 = 3;
        for command in [
            EngineCommand::AddModule { node_id: CLOCK, module_id: "util.clock" },
            EngineCommand::AddModule { node_id: LFO, module_id: "mod.lfo" },
            EngineCommand::AddModule { node_id: OUT, module_id: "output.audio" },
            EngineCommand::Connect { from_node: LFO, from_port: 2, to_node: OUT, to_port: 2 },
            EngineCommand::SetParameter { node_id: CLOCK, param_index: 0, value: 120.0 },
            EngineCommand::SetParameter { node_id: CLOCK, param_index: 4, value: clock_source },
            EngineCommand::SetParameter { node_id: LFO, param_index: 0, value: 7.3 }, // ignored while synced
            EngineCommand::SetParameter { node_id: LFO, param_index: 1, value: 2.0 }, // square
            EngineCommand::SetParameter { node_id: LFO, param_index: 4, value: 7.0 }, // 1/4
        ] {
            r.apply(command);
        }
    }

    #[test]
    fn test_synced_lfo_follows_the_clock_through_the_patch() {
        let sr = 48000.0;
        let mut r = OfflineRenderer::new(sr, 256);
        synced_lfo_patch(&mut r, 0.0);

        // A beat every half second, each on its sample, for a minute
        // The downbeat opens the first edge as playing starts, after the
        // output stage's latency. That one rises from silence; the rest
        // rise from -1, through 0 on the beat, so they pass the threshold
        // a sample later: time them from the second
        let edges = onsets(&r.render_seconds(60.0).left, 0.5, sr);
        assert_eq!(edges.len(), 120, "{edges:?}");
        let latency = edges[1] - 0.5;
        for (n, &edge) in edges.iter().enumerate() {
            let beat = n as f32 * 0.5;
            assert!((edge - latency - beat).abs() * sr <= 1.0, "beat {n} at {edge} s");
        }
    }

    #[test]
    fn test_an_arranger_lane_slows_the_clock_that_drives_it() {
        // A ritardando, as the Clock manual's recipe has it: the Clock steps
        // an Arranger, whose lane (through an Attenuverter at log2 0.85)
        // plays back into the Clock's Tempo. Two bars in time, then two
        // ramping on Log to 85% of the tempo. An LFO synced to 1/4 marks
        // each beat of the patch's transport
        let num = |name: &str, value: f32| NamedParameter::new(name, ParameterValue::Number(value));
        let mut patch = Patch::new("rit");
        let mut clock = NodeData::new(1, "util.clock", (0.0, 0.0));
        clock.parameters = vec![num("Tempo", 120.0), NamedParameter::new("Division", ParameterValue::Select(4))];
        let mut arranger = NodeData::new(2, "seq.arranger", (0.0, 0.0));
        arranger.parameters = vec![
            num("Sections", 2.0),
            NamedParameter::new("Loop", ParameterValue::Select(0)),
            num("Length 01", 2.0),
            num("Length 02", 2.0),
            num("Section 01 Lane 1", 1_000_000.0), // Jump to 0
            num("Section 02 Lane 1", 6_001_000.0), // Ramp on Log to 100%
        ];
        let mut rit = NodeData::new(3, "util.attenuverter", (0.0, 0.0));
        rit.parameters = vec![num("Amount", 0.85f32.log2())];
        let mut lfo = NodeData::new(4, "mod.lfo", (0.0, 0.0));
        lfo.parameters = vec![
            NamedParameter::new("Waveform", ParameterValue::Select(2)), // square
            NamedParameter::new("Tempo Sync", ParameterValue::Select(7)), // 1/4
        ];
        patch.nodes.extend([clock, arranger, rit, lfo, NodeData::new(5, "output.audio", (0.0, 0.0))]);
        patch.connections.extend([
            ConnectionData::new(1, "Gate", 2, "Clock"),
            ConnectionData::new(2, "Lane 1", 3, "In"),
            ConnectionData::new(3, "Out", 1, "Tempo"),
            ConnectionData::new(4, "Out", 5, "Mono"),
        ]);
        let sr = 48000.0;
        let (mut r, compiled) = OfflineRenderer::from_patch(&patch, sr, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);

        // A beat every half second for two bars, then each a little longer,
        // never shorter, to 0.5 / 0.85 s by the end of the fourth bar
        let edges = onsets(&r.render_seconds(9.5).left, 0.5, sr);
        let beats: Vec<f32> = edges.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(beats.len() >= 16, "{edges:?}");
        for (n, &beat) in beats[..7].iter().enumerate() {
            assert!((beat - 0.5).abs() * sr <= 1.0, "beat {n} in time: {beat} s");
        }
        for pair in beats[7..16].windows(2) {
            assert!(pair[1] >= pair[0] - 1.0 / sr, "a beat hurried: {pair:?}");
        }
        assert!((beats[15] - 0.5 / 0.85).abs() < 0.005, "the last beat of the rit.: {} s", beats[15]);
    }

    #[test]
    fn test_synced_lfo_follows_a_midi_clock() {
        // A master at 100 BPM, Start then ticks: a tick every 1200 frames.
        // Halfway it speeds up to 130 BPM
        let sr = 48000.0;
        let mut r = OfflineRenderer::new(sr, 256);
        synced_lfo_patch(&mut r, 1.0);
        r.queue_midi(100, 0, MidiMessage::Start);
        let mut tick_frames = Vec::new();
        let mut frame = 1000.0;
        for n in 0..24 * 40 {
            let bpm = if n < 24 * 20 { 100.0 } else { 130.0 };
            tick_frames.push(frame as u64);
            frame += 60.0 * sr as f64 / (bpm * 24.0);
        }
        for &tick in &tick_frames {
            r.queue_midi(tick, 0, MidiMessage::Clock);
        }

        // The LFO waits on the downbeat (its square high) until the first
        // tick, so the first edge is the output stage's latency
        let edges = onsets(&r.render(*tick_frames.last().unwrap() as usize).left, 0.5, sr);
        assert_eq!(edges.len(), 40, "{edges:?}");
        let latency = edges[0];
        // Every beat after it lands with its tick, before and after the
        // tempo change. The first beat after a sudden change can come up to
        // a block late: the LFO paces the block from the tempo it knew at
        // the block's start
        let block = 256.0 / sr;
        for (n, &edge) in edges.iter().enumerate().skip(1) {
            let tick = tick_frames[n * 24] as f32 / sr;
            let allowed = if n == 21 { block } else { 0.0001 };
            assert!((edge - latency - tick).abs() <= allowed, "beat {n} at {edge} s, its tick at {tick} s");
        }
    }

    /// A 1 kHz tone at -12 dBFS from `start` for `length` seconds, in
    /// `seconds` of silence.
    fn tone_burst(sr: f32, seconds: f32, start: f32, length: f32) -> StereoBuffer {
        let left: Vec<f32> = (0..(seconds * sr) as usize)
            .map(|n| {
                let t = n as f32 / sr;
                let on = (start..start + length).contains(&t);
                if on { 0.25 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin() } else { 0.0 }
            })
            .collect();
        StereoBuffer { right: left.iter().map(|s| -s).collect(), left }
    }

    #[test]
    fn test_audio_input_plays_what_it_is_given() {
        let mut patch = Patch::new("input");
        patch.nodes.push(NodeData::new(1, "source.audio_input", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "L", 2, "Left"));
        patch.connections.push(ConnectionData::new(1, "R", 2, "Right"));
        let (mut r, _) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();

        // Silent until given input, as in the renderer with no device
        assert_eq!(peak(&r.render(4800).left), 0.0);

        r.set_audio_input(tone_burst(48000.0, 1.5, 0.2, 1.0));
        let out = r.render_seconds(1.5);
        let heard = &out.left[(0.5 * 48000.0) as usize..(1.0 * 48000.0) as usize];
        // At the output's default volume of 0.8
        let level = rms(heard) / (0.8 * 0.25 / 2f32.sqrt());
        assert!((level - 1.0).abs() < 0.02, "the tone comes through at {level}");
        // Its own right side, not a copy of the left
        let right = &out.right[(0.5 * 48000.0) as usize..(1.0 * 48000.0) as usize];
        assert!(heard.iter().zip(right).all(|(l, r)| (l + r).abs() < 1e-3));
    }

    #[test]
    fn test_two_audio_inputs_each_hear_their_own_channel() {
        // A guitar in input 1 and a voice in input 2, each Audio Input on
        // its own channel, one to each side of the output
        let mut patch = Patch::new("two inputs");
        for (id, channel) in [(1, 1), (2, 2)] {
            let mut input = NodeData::new(id, "source.audio_input", (0.0, 0.0));
            input.parameters = vec![NamedParameter::new("Channel", ParameterValue::Select(channel))];
            patch.nodes.push(input);
        }
        patch.nodes.push(NodeData::new(3, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "R", 3, "Left"));
        patch.connections.push(ConnectionData::new(2, "L", 3, "Right"));
        let (mut r, _) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();

        // Input 1 a tone, input 2 a quieter one an octave down
        let sr = 48000.0;
        let wave = |amplitude: f32, hz: f32| -> Vec<f32> {
            (0..(sr as usize)).map(|n| amplitude * (2.0 * std::f32::consts::PI * hz * n as f32 / sr).sin()).collect()
        };
        r.set_audio_input(StereoBuffer { left: wave(0.25, 1000.0), right: wave(0.125, 500.0) });
        let out = r.render_seconds(1.0);
        let middle = |side: &[f32]| side[(0.5 * sr) as usize..(0.9 * sr) as usize].to_vec();
        let (left, right) = (middle(&out.left), middle(&out.right));
        // Each side has its own input's level, at the output's 0.8 volume
        assert!((rms(&left) / (0.8 * 0.25 / 2f32.sqrt()) - 1.0).abs() < 0.02, "left at {}", rms(&left));
        assert!((rms(&right) / (0.8 * 0.125 / 2f32.sqrt()) - 1.0).abs() < 0.02, "right at {}", rms(&right));
        // And its pitch
        let crossings = |s: &[f32]| s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((crossings(&left) as i32 - 400).abs() <= 1, "left crosses {} times", crossings(&left));
        assert!((crossings(&right) as i32 - 200).abs() <= 1, "right crosses {} times", crossings(&right));
    }

    #[test]
    fn test_audio_input_gate_plays_an_envelope() {
        // A tone burst on the input opens the gate, which plays the
        // oscillator through an envelope and a VCA
        let mut patch = Patch::new("trigger");
        patch.nodes.push(NodeData::new(1, "source.audio_input", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, "mod.adsr", (200.0, 0.0)));
        patch.nodes.push(NodeData::new(3, "osc.sine", (200.0, 200.0)));
        patch.nodes.push(NodeData::new(4, "util.vca", (400.0, 0.0)));
        patch.nodes.push(NodeData::new(5, "output.audio", (600.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Gate", 2, "Gate"));
        patch.connections.push(ConnectionData::new(3, "Out", 4, "In"));
        patch.connections.push(ConnectionData::new(2, "Out", 4, "CV"));
        patch.connections.push(ConnectionData::new(4, "Out", 5, "Mono"));
        let (mut r, _) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();

        r.set_audio_input(tone_burst(48000.0, 3.0, 0.5, 0.5));
        let out = r.render_seconds(3.0);
        let window = |from: f32, to: f32| rms(&out.left[(from * 48000.0) as usize..(to * 48000.0) as usize]);
        assert!(window(0.0, 0.5) < 1e-4, "quiet before the burst");
        assert!(window(0.55, 0.95) > 0.1, "the burst plays the oscillator");
        // The gate closes about 0.4 s after the burst, then the envelope
        // releases
        assert!(window(2.6, 3.0) < 1e-3, "and lets it go after");
    }

    /// Oscillator (saw) into a delay's left input, both delay outputs to the
    /// output module, with the delay bypassed or not.
    fn osc_through_delay(bypassed: bool) -> Patch {
        let mut patch = osc_patch(1);
        patch.connections.clear();
        let mut delay = NodeData::new(3, "fx.delay", (100.0, 0.0));
        delay.parameters = vec![NamedParameter::new("Feedback", ParameterValue::Number(0.9))];
        delay.bypassed = bypassed;
        patch.nodes.push(delay);
        patch.connections.push(ConnectionData::new(1, "Out", 3, "In L"));
        patch.connections.push(ConnectionData::new(3, "Out L", 2, "Left"));
        patch.connections.push(ConnectionData::new(3, "Out R", 2, "Right"));
        patch
    }

    #[test]
    fn test_bypassed_effect_passes_its_input_untouched() {
        let dry = OfflineRenderer::from_patch(&osc_patch(1), 48000.0, 256).unwrap().0.render(4096);

        let (mut r, compiled) = OfflineRenderer::from_patch(&osc_through_delay(true), 48000.0, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        let out = r.render(4096);

        // Only the left input is patched, and bypass normals it to both sides
        // just as the running delay would
        assert_eq!(out.left, dry.left);
        assert_eq!(out.right, dry.left);
    }

    #[test]
    fn test_effect_comes_back_from_bypass_without_old_echoes() {
        let (mut r, compiled) = OfflineRenderer::from_patch(&osc_through_delay(false), 48000.0, 256).unwrap();
        let delay = compiled.node_ids[&3];

        // Fill the delay line with echoes, then bypass and pull the cable
        r.render_seconds(0.5);
        r.apply(EngineCommand::SetBypass { node_id: delay, bypassed: true });
        r.apply(EngineCommand::Disconnect { node_id: delay, port: 0, is_input: true });
        let bypassed = r.render_seconds(0.3);
        let tail = &bypassed.left[bypassed.left.len() - 1024..];
        assert!(peak(tail) < 1e-4, "nothing patched in, nothing passes: {}", peak(tail));

        // Switched back in, the echoes it held when bypassed are gone
        r.apply(EngineCommand::SetBypass { node_id: delay, bypassed: false });
        let back = r.render_seconds(1.0);
        assert!(peak(&back.left) < 1e-6, "old echoes came back: peak {}", peak(&back.left));
    }

    #[test]
    fn test_modules_that_cant_bypass_ignore_it() {
        let whole = OfflineRenderer::from_patch(&osc_patch(0), 48000.0, 256).unwrap().0.render(2048);

        let (mut r, compiled) = OfflineRenderer::from_patch(&osc_patch(0), 48000.0, 256).unwrap();
        r.apply(EngineCommand::SetBypass { node_id: compiled.node_ids[&1], bypassed: true });
        assert_eq!(r.render(2048).left, whole.left);
    }

    #[test]
    fn test_graph_without_output_renders_silence() {
        let mut r = OfflineRenderer::new(48000.0, 128);
        let out = r.render(300);
        assert!(out.left.iter().chain(&out.right).all(|&s| s == 0.0));
    }

    /// A one-voice Note In playing an oscillator (chosen waveform) from
    /// MIDI, through a VCA opened by its gate.
    fn midi_voice_patch(waveform: usize) -> Patch {
        let mut patch = osc_patch(waveform);
        patch.connections.clear();
        patch.nodes.push(note_in(3, Source::Midi));
        patch.nodes.push(NodeData::new(4, "util.vca", (100.0, 0.0)));
        patch.connections.push(ConnectionData::new(3, "Pitch", 1, "V/Oct"));
        patch.connections.push(ConnectionData::new(3, "Gate", 4, "CV"));
        patch.connections.push(ConnectionData::new(1, "Out", 4, "In"));
        patch.connections.push(ConnectionData::new(4, "Out", 2, "Mono"));
        patch
    }

    #[test]
    fn test_midi_sixteenths_land_on_their_samples() {
        // 16th notes at 120 BPM (6000 samples apart), starting mid-block so
        // every note falls inside a block rather than on its edge
        let sr = 48000.0;
        let (mut r, compiled) = OfflineRenderer::from_patch(&midi_voice_patch(2), sr, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        let first = 1037;
        for n in 0..8 {
            let on = first + n * 6000;
            r.queue_midi(on, 0, MidiMessage::NoteOn { note: 60, velocity: 100 });
            r.queue_midi(on + 3000, 0, MidiMessage::NoteOff { note: 60, velocity: 0 });
        }
        let out = r.render(first as usize + 8 * 6000);

        // A note starts where the square wave appears after silence
        let loud = |s: f32| s.abs() > 0.05;
        let onsets: Vec<usize> = (32..out.left.len())
            .filter(|&i| loud(out.left[i]) && out.left[i - 32..i].iter().all(|&s| !loud(s)))
            .collect();
        assert_eq!(onsets.len(), 8, "onsets at {onsets:?}");

        // The output stage's lookahead delays every note equally
        let latency = onsets[0] - first as usize;
        assert!(latency <= 64, "latency {latency}");
        for (n, &onset) in onsets.iter().enumerate() {
            let expected = first as usize + n * 6000 + latency;
            assert!(onset.abs_diff(expected) <= 1, "note {n} at {onset}, expected {expected}");
        }
    }

    /// Note In playing a polyphonic voice: sine oscillator through a VCA
    /// opened by an envelope, one of each per note, summed at the output.
    fn poly_voice_patch() -> Patch {
        let mut patch = osc_patch(0);
        patch.connections.clear();
        patch.nodes.push(NodeData::new(3, note_in::ID, (-200.0, 0.0)));
        let mut env = NodeData::new(4, "mod.adsr", (0.0, 200.0));
        env.parameters = vec![
            NamedParameter::new("Attack", ParameterValue::Number(0.002)),
            NamedParameter::new("Sustain", ParameterValue::Number(1.0)),
            NamedParameter::new("Release", ParameterValue::Number(0.01)),
        ];
        patch.nodes.push(env);
        // Quiet enough that four voices stay clear of the output limiter
        let mut vca = NodeData::new(5, "util.vca", (100.0, 0.0));
        vca.parameters = vec![NamedParameter::new("Level", ParameterValue::Number(0.2))];
        patch.nodes.push(vca);
        patch.connections.push(ConnectionData::new(3, "Pitch", 1, "V/Oct"));
        patch.connections.push(ConnectionData::new(3, "Gate", 4, "Gate"));
        patch.connections.push(ConnectionData::new(4, "Out", 5, "CV"));
        patch.connections.push(ConnectionData::new(1, "Out", 5, "In"));
        patch.connections.push(ConnectionData::new(5, "Out", 2, "Mono"));
        patch
    }

    #[test]
    fn test_poly_chord_plays_independent_voices() {
        let sr = 48000.0;
        let (mut r, compiled) = OfflineRenderer::from_patch(&poly_voice_patch(), sr, 256).unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);

        // A C major chord, its notes let go one at a time
        let chord = [60_u8, 64, 67, 72];
        let quarter = (sr / 4.0) as u64;
        for (n, &note) in chord.iter().enumerate() {
            r.queue_midi(0, 0, MidiMessage::NoteOn { note, velocity: 100 });
            if n < 3 {
                r.queue_midi((n as u64 + 1) * quarter, 0, MidiMessage::NoteOff { note, velocity: 0 });
            }
        }
        let out = r.render_seconds(1.0);

        // Each quarter second, the notes still held ring and the rest are gone
        let hz = |note: u8| 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
        for quarter_index in 0..4 {
            let start = quarter_index * quarter as usize + 2400;
            let spectrum = Spectrum::of(&out.left[start..start + 8192], sr);
            let level = |f: f64| {
                let bin = (f / spectrum.bin_hz).round() as usize;
                spectrum.magnitudes[bin - 2..=bin + 2].iter().copied().fold(0.0, f64::max)
            };
            for (n, &note) in chord.iter().enumerate() {
                let magnitude = level(hz(note));
                if n >= quarter_index {
                    assert!(magnitude > 0.05, "quarter {quarter_index}: note {note} should ring, {magnitude:.4}");
                } else {
                    assert!(magnitude < 0.002, "quarter {quarter_index}: note {note} should be released, {magnitude:.4}");
                }
            }
        }
    }

    #[test]
    fn test_midi_pitch_bend_bends_the_oscillator() {
        let sr = 48000.0;
        let (mut r, _) = OfflineRenderer::from_patch(&midi_voice_patch(0), sr, 256).unwrap();
        // A4, bent fully up: two semitones (the default range) to B4
        r.queue_midi(0, 0, MidiMessage::NoteOn { note: 69, velocity: 100 });
        r.queue_midi(0, 0, MidiMessage::PitchBend { value: 8191 });
        r.render_seconds(0.1);
        let out = r.render(65536);

        let f = Spectrum::of(&out.left, sr).dominant_frequency();
        let b4 = 440.0 * 2f64.powf(2.0 / 12.0);
        let cents = 1200.0 * (f / b4).log2();
        assert!(cents.abs() < 1.0, "expected B4, measured {:.2} Hz ({:+.2} cents)", f, cents);
    }

    #[test]
    fn test_a_saved_loop_waits_and_plays_on_rec() {
        use crate::dsp::SampleData;
        use crate::modules::looper::Looper;
        let folder = std::env::temp_dir().join(format!("soba-loop-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("song loops")).unwrap();
        // Half a second of 330 Hz, saved as a Looper saves it
        let tone: Vec<f32> = (0..24000).map(|n| (std::f32::consts::TAU * 330.0 * n as f32 / 48000.0).sin() * 0.5).collect();
        let wav = crate::persistence::sample_files::encode_wav(&SampleData::mono(tone, 48000.0));
        std::fs::write(folder.join("song loops").join("Looper 2.wav"), wav).unwrap();

        let mut patch = Patch::new("song");
        let mut looper = NodeData::new(2, "util.looper", (0.0, 0.0));
        looper.file = Some("song loops/Looper 2.wav".to_string());
        patch.nodes.push(looper);
        patch.nodes.push(NodeData::new(3, "output.audio", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(2, "Loop L", 3, "Left"));
        patch.connections.push(ConnectionData::new(2, "Loop R", 3, "Right"));
        let (mut r, compiled) = OfflineRenderer::from_patch(&patch, 48000.0, 256).unwrap();
        assert!(r.load_samples(&compiled, SampleBase::Folder(&folder)).is_empty());

        // Silent, Stopped, until the Rec footswitch, then the loop
        let node_id = compiled.node_ids[&2];
        let pedal = |down: bool| EngineCommand::SetParameter { node_id, param_index: Looper::PARAM_PEDALS, value: down as u8 as f32 };
        let out = r.render_with_commands(1.0, vec![(12800, pedal(true)), (16384, pedal(false))]);
        assert_eq!(peak(&out.left[..12800]), 0.0, "it waits");
        assert!(rms(&out.left[16000..]) > 0.2, "rms {}", rms(&out.left[16000..]));
        let hz = Spectrum::of(&out.left[16000..], 48000.0).dominant_frequency();
        assert!((hz - 330.0).abs() < 3.0, "{hz}");
        std::fs::remove_dir_all(&folder).ok();
    }
}
