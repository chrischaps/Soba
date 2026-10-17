//! One-pole smoothing, and the envelope follower built from it.
//!
//! One convention throughout: a time is the time constant τ, the time a step
//! takes to cover 63% (1 - 1/e) of the way to its target. A one-pole with
//! constant τ moves each sample by [`rate_for_seconds`] of the gap that is
//! left, which is the same as keeping [`decay_for_seconds`] of it.

use crate::dsp::denormal::flush;

/// The fraction of the gap a one-pole with time constant `seconds` keeps each
/// sample: `exp(-1 / (τ·fs))`. Multiply a decaying level by this every sample.
/// Times shorter than a microsecond count as one, so zero is a jump, not NaN.
#[inline]
pub fn decay_for_seconds(seconds: f32, sample_rate: f32) -> f32 {
    (-1.0 / (seconds.max(1e-6) * sample_rate)).exp()
}

/// The fraction of the gap a one-pole with time constant `seconds` closes each
/// sample: `1 - exp(-1 / (τ·fs))`. The `rate` of [`OnePole::follow`].
#[inline]
pub fn rate_for_seconds(seconds: f32, sample_rate: f32) -> f32 {
    1.0 - decay_for_seconds(seconds, sample_rate)
}

/// A one-pole lowpass that follows its target at a given rate.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    pub level: f32,
}

impl OnePole {
    /// Moves `rate` of the way to `target` and returns where it got to.
    #[inline]
    pub fn follow(&mut self, target: f32, rate: f32) -> f32 {
        self.level = flush(self.level + rate * (target - self.level));
        self.level
    }
}

/// An envelope follower: a rectifier into a one-pole that rises at one rate
/// and falls at another, as on an analog vocoder's band.
#[derive(Clone, Copy, Debug, Default)]
pub struct Follower {
    pub level: f32,
}

impl Follower {
    /// Follows `|x|`, at `attack` while it is above the level and `release`
    /// while it is below (both rates, from [`rate_for_seconds`]).
    #[inline]
    pub fn follow(&mut self, x: f32, attack: f32, release: f32) -> f32 {
        let x = x.abs();
        let rate = if x > self.level { attack } else { release };
        self.level = flush(self.level + rate * (x - self.level));
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_step_covers_63_percent_in_one_time_constant() {
        let sr = 48000.0;
        let rate = rate_for_seconds(0.01, sr);
        let mut p = OnePole::default();
        for _ in 0..480 {
            p.follow(1.0, rate);
        }
        assert!((p.level - (1.0 - (-1.0f32).exp())).abs() < 1e-3, "level {}", p.level);
        assert!((decay_for_seconds(0.01, sr) + rate - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_follower_rises_fast_and_falls_slowly() {
        let mut f = Follower::default();
        f.follow(-1.0, 0.5, 0.01);
        assert_eq!(f.level, 0.5);
        f.follow(0.0, 0.5, 0.01);
        assert!((f.level - 0.495).abs() < 1e-6);
        for _ in 0..100_000 {
            f.follow(0.0, 0.5, 0.01);
        }
        assert_eq!(f.level, 0.0, "the tail lands on zero");
    }
}
