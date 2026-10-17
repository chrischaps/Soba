//! Audio Processor
//!
//! Handles audio processing in the audio callback: runs the compiled
//! [`GraphPlan`] and applies plans and parameter changes from the UI thread.

use web_time::Instant;

use cpal::{FromSample, Sample};
use rtrb::Consumer;

use crate::dsp::denormal::DenormalGuard;
use crate::dsp::{HostTransport, InputAudio, MidiEvent, ModuleRegistry, Poly, ProcessContext, Snapshot, SnapshotOutcome};
use crate::modules::{AdsrEnvelope, Arranger, Attenuverter, AudioInput, AudioOutput, Chopper, ChordSequencer, Cloud, Chorus, Clock, ClockDivider, Compressor, Dancer, Distortion, Drum, KeyboardInput, LadderFilter, Lfo, LinkOut, Logic, MidiOut, Looper, MidiMonitor, MidiNote, Mix, Mixer, Noise, Oscilloscope, PolyMidi, ParametricEq, Quantizer, Resonator, Reverb, SampleHold, Sampler, Scenes, Oscillator, Slicer, Slope, StepSequencer, StereoDelay, Tape, TriggerSequencer, SvfFilter, Vca, Vocoder};

use super::audio_input::InputFeed;
use super::channels::{EngineHandle, MAX_SNAPSHOTS};
use super::commands::{AudioMessage, CpuLoad, EngineEvent, NodeId, ScopeFrame};
use super::cue::CuePlayer;
use super::probe::ProbePlayer;
use super::graph_plan::GraphPlan;
use super::midi_engine::TimestampedMidiEvent;
use super::midi_scheduler::{take_chunk, MidiScheduler};
use super::recorder::RecordTap;

/// Creates a module registry with all built-in modules.
///
/// Registering a module here is all it takes to make it available in the
/// editor: node templates are generated from the registry. Registration
/// order is the order modules appear in the add-node menu.
///
/// Modules wrapped in [`Poly`] are polyphonic: they run one voice per
/// channel of a polyphonic cable.
pub fn create_module_registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    registry.register::<Poly<Oscillator>>();
    registry.register::<Poly<Noise>>();
    registry.register::<Poly<Drum>>();
    registry.register::<Poly<Resonator>>();
    registry.register::<Sampler>();
    registry.register::<Chopper>();
    registry.register::<Cloud>();
    registry.register::<AudioInput>();
    registry.register::<KeyboardInput>();
    registry.register::<MidiNote>();
    registry.register::<PolyMidi>();
    registry.register::<Poly<SvfFilter>>();
    registry.register::<Poly<LadderFilter>>();
    registry.register::<Poly<AdsrEnvelope>>();
    registry.register::<Lfo>();
    registry.register::<Poly<Slope>>();
    registry.register::<Clock>();
    registry.register::<Poly<Vca>>();
    registry.register::<Poly<Attenuverter>>();
    registry.register::<Mix>();
    registry.register::<Mixer>();
    registry.register::<Poly<SampleHold>>();
    registry.register::<Poly<Quantizer>>();
    registry.register::<ClockDivider>();
    registry.register::<Logic>();
    registry.register::<Looper>();
    registry.register::<Oscilloscope>();
    registry.register::<Dancer>();
    registry.register::<StepSequencer>();
    registry.register::<ChordSequencer>();
    registry.register::<TriggerSequencer>();
    registry.register::<Arranger>();
    registry.register::<Scenes>();
    registry.register::<StereoDelay>();
    registry.register::<Reverb>();
    registry.register::<ParametricEq>();
    registry.register::<Distortion>();
    registry.register::<Chorus>();
    registry.register::<Tape>();
    registry.register::<Slicer>();
    registry.register::<Vocoder>();
    registry.register::<Compressor>();
    registry.register::<MidiMonitor>();
    registry.register::<AudioOutput>();
    registry.register::<LinkOut>();
    registry.register::<MidiOut>();
    registry
}

/// The patch's audio input on the audio thread: the feed from the input
/// device, if one is open, and the block it's read into.
struct LiveInput {
    feed: Option<InputFeed>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl LiveInput {
    fn new(block_size: usize) -> Self {
        Self { feed: None, left: vec![0.0; block_size], right: vec![0.0; block_size] }
    }

    /// Reads the next `frames` frames (at most a block), or silence with no
    /// input open.
    fn next_block(&mut self, frames: usize) -> InputAudio<'_> {
        let Some(feed) = self.feed.as_mut() else {
            return InputAudio::default();
        };
        let frames = frames.min(self.left.len());
        let (left, right) = (&mut self.left[..frames], &mut self.right[..frames]);
        feed.read(left, right);
        InputAudio { left, right }
    }

    /// Drops input that arrived while nothing reads it.
    fn discard(&mut self) {
        if let Some(feed) = self.feed.as_mut() {
            feed.discard();
        }
    }
}

/// Where a render's audio input comes from.
enum RenderInput<'a> {
    /// The input device, a block at a time.
    Live(&'a mut LiveInput),
    /// Audio given for the whole buffer, as when filming.
    Given(InputAudio<'a>),
}

/// The block the app runs its graph in, and `render`'s default. A capture
/// cuts its audio on a grid of these, as `render` does, so the two make the
/// same samples.
pub const BLOCK_SIZE: usize = 256;

/// Audio processor that runs in the audio callback.
///
/// This struct is moved into the audio callback closure and handles
/// all audio processing, including:
/// - Receiving compiled plans and parameter changes from the UI thread
/// - Running the current plan to generate samples
/// - Extracting output from the AudioOutput module
///
/// Nothing here allocates once constructed: graph changes arrive fully
/// built, and replaced plans are handed back to the UI thread to drop.
pub struct AudioProcessor {
    /// The graph currently playing.
    plan: Box<GraphPlan>,
    /// Handle for receiving messages from the UI thread.
    engine_handle: EngineHandle,
    /// Live MIDI input, placed at sample offsets for each callback.
    midi: MidiScheduler,
    /// Current sample rate.
    sample_rate: f32,
    /// Whether audio processing is active.
    is_playing: bool,
    /// Frame counter for throttling CPU load events.
    frame_counter: u32,
    /// Running average of CPU load (0.0-100.0).
    cpu_load_avg: f32,
    /// The slowest callback since the last CPU load event (0.0-100.0+).
    cpu_load_peak: f32,
    /// Where the finished output is copied while recording.
    recorder: Option<RecordTap>,
    /// Audio from the input device, for Audio Input modules.
    input: LiveInput,
    /// Loop snapshots being copied, a piece each callback. Room for
    /// [`MAX_SNAPSHOTS`] is set aside up front.
    snapshots: Vec<(NodeId, Box<Snapshot>)>,
    /// A recording heard from a module's face, beside the patch.
    cue: CuePlayer,
    /// A cable heard on its own, beside the patch.
    probe: Box<ProbePlayer>,
    /// The DAW's transport at the start of the next buffer, in the plugin.
    host: Option<HostTransport>,
}

/// Frames of background copying (loops loaded or snapshotted) per frame a
/// callback plays: about 2% of the callback's time, which copies a
/// two-minute loop in about half a second.
const BACKGROUND_FRAMES_PER_FRAME: usize = 256;

impl AudioProcessor {
    /// Creates a new audio processor.
    ///
    /// # Arguments
    /// * `sample_rate` - The audio sample rate in Hz
    /// * `block_size` - The largest block the graph processes at once; device
    ///   buffers larger than this are processed in several blocks
    /// * `engine_handle` - Handle for receiving messages from the UI
    pub fn new(sample_rate: f32, block_size: usize, engine_handle: EngineHandle) -> Self {
        // Tell the UI side what to prepare new modules for
        engine_handle.set_audio_config(sample_rate, block_size);

        Self {
            plan: Box::new(GraphPlan::empty(block_size)),
            engine_handle,
            midi: MidiScheduler::new(),
            sample_rate,
            is_playing: false,
            frame_counter: 0,
            cpu_load_avg: 0.0,
            cpu_load_peak: 0.0,
            recorder: None,
            input: LiveInput::new(block_size),
            snapshots: Vec::with_capacity(MAX_SNAPSHOTS),
            cue: CuePlayer::default(),
            probe: Box::default(),
            host: None,
        }
    }

    /// In the plugin, the DAW's transport at the start of the next buffer
    /// [`process_offline`](Self::process_offline) is given, for Clocks that
    /// follow the host.
    ///
    /// REAL-TIME SAFE.
    pub fn set_host_transport(&mut self, host: Option<HostTransport>) {
        self.host = host;
    }

    /// Feeds live MIDI from the MIDI engine to the graph. Events are placed
    /// at the sample matching when they arrived, one callback later.
    pub fn set_midi_input(&mut self, input: Consumer<TimestampedMidiEvent>) {
        self.midi.set_input(input);
    }

    /// How often to send CPU load events (in audio callbacks).
    /// At 44100Hz with 256 sample blocks, this is about 172 callbacks/sec.
    /// Sending every 8 callbacks gives ~21Hz update rate.
    const CPU_REPORT_INTERVAL: u32 = 8;

    /// Smoothing factor for CPU load averaging (0-1, higher = more responsive).
    const CPU_SMOOTHING: f32 = 0.3;

    /// Processes into `output` of any sample type, by way of `scratch`: the
    /// same as [`process`](Self::process), converted, for devices that take
    /// integer samples (most ASIO drivers). A callback longer than `scratch`
    /// is processed in parts.
    ///
    /// REAL-TIME SAFE: no allocation, locking or blocking.
    pub fn process_into<T: Sample + FromSample<f32>>(&mut self, output: &mut [T], scratch: &mut [f32], channels: usize) {
        let part = scratch.len() / channels.max(1) * channels.max(1);
        if part == 0 {
            output.fill(T::EQUILIBRIUM);
            return;
        }
        for chunk in output.chunks_mut(part) {
            let rendered = &mut scratch[..chunk.len()];
            self.process(rendered, channels);
            for (out, &sample) in chunk.iter_mut().zip(rendered.iter()) {
                *out = T::from_sample(sample);
            }
        }
    }

    /// Processes a block of audio.
    ///
    /// This is called from the cpal audio callback. It:
    /// 1. Applies pending messages from the UI (new plans, parameters, play/stop)
    /// 2. Places the MIDI that arrived since the last callback in this buffer
    /// 3. If playing, runs the graph, in chunks of at most the plan's block
    ///    size, each with its share of the audio input
    /// 4. Writes the output module's audio to the output buffer
    /// 5. While recording, copies that buffer to the recording
    /// 6. Mixes in the cue, if one is playing, stopped or not, and the
    ///    cable being probed, while playing
    ///
    /// REAL-TIME SAFE: no allocation, locking or blocking.
    ///
    /// # Arguments
    /// * `output` - The output buffer to fill with audio samples
    /// * `channels` - Number of output channels (typically 2 for stereo)
    pub fn process(&mut self, output: &mut [f32], channels: usize) {
        // Decaying filter and reverb tails must not fall into slow denormal
        // arithmetic; restored when the callback returns
        let _denormals = DenormalGuard::new();

        // Start timing for CPU measurement; also the moment MIDI is placed against
        let start_time = Instant::now();

        // Process pending messages from UI
        self.process_messages(output.len() / channels.max(1));

        // Clear output buffer
        output.fill(0.0);

        if !self.is_playing || channels == 0 {
            // Notes played while stopped shouldn't all sound at once on Play,
            // nor should the input arrive late
            self.midi.skip(start_time);
            self.input.discard();
            // Reset CPU load when not playing
            self.cpu_load_avg = 0.0;
            self.cpu_load_peak = 0.0;
            self.record(output, channels);
            self.probe.idle();
            self.play_cue(output, channels);
            return;
        }

        let num_frames = output.len() / channels;
        let midi = self.midi.collect(start_time, num_frames);
        if let Some(feed) = self.input.feed.as_mut() {
            feed.begin(num_frames);
        }
        let latency = if self.input.feed.is_some() { self.engine_handle.input_latency() } else { 0 };
        Self::render(&mut self.plan, &mut self.probe, self.sample_rate, output, channels, midi, RenderInput::Live(&mut self.input), latency, Some(start_time), None);
        self.record(output, channels);
        self.probe.mix(output, channels);
        self.play_cue(output, channels);

        self.plan.publish_monitors();
        self.send_scope_captures();
        self.send_output_level();
        self.send_faults();

        // Calculate CPU load
        let elapsed = start_time.elapsed();
        let available_time = num_frames as f64 / self.sample_rate as f64;
        let cpu_percent = (elapsed.as_secs_f64() / available_time * 100.0) as f32;

        // Smooth the CPU load value using exponential moving average
        self.cpu_load_avg = Self::CPU_SMOOTHING * cpu_percent
            + (1.0 - Self::CPU_SMOOTHING) * self.cpu_load_avg;
        self.cpu_load_peak = self.cpu_load_peak.max(cpu_percent);

        // Send CPU load event at regular intervals (to avoid flooding UI)
        self.frame_counter += 1;
        if self.frame_counter >= Self::CPU_REPORT_INTERVAL {
            self.frame_counter = 0;
            // A full queue would lose the peak: keep it for the next reading
            if self.engine_handle.event_slots_available() > 0 {
                let load = CpuLoad { average: self.cpu_load_avg, peak: self.cpu_load_peak };
                self.engine_handle.send_event_lossy(EngineEvent::CpuLoad(load));
                self.cpu_load_peak = 0.0;
            }
        }
    }

    /// Processes one buffer with MIDI the caller has already placed, instead
    /// of live input, and without a device clock.
    ///
    /// The capture mode renders this way, one video frame's worth of audio
    /// per UI frame, so picture and sound advance in lockstep however long
    /// each frame takes to draw. Live MIDI and audio input that arrived
    /// meanwhile are dropped: Audio Input modules hear `input` instead,
    /// from the start of the buffer.
    pub fn process_offline(&mut self, output: &mut [f32], channels: usize, midi: &mut [MidiEvent], input: InputAudio<'_>) {
        let _denormals = DenormalGuard::new();
        self.process_messages(output.len() / channels.max(1));
        self.midi.skip(Instant::now());
        self.input.discard();
        output.fill(0.0);
        if !self.is_playing || channels == 0 {
            self.record(output, channels);
            self.probe.idle();
            self.play_cue(output, channels);
            return;
        }

        // Input given with the picture is already in time with it
        Self::render(&mut self.plan, &mut self.probe, self.sample_rate, output, channels, midi, RenderInput::Given(input), 0, None, self.host);
        self.record(output, channels);
        self.probe.mix(output, channels);
        self.play_cue(output, channels);

        self.plan.publish_monitors();
        self.send_scope_captures();
        self.send_output_level();
        self.send_faults();
    }

    /// Runs the graph over a buffer in plan-sized blocks, each with the MIDI
    /// that falls inside it and the next block of `input` (`latency` frames
    /// late), and writes the output module's audio into it. The probe
    /// listens to each block as it's made. Played live, the buffer leaves
    /// for the device at `sent_at`, and each block its frames after. In a
    /// DAW, `host` is its transport at the buffer's start.
    #[allow(clippy::too_many_arguments)]
    fn render(
        plan: &mut GraphPlan,
        probe: &mut ProbePlayer,
        sample_rate: f32,
        output: &mut [f32],
        channels: usize,
        mut midi: &mut [MidiEvent],
        mut input: RenderInput<'_>,
        latency: usize,
        sent_at: Option<Instant>,
        host: Option<HostTransport>,
    ) {
        let block = plan.max_block_size().max(1);
        probe.begin(plan);
        for (index, chunk) in output.chunks_mut(block * channels).enumerate() {
            let frames = chunk.len() / channels;
            let start = index * block;
            let chunk_midi = take_chunk(&mut midi, start, start + frames);
            let chunk_input = match &mut input {
                RenderInput::Live(input) => input.next_block(frames),
                RenderInput::Given(given) => given.slice(start, frames),
            };
            let mut context = ProcessContext::new(sample_rate, frames)
                .with_midi(chunk_midi)
                .with_input(chunk_input)
                .with_input_latency(latency)
                .with_host(host.map(|host| HostTransport { beat: host.beat_after(start, sample_rate), ..host }));
            if let Some(at) = sent_at {
                context = context.sent_at(at + std::time::Duration::from_secs_f64(start as f64 / sample_rate as f64));
            }
            plan.process(&context);
            probe.listen(plan, frames, sample_rate);
            Self::write_output(plan, chunk, channels, frames);
        }
    }

    /// Copies a finished buffer, exactly as the device gets it, to the
    /// recording, if one is running.
    fn record(&mut self, output: &[f32], channels: usize) {
        if let Some(tap) = self.recorder.as_mut() {
            tap.write(output, channels);
        }
    }

    /// Mixes the cue into a finished buffer, after the patch's limiter and
    /// after the recording's copy (it's for the player's ears, not the
    /// take), then tells the UI where it is and hands back a recording
    /// it's done with.
    fn play_cue(&mut self, output: &mut [f32], channels: usize) {
        let Self { cue, engine_handle, sample_rate, .. } = self;
        cue.mix(output, channels, *sample_rate);
        if let Some((node_id, place)) = cue.report() {
            engine_handle.send_event_lossy(EngineEvent::Cue { node_id, place });
        }
        while let Some(sample) = cue.take_retired() {
            engine_handle.retire_sample(sample);
        }
    }

    /// Sends oscilloscope captures to the UI thread for waveform display.
    fn send_scope_captures(&mut self) {
        let Self { plan, engine_handle, .. } = self;
        plan.take_scope_captures(|node_id, channel1, channel2, triggered| {
            engine_handle.send_scope_frame_lossy(ScopeFrame::new(node_id, channel1, channel2, triggered));
        });
    }

    /// Sends this callback's output levels to the UI for metering.
    fn send_output_level(&mut self) {
        if let Some(levels) = self.plan.take_output_levels() {
            self.engine_handle.send_event_lossy(EngineEvent::OutputLevel(levels));
        }
    }

    /// Tells the UI which modules made a NaN or infinity this callback.
    fn send_faults(&mut self) {
        let Self { plan, engine_handle, .. } = self;
        plan.take_faults(|node_id| engine_handle.send_event_lossy(EngineEvent::ModuleFault { node_id }));
    }

    /// Applies all pending messages from the UI thread, then does a
    /// callback of `frames` frames' share of background copying.
    fn process_messages(&mut self, frames: usize) {
        while let Some(message) = self.engine_handle.recv_message() {
            match message {
                AudioMessage::InstallPlan(mut plan) => {
                    // Running modules move into the new plan; the old one,
                    // with any removed modules, goes back to be dropped
                    plan.take_over(&mut self.plan);
                    let retired = std::mem::replace(&mut self.plan, plan);
                    self.engine_handle.retire_plan(retired);
                }
                AudioMessage::SetParameter { node_id, param_index, value } => {
                    self.plan.set_parameter(node_id, param_index, value);
                }
                AudioMessage::SetBypass { node_id, bypassed } => {
                    self.plan.set_bypass(node_id, bypassed);
                }
                AudioMessage::Audition { node_id, chord } => {
                    self.plan.audition(node_id, chord);
                }
                AudioMessage::SetPlaying(playing) => {
                    if self.is_playing && !playing {
                        // Clear tails so pressing Play again starts from silence
                        self.plan.reset_modules();
                    }
                    self.is_playing = playing;
                    let event = if playing {
                        EngineEvent::Started
                    } else {
                        EngineEvent::Stopped
                    };
                    self.engine_handle.send_event_lossy(event);
                }
                AudioMessage::StartRecording(tap) => {
                    if let Some(old) = self.recorder.replace(tap) {
                        self.engine_handle.retire_tap(old);
                    }
                }
                AudioMessage::StopRecording => {
                    if let Some(tap) = self.recorder.take() {
                        self.engine_handle.retire_tap(tap);
                    }
                }
                AudioMessage::ConnectInput(feed) => {
                    if let Some(old) = self.input.feed.replace(feed) {
                        self.engine_handle.retire_input(old);
                    }
                }
                AudioMessage::DisconnectInput => {
                    if let Some(feed) = self.input.feed.take() {
                        self.engine_handle.retire_input(feed);
                    }
                }
                AudioMessage::LoadSample { node_id, sample } => {
                    if let Some(old) = self.plan.load_sample(node_id, sample) {
                        self.engine_handle.retire_sample(old);
                    }
                }
                AudioMessage::Cue(cue) => {
                    self.cue.play(cue, self.sample_rate);
                }
                AudioMessage::Probe(probe) => {
                    self.probe.probe(probe);
                }
                AudioMessage::GiveRoom { node_id, room } => {
                    if let Some(old) = self.plan.give_room(node_id, room) {
                        self.engine_handle.retire_sample(old);
                    }
                }
                AudioMessage::SettleTake { node_id, take, sample } => {
                    if let Some(old) = self.plan.settle_take(node_id, take, sample) {
                        self.engine_handle.retire_sample(old);
                    }
                }
                AudioMessage::SetMarkers { node_id, markers } => {
                    self.plan.set_markers(node_id, markers);
                }
                AudioMessage::SnapshotLoop { node_id, mut snapshot } => {
                    if self.snapshots.len() < self.snapshots.capacity() {
                        self.snapshots.push((node_id, snapshot));
                    } else {
                        snapshot.finish(SnapshotOutcome::Busy, 0);
                        self.engine_handle.return_snapshot(node_id, snapshot);
                    }
                }
            }
        }
        // Recordings Samplers have finished fading out of, or let go of
        // when stopped, go back to be freed: before the background work,
        // which may let go of another (a Looper finishing a load), and after
        self.retire_samples();
        self.background_work(frames);
        self.retire_samples();
    }

    /// Hands recordings modules have let go of back to the UI, and tells
    /// it of takes they've just recorded.
    fn retire_samples(&mut self) {
        let Self { plan, engine_handle, .. } = self;
        plan.take_retired_samples(|sample| engine_handle.retire_sample(sample));
        plan.take_finished_takes(|node_id, take| engine_handle.send_take(node_id, take));
    }

    /// Copies the next pieces of loops being loaded and snapshotted, within
    /// the budget of a callback `frames` long, and hands back finished
    /// snapshots. Runs whether or not the transport does.
    ///
    /// REAL-TIME SAFE: copies into room set aside beforehand.
    fn background_work(&mut self, frames: usize) {
        let mut budget = frames * BACKGROUND_FRAMES_PER_FRAME;
        self.plan.background(&mut budget);
        let mut i = 0;
        while i < self.snapshots.len() {
            let (node_id, snapshot) = &mut self.snapshots[i];
            if self.plan.fill_snapshot(*node_id, snapshot, &mut budget) {
                // Order doesn't matter, and this never reallocates
                let (node_id, snapshot) = self.snapshots.swap_remove(i);
                self.engine_handle.return_snapshot(node_id, snapshot);
            } else {
                i += 1;
            }
            if budget == 0 {
                break;
            }
        }
    }

    /// Writes the output module's audio for one block into `output`
    /// (interleaved), duplicating to any channels beyond stereo.
    fn write_output(plan: &GraphPlan, output: &mut [f32], channels: usize, frames: usize) {
        let Some((left, right)) = plan.audio_output() else {
            return;
        };
        Self::interleave(left, right, output, channels, frames);
    }

    /// Lays a stereo pair into the device's frames: a mono device gets the
    /// two mixed, so nothing panned right goes missing, and a surround
    /// device gets the pair in its front two speakers and silence in the
    /// rest, rather than a mono copy that smears the image.
    fn interleave(left: &[f32], right: &[f32], output: &mut [f32], channels: usize, frames: usize) {
        for (i, frame) in output.chunks_mut(channels).take(frames).enumerate() {
            let l = left.get(i).copied().unwrap_or(0.0);
            let r = right.get(i).copied().unwrap_or(0.0);

            match frame {
                [mono] => *mono = (l + r) * 0.5,
                [front_left, front_right, rest @ ..] => {
                    *front_left = l;
                    *front_right = r;
                    rest.fill(0.0);
                }
                [] => {}
            }
        }
    }

    /// Whether an audio input is connected.
    pub fn has_input(&self) -> bool {
        self.input.feed.is_some()
    }

    /// Whether the output is being copied to a recording.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Returns whether audio processing is currently active.
    pub fn is_playing(&self) -> bool {
        self.is_playing
    }

    /// The plan currently playing.
    pub fn plan(&self) -> &GraphPlan {
        &self.plan
    }

    /// Changes the sample rate, re-preparing every module. Only call this
    /// while no audio stream is running the processor.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        // Install anything already compiled for the old rate first, so it
        // gets re-prepared below along with everything else
        self.process_messages(0);

        self.sample_rate = sample_rate;
        self.plan.set_sample_rate(sample_rate);
        self.engine_handle.set_audio_config(sample_rate, self.plan.max_block_size());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineChannels, EngineCommand, UiHandle};

    fn processor() -> (UiHandle, AudioProcessor) {
        let (ui, engine) = EngineChannels::with_defaults().split();
        (ui, AudioProcessor::new(44100.0, 256, engine))
    }

    /// Oscillator into the output module's Mono input, playing.
    fn playing_patch(ui: &mut UiHandle) {
        ui.send_command(EngineCommand::AddModule { node_id: 1, module_id: "osc.sine" });
        ui.send_command(EngineCommand::AddModule { node_id: 2, module_id: "output.audio" });
        ui.send_command(EngineCommand::Connect { from_node: 1, from_port: 5, to_node: 2, to_port: 2 });
        ui.send_command(EngineCommand::SetPlaying(true));
        ui.flush();
    }

    #[test]
    fn test_mono_device_hears_both_sides() {
        let mut output = [9.0; 2];
        AudioProcessor::interleave(&[1.0, 0.0], &[0.0, 0.5], &mut output, 1, 2);
        assert_eq!(output, [0.5, 0.25]);
    }

    #[test]
    fn test_surround_device_gets_the_pair_in_front_and_silence_behind() {
        let mut output = [9.0; 12];
        AudioProcessor::interleave(&[1.0, 0.25], &[-1.0, 0.5], &mut output, 6, 2);
        assert_eq!(output, [1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.5, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_stereo_device_gets_the_pair() {
        let mut output = [9.0; 4];
        AudioProcessor::interleave(&[1.0, 0.25], &[-1.0, 0.5], &mut output, 2, 2);
        assert_eq!(output, [1.0, -1.0, 0.25, 0.5]);
    }

    #[test]
    fn test_create_module_registry() {
        let registry = create_module_registry();
        assert!(registry.contains("osc.sine"));
        assert!(registry.contains("filter.svf"));
        assert!(registry.contains("filter.ladder"));
        assert!(registry.contains("mod.adsr"));
        assert!(registry.contains("util.clock"));
        assert!(registry.contains("util.vca"));
        assert!(registry.contains("util.attenuverter"));
        assert!(registry.contains("output.audio"));
        assert!(registry.contains("mod.lfo"));
        assert!(registry.contains("input.keyboard"));
        assert!(registry.contains("util.midi_monitor"));
        assert!(registry.contains("input.midi_note"));
        assert!(registry.contains("input.poly_midi"));
        assert!(registry.contains("util.sample_hold"));
        assert!(registry.contains("util.oscilloscope"));
        assert!(registry.contains("util.dancer"));
        assert!(registry.contains("output.link"));
        assert!(registry.contains("seq.step"));
        assert!(registry.contains("fx.delay"));
        assert!(registry.contains("fx.reverb"));
        assert!(registry.contains("fx.slicer"));
        assert!(registry.contains("fx.eq"));
        assert!(registry.contains("fx.distortion"));
        assert!(registry.contains("fx.chorus"));
        assert!(registry.contains("fx.tape"));
        assert!(registry.contains("fx.compressor"));
        assert!(registry.contains("util.mix"));
        assert!(registry.contains("util.mixer"));
        assert!(registry.contains("source.noise"));
        assert!(registry.contains("util.quantizer"));
        assert!(registry.contains("util.divider"));
        assert!(registry.contains("util.logic"));
        assert!(registry.contains("source.audio_input"));
        assert!(registry.contains("source.drum"));
        assert!(registry.contains("source.sampler"));
        assert!(registry.contains("seq.trigger"));
        assert!(registry.contains("util.looper"));
        assert!(registry.contains("mod.slope"));
        assert!(registry.contains("seq.arranger"));
        assert!(registry.contains("seq.chord"));
        assert!(registry.contains("fx.vocoder"));
        assert!(registry.contains("source.chopper"));
        assert!(registry.contains("util.scenes"));
        assert!(registry.contains("output.midi"));
        assert!(registry.contains("source.cloud"));
        assert!(registry.contains("source.resonator"));
        assert_eq!(registry.len(), 46);
    }

    #[test]
    fn test_audio_processor_creation() {
        let (_ui, processor) = processor();
        assert!(!processor.is_playing());
    }

    #[test]
    fn test_audio_processor_silence_when_stopped() {
        let (_ui, mut processor) = processor();

        let mut output = vec![1.0; 512]; // Fill with non-zero
        processor.process(&mut output, 2);

        // Should be silence when not playing
        assert!(output.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_audio_processor_responds_to_play_command() {
        let (mut ui, mut processor) = processor();

        // Send play command
        ui.send_command(EngineCommand::SetPlaying(true));

        // Process to handle the command
        let mut output = vec![0.0; 512];
        processor.process(&mut output, 2);

        assert!(processor.is_playing());

        // Check for Started event
        let event = ui.recv_event();
        assert!(matches!(event, Some(EngineEvent::Started)));
    }

    #[test]
    fn test_plays_a_patch_built_on_the_ui_side() {
        let (mut ui, mut processor) = processor();
        playing_patch(&mut ui);

        let mut output = vec![0.0; 1024];
        processor.process(&mut output, 2);

        assert_eq!(processor.plan().len(), 2);
        assert!(output.iter().any(|&s| s.abs() > 0.01), "patch should be audible");
    }

    #[test]
    fn test_a_big_patch_reports_every_module_however_many_callbacks_ran() {
        let (mut ui, mut processor) = processor();
        // More readings each callback than the event queue holds
        let lfos = 1..=200;
        for node_id in lfos.clone() {
            ui.send_command(EngineCommand::AddModule { node_id, module_id: "mod.lfo" });
            ui.send_command(EngineCommand::MonitorOutput { node_id, output_index: 0 });
        }
        ui.send_command(EngineCommand::SetPlaying(true));
        ui.flush();

        // Several callbacks between two UI frames
        let mut output = vec![0.0; 512];
        for _ in 0..8 {
            processor.process(&mut output, 2);
        }
        ui.flush();
        let (mut outputs, mut readouts) = (Vec::new(), Vec::new());
        for event in ui.drain_events() {
            match event {
                EngineEvent::OutputValue { node_id, .. } => outputs.push(node_id),
                EngineEvent::Readout { node_id, .. } => readouts.push(node_id),
                _ => {}
            }
        }
        outputs.sort_unstable();
        readouts.sort_unstable();
        assert_eq!(outputs, lfos.clone().collect::<Vec<_>>(), "every output, once");
        assert_eq!(readouts, lfos.collect::<Vec<_>>(), "every readout, once");
    }

    #[test]
    fn test_device_buffers_larger_than_a_block_are_chunked() {
        let (mut ui, mut processor) = processor();
        playing_patch(&mut ui);

        // 441 frames: one full 256-frame block and a shorter one
        let mut output = vec![0.0; 441 * 2];
        processor.process(&mut output, 2);

        let tail = &output[256 * 2..];
        assert!(tail.iter().any(|&s| s.abs() > 0.01), "second chunk was rendered");
    }

    #[test]
    fn test_process_into_converts_for_integer_devices() {
        // The same patch twice: once as floats, once as an ASIO driver's
        // 32-bit integers, through a scratch buffer that holds less than
        // a callback, so it's rendered in parts
        let (mut ui, mut floats) = processor();
        playing_patch(&mut ui);
        let (mut ui, mut ints) = processor();
        playing_patch(&mut ui);

        let mut expected = vec![0.0f32; 441 * 2];
        floats.process(&mut expected, 2);
        let mut output = vec![0i32; 441 * 2];
        let mut scratch = vec![0.0f32; 100 * 2];
        ints.process_into(&mut output, &mut scratch, 2);

        assert!(output.iter().any(|&s| s.unsigned_abs() > i32::MAX as u32 / 100), "audible");
        for (&int, &float) in output.iter().zip(&expected) {
            assert_eq!(int, i32::from_sample(float));
        }

        // No room even for one frame: silence, not a panic
        ints.process_into(&mut output, &mut [0.0f32; 1], 2);
        assert!(output.iter().all(|&s| s == 0));
    }

    #[test]
    fn test_retired_plans_go_back_to_ui() {
        let (mut ui, mut processor) = processor();
        playing_patch(&mut ui);
        let mut output = vec![0.0; 512];
        processor.process(&mut output, 2);

        // Remove the oscillator: the new plan drops it, the old plan carries
        // it back to the UI thread
        ui.send_command(EngineCommand::RemoveModule { node_id: 1 });
        ui.flush();
        processor.process(&mut output, 2);
        assert_eq!(processor.plan().len(), 1);

        // The output limiter's 1 ms lookahead still holds the last of it, and
        // the DC blocker settles with a slow sub-audio tail. Neither moves
        // like a 261 Hz oscillator (about 0.02 per sample at full scale).
        processor.process(&mut output, 2);
        let left: Vec<f32> = output.iter().step_by(2).copied().collect();
        assert!(left.windows(2).all(|w| (w[1] - w[0]).abs() < 1e-3), "oscillator is gone");
        assert!(ui.flush());
    }

    /// A distinct stereo loop at 48 kHz.
    fn a_loop(frames: usize, hz: f32) -> std::sync::Arc<crate::dsp::SampleData> {
        let left: Vec<f32> = (0..frames).map(|n| (n as f32 * hz / 48000.0 * std::f32::consts::TAU).sin() * 0.5).collect();
        let right: Vec<f32> = left.iter().map(|s| s * -0.5).collect();
        std::sync::Arc::new(crate::dsp::SampleData::stereo(left, right, 48000.0))
    }

    #[test]
    fn test_a_take_comes_back_to_the_ui_and_settling_it_lets_the_room_go() {
        use crate::modules::Sampler;
        use std::sync::Arc;
        let (mut ui, engine) = EngineChannels::with_defaults().split();
        let mut processor = AudioProcessor::new(48000.0, 256, engine);
        let mut output = vec![0.0; 512];
        let mut run = |ui: &mut UiHandle, callbacks: usize| {
            for _ in 0..callbacks {
                ui.flush();
                processor.process(&mut output, 2);
            }
            ui.flush();
        };
        // A sine into a Sampler's In L
        let registry = create_module_registry();
        let port = |module: &str, name: &str| registry.create(module).unwrap().ports().iter().position(|p| p.name == name).unwrap();
        ui.send_command(EngineCommand::AddModule { node_id: 1, module_id: "osc.sine" });
        ui.send_command(EngineCommand::AddModule { node_id: 2, module_id: "source.sampler" });
        ui.send_command(EngineCommand::Connect { from_node: 1, from_port: port("osc.sine", "Out"), to_node: 2, to_port: port("source.sampler", "In L") });
        ui.send_command(EngineCommand::SetPlaying(true));
        run(&mut ui, 1);

        // Room, given to the running Sampler, and Rec pressed twice
        let room = Arc::new(crate::dsp::SampleData::with_room(48000, 48000.0));
        ui.send_command(EngineCommand::GiveRoom { node_id: 2, room });
        run(&mut ui, 1);
        for _ in 0..2 {
            ui.send_command(EngineCommand::SetParameter { node_id: 2, param_index: Sampler::PARAM_REC_BUTTON, value: 1.0 });
            run(&mut ui, 1);
            ui.send_command(EngineCommand::SetParameter { node_id: 2, param_index: Sampler::PARAM_REC_BUTTON, value: 0.0 });
            run(&mut ui, 10);
        }
        let (node_id, crate::dsp::FinishedTake { count: take, sample, beats }) = ui.take_finished_take().expect("the take comes back");
        assert_eq!((node_id, take, beats), (2, 1, None));
        assert_eq!(sample.frames(), 11 * 256, "from one press to the next");
        assert!(sample.left().iter().any(|&s| s.abs() > 0.5), "the sine");

        // Settled with a copy made to measure, the room comes back to be dropped here
        let measured = Arc::new(sample.to_measure());
        ui.send_command(EngineCommand::SettleTake { node_id: 2, take, sample: Arc::clone(&measured) });
        run(&mut ui, 2);
        assert_eq!(Arc::strong_count(&sample), 1, "the room is let go of");
        assert!(Arc::ptr_eq(ui.graph().sample(2).unwrap(), &measured));
    }

    /// Runs callbacks until a snapshot comes back, returning it and how
    /// many callbacks it took.
    fn wait_for_snapshot(ui: &mut UiHandle, processor: &mut AudioProcessor) -> ((NodeId, Box<Snapshot>), usize) {
        let mut output = vec![0.0; 512];
        for callbacks in 1..200 {
            processor.process(&mut output, 2);
            ui.flush();
            if let Some(back) = ui.take_snapshot() {
                return (back, callbacks);
            }
        }
        panic!("the snapshot never came back");
    }

    #[test]
    fn test_a_loop_goes_in_and_comes_back_out_whole_while_stopped() {
        let (mut ui, engine) = EngineChannels::with_defaults().split();
        let mut processor = AudioProcessor::new(48000.0, 256, engine);
        let mut output = vec![0.0; 512];
        // The graph learns the engine's rate
        ui.flush();
        ui.send_command(EngineCommand::AddModule { node_id: 1, module_id: "util.looper" });

        // Given before the Looper reaches the audio thread, it's in at once
        let first = a_loop(100_000, 220.0);
        ui.send_command(EngineCommand::LoadSample { node_id: 1, sample: Some(first.clone()) });
        ui.flush();
        processor.process(&mut output, 2);
        assert!(!processor.is_playing());

        // Out a piece a callback (65 536 frames each here), stopped or not
        assert!(ui.request_snapshot(1, Box::new(Snapshot::with_capacity(100_064))));
        ui.flush();
        let ((node_id, snapshot), callbacks) = wait_for_snapshot(&mut ui, &mut processor);
        assert_eq!(node_id, 1);
        assert_eq!(callbacks, 2);
        assert_eq!(snapshot.edits(), 0);
        assert_eq!(&snapshot.into_sample().unwrap(), first.as_ref(), "bit for bit");
        assert_eq!(ui.snapshots_out(), 0);

        // Given to the running Looper, a loop goes in a piece a callback too,
        // and comes back to be dropped here
        let second = a_loop(150_000, 330.0);
        ui.send_command(EngineCommand::LoadSample { node_id: 1, sample: Some(second.clone()) });
        ui.flush();
        for _ in 0..4 {
            processor.process(&mut output, 2);
        }
        ui.flush();
        assert_eq!(std::sync::Arc::strong_count(&first), 1, "the first loop is let go of");
        assert_eq!(std::sync::Arc::strong_count(&second), 2, "only the graph's record and ours are left");
        ui.request_snapshot(1, Box::new(Snapshot::with_capacity(150_064)));
        ui.flush();
        let ((_, snapshot), _) = wait_for_snapshot(&mut ui, &mut processor);
        assert_eq!(&snapshot.into_sample().unwrap(), second.as_ref());

        // A node that isn't there says so
        ui.request_snapshot(9, Box::new(Snapshot::with_capacity(10)));
        ui.flush();
        let ((_, snapshot), _) = wait_for_snapshot(&mut ui, &mut processor);
        assert_eq!(snapshot.outcome(), SnapshotOutcome::Unsupported);
    }

    #[test]
    fn test_a_cue_plays_with_the_transport_stopped_then_goes_back_to_the_ui() {
        use crate::engine::{Cue, CUE_LEVEL};
        let (mut ui, engine) = EngineChannels::with_defaults().split();
        let mut processor = AudioProcessor::new(48000.0, 256, engine);
        // A rising ramp, 4800 frames: its middle half plays
        let ramp: Vec<f32> = (0..4800).map(|n| n as f32 / 4800.0).collect();
        let sample = std::sync::Arc::new(crate::dsp::SampleData::mono(ramp, 48000.0));
        ui.cue(Some(Cue { node_id: 4, sample: sample.clone(), from: 0.25, to: 0.75, semitones: 0.0 }));

        let mut heard = Vec::new();
        let mut places = Vec::new();
        for _ in 0..12 {
            let mut output = vec![1.0; 512];
            processor.process(&mut output, 2);
            assert!(output.chunks(2).all(|frame| frame[0] == frame[1]), "a mono file on both sides");
            heard.extend(output.iter().step_by(2).copied());
            ui.flush();
            while let Some(event) = ui.recv_event() {
                if let EngineEvent::Cue { node_id: 4, place } = event {
                    places.push(place);
                }
            }
        }
        assert!(!processor.is_playing());
        // The file's samples at the cue's level while it plays, past its fade in...
        for n in [100, 1200, 2200] {
            let expected = (1200 + n) as f32 / 4800.0 * CUE_LEVEL;
            assert!((heard[n] - expected).abs() < 1e-4, "at {n}: {} vs {expected}", heard[n]);
        }
        // ...and silence once it's done
        assert!(heard[2400..].iter().all(|&s| s == 0.0));
        // The UI heard it go by, then end
        assert!(places.windows(2).filter(|w| w[0].is_some() && w[1].is_some()).all(|w| w[1] > w[0]), "{places:?}");
        assert_eq!(places.last(), Some(&None));
        assert_eq!(places.iter().filter(|p| p.is_none()).count(), 1);
        assert_eq!(std::sync::Arc::strong_count(&sample), 1, "freed on the UI side");
    }

    #[test]
    fn test_a_cue_plays_over_the_patch_and_a_stop_stops_it() {
        use crate::engine::Cue;
        let (mut ui, engine) = EngineChannels::with_defaults().split();
        let mut processor = AudioProcessor::new(48000.0, 256, engine);
        let mut patch_alone = vec![0.0; 512];
        playing_patch(&mut ui);
        processor.process(&mut patch_alone, 2);

        // The same patch again, with a cue of DC on top
        let (mut ui, engine) = EngineChannels::with_defaults().split();
        let mut processor = AudioProcessor::new(48000.0, 256, engine);
        playing_patch(&mut ui);
        let dc = std::sync::Arc::new(crate::dsp::SampleData::mono(vec![0.2; 48000], 48000.0));
        ui.cue(Some(Cue { node_id: 1, sample: dc, from: 0.0, to: 1.0, semitones: 0.0 }));
        let mut output = vec![0.0; 512];
        processor.process(&mut output, 2);
        let difference = output[400] - patch_alone[400];
        assert!((difference - 0.1).abs() < 1e-4, "{difference}");

        ui.cue(None);
        processor.process(&mut output, 2);
        processor.process(&mut output, 2);
        assert!(!processor.cue.is_playing());
    }
}
