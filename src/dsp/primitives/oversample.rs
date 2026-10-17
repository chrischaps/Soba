//! 2x and 4x oversampling with polyphase IIR halfband filters.
//!
//! A nonlinearity (a saturating filter stage, a waveshaper) creates harmonics
//! above Nyquist that fold back down as inharmonic aliases. Running it at
//! twice the sample rate gives those harmonics room: the upsampler removes
//! the spectral image the doubling creates, the nonlinearity runs on the
//! wider band, and the downsampler removes everything above the original
//! Nyquist before decimating, so it never folds back.
//!
//! Both directions use the polyphase allpass halfband structure (Regalia,
//! Mitra and Vaidyanathan; coefficient design after Laurent de Soras' HIIR).
//! Two chains of first-order allpasses run at the low rate, one per polyphase
//! branch. That makes it very cheap, gives it low latency (a few samples, not
//! the tens a linear-phase FIR needs), and its passband ripple is
//! effectively zero. Its phase is not linear, which inside a filter's
//! signal path is inaudible.

use std::f64::consts::PI;

use crate::dsp::denormal::flush;

/// Allpass coefficients per halfband filter.
pub const HALFBAND_COEFS: usize = 8;

/// Allpass coefficients in the outer stage of a 4x oversampler.
const OUTER_COEFS: usize = 6;

/// Transition half-width of the outer stage, between 2x and 4x. Everything
/// it has to pass was already band-limited to 27.8 kHz by the inner stage,
/// and everything it has to stop lies above 67 kHz (at 48 kHz in), so a far
/// wider transition, and half the coefficients, will do.
const OUTER_TRANSITION: f64 = 0.1;

/// Transition half-width, as a fraction of the oversampled rate. The filter
/// passes up to `0.25 - HALFBAND_TRANSITION` and stops from
/// `0.25 + HALFBAND_TRANSITION`: at 48 kHz in, flat to 20.2 kHz and
/// about 100 dB down from 27.8 kHz.
const HALFBAND_TRANSITION: f64 = 0.04;

/// Designs the allpass coefficients of an elliptic polyphase halfband filter
/// with `N` coefficients and the given transition half-width.
fn design_halfband<const N: usize>(transition: f64) -> [f32; N] {
    // Elliptic modulus and nome for the transition band
    let k = ((1.0 - transition * 2.0) * PI / 4.0).tan().powi(2);
    let kk = (1.0 - k * k).powf(0.25);
    let e = 0.5 * (1.0 - kk) / (1.0 + kk);
    let e4 = e.powi(4);
    let q = e * (1.0 + e4 * (2.0 + e4 * (15.0 + 150.0 * e4)));

    let order = (N * 2 + 1) as f64;
    let mut coefs = [0.0f32; N];
    for (index, coef) in coefs.iter_mut().enumerate() {
        let c = (index + 1) as f64;
        // Theta-function series for the pole positions
        let mut num = 0.0;
        let mut i = 0;
        loop {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            let term = q.powi(i * (i + 1)) * ((2 * i + 1) as f64 * c * PI / order).sin() * sign;
            num += term;
            i += 1;
            if term.abs() <= 1e-100 {
                break;
            }
        }
        let mut den = 0.0;
        let mut i = 1;
        loop {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            let term = q.powi(i * i) * ((2 * i) as f64 * c * PI / order).cos() * sign;
            den += term;
            i += 1;
            if term.abs() <= 1e-100 {
                break;
            }
        }
        let ww = num * q.powf(0.25) / (den + 0.5);
        let wwsq = ww * ww;
        let x = ((1.0 - wwsq * k) * (1.0 - wwsq / k)).sqrt() / (1.0 + wwsq);
        *coef = ((1.0 - x) / (1.0 + x)) as f32;
    }
    coefs
}

/// The two allpass chains of a polyphase halfband filter.
///
/// Coefficients alternate between the chains: even indices on branch 0, odd
/// on branch 1. Each section is `H(z) = (c + z⁻¹) / (1 + c·z⁻¹)` at the low
/// rate, which is `(c + z⁻²) / (1 + c·z⁻²)` at the high rate.
#[derive(Clone, Debug)]
struct AllpassPair<const N: usize = HALFBAND_COEFS> {
    coefs: [f32; N],
    x: [f32; N],
    y: [f32; N],
}

impl<const N: usize> AllpassPair<N> {
    fn new(transition: f64) -> Self {
        Self {
            coefs: design_halfband::<N>(transition),
            x: [0.0; N],
            y: [0.0; N],
        }
    }

    /// Runs one sample through each branch.
    #[inline]
    fn process(&mut self, mut branch0: f32, mut branch1: f32) -> (f32, f32) {
        for i in (0..N).step_by(2) {
            let y0 = (branch0 - self.y[i]) * self.coefs[i] + self.x[i];
            let y1 = (branch1 - self.y[i + 1]) * self.coefs[i + 1] + self.x[i + 1];
            self.x[i] = branch0;
            self.x[i + 1] = branch1;
            self.y[i] = flush(y0);
            self.y[i + 1] = flush(y1);
            branch0 = y0;
            branch1 = y1;
        }
        (branch0, branch1)
    }

    fn reset(&mut self) {
        self.x = [0.0; N];
        self.y = [0.0; N];
    }
}

/// Doubles the sample rate: each input sample becomes two output samples,
/// with the image above the original Nyquist removed.
#[derive(Clone, Debug)]
pub struct Upsampler2x {
    allpass: AllpassPair,
}

impl Upsampler2x {
    pub fn new() -> Self {
        Self { allpass: AllpassPair::new(HALFBAND_TRANSITION) }
    }

    /// Returns the two oversampled samples for one input sample, in order.
    #[inline]
    pub fn process(&mut self, x: f32) -> [f32; 2] {
        let (a, b) = self.allpass.process(x, x);
        [a, b]
    }

    pub fn reset(&mut self) {
        self.allpass.reset();
    }
}

impl Default for Upsampler2x {
    fn default() -> Self {
        Self::new()
    }
}

/// Halves the sample rate: removes everything above the new Nyquist, then
/// keeps one sample in two.
#[derive(Clone, Debug)]
pub struct Downsampler2x {
    allpass: AllpassPair,
}

impl Downsampler2x {
    pub fn new() -> Self {
        Self { allpass: AllpassPair::new(HALFBAND_TRANSITION) }
    }

    /// Returns one output sample for two oversampled input samples.
    #[inline]
    pub fn process(&mut self, x: [f32; 2]) -> f32 {
        let (a, b) = self.allpass.process(x[1], x[0]);
        0.5 * (a + b)
    }

    pub fn reset(&mut self) {
        self.allpass.reset();
    }
}

impl Default for Downsampler2x {
    fn default() -> Self {
        Self::new()
    }
}

/// Quadruples the sample rate with two cascaded halfband stages: the full
/// filter from 1x to 2x, then a cheaper one from 2x to 4x.
#[derive(Clone, Debug)]
pub struct Upsampler4x {
    inner: Upsampler2x,
    outer: AllpassPair<OUTER_COEFS>,
}

impl Upsampler4x {
    pub fn new() -> Self {
        Self {
            inner: Upsampler2x::new(),
            outer: AllpassPair::new(OUTER_TRANSITION),
        }
    }

    /// Returns the four oversampled samples for one input sample, in order.
    #[inline]
    pub fn process(&mut self, x: f32) -> [f32; 4] {
        let [a, b] = self.inner.process(x);
        let (a0, a1) = self.outer.process(a, a);
        let (b0, b1) = self.outer.process(b, b);
        [a0, a1, b0, b1]
    }

    pub fn reset(&mut self) {
        self.inner.reset();
        self.outer.reset();
    }
}

impl Default for Upsampler4x {
    fn default() -> Self {
        Self::new()
    }
}

/// Quarters the sample rate: the mirror image of [`Upsampler4x`].
#[derive(Clone, Debug)]
pub struct Downsampler4x {
    outer: AllpassPair<OUTER_COEFS>,
    inner: Downsampler2x,
}

impl Downsampler4x {
    pub fn new() -> Self {
        Self {
            outer: AllpassPair::new(OUTER_TRANSITION),
            inner: Downsampler2x::new(),
        }
    }

    /// Returns one output sample for four oversampled input samples.
    #[inline]
    pub fn process(&mut self, x: [f32; 4]) -> f32 {
        let (a0, a1) = self.outer.process(x[1], x[0]);
        let (b0, b1) = self.outer.process(x[3], x[2]);
        self.inner.process([0.5 * (a0 + a1), 0.5 * (b0 + b1)])
    }

    pub fn reset(&mut self) {
        self.outer.reset();
        self.inner.reset();
    }
}

impl Default for Downsampler4x {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{amp_to_db, rms, Spectrum};

    /// A test tone, computed in f64: an f32 phase thousands of radians long
    /// carries enough phase noise to mask a 60 dB measurement.
    fn sine(freq: f32, sample_rate: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq as f64 * i as f64 / sample_rate as f64).sin() as f32)
            .collect()
    }

    /// Magnitude of the halfband filter `0.5·(A0(z²) + z⁻¹·A1(z²))` at
    /// `f`, a fraction of the oversampled rate.
    fn halfband_magnitude(coefs: &[f32], f: f64) -> f64 {
        type C = (f64, f64);
        let mul = |a: C, b: C| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
        let div = |a: C, b: C| {
            let d = b.0 * b.0 + b.1 * b.1;
            ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
        };
        let w = 2.0 * PI * f;
        let z1: C = (w.cos(), -w.sin());
        let z2 = mul(z1, z1);
        let mut branches: [C; 2] = [(1.0, 0.0), (1.0, 0.0)];
        for (i, &c) in coefs.iter().enumerate() {
            let c = c as f64;
            let section = div((c + z2.0, z2.1), (1.0 + c * z2.0, c * z2.1));
            branches[i % 2] = mul(branches[i % 2], section);
        }
        let odd = mul(z1, branches[1]);
        (0.5 * (branches[0].0 + odd.0)).hypot(0.5 * (branches[0].1 + odd.1))
    }

    #[test]
    fn test_designed_stopband_is_about_100_db() {
        let coefs = design_halfband::<HALFBAND_COEFS>(HALFBAND_TRANSITION);
        let stop = 0.25 + HALFBAND_TRANSITION;
        let worst = (0..=500)
            .map(|i| halfband_magnitude(&coefs, stop + (0.5 - stop) * i as f64 / 500.0))
            .fold(0.0, f64::max);
        assert!(20.0 * worst.log10() < -95.0, "stopband {:.1} dB", 20.0 * worst.log10());
        // Power complementary: exactly -3 dB at the halfband point
        let half = 20.0 * halfband_magnitude(&coefs, 0.25).log10();
        assert!((half + 3.01).abs() < 0.01, "{:.3} dB at fs/4", half);
    }

    #[test]
    fn test_outer_stage_stopband_is_about_100_db() {
        let coefs = design_halfband::<OUTER_COEFS>(OUTER_TRANSITION);
        let stop = 0.25 + OUTER_TRANSITION;
        let worst = (0..=500)
            .map(|i| halfband_magnitude(&coefs, stop + (0.5 - stop) * i as f64 / 500.0))
            .fold(0.0, f64::max);
        let db = 20.0 * worst.log10();
        assert!(db < -95.0, "outer stopband {:.1} dB", db);
    }

    #[test]
    fn test_coefficients_are_stable_and_ascending() {
        let coefs = design_halfband::<HALFBAND_COEFS>(HALFBAND_TRANSITION);
        for pair in coefs.windows(2) {
            assert!(pair[0] < pair[1], "{:?}", coefs);
        }
        assert!(coefs.iter().all(|&c| c > 0.0 && c < 1.0), "{:?}", coefs);
    }

    /// Gain of an up-then-down round trip at `freq` (base rate 48 kHz).
    fn round_trip_gain(freq: f32) -> f32 {
        let sr = 48000.0;
        let input = sine(freq, sr, 9600);
        let mut up = Upsampler2x::new();
        let mut down = Downsampler2x::new();
        let out: Vec<f32> = input.iter().map(|&x| down.process(up.process(x))).collect();
        rms(&out[4800..]) / rms(&input[4800..])
    }

    #[test]
    fn test_round_trip_passband_is_flat() {
        for f in [50.0, 1000.0, 10000.0, 18000.0, 20000.0] {
            let db = amp_to_db(round_trip_gain(f));
            assert!(db.abs() < 0.01, "{} Hz: {:.4} dB", f, db);
        }
    }

    #[test]
    fn test_upsampler_removes_the_image() {
        // Doubling 48 kHz to 96 kHz mirrors a tone at f to 48 kHz - f.
        for f in [5000.0, 15000.0, 20000.0] {
            let input = sine(f, 48000.0, 8192);
            let mut up = Upsampler2x::new();
            let out: Vec<f32> = input.iter().flat_map(|&x| up.process(x)).collect();
            let spectrum = Spectrum::of(&out[4096..], 96000.0);
            let level = |hz: f64| {
                let bin = (hz / spectrum.bin_hz).round() as usize;
                spectrum.magnitudes[bin - 3..=bin + 3].iter().cloned().fold(0.0, f64::max)
            };
            let image = 20.0 * (level(48000.0 - f as f64) / level(f as f64)).log10();
            assert!(image < -85.0, "{} Hz image at {:.1} dB", f, image);
        }
    }

    #[test]
    fn test_downsampler_rejects_above_the_new_nyquist() {
        // Tones between 28 and 48 kHz at the high rate would fold into the
        // audible band; the downsampler must stop them first.
        for f in [28000.0, 30000.0, 40000.0, 47000.0] {
            let input = sine(f, 96000.0, 19200);
            let mut down = Downsampler2x::new();
            let out: Vec<f32> = input.as_chunks::<2>().0.iter().map(|p| down.process([p[0], p[1]])).collect();
            let db = amp_to_db(rms(&out[4800..]) / rms(&input));
            assert!(db < -85.0, "{} Hz leaks at {:.1} dB", f, db);
        }
    }

    #[test]
    fn test_4x_round_trip_passband_is_flat() {
        let sr = 48000.0;
        for f in [50.0, 1000.0, 10000.0, 18000.0, 20000.0] {
            let input = sine(f, sr, 9600);
            let mut up = Upsampler4x::new();
            let mut down = Downsampler4x::new();
            let out: Vec<f32> = input.iter().map(|&x| down.process(up.process(x))).collect();
            let db = amp_to_db(rms(&out[4800..]) / rms(&input[4800..]));
            assert!(db.abs() < 0.01, "{} Hz: {:.4} dB", f, db);
        }
    }

    #[test]
    fn test_4x_upsampler_removes_every_image() {
        // 48 kHz to 192 kHz mirrors a tone at f to 48k ± f, 96k ± f and 144k ± f
        for f in [5000.0, 15000.0, 20000.0] {
            let input = sine(f, 48000.0, 8192);
            let mut up = Upsampler4x::new();
            let out: Vec<f32> = input.iter().flat_map(|&x| up.process(x)).collect();
            let spectrum = Spectrum::of(&out[8192..], 192000.0);
            let level = |hz: f64| {
                let bin = (hz / spectrum.bin_hz).round() as usize;
                spectrum.magnitudes[bin - 3..=bin + 3].iter().cloned().fold(0.0, f64::max)
            };
            let f = f as f64;
            for image in [48000.0 - f, 48000.0 + f, 96000.0 - f] {
                let db = 20.0 * (level(image) / level(f)).log10();
                assert!(db < -85.0, "{} Hz image at {} Hz: {:.1} dB", f, image, db);
            }
        }
    }

    #[test]
    fn test_4x_downsampler_rejects_everything_that_would_fold() {
        for f in [28000.0, 40000.0, 60000.0, 70000.0, 90000.0] {
            let input = sine(f, 192000.0, 38400);
            let mut down = Downsampler4x::new();
            let out: Vec<f32> = input
                .as_chunks::<4>().0.iter()
                .map(|p| down.process([p[0], p[1], p[2], p[3]]))
                .collect();
            let db = amp_to_db(rms(&out[4800..]) / rms(&input));
            assert!(db < -85.0, "{} Hz leaks at {:.1} dB", f, db);
        }
    }
}
