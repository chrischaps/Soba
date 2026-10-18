//! Band-limited corrections for discontinuities: four-point polyBLEP (steps)
//! and polyBLAMP (corners), applied through a two-sample delay.
//!
//! A naive waveform jumps between samples; the correction spreads each jump
//! over the two samples either side of it. The kernel is the cubic B-spline,
//! whose spectrum falls as sinc⁴: twice the rejection, in dB, of the common
//! two-point (linear) polyBLEP, which matters for the harmonics that fold
//! back into the audible band.
//!
//! Holding two samples back means an event discovered while advancing to
//! sample `n` can still correct samples `n - 2` and `n - 1`, so any
//! discontinuity works the same way: a phase wrap, a pulse edge, a hard-sync
//! reset, or a wrap running backwards under through-zero FM.

/// Residual of a unit step, at offset `x` in -2..=0 before the step: the
/// B-spline's running integral. After the step the residual is the mirror
/// image, negated.
#[inline]
fn step_residual(x: f32) -> f32 {
    if x <= -1.0 {
        let t = x + 2.0;
        t * t * t * t * (1.0 / 24.0)
    } else {
        0.5 + x * (2.0 / 3.0) - x * x * x * (1.0 / 3.0) - x * x * x * x * 0.125
    }
}

/// Residual of a unit change of slope, at offset `x` in -2..=0 from the
/// corner. It's symmetric, so the same values apply after it.
#[inline]
fn corner_residual(x: f32) -> f32 {
    if x <= -1.0 {
        let t = x + 2.0;
        t * t * t * t * t * (1.0 / 120.0)
    } else {
        let x2 = x * x;
        7.0 / 30.0 + 0.5 * x + x2 * (1.0 / 3.0) - x2 * x2 * (1.0 / 12.0) - x2 * x2 * x * (1.0 / 40.0)
    }
}

/// Where the phase meets breakpoint `b` between `p0` and `p1`, as the
/// unwrapped phase of the crossing. A phase of exactly 1.0 sits before the
/// wrap and 0.0 after it, so leaving either of them across the wrap counts.
#[inline]
pub fn crossing(p0: f32, p1: f32, b: f32) -> Option<f32> {
    if p1 > p0 {
        let mut target = b + (p0 - b).ceil();
        if target == p0 && p0 < 1.0 {
            target += 1.0;
        }
        (target <= p1).then_some(target)
    } else if p1 < p0 {
        let mut target = b + (p0 - b).floor();
        if target == p0 && p0 > 0.0 {
            target -= 1.0;
        }
        (target >= p1).then_some(target)
    } else {
        None
    }
}

/// One output's correction state. Feed it events for the interval between
/// the newest held sample and the next one, then push the next naive sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlepDelay {
    /// The two previous samples, oldest first, still open to correction.
    held: [f32; 2],
    /// Corrections already owed to the next two samples.
    pending: [f32; 2],
}

impl BlepDelay {
    /// Samples between a naive sample going in and coming out.
    pub const LATENCY: usize = 2;

    pub const fn new() -> Self {
        Self { held: [0.0; 2], pending: [0.0; 2] }
    }

    /// A step of `height` at fraction `d` (0..=1) of the way from the newest
    /// held sample to the next one.
    #[inline]
    pub fn step(&mut self, d: f32, height: f32) {
        self.held[0] += height * step_residual(-1.0 - d);
        self.held[1] += height * step_residual(-d);
        self.pending[0] -= height * step_residual(d - 1.0);
        self.pending[1] -= height * step_residual(d - 2.0);
    }

    /// A change of slope at fraction `d`, with `slope_change` in output units
    /// per sample.
    #[inline]
    pub fn corner(&mut self, d: f32, slope_change: f32) {
        self.held[0] += slope_change * corner_residual(-1.0 - d);
        self.held[1] += slope_change * corner_residual(-d);
        self.pending[0] += slope_change * corner_residual(d - 1.0);
        self.pending[1] += slope_change * corner_residual(d - 2.0);
    }

    /// Takes the next naive sample and returns the finished one from two
    /// samples ago.
    #[inline]
    pub fn push(&mut self, naive: f32) -> f32 {
        let out = self.held[0];
        self.held = [self.held[1], naive + self.pending[0]];
        self.pending = [self.pending[1], 0.0];
        out
    }

    /// Holds `value` as if it had been coming in all along, so a waveform
    /// can start without a step up from silence.
    pub fn fill(&mut self, value: f32) {
        *self = Self { held: [value; 2], pending: [0.0; 2] };
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pushes `naive` values, applying `events` just before sample 3 goes in,
    /// and returns what comes out, realigned to the input.
    fn run(naive: &[f32], events: impl FnOnce(&mut BlepDelay)) -> Vec<f32> {
        let mut b = BlepDelay::new();
        let mut out = Vec::new();
        let mut events = Some(events);
        for (i, &x) in naive.iter().enumerate() {
            if i == 3 {
                (events.take().unwrap())(&mut b);
            }
            out.push(b.push(x));
        }
        out.drain(..BlepDelay::LATENCY);
        out
    }

    #[test]
    fn test_kernel_pieces_join_up() {
        assert!((step_residual(-1.0) - 1.0 / 24.0).abs() < 1e-6);
        assert!((step_residual(0.0) - 0.5).abs() < 1e-6);
        assert!(step_residual(-2.0).abs() < 1e-9);
        assert!((corner_residual(-1.0) - 1.0 / 120.0).abs() < 1e-6);
        assert!(corner_residual(-2.0).abs() < 1e-9);
    }

    #[test]
    fn test_step_halfway_is_antisymmetric() {
        // A unit step halfway between samples 2 and 3: the corrected samples
        // mirror each other about the midpoint of the step
        let out = run(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0], |b| b.step(0.5, 1.0));
        for k in 0..2 {
            let before = out[2 - k];
            let after = out[3 + k];
            assert!((before + after - 1.0).abs() < 1e-6, "{out:?}");
        }
        assert!(out[2] > 0.0 && out[1] > 0.0, "{out:?}");
    }

    #[test]
    fn test_step_on_a_sample_puts_it_halfway() {
        let out = run(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0], |b| b.step(1.0, 1.0));
        assert!((out[3] - 0.5).abs() < 1e-6, "{out:?}");
        assert!((out[2] - 1.0 / 24.0).abs() < 1e-6, "{out:?}");
        assert!((out[4] - (1.0 - 1.0 / 24.0)).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn test_corner_matches_bandlimited_ramp() {
        // max(0, t) smoothed by the cubic B-spline is 7/30 at the corner
        let out = run(&[0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0], |b| b.corner(1.0, 1.0));
        assert!((out[3] - 7.0 / 30.0).abs() < 1e-6, "{out:?}");
        assert!((out[2] - 1.0 / 120.0).abs() < 1e-6, "{out:?}");
        assert!((out[4] - (1.0 + 1.0 / 120.0)).abs() < 1e-6, "{out:?}");
    }
}
