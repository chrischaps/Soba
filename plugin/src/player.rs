//! The plugin's audio thread: the host's buffers through Soba's processor.

use rtrb::Producer;

use soba::dsp::{HostTransport, InputAudio, MidiEvent};
use soba::engine::{AudioProcessor, MidiEvent as WireMidi, TimestampedMidiEvent};

/// The most MIDI events one buffer passes on; more are dropped, rather than
/// growing the list on the audio thread.
pub const MAX_BUFFER_MIDI: usize = 1024;

/// Plays the patch the app builds into the host's buffers.
///
/// REAL-TIME SAFE once made: [`queue`](Self::queue) and
/// [`play`](Self::play) never allocate, lock or block.
pub struct Player {
    processor: AudioProcessor,
    /// A copy of the host's MIDI for the app: its monitor and MIDI Learn.
    ui_midi: Producer<TimestampedMidiEvent>,
    /// The current buffer's MIDI, placed on its samples.
    midi: Vec<MidiEvent>,
    /// A part of the current buffer, interleaved as the processor writes it.
    output: Vec<f32>,
    sample_rate: f32,
}

impl Player {
    /// A player for buffers of up to `max_frames`, whose MIDI the app hears
    /// through `ui_midi`.
    /// The processor must be playing at `sample_rate`.
    pub fn new(processor: AudioProcessor, ui_midi: Producer<TimestampedMidiEvent>, sample_rate: f32, max_frames: usize) -> Self {
        Self { processor, ui_midi, midi: Vec::with_capacity(MAX_BUFFER_MIDI), output: vec![0.0; max_frames.max(1) * 2], sample_rate }
    }

    /// Prepares again, for another rate or buffer size. Not real-time safe.
    pub fn reset(&mut self, sample_rate: f32, max_frames: usize) {
        self.processor.set_sample_rate(sample_rate);
        self.sample_rate = sample_rate;
        self.output = vec![0.0; max_frames.max(1) * 2];
    }

    /// Queues a MIDI message from the host for the next buffer, `timing`
    /// samples into it. Events must come in time order, as hosts send them.
    pub fn queue(&mut self, timing: u32, bytes: &[u8]) {
        let Some(wire) = WireMidi::from_bytes(bytes) else { return };
        if let Some(event) = wire.to_dsp(timing) {
            if self.midi.len() < self.midi.capacity() {
                self.midi.push(event);
            }
        }
        let _ = self.ui_midi.push(TimestampedMidiEvent::now(wire));
    }

    /// Plays the next buffer into `channels` (left and right), `frames`
    /// long, with the MIDI queued for it, while the host's transport is
    /// `host` at its start.
    pub fn play(&mut self, channels: &mut [&mut [f32]], frames: usize, host: Option<HostTransport>) {
        self.processor.set_host_transport(host);
        // A buffer longer than the host promised plays in parts
        let part_frames = self.output.len() / 2;
        let mut done = 0;
        while done < frames {
            let part = (frames - done).min(part_frames);
            let output = &mut self.output[..part * 2];
            let end = self.midi.partition_point(|e| (e.sample_offset as usize) < done + part);
            for event in &mut self.midi[..end] {
                event.sample_offset = event.sample_offset.saturating_sub(done as u32);
            }
            self.processor.process_offline(output, 2, &mut self.midi[..end], InputAudio::default());
            self.midi.drain(..end);
            for (i, frame) in output.as_chunks::<2>().0.iter().enumerate() {
                for (channel, &sample) in channels.iter_mut().zip(frame) {
                    channel[done + i] = sample;
                }
            }
            // The transport moves on with the part
            if let Some(host) = host {
                let beat = host.beat_after(done + part, self.sample_rate);
                self.processor.set_host_transport(Some(HostTransport { beat, ..host }));
            }
            done += part;
        }
        self.midi.clear();
    }
}
