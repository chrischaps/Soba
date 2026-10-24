//! Processing context for DSP modules.
//!
//! Provides runtime information that modules need during audio processing.

use super::note_claims::{NoteClaims, NO_CLAIMS};
use super::signal::MidiEvent;
use web_time::Instant;

/// Transport state information for synchronization.
///
/// Provides tempo and playback state for modules that need to sync
/// to a timeline (e.g., tempo-synced LFOs, sequencers). A patch's transport
/// is its first Clock: see [`DspModule::transport`](super::DspModule::transport).
#[derive(Clone, Copy, Debug, Default)]
pub struct TransportState {
    /// Whether playback is currently active: the beat position is moving.
    pub playing: bool,
    /// Current position in samples from the start.
    pub sample_position: u64,
    /// Current tempo in beats per minute (if available).
    pub tempo_bpm: Option<f32>,
    /// Beats since the transport started, at the first sample of the block,
    /// or `None` with no transport (a patch without a Clock). Beat 0 is the
    /// first downbeat; bars are [`time_sig_numerator`](Self::time_sig_numerator)
    /// beats long.
    pub beat_position: Option<f64>,
    /// Time signature numerator (beats per bar).
    pub time_sig_numerator: u8,
    /// Time signature denominator (beat unit).
    pub time_sig_denominator: u8,
    /// The MIDI input whose clock the transport follows (see
    /// [`MidiEvent::port`](super::MidiEvent::port)), or `None` while it
    /// keeps its own time (#438). Its device isn't sent the clock back.
    pub leader: Option<u8>,
}

impl TransportState {
    /// Creates a new transport state with default values.
    ///
    /// Defaults to stopped, at position 0, with no tempo and 4/4 time.
    pub fn new() -> Self {
        Self {
            playing: false,
            sample_position: 0,
            tempo_bpm: None,
            beat_position: None,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            leader: None,
        }
    }

    /// Creates a transport state that is playing at the given tempo.
    pub fn playing_at(tempo_bpm: f32) -> Self {
        Self {
            playing: true,
            sample_position: 0,
            tempo_bpm: Some(tempo_bpm),
            beat_position: None,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            leader: None,
        }
    }

    /// Returns the current position in beats (if tempo is known).
    pub fn position_in_beats(&self, sample_rate: f32) -> Option<f64> {
        self.tempo_bpm.map(|bpm| {
            let seconds = self.sample_position as f64 / sample_rate as f64;
            let beats_per_second = bpm as f64 / 60.0;
            seconds * beats_per_second
        })
    }

    /// Returns the current position in bars (if tempo is known).
    pub fn position_in_bars(&self, sample_rate: f32) -> Option<f64> {
        self.position_in_beats(sample_rate)
            .map(|beats| beats / self.time_sig_numerator as f64)
    }

    /// The beat position `sample` samples into the block, or `None` with no
    /// transport. It moves on at the tempo while playing and holds while
    /// stopped.
    #[inline]
    pub fn beat_at(&self, sample: usize, sample_rate: f32) -> Option<f64> {
        let start = self.beat_position?;
        if !self.playing {
            return Some(start);
        }
        let beats_per_sample = self.tempo_bpm.unwrap_or(120.0) as f64 / 60.0 / sample_rate as f64;
        Some(start + sample as f64 * beats_per_sample)
    }

    /// Returns the duration of one beat in samples (if tempo is known).
    pub fn samples_per_beat(&self, sample_rate: f32) -> Option<f32> {
        self.tempo_bpm
            .map(|bpm| sample_rate * 60.0 / bpm)
    }
}

/// Live audio from the input device (a microphone, guitar, line source or
/// a whole mixer's worth of instruments) for one block: `block_size`
/// frames, interleaved, of as many channels as the device has (at least
/// two, a mono device's one heard on both), or empty when no input is open,
/// as in the offline renderer. See [`InputChannels`] for picking channels.
///
/// [`InputChannels`]: super::input_channels::InputChannels
#[derive(Clone, Copy, Debug, Default)]
pub struct InputAudio<'a> {
    samples: &'a [f32],
    channels: usize,
}

impl<'a> InputAudio<'a> {
    /// Interleaved frames of `channels` channels.
    pub fn interleaved(samples: &'a [f32], channels: usize) -> Self {
        if channels == 0 {
            return Self::default();
        }
        Self { samples: &samples[..samples.len() / channels * channels], channels }
    }

    /// How many channels each frame holds.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// How many frames it holds.
    pub fn frames(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }

    /// The `frames` frames from `start`, or as many of them as there are.
    pub fn slice(&self, start: usize, frames: usize) -> InputAudio<'a> {
        let start = start.min(self.frames());
        let end = (start + frames).min(self.frames());
        InputAudio { samples: &self.samples[start * self.channels..end * self.channels], channels: self.channels }
    }

    /// The sample of `channel` (from 0) at frame `index`, or silence past
    /// the end or for a channel it hasn't.
    #[inline]
    pub fn sample(&self, index: usize, channel: usize) -> f32 {
        if channel >= self.channels {
            return 0.0;
        }
        self.samples.get(index * self.channels + channel).copied().unwrap_or(0.0)
    }
}

/// Context provided to modules during audio processing.
///
/// Contains all the runtime information a module needs to process audio,
/// including sample rate, buffer size, transport state, and the MIDI that
/// arrived for this block.
#[derive(Clone, Copy, Debug)]
pub struct ProcessContext<'a> {
    /// The audio sample rate in Hz (e.g., 44100, 48000).
    pub sample_rate: f32,
    /// The number of samples in the current processing block.
    pub block_size: usize,
    /// Current transport/timeline state.
    pub transport: TransportState,
    /// Incoming MIDI for this block, in time order. Each event's
    /// `sample_offset` is the sample within the block it lands on.
    pub midi: &'a [MidiEvent],
    /// Audio from the input device for this block.
    pub input: InputAudio<'a>,
    /// How late live input reaches this module, in samples: the round trip
    /// from the input jack to the speakers, when an Audio Input feeds the
    /// module through any chain of modules, and 0 otherwise. A player
    /// playing along with the patch is heard this long after the moment
    /// they played to, which a Looper takes back off its overdubs.
    pub input_latency: usize,
    /// When this block's first sample leaves for the output device: the
    /// audio callback's start, plus the block's place in the callback's
    /// buffer. The device plays it its own delay after that. `None` where
    /// blocks aren't played as they're made (an offline render, a test).
    pub sent_at: Option<Instant>,
    /// The DAW's transport, when Soba runs inside one as a plugin, for a
    /// Clock whose Source is Host. `None` everywhere else.
    pub host: Option<HostTransport>,
    /// Which of the module's outputs anything hears: bit `i` is set when
    /// output `i` (counted among the outputs) is patched to a module, shown
    /// on the module's face or watched by its display. A module may leave an
    /// unheard output silent rather than work it out. Every bit is set
    /// outside a compiled graph, so a module tested on its own works out all
    /// of them. See [`output_heard`](Self::output_heard).
    pub heard_outputs: u64,
    /// The notes Pads modules claim (#428), which a Note In on Omni skips.
    /// None outside a compiled graph.
    pub claimed_notes: &'a NoteClaims,
}

/// The DAW's transport at the first sample of a block, as the plugin's host
/// reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostTransport {
    /// Whether the DAW is playing.
    pub playing: bool,
    /// Its tempo, in beats per minute.
    pub tempo_bpm: f64,
    /// Where it is, in beats (quarter notes) from the start of the song.
    pub beat: f64,
}

impl HostTransport {
    /// The beat `samples` into the block at `sample_rate`.
    #[inline]
    pub fn beat_after(&self, samples: usize, sample_rate: f32) -> f64 {
        if self.playing {
            self.beat + samples as f64 * self.tempo_bpm / 60.0 / sample_rate as f64
        } else {
            self.beat
        }
    }
}

impl<'a> ProcessContext<'a> {
    /// Creates a new process context.
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        Self {
            sample_rate,
            block_size,
            transport: TransportState::new(),
            midi: &[],
            input: InputAudio::default(),
            input_latency: 0,
            sent_at: None,
            host: None,
            heard_outputs: u64::MAX,
            claimed_notes: &NO_CLAIMS,
        }
    }

    /// Creates a process context with transport information.
    pub fn with_transport(sample_rate: f32, block_size: usize, transport: TransportState) -> Self {
        Self {
            sample_rate,
            block_size,
            transport,
            midi: &[],
            input: InputAudio::default(),
            input_latency: 0,
            sent_at: None,
            host: None,
            heard_outputs: u64::MAX,
            claimed_notes: &NO_CLAIMS,
        }
    }

    /// The same context, carrying `midi` as this block's MIDI input.
    pub fn with_midi<'b>(self, midi: &'b [MidiEvent]) -> ProcessContext<'b>
    where
        'a: 'b,
    {
        ProcessContext { midi, ..self }
    }

    /// The same context, with the notes Pads modules claim.
    pub fn with_claims<'b>(self, claimed_notes: &'b NoteClaims) -> ProcessContext<'b>
    where
        'a: 'b,
    {
        ProcessContext { claimed_notes, ..self }
    }

    /// The same context, carrying `input` as this block's audio input.
    pub fn with_input<'b>(self, input: InputAudio<'b>) -> ProcessContext<'b>
    where
        'a: 'b,
    {
        ProcessContext { input, ..self }
    }

    /// The same context, with live input `frames` late (see
    /// [`input_latency`](Self::input_latency)).
    pub fn with_input_latency(self, frames: usize) -> Self {
        Self { input_latency: frames, ..self }
    }

    /// The same context, inside a DAW whose transport is `host`.
    pub fn with_host(self, host: Option<HostTransport>) -> Self {
        Self { host, ..self }
    }

    /// The same context, for a module only the outputs in `mask` of which
    /// are heard (see [`heard_outputs`](Self::heard_outputs)).
    pub fn hearing(self, mask: u64) -> Self {
        Self { heard_outputs: mask, ..self }
    }

    /// Whether anything hears output `index`. Outputs past the 64th always
    /// count as heard.
    #[inline]
    pub fn output_heard(&self, index: usize) -> bool {
        index >= 64 || self.heard_outputs & (1 << index) != 0
    }

    /// The same context, its first sample leaving for the device at `at`.
    pub fn sent_at(self, at: Instant) -> Self {
        Self { sent_at: Some(at), ..self }
    }

    /// Returns the duration of the current block in seconds.
    pub fn block_duration(&self) -> f32 {
        self.block_size as f32 / self.sample_rate
    }

    /// Converts a duration in seconds to samples.
    pub fn seconds_to_samples(&self, seconds: f32) -> usize {
        (seconds * self.sample_rate).round() as usize
    }

    /// Converts a sample count to seconds.
    pub fn samples_to_seconds(&self, samples: usize) -> f32 {
        samples as f32 / self.sample_rate
    }

    /// Converts a frequency in Hz to radians per sample.
    ///
    /// Useful for oscillator phase increments.
    pub fn frequency_to_radians(&self, frequency: f32) -> f32 {
        2.0 * std::f32::consts::PI * frequency / self.sample_rate
    }

    /// Returns the Nyquist frequency (half the sample rate).
    pub fn nyquist(&self) -> f32 {
        self.sample_rate / 2.0
    }
}

impl Default for ProcessContext<'_> {
    fn default() -> Self {
        Self::new(44100.0, 256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_state_default() {
        let transport = TransportState::new();
        assert!(!transport.playing);
        assert_eq!(transport.sample_position, 0);
        assert_eq!(transport.tempo_bpm, None);
        assert_eq!(transport.time_sig_numerator, 4);
        assert_eq!(transport.time_sig_denominator, 4);
    }

    #[test]
    fn test_transport_state_playing() {
        let transport = TransportState::playing_at(120.0);
        assert!(transport.playing);
        assert_eq!(transport.tempo_bpm, Some(120.0));
    }

    #[test]
    fn test_transport_position_in_beats() {
        let mut transport = TransportState::playing_at(120.0); // 2 beats per second
        let sample_rate = 48000.0;

        // At sample 0, position is 0 beats
        assert_eq!(transport.position_in_beats(sample_rate), Some(0.0));

        // After 1 second (48000 samples) at 120 BPM, we should be at beat 2
        transport.sample_position = 48000;
        let beats = transport.position_in_beats(sample_rate).unwrap();
        assert!((beats - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_transport_samples_per_beat() {
        let transport = TransportState::playing_at(120.0);
        let sample_rate = 48000.0;

        // 120 BPM = 2 beats/sec, so 48000/2 = 24000 samples/beat
        let spb = transport.samples_per_beat(sample_rate).unwrap();
        assert!((spb - 24000.0).abs() < 0.1);
    }

    #[test]
    fn test_beat_at_moves_only_while_playing() {
        let mut transport = TransportState::playing_at(120.0);
        assert_eq!(transport.beat_at(100, 48000.0), None);

        transport.beat_position = Some(3.0);
        // 120 BPM at 48 kHz: a beat every 24000 samples
        assert!((transport.beat_at(12000, 48000.0).unwrap() - 3.5).abs() < 1e-12);

        transport.playing = false;
        assert_eq!(transport.beat_at(12000, 48000.0), Some(3.0));
    }

    #[test]
    fn test_process_context_creation() {
        let ctx = ProcessContext::new(44100.0, 256);
        assert_eq!(ctx.sample_rate, 44100.0);
        assert_eq!(ctx.block_size, 256);
    }

    #[test]
    fn test_process_context_block_duration() {
        let ctx = ProcessContext::new(44100.0, 441);
        // 441 samples at 44100 Hz = 0.01 seconds = 10ms
        assert!((ctx.block_duration() - 0.01).abs() < 0.0001);
    }

    #[test]
    fn test_process_context_time_conversions() {
        let ctx = ProcessContext::new(48000.0, 256);

        // 1 second = 48000 samples
        assert_eq!(ctx.seconds_to_samples(1.0), 48000);

        // 48000 samples = 1 second
        assert!((ctx.samples_to_seconds(48000) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_process_context_frequency_to_radians() {
        let ctx = ProcessContext::new(48000.0, 256);

        // 1 Hz = 2π/48000 radians per sample
        let rad = ctx.frequency_to_radians(1.0);
        let expected = 2.0 * std::f32::consts::PI / 48000.0;
        assert!((rad - expected).abs() < 0.0000001);
    }

    #[test]
    fn test_process_context_nyquist() {
        let ctx = ProcessContext::new(44100.0, 256);
        assert_eq!(ctx.nyquist(), 22050.0);
    }

    #[test]
    fn test_process_context_default() {
        let ctx = ProcessContext::default();
        assert!(ctx.output_heard(0) && ctx.output_heard(63) && ctx.output_heard(64));
        assert_eq!(ctx.sample_rate, 44100.0);
        assert_eq!(ctx.block_size, 256);
    }
}
