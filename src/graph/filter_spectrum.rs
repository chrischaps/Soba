//! The sound through a filter, as its display draws it under the curve.
//!
//! Each frame, every filter's [`SpectrumTap`] gives up its newest window of
//! input and output. Both go through one FFT (two real signals packed as one
//! complex one), and the bins are gathered into bands an eighth of an octave
//! wide on the display's 20 Hz to 20 kHz axis. Each band holds the power in
//! it, the way an analyser in a studio does, so pink noise lies flat.
//!
//! What's drawn moves like a landscape rather than a meter: bands rise
//! quickly and settle slowly, and the whole picture is scaled to the
//! loudest band coming in, which itself falls slowly, so a quiet passage
//! still fills the plot and a loud one never clips it. Only the filters
//! drawn last frame are analysed.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::spectrum_tap::{SpectrumTap, WINDOW};
use crate::engine::NodeId as EngineNodeId;

use super::filter_trail::Trail;

/// The frequency axis, in Hz, shared with the display.
pub const LOWEST_HZ: f32 = 20.0;
pub const HIGHEST_HZ: f32 = 20_000.0;

/// Bands across the axis: an eighth of an octave each.
pub const BANDS: usize = 80;

/// What a band shows when nothing is there, in dB.
const SILENCE_DB: f32 = -120.0;

/// How fast a band rises to a louder reading and falls from a quieter one,
/// as time constants in seconds.
const RISE_SECONDS: f32 = 0.03;
const FALL_SECONDS: f32 = 0.35;

/// How fast the scale follows the loudest band down, in dB a second, and
/// the quietest it will scale to: quieter than this is drawn as quiet.
const REFERENCE_FALL_DB: f32 = 6.0;
const REFERENCE_FLOOR_DB: f32 = -60.0;

/// The loudest input band at which the landscape starts to show, and at
/// which it's fully there, in dB.
const FADE_IN_FROM_DB: f32 = -84.0;
const FADE_IN_TO_DB: f32 = -66.0;

/// One filter's spectra, in dB, band by band from the bottom of the axis.
pub struct FilterSpectrum {
    /// What comes in.
    pub input: [f32; BANDS],
    /// What the heard output lets through.
    pub output: [f32; BANDS],
    /// The loudest band coming in, falling slowly: the top of the picture.
    pub reference: f32,
    /// How much there is to see, 0 (silence) to 1.
    pub presence: f32,
    /// The last two seconds of it, for the strip along the display's floor.
    pub trail: Trail,
    /// How many samples the tap had written when last read.
    written: usize,
    /// Whether the display drew it since the last update.
    drawn: Cell<bool>,
}

impl FilterSpectrum {
    fn new() -> Self {
        Self {
            input: [SILENCE_DB; BANDS],
            output: [SILENCE_DB; BANDS],
            reference: REFERENCE_FLOOR_DB,
            presence: 0.0,
            trail: Trail::default(),
            written: 0,
            drawn: Cell::new(true),
        }
    }

    /// Notes that the display is showing it, so it's kept up to date.
    pub fn mark_drawn(&self) {
        self.drawn.set(true);
    }

    /// The centre of band `band`, in Hz.
    pub fn band_hz(band: usize) -> f32 {
        LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf((band as f32 + 0.5) / BANDS as f32)
    }
}

/// Every filter's spectra, and what it takes to work them out.
#[derive(Default)]
pub struct FilterSpectra {
    spectra: HashMap<EngineNodeId, FilterSpectrum>,
    analyser: Option<Analyser>,
}

impl FilterSpectra {
    /// A filter's spectra, if it has been heard.
    pub fn get(&self, node_id: EngineNodeId) -> Option<&FilterSpectrum> {
        self.spectra.get(&node_id)
    }

    /// Reads each tap and moves its filter's spectra on by `dt` seconds.
    /// Filters the display didn't draw last frame are skipped, and filters
    /// that are gone are forgotten.
    pub fn listen<'a>(&mut self, taps: impl Iterator<Item = (EngineNodeId, &'a Arc<SpectrumTap>)>, dt: f32) {
        let mut heard = Vec::new();
        for (node_id, tap) in taps {
            heard.push(node_id);
            let spectrum = self.spectra.entry(node_id).or_insert_with(FilterSpectrum::new);
            if !spectrum.drawn.replace(false) {
                // Off screen: whatever it heard before won't follow on from now
                spectrum.trail.clear();
                continue;
            }
            let analyser = self.analyser.get_or_insert_with(Analyser::new);
            let (input, output) = match analyser.read(tap, &mut spectrum.written) {
                Some(bands) => bands,
                // Nothing new, as when the audio has stopped: let it sink
                None => ([SILENCE_DB; BANDS], [SILENCE_DB; BANDS]),
            };
            settle(&mut spectrum.input, &input, dt);
            settle(&mut spectrum.output, &output, dt);

            let loudest = spectrum.input.iter().copied().fold(SILENCE_DB, f32::max);
            spectrum.reference = loudest.max(spectrum.reference - REFERENCE_FALL_DB * dt).max(REFERENCE_FLOOR_DB);
            spectrum.presence = ((loudest - FADE_IN_FROM_DB) / (FADE_IN_TO_DB - FADE_IN_FROM_DB)).clamp(0.0, 1.0);
            spectrum.trail.advance(&input, &output, spectrum.reference, spectrum.presence, dt);
        }
        self.spectra.retain(|node_id, _| heard.contains(node_id));
    }

    pub fn clear(&mut self) {
        self.spectra.clear();
    }
}

/// Moves each band toward its new reading: up quickly, down slowly.
fn settle(bands: &mut [f32; BANDS], target: &[f32; BANDS], dt: f32) {
    let rise = 1.0 - (-dt / RISE_SECONDS).exp();
    let fall = 1.0 - (-dt / FALL_SECONDS).exp();
    for (band, &target) in bands.iter_mut().zip(target) {
        let rate = if target > *band { rise } else { fall };
        *band += (target - *band) * rate;
    }
}

/// Where a band's power is read from, in bins.
#[derive(Clone, Copy)]
enum BandBins {
    /// The bins whose centres lie in the band, summed.
    Sum(usize, usize),
    /// A band narrower than a bin, read between the two bins around its
    /// centre: this far from the first toward the second.
    Between(usize, f32),
}

/// The FFT and its scratch room, shared by every filter.
struct Analyser {
    fft: Fft,
    window: Vec<f32>,
    /// Turns a band's summed bin power into a sine's amplitude squared.
    scale: f32,
    input: Box<[f32; WINDOW]>,
    output: Box<[f32; WINDOW]>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Each bin's power, in and out
    power_in: Vec<f32>,
    power_out: Vec<f32>,
    /// The bins each band reads, for the sample rate they were laid out at.
    bands: Vec<BandBins>,
    sample_rate: f32,
}

impl Analyser {
    fn new() -> Self {
        let n = WINDOW;
        let window: Vec<f32> =
            (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
        let energy: f32 = window.iter().map(|w| w * w).sum();
        Self {
            fft: Fft::new(n),
            scale: 4.0 / (n as f32 * energy),
            window,
            input: Box::new([0.0; WINDOW]),
            output: Box::new([0.0; WINDOW]),
            re: vec![0.0; n],
            im: vec![0.0; n],
            power_in: vec![0.0; n / 2 + 1],
            power_out: vec![0.0; n / 2 + 1],
            bands: Vec::new(),
            sample_rate: 0.0,
        }
    }

    /// The band levels in and out, in dB, if `tap` has anything newer than
    /// `written`, which it updates.
    fn read(&mut self, tap: &SpectrumTap, written: &mut usize) -> Option<([f32; BANDS], [f32; BANDS])> {
        let now = tap.read(&mut self.input, &mut self.output);
        if now == *written {
            return None;
        }
        *written = now;
        let sample_rate = tap.sample_rate();
        if sample_rate != self.sample_rate {
            self.bands = lay_out_bands(sample_rate, WINDOW);
            self.sample_rate = sample_rate;
        }

        // Input as the real part, output as the imaginary: one FFT for both
        for n in 0..WINDOW {
            self.re[n] = self.input[n] * self.window[n];
            self.im[n] = self.output[n] * self.window[n];
        }
        self.fft.transform(&mut self.re, &mut self.im);

        // Unpacked, bin by bin: each signal's power
        for k in 0..=WINDOW / 2 {
            let (a, b) = (self.re[k], self.im[k]);
            let mirror = (WINDOW - k) % WINDOW;
            let (c, d) = (self.re[mirror], self.im[mirror]);
            self.power_in[k] = ((a + c) * (a + c) + (b - d) * (b - d)) * 0.25;
            self.power_out[k] = ((b + d) * (b + d) + (a - c) * (a - c)) * 0.25;
        }

        let input = band_levels(&self.power_in, &self.bands, self.scale);
        let output = band_levels(&self.power_out, &self.bands, self.scale);
        Some((input, output))
    }
}

/// Which bins each band reads, for a window of `n` samples at `sample_rate`.
fn lay_out_bands(sample_rate: f32, n: usize) -> Vec<BandBins> {
    let bin_hz = sample_rate / n as f32;
    let top_bin = n / 2;
    let edge = |band: usize| LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(band as f32 / BANDS as f32);
    (0..BANDS)
        .map(|band| {
            let first = (edge(band) / bin_hz).ceil() as usize;
            let past = ((edge(band + 1) / bin_hz).ceil() as usize).min(top_bin + 1);
            if first < past {
                BandBins::Sum(first, past)
            } else {
                let at = (FilterSpectrum::band_hz(band) / bin_hz).min(top_bin as f32 - 1.0);
                BandBins::Between(at.floor() as usize, at.fract())
            }
        })
        .collect()
}

/// Each band's level in dB, as the amplitude of a sine with that power.
fn band_levels(power: &[f32], bands: &[BandBins], scale: f32) -> [f32; BANDS] {
    let mut levels = [SILENCE_DB; BANDS];
    for (level, bins) in levels.iter_mut().zip(bands) {
        let sum = match *bins {
            BandBins::Sum(first, past) => power[first..past].iter().sum(),
            BandBins::Between(below, t) => between_bins(power, below, t),
        };
        *level = (10.0 * (sum * scale).max(1e-12).log10()).max(SILENCE_DB);
    }
    levels
}

/// The power `t` of the way from bin `below` to the next, read along a
/// smooth curve through the log power of the bins around. A windowed
/// tone's peak is close to a parabola in log power, so this finds it
/// between bins, where a straight line would put it on one.
fn between_bins(power: &[f32], below: usize, t: f32) -> f32 {
    let log = |bin: usize| power[bin.min(power.len() - 1)].max(1e-20).ln();
    let (p0, p1, p2, p3) = (log(below.saturating_sub(1)), log(below), log(below + 1), log(below + 2));
    let curve = 0.5
        * (2.0 * p1
            + (p2 - p0) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
            + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t);
    curve.exp()
}

/// A radix-2 complex FFT of one size, its twiddles and bit-reversal worked
/// out once.
struct Fft {
    twiddles: Vec<(f32, f32)>,
    reversed: Vec<usize>,
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        Self {
            twiddles: (0..n / 2)
                .map(|k| {
                    let angle = -std::f64::consts::TAU * k as f64 / n as f64;
                    (angle.cos() as f32, angle.sin() as f32)
                })
                .collect(),
            reversed: (0..n).map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) }).collect(),
        }
    }

    /// Transforms `re` + i`im` in place.
    fn transform(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.reversed.len();
        for i in 0..n {
            let j = self.reversed[i];
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let stride = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * stride];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::SignalBuffer;

    const SR: f32 = 48_000.0;

    fn sine(hz: f32, amplitude: f32) -> impl Fn(usize) -> f32 {
        move |n| amplitude * (std::f32::consts::TAU * hz * n as f32 / SR).sin()
    }

    /// A tap that has heard `input` going in and `output` coming out.
    fn tap(input: impl Fn(usize) -> f32, output: impl Fn(usize) -> f32) -> Arc<SpectrumTap> {
        let tap = Arc::new(SpectrumTap::new());
        feed(&tap, input, output);
        tap
    }

    /// Writes a window more of `input` and `output` to `tap`.
    fn feed(tap: &SpectrumTap, input: impl Fn(usize) -> f32, output: impl Fn(usize) -> f32) {
        let (mut a, mut b) = (SignalBuffer::audio(WINDOW), SignalBuffer::audio(WINDOW));
        for n in 0..WINDOW {
            a.channel_mut(0)[n] = input(n);
            b.channel_mut(0)[n] = output(n);
        }
        tap.write(&a, &b, SR);
    }

    fn band_of(hz: f32) -> usize {
        ((hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln() * BANDS as f32) as usize
    }

    fn loudest(bands: &[f32; BANDS]) -> (usize, f32) {
        bands.iter().copied().enumerate().fold((0, SILENCE_DB), |best, (i, db)| if db > best.1 { (i, db) } else { best })
    }

    #[test]
    fn the_fft_matches_a_plain_dft() {
        let n = 64;
        let signal: Vec<(f32, f32)> = (0..n).map(|i| ((i as f32 * 0.37).sin(), (i as f32 * 1.1).cos() * 0.5)).collect();
        let (mut re, mut im): (Vec<f32>, Vec<f32>) = signal.iter().copied().unzip();
        Fft::new(n).transform(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for (i, &(x, y)) in signal.iter().enumerate() {
                let angle = -std::f64::consts::TAU * (k * i) as f64 / n as f64;
                sr += x as f64 * angle.cos() - y as f64 * angle.sin();
                si += x as f64 * angle.sin() + y as f64 * angle.cos();
            }
            assert!((re[k] as f64 - sr).abs() < 1e-3 && (im[k] as f64 - si).abs() < 1e-3, "bin {k}");
        }
    }

    #[test]
    fn input_and_output_come_apart_from_one_fft() {
        let tap = tap(sine(220.0, 0.5), sine(3_000.0, 0.25));
        let mut analyser = Analyser::new();
        let (input, output) = analyser.read(&tap, &mut 0).expect("fresh samples");

        let (band, db) = loudest(&input);
        assert!(band.abs_diff(band_of(220.0)) <= 1, "input peaks at 220 Hz, not band {band}");
        assert!((db - -6.0).abs() < 3.0, "a sine at half scale reads -6 dB, not {db:.1}");
        assert!(input[band_of(3_000.0)] < db - 60.0, "the output's tone stays out of the input");

        let (band, db) = loudest(&output);
        assert!(band.abs_diff(band_of(3_000.0)) <= 1, "output peaks at 3 kHz, not band {band}");
        assert!((db - -12.0).abs() < 1.5, "a sine at quarter scale reads -12 dB, not {db:.1}");
        assert!(output[band_of(220.0)] < db - 60.0, "the input's tone stays out of the output");
    }

    #[test]
    fn a_bass_note_below_the_bins_still_reads() {
        // 40 Hz bands are narrower than a bin: read between bins instead
        let tap = tap(sine(40.0, 0.5), |_| 0.0);
        let (input, _) = Analyser::new().read(&tap, &mut 0).unwrap();
        let (band, db) = loudest(&input);
        assert!(band.abs_diff(band_of(40.0)) <= 1, "peaks at 40 Hz, not band {band}");
        assert!(db > -12.0, "and loud: {db:.1} dB");
    }

    #[test]
    fn nothing_new_reads_as_nothing() {
        let tap = tap(sine(220.0, 0.5), |_| 0.0);
        let mut analyser = Analyser::new();
        let mut written = 0;
        assert!(analyser.read(&tap, &mut written).is_some());
        assert!(analyser.read(&tap, &mut written).is_none());
    }

    #[test]
    fn rises_quickly_falls_slowly_and_fades_out_in_silence() {
        let loud = tap(sine(1_000.0, 0.5), sine(1_000.0, 0.5));
        let mut spectra = FilterSpectra::default();
        for _ in 0..30 {
            spectra.get(7).inspect(|s| s.mark_drawn());
            spectra.listen(std::iter::once((7, &loud)), 1.0 / 60.0);
            // Fresh samples each frame, as the engine would write
            feed(&loud, sine(1_000.0, 0.5), sine(1_000.0, 0.5));
        }
        let heard = spectra.get(7).unwrap();
        assert!(heard.presence > 0.99, "a tone is there to see");
        assert!((heard.reference - loudest(&heard.input).1).abs() < 0.5, "the scale is set by the loudest band");

        // The tone stops: nothing new arrives, and it sinks away
        for _ in 0..12 {
            spectra.get(7).inspect(|s| s.mark_drawn());
            spectra.listen(std::iter::once((7, &loud)), 1.0 / 60.0);
        }
        let fading = loudest(&spectra.get(7).unwrap().input).1;
        assert!(fading < -15.0 && fading > -80.0, "a fifth of a second on, it's falling but not gone: {fading:.1}");
        for _ in 0..240 {
            spectra.get(7).inspect(|s| s.mark_drawn());
            spectra.listen(std::iter::once((7, &loud)), 1.0 / 60.0);
        }
        assert_eq!(spectra.get(7).unwrap().presence, 0.0, "four seconds on, it's gone");
    }

    #[test]
    fn only_filters_on_screen_are_analysed_and_gone_ones_forgotten() {
        let tap = tap(sine(1_000.0, 0.5), |_| 0.0);
        let mut spectra = FilterSpectra::default();
        // New filters are analysed straight away
        spectra.listen(std::iter::once((3, &tap)), 1.0 / 60.0);
        let first = spectra.get(3).unwrap().input;
        assert!(loudest(&first).1 > SILENCE_DB);
        // Not drawn since: left as it was
        feed(&tap, sine(500.0, 0.5), |_| 0.0);
        spectra.listen(std::iter::once((3, &tap)), 1.0 / 60.0);
        assert_eq!(spectra.get(3).unwrap().input, first);
        // Gone from the graph: forgotten
        spectra.listen(std::iter::empty(), 1.0 / 60.0);
        assert!(spectra.get(3).is_none());
    }
}
