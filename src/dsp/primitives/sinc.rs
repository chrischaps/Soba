//! Band-limited reading between samples: a Kaiser-windowed sinc.
//!
//! A Hermite curve (see [`hermite`](super::hermite)) is fine for reading a
//! recording at its own rate or slower, but it has no low-pass: stepping
//! through faster than one frame per frame folds everything above the new
//! Nyquist back into the audible band. A sinc kernel scaled to the step
//! removes that band first, so a 96 kHz file converted down to 48 kHz keeps
//! only what 48 kHz can hold, and a recording's octaves (see
//! [`SampleData`](crate::dsp::SampleData)) hold only what half the rate can.
//! Both are made off the audio thread, when a recording is loaded.
//!
//! The kernel is tabulated once, finely enough to read between entries on
//! a straight line, and each tap is weighted from the table. The weights
//! are normalised by their sum, so a constant comes out unchanged at any
//! cutoff and the ripple of a truncated kernel never reaches DC.

use std::sync::LazyLock;

/// Table entries per zero crossing of the kernel.
const RESOLUTION: usize = 512;

/// A windowed sinc, `half_width` zero crossings either side of its centre.
pub struct SincKernel {
    half_width: usize,
    /// The kernel at `i / RESOLUTION` zero crossings from its centre, from
    /// 0 to `half_width` and a zero past it, for the straight-line read.
    table: Vec<f32>,
}

/// For converting a recording to another rate at load, off the audio
/// thread: wide, so the transition band is narrow (about 2.5 kHz at
/// 44.1 kHz) and everything past it is some 80 dB down.
pub static CONVERT: LazyLock<SincKernel> = LazyLock::new(|| SincKernel::new(32, 8.0));

/// For building a recording's octaves at load, off the audio thread.
pub static OCTAVE: LazyLock<SincKernel> = LazyLock::new(|| SincKernel::new(16, 7.0));

impl SincKernel {
    /// A sinc `half_width` zero crossings either side, under a Kaiser
    /// window of shape `beta` (higher: lower sidelobes, wider transition).
    pub fn new(half_width: usize, beta: f64) -> Self {
        let half_width = half_width.max(1);
        let length = half_width * RESOLUTION;
        let i0_beta = bessel_i0(beta);
        let mut table: Vec<f32> = (0..=length)
            .map(|i| {
                let x = i as f64 / RESOLUTION as f64;
                let sinc = if i == 0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let w = x / half_width as f64;
                let window = bessel_i0(beta * (1.0 - w * w).max(0.0).sqrt()) / i0_beta;
                (sinc * window) as f32
            })
            .collect();
        table.push(0.0);
        Self { half_width, table }
    }

    /// Zero crossings either side of its centre.
    pub fn half_width(&self) -> usize {
        self.half_width
    }

    /// The kernel `x` zero crossings from its centre; nothing past its edge.
    ///
    /// REAL-TIME SAFE.
    #[inline]
    fn at(&self, x: f32) -> f32 {
        let f = x.abs() * RESOLUTION as f32;
        let i = f as usize;
        match (self.table.get(i), self.table.get(i + 1)) {
            (Some(&a), Some(&b)) => a + (b - a) * (f - i as f32),
            _ => 0.0,
        }
    }

    /// Reads `left` (and `right`, if there is one) at `position`, in frames,
    /// passing only what's below `cutoff` of their Nyquist (0 to 1). Past
    /// either end it holds the first or last frame, as a Hermite read does.
    ///
    /// REAL-TIME SAFE: `half_width / cutoff` taps either side, each one a
    /// table read.
    #[inline]
    pub fn read(&self, left: &[f32], right: Option<&[f32]>, position: f64, cutoff: f64) -> (f32, f32) {
        let frames = left.len();
        if frames == 0 {
            return (0.0, 0.0);
        }
        let cutoff = cutoff.clamp(1e-3, 1.0);
        let reach = self.half_width as f64 / cutoff;
        let first = (position - reach).ceil() as i64;
        let last = (position + reach).floor() as i64;
        let end = frames as i64 - 1;
        // Tap j is `x0 - j * dx` zero crossings from the centre
        let x0 = ((position - first as f64) * cutoff) as f32;
        let dx = cutoff as f32;
        let (mut l, mut r, mut total) = (0.0f32, 0.0f32, 0.0f32);
        if first >= 0 && last <= end {
            // Clear of both ends: straight through the slices
            let (first, last) = (first as usize, last as usize);
            let lefts = &left[first..=last];
            match right.map(|right| &right[first..=last]) {
                Some(rights) => {
                    for (j, (&a, &b)) in lefts.iter().zip(rights).enumerate() {
                        let w = self.at(x0 - j as f32 * dx);
                        l += a * w;
                        r += b * w;
                        total += w;
                    }
                }
                None => {
                    for (j, &a) in lefts.iter().enumerate() {
                        let w = self.at(x0 - j as f32 * dx);
                        l += a * w;
                        total += w;
                    }
                }
            }
        } else {
            for (j, k) in (first..=last).enumerate() {
                let w = self.at(x0 - j as f32 * dx);
                let i = k.clamp(0, end) as usize;
                l += left[i] * w;
                if let Some(right) = right {
                    r += right[i] * w;
                }
                total += w;
            }
        }
        if total.abs() < 1e-9 {
            return (0.0, 0.0);
        }
        let l = l / total;
        (l, if right.is_some() { r / total } else { l })
    }

    /// `x`, recorded at `from` Hz, read at `to` Hz for `frames` frames,
    /// passing only what's below `cutoff` of its Nyquist: frame `n` is `x`
    /// at frame `n * from / to`. Off the audio thread. Between two whole
    /// rates the reads fall on a few hundred phases at most (160 from
    /// 44.1 kHz to 48 kHz), so each phase's taps are weighed once; other
    /// rates are read frame by frame.
    pub fn resample(&self, x: &[f32], from: f32, to: f32, frames: usize, cutoff: f64) -> Vec<f32> {
        let step = from as f64 / to as f64;
        let whole = |rate: f32| rate.fract() == 0.0 && rate >= 1.0;
        let (num, den) = if whole(from) && whole(to) {
            let (a, b) = (from as u64, to as u64);
            let g = gcd(a, b);
            (a / g, b / g)
        } else {
            (0, 0)
        };
        if den == 0 || den > MAX_PHASES || x.is_empty() {
            return (0..frames).map(|n| self.read(x, None, n as f64 * step, cutoff).0).collect();
        }
        let cutoff = cutoff.clamp(1e-3, 1.0);
        let reach = (self.half_width as f64 / cutoff).floor() as i64;
        // Tap o of phase r is frame `base + o`, `r / den - o` frames from the read
        let taps = (2 * reach + 2) as usize;
        let mut weights = vec![0.0f32; taps * den as usize];
        for (r, phase) in weights.chunks_mut(taps).enumerate() {
            let fraction = r as f64 / den as f64;
            for (j, w) in phase.iter_mut().enumerate() {
                *w = self.at(((fraction - (j as i64 - reach) as f64) * cutoff) as f32);
            }
            let total: f32 = phase.iter().sum();
            phase.iter_mut().for_each(|w| *w /= total);
        }
        let end = x.len() as i64 - 1;
        (0..frames as u64)
            .map(|n| {
                let base = (n * num / den) as i64;
                let phase = &weights[(n * num % den) as usize * taps..][..taps];
                let first = base - reach;
                if first >= 0 && first + taps as i64 <= x.len() as i64 {
                    x[first as usize..][..taps].iter().zip(phase).map(|(a, w)| a * w).sum()
                } else {
                    phase.iter().enumerate().map(|(j, w)| x[(first + j as i64).clamp(0, end) as usize] * w).sum()
                }
            })
            .collect()
    }

    /// `x` filtered to `cutoff` of its Nyquist and cut to every other frame:
    /// frame `m` of what comes back is `x` at frame `2m`. Off the audio
    /// thread. Every frame takes the same taps, so they're weighed once.
    pub fn halve(&self, x: &[f32], cutoff: f64) -> Vec<f32> {
        let frames = x.len().div_ceil(2);
        let cutoff = cutoff.clamp(1e-3, 1.0);
        let reach = (self.half_width as f64 / cutoff).floor() as usize;
        let mut weights: Vec<f32> = (0..=2 * reach).map(|j| self.at(((j as f64 - reach as f64) * cutoff) as f32)).collect();
        let total: f32 = weights.iter().sum();
        weights.iter_mut().for_each(|w| *w /= total);
        let end = x.len() as i64 - 1;
        (0..frames)
            .map(|m| {
                let centre = 2 * m;
                if centre >= reach && centre + reach < x.len() {
                    x[centre - reach..=centre + reach].iter().zip(&weights).map(|(a, w)| a * w).sum()
                } else {
                    weights.iter().enumerate().map(|(j, w)| x[(centre as i64 + j as i64 - reach as i64).clamp(0, end) as usize] * w).sum()
                }
            })
            .collect()
    }
}

/// The most phases [`SincKernel::resample`] weighs ahead.
const MAX_PHASES: u64 = 4096;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The zeroth-order modified Bessel function of the first kind, for the
/// Kaiser window: its power series, to where the terms stop counting.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..64 {
        term *= half / k as f64;
        let squared = term * term;
        sum += squared;
        if squared < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_frames_at_full_band_read_exactly() {
        let x: Vec<f32> = (0..64).map(|n| ((n * 7) % 11) as f32 - 5.0).collect();
        for n in [3usize, 20, 40] {
            let (l, r) = OCTAVE.read(&x, None, n as f64, 1.0);
            assert!((l - x[n]).abs() < 1e-4, "frame {n}: {l}");
            assert_eq!(l, r, "mono reads the same on both sides");
        }
    }

    #[test]
    fn a_constant_stays_constant_at_any_cutoff() {
        let x = vec![0.75f32; 200];
        for cutoff in [1.0, 0.7, 0.5, 0.2] {
            let (l, _) = CONVERT.read(&x, None, 100.37, cutoff);
            assert!((l - 0.75).abs() < 1e-5, "cutoff {cutoff}: {l}");
        }
    }

    #[test]
    fn passes_what_is_below_the_cutoff_and_stops_what_is_above() {
        let rate = 48000.0f32;
        let tone = |hz: f32| -> Vec<f32> { (0..4800).map(|n| (std::f32::consts::TAU * hz * n as f32 / rate).sin()).collect() };
        // Cut at half the Nyquist: 12 kHz
        let rms = |x: &[f32]| ((1000..3800).map(|n| CONVERT.read(x, None, n as f64 + 0.5, 0.5).0.powi(2)).sum::<f32>() / 2800.0).sqrt();
        assert!((rms(&tone(3000.0)) - 0.5f32.sqrt()).abs() < 0.01);
        assert!(rms(&tone(16000.0)) < 1e-3);
    }

    #[test]
    fn halving_matches_reading_every_other_frame() {
        let x: Vec<f32> = (0..300).map(|n| ((n * 13) % 17) as f32 - 8.0).collect();
        let halved = OCTAVE.halve(&x, 0.475);
        assert_eq!(halved.len(), 150);
        for m in [0usize, 3, 75, 149] {
            let read = OCTAVE.read(&x, None, 2.0 * m as f64, 0.475).0;
            assert!((halved[m] - read).abs() < 1e-3, "frame {m}: {} against {read}", halved[m]);
        }
    }

    #[test]
    fn resampling_by_phase_matches_reading_frame_by_frame() {
        let x: Vec<f32> = (0..3000).map(|n| ((n * 13) % 17) as f32 - 8.0).collect();
        for (from, to) in [(44100.0, 48000.0), (96000.0, 48000.0), (48000.0, 44100.0), (44100.5, 48000.0)] {
            let step = from as f64 / to as f64;
            let frames = (3000.0 / step) as usize;
            let cutoff = (1.0 / step).min(1.0) * 0.95;
            let resampled = CONVERT.resample(&x, from, to, frames, cutoff);
            assert_eq!(resampled.len(), frames);
            for n in [0, 1, 7, 160, 1001, frames - 1] {
                let read = CONVERT.read(&x, None, n as f64 * step, cutoff).0;
                assert!((resampled[n] - read).abs() < 2e-3, "{from} to {to}, frame {n}: {} against {read}", resampled[n]);
            }
        }
    }

    #[test]
    fn the_right_side_is_weighted_like_the_left() {
        let left: Vec<f32> = (0..200).map(|n| n as f32).collect();
        let right: Vec<f32> = left.iter().map(|x| -x).collect();
        let (l, r) = OCTAVE.read(&left, Some(&right), 100.25, 0.6);
        assert!((l + r).abs() < 1e-4);
        assert!((l - 100.25).abs() < 0.05, "a straight line stays straight: {l}");
    }
}
