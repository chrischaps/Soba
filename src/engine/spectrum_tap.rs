//! Spectrum Tap
//!
//! The recent sound going into a module and coming out of it, kept for its
//! display to analyse: a filter's face draws what it's fed and what it lets
//! through under its response curve.
//!
//! The audio thread only copies. After the module runs, each block of its
//! input and output (every channel summed) goes into a ring the UI shares,
//! and the UI takes the newest stretch whenever it draws, FFT and all. The
//! ring is atomics, so neither side ever waits. It holds twice the window
//! the UI reads, so the block being written is never in the half being
//! read, unless the UI stalls mid-copy for longer than the whole window. A
//! read torn that way only marks the window's oldest end, where the
//! analysis window is all but silent anyway.
//!
//! The UI side owns the tap for as long as the node exists, and each plan
//! that taps the node carries another handle. Retired plans are dropped on
//! the UI thread, so the ring is never freed on the audio thread.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::dsp::SignalBuffer;

/// How many samples the UI reads at a time: about 85 ms at 48 kHz, enough
/// to resolve a bass note's fundamental.
pub const WINDOW: usize = 4096;

/// The ring's length: twice the window, so writing and reading keep apart.
const RING: usize = 2 * WINDOW;

/// A module's recent input and output, summed to one channel each.
pub struct SpectrumTap {
    input: Box<[AtomicU32]>,
    output: Box<[AtomicU32]>,
    /// Samples written so far, wrapping. Published after the samples.
    written: AtomicUsize,
    /// The rate they were written at, as f32 bits.
    sample_rate: AtomicU32,
}

impl SpectrumTap {
    pub fn new() -> Self {
        let ring = || (0..RING).map(|_| AtomicU32::new(0)).collect();
        Self {
            input: ring(),
            output: ring(),
            written: AtomicUsize::new(0),
            sample_rate: AtomicU32::new(48_000.0f32.to_bits()),
        }
    }

    /// Adds a block: `input` and `output` with their channels summed.
    ///
    /// REAL-TIME SAFE: stores into the ring, never allocates or waits.
    pub fn write(&self, input: &SignalBuffer, output: &SignalBuffer, sample_rate: f32) {
        let start = self.written.load(Ordering::Relaxed);
        let len = output.len();
        for n in 0..len {
            let at = start.wrapping_add(n) % RING;
            self.input[at].store(sum(input, n).to_bits(), Ordering::Relaxed);
            self.output[at].store(sum(output, n).to_bits(), Ordering::Relaxed);
        }
        self.sample_rate.store(sample_rate.to_bits(), Ordering::Relaxed);
        self.written.store(start.wrapping_add(len), Ordering::Release);
    }

    /// Copies the newest [`WINDOW`] samples of input and output, oldest
    /// first, and returns how many samples had been written by then, so a
    /// reader can tell whether anything new has arrived.
    pub fn read(&self, input: &mut [f32; WINDOW], output: &mut [f32; WINDOW]) -> usize {
        let written = self.written.load(Ordering::Acquire);
        let oldest = written.wrapping_sub(WINDOW);
        for n in 0..WINDOW {
            let at = oldest.wrapping_add(n) % RING;
            input[n] = f32::from_bits(self.input[at].load(Ordering::Relaxed));
            output[n] = f32::from_bits(self.output[at].load(Ordering::Relaxed));
        }
        written
    }

    /// The sample rate of the last block written.
    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate.load(Ordering::Relaxed))
    }
}

impl Default for SpectrumTap {
    fn default() -> Self {
        Self::new()
    }
}

/// Sample `n` of every channel of `buffer`, added together.
#[inline]
fn sum(buffer: &SignalBuffer, n: usize) -> f32 {
    (0..buffer.channels()).map(|channel| buffer.voice(channel).samples.get(n).copied().unwrap_or(0.0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(values: impl Fn(usize) -> f32, len: usize) -> SignalBuffer {
        let mut buffer = SignalBuffer::audio(len);
        for (n, sample) in buffer.channel_mut(0).iter_mut().enumerate() {
            *sample = values(n);
        }
        buffer
    }

    #[test]
    fn reads_the_newest_window_oldest_first() {
        let tap = SpectrumTap::new();
        let mut written = 0;
        // Three windows' worth in uneven blocks, so the ring wraps mid-block
        for len in [700, 1000, 333, 4096, 2048, 3000, 1111] {
            let input = block(|n| (written + n) as f32, len);
            let output = block(|n| -((written + n) as f32), len);
            tap.write(&input, &output, 44_100.0);
            written += len;
        }
        let (mut input, mut output) = ([0.0; WINDOW], [0.0; WINDOW]);
        assert_eq!(tap.read(&mut input, &mut output), written);
        for n in 0..WINDOW {
            let expected = (written - WINDOW + n) as f32;
            assert_eq!(input[n], expected);
            assert_eq!(output[n], -expected);
        }
        assert_eq!(tap.sample_rate(), 44_100.0);
    }

    #[test]
    fn sums_a_chords_voices() {
        let tap = SpectrumTap::new();
        let mut chord = SignalBuffer::polyphonic(WINDOW, crate::dsp::SignalType::Audio);
        chord.set_channels(3);
        for channel in 0..3 {
            chord.channel_mut(channel).fill(0.25 * (channel + 1) as f32);
        }
        let silence = SignalBuffer::audio(WINDOW);
        tap.write(&chord, &silence, 48_000.0);
        let (mut input, mut output) = ([0.0; WINDOW], [0.0; WINDOW]);
        tap.read(&mut input, &mut output);
        assert!(input.iter().all(|&s| (s - 1.5).abs() < 1e-6), "0.25 + 0.5 + 0.75");
        assert!(output.iter().all(|&s| s == 0.0));
    }
}
