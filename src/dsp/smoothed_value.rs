//! Smoothed parameter values for click-free audio.
//!
//! Provides exponential smoothing for parameters to prevent audible clicks
//! and zipper noise when values change rapidly (from knob movements, automation, etc.).
//!
//! The first target a value is given is where it starts, not where it glides
//! to. A module builds its smoothers at its defaults before it knows the
//! patch's settings, and a Level saved at 0 must not fade down from the
//! default 0.5 for the first few milliseconds of every load.

/// A value that smoothly interpolates toward a target.
///
/// Uses exponential smoothing (one-pole lowpass) to create natural-feeling
/// parameter transitions. The smoothing factor is calculated from a time
/// constant that defines how quickly the value reaches its target.
///
/// The value it's created with is a stand-in until the first
/// [`set_target`](Self::set_target), which jumps straight there. Only later
/// targets glide.
///
/// # Example
///
/// ```ignore
/// let mut freq = SmoothedValue::new(440.0, 10.0, 44100.0);
///
/// // The first target is where it starts: the patch's setting
/// freq.set_target(330.0);
///
/// // Later ones glide, as when a knob turns
/// freq.set_target(880.0);
///
/// // In the audio loop, get smoothed values sample-by-sample
/// for i in 0..block_size {
///     let smoothed_freq = freq.next();
///     // Use smoothed_freq for oscillator...
/// }
/// ```
#[derive(Clone, Debug)]
pub struct SmoothedValue {
    /// Current smoothed value.
    current: f32,
    /// Target value we're smoothing toward.
    target: f32,
    /// Smoothing coefficient (0-1). Higher = slower smoothing.
    smoothing_factor: f32,
    /// Sample rate, stored for recalculating coefficient.
    sample_rate: f32,
    /// Time constant in milliseconds.
    time_constant_ms: f32,
    /// Whether a target has arrived yet. Until one does, the value is only
    /// the stand-in it was created with, and the first target is jumped to.
    has_target: bool,
}

impl SmoothedValue {
    /// Default smoothing time constant in milliseconds.
    /// 10ms provides a good balance between responsiveness and smoothness.
    pub const DEFAULT_TIME_CONSTANT_MS: f32 = 10.0;

    /// Creates a new smoothed value.
    ///
    /// # Arguments
    ///
    /// * `initial` - Starting value (both current and target), until the
    ///   first [`set_target`](Self::set_target) replaces it outright
    /// * `time_constant_ms` - Time in milliseconds to reach ~63% of target (one time constant)
    /// * `sample_rate` - Audio sample rate in Hz
    pub fn new(initial: f32, time_constant_ms: f32, sample_rate: f32) -> Self {
        let smoothing_factor = Self::calc_smoothing_factor(time_constant_ms, sample_rate);
        Self {
            current: initial,
            target: initial,
            smoothing_factor,
            sample_rate,
            time_constant_ms,
            has_target: false,
        }
    }

    /// Creates a smoothed value with the default time constant (10ms).
    pub fn with_default_smoothing(initial: f32, sample_rate: f32) -> Self {
        Self::new(initial, Self::DEFAULT_TIME_CONSTANT_MS, sample_rate)
    }

    /// Calculates the smoothing factor from time constant and sample rate.
    ///
    /// Uses the formula: factor = exp(-1 / (time_constant * sample_rate))
    /// This creates an exponential curve where the value reaches ~63% of the
    /// target after one time constant.
    fn calc_smoothing_factor(time_constant_ms: f32, sample_rate: f32) -> f32 {
        if time_constant_ms <= 0.0 || sample_rate <= 0.0 {
            return 0.0; // Instant (no smoothing)
        }
        let time_constant_samples = time_constant_ms * 0.001 * sample_rate;
        if time_constant_samples < 1.0 {
            return 0.0; // Instant
        }
        (-1.0 / time_constant_samples).exp()
    }

    /// Sets a new target value to smooth toward.
    ///
    /// The first target is jumped to instead: it's the setting the value
    /// should have had from the start, such as a knob's saved position.
    #[inline]
    pub fn set_target(&mut self, value: f32) {
        if !self.has_target {
            self.current = value;
            self.has_target = true;
        }
        self.target = value;
    }

    /// Gets the current target value.
    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Gets the current smoothed value without advancing.
    #[inline]
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Advances the smoothing by one sample and returns the new value.
    ///
    /// Call this once per sample in your audio processing loop.
    #[inline]
    #[allow(clippy::should_implement_trait)] // never ends, so not an Iterator
    pub fn next(&mut self) -> f32 {
        // Exponential smoothing: current = target + factor * (current - target)
        // This is mathematically equivalent to: current += (1 - factor) * (target - current)
        // Snap to target when very close to avoid floating-point accumulation issues
        // and skip unnecessary computation when already converged.
        // Threshold of 1e-4 accounts for f32 precision limits in the smoothing calculation.
        let diff = self.current - self.target;
        let next = self.target + self.smoothing_factor * diff;
        // Also snap when a step rounds to nothing: with a slow time constant
        // on a large value (a 645 ms delay time) the step falls below half an
        // f32 ulp long before the 1e-4 threshold, and the value would stall
        // short of its target for good.
        if diff.abs() <= 1e-4 || next == self.current {
            self.current = self.target;
        } else {
            self.current = next;
        }
        self.current
    }

    /// Sets the value immediately without smoothing.
    ///
    /// This doesn't count as the first target: a module resetting its
    /// smoothers before it has run still jumps to the patch's settings.
    ///
    /// Use this for:
    /// - Initial setup
    /// - Resetting after note-off
    /// - Discrete parameter changes (like waveform selection)
    #[inline]
    pub fn set_immediate(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    /// Returns true if the current value is very close to the target.
    ///
    /// Useful for optimization: skip smoothing calculations when settled.
    /// Uses a threshold of 1e-4 to match the snap threshold in next().
    #[inline]
    pub fn is_smoothing(&self) -> bool {
        (self.current - self.target).abs() > 1e-4
    }

    /// Updates the sample rate and recalculates the smoothing factor.
    ///
    /// Call this in your module's `prepare()` method.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.smoothing_factor = Self::calc_smoothing_factor(self.time_constant_ms, sample_rate);
    }

    /// Updates the time constant and recalculates the smoothing factor.
    pub fn set_time_constant(&mut self, time_constant_ms: f32) {
        self.time_constant_ms = time_constant_ms;
        self.smoothing_factor = Self::calc_smoothing_factor(time_constant_ms, self.sample_rate);
    }

    /// Resets to a value immediately (alias for set_immediate for clarity).
    #[inline]
    pub fn reset(&mut self, value: f32) {
        self.set_immediate(value);
    }
}

impl Default for SmoothedValue {
    fn default() -> Self {
        Self::new(0.0, Self::DEFAULT_TIME_CONSTANT_MS, 44100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value that has had its first target, so the next one glides.
    fn gliding_from(start: f32, time_constant_ms: f32, sample_rate: f32) -> SmoothedValue {
        let mut sv = SmoothedValue::new(0.0, time_constant_ms, sample_rate);
        sv.set_target(start);
        sv
    }

    #[test]
    fn test_initial_value() {
        let sv = SmoothedValue::new(440.0, 10.0, 44100.0);
        assert_eq!(sv.current(), 440.0);
        assert_eq!(sv.target(), 440.0);
    }

    #[test]
    fn test_set_target() {
        let mut sv = gliding_from(440.0, 10.0, 44100.0);
        sv.set_target(880.0);
        assert_eq!(sv.target(), 880.0);
        assert_eq!(sv.current(), 440.0); // Current unchanged until next()
    }

    #[test]
    fn test_smoothing_approaches_target() {
        let mut sv = gliding_from(0.0, 10.0, 44100.0);
        sv.set_target(1.0);

        // After many samples, should approach target
        for _ in 0..4410 {
            // 100ms worth
            sv.next();
        }

        // Should be very close to 1.0 after 10x the time constant
        assert!(
            (sv.current() - 1.0).abs() < 0.001,
            "Expected ~1.0, got {}",
            sv.current()
        );
    }

    #[test]
    fn test_smoothing_is_gradual() {
        let mut sv = gliding_from(0.0, 10.0, 44100.0);
        sv.set_target(1.0);

        let first = sv.next();
        let second = sv.next();
        let third = sv.next();

        // Values should increase gradually
        assert!(first > 0.0);
        assert!(second > first);
        assert!(third > second);

        // But not jump to target
        assert!(third < 0.5);
    }

    #[test]
    fn test_set_immediate() {
        let mut sv = SmoothedValue::new(0.0, 10.0, 44100.0);
        sv.set_immediate(1.0);

        assert_eq!(sv.current(), 1.0);
        assert_eq!(sv.target(), 1.0);
        assert!(!sv.is_smoothing());
    }

    #[test]
    fn test_is_smoothing() {
        let mut sv = gliding_from(0.0, 10.0, 44100.0);

        // Initially not smoothing (current == target)
        assert!(!sv.is_smoothing());

        // After setting target, should be smoothing
        sv.set_target(1.0);
        assert!(sv.is_smoothing());

        // After reaching target, should stop smoothing
        for _ in 0..44100 {
            sv.next();
        }
        assert!(!sv.is_smoothing());
    }

    #[test]
    fn test_zero_time_constant_is_instant() {
        let mut sv = gliding_from(0.0, 0.0, 44100.0);
        sv.set_target(1.0);
        sv.next();

        // With zero time constant, should jump to target immediately
        assert_eq!(sv.current(), 1.0);
    }

    #[test]
    fn test_slow_smoothing_of_a_large_value_reaches_its_target() {
        // A 250 ms glide from 500 to 645.16 used to stall about 0.4 short,
        // where each step rounded away to nothing
        let mut sv = gliding_from(500.0, 250.0, 48000.0);
        sv.set_target(645.161_3);
        for _ in 0..48000 * 5 {
            sv.next();
        }
        assert_eq!(sv.current(), sv.target());
    }

    #[test]
    fn test_sample_rate_update() {
        let mut sv = gliding_from(0.0, 10.0, 44100.0);
        sv.set_sample_rate(48000.0);

        // Smoothing should still work correctly at new sample rate
        sv.set_target(1.0);
        for _ in 0..4800 {
            // ~100ms at 48kHz
            sv.next();
        }
        assert!(
            (sv.current() - 1.0).abs() < 0.001,
            "Smoothing failed after sample rate change"
        );
    }

    #[test]
    fn test_time_constant_update() {
        let mut sv = gliding_from(0.0, 10.0, 44100.0);
        sv.set_time_constant(5.0); // Faster smoothing

        sv.set_target(1.0);
        for _ in 0..2205 {
            // ~50ms at 44.1kHz
            sv.next();
        }
        // With 5ms time constant, should be close to target after 50ms (~10 time constants)
        assert!(
            (sv.current() - 1.0).abs() < 0.001,
            "Fast smoothing failed"
        );
    }

    #[test]
    fn test_downward_smoothing() {
        let mut sv = gliding_from(1.0, 10.0, 44100.0);
        sv.set_target(0.0);

        let first = sv.next();
        let second = sv.next();

        // Values should decrease gradually
        assert!(first < 1.0);
        assert!(second < first);
        assert!(second > 0.5); // Not too fast
    }

    #[test]
    fn test_reset() {
        let mut sv = gliding_from(0.5, 10.0, 44100.0);
        sv.set_target(1.0);
        sv.next();
        sv.next();

        sv.reset(0.0);
        assert_eq!(sv.current(), 0.0);
        assert_eq!(sv.target(), 0.0);
    }

    #[test]
    fn test_default() {
        let sv = SmoothedValue::default();
        assert_eq!(sv.current(), 0.0);
        assert_eq!(sv.target(), 0.0);
    }

    #[test]
    fn test_with_default_smoothing() {
        let sv = SmoothedValue::with_default_smoothing(440.0, 44100.0);
        assert_eq!(sv.current(), 440.0);
        assert_eq!(sv.time_constant_ms, SmoothedValue::DEFAULT_TIME_CONSTANT_MS);
    }

    #[test]
    fn test_smoothed_value_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SmoothedValue>();
    }

    #[test]
    fn test_exponential_curve_shape() {
        // Verify the smoothing follows an exponential curve
        let mut sv = gliding_from(0.0, 10.0, 44100.0);
        sv.set_target(1.0);

        // After one time constant (~441 samples), should reach ~63% of target
        for _ in 0..441 {
            sv.next();
        }
        let after_one_tc = sv.current();

        // Should be around 0.632 (1 - e^-1)
        assert!(
            (after_one_tc - 0.632).abs() < 0.05,
            "After one time constant, expected ~0.632, got {}",
            after_one_tc
        );
    }

    #[test]
    fn test_first_target_is_jumped_to() {
        // A Level saved at 0 is silent from the first sample, not a fade
        // down from the default
        let mut sv = SmoothedValue::new(0.5, 10.0, 44100.0);
        sv.set_target(0.0);
        assert_eq!(sv.current(), 0.0);
        assert_eq!(sv.next(), 0.0);
        assert!(!sv.is_smoothing());
    }

    #[test]
    fn test_later_targets_glide() {
        // Turning the knob afterwards is still smoothed, even before the
        // value has been heard
        let mut sv = SmoothedValue::new(0.5, 10.0, 44100.0);
        sv.set_target(0.0);
        sv.set_target(1.0);
        assert_eq!(sv.current(), 0.0);
        let first = sv.next();
        assert!(first > 0.0 && first < 0.01, "{first}");
    }

    #[test]
    fn test_reset_before_the_first_target_still_jumps() {
        // Modules reset their smoothers to their targets; done before the
        // patch's settings arrive, that mustn't count as the first target
        let mut sv = SmoothedValue::new(0.5, 10.0, 44100.0);
        sv.reset(sv.target());
        sv.set_target(0.0);
        assert_eq!(sv.current(), 0.0);
    }
}
