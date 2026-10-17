//! The state-variable filter: lowpass, bandpass and highpass from one pair of
//! trapezoidal integrators.
//!
//! This is the TPT form (Zavalishin, *The Art of VA Filter Design*) as Andrew
//! Simper writes it out: the zero-delay loop is solved ahead of time into three
//! coefficients, so a sample costs a handful of multiplies. It stays stable
//! however fast the cutoff moves, and its state is flushed every sample, so a
//! tail decays to exactly zero even where [`DenormalGuard`] can't help (the
//! browser build, Apple silicon).
//!
//! [`DenormalGuard`]: crate::dsp::denormal::DenormalGuard

use super::tpt::prewarp;
use crate::dsp::denormal::flush;

/// The coefficients for one cutoff and Q. Cheap to copy; work them out when
/// the cutoff moves, not every sample, since [`new`](Self::new) costs a `tan`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvfCoeffs {
    a1: f32,
    a2: f32,
    a3: f32,
    /// Damping, 1/Q.
    k: f32,
}

impl SvfCoeffs {
    /// A filter at `cutoff_hz` (held below 0.49·fs, see [`prewarp`]) with
    /// resonance `q`: 0.707 is Butterworth, higher rings.
    #[inline]
    pub fn new(cutoff_hz: f32, q: f32, sample_rate: f32) -> Self {
        Self::from_gain(prewarp(cutoff_hz, sample_rate), 1.0 / q)
    }

    /// From an integrator gain `g` already prewarped and a damping `k` (1/Q).
    #[inline]
    pub fn from_gain(g: f32, k: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2, k }
    }
}

/// The three outputs of one step. They sum back to the input:
/// `low + band + high == x`.
#[derive(Clone, Copy, Debug)]
pub struct SvfOut {
    pub low: f32,
    /// Scaled by 1/Q, so it peaks at unity at the cutoff whatever the Q.
    pub band: f32,
    pub high: f32,
}

/// One filter's state: two integrators.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Filters one sample, returning every output.
    #[inline]
    pub fn tick(&mut self, x: f32, c: &SvfCoeffs) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        SvfOut { low: v2, band: c.k * v1, high: x - c.k * v1 - v2 }
    }

    #[inline]
    pub fn low(&mut self, x: f32, c: &SvfCoeffs) -> f32 {
        self.tick(x, c).low
    }

    #[inline]
    pub fn band(&mut self, x: f32, c: &SvfCoeffs) -> f32 {
        self.tick(x, c).band
    }

    #[inline]
    pub fn high(&mut self, x: f32, c: &SvfCoeffs) -> f32 {
        self.tick(x, c).high
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_1_SQRT_2, PI};

    const SR: f32 = 48000.0;

    /// The peak gain of `output` for a sine at `hz`, once it has settled.
    fn gain(hz: f32, c: &SvfCoeffs, output: impl Fn(SvfOut) -> f32) -> f32 {
        let mut f = Svf::default();
        let n = 9600;
        let mut peak = 0.0f32;
        for i in 0..n {
            let y = output(f.tick((2.0 * PI * hz * i as f32 / SR).sin(), c));
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn test_butterworth_is_3db_down_at_the_cutoff() {
        let c = SvfCoeffs::new(1000.0, FRAC_1_SQRT_2, SR);
        assert!((gain(1000.0, &c, |o| o.low) - FRAC_1_SQRT_2).abs() < 0.01);
        assert!((gain(1000.0, &c, |o| o.high) - FRAC_1_SQRT_2).abs() < 0.01);
        assert!(gain(50.0, &c, |o| o.low) > 0.99);
        assert!(gain(50.0, &c, |o| o.high) < 0.01);
    }

    #[test]
    fn test_band_peaks_at_unity_whatever_the_q() {
        for q in [0.5, 2.0, 10.0] {
            let c = SvfCoeffs::new(2000.0, q, SR);
            let g = gain(2000.0, &c, |o| o.band);
            assert!((g - 1.0).abs() < 0.02, "q {} gain {}", q, g);
        }
    }

    #[test]
    fn test_outputs_sum_to_the_input() {
        let c = SvfCoeffs::new(700.0, 3.0, SR);
        let mut f = Svf::default();
        for i in 0..1000 {
            let x = ((i * 7919) % 101) as f32 / 50.0 - 1.0;
            let o = f.tick(x, &c);
            assert!((o.low + o.band + o.high - x).abs() < 1e-5);
        }
    }

    #[test]
    fn test_ringing_decays_to_exact_zero() {
        let c = SvfCoeffs::new(300.0, 20.0, SR);
        let mut f = Svf::default();
        f.tick(1.0, &c);
        for _ in 0..(SR as usize * 10) {
            f.tick(0.0, &c);
        }
        assert_eq!((f.ic1, f.ic2), (0.0, 0.0));
        f.tick(1.0, &c);
        f.reset();
        assert_eq!((f.ic1, f.ic2), (0.0, 0.0));
    }
}
