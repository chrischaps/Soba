//! First-order antiderivative anti-aliasing (ADAA) for static waveshapers.
//!
//! A waveshaper `f` applied sample by sample turns a smooth input into a
//! curve with corners, and the corners' harmonics run far past Nyquist and
//! fold back. ADAA (Parker, Zavalishin and Le Bivic, 2016) instead outputs
//! the *average* of `f` over the straight line between consecutive inputs:
//!
//! ```text
//! y[n] = (F(x[n]) - F(x[n-1])) / (x[n] - x[n-1])
//! ```
//!
//! where `F` is the antiderivative of `f`. Averaging over each step is a
//! continuous-time boxcar filter applied before sampling, which knocks the
//! high harmonics down by about 6 dB/oct, and costs one evaluation of `F`
//! per sample. It adds half a sample of delay and a gentle top-end droop,
//! both negligible when it runs oversampled.
//!
//! Everything is evaluated in f64: `F` grows like `|x|`, and its difference
//! over a tiny step would otherwise lose all its precision to cancellation.

use std::f64::consts::LN_2;

/// A static waveshaping curve with a closed-form antiderivative.
pub trait Curve {
    /// The curve, `f(x)`.
    fn value(&self, x: f64) -> f64;
    /// Any antiderivative of the curve, `F(x)` with `F' = f`.
    fn antiderivative(&self, x: f64) -> f64;
}

/// `ln(cosh(x))`, the antiderivative of `tanh`, without overflowing.
#[inline]
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - LN_2
}

/// Symmetric soft saturation, `tanh(x)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tanh;

impl Curve for Tanh {
    #[inline]
    fn value(&self, x: f64) -> f64 {
        x.tanh()
    }

    #[inline]
    fn antiderivative(&self, x: f64) -> f64 {
        ln_cosh(x)
    }
}

/// Hard clipping at ±1.
#[derive(Clone, Copy, Debug, Default)]
pub struct HardClip;

impl Curve for HardClip {
    #[inline]
    fn value(&self, x: f64) -> f64 {
        x.clamp(-1.0, 1.0)
    }

    #[inline]
    fn antiderivative(&self, x: f64) -> f64 {
        if x.abs() <= 1.0 {
            0.5 * x * x
        } else {
            x.abs() - 0.5
        }
    }
}

/// A `tanh` biased off centre, like a triode run from its operating point:
/// `n·(tanh(x + b) − tanh(b))`.
///
/// Zero in still gives zero out, the slope at zero is normalised to 1, and
/// the two halves saturate at different levels, so the curve makes even
/// harmonics from the first decibel of drive, not only odd ones. The
/// rectified signal carries a DC offset that follows its level; follow the
/// curve with a DC blocker.
#[derive(Clone, Copy, Debug)]
pub struct BiasedTanh {
    bias: f64,
    tanh_bias: f64,
    /// `cosh²(b)`, the reciprocal of the slope at zero.
    norm: f64,
}

impl BiasedTanh {
    pub fn new(bias: f64) -> Self {
        Self {
            bias,
            tanh_bias: bias.tanh(),
            norm: bias.cosh().powi(2),
        }
    }
}

impl Curve for BiasedTanh {
    #[inline]
    fn value(&self, x: f64) -> f64 {
        self.norm * ((x + self.bias).tanh() - self.tanh_bias)
    }

    #[inline]
    fn antiderivative(&self, x: f64) -> f64 {
        self.norm * (ln_cosh(x + self.bias) - x * self.tanh_bias)
    }
}

/// A wavefolder: past ±1 the signal reflects back toward zero instead of
/// clipping, then past ∓1 again, and so on, so each extra step of drive adds
/// another fold and another pair of peaks to the wave.
///
/// The ideal folder is a triangle function of its input, whose sharp
/// corners alias badly. Real folders (the Serge and Buchla circuits) round
/// those corners because diodes and transistors turn on gradually; here each
/// corner is replaced by a parabola over `±round` of the fold point, which
/// keeps the slope continuous. The output is rescaled so the rounded peaks
/// still reach ±1.
#[derive(Clone, Copy, Debug)]
pub struct RoundedFolder {
    round: f64,
    /// `1 / (1 − round/2)`: the rounded peak height, inverted.
    norm: f64,
    /// `Q(round)`, the integral of one segment's curved start.
    q_round: f64,
}

impl RoundedFolder {
    /// Creates a folder whose corners are rounded over `round` (0 < round ≤ 1)
    /// either side of each fold point.
    pub fn new(round: f64) -> Self {
        let round = round.clamp(1e-3, 1.0);
        Self {
            round,
            norm: 1.0 / (1.0 - 0.5 * round),
            q_round: round - 2.0 * round * round / 3.0,
        }
    }

    /// Maps `x` to its place in the fold's period of 4: `s` in [-2, 2), with
    /// the positive peak at 0 and the negative peaks at ±2.
    #[inline]
    fn phase(x: f64) -> f64 {
        (x + 1.0).rem_euclid(4.0) - 2.0
    }

    /// The (unscaled) shape at distance `a` (0..=2) from a positive peak:
    /// a straight fall from +1 to −1 with both ends rounded.
    #[inline]
    fn shape(&self, a: f64) -> f64 {
        let w = self.round;
        if a < w {
            1.0 - 0.5 * w - a * a / (2.0 * w)
        } else if a > 2.0 - w {
            let b = 2.0 - a;
            -1.0 + 0.5 * w + b * b / (2.0 * w)
        } else {
            1.0 - a
        }
    }

    /// `∫₀ᵃ shape`. Zero at both 0 and 2, since the shape is antisymmetric
    /// about its midpoint.
    #[inline]
    fn shape_integral(&self, a: f64) -> f64 {
        let w = self.round;
        if a <= w {
            (1.0 - 0.5 * w) * a - a * a * a / (6.0 * w)
        } else if a <= 2.0 - w {
            self.q_round + (a - 0.5 * a * a) - (w - 0.5 * w * w)
        } else {
            let b = 2.0 - a;
            (1.0 - 0.5 * w) * b - b * b * b / (6.0 * w)
        }
    }
}

impl Curve for RoundedFolder {
    #[inline]
    fn value(&self, x: f64) -> f64 {
        self.norm * self.shape(Self::phase(x).abs())
    }

    #[inline]
    fn antiderivative(&self, x: f64) -> f64 {
        // The shape is even in s, so its integral is odd. It is periodic too:
        // each period's positive and negative lobes cancel.
        let s = Self::phase(x);
        self.norm * s.signum() * self.shape_integral(s.abs())
    }
}

/// Steps closer than this fall back to evaluating the curve at the midpoint.
const ADAA_EPSILON: f64 = 1e-6;

/// First-order ADAA state for one channel: the previous input and its
/// antiderivative.
///
/// A fresh (or reset) state starts from an input of 0, with the
/// antiderivative there taken from the first curve it shapes. Assuming
/// `F(0) = 0` instead would be wrong for a curve like [`BiasedTanh`], whose
/// `F(0)` isn't zero: the first step would divide that by a tiny `dx`, and
/// an inaudibly quiet first sample would come out as a full-scale click.
#[derive(Clone, Copy, Debug, Default)]
pub struct Adaa1 {
    prev_x: f64,
    prev_big_f: f64,
    /// Whether `prev_big_f` has been taken from a curve yet.
    primed: bool,
}

impl Adaa1 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shapes one sample through `curve`.
    #[inline]
    pub fn process<C: Curve>(&mut self, curve: &C, x: f32) -> f32 {
        let x = x as f64;
        if !self.primed {
            self.prime(curve, 0.0);
        }
        let big_f = curve.antiderivative(x);
        let dx = x - self.prev_x;
        let y = if dx.abs() > ADAA_EPSILON {
            (big_f - self.prev_big_f) / dx
        } else {
            curve.value(0.5 * (x + self.prev_x))
        };
        self.prev_x = x;
        self.prev_big_f = big_f;
        y as f32
    }

    /// Restarts from input `x` under `curve`, as if it had been the last
    /// sample. Call when switching curves, so the cached antiderivative
    /// belongs to the curve that will use it.
    pub fn prime<C: Curve>(&mut self, curve: &C, x: f32) {
        self.prev_x = x as f64;
        self.prev_big_f = curve.antiderivative(self.prev_x);
        self.primed = true;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::Spectrum;
    use std::f64::consts::PI;

    /// `F' = f` everywhere, checked by central differences.
    fn check_antiderivative<C: Curve>(curve: &C, name: &str) {
        let h = 1e-5;
        for i in -2400..=2400 {
            let x = i as f64 * 0.005 + 0.0013;
            let slope = (curve.antiderivative(x + h) - curve.antiderivative(x - h)) / (2.0 * h);
            let err = (slope - curve.value(x)).abs();
            assert!(err < 1e-5, "{}: F'({}) = {} but f = {}", name, x, slope, curve.value(x));
        }
    }

    #[test]
    fn test_antiderivatives_are_exact() {
        check_antiderivative(&Tanh, "tanh");
        check_antiderivative(&HardClip, "hard clip");
        check_antiderivative(&BiasedTanh::new(0.5), "biased tanh");
        check_antiderivative(&RoundedFolder::new(0.4), "folder");
        check_antiderivative(&RoundedFolder::new(1.0), "round folder");
    }

    #[test]
    fn test_fresh_state_starts_quietly_on_any_curve() {
        // BiasedTanh's antiderivative isn't zero at zero, so a fresh state
        // that assumed it was would turn a whisper into a click
        let curve = BiasedTanh::new(0.5);
        assert!(curve.antiderivative(0.0).abs() > 0.1);
        for mut adaa in [Adaa1::new(), {
            let mut used = Adaa1::new();
            used.process(&curve, 0.8);
            used.reset();
            used
        }] {
            let y = adaa.process(&curve, 1e-4);
            assert!(y.abs() < 1e-3, "first sample {}", y);
        }
    }

    #[test]
    fn test_ln_cosh_survives_large_inputs() {
        assert!((ln_cosh(800.0) - (800.0 - LN_2)).abs() < 1e-9);
        assert!(ln_cosh(0.0).abs() < 1e-15);
    }

    #[test]
    fn test_biased_tanh_is_asymmetric_with_unit_slope() {
        let curve = BiasedTanh::new(0.5);
        assert!(curve.value(0.0).abs() < 1e-15);
        assert!((curve.value(1e-6) / 1e-6 - 1.0).abs() < 1e-5);
        // Saturates softer on one side than the other
        let (pos, neg) = (curve.value(20.0), -curve.value(-20.0));
        assert!(pos < 0.75 && neg > 1.8, "ceilings +{} / -{}", pos, neg);
    }

    #[test]
    fn test_folder_folds() {
        let curve = RoundedFolder::new(0.4);
        // Unity-ish slope through zero, peaks at ±1, and folds past them
        assert!(curve.value(0.0).abs() < 1e-12);
        assert!((curve.value(1.0) - 1.0).abs() < 1e-12);
        assert!((curve.value(-1.0) + 1.0).abs() < 1e-12);
        assert!(curve.value(2.0).abs() < 1e-12, "{}", curve.value(2.0));
        assert!((curve.value(3.0) + 1.0).abs() < 1e-12);
        assert!(curve.value(1.5) < curve.value(1.0));
        // Bounded and odd
        for i in -1000..=1000 {
            let x = i as f64 * 0.013;
            assert!(curve.value(x).abs() <= 1.0 + 1e-12);
            assert!((curve.value(x) + curve.value(-x)).abs() < 1e-12);
        }
    }

    #[test]
    fn test_adaa_matches_the_curve_on_slow_signals() {
        let mut adaa = Adaa1::new();
        let mut worst = 0.0f64;
        for i in 0..4000 {
            let x = 3.0 * (i as f64 * 0.001).sin();
            let y = adaa.process(&Tanh, x as f32) as f64;
            if i > 0 {
                // ADAA's output sits half a step back
                let mid = 3.0 * ((i as f64 - 0.5) * 0.001).sin();
                worst = worst.max((y - mid.tanh()).abs());
            }
        }
        assert!(worst < 1e-5, "max error {}", worst);
    }

    #[test]
    fn test_adaa_survives_a_constant_input() {
        let mut adaa = Adaa1::new();
        for _ in 0..10 {
            let y = adaa.process(&HardClip, 0.5);
            assert!(y.is_finite());
        }
        assert!((adaa.process(&HardClip, 0.5) - 0.5).abs() < 1e-7);
    }

    /// Inharmonic energy of a hard-driven sine at the base rate, naive vs
    /// ADAA. (At the base rate alone neither is clean; the module also
    /// oversamples. This only checks that ADAA pulls its weight.)
    fn alias_db<C: Curve>(curve: &C, amplitude: f64, use_adaa: bool) -> f64 {
        let (sr, n) = (44100.0, 16384);
        let f = 1115.0 * sr / n as f64; // periodic in n, not in sr
        let mut adaa = Adaa1::new();
        let out: Vec<f32> = (0..n * 2)
            .map(|i| {
                let x = amplitude * (2.0 * PI * f * i as f64 / sr).sin();
                if use_adaa {
                    adaa.process(curve, x as f32)
                } else {
                    curve.value(x) as f32
                }
            })
            .collect();
        Spectrum::of_periodic(&out[n..], sr as f32).alias_energy_db(f, 0.5, 20000.0)
    }

    #[test]
    fn test_adaa_cuts_aliasing() {
        let naive = alias_db(&HardClip, 8.0, false);
        let adaa = alias_db(&HardClip, 8.0, true);
        assert!(adaa < naive - 6.0, "hard clip: naive {:.1} dB, ADAA {:.1} dB", naive, adaa);
        let naive = alias_db(&RoundedFolder::new(0.4), 2.5, false);
        let adaa = alias_db(&RoundedFolder::new(0.4), 2.5, true);
        assert!(adaa < naive - 6.0, "folder: naive {:.1} dB, ADAA {:.1} dB", naive, adaa);
    }
}
