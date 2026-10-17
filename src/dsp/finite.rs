//! Keeping NaN and infinity out of the patch.
//!
//! One non-finite sample reaching a filter, an EQ or a delay's feedback
//! latches there: every sample after it is NaN, and so is everything
//! downstream. Two layers keep that from happening:
//!
//! - What comes in from outside is made finite before any module hears it:
//!   a decoded sound file with [`sanitize`], the input device with [`finite`].
//! - After every module runs, the engine checks its outputs with
//!   [`all_finite`]. A module that made a NaN is silenced and reset for that
//!   block, and the editor is told which one it was.

/// The loudest sample let in from outside: +24 dBFS. A float WAV may hold
/// peaks past full scale, but nothing real is this loud, and anything louder
/// (a corrupt file's 1e30) would only overflow the patch.
pub const SAMPLE_CEILING: f32 = 16.0;

/// The sample, held within ±[`SAMPLE_CEILING`], or silence if it is NaN or
/// infinite.
#[inline]
pub fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-SAMPLE_CEILING, SAMPLE_CEILING)
    } else {
        0.0
    }
}

/// The sample, or silence if it is NaN or infinite. For the input device,
/// which can't go past full scale but may hand over garbage.
#[inline]
pub fn finite(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

/// [`sanitize`]s every sample in place.
pub fn sanitize_all(samples: &mut [f32]) {
    for sample in samples {
        *sample = sanitize(*sample);
    }
}

/// Whether every sample is finite.
///
/// Sums each sample times zero, which is zero for any finite sample and NaN
/// for NaN or infinity, eight lanes at a time so it vectorises: about one
/// multiply-add per sample, with no branch.
///
/// REAL-TIME SAFE.
#[inline]
pub fn all_finite(samples: &[f32]) -> bool {
    let mut lanes = [0.0f32; 8];
    let chunks = samples.chunks_exact(8);
    let rest = chunks.remainder();
    for chunk in chunks {
        for (lane, sample) in lanes.iter_mut().zip(chunk) {
            *lane += sample * 0.0;
        }
    }
    let tail: f32 = rest.iter().map(|sample| sample * 0.0).sum();
    lanes.iter().sum::<f32>() + tail == 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_silences_nan_and_infinity_and_clamps_the_rest() {
        assert_eq!(sanitize(f32::NAN), 0.0);
        assert_eq!(sanitize(f32::INFINITY), 0.0);
        assert_eq!(sanitize(f32::NEG_INFINITY), 0.0);
        assert_eq!(sanitize(1e30), SAMPLE_CEILING);
        assert_eq!(sanitize(-1e30), -SAMPLE_CEILING);
        assert_eq!(sanitize(1.5), 1.5);
        assert_eq!(sanitize(-0.25), -0.25);
        assert_eq!(finite(f32::NAN), 0.0);
        assert_eq!(finite(1e30), 1e30);
    }

    #[test]
    fn all_finite_finds_one_bad_sample_anywhere() {
        let mut block = vec![0.5f32; 131];
        assert!(all_finite(&block));
        assert!(all_finite(&[]));
        // Huge but finite is still finite: no overflow from the sum
        block[3] = f32::MAX;
        block[4] = f32::MAX;
        assert!(all_finite(&block));
        for at in [0, 7, 64, 127, 130] {
            for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut block = vec![0.5f32; 131];
                block[at] = bad;
                assert!(!all_finite(&block), "missed {bad} at {at}");
            }
        }
    }
}
