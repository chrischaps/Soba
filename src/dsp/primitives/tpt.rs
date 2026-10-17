//! Topology-preserving transform (TPT) building blocks.
//!
//! The TPT approach (Zavalishin, *The Art of VA Filter Design*) discretises
//! an analog block diagram by swapping each integrator for a trapezoidal one
//! and solving the resulting zero-delay feedback loops directly. Filters built
//! this way keep the analog response, stay stable under fast modulation, and
//! can carry nonlinearities in the same places the circuit has them.

use std::f32::consts::PI;

use crate::dsp::denormal::flush;

/// Prewarped integrator gain for a cutoff of `cutoff_hz`: `tan(π·fc/fs)`.
///
/// Prewarping makes the digital filter's cutoff land exactly on `cutoff_hz`.
/// The cutoff is held below 0.49·fs to stay clear of the pole of `tan` at
/// Nyquist.
#[inline]
pub fn prewarp(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let fc = cutoff_hz.clamp(0.0, sample_rate * 0.49);
    (PI * fc / sample_rate).tan()
}

/// A trapezoidal integrator, the building block of TPT filters.
///
/// For an input already scaled by the prewarped gain `g`, each tick outputs
/// `y = s + g·x` and moves the state on to `s = y + g·x`. Inside a filter the
/// loop equation is solved first using [`state`](Self::state), then the
/// integrator is ticked with the solved input.
#[derive(Clone, Copy, Debug, Default)]
pub struct TptIntegrator {
    state: f32,
}

impl TptIntegrator {
    /// Creates an integrator with zero state.
    pub const fn new() -> Self {
        Self { state: 0.0 }
    }

    /// The integrator's state, which is its contribution to the current output
    /// before this sample's input is added.
    #[inline]
    pub fn state(&self) -> f32 {
        self.state
    }

    /// Integrates `x` with gain `g` and returns the output.
    #[inline]
    pub fn tick(&mut self, x: f32, g: f32) -> f32 {
        let v = g * x;
        let y = self.state + v;
        self.state = flush(y + v);
        y
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// A one-pole lowpass, trapezoidal: one [`TptIntegrator`] in a loop, solved
/// with zero delay. Its `gg` is [`gain`](Self::gain) of the prewarped `g`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TptOnePole {
    state: f32,
}

impl TptOnePole {
    /// `g/(1+g)` for a prewarped gain `g`: the loop already solved.
    #[inline]
    pub fn gain(g: f32) -> f32 {
        g / (1.0 + g)
    }

    #[inline]
    pub fn lowpass(&mut self, x: f32, gg: f32) -> f32 {
        let v = (x - self.state) * gg;
        let y = v + self.state;
        self.state = flush(y + v);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prewarp_is_finite_up_to_nyquist() {
        let g = prewarp(1000.0, 48000.0);
        assert!((g - (PI * 1000.0 / 48000.0).tan()).abs() < 1e-7);
        assert!(prewarp(1e6, 48000.0).is_finite());
        assert_eq!(prewarp(-5.0, 48000.0), 0.0);
    }

    #[test]
    fn test_integrator_accumulates_a_constant() {
        // Each tick adds 2g (g is half the per-sample angle): after n ticks y = (2n-1)g
        let mut int = TptIntegrator::new();
        let g = 0.01;
        let mut y = 0.0;
        for _ in 0..100 {
            y = int.tick(1.0, g);
        }
        assert!((y - g * 199.0).abs() < 1e-5, "y = {}", y);
        int.reset();
        assert_eq!(int.state(), 0.0);
    }

    #[test]
    fn test_one_pole_lowpass_from_an_integrator() {
        // y = int(g·(x - y)) solved with zero delay: -3 dB at the prewarped cutoff
        let sr = 48000.0;
        let fc = 2000.0;
        let g = prewarp(fc, sr);
        let mut int = TptIntegrator::new();
        let n = 9600;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let x = (2.0 * PI * fc * i as f32 / sr).sin();
            let v = (x - int.state()) / (1.0 + g);
            out.push(int.tick(v, g));
        }
        let tail = &out[n / 2..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        let gain = rms * 2.0f32.sqrt();
        assert!((gain - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "gain {}", gain);
    }
}
