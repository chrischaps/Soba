//! Hearing a ping come back (#449): where a sound begins in a window that
//! was quiet before it, and what a handful of them agree on.
//!
//! Measuring how long a device takes to answer a note, Soba plays it a few
//! times and listens. Unlike a recording's onsets ([`onsets`](super::onsets)),
//! each ping is a single sound into a known silence, and what matters is
//! when it starts to the sample, so the search is plainer and has no
//! filters to lag behind the sound:
//!
//! - **The quiet before** says how loud nothing is: the room's hiss, the
//!   interface's own noise, the tail of the ping before.
//! - **It came back** only if its peak stands well clear of that quiet
//!   ([`CLEAR`], 16 dB) and above [`SILENCE`]. Otherwise the ping was lost,
//!   and isn't guessed at.
//! - **Where it begins:** the first sample past a tenth of its peak (or
//!   clear of the noise, if that's higher), then back down the flank it
//!   climbed to where it left the quiet. A kick's first cycle crosses a
//!   tenth of its peak a fraction of a millisecond in; the flank finds the
//!   sample it started on.
//!
//! [`agree`] takes the delays the pings found: their median, how far apart
//! the ones that agree lie, and whether enough agree to trust. One that
//! landed far off (a stray sound, a dropped buffer) is left out; too few
//! heard, or too scattered, and the measurement gives up rather than guess.
//!
//! All of it runs on the UI side, after the pings are in.

/// The peak must stand this many times above the quiet's: 16 dB.
pub const CLEAR: f32 = 6.0;
/// Quieter than this (-60 dBFS) is nothing, however still the input.
pub const SILENCE: f32 = 0.001;
/// A sound has begun once it's past this much of its peak...
const RISE: f32 = 0.1;
/// ...and this many times the quiet's loudest.
const OVER_NOISE: f32 = 2.5;
/// Back down its flank, the sound left the quiet here, against its peak...
const FLOOR: f32 = 0.01;
/// ...or against the quiet's loudest, if higher.
const FLOOR_OVER_NOISE: f32 = 1.5;

/// Where a sound begins in `window`, which `quiet` came just before: an
/// index into `window`, or `None` if nothing rose clear of the quiet.
pub fn arrival(quiet: &[f32], window: &[f32]) -> Option<usize> {
    // An interface with a little DC reads its offset as the quiet's level
    let centre = if quiet.is_empty() { 0.0 } else { quiet.iter().sum::<f32>() / quiet.len() as f32 };
    let level = |s: f32| (s - centre).abs();
    let noise = quiet.iter().fold(0.0f32, |most, &s| most.max(level(s)));
    let peak = window.iter().fold(0.0f32, |most, &s| most.max(level(s)));
    if !(peak >= SILENCE && peak >= CLEAR * noise) {
        return None;
    }
    let mark = (RISE * peak).max(OVER_NOISE * noise);
    let first = window.iter().position(|&s| level(s) > mark)?;
    // Back down the flank, while it keeps falling and is still above the quiet
    let floor = (FLOOR * peak).max(FLOOR_OVER_NOISE * noise);
    let mut at = first;
    while at > 0 && level(window[at - 1]) > floor && level(window[at - 1]) <= level(window[at]) {
        at -= 1;
    }
    Some(at)
}

/// What the pings agree on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Agreement {
    /// The median of those that agree.
    pub median: f32,
    /// How far apart those that agree lie, earliest to latest.
    pub spread: f32,
    /// How many pings were heard.
    pub heard: usize,
    /// How many of those agree: the rest landed far off.
    pub kept: usize,
    /// How many pings there were.
    pub of: usize,
}

/// Why the pings can't be trusted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Disagreement {
    /// Too few came back clear of the noise.
    TooFewHeard { heard: usize, of: usize },
    /// They came back, but too far apart to line anything up with.
    Scattered { spread: f32, heard: usize, of: usize },
}

/// The most two agreeing pings may lie apart, in the units given.
pub const MAX_SPREAD: f32 = 5.0;
/// A ping this close to the median always agrees, however tight the rest.
const TOLERANCE: f32 = 1.0;
/// Further than this many deviations from the median is far off.
const DEVIATIONS: f32 = 3.0;

/// The fewest of `of` pings that must agree: more than half, and three.
pub fn needed(of: usize) -> usize {
    (of / 2 + 1).max(3)
}

/// What the pings' delays (`None` for one lost) agree on, in their units
/// (milliseconds, say): the median of those within a few deviations of the
/// median of all, or why they don't.
pub fn agree(delays: &[Option<f32>]) -> Result<Agreement, Disagreement> {
    let of = delays.len();
    let mut heard: Vec<f32> = delays.iter().flatten().copied().filter(|d| d.is_finite()).collect();
    let needed = needed(of);
    if heard.len() < needed {
        return Err(Disagreement::TooFewHeard { heard: heard.len(), of });
    }
    heard.sort_by(f32::total_cmp);
    let middle = median(&heard);
    let mut deviations: Vec<f32> = heard.iter().map(|d| (d - middle).abs()).collect();
    deviations.sort_by(f32::total_cmp);
    // The median absolute deviation, scaled to a normal spread's
    let deviation = 1.4826 * median(&deviations);
    let tolerance = (DEVIATIONS * deviation).max(TOLERANCE);
    let kept: Vec<f32> = heard.iter().copied().filter(|d| (d - middle).abs() <= tolerance).collect();
    let spread = kept.last().zip(kept.first()).map_or(0.0, |(last, first)| last - first);
    if kept.len() < needed || spread > MAX_SPREAD {
        return Err(Disagreement::Scattered { spread: heard[heard.len() - 1] - heard[0], heard: heard.len(), of });
    }
    Ok(Agreement { median: median(&kept), spread, heard: heard.len(), kept: kept.len(), of })
}

/// The middle of `sorted`, or the mean of its middle two.
fn median(sorted: &[f32]) -> f32 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => 0.5 * (sorted[n / 2 - 1] + sorted[n / 2]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::onsets::tests::{add_hit, Hit, Noise};
    use std::f32::consts::TAU;

    const RATE: f32 = 48000.0;

    /// Noise at `level`, `seconds` long.
    fn hiss(seconds: f32, level: f32, seed: u32) -> Vec<f32> {
        let mut noise = Noise(seed);
        (0..(seconds * RATE) as usize).map(|_| level * noise.next()).collect()
    }

    /// A tenth of a second of quiet, then half a second with `hit` at
    /// `at` seconds: the quiet and the window.
    fn ping(hit: Hit, at: f32, level: f32, hiss_level: f32) -> (Vec<f32>, Vec<f32>) {
        let quiet = hiss(0.1, hiss_level, 7);
        let mut window = hiss(0.5, hiss_level, 11);
        let mut noise = Noise(3);
        add_hit(&mut window, hit, at, level, &mut noise);
        (quiet, window)
    }

    fn ms(frames: usize) -> f32 {
        frames as f32 / RATE * 1000.0
    }

    #[test]
    fn test_a_kick_is_found_to_the_sample() {
        let (quiet, window) = ping(Hit::Kick, 0.0213, 0.5, 0.0003);
        let at = arrival(&quiet, &window).expect("heard");
        let error = ms(at) - 21.3;
        assert!(error.abs() < 0.1, "found at {:.3} ms, {error:+.3} off", ms(at));
    }

    #[test]
    fn test_a_snare_is_found_to_the_sample() {
        let (quiet, window) = ping(Hit::Snare, 0.0087, 0.3, 0.0003);
        let at = arrival(&quiet, &window).expect("heard");
        let error = ms(at) - 8.7;
        assert!(error.abs() < 0.1, "found at {:.3} ms, {error:+.3} off", ms(at));
    }

    #[test]
    fn test_a_soft_attack_is_found_within_a_millisecond() {
        // A 220 Hz tone fading in over 5 ms, at 31.25 ms
        let quiet = hiss(0.1, 0.0005, 5);
        let mut window = hiss(0.5, 0.0005, 9);
        let start = (0.03125 * RATE) as usize;
        for (n, s) in window[start..].iter_mut().enumerate().take((0.3 * RATE) as usize) {
            let t = n as f32 / RATE;
            *s += 0.4 * (t / 0.005).min(1.0) * (TAU * 220.0 * t).sin();
        }
        let at = arrival(&quiet, &window).expect("heard");
        let late = ms(at) - 31.25;
        assert!((0.0..1.0).contains(&late), "found {late:+.3} ms from where it starts");
    }

    #[test]
    fn test_dc_on_the_input_doesnt_hide_a_ping() {
        let (mut quiet, mut window) = ping(Hit::Kick, 0.015, 0.2, 0.0003);
        for s in quiet.iter_mut().chain(window.iter_mut()) {
            *s += 0.05;
        }
        let at = arrival(&quiet, &window).expect("heard over the offset");
        assert!((ms(at) - 15.0).abs() < 0.1, "{}", ms(at));
    }

    #[test]
    fn test_a_ping_lost_in_the_noise_is_not_guessed_at() {
        // Hiss at -20 dBFS; the kick peaks only 6 dB over it
        let (quiet, window) = ping(Hit::Kick, 0.02, 0.2, 0.1);
        assert_eq!(arrival(&quiet, &window), None);
    }

    #[test]
    fn test_silence_is_not_a_ping() {
        assert_eq!(arrival(&[0.0; 4800], &[0.0; 24000]), None);
        // Nor a whisper in perfect quiet
        let mut window = vec![0.0f32; 24000];
        window[1000] = 0.0005;
        assert_eq!(arrival(&[0.0; 4800], &window), None);
        assert_eq!(arrival(&[], &[]), None);
    }

    #[test]
    fn test_the_pings_agree_on_their_median() {
        let delays = [12.1, 11.9, 12.0, 12.2, 11.8, 12.0, 12.1, 11.9].map(Some);
        let agreement = agree(&delays).unwrap();
        assert!((agreement.median - 12.0).abs() < 1e-4, "{agreement:?}");
        assert!((agreement.spread - 0.4).abs() < 1e-4, "{agreement:?}");
        assert_eq!((agreement.heard, agreement.kept, agreement.of), (8, 8, 8));
    }

    #[test]
    fn test_one_far_off_is_left_out() {
        let delays = [Some(12.1), Some(11.9), Some(31.0), Some(12.0), None, Some(12.2), Some(11.8), Some(12.0)];
        let agreement = agree(&delays).unwrap();
        assert!((agreement.median - 12.0).abs() < 1e-4, "{agreement:?}");
        assert_eq!((agreement.heard, agreement.kept, agreement.of), (7, 6, 8));
        assert!(agreement.spread < 0.5);
    }

    #[test]
    fn test_too_few_heard_gives_up() {
        let delays = [Some(12.0), None, None, Some(12.1), None, None, Some(11.9), None];
        assert_eq!(agree(&delays), Err(Disagreement::TooFewHeard { heard: 3, of: 8 }));
        assert_eq!(agree(&[]), Err(Disagreement::TooFewHeard { heard: 0, of: 0 }));
        assert_eq!(agree(&[Some(1.0), Some(1.0)]), Err(Disagreement::TooFewHeard { heard: 2, of: 2 }));
    }

    #[test]
    fn test_scattered_pings_give_up() {
        let delays = [3.0, 18.0, 7.0, 11.0, 26.0, 1.0, 14.0, 22.0].map(Some);
        match agree(&delays) {
            Err(Disagreement::Scattered { heard: 8, of: 8, spread }) => assert_eq!(spread, 25.0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn test_needed_is_more_than_half() {
        assert_eq!(needed(8), 5);
        assert_eq!(needed(6), 4);
        assert_eq!(needed(3), 3);
        assert_eq!(needed(1), 3);
    }
}
