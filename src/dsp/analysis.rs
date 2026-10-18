//! Offline signal analysis for tests and the render tool.
//!
//! These helpers measure rendered audio: level, decay, pitch and spectral
//! content. They allocate freely and are **not** for use on the audio thread.
//! Spectral work runs in `f64` so measurements are limited by the signal, not
//! by rounding.

use std::f64::consts::PI;

/// Root-mean-square level of a block of samples.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Largest absolute sample value.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, &s| m.max(s.abs()))
}

/// Converts a linear amplitude to decibels (floored at -200 dB).
pub use super::decibels::gain_to_db as amp_to_db;

/// RMS of consecutive, non-overlapping windows of `window` samples.
/// A trailing partial window is dropped.
pub fn windowed_rms(samples: &[f32], window: usize) -> Vec<f32> {
    samples.chunks_exact(window.max(1)).map(rms).collect()
}

/// Reverberation time (RT60) of an impulse response, in seconds.
///
/// Uses Schroeder's backward-integrated energy decay curve and the T30
/// method: a straight line is fitted to the curve between -5 and -35 dB and
/// extended to -60 dB. The response should run until it has fallen well past
/// -35 dB. Returns `None` if it never falls that far.
pub fn rt60(impulse_response: &[f32], sample_rate: f32) -> Option<f32> {
    let mut edc = vec![0.0f64; impulse_response.len()];
    let mut energy = 0.0;
    for (i, &s) in impulse_response.iter().enumerate().rev() {
        energy += (s as f64) * (s as f64);
        edc[i] = energy;
    }
    let total = *edc.first()?;
    if total <= 0.0 {
        return None;
    }

    // Least-squares line through (time, dB) over the -5..-35 dB span
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &e) in edc.iter().enumerate() {
        let db = 10.0 * (e / total).max(1e-30).log10();
        if (-35.0..=-5.0).contains(&db) {
            let t = i as f64 / sample_rate as f64;
            n += 1.0;
            sx += t;
            sy += db;
            sxx += t * t;
            sxy += t * db;
        }
    }
    let reached_35 = edc.iter().any(|&e| e / total < 10f64.powf(-3.5));
    if n < 2.0 || !reached_35 {
        return None;
    }
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    (slope < 0.0).then(|| (-60.0 / slope) as f32)
}

/// Normalized echo density (Abel & Huang) of a stretch of reverb.
///
/// The fraction of samples lying more than one standard deviation from zero,
/// divided by the fraction expected for Gaussian noise (0.3173). Sparse,
/// distinct echoes read well below 1; a fully diffuse tail reads about 1.
pub fn echo_density(samples: &[f32]) -> f32 {
    let sd = rms(samples);
    if sd == 0.0 {
        return 0.0;
    }
    let outside = samples.iter().filter(|s| s.abs() > sd).count();
    outside as f32 / samples.len() as f32 / 0.317_310_5
}

/// A one-sided magnitude spectrum.
pub struct Spectrum {
    /// Magnitude of each bin from DC to Nyquist. A full-scale sine at a bin
    /// centre reads about 1.0.
    pub magnitudes: Vec<f64>,
    /// Frequency spacing between bins, in Hz.
    pub bin_hz: f64,
}

impl Spectrum {
    /// Hann-windowed magnitude spectrum, zero-padded to the next power of two.
    pub fn of(samples: &[f32], sample_rate: f32) -> Self {
        let n = samples.len().next_power_of_two().max(2);
        let mut re = vec![0.0f64; n];
        let mut im = vec![0.0f64; n];

        let len = samples.len();
        let mut window_sum = 0.0;
        for (i, &s) in samples.iter().enumerate() {
            let w = if len > 1 {
                0.5 - 0.5 * (2.0 * PI * i as f64 / (len - 1) as f64).cos()
            } else {
                1.0
            };
            window_sum += w;
            re[i] = s as f64 * w;
        }

        fft(&mut re, &mut im);

        // A sine of amplitude A gives a peak of A * window_sum / 2.
        let scale = if window_sum > 0.0 { 2.0 / window_sum } else { 0.0 };
        let magnitudes = (0..=n / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * scale)
            .collect();

        Self {
            magnitudes,
            bin_hz: sample_rate as f64 / n as f64,
        }
    }

    /// Unwindowed magnitude spectrum of a power-of-two length buffer that
    /// holds a whole number of periods. Every partial then lands exactly on
    /// a bin with no leakage, so energy between harmonics is genuinely there
    /// (aliasing, jitter) rather than the window's skirt.
    pub fn of_periodic(samples: &[f32], sample_rate: f32) -> Self {
        let n = samples.len();
        assert!(n.is_power_of_two(), "of_periodic needs a power-of-two length");
        let mut re: Vec<f64> = samples.iter().map(|&s| s as f64).collect();
        let mut im = vec![0.0f64; n];
        fft(&mut re, &mut im);
        let scale = 2.0 / n as f64;
        let magnitudes = (0..=n / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * scale)
            .collect();
        Self {
            magnitudes,
            bin_hz: sample_rate as f64 / n as f64,
        }
    }

    /// Energy more than `tolerance_bins` from every harmonic of `fundamental`,
    /// below `max_hz`, in dB relative to all energy below `max_hz`.
    pub fn alias_energy_db(&self, fundamental: f64, tolerance_bins: f64, max_hz: f64) -> f64 {
        let mut total = 0.0;
        let mut alias = 0.0;
        for (k, &mag) in self.magnitudes.iter().enumerate().skip(1) {
            let freq = k as f64 * self.bin_hz;
            if freq > max_hz {
                break;
            }
            let energy = mag * mag;
            total += energy;
            let harmonic = (freq / fundamental).round().max(1.0) * fundamental;
            if (freq - harmonic).abs() > tolerance_bins * self.bin_hz {
                alias += energy;
            }
        }
        if total <= 0.0 {
            return f64::NEG_INFINITY;
        }
        10.0 * (alias.max(1e-30) / total).log10()
    }

    /// Frequency of the strongest non-DC component, refined between bins by
    /// fitting a parabola to the log magnitudes around the peak.
    pub fn dominant_frequency(&self) -> f64 {
        let m = &self.magnitudes;
        let k = (1..m.len())
            .max_by(|&a, &b| m[a].total_cmp(&m[b]))
            .unwrap_or(1);
        if k == 0 || k + 1 >= m.len() {
            return k as f64 * self.bin_hz;
        }
        let (a, b, c) = (
            m[k - 1].max(1e-30).ln(),
            m[k].max(1e-30).ln(),
            m[k + 1].max(1e-30).ln(),
        );
        let denom = a - 2.0 * b + c;
        let offset = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
        (k as f64 + offset) * self.bin_hz
    }

    /// Fraction of energy *not* near a harmonic of `fundamental`, in dB
    /// relative to total energy. Bins within `tolerance_bins` of any harmonic
    /// count as harmonic. Aliasing shows up as energy between harmonics, so
    /// lower is cleaner (a pure sine reads below -100 dB).
    pub fn inharmonic_energy_db(&self, fundamental: f64, tolerance_bins: usize) -> f64 {
        let mut total = 0.0;
        let mut inharmonic = 0.0;
        for (k, &mag) in self.magnitudes.iter().enumerate().skip(1) {
            let energy = mag * mag;
            total += energy;
            let freq = k as f64 * self.bin_hz;
            let harmonic = (freq / fundamental).round().max(1.0);
            let distance_bins = (freq - harmonic * fundamental).abs() / self.bin_hz;
            if distance_bins > tolerance_bins as f64 {
                inharmonic += energy;
            }
        }
        if total <= 0.0 {
            return f64::NEG_INFINITY;
        }
        10.0 * (inharmonic.max(1e-30) / total).log10()
    }
}

/// In-place iterative radix-2 Cooley-Tukey FFT. Lengths must be a power of two.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);

    // Bit-reversal permutation
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Butterflies
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * PI / len as f64;
        let (w_im, w_re) = angle.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cur_re, mut cur_im) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = start + k;
                let b = a + len / 2;
                let t_re = re[b] * cur_re - im[b] * cur_im;
                let t_im = re[b] * cur_im + im[b] * cur_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                let next_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = next_re;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amplitude: f32, sample_rate: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate).sin())
            .collect()
    }

    #[test]
    fn test_rms_and_peak_of_sine() {
        let s = sine(1000.0, 0.5, 48000.0, 48000);
        assert!((rms(&s) - 0.5 / 2f32.sqrt()).abs() < 1e-3);
        assert!((peak(&s) - 0.5).abs() < 1e-3);
        assert!((amp_to_db(0.5) + 6.0206).abs() < 1e-3);
    }

    #[test]
    fn test_windowed_rms_drops_partial_window() {
        assert_eq!(windowed_rms(&[1.0; 10], 4), vec![1.0, 1.0]);
    }

    #[test]
    fn test_rt60_of_exponential_noise() {
        // Noise under an envelope falling 60 dB in 1.5 s
        let sr = 48000.0;
        let mut seed = 1u32;
        let ir: Vec<f32> = (0..(3.0 * sr) as usize)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = seed as i32 as f32 / i32::MAX as f32;
                noise * 10f32.powf(-3.0 * i as f32 / sr / 1.5)
            })
            .collect();
        let t = rt60(&ir, sr).unwrap();
        assert!((t - 1.5).abs() < 0.03, "rt60 {}", t);
        assert_eq!(rt60(&[0.0; 100], sr), None);
    }

    #[test]
    fn test_echo_density_tells_clicks_from_noise() {
        let mut clicks = vec![0.0f32; 4800];
        for i in (0..4800).step_by(480) {
            clicks[i] = 1.0;
        }
        assert!(echo_density(&clicks) < 0.1);
        let noise: Vec<f32> = (0..4800u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 8) as f32 / 8_388_608.0 - 1.0)
            .collect();
        // Uniform noise has fewer outliers than Gaussian: 0.42 / 0.3173
        assert!((echo_density(&noise) - 1.33).abs() < 0.1);
    }

    #[test]
    fn test_fft_matches_naive_dft() {
        let x: Vec<f64> = (0..16).map(|i| ((i * 7 % 5) as f64) - 2.0).collect();
        let mut re = x.clone();
        let mut im = vec![0.0; 16];
        fft(&mut re, &mut im);
        for k in 0..16 {
            let (mut dr, mut di) = (0.0, 0.0);
            for (t, &v) in x.iter().enumerate() {
                let a = -2.0 * PI * (k * t) as f64 / 16.0;
                dr += v * a.cos();
                di += v * a.sin();
            }
            assert!((re[k] - dr).abs() < 1e-9 && (im[k] - di).abs() < 1e-9, "bin {}", k);
        }
    }

    #[test]
    fn test_dominant_frequency_is_sub_bin_accurate() {
        let sr = 44100.0;
        let s = sine(261.625_58, 0.8, sr, 65536);
        let f = Spectrum::of(&s, sr).dominant_frequency();
        let cents = 1200.0 * (f / 261.625_58).log2();
        assert!(cents.abs() < 0.1, "measured {} Hz ({} cents off)", f, cents);
    }

    #[test]
    fn test_spectrum_amplitude_scaling() {
        let sr = 48000.0;
        // 3000 Hz is an exact bin centre for n = 16384 at 48 kHz
        let spectrum = Spectrum::of(&sine(3000.0, 0.25, sr, 16384), sr);
        let max = spectrum.magnitudes.iter().cloned().fold(0.0, f64::max);
        assert!((max - 0.25).abs() < 0.01, "peak magnitude {}", max);
    }

    #[test]
    fn test_inharmonic_energy() {
        let sr = 48000.0;
        let n = 16384;
        // Pure tone: essentially no inharmonic energy
        let pure = Spectrum::of(&sine(375.0, 0.5, sr, n), sr);
        assert!(pure.inharmonic_energy_db(375.0, 3) < -80.0);

        // Add a non-harmonic partial 20 dB down: about -20 dB inharmonic
        let mut mixed = sine(375.0, 0.5, sr, n);
        for (m, t) in mixed.iter_mut().zip(sine(1234.0, 0.05, sr, n)) {
            *m += t;
        }
        let db = Spectrum::of(&mixed, sr).inharmonic_energy_db(375.0, 3);
        assert!((db + 20.0).abs() < 1.0, "inharmonic energy {} dB", db);
    }
}
