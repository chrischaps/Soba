//! Noise source module.
//!
//! White, pink and brown noise for snares, hats, wind, breath and surf, and
//! a slowly wandering random voltage for modulation. Noise into a clocked
//! Sample & Hold is the classic random melody.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// Counts the Noise modules built so far. Each one takes the next number as
/// its random stream, so no two modules, and no two voices of a polyphonic
/// one, play the same noise.
static NEXT_STREAM: AtomicU64 = AtomicU64::new(0);

/// A small PCG random number generator (PCG32, XSH-RR).
///
/// Each stream is its own sequence rather than a shifted copy of one shared
/// sequence, so voices on different streams never line up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pcg32 {
    state: u64,
    increment: u64,
}

impl Pcg32 {
    /// A generator on the next unused stream, so no two modules (or voices)
    /// that draw noise share a sequence.
    pub(crate) fn next_stream() -> Self {
        Self::new(NEXT_STREAM.fetch_add(1, Ordering::Relaxed))
    }

    const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

    /// A generator on the given stream. Tests use it to give two modules
    /// the same noise, so they differ only in what's being tested.
    pub(crate) fn new(stream: u64) -> Self {
        let mut rng = Self { state: 0, increment: (stream << 1) | 1 };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(stream.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        rng.next_u32();
        rng
    }

    #[inline]
    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(Self::MULTIPLIER).wrapping_add(self.increment);
        let shifted = (((old >> 18) ^ old) >> 27) as u32;
        shifted.rotate_right((old >> 59) as u32)
    }

    /// Uniform in [-1, 1).
    #[inline]
    pub(crate) fn bipolar(&mut self) -> f32 {
        self.next_u32() as i32 as f32 * (1.0 / 2_147_483_648.0)
    }
}

/// Paul Kellet's pink noise filter: seven one-pole sections whose sum falls
/// at 3 dB per octave to within ±0.05 dB from about 10 Hz up.
///
/// The coefficients are tuned for 44.1 kHz. At 48 or 96 kHz the slope is
/// the same; only its lowest corner moves up a little, still below 20 Hz.
#[derive(Clone, Copy, Debug, Default)]
struct PinkFilter {
    b: [f32; 7],
}

impl PinkFilter {
    #[inline]
    fn next(&mut self, white: f32) -> f32 {
        let b = &mut self.b;
        b[0] = 0.998_86 * b[0] + white * 0.055_517_9;
        b[1] = 0.993_32 * b[1] + white * 0.075_075_9;
        b[2] = 0.969_00 * b[2] + white * 0.153_852;
        b[3] = 0.866_50 * b[3] + white * 0.310_485_6;
        b[4] = 0.550_00 * b[4] + white * 0.532_952_2;
        b[5] = -0.761_6 * b[5] - white * 0.016_898_0;
        let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.536_2;
        b[6] = white * 0.115_926;
        pink
    }
}

/// Holds a signal inside ±1 without touching anything below the knee.
#[inline]
pub(crate) fn soft_ceiling(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        x
    } else {
        let over = (magnitude - KNEE) / (1.0 - KNEE);
        (KNEE + (1.0 - KNEE) * over.tanh()).copysign(x)
    }
}

/// A noise source: white, pink and brown noise, and a smooth random voltage.
///
/// # Ports
///
/// **Inputs:**
/// - **Level** (Control): Added to the Level knob.
/// - **Rate** (Control): Speeds up or slows down Random; +1 doubles the rate.
///
/// **Outputs:**
/// - **White** (Audio): Equal energy at every frequency, spanning ±1.
/// - **Pink** (Audio): Falls 3 dB per octave: equal energy in every octave.
/// - **Brown** (Audio): Falls 6 dB per octave, like a random walk.
/// - **Random** (Control): Glides with a cosine curve to a new random
///   value between -1 and 1 at the Rate, regardless of Level.
///
/// # Parameters
///
/// - **Level** (0-1): Level of the three noise outputs.
/// - **Rate** (0.01-20 Hz): How often Random picks a new value.
pub struct Noise {
    rng: Pcg32,
    pink: PinkFilter,
    /// Brown noise: a leaky integrator of white noise.
    brown: f32,
    /// Feedback of the brown integrator, set from the sample rate.
    brown_leak: f32,
    /// Input gain of the brown integrator, scaled to `NOISE_RMS`.
    brown_gain: f32,
    /// Random: the value it glides from, the one it glides to, and how far
    /// along it is (0 to 1).
    random_from: f32,
    random_to: f32,
    random_phase: f64,
    /// The Level knob, smoothed. Level CV is added after, unsmoothed.
    level: SmoothedValue,
    sample_rate: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Noise {
    /// Creates a Noise module with its own random stream.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let mut noise = Self {
            rng: Pcg32::next_stream(),
            pink: PinkFilter::default(),
            brown: 0.0,
            brown_leak: 0.0,
            brown_gain: 0.0,
            random_from: 0.0,
            random_to: 0.0,
            random_phase: 0.0,
            level: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            sample_rate,
            ports: vec![
                PortDefinition::input_with_default("level_cv", "Level", SignalType::Control, 0.0).describe("CV added to the Level knob"),
                PortDefinition::input_with_default("rate_cv", "Rate", SignalType::Control, 0.0).describe("CV that speeds up or slows down Random; 1 doubles the rate"),
                PortDefinition::output("white", "White", SignalType::Audio).describe("Equal energy at every frequency: bright hiss, and the classic source for Sample & Hold"),
                PortDefinition::output("pink", "Pink", SignalType::Audio).describe("Falls 3 dB per octave, equal energy in every octave: rain, surf, a balanced hiss"),
                PortDefinition::output("brown", "Brown", SignalType::Audio).describe("Falls 6 dB per octave: a deep rumble, like wind or distant thunder"),
                PortDefinition::output("random", "Random", SignalType::Control).describe("Glides smoothly to a new random value between -1 and 1, Rate times a second"),
            ],
            parameters: vec![
                ParameterDefinition::new("level", "Level", 0.0, 1.0, 0.5, ParameterDisplay::linear(""))
                    .describe("Level of the White, Pink and Brown outputs"),
                ParameterDefinition::new("rate", "Rate", 0.01, 20.0, 1.0, ParameterDisplay::logarithmic("Hz"))
                    .describe("How many new values Random glides to each second"),
            ],
        };
        noise.prepare(sample_rate, 0);
        noise.reset();
        noise
    }

    const PORT_LEVEL_CV: usize = 0;
    const PORT_RATE_CV: usize = 1;

    const PARAM_LEVEL: usize = 0;
    const PARAM_RATE: usize = 1;

    /// RMS level of Pink and Brown at full Level, about -12 dBFS. Their
    /// peaks then rarely reach the soft ceiling. White is uniform over ±1,
    /// about 7 dB louder, so it spans the whole range for Sample & Hold.
    const NOISE_RMS: f32 = 0.25;

    /// Gain bringing Kellet's filter, fed uniform ±1 noise (its output is
    /// 1.748 RMS, measured at 44.1 and 48 kHz alike), to `NOISE_RMS`.
    const PINK_GAIN: f32 = Self::NOISE_RMS / 1.748;

    /// Below this frequency Brown stops rising and levels off, so it never
    /// wanders away from zero.
    const BROWN_CORNER_HZ: f32 = 10.0;

    /// The fastest Random goes, however hard its Rate CV pushes.
    const MAX_RATE_HZ: f32 = 1000.0;

    /// The next value of Random, `step` cycles further on.
    #[inline]
    fn next_random(&mut self, step: f32) -> f32 {
        // In f64, so a glide that takes minutes still moves every sample (#256)
        self.random_phase += f64::from(step);
        if self.random_phase >= 1.0 {
            self.random_phase = (self.random_phase - 1.0).min(1.0);
            self.random_from = self.random_to;
            self.random_to = self.rng.bipolar();
        }
        let shape = 0.5 - 0.5 * (PI * self.random_phase as f32).cos();
        self.random_from + (self.random_to - self.random_from) * shape
    }
}

impl Default for Noise {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Noise {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "source.noise",
            name: "Noise",
            category: ModuleCategory::Source,
            description: "White, pink and brown noise, and a smooth random voltage",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.level.set_sample_rate(sample_rate);

        // A leaky integrator of uniform noise (variance 1/3) has variance
        // gain² / 3 / (1 - leak²): pick the gain that lands on NOISE_RMS
        self.brown_leak = (-2.0 * PI * Self::BROWN_CORNER_HZ / sample_rate).exp();
        self.brown_gain = Self::NOISE_RMS * (3.0 * (1.0 - self.brown_leak * self.brown_leak)).sqrt();
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [white_out, pink_out, brown_out, random_out, ..] = outputs else {
            return;
        };
        let level_knob = params[Self::PARAM_LEVEL];
        let rate_knob = params[Self::PARAM_RATE];
        let level_cv = connected_input(inputs, Self::PORT_LEVEL_CV);
        let rate_cv = connected_input(inputs, Self::PORT_RATE_CV);

        let cv_at = |buffer: Option<&SignalBuffer>, i: usize| {
            buffer.map_or(0.0, |buf| buf.samples.get(i).copied().unwrap_or(0.0))
        };
        let sample_rate = self.sample_rate;
        let step_for = |rate: f32| rate.clamp(0.0, Self::MAX_RATE_HZ) / sample_rate;
        let steady_step = step_for(rate_knob);

        // Only the knob is smoothed. CV is already a signal, and an envelope
        // patched in to shape a drum hit needs its sharp attack kept
        self.level.set_target(level_knob.clamp(0.0, 1.0));

        for i in 0..context.block_size {
            let level = (self.level.next() + cv_at(level_cv, i)).clamp(0.0, 1.0);

            let white = self.rng.bipolar();
            let pink = self.pink.next(white) * Self::PINK_GAIN;
            self.brown = self.brown_leak * self.brown + self.brown_gain * white;

            white_out.samples[i] = white * level;
            pink_out.samples[i] = soft_ceiling(pink) * level;
            brown_out.samples[i] = soft_ceiling(self.brown) * level;

            let step = if rate_cv.is_some() {
                step_for(rate_knob * 2.0_f32.powf(cv_at(rate_cv, i)))
            } else {
                steady_step
            };
            random_out.samples[i] = self.next_random(step);
        }
    }

    /// Clears the filters and starts Random from 0. The random stream keeps
    /// going, so voices reset together still play different noise.
    fn reset(&mut self) {
        self.pink = PinkFilter::default();
        self.brown = 0.0;
        self.random_from = 0.0;
        self.random_to = self.rng.bipolar();
        self.random_phase = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::Spectrum;
    use crate::dsp::Poly;

    const SAMPLE_RATE: f32 = 48000.0;
    const BLOCK: usize = 512;

    /// Runs a Noise for `seconds`, returning (white, pink, brown, random).
    fn render(noise: &mut Noise, params: &[f32], seconds: f32) -> [Vec<f32>; 4] {
        noise.prepare(SAMPLE_RATE, BLOCK);
        let total = (seconds * SAMPLE_RATE) as usize;
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let level = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let rate = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        let mut rendered: [Vec<f32>; 4] = Default::default();
        while rendered[0].len() < total {
            noise.process(&[&level, &rate], &mut outputs, params, &ctx);
            for (out, buffer) in rendered.iter_mut().zip(&outputs) {
                out.extend_from_slice(&buffer.samples);
            }
        }
        rendered.iter_mut().for_each(|out| out.truncate(total));
        rendered
    }

    /// Full-level noise, after the Level smoothing has settled.
    fn full_level_noise(seconds: f32) -> [Vec<f32>; 4] {
        let mut noise = Noise::new();
        noise.level.set_immediate(1.0);
        render(&mut noise, &[1.0, 1.0], seconds)
    }

    /// Mean power density of each octave band from `lowest_hz` up, in dB.
    fn octave_bands_db(samples: &[f32], lowest_hz: f64, octaves: usize) -> Vec<f64> {
        let spectrum = Spectrum::of(samples, SAMPLE_RATE);
        (0..octaves)
            .map(|octave| {
                let low = lowest_hz * 2f64.powi(octave as i32);
                let bins = (low / spectrum.bin_hz) as usize..(2.0 * low / spectrum.bin_hz) as usize;
                let count = bins.len() as f64;
                let power: f64 = spectrum.magnitudes[bins].iter().map(|m| m * m).sum::<f64>() / count;
                10.0 * power.log10()
            })
            .collect()
    }

    /// Least-squares slope of band levels, in dB per octave.
    fn slope_db_per_octave(bands: &[f64]) -> f64 {
        let n = bands.len() as f64;
        let mean_x = (n - 1.0) / 2.0;
        let mean_y = bands.iter().sum::<f64>() / n;
        let (mut covariance, mut variance) = (0.0, 0.0);
        for (x, y) in bands.iter().enumerate() {
            let dx = x as f64 - mean_x;
            covariance += dx * (y - mean_y);
            variance += dx * dx;
        }
        covariance / variance
    }

    fn mean(samples: &[f32]) -> f32 {
        samples.iter().map(|&s| s as f64).sum::<f64>() as f32 / samples.len() as f32
    }

    fn rms(samples: &[f32]) -> f32 {
        crate::dsp::analysis::rms(samples)
    }

    /// Pearson correlation of two equal-length signals.
    fn correlation(a: &[f32], b: &[f32]) -> f32 {
        let (mean_a, mean_b) = (mean(a), mean(b));
        let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
        for (&x, &y) in a.iter().zip(b) {
            let (dx, dy) = ((x - mean_a) as f64, (y - mean_b) as f64);
            ab += dx * dy;
            aa += dx * dx;
            bb += dy * dy;
        }
        (ab / (aa * bb).sqrt()) as f32
    }

    #[test]
    fn test_noise_info_ports_and_parameters() {
        let noise = Noise::new();
        assert_eq!(noise.info().id, "source.noise");
        assert_eq!(noise.info().category, ModuleCategory::Source);

        let ports = noise.ports();
        let ids: Vec<_> = ports.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["level_cv", "rate_cv", "white", "pink", "brown", "random"]);
        assert!(ports[..2].iter().all(|p| p.is_input() && p.signal_type == SignalType::Control));
        assert!(ports[2..5].iter().all(|p| p.is_output() && p.signal_type == SignalType::Audio));
        assert_eq!(ports[5].signal_type, SignalType::Control);

        let params = noise.parameters();
        assert_eq!(params[0].id, "level");
        assert_eq!((params[1].min, params[1].max, params[1].default), (0.01, 20.0, 1.0));
    }

    #[test]
    fn test_white_is_flat() {
        let [white, ..] = full_level_noise(6.0);
        let bands = octave_bands_db(&white, 62.5, 8); // 62.5 Hz to 16 kHz
        let average = bands.iter().sum::<f64>() / bands.len() as f64;
        for (octave, db) in bands.iter().enumerate() {
            assert!((db - average).abs() < 1.5, "white octave {octave} is {:.2} dB off flat", db - average);
        }
    }

    #[test]
    fn test_pink_falls_3_db_per_octave() {
        let [_, pink, ..] = full_level_noise(6.0);
        let bands = octave_bands_db(&pink, 62.5, 8);
        let slope = slope_db_per_octave(&bands);
        assert!((slope + 3.01).abs() < 0.5, "pink slope {slope:.2} dB/oct");
        // Every octave on the line, not just the average
        for (octave, pair) in bands.windows(2).enumerate() {
            let step = pair[1] - pair[0];
            assert!((step + 3.01).abs() < 1.25, "pink octave {octave} falls {step:.2} dB");
        }
    }

    #[test]
    fn test_brown_falls_6_db_per_octave() {
        let [_, _, brown, _] = full_level_noise(6.0);
        let bands = octave_bands_db(&brown, 62.5, 7); // 62.5 Hz to 8 kHz
        let slope = slope_db_per_octave(&bands);
        assert!((slope + 6.02).abs() < 0.6, "brown slope {slope:.2} dB/oct");
    }

    #[test]
    fn test_noise_is_centred_and_bounded() {
        let [white, pink, brown, random] = full_level_noise(10.0);
        for (name, signal) in [("white", &white), ("pink", &pink), ("brown", &brown)] {
            // Pink and Brown move slowly, so their 10 s mean wanders by about 0.015
            assert!(mean(signal).abs() < 0.06, "{name} mean {}", mean(signal));
            assert!(signal.iter().all(|s| s.abs() <= 1.0), "{name} left ±1");
        }
        assert!(random.iter().all(|s| s.abs() <= 1.0), "random left ±1");
    }

    #[test]
    fn test_noise_levels() {
        let [white, pink, brown, _] = full_level_noise(20.0);
        // Uniform over ±1
        assert!((rms(&white) - 1.0 / 3f32.sqrt()).abs() < 0.01, "white rms {}", rms(&white));
        // Brown changes slowly, so even 20 s pins its level to only about 3%
        for (name, signal, margin) in [("pink", &pink, 0.1), ("brown", &brown, 0.15)] {
            let level = rms(signal);
            assert!((level / Noise::NOISE_RMS - 1.0).abs() < margin, "{name} rms {level}");
        }
    }

    #[test]
    fn test_level_scales_noise_but_not_random() {
        let mut noise = Noise::new();
        noise.level.set_immediate(0.0);
        let [white, pink, brown, random] = render(&mut noise, &[0.0, 20.0], 1.0);
        for signal in [&white, &pink, &brown] {
            assert!(signal.iter().all(|&s| s == 0.0));
        }
        assert!(rms(&random) > 0.2, "Random ignores Level");
    }

    #[test]
    fn test_level_cv_adds_to_the_knob() {
        let mut noise = Noise::new();
        noise.prepare(SAMPLE_RATE, BLOCK);
        noise.level.set_immediate(0.0);
        let mut level_cv = SignalBuffer::control(BLOCK);
        level_cv.fill(0.5);
        let rate = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut white = Vec::new();
        for _ in 0..200 {
            noise.process(&[&level_cv, &rate], &mut outputs, &[0.25, 1.0], &ctx);
            white.extend_from_slice(&outputs[0].samples);
        }
        let settled = &white[white.len() / 2..];
        let expected = 0.75 / 3f32.sqrt();
        assert!((rms(settled) / expected - 1.0).abs() < 0.05, "white rms {}", rms(settled));
    }

    #[test]
    fn test_level_zero_is_silent_from_the_first_block() {
        // Loaded at Level 0 for an envelope to play: before the first hit
        // there's nothing, rather than a fade down from the default 0.5
        let mut noise = Noise::new();
        noise.prepare(SAMPLE_RATE, BLOCK);
        let envelope = SignalBuffer::control(BLOCK);
        let rate = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        noise.process(&[&envelope, &rate], &mut outputs, &[0.0, 1.0], &ctx);
        for out in &outputs[..3] {
            assert!(out.samples.iter().all(|&s| s == 0.0));
        }
    }

    #[test]
    fn test_level_knob_turns_are_still_smoothed() {
        let mut noise = Noise::new();
        noise.prepare(SAMPLE_RATE, BLOCK);
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        noise.process(&[&unpatched, &unpatched], &mut outputs, &[0.0, 1.0], &ctx);
        noise.process(&[&unpatched, &unpatched], &mut outputs, &[1.0, 1.0], &ctx);
        // White is uniform ±1 at full level; just after the turn it's still near 0
        let opening = outputs[0].samples[..8].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(opening < 0.05, "Level jumped: {opening}");
    }

    #[test]
    fn test_level_cv_is_not_smoothed() {
        // A drum envelope through Level: the noise must start at once
        let mut noise = Noise::new();
        noise.prepare(SAMPLE_RATE, BLOCK);
        noise.level.set_immediate(0.0);
        let mut level_cv = SignalBuffer::control(BLOCK);
        level_cv.samples[BLOCK / 2..].fill(1.0);
        let rate = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        noise.process(&[&level_cv, &rate], &mut outputs, &[0.0, 1.0], &ctx);
        let white = &outputs[0].samples;
        assert!(white[..BLOCK / 2].iter().all(|&s| s == 0.0));
        // Uniform ±1 at full level from the very first sample
        let onset = &white[BLOCK / 2..BLOCK / 2 + 48];
        assert!(rms(onset) > 0.4, "onset rms {}", rms(onset));
    }

    #[test]
    fn test_random_glides_at_the_rate() {
        let mut noise = Noise::new();
        let step = 2.0 / SAMPLE_RATE;
        let mut random = Vec::new();
        let mut arrivals = 0;
        for _ in 0..(10.0 * SAMPLE_RATE) as usize {
            let target = noise.random_to;
            random.push(noise.next_random(step));
            arrivals += usize::from(noise.random_to != target);
        }
        assert!((19..=21).contains(&arrivals), "{arrivals} new values in 10 s at 2 Hz");

        // A cosine glide between values at most 2 apart moves at most
        // π·rate per second, and it lands on each value flat
        let max_step = random.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_step <= PI * 2.0 / SAMPLE_RATE * 1.01, "random jumped {max_step}");
        assert!(rms(&random) > 0.2, "random barely moved");
    }

    #[test]
    fn test_very_slow_random_keeps_its_rate() {
        // A glide taking 200 s moves every sample, at the speed it should (#256)
        let mut noise = Noise::new();
        let step = 0.005 / SAMPLE_RATE;
        for _ in 0..(150.0 * SAMPLE_RATE) as usize {
            noise.next_random(step);
        }
        assert!((noise.random_phase - 0.75).abs() < 0.75e-3, "phase {}", noise.random_phase);
    }

    #[test]
    fn test_rate_cv_doubles_per_unit() {
        // A quarter of a second at 1 Hz, doubled by +1 of CV: half a glide
        const QUARTER: usize = 480;
        let mut noise = Noise::new();
        noise.prepare(SAMPLE_RATE, QUARTER);
        let level = SignalBuffer::unconnected(QUARTER, SignalType::Control);
        let mut rate_cv = SignalBuffer::control(QUARTER);
        rate_cv.fill(1.0);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(QUARTER)).collect();
        let ctx = ProcessContext::new(SAMPLE_RATE, QUARTER);
        for _ in 0..25 {
            noise.process(&[&level, &rate_cv], &mut outputs, &[0.5, 1.0], &ctx);
        }
        assert!((noise.random_phase - 0.5).abs() < 1e-3, "phase {}", noise.random_phase);
    }

    #[test]
    fn test_two_modules_play_different_noise() {
        let [a, ..] = full_level_noise(1.0);
        let [b, ..] = full_level_noise(1.0);
        assert!(correlation(&a, &b).abs() < 0.03);
    }

    #[test]
    fn test_poly_voices_are_uncorrelated() {
        let mut poly = Poly::<Noise>::default();
        poly.prepare(SAMPLE_RATE, BLOCK);

        // A two-channel Level cable gives two voices
        let mut level_cv = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        level_cv.set_channels(2);
        level_cv.channel_mut(0).fill(0.5);
        level_cv.channel_mut(1).fill(0.5);
        let rate = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs: Vec<SignalBuffer> =
            (0..4).map(|_| SignalBuffer::polyphonic(BLOCK, SignalType::Audio)).collect();
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);

        let mut voices = [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()], [Vec::new(), Vec::new()]];
        for _ in 0..940 {
            poly.process(&[&level_cv, &rate], &mut outputs, &[0.5, 1.0], &ctx);
            assert_eq!(outputs[0].channels(), 2);
            for (output, voice) in outputs.iter().zip(voices.iter_mut()) {
                voice[0].extend_from_slice(&output.voice(0).samples);
                voice[1].extend_from_slice(&output.voice(1).samples);
            }
        }
        // Ten seconds. Brown wanders slowly, so it has fewer independent
        // samples to average and needs the wider margin
        for (name, [first, second], margin) in [("white", &voices[0], 0.02), ("pink", &voices[1], 0.1), ("brown", &voices[2], 0.25)] {
            let r = correlation(first, second);
            assert!(r.abs() < margin, "{name} voices correlate at {r}");
            assert!(rms(first) > 0.05 && rms(second) > 0.05, "{name} voices are silent");
        }
    }

    #[test]
    fn test_soft_ceiling() {
        assert_eq!(soft_ceiling(0.5), 0.5);
        assert_eq!(soft_ceiling(-0.8), -0.8);
        assert!(soft_ceiling(0.9) > 0.85 && soft_ceiling(0.9) < 0.9);
        assert!(soft_ceiling(100.0) <= 1.0);
        assert!(soft_ceiling(-100.0) >= -1.0);
    }

    #[test]
    fn test_noise_registry_instantiation() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create("source.noise").expect("Noise is registered");
        assert_eq!(module.info().name, "Noise");
        assert!(module.polyphonic());
    }
}
