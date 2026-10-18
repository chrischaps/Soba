//! Output-stage dynamics: a DC blocker and a transparent lookahead peak limiter.
//!
//! These are the last things a signal passes through before it reaches the
//! speakers, so they are built to be inaudible when nothing is wrong and to
//! make guarantees when something is.

use super::primitives::rate_for_seconds;
use super::denormal::flush;
pub use super::decibels::db_to_gain;

/// One-pole DC-blocking highpass at about 5 Hz.
///
/// `y[n] = x[n] - x[n-1] + r * y[n-1]`, with the pole `r` set from the cutoff.
/// Removes any constant offset (from an asymmetric waveshaper, a stray CV
/// patched into the output, ...) without touching audible bass.
#[derive(Debug, Clone)]
pub struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    /// Corner frequency in Hz.
    pub const CUTOFF_HZ: f32 = 5.0;

    /// Creates a DC blocker for the given sample rate.
    pub fn new(sample_rate: f32) -> Self {
        let mut blocker = Self { r: 0.0, x1: 0.0, y1: 0.0 };
        blocker.set_sample_rate(sample_rate);
        blocker
    }

    /// Recomputes the pole for a new sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.r = (-std::f32::consts::TAU * Self::CUTOFF_HZ / sample_rate.max(1.0)).exp();
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = flush(y);
        y
    }

    /// Clears the filter memory.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// Running minimum over the last `window` values (a monotonic queue).
///
/// The queue keeps only values that could still become the minimum: each new
/// value evicts every larger value behind it, so the front is always the
/// minimum and each value is pushed and popped at most once (O(1) amortised).
#[derive(Debug, Clone)]
struct SlidingMin {
    values: Vec<f32>,
    stamps: Vec<u64>,
    head: usize,
    len: usize,
    window: u64,
    now: u64,
}

impl SlidingMin {
    fn new(window: usize) -> Self {
        let window = window.max(1);
        Self {
            values: vec![0.0; window],
            stamps: vec![0; window],
            head: 0,
            len: 0,
            window: window as u64,
            now: 0,
        }
    }

    /// Adds a value and returns the minimum of the last `window` values.
    #[inline]
    fn push(&mut self, value: f32) -> f32 {
        let capacity = self.values.len();

        // Drop the front once it has slid out of the window
        if self.len > 0 && self.stamps[self.head] + self.window <= self.now {
            self.head = (self.head + 1) % capacity;
            self.len -= 1;
        }
        // Values at the back that are no smaller can never be the minimum again
        while self.len > 0 {
            let back = (self.head + self.len - 1) % capacity;
            if self.values[back] < value {
                break;
            }
            self.len -= 1;
        }
        let slot = (self.head + self.len) % capacity;
        self.values[slot] = value;
        self.stamps[slot] = self.now;
        self.len += 1;
        self.now += 1;

        self.values[self.head]
    }

    fn reset(&mut self) {
        self.head = 0;
        self.len = 0;
        self.now = 0;
    }
}

/// Stereo-linked lookahead peak limiter.
///
/// The audio is delayed by the attack time, which lets the gain start falling
/// *before* a peak arrives, so peaks are caught without clipping them:
///
/// 1. **Required gain**: `ceiling / peak` for each incoming sample (1.0 when
///    under the ceiling), linked across both channels so the image doesn't shift.
/// 2. **Hold**: the minimum required gain over the attack window, so a peak is
///    seen for the whole time it takes to ramp down to it.
/// 3. **Release**: gain recovers with a program-dependent time constant. Brief
///    transients release quickly (no audible ducking); sustained limiting
///    releases slowly (no pumping or low-frequency distortion).
/// 4. **Smooth**: a moving average over the attack window turns the stepped
///    hold into a straight ramp lasting exactly the attack time.
///
/// Every value averaged in step 4 is already at or below the gain the delayed
/// sample needs, so the output can never exceed the ceiling. A final clamp
/// only guards against floating-point rounding.
#[derive(Debug, Clone)]
pub struct PeakLimiter {
    sample_rate: f32,
    /// Linear output ceiling.
    ceiling: f32,
    /// Lookahead delay lines, one per channel (`lookahead` samples long).
    delay: [Vec<f32>; 2],
    /// Write position shared by both delay lines and the average window.
    pos: usize,
    /// Step 2: windowed minimum of the required gain.
    hold: SlidingMin,
    /// Step 3: released gain envelope.
    envelope: f32,
    /// Slow average of how hard the limiter is working (0 = idle).
    sustain: f32,
    /// One-pole coefficients for the fast and slow release, and for `sustain`.
    release_fast: f32,
    release_slow: f32,
    sustain_coeff: f32,
    /// Step 4: moving-average window of the envelope, with its running sum.
    average: Vec<f32>,
    average_sum: f64,
    /// Smallest gain applied since the last call to `take_min_gain`.
    min_gain: f32,
}

impl PeakLimiter {
    /// Default output ceiling, -0.3 dBFS: headroom for inter-sample peaks and
    /// lossy encoders, while staying effectively at full scale.
    pub const DEFAULT_CEILING_DB: f32 = -0.3;
    /// Attack (and lookahead) time in milliseconds.
    pub const ATTACK_MS: f32 = 1.0;
    /// Release time for isolated transients.
    pub const RELEASE_FAST_MS: f32 = 40.0;
    /// Release time under sustained limiting.
    pub const RELEASE_SLOW_MS: f32 = 400.0;
    /// How long the limiter has to keep working before release slows down.
    const SUSTAIN_MS: f32 = 250.0;
    /// Gain-reduction depth (linear, about -3 dB) that selects the slow release.
    const SUSTAIN_FULL: f32 = 0.3;

    /// Creates a limiter with the default ceiling.
    pub fn new(sample_rate: f32) -> Self {
        let mut limiter = Self {
            sample_rate,
            ceiling: db_to_gain(Self::DEFAULT_CEILING_DB),
            delay: [Vec::new(), Vec::new()],
            pos: 0,
            hold: SlidingMin::new(1),
            envelope: 1.0,
            sustain: 0.0,
            release_fast: 0.0,
            release_slow: 0.0,
            sustain_coeff: 0.0,
            average: Vec::new(),
            average_sum: 0.0,
            min_gain: 1.0,
        };
        limiter.set_sample_rate(sample_rate);
        limiter
    }

    /// Re-sizes the lookahead for a new sample rate (allocates; not real-time
    /// safe) and clears all state.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        let lookahead = self.lookahead_for(self.sample_rate);

        for line in &mut self.delay {
            line.clear();
            line.resize(lookahead, 0.0);
        }
        self.average.clear();
        self.average.resize(lookahead, 1.0);
        self.hold = SlidingMin::new(lookahead);

        self.release_fast = one_pole_coeff(Self::RELEASE_FAST_MS, self.sample_rate);
        self.release_slow = one_pole_coeff(Self::RELEASE_SLOW_MS, self.sample_rate);
        self.sustain_coeff = one_pole_coeff(Self::SUSTAIN_MS, self.sample_rate);
        self.reset();
    }

    fn lookahead_for(&self, sample_rate: f32) -> usize {
        ((Self::ATTACK_MS * 0.001 * sample_rate).round() as usize).max(1)
    }

    /// Delay a limiter at `sample_rate` adds, in samples: what
    /// [`latency`](Self::latency) reports, without building one.
    pub fn latency_at(sample_rate: f32) -> usize {
        ((Self::ATTACK_MS * 0.001 * sample_rate.max(1.0)).round() as usize).max(1) - 1
    }

    /// Sets the output ceiling in dBFS.
    pub fn set_ceiling_db(&mut self, ceiling_db: f32) {
        self.ceiling = db_to_gain(ceiling_db);
    }

    /// The linear output ceiling.
    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// Delay the limiter adds, in samples.
    pub fn latency(&self) -> usize {
        self.delay[0].len() - 1
    }

    /// Processes one stereo frame.
    ///
    /// With `enabled` false the audio still passes through the lookahead delay
    /// (so latency doesn't jump when the limiter is toggled) but no gain is
    /// applied. The detector keeps running, so switching on mid-signal still
    /// catches the very next peak.
    #[inline]
    pub fn process(&mut self, left: f32, right: f32, enabled: bool) -> (f32, f32) {
        // 1. Required gain for the incoming (undelayed) frame
        let peak = left.abs().max(right.abs());
        let required = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };

        // 2. Hold it across the lookahead window
        let held = self.hold.push(required);

        // 3. Instant attack, program-dependent release
        let depth = 1.0 - held;
        self.sustain += (depth - self.sustain) * self.sustain_coeff;
        if held < self.envelope {
            self.envelope = held;
        } else {
            let slow = (self.sustain / Self::SUSTAIN_FULL).min(1.0);
            let coeff = self.release_fast + (self.release_slow - self.release_fast) * slow;
            self.envelope += (held - self.envelope) * coeff;
        }
        self.sustain = flush(self.sustain);

        // 4. Moving average over the window: a linear ramp into each peak
        let len = self.average.len();
        self.average_sum += self.envelope as f64 - self.average[self.pos] as f64;
        self.average[self.pos] = self.envelope;
        let gain = (self.average_sum / len as f64) as f32;

        // Delay the audio by the same window. After writing, the oldest sample
        // (`len - 1` frames ago) sits at the next position.
        self.delay[0][self.pos] = left;
        self.delay[1][self.pos] = right;
        self.pos = (self.pos + 1) % len;
        let delayed_left = self.delay[0][self.pos];
        let delayed_right = self.delay[1][self.pos];

        if !enabled {
            return (delayed_left, delayed_right);
        }

        self.min_gain = self.min_gain.min(gain);
        let ceiling = self.ceiling;
        (
            (delayed_left * gain).clamp(-ceiling, ceiling),
            (delayed_right * gain).clamp(-ceiling, ceiling),
        )
    }

    /// Returns the smallest gain applied since the last call (1.0 = no
    /// limiting) and starts a new measurement.
    pub fn take_min_gain(&mut self) -> f32 {
        std::mem::replace(&mut self.min_gain, 1.0)
    }

    /// Clears the delay lines and envelopes.
    pub fn reset(&mut self) {
        for line in &mut self.delay {
            line.fill(0.0);
        }
        self.average.fill(1.0);
        self.average_sum = self.average.len() as f64;
        self.hold.reset();
        self.pos = 0;
        self.envelope = 1.0;
        self.sustain = 0.0;
        self.min_gain = 1.0;
    }
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
fn one_pole_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    rate_for_seconds(0.001 * time_ms, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, amplitude: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| amplitude * (std::f32::consts::TAU * freq * n as f32 / SR).sin())
            .collect()
    }

    #[test]
    fn test_dc_offset_decays_to_zero() {
        let mut blocker = DcBlocker::new(SR);
        let mut last = 1.0;
        for _ in 0..SR as usize {
            last = blocker.process(0.5);
        }
        assert!(last.abs() < 1e-4, "DC should be gone after 1 s, got {}", last);
    }

    #[test]
    fn test_dc_blocker_passes_audio() {
        // 100 Hz is well above the 5 Hz corner: level within 0.05 dB
        let mut blocker = DcBlocker::new(SR);
        let input = sine(100.0, 0.5, SR as usize);
        let output: Vec<f32> = input.iter().map(|&x| blocker.process(x)).collect();
        let settled = &output[SR as usize / 2..];
        let peak = settled.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.003, "100 Hz peak {}", peak);
    }

    #[test]
    fn test_sliding_min_matches_brute_force() {
        let window = 7;
        let mut min = SlidingMin::new(window);
        let values: Vec<f32> = (0..200).map(|n| ((n * 37 % 23) as f32).sin()).collect();
        for (n, &v) in values.iter().enumerate() {
            let got = min.push(v);
            let start = n.saturating_sub(window - 1);
            let want = values[start..=n].iter().copied().fold(f32::INFINITY, f32::min);
            assert_eq!(got, want, "at sample {}", n);
        }
    }

    #[test]
    fn test_limiter_never_exceeds_ceiling_at_plus_12_db() {
        let mut limiter = PeakLimiter::new(SR);
        let ceiling = limiter.ceiling();
        let loud = db_to_gain(12.0);

        // A sine, a full-scale square burst and isolated spikes, all at +12 dB
        let mut input = sine(220.0, loud, SR as usize / 2);
        input.extend((0..4800).map(|n| if (n / 50) % 2 == 0 { loud } else { -loud }));
        input.extend((0..4800).map(|n| if n % 997 == 0 { loud } else { 0.01 }));

        for (n, &x) in input.iter().enumerate() {
            let (l, r) = limiter.process(x, -x * 0.5, true);
            assert!(l.abs() <= ceiling, "sample {}: {} over ceiling {}", n, l, ceiling);
            assert!(r.abs() <= ceiling, "sample {}: {} over ceiling {}", n, r, ceiling);
        }
        let gain = limiter.take_min_gain();
        assert!(gain < 0.3, "+12 dB needs about -12 dB of reduction, got {}", gain);
    }

    #[test]
    fn test_limiter_is_transparent_below_ceiling() {
        let mut limiter = PeakLimiter::new(SR);
        let latency = limiter.latency();
        let input = sine(440.0, 0.5, 4800);
        let output: Vec<f32> = input.iter().map(|&x| limiter.process(x, x, true).0).collect();
        for n in latency..output.len() {
            assert_eq!(output[n], input[n - latency], "sample {} changed", n);
        }
        assert_eq!(limiter.take_min_gain(), 1.0);
    }

    #[test]
    fn test_limiter_catches_peak_without_clipping_it() {
        // The gain must already be down when the peak leaves the delay line,
        // i.e. the waveform is scaled rather than flattened
        let mut limiter = PeakLimiter::new(SR);
        let latency = limiter.latency();
        let mut input = vec![0.1; 2000];
        input[1000] = 2.0;
        let output: Vec<f32> = input.iter().map(|&x| limiter.process(x, x, true).0).collect();

        let peak_out = output[1000 + latency];
        assert!((peak_out - limiter.ceiling()).abs() < 1e-3, "peak lands on the ceiling: {}", peak_out);
        // The ramp starts before the peak: the sample just ahead is already turned down
        assert!(output[999 + latency] < 0.1);
    }

    #[test]
    fn test_release_is_program_dependent() {
        // Time to recover to -1 dB after the overload stops
        fn recovery_samples(overload_frames: usize) -> usize {
            let mut limiter = PeakLimiter::new(SR);
            for x in sine(100.0, 4.0, overload_frames) {
                limiter.process(x, x, true);
            }
            let target = db_to_gain(-1.0);
            let mut n = 0;
            loop {
                limiter.process(0.1, 0.1, true);
                if limiter.take_min_gain() >= target || n > SR as usize * 5 {
                    return n;
                }
                n += 1;
            }
        }

        let after_transient = recovery_samples(240); // 5 ms burst
        let after_sustained = recovery_samples(SR as usize * 2); // 2 s of overload
        assert!(
            after_sustained > after_transient * 3,
            "sustained limiting should release slower: {} vs {} samples",
            after_sustained,
            after_transient
        );
    }

    #[test]
    fn test_disabled_limiter_only_delays() {
        let mut limiter = PeakLimiter::new(SR);
        let latency = limiter.latency();
        let input: Vec<f32> = (0..500).map(|n| n as f32 * 0.01).collect();
        let output: Vec<f32> = input.iter().map(|&x| limiter.process(x, x, false).0).collect();
        for n in latency..output.len() {
            assert_eq!(output[n], input[n - latency]);
        }
    }

    #[test]
    fn test_lookahead_is_about_one_ms() {
        for &sr in &[44_100.0, 48_000.0, 96_000.0] {
            let limiter = PeakLimiter::new(sr);
            let expected = (sr * 0.001).round() as usize - 1;
            assert_eq!(limiter.latency(), expected);
            assert_eq!(PeakLimiter::latency_at(sr), expected, "the same without building one");
        }
    }
}
