//! Loudness of a stereo render: ITU-R BS.1770-4 integrated loudness and
//! EBU Tech 3342 loudness range.
//!
//! Like [`analysis`](super::analysis), this is offline measurement for tests
//! and the render tool. It allocates freely and is **not** for the audio
//! thread. The arithmetic runs in `f64`.
//!
//! Both channels are K-weighted (a high shelf for the head, then a high pass
//! for the ear's bass roll-off) and their mean squares summed, so a 1 kHz sine
//! reads its RMS level on each channel, a 0 dBFS sine on one channel -3.01
//! LUFS. The measurement is taken in 100 ms steps: 400 ms blocks for
//! integrated loudness and 3 s windows for the range.
//!
//! [`true_peak`] is the same standard's peak meter (Annex 2): the highest the
//! waveform goes between the samples, which is what a DAC or a lossy encoder
//! clips, rather than at them.

/// The two loudness readings of a render. `None` where the render is too
/// short to measure or nothing in it clears the -70 LUFS gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness in LUFS (BS.1770-4: 400 ms blocks overlapping by
    /// 75%, gated at -70 LUFS absolute and 10 LU below the ungated mean).
    pub integrated: Option<f64>,
    /// Loudness range in LU (Tech 3342: the spread from the 10th to the 95th
    /// percentile of 3 s short-term loudness, gated at -70 LUFS absolute and
    /// 20 LU below the mean).
    pub range: Option<f64>,
}

/// Blocks absolutely gated out sit below this, in LUFS.
const ABSOLUTE_GATE: f64 = -70.0;
/// The relative gate for integrated loudness, in LU below the gated mean.
const INTEGRATED_RELATIVE_GATE: f64 = -10.0;
/// The relative gate for the loudness range.
const RANGE_RELATIVE_GATE: f64 = -20.0;
/// 100 ms steps in a 400 ms momentary block and a 3 s short-term window.
const MOMENTARY_STEPS: usize = 4;
const SHORT_TERM_STEPS: usize = 30;

/// Measures the loudness of a stereo signal at `sample_rate`.
pub fn loudness(left: &[f32], right: &[f32], sample_rate: f32) -> Loudness {
    let step = (sample_rate as f64 * 0.1).round().max(1.0) as usize;
    let energies = step_energies(left, right, sample_rate as f64, step);
    let momentary = block_powers(&energies, MOMENTARY_STEPS, step);
    let short_term = block_powers(&energies, SHORT_TERM_STEPS, step);
    Loudness { integrated: integrated(&momentary), range: range(&short_term) }
}

/// The K-weighted energy (both channels' squared samples, summed) of each
/// whole step of `step` samples. A trailing part step is dropped.
fn step_energies(left: &[f32], right: &[f32], sample_rate: f64, step: usize) -> Vec<f64> {
    let mut filters = [KWeighting::new(sample_rate), KWeighting::new(sample_rate)];
    left.chunks_exact(step)
        .zip(right.chunks_exact(step))
        .map(|(l, r)| {
            l.iter().zip(r).fold(0.0, |energy, (&l, &r)| {
                let (l, r) = (filters[0].process(l as f64), filters[1].process(r as f64));
                energy + l * l + r * r
            })
        })
        .collect()
}

/// The mean-square power of each block of `len` consecutive steps, one block
/// starting at each step.
fn block_powers(energies: &[f64], len: usize, step: usize) -> Vec<f64> {
    energies.windows(len).map(|block| block.iter().sum::<f64>() / (len * step) as f64).collect()
}

/// The loudness, in LUFS, of a summed mean-square power.
fn lufs(power: f64) -> f64 {
    -0.691 + 10.0 * power.max(1e-20).log10()
}

/// Mean power of the blocks whose loudness clears `gate`, or `None` if none do.
fn gated_mean(powers: &[f64], gate: f64) -> Option<f64> {
    let kept: Vec<f64> = powers.iter().copied().filter(|&p| lufs(p) > gate).collect();
    (!kept.is_empty()).then(|| kept.iter().sum::<f64>() / kept.len() as f64)
}

fn integrated(momentary: &[f64]) -> Option<f64> {
    let relative_gate = lufs(gated_mean(momentary, ABSOLUTE_GATE)?) + INTEGRATED_RELATIVE_GATE;
    gated_mean(momentary, relative_gate.max(ABSOLUTE_GATE)).map(lufs)
}

fn range(short_term: &[f64]) -> Option<f64> {
    let relative_gate = lufs(gated_mean(short_term, ABSOLUTE_GATE)?) + RANGE_RELATIVE_GATE;
    let gate = relative_gate.max(ABSOLUTE_GATE);
    let mut kept: Vec<f64> = short_term.iter().map(|&p| lufs(p)).filter(|&l| l > gate).collect();
    kept.sort_by(f64::total_cmp);
    // Nearest-rank percentiles, as libebur128 takes them
    let at = |fraction: f64| kept[((kept.len() - 1) as f64 * fraction + 0.5) as usize];
    Some(at(0.95) - at(0.10))
}

/// The true peak of one channel as an amplitude, to read in dBTP: the largest
/// absolute value of the waveform, oversampled 4× below 96 kHz and 2× below
/// 192 kHz with BS.1770-4's interpolation filter. From 192 kHz up the samples
/// are close enough together to read on their own. Never less than the sample
/// peak, since the filter's points fall between the samples, not on them.
pub fn true_peak(channel: &[f32], sample_rate: f32) -> f32 {
    let phases: &[usize] = match sample_rate {
        r if r < 96_000.0 => &[0, 1, 2, 3],
        r if r < 192_000.0 => &[0, 2],
        _ => &[],
    };
    let sample_peak = channel.iter().fold(0.0f64, |peak, &s| peak.max(s.abs() as f64));
    if phases.is_empty() {
        return sample_peak as f32;
    }
    // Silence either side, so the filter runs into the first sample and out
    // past the last
    let taps = INTERPOLATION_PHASES[0].len();
    let padding = std::iter::repeat_n(0.0, taps - 1);
    let padded: Vec<f64> = padding.clone().chain(channel.iter().map(|&s| s as f64)).chain(padding).collect();
    padded
        .windows(taps)
        .flat_map(|window| {
            phases.iter().map(move |&phase| {
                let coefficients = INTERPOLATION_PHASES[phase].iter();
                coefficients.zip(window.iter().rev()).map(|(c, x)| c * x).sum::<f64>().abs()
            })
        })
        .fold(sample_peak, f64::max) as f32
}

/// BS.1770-4 Annex 2's 48-tap interpolation filter for 4× oversampling, as
/// its four 12-tap phases: each makes one of the four points per sample, at
/// an eighth, three eighths, five eighths and seven eighths of the way to
/// the next. The last two phases are the first two reversed. Its passband is
/// flat to within 0.1 dB up to 20 kHz at 48 kHz.
const INTERPOLATION_PHASES: [[f64; 12]; 4] = [
    [
        0.0017089843750, 0.0109863281250, -0.0196533203125, 0.0332031250000, -0.0594482421875, 0.1373291015625,
        0.9721679687500, -0.1022949218750, 0.0476074218750, -0.0266113281250, 0.0148925781250, -0.0083007812500,
    ],
    [
        -0.0291748046875, 0.0292968750000, -0.0517578125000, 0.0891113281250, -0.1665039062500, 0.4650878906250,
        0.7797851562500, -0.2003173828125, 0.1015625000000, -0.0582275390625, 0.0330810546875, -0.0189208984375,
    ],
    [
        -0.0189208984375, 0.0330810546875, -0.0582275390625, 0.1015625000000, -0.2003173828125, 0.7797851562500,
        0.4650878906250, -0.1665039062500, 0.0891113281250, -0.0517578125000, 0.0292968750000, -0.0291748046875,
    ],
    [
        -0.0083007812500, 0.0148925781250, -0.0266113281250, 0.0476074218750, -0.1022949218750, 0.9721679687500,
        0.1373291015625, -0.0594482421875, 0.0332031250000, -0.0196533203125, 0.0109863281250, 0.0017089843750,
    ],
];

/// BS.1770's two-stage K-weighting filter, designed for any sample rate from
/// the analog prototype behind the standard's 48 kHz coefficients.
struct KWeighting {
    stages: [Biquad; 2],
}

impl KWeighting {
    fn new(sample_rate: f64) -> Self {
        use std::f64::consts::PI;

        // Stage 1: a high shelf of about +4 dB above 1.5 kHz, for the head
        let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
        let k = (PI * f0 / sample_rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        let shelf = Biquad::new(
            [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
            [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        );

        // Stage 2: a high pass at 38 Hz, the RLB weighting
        let (f0, q) = (38.13547087602444, 0.5003270373238773);
        let k = (PI * f0 / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = Biquad::new([1.0, -2.0, 1.0], [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0]);

        Self { stages: [shelf, high_pass] }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.stages[0].process(x);
        self.stages[1].process(y)
    }
}

/// A transposed direct form II biquad with `a0` normalized to 1.
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    s1: f64,
    s2: f64,
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self { b, a, s1: 0.0, s2: 0.0 }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s1;
        self.s1 = self.b[1] * x - self.a[0] * y + self.s2;
        self.s2 = self.b[2] * x - self.a[1] * y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, db: f64, sample_rate: f64, seconds: f64) -> Vec<f32> {
        let amplitude = 10f64.powf(db / 20.0);
        (0..(seconds * sample_rate) as usize)
            .map(|i| (amplitude * (std::f64::consts::TAU * freq * i as f64 / sample_rate).sin()) as f32)
            .collect()
    }

    fn assert_near(got: Option<f64>, want: f64, tolerance: f64) {
        let got = got.expect("a reading");
        assert!((got - want).abs() <= tolerance, "read {:.3}, expected {:.2} ±{}", got, want, tolerance);
    }

    #[test]
    fn test_k_weighting_matches_the_standards_48k_coefficients() {
        // BS.1770-4 Tables 1 and 2
        let k = KWeighting::new(48_000.0);
        let [shelf, high_pass] = &k.stages;
        let expected_shelf = [1.53512485958697, -2.69169618940638, 1.19839281085285, -1.69065929318241, 0.73248077421585];
        let got_shelf = [shelf.b[0], shelf.b[1], shelf.b[2], shelf.a[0], shelf.a[1]];
        for (got, want) in got_shelf.iter().zip(expected_shelf) {
            assert!((got - want).abs() < 1e-9, "shelf {:?}", got_shelf);
        }
        assert_eq!(high_pass.b, [1.0, -2.0, 1.0]);
        assert!((high_pass.a[0] - -1.99004745483398).abs() < 1e-9, "high pass {:?}", high_pass.a);
        assert!((high_pass.a[1] - 0.99007225036621).abs() < 1e-9, "high pass {:?}", high_pass.a);
    }

    #[test]
    fn test_sine_at_minus_20_dbfs_on_both_channels_reads_minus_20_lufs() {
        let s = sine(1000.0, -20.0, 48_000.0, 5.0);
        let reading = loudness(&s, &s, 48_000.0);
        assert_near(reading.integrated, -20.0, 0.1);
        assert_near(reading.range, 0.0, 0.1);
    }

    #[test]
    fn test_full_scale_sine_on_one_channel_reads_minus_3_01() {
        for sample_rate in [48_000.0, 44_100.0, 96_000.0] {
            let s = sine(1000.0, 0.0, sample_rate, 5.0);
            let silence = vec![0.0; s.len()];
            assert_near(loudness(&s, &silence, sample_rate as f32).integrated, -3.01, 0.1);
            assert_near(loudness(&silence, &s, sample_rate as f32).integrated, -3.01, 0.1);
        }
    }

    #[test]
    fn test_k_weighting_lowers_the_bass_and_lifts_the_treble() {
        let reading = |freq| {
            let s = sine(freq, -20.0, 48_000.0, 5.0);
            loudness(&s, &s, 48_000.0).integrated.unwrap()
        };
        assert!(reading(40.0) < -21.0, "40 Hz {:.2}", reading(40.0));
        assert!(reading(4000.0) > -17.0, "4 kHz {:.2}", reading(4000.0));
    }

    #[test]
    fn test_quiet_second_half_is_gated_out() {
        // Half at -20, half at -40: the ungated mean of the powers is -23 LUFS,
        // but the -40 half falls below the relative gate (-33) and drops out
        let mut s = sine(1000.0, -20.0, 48_000.0, 5.0);
        s.extend(sine(1000.0, -40.0, 48_000.0, 5.0));
        assert_near(loudness(&s, &s, 48_000.0).integrated, -20.0, 0.2);
    }

    #[test]
    fn test_quiet_half_above_the_relative_gate_counts() {
        // -20 then -26: both halves clear the gate and the reading falls between
        let mut s = sine(1000.0, -20.0, 48_000.0, 5.0);
        s.extend(sine(1000.0, -26.0, 48_000.0, 5.0));
        let reading = loudness(&s, &s, 48_000.0).integrated.unwrap();
        assert!(reading < -21.5 && reading > -24.0, "read {:.2}", reading);
    }

    #[test]
    fn test_silent_tail_is_gated_out() {
        let mut s = sine(1000.0, -20.0, 48_000.0, 3.0);
        s.extend(vec![0.0; 48_000 * 7]);
        // The three blocks straddling the cut (-21, -23 and -26 LUFS) clear
        // the relative gate and pull the reading down a fifth of a dB
        assert_near(loudness(&s, &s, 48_000.0).integrated, -20.2, 0.1);
    }

    #[test]
    fn test_silence_and_short_renders_have_no_reading() {
        let silence = vec![0.0; 48_000 * 5];
        assert_eq!(loudness(&silence, &silence, 48_000.0), Loudness { integrated: None, range: None });

        let s = sine(1000.0, -20.0, 48_000.0, 1.0);
        let reading = loudness(&s, &s, 48_000.0);
        assert_near(reading.integrated, -20.0, 0.1);
        assert_eq!(reading.range, None, "a second is shorter than a short-term window");
    }

    #[test]
    fn test_loudness_range_spans_a_step_down() {
        // 10 s at -20 then 10 s at -30: the range is the 10 LU between them
        let mut s = sine(1000.0, -20.0, 48_000.0, 10.0);
        s.extend(sine(1000.0, -30.0, 48_000.0, 10.0));
        assert_near(loudness(&s, &s, 48_000.0).range, 10.0, 0.2);
    }

    fn db(amplitude: f32) -> f64 {
        20.0 * (amplitude as f64).log10()
    }

    /// A full-scale sine at a quarter of the sample rate, a 45° phase offset
    /// putting every sample at ±0.707: the peaks fall halfway between them.
    fn quarter_rate_sine(samples: usize) -> Vec<f32> {
        (0..samples).map(|i| (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4).sin() as f32).collect()
    }

    #[test]
    fn test_peaks_between_the_samples_read_full_scale() {
        let s = quarter_rate_sine(4800);
        assert!((db(crate::dsp::analysis::peak(&s)) - -3.01).abs() < 0.01);
        for sample_rate in [44_100.0, 48_000.0] {
            let reading = db(true_peak(&s, sample_rate));
            assert!(reading.abs() < 0.2, "{} Hz: read {:.2} dBTP", sample_rate, reading);
        }
    }

    #[test]
    fn test_oversampling_halves_at_96k_and_stops_at_192k() {
        let s = quarter_rate_sine(4800);
        // 2× finds points an eighth of a sample from the crest, 1× only the samples
        let reading = db(true_peak(&s, 96_000.0));
        assert!(reading > -0.5 && reading < 0.2, "96 kHz: read {:.2} dBTP", reading);
        assert!((db(true_peak(&s, 192_000.0)) - -3.01).abs() < 0.01);
    }

    #[test]
    fn test_square_wave_overshoots_its_sample_peak() {
        // A naive ±0.5 square at 1 kHz: the band-limited waveform rings past
        // its flat tops at each edge
        let s: Vec<f32> = (0..48_000).map(|i| if i % 48 < 24 { 0.5 } else { -0.5 }).collect();
        let (sample, true_) = (db(crate::dsp::analysis::peak(&s)), db(true_peak(&s, 48_000.0)));
        assert!(true_ > sample + 0.5, "sample peak {:.2} dBFS, true peak {:.2} dBTP", sample, true_);
    }

    #[test]
    fn test_true_peak_never_reads_under_the_sample_peak() {
        assert_eq!(true_peak(&[], 48_000.0), 0.0);
        assert_eq!(true_peak(&[0.0; 100], 48_000.0), 0.0);
        // A lone click: the filter's points either side of it fall short of it
        let mut click = vec![0.0; 100];
        click[50] = -0.8;
        assert_eq!(true_peak(&click, 48_000.0), 0.8);
    }
}
