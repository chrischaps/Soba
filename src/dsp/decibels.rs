//! Decibels and linear gain, the one pair of conversions every module uses.
//!
//! Both are plain `f32` arithmetic with no allocation, so they are safe on the
//! audio thread.

/// Converts decibels to a linear gain.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Converts a linear gain (or amplitude) to decibels, floored at -200 dB so
/// silence reads as a very quiet number rather than minus infinity.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-10).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip_and_landmarks() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert!((gain_to_db(2.0) - 6.0206).abs() < 1e-3);
        for db in [-60.0, -12.5, 0.0, 9.0, 24.0] {
            assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1e-3, "{db}");
        }
    }

    #[test]
    fn test_silence_is_floored() {
        assert_eq!(gain_to_db(0.0), -200.0);
        assert_eq!(gain_to_db(-1.0), -200.0);
    }
}
