//! The crossfade that hides a loop's seam.

/// How much of what comes before the loop's start replaces the frame `k`
/// frames from the seam: all of it at the seam, none `fade` frames away,
/// along a raised cosine. A straight line would bend the waveform at both
/// ends, which is heard.
#[inline]
pub fn seam_weight(k: f32, fade: f32) -> f32 {
    0.5 + 0.5 * (std::f32::consts::PI * (k / fade).clamp(0.0, 1.0)).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seam_weight_runs_from_all_to_none() {
        assert_eq!(seam_weight(0.0, 10.0), 1.0);
        assert!((seam_weight(5.0, 10.0) - 0.5).abs() < 1e-6);
        assert!(seam_weight(10.0, 10.0).abs() < 1e-6);
        assert!(seam_weight(20.0, 10.0).abs() < 1e-6);
    }
}
