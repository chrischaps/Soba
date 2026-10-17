//! Reading gates and triggers, and sending pulses.
//!
//! Every module that listens for a clock, a reset or a gate hears it the same
//! way: high above [`GATE_THRESHOLD`], with a little hysteresis so a slow or
//! noisy CV crossing 0.5 switches once rather than chattering. And every
//! trigger a module sends is timed in seconds, so it's as long at 96 kHz as
//! at 44.1 kHz.

/// Where a gate counts as high.
pub const GATE_THRESHOLD: f32 = 0.5;

/// How far past [`GATE_THRESHOLD`] a gate must go to switch, either way.
pub const GATE_HYSTERESIS: f32 = 0.02;

/// Which way a gate just went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Rising,
    Falling,
}

/// A gate input as a Schmitt trigger: it goes high above
/// `GATE_THRESHOLD + GATE_HYSTERESIS` and low again below
/// `GATE_THRESHOLD - GATE_HYSTERESIS`, and tells you when it does.
///
/// REAL-TIME SAFE.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edge {
    high: bool,
}

impl Edge {
    /// A gate that starts low.
    pub const LOW: Self = Self { high: false };

    /// Hears one sample, and says whether the gate went up or down on it.
    #[inline]
    pub fn tick(&mut self, x: f32) -> Option<Transition> {
        if !self.high && x > GATE_THRESHOLD + GATE_HYSTERESIS {
            self.high = true;
            Some(Transition::Rising)
        } else if self.high && x < GATE_THRESHOLD - GATE_HYSTERESIS {
            self.high = false;
            Some(Transition::Falling)
        } else {
            None
        }
    }

    /// Hears one sample, and says whether the gate went up on it.
    #[inline]
    pub fn rising(&mut self, x: f32) -> bool {
        self.tick(x) == Some(Transition::Rising)
    }

    /// The gate is high.
    #[inline]
    pub fn is_high(&self) -> bool {
        self.high
    }

    /// Forgets the gate, as if it had been low.
    #[inline]
    pub fn reset(&mut self) {
        self.high = false;
    }
}

/// A trigger output: high for a set time after it fires.
///
/// REAL-TIME SAFE.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pulse {
    remaining: usize,
}

impl Pulse {
    /// A pulse that hasn't fired.
    pub const IDLE: Self = Self { remaining: 0 };

    /// Fires for `seconds` at `sample_rate`, starting on the next
    /// [`tick`](Self::tick): never shorter than one sample.
    #[inline]
    pub fn fire(&mut self, seconds: f32, sample_rate: f32) {
        self.remaining = ((seconds * sample_rate).round() as usize).max(1);
    }

    /// Whether the pulse is high on this sample, counting it off.
    #[inline]
    pub fn tick(&mut self) -> bool {
        let high = self.remaining > 0;
        self.remaining = self.remaining.saturating_sub(1);
        high
    }

    /// [`tick`](Self::tick) as a gate's value: 1.0 or 0.0.
    #[inline]
    pub fn sample(&mut self) -> f32 {
        if self.tick() {
            1.0
        } else {
            0.0
        }
    }

    /// The pulse is still high.
    #[inline]
    pub fn is_high(&self) -> bool {
        self.remaining > 0
    }

    /// Stops the pulse.
    #[inline]
    pub fn clear(&mut self) {
        self.remaining = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_gate_rises_and_falls_once() {
        let mut edge = Edge::LOW;
        let heard: Vec<_> = [0.0, 1.0, 1.0, 0.0, 0.0, 1.0].iter().map(|&x| edge.tick(x)).collect();
        assert_eq!(
            heard,
            [None, Some(Transition::Rising), None, Some(Transition::Falling), None, Some(Transition::Rising)]
        );
        assert!(edge.is_high());
        edge.reset();
        assert!(!edge.is_high());
    }

    #[test]
    fn a_noisy_crossing_switches_only_once() {
        // A slow ramp through 0.5 with noise of ±0.015 on it
        let mut edge = Edge::LOW;
        let rises = (0..1000)
            .map(|n| n as f32 / 1000.0 + if n % 2 == 0 { 0.015 } else { -0.015 })
            .filter(|&x| edge.rising(x))
            .count();
        assert_eq!(rises, 1);
    }

    #[test]
    fn a_pulse_lasts_as_long_at_any_sample_rate() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut pulse = Pulse::IDLE;
            pulse.fire(0.001, sample_rate);
            let high = (0..1000).filter(|_| pulse.tick()).count();
            assert_eq!(high, (0.001 * sample_rate as f64).round() as usize);
        }
        let mut pulse = Pulse::IDLE;
        pulse.fire(0.0, 44_100.0);
        assert_eq!(pulse.sample(), 1.0);
        assert_eq!(pulse.sample(), 0.0);
    }
}
