//! Finding where a recording's hits begin: its onsets, where a Sampler
//! sliced by Onsets puts its slice markers.
//!
//! **A rise in level, band by band.** The recording's mono mix is split into
//! four bands (under 150 Hz, to 1 kHz, to 4 kHz, and above), and each is
//! followed by an envelope that rises at once and falls away gently, slower
//! in the low bands so a kick's slow cycles don't ripple it. Every 2 ms, how
//! far each band has risen over the last 6 ms is added up, in dB: a hit
//! rises in several bands at once, a lot, where a ringing tail or a
//! wavering note barely rises at all. It's a rough spectral flux: a hi-hat
//! over a booming kick still stands out, in the top band. Levels more than
//! 50 dB under the recording's loudest count as silence, so the room's
//! hiss doesn't count as rising.
//!
//! **Each rise once.** A frame that rises more than any other within 30 ms
//! either side is an onset, as strong as its rise against the strongest in
//! the recording. Sensitivity then only chooses how strong an onset must be
//! to count, so turning it moves the markers at once, without measuring
//! again.
//!
//! **To the sample.** The 2 ms frame says roughly where a hit is. The
//! marker goes where the band that rose most first climbs a fifth of the
//! way from what was sounding before to the hit's peak, a millisecond
//! early, so a slice starting there keeps its whole attack.
//!
//! All of this allocates freely and runs on the UI side, never the audio
//! thread. The Dancer hears the beat with the same bands as it plays
//! (`beat_tracker.rs`).

use serde::{Deserialize, Serialize};

use super::SampleData;

/// How often the level is measured: every 2 ms.
const HOP_SECONDS: f32 = 0.002;
/// How far back a rise is measured from: 3 hops, 6 ms.
const RISE_HOPS: usize = 3;
/// Where the bands divide, in Hz.
pub(crate) const CROSSOVERS: [f64; 3] = [150.0, 1000.0, 4000.0];
/// How fast each band's envelope falls away (low to high): slow enough to
/// ride over a cycle of its lowest notes.
pub(crate) const RELEASE_SECONDS: [f64; 4] = [0.05, 0.015, 0.008, 0.005];
/// Levels this far under the loudest are silence.
pub(crate) const FLOOR_DB: f32 = 50.0;
/// Quieter than this is silence, however loud the rest is.
pub(crate) const SILENCE_DB: f32 = -90.0;
/// A rise smaller than this, all bands together, is never an onset.
pub(crate) const LEAST_RISE_DB: f32 = 6.0;
/// Two onsets are at least this far apart: 30 ms, a fast flam.
const GAP_SECONDS: f32 = 0.03;
/// Markers go this far before the hit: 1 ms.
const LEAD_SECONDS: f32 = 0.001;
/// How far a hit climbs, from what sounded before to its peak, where it begins.
const CLIMB: f32 = 0.2;

/// The Sensitivity a Sampler starts at.
pub const DEFAULT_SENSITIVITY: f32 = 0.5;

/// Slice markers placed by hand: the ones found, moved, added to or taken
/// away from. They stand at the Sensitivity they were placed at; turning it
/// finds the markers afresh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandMarkers {
    pub sensitivity: f32,
    /// Fractions of the recording, in order.
    pub places: Vec<f32>,
}

impl HandMarkers {
    /// Markers at `places`, placed at `sensitivity`, put in order.
    pub fn new(sensitivity: f32, mut places: Vec<f32>) -> Self {
        places.retain(|place| (0.0..=1.0).contains(place));
        places.sort_by(f32::total_cmp);
        places.dedup();
        Self { sensitivity, places }
    }

    /// Whether they still stand with Sensitivity at `sensitivity`.
    pub fn stand_at(&self, sensitivity: f32) -> bool {
        (self.sensitivity - sensitivity).abs() < 1e-4
    }
}

/// Where one hit begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    /// Where, as a fraction of the recording, 0 to 1.
    pub place: f32,
    /// How much it rose, against the recording's strongest onset: 0 to 1.
    pub strength: f32,
}

/// Every onset a recording has, strong and faint, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Onsets {
    found: Vec<Onset>,
}

impl Onsets {
    /// Finds the onsets in a recording.
    pub fn detect(sample: &SampleData) -> Self {
        let rate = sample.sample_rate();
        let frames = sample.frames();
        let hop = ((HOP_SECONDS * rate) as usize).max(1);
        if frames < hop * (RISE_HOPS + 2) {
            return Self::default();
        }
        let mono: Vec<f32> = sample.left().iter().zip(sample.right()).map(|(l, r)| 0.5 * (l + r)).collect();
        let bands = split(&mono, rate as f64);

        // Each band's envelope at the end of every hop, in dB
        let hops = frames / hop;
        let levels: Vec<Vec<f32>> = bands
            .iter()
            .zip(RELEASE_SECONDS)
            .map(|(band, release)| {
                let fall = (-1.0 / (release * rate as f64)).exp() as f32;
                let mut envelope = 0.0f32;
                let mut levels = Vec::with_capacity(hops);
                for chunk in band.chunks_exact(hop) {
                    for &s in chunk {
                        envelope = s.abs().max(envelope * fall);
                    }
                    levels.push(20.0 * envelope.max(1e-9).log10());
                }
                levels
            })
            .collect();
        let loudest = levels.iter().flatten().fold(f32::MIN, |a, &b| a.max(b));
        if loudest < SILENCE_DB {
            return Self::default();
        }
        let floor = (loudest - FLOOR_DB).max(SILENCE_DB);

        // How far the bands rose together, each hop; and which rose most
        let mut rise = vec![0.0f32; hops];
        let mut widest = vec![0usize; hops];
        for (k, (rise, widest)) in rise.iter_mut().zip(&mut widest).enumerate() {
            let mut most = 0.0;
            for (b, band) in levels.iter().enumerate() {
                let now = band[k].max(floor);
                // Before the recording began, it was silent
                let before = if k >= RISE_HOPS { band[k - RISE_HOPS].max(floor) } else { floor };
                let up = (now - before).max(0.0);
                *rise += up;
                if up > most {
                    most = up;
                    *widest = b;
                }
            }
        }

        // A rise is an onset if nothing within the gap rises further: the
        // first of equals, so a plateau counts once
        let gap = ((GAP_SECONDS * rate) as usize / hop).max(1);
        let peaks: Vec<usize> = (0..hops)
            .filter(|&k| {
                rise[k] >= LEAST_RISE_DB
                    && rise[k.saturating_sub(gap)..k].iter().all(|&r| r < rise[k])
                    && rise[k + 1..(k + gap + 1).min(hops)].iter().all(|&r| r <= rise[k])
            })
            .collect();
        let strongest = peaks.iter().map(|&k| rise[k]).fold(0.0f32, f32::max);

        let lead = (LEAD_SECONDS * rate) as usize;
        let found = peaks
            .into_iter()
            .map(|k| {
                let at = begins(&bands[widest[k]], k, hop).saturating_sub(lead);
                Onset { place: at as f32 / frames as f32, strength: rise[k] / strongest }
            })
            .collect();
        Self { found }
    }

    /// Where a recording is cut at `sensitivity`: at the markers placed by
    /// hand, while they stand, or else where its onsets are.
    pub fn markers(&self, hand: Option<&HandMarkers>, sensitivity: f32) -> Vec<f32> {
        match hand.filter(|hand| hand.stand_at(sensitivity)) {
            Some(hand) => hand.places.clone(),
            None => self.places(sensitivity),
        }
    }

    /// Every onset found, in order.
    pub fn all(&self) -> &[Onset] {
        &self.found
    }

    /// Where the onsets that count at `sensitivity` (0 to 1) are, as
    /// fractions of the recording, in order. At 0 only the strongest
    /// count; at 1 every rise of a few dB does.
    pub fn places(&self, sensitivity: f32) -> Vec<f32> {
        let least = least_strength(sensitivity);
        self.found.iter().filter(|onset| onset.strength >= least).map(|onset| onset.place).collect()
    }
}

/// How strong an onset must be to count at `sensitivity`: most of the
/// strongest at 0, a fifth of it halfway, anything at all at 1.
fn least_strength(sensitivity: f32) -> f32 {
    let s = 1.0 - sensitivity.clamp(0.0, 1.0);
    0.8 * s * s
}

/// Where the hit found in hop `k` begins in `band`: the first sample to
/// climb a fifth of the way from what sounded before it to its peak.
fn begins(band: &[f32], k: usize, hop: usize) -> usize {
    // The rise ended by the end of hop k, and began after hop k - RISE_HOPS
    // ended; a hop either side, for a slow attack and the envelope's lag
    let from = (k + 1).saturating_sub(RISE_HOPS + 2) * hop;
    let to = ((k + 2) * hop).min(band.len());
    let before = band[from.saturating_sub(2 * hop)..from].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let peak = band[from..to].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let mark = before + CLIMB * (peak - before);
    (from..to).find(|&i| band[i].abs() > mark).unwrap_or(from)
}

/// The mix in four bands, from the lowest, which add back up to it.
fn split(mono: &[f32], rate: f64) -> [Vec<f32>; 4] {
    let mut bands: [Vec<f32>; 4] = Default::default();
    for band in bands.iter_mut() {
        band.reserve_exact(mono.len());
    }
    let mut splitter = BandSplit::new(rate);
    for &x in mono {
        for (band, s) in bands.iter_mut().zip(splitter.step(x)) {
            band.push(s);
        }
    }
    bands
}

/// Splits a mix into four bands a sample at a time: each crossover's
/// low-pass takes what the ones below it left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BandSplit {
    lowpasses: [Lowpass; 3],
}

impl BandSplit {
    pub(crate) fn new(rate: f64) -> Self {
        Self { lowpasses: CROSSOVERS.map(|hz| Lowpass::new(hz, rate)) }
    }

    /// The next sample of each band, from the lowest.
    #[inline]
    pub(crate) fn step(&mut self, x: f32) -> [f32; 4] {
        let mut rest = x;
        let mut bands = [0.0; 4];
        for (band, lowpass) in bands.iter_mut().zip(&mut self.lowpasses) {
            *band = lowpass.step(rest);
            rest -= *band;
        }
        bands[3] = rest;
        bands
    }
}

/// A two-pole low-pass (a state-variable filter, Butterworth), run forwards.
#[derive(Clone, Copy, Debug)]
struct Lowpass {
    a1: f64,
    a2: f64,
    a3: f64,
    ic1: f64,
    ic2: f64,
}

impl Lowpass {
    fn new(hz: f64, rate: f64) -> Self {
        let g = (std::f64::consts::PI * (hz / rate).min(0.49)).tan();
        let k = std::f64::consts::SQRT_2;
        let a1 = 1.0 / (1.0 + g * (g + k));
        Self { a1, a2: g * a1, a3: g * g * a1, ic1: 0.0, ic2: 0.0 }
    }

    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        let v3 = x as f64 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2 as f32
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: f32 = 48000.0;

    /// A deterministic white noise, -1 to 1.
    pub(crate) struct Noise(pub(crate) u32);

    impl Noise {
        pub(crate) fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
        }
    }

    /// A drum hit for the test break.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Hit {
        /// A sine falling from 150 Hz to 50, dying away over a few hundred ms.
        Kick,
        /// A burst of noise over a 190 Hz tone.
        Snare,
    }

    /// Adds `hit` at `at` seconds into `out`, at `level`.
    fn add_hit(out: &mut [f32], hit: Hit, at: f32, level: f32, noise: &mut Noise) {
        let start = (at * RATE).round() as usize;
        let mut phase = 0.0f32;
        for (n, s) in out[start..].iter_mut().enumerate().take((0.6 * RATE) as usize) {
            let t = n as f32 / RATE;
            let value = match hit {
                Hit::Kick => {
                    let hz = 50.0 + 100.0 * (-t / 0.03).exp();
                    phase += TAU * hz / RATE;
                    (phase - TAU * hz / RATE).sin() * (-t / 0.12).exp()
                }
                Hit::Snare => 0.7 * noise.next() * (-t / 0.06).exp() + 0.4 * (TAU * 190.0 * t).sin() * (-t / 0.04).exp(),
            };
            *s += level * value;
        }
    }

    /// Two bars of a swung break at 96 BPM, pushed and dragged a little,
    /// over a faint hiss: the recording, and where each hit is in seconds.
    pub(crate) fn test_break() -> (SampleData, Vec<f32>) {
        let sixteenth = 60.0 / 96.0 / 4.0;
        // (sixteenth, hit, level, ms early or late)
        let pattern: [(usize, Hit, f32, f32); 12] = [
            (0, Hit::Kick, 1.0, 0.0),
            (3, Hit::Kick, 0.7, 3.0),
            (4, Hit::Snare, 0.9, -2.0),
            (7, Hit::Snare, 0.5, 4.0),
            (10, Hit::Kick, 0.85, -3.0),
            (12, Hit::Snare, 1.0, 1.0),
            (16, Hit::Kick, 0.95, 2.0),
            (18, Hit::Kick, 0.6, 0.0),
            (20, Hit::Snare, 0.9, -4.0),
            (25, Hit::Kick, 0.8, 3.0),
            (28, Hit::Snare, 0.95, 0.0),
            (31, Hit::Snare, 0.45, -1.0),
        ];
        let seconds = 32.0 * sixteenth + 0.5;
        let mut out = vec![0.0f32; (seconds * RATE) as usize];
        let mut noise = Noise(0x9e37_79b9);
        for s in out.iter_mut() {
            *s = 0.001 * noise.next();
        }
        let mut times = Vec::new();
        for (step, hit, level, nudge_ms) in pattern {
            // Every other sixteenth swung late
            let swing = if step % 2 == 1 { 0.18 * sixteenth } else { 0.0 };
            let at = step as f32 * sixteenth + swing + nudge_ms / 1000.0;
            add_hit(&mut out, hit, at, level, &mut noise);
            times.push(at);
        }
        (SampleData::mono(out, RATE), times)
    }

    #[test]
    fn test_every_hit_of_a_swung_break_gets_a_marker_and_nothing_else() {
        let (sample, times) = test_break();
        let onsets = Onsets::detect(&sample);
        let seconds = sample.seconds();
        let found: Vec<f32> = onsets.places(DEFAULT_SENSITIVITY).iter().map(|place| place * seconds).collect();
        assert_eq!(found.len(), times.len(), "found {found:?}\nwanted {times:?}");
        for (found, wanted) in found.iter().zip(&times) {
            assert!((found - wanted).abs() < 0.005, "a marker {:.1} ms from its hit: {found} vs {wanted}", (found - wanted) * 1000.0);
            // Never after the hit, so its attack is whole
            assert!(*found <= wanted + 0.0005, "{found} after {wanted}");
        }
    }

    #[test]
    fn test_sensitivity_chooses_how_many_count() {
        let (sample, times) = test_break();
        let onsets = Onsets::detect(&sample);
        let mut last = 0;
        for step in 0..=10 {
            let count = onsets.places(step as f32 / 10.0).len();
            assert!(count >= last, "turning it up never loses a marker");
            last = count;
        }
        // Turned down, only the strongest hits count
        let fewest = onsets.places(0.0).len();
        assert!(fewest >= 1 && fewest < times.len(), "{fewest}");
        // Every count is a subset of the next
        let half = onsets.places(0.5);
        assert!(onsets.places(0.3).iter().all(|place| half.contains(place)));
    }

    #[test]
    fn test_silence_and_steady_sound_have_no_onsets_past_the_first() {
        let silence = SampleData::mono(vec![0.0; 48000], RATE);
        assert!(Onsets::detect(&silence).all().is_empty());
        let short = SampleData::mono(vec![0.5; 100], RATE);
        assert!(Onsets::detect(&short).all().is_empty());

        // A steady low note, fading in over 50 ms: one onset, at its start
        let note: Vec<f32> = (0..96000).map(|n| (TAU * 55.0 * n as f32 / RATE).sin() * (n as f32 / 2400.0).min(1.0)).collect();
        let onsets = Onsets::detect(&SampleData::mono(note, RATE));
        let places = onsets.places(1.0);
        assert_eq!(places.len(), 1, "{places:?}");
        assert!(places[0] < 0.05 / 2.0);
    }

    #[test]
    fn test_a_hat_over_a_ringing_kick_is_found() {
        // A loud kick, and 120 ms into its ring a quiet hi-hat: noise above 6 kHz
        let mut out = vec![0.0f32; (RATE * 0.5) as usize];
        let mut noise = Noise(7);
        add_hit(&mut out, Hit::Kick, 0.01, 1.0, &mut noise);
        let at = (0.13 * RATE) as usize;
        let hat: Vec<f32> = (0..(0.05 * RATE) as usize).map(|_| noise.next()).collect();
        let hat = super::split(&hat, RATE as f64)[3].clone();
        for (n, s) in hat.iter().enumerate() {
            out[at + n] += 0.15 * s * (-(n as f32) / (0.01 * RATE)).exp();
        }
        let sample = SampleData::mono(out, RATE);
        let found: Vec<f32> = Onsets::detect(&sample).places(DEFAULT_SENSITIVITY).iter().map(|p| p * sample.seconds()).collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!((found[1] - 0.13).abs() < 0.005, "{found:?}");
    }

    #[test]
    fn test_stereo_is_heard_as_its_mix_and_rate_doesnt_matter() {
        let (sample, times) = test_break();
        let resampled = sample.resampled(44100.0);
        let found = Onsets::detect(&resampled).places(DEFAULT_SENSITIVITY);
        assert_eq!(found.len(), times.len());
        let left = sample.left().to_vec();
        let stereo = SampleData::stereo(left.clone(), left.iter().map(|s| s * 0.5).collect(), RATE);
        assert_eq!(Onsets::detect(&stereo).places(DEFAULT_SENSITIVITY).len(), times.len());
    }
}
