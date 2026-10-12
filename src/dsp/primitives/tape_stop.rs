//! Tape stop: a transport that brakes to a halt and winds back up.
//!
//! Stopping a tape machine (or a turntable) mid-song slows everything on it
//! together, so the whole sound falls in pitch to nothing. Here that's a
//! play head falling behind the record head: at speed `s` it lags a further
//! `1 − s` samples each sample, so as the brake takes the speed from 1 to 0
//! the pitch falls with it, and at 0 the sound freezes.
//!
//! Letting go winds the transport back up. The play head jumps to the record
//! head (it's silent when stopped, so the jump can't be heard), and the speed
//! rises from 0 on a curve that overshoots 1 just enough to make up the lag
//! it gathered on the way: the pitch climbs, peaks a major third high, and
//! settles exactly in time. A song on a clock comes back on the beat, with
//! no latency left behind. Let go before the tape has stopped and it's still
//! sounding, so the new play head fades in over the old one instead.

/// Below this speed the playback fades to silence, as the music sinks below
/// hearing: a stopped tape plays nothing, rather than the one sample it's on.
const FADE_BELOW_SPEED: f32 = 0.12;

/// Winding back up from a standstill takes this much of the Stop Time.
pub const WIND_UP_RATIO: f32 = 0.5;

/// How long the old play head fades out, when the tape is let go still sounding.
const CROSSFADE_SECONDS: f32 = 0.03;

/// The play head's position, and a fading one, for the next sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StopHeads {
    /// How far the play head lags the record head because of the stop, in samples.
    pub lag: f32,
    /// The old play head's lag, while it fades out after a release.
    pub old_lag: f32,
    /// How much of the old play head is heard: 0 when there is none.
    pub old_weight: f32,
    /// The playback's gain: 1 at speed, fading to 0 as the tape stops.
    pub gain: f32,
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    /// At speed, the play head on the record head.
    Running,
    /// Braking at a steady rate: the speed falls to 0 over the Stop Time.
    Stopping,
    /// Winding up: the speed follows `s0 + a·u + b·u² + c·u³` over `len`
    /// samples, `u` running 0 to 1, from a lag of `from`.
    WindingUp { n: u32, len: u32, from: f32, s0: f32, a: f32, b: f32, c: f32 },
}

/// The stop's transport, shared by every track of a tape.
#[derive(Clone, Debug)]
pub struct TapeStop {
    sample_rate: f32,
    phase: Phase,
    speed: f32,
    lag: f32,
    /// The most the play head can lag, in samples: the room on the tape.
    max_lag: f32,
    /// The old play head lags the new one by this much while it fades.
    old_offset: f32,
    fade_left: u32,
    fade_len: u32,
    gate: bool,
}

impl TapeStop {
    /// A stop with room on the tape for a lag of `max_lag_seconds`.
    pub fn new(sample_rate: f32, max_lag_seconds: f32) -> Self {
        Self {
            sample_rate,
            phase: Phase::Running,
            speed: 1.0,
            lag: 0.0,
            max_lag: max_lag_seconds * sample_rate,
            old_offset: 0.0,
            fade_left: 0,
            fade_len: 0,
            gate: false,
        }
    }

    /// The longest lag a stop of `stop_seconds` builds, with the wind-up's.
    pub fn lag_needed(stop_seconds: f32) -> f32 {
        // Braking from 1 to 0 at a steady rate lags half the stop time; the
        // wind-up's hump adds about a tenth of its own length
        0.5 * stop_seconds + 0.11 * WIND_UP_RATIO * stop_seconds
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32, max_lag_seconds: f32) {
        self.sample_rate = sample_rate;
        self.max_lag = max_lag_seconds * sample_rate;
        self.reset();
    }

    /// The transport's speed now: 1 at rest, 0 stopped.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// True when the play head is on the record head and nothing is fading:
    /// the stop adds nothing to the sound.
    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running) && self.fade_left == 0
    }

    /// The gain of playback at a speed.
    #[inline]
    fn gain_at(speed: f32) -> f32 {
        if speed >= FADE_BELOW_SPEED {
            1.0
        } else {
            let t = speed.max(0.0) / FADE_BELOW_SPEED;
            t * t * (3.0 - 2.0 * t)
        }
    }

    /// Advances one sample, with the Stop gate `gate` and a Stop Time of
    /// `stop_seconds`, and returns where the play heads are.
    #[inline]
    pub fn next(&mut self, gate: bool, stop_seconds: f32) -> StopHeads {
        if gate != self.gate {
            self.gate = gate;
            if gate {
                self.phase = Phase::Stopping;
            } else {
                self.release(stop_seconds);
            }
        }

        match self.phase {
            Phase::Running => {}
            Phase::Stopping => {
                let before = self.speed;
                self.speed = (self.speed - 1.0 / (stop_seconds * self.sample_rate)).max(0.0);
                // Trapezoidal, so the lag follows the speed's straight line exactly
                self.lag = (self.lag + 1.0 - 0.5 * (before + self.speed)).clamp(0.0, self.max_lag);
            }
            Phase::WindingUp { n, len, from, s0, a, b, c } => {
                let n = n + 1;
                if n >= len {
                    self.phase = Phase::Running;
                    self.speed = 1.0;
                    self.lag = 0.0;
                } else {
                    // The lag is the speed's integral, worked out exactly so it lands on 0
                    let u = n as f32 / len as f32;
                    let travelled = u * (s0 + u * (a / 2.0 + u * (b / 3.0 + u * c / 4.0)));
                    self.lag = (from + len as f32 * (u - travelled)).clamp(0.0, self.max_lag);
                    self.speed = s0 + u * (a + u * (b + u * c));
                    self.phase = Phase::WindingUp { n, len, from, s0, a, b, c };
                }
            }
        }

        let mut old_weight = 0.0;
        if self.fade_left > 0 {
            self.fade_left -= 1;
            let done = 1.0 - self.fade_left as f32 / self.fade_len as f32;
            old_weight = 0.5 + 0.5 * (std::f32::consts::PI * done).cos();
        }
        StopHeads {
            lag: self.lag,
            old_lag: (self.lag + self.old_offset).min(self.max_lag),
            old_weight,
            gain: Self::gain_at(self.speed),
        }
    }

    /// Lets go of the brake: plan the wind-up from where the tape is now.
    fn release(&mut self, stop_seconds: f32) {
        let s0 = self.speed;
        let mut from = self.lag;
        if Self::gain_at(s0) == 0.0 {
            // Silent: the play head can jump to the record head unheard
            from = 0.0;
            self.fade_left = 0;
        } else if self.fade_left == 0 && from > 0.0 {
            // Still sounding: start a new play head on the record head, and
            // fade the old one out as both wind up together
            self.old_offset = from;
            self.fade_len = ((CROSSFADE_SECONDS * self.sample_rate) as u32).max(1);
            self.fade_left = self.fade_len;
            from = 0.0;
        }
        // (Let go again mid-fade, the play head catches up its small lag itself)

        let d = 1.0 - s0;
        let len = (WIND_UP_RATIO * stop_seconds * d.abs() * self.sample_rate).round().max(1.0);
        if d.abs() < 1e-6 && from == 0.0 {
            self.phase = Phase::Running;
            self.speed = 1.0;
            self.lag = 0.0;
            return;
        }
        // A cubic from (0, s0) to (1, 1), level at the end, travelling exactly
        // enough further than the record head to make up the lag `from`
        let e = d + from / len;
        let c = 12.0 * e - 8.0 * d;
        let b = -d - 2.0 * c;
        let a = 2.0 * d + c;
        self.lag = from;
        self.phase = Phase::WindingUp { n: 0, len: len as u32, from, s0, a, b, c };
    }

    pub fn reset(&mut self) {
        self.phase = Phase::Running;
        self.speed = 1.0;
        self.lag = 0.0;
        self.old_offset = 0.0;
        self.fade_left = 0;
        self.gate = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn run(stop: &mut TapeStop, gate: bool, seconds: f32, stop_seconds: f32) -> Vec<StopHeads> {
        (0..(seconds * SR) as usize).map(|_| stop.next(gate, stop_seconds)).collect()
    }

    #[test]
    fn test_at_rest_the_play_head_is_on_the_record_head() {
        let mut stop = TapeStop::new(SR, 3.0);
        for heads in run(&mut stop, false, 0.1, 1.0) {
            assert_eq!(heads, StopHeads { lag: 0.0, old_lag: 0.0, old_weight: 0.0, gain: 1.0 });
        }
        assert!(stop.is_running());
    }

    #[test]
    fn test_braking_slows_the_speed_in_a_straight_line() {
        let mut stop = TapeStop::new(SR, 3.0);
        let heads = run(&mut stop, true, 1.0, 1.0);
        // Halfway through a 1 s stop: half speed, and a quarter second behind
        assert!((stop.speed()).abs() < 1e-3);
        let half = heads[SR as usize / 2];
        assert!((half.lag / SR - 0.125).abs() < 1e-3, "lag {}", half.lag / SR);
        // At the end it has fallen half the stop time behind, silent
        let end = heads.last().unwrap();
        assert!((end.lag / SR - 0.5).abs() < 1e-3, "lag {}", end.lag / SR);
        assert_eq!(end.gain, 0.0);
    }

    #[test]
    fn test_winding_up_from_a_standstill_lands_back_on_the_record_head() {
        let mut stop = TapeStop::new(SR, 3.0);
        run(&mut stop, true, 1.2, 1.0);
        let heads = run(&mut stop, false, 0.6, 1.0);
        // It jumped while silent, so nothing fades
        assert!(heads.iter().all(|h| h.old_weight == 0.0));
        // The lag rises then falls back to exactly nothing
        let most = heads.iter().map(|h| h.lag).fold(0.0, f32::max);
        assert!(most > 0.0 && most < TapeStop::lag_needed(1.0) * SR);
        assert!(heads.iter().all(|h| h.lag >= 0.0));
        assert!(stop.is_running());
        assert_eq!(heads.last().unwrap().lag, 0.0);
        assert_eq!(stop.speed(), 1.0);
    }

    #[test]
    fn test_the_wind_up_overshoots_a_major_third_at_most() {
        let mut stop = TapeStop::new(SR, 3.0);
        run(&mut stop, true, 1.2, 1.0);
        let mut fastest = 0.0f32;
        for _ in 0..(0.6 * SR) as usize {
            stop.next(false, 1.0);
            fastest = fastest.max(stop.speed());
        }
        assert!(fastest > 1.2 && fastest <= 1.2501, "peak speed {fastest}");
    }

    #[test]
    fn test_letting_go_while_sounding_fades_the_old_head_out() {
        let mut stop = TapeStop::new(SR, 3.0);
        run(&mut stop, true, 0.5, 1.0);
        let lag = stop.lag;
        let first = stop.next(false, 1.0);
        assert!(first.lag < 1.0, "the new head starts on the record head");
        assert!((first.old_lag - lag).abs() < 1.0);
        assert!(first.old_weight > 0.99);
        let heads = run(&mut stop, false, 0.5, 1.0);
        let faded = heads.iter().position(|h| h.old_weight == 0.0).unwrap();
        assert!((faded as f32 / SR - CROSSFADE_SECONDS).abs() < 0.002);
        assert!(stop.is_running());
    }

    #[test]
    fn test_a_long_hold_stays_within_the_tape() {
        let mut stop = TapeStop::new(SR, 3.0);
        let heads = run(&mut stop, true, 10.0, 4.0);
        assert!(heads.iter().all(|h| h.lag <= 3.0 * SR));
        assert!(TapeStop::lag_needed(4.0) < 3.0);
    }
}
