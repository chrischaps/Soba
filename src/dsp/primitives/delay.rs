//! A delay line read at fractional positions.

use crate::dsp::denormal::flush;

/// A delay line with a power-of-two ring buffer and cubic (Catmull-Rom) reads.
///
/// Reading between samples lets a delay time glide (for modulation, or a
/// Size knob) without the clicks of jumping from one whole sample to the next.
/// Cubic interpolation keeps the top octave that linear interpolation would
/// dull, which matters inside a feedback loop where every pass compounds it.
#[derive(Clone, Debug, Default)]
pub struct FracDelay {
    buffer: Vec<f32>,
    mask: usize,
    write: usize,
}

impl FracDelay {
    /// A delay line that can be read up to `max_delay` samples back.
    ///
    /// Allocates; call from `prepare`, never on the audio thread.
    pub fn new(max_delay: usize) -> Self {
        let mut line = Self::default();
        line.allocate(max_delay);
        line
    }

    /// Reallocates for a new maximum delay and clears the line.
    pub fn allocate(&mut self, max_delay: usize) {
        // Room for the delay, the interpolator's taps either side, and the write
        let len = (max_delay + 4).next_power_of_two();
        self.buffer = vec![0.0; len];
        self.mask = len - 1;
        self.write = 0;
    }

    /// The longest delay that can be read, in samples.
    pub fn max_delay(&self) -> f32 {
        self.buffer.len().saturating_sub(4) as f32
    }

    /// Appends a sample to the line.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buffer[self.write] = flush(x);
        self.write = (self.write + 1) & self.mask;
    }

    /// The sample pushed `delay` pushes ago (0 is the most recent), read
    /// between samples with a Catmull-Rom spline. `delay` is clamped to
    /// 1..=max_delay so all four taps hold real history, and a NaN delay
    /// reads as 1.
    #[inline]
    pub fn read(&self, delay: f32) -> f32 {
        // max and min, not clamp: clamp passes NaN through, and a NaN
        // delay would read from before the buffer
        let delay = delay.max(1.0).min(self.max_delay());
        let whole = delay as usize;
        let t = delay - whole as f32;

        // Taps from newest to oldest; we interpolate between p1 and p2
        let newest = self.write.wrapping_sub(1);
        let at = |back: usize| self.buffer[newest.wrapping_sub(back) & self.mask];
        let p0 = at(whole - 1);
        let p1 = at(whole);
        let p2 = at(whole + 1);
        let p3 = at(whole + 2);

        let c1 = 0.5 * (p2 - p0);
        let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);
        ((c3 * t + c2) * t + c1) * t + p1
    }

    /// Silences the line.
    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whole_sample_reads_are_exact() {
        let mut line = FracDelay::new(64);
        for i in 0..100 {
            line.push(i as f32);
        }
        assert_eq!(line.read(1.0), 98.0);
        assert_eq!(line.read(10.0), 89.0);
        assert_eq!(line.read(64.0), 35.0);
    }

    #[test]
    fn test_fractional_reads_follow_a_ramp() {
        // A cubic spline reproduces a straight line exactly
        let mut line = FracDelay::new(64);
        for i in 0..100 {
            line.push(i as f32 * 0.5);
        }
        assert!((line.read(10.25) - (99.0 - 10.25) * 0.5).abs() < 1e-4);
        assert!((line.read(3.75) - (99.0 - 3.75) * 0.5).abs() < 1e-4);
    }

    #[test]
    fn test_a_nan_delay_reads_one_sample_back() {
        let mut line = FracDelay::new(16);
        for i in 0..32 {
            line.push(i as f32);
        }
        assert_eq!(line.read(f32::NAN), 30.0);
    }

    #[test]
    fn test_delay_is_clamped_to_the_buffer() {
        let mut line = FracDelay::new(16);
        for i in 0..64 {
            line.push(i as f32);
        }
        let max = line.max_delay();
        assert!(max >= 16.0);
        assert_eq!(line.read(1e9), line.read(max));
        assert_eq!(line.read(0.0), line.read(1.0));
    }
}
