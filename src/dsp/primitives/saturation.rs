//! Saturation curves.

/// Input magnitude at which the rational approximation in [`fast_tanh`]
/// reaches 1.0. Beyond it the curve is held flat.
const FAST_TANH_LIMIT: f32 = 4.97;

/// A fast `tanh` approximation for audio-rate saturation.
///
/// Uses the [7/6] Padé approximant (Lambert's continued fraction, truncated).
/// It is odd, monotonic and bounded to ±1, and stays within about 1e-4 of the
/// true `tanh` everywhere: well below anything audible in a saturator, at a
/// fraction of the cost of `f32::tanh`.
#[inline]
pub fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-FAST_TANH_LIMIT, FAST_TANH_LIMIT);
    let x2 = x * x;
    let num = x * (135135.0 + x2 * (17325.0 + x2 * (378.0 + x2)));
    let den = 135135.0 + x2 * (62370.0 + x2 * (3150.0 + x2 * 28.0));
    (num / den).clamp(-1.0, 1.0)
}

/// A soft clipper that leaves everything inside ±`knee` untouched.
///
/// Above the knee a tanh segment takes over, meeting the straight line with
/// the same value and slope and easing toward ±`ceiling` without reaching it,
/// so a signal crossing the knee bends instead of jumping. `ceiling` must be
/// above `knee`.
#[inline]
pub fn knee_clip(x: f32, knee: f32, ceiling: f32) -> f32 {
    let over = x.abs() - knee;
    if over <= 0.0 {
        x
    } else {
        let room = ceiling - knee;
        (knee + room * (over / room).tanh()).copysign(x)
    }
}

/// A gain-compensated `tanh` saturator: `level * tanh(x / level)`.
///
/// Its slope at zero is exactly 1, so small signals pass unchanged and only
/// signals approaching `level` are squashed. Putting it inside a feedback
/// loop therefore leaves the loop's small-signal behaviour (cutoff, Q) alone
/// while still limiting it at high levels.
#[derive(Clone, Copy, Debug)]
pub struct SoftSaturator {
    level: f32,
    inv_level: f32,
}

impl SoftSaturator {
    /// Creates a saturator that limits its output to ±`level`.
    pub fn new(level: f32) -> Self {
        let level = level.max(1e-6);
        Self { level, inv_level: 1.0 / level }
    }

    /// Saturates `x`.
    #[inline]
    pub fn process(&self, x: f32) -> f32 {
        self.level * fast_tanh(x * self.inv_level)
    }

    /// The saturator's large-signal gain at `x`, `process(x) / x`.
    ///
    /// It is 1 for small `x` and falls toward 0 as `x` grows. Nonlinear
    /// zero-delay-feedback filters use it to linearise the curve around the
    /// previous sample's operating point.
    #[inline]
    pub fn gain_at(&self, x: f32) -> f32 {
        let u = x * self.inv_level;
        if u.abs() < 1e-4 {
            1.0
        } else {
            fast_tanh(u) / u
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_knee_clip_is_transparent_below_the_knee_and_bounded_above() {
        for x in [-0.8, -0.3, 0.0, 0.5, 0.8] {
            assert_eq!(knee_clip(x, 0.8, 1.0), x);
        }
        let mut prev = 0.8;
        for i in 1..2000 {
            let x = 0.8 + i as f32 * 0.01;
            let y = knee_clip(x, 0.8, 1.0);
            assert!(y >= prev && y <= 1.0, "{x} -> {y}");
            assert_eq!(knee_clip(-x, 0.8, 1.0), -y);
            prev = y;
        }
        // Same slope either side of the knee: no corner where it starts
        let slope = (knee_clip(1.0 + 1e-3, 1.0, 1.5) - knee_clip(1.0, 1.0, 1.5)) / 1e-3;
        assert!((slope - 1.0).abs() < 1e-2, "slope {slope}");
    }

    #[test]
    fn test_fast_tanh_tracks_tanh() {
        let mut worst = 0.0f32;
        for i in -20000..=20000 {
            let x = i as f32 * 0.001;
            worst = worst.max((fast_tanh(x) - x.tanh()).abs());
        }
        assert!(worst < 2e-4, "max error {}", worst);
    }

    #[test]
    fn test_fast_tanh_is_odd_monotonic_and_bounded() {
        let mut prev = fast_tanh(-50.0);
        assert!((prev + 1.0).abs() < 1e-5);
        for i in -5000..=5000 {
            let x = i as f32 * 0.002;
            let y = fast_tanh(x);
            assert!(y >= prev, "not monotonic at {}", x);
            assert!(y.abs() <= 1.0);
            assert_eq!(y, -fast_tanh(-x));
            prev = y;
        }
        assert!((fast_tanh(1e9) - 1.0).abs() < 1e-5);
        assert_eq!(fast_tanh(0.0), 0.0);
    }

    #[test]
    fn test_saturator_has_unity_small_signal_gain() {
        let sat = SoftSaturator::new(0.5);
        assert!((sat.process(1e-3) - 1e-3).abs() < 1e-8);
        assert_eq!(sat.gain_at(0.0), 1.0);
        // Large signals are held below the level, and the gain falls with them
        assert!(sat.process(100.0) <= 0.5);
        assert!(sat.gain_at(2.0) < sat.gain_at(0.5));
        assert!((sat.gain_at(0.3) * 0.3 - sat.process(0.3)).abs() < 1e-6);
    }
}
