//! Tidying a recording: Trim cuts it to its sound, Normalize sets its level.
//!
//! The rules are `tools/voice/record.py`'s, which made Chris's take for the
//! Soba Speaks example, so a take tidied here comes out as that one did.
//!
//! **Where the sound is.** The recording is measured in 10 ms frames, on a
//! mono mix with the rumble under 70 Hz taken out (a stand bumped, traffic
//! outside). The room is how loud the quietest tenth of it is. A sound is
//! *sustained* if it lasts 80 ms within 30 dB of the loudest and 10 dB over
//! the room, so a lone thump or lip smack isn't one. The sound starts where
//! the first sustained one began: walking back from it while the level
//! keeps falling, which keeps a breath and an attack, and stops in the dip
//! before anything earlier, such as a thump. It ends 300 ms after the last.
//!
//! **Its level.** Normalize sets the loudest sample to −1 dBFS, or the
//! sound's RMS (its frames over the room, at an even level) to −16 dBFS. Up
//! to 6 dB of peaks over −1 dBFS are then taken by a gentle look-ahead
//! limiter; a peak louder than that lowers the whole recording instead.
//!
//! All of this allocates freely and runs on the UI side, never the audio thread.

use std::collections::VecDeque;
use std::ops::Range;

use super::primitives::rate_for_seconds;
use super::{db_to_gain, gain_to_db, SampleData};

/// Frames a second the level is measured in: 10 ms each.
const FRAMES_PER_SECOND: f32 = 100.0;
/// Rumble under this is left out of the measurement (not out of the recording).
const RUMBLE_HZ: f64 = 70.0;
/// How far over the room a sound has to stand.
const OVER_ROOM_DB: f32 = 10.0;
/// How far under the loudest a sustained sound may be.
const UNDER_LOUDEST_DB: f32 = 30.0;
/// How long a sound lasts before it's sustained, not a thump: 80 ms.
const SUSTAINED_FRAMES: usize = 8;
/// Gaps shorter than this (30 ms) don't split a sound in two.
const BRIDGE_FRAMES: usize = 3;
/// How far before the first sustained sound the start may walk back: 300 ms.
const LEAD_IN_FRAMES: usize = 30;
/// How much a level may rise again, walking back, and still be the lead-in.
const LEAD_IN_RISE_DB: f32 = 3.0;
/// Kept after the last sustained sound, so it can breathe out.
const TAIL_SECONDS: f32 = 0.3;
/// The fades at a cut: quick at the start, so an attack stays sharp, and
/// slow at the end, on a curve, so a tail dies away rather than stops.
const FADE_IN_SECONDS: f32 = 0.005;
const FADE_OUT_SECONDS: f32 = 0.08;

/// Where Normalize puts the loudest sample.
pub const PEAK_CEILING_DB: f32 = -1.0;
/// Where Normalize puts the sound's RMS, in its other mode.
pub const TARGET_RMS_DB: f32 = -16.0;
/// Changes of level smaller than this aren't worth a new file.
pub const NO_CHANGE_DB: f32 = 0.05;
/// How far over the ceiling peaks may be limited; beyond it the recording is lowered.
const LIMIT_DB: f32 = 6.0;
/// The limiter's look-ahead, and its recovery.
const LIMITER_AHEAD_SECONDS: f32 = 0.003;
const LIMITER_RELEASE_SECONDS: f32 = 0.06;

/// What Normalize sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    /// The loudest sample to −1 dBFS: as loud as it can be, unchanged in shape.
    Peak,
    /// The sound's RMS to −16 dBFS, peaks limited: recordings at one loudness.
    Rms,
}

impl Level {
    /// Its target, for a menu or a file name.
    pub fn describe(self) -> String {
        match self {
            Level::Peak => format!("peak to {} dBFS", minus(PEAK_CEILING_DB)),
            Level::Rms => format!("RMS to {} dBFS", minus(TARGET_RMS_DB)),
        }
    }
}

/// A number of decibels with a true minus sign.
pub fn minus(db: f32) -> String {
    let text = format!("{db:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text).to_string();
    text.replace('-', "−")
}

/// A change of level with its sign: "+6.8 dB", "−2.1 dB".
pub fn signed_db(db: f32) -> String {
    let text = format!("{db:+.1} dB");
    text.replace('-', "−")
}

/// What a recording holds, measured once for Trim and Normalize to use.
#[derive(Clone, Debug, PartialEq)]
pub struct Survey {
    /// The frames Trim keeps, or `None` if no sound stands out of the room
    /// (silence, or a recording too short to tell).
    pub sound: Option<Range<usize>>,
    /// The loudest sample, either channel.
    pub peak: f32,
    /// The RMS of the sound: its 10 ms frames over the room, both channels.
    pub rms: f32,
    /// Samples per measured frame, and which frames are the sound.
    frame: usize,
    voiced: Vec<bool>,
}

impl Survey {
    /// Measures a recording.
    pub fn of(sample: &SampleData) -> Self {
        let frame = ((sample.sample_rate() / FRAMES_PER_SECOND) as usize).max(1);
        let levels = frame_levels(&measured_mix(sample), frame);
        let peak = sample.left().iter().chain(sample.right()).fold(0.0f32, |m, s| m.max(s.abs()));

        let mut sound = None;
        let mut voiced = vec![false; levels.len()];
        if levels.len() >= 10 {
            let room = percentile(&levels, 10.0);
            let loudest = percentile(&levels, 99.5);
            let threshold = (room + OVER_ROOM_DB).max(loudest - 45.0);
            for (voiced, &level) in voiced.iter_mut().zip(&levels) {
                *voiced = level > threshold;
            }
            if let Some((first, last)) = sound_frames(&levels, loudest, room) {
                let end = ((last + 1) * frame + (TAIL_SECONDS * sample.sample_rate()) as usize).min(sample.frames());
                sound = Some(first * frame..end);
            } else if loudest > -90.0 {
                // Loud throughout, a drone or a pad: all of it is the sound
                sound = Some(0..sample.frames());
                voiced.fill(true);
            }
        }
        let mut survey = Self { sound, peak, rms: 0.0, frame, voiced };
        survey.rms = survey.rms_of(sample);
        survey
    }

    /// Whether Trim would cut anything: a measured frame (10 ms) or more
    /// at either end, so a recording just trimmed has nothing left to cut.
    pub fn trims(&self, frames: usize) -> bool {
        self.sound.as_ref().is_some_and(|sound| sound.start >= self.frame || sound.end + self.frame <= frames)
    }

    /// How many dB Normalize would turn it up (or, below zero, down) by,
    /// before any limiting: `None` if there's nothing to set the level of.
    pub fn gain_db(&self, level: Level) -> Option<f32> {
        let (now, target) = match level {
            Level::Peak => (self.peak, PEAK_CEILING_DB),
            Level::Rms => (self.rms, TARGET_RMS_DB),
        };
        (now > 1e-6).then(|| target - gain_to_db(now))
    }

    /// The RMS of a recording over the frames this survey found to be its
    /// sound: of the recording it measured, or one made from it at another level.
    pub fn rms_of(&self, sample: &SampleData) -> f32 {
        let mut sum = 0.0f64;
        let mut count = 0usize;
        let any_voiced = self.voiced.iter().any(|&v| v);
        for channel in [sample.left(), sample.right()] {
            for (i, chunk) in channel.chunks(self.frame).enumerate() {
                if !any_voiced || self.voiced.get(i).copied().unwrap_or(false) {
                    sum += chunk.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>();
                    count += chunk.len();
                }
            }
        }
        if count == 0 {
            0.0
        } else {
            (sum / count as f64).sqrt() as f32
        }
    }
}

/// The recording cut to its sound, with a short fade at each cut. `None`
/// if there's nothing to cut.
pub fn trim(sample: &SampleData, survey: &Survey) -> Option<SampleData> {
    if !survey.trims(sample.frames()) {
        return None;
    }
    let sound = survey.sound.clone()?;
    let rate = sample.sample_rate();
    let cut = |channel: &[f32]| {
        let mut kept = channel[sound.clone()].to_vec();
        if sound.start > 0 {
            let fade = ((FADE_IN_SECONDS * rate) as usize).min(kept.len());
            for (i, s) in kept[..fade].iter_mut().enumerate() {
                *s *= i as f32 / fade as f32;
            }
        }
        if sound.end < channel.len() {
            let fade = ((FADE_OUT_SECONDS * rate) as usize).min(kept.len());
            let from = kept.len() - fade;
            for (i, s) in kept[from..].iter_mut().enumerate() {
                let left = 1.0 - (i + 1) as f32 / fade as f32;
                *s *= left * left;
            }
        }
        kept
    };
    Some(if sample.is_stereo() {
        SampleData::stereo(cut(sample.left()), cut(sample.right()), rate)
    } else {
        SampleData::mono(cut(sample.left()), rate)
    })
}

/// A recording made louder or quieter by [`normalize`], and how.
#[derive(Clone, Debug)]
pub struct Normalized {
    pub sample: SampleData,
    /// The gain, in dB, before the limiter.
    pub gain_db: f32,
    /// How far the loudest peak was over −1 dBFS before the limiter took it.
    pub limited_db: f32,
    /// How far a peak too loud to limit held the whole recording under its target.
    pub held_back_db: f32,
}

/// The recording at the level asked for. `None` if it's silent.
pub fn normalize(sample: &SampleData, survey: &Survey, level: Level) -> Option<Normalized> {
    let gain_db = survey.gain_db(level)?;
    let ceiling = db_to_gain(PEAK_CEILING_DB);
    if level == Level::Peak {
        let gain = ceiling / survey.peak;
        let sample = scaled(sample, gain);
        return Some(Normalized { sample, gain_db, limited_db: 0.0, held_back_db: 0.0 });
    }

    // Up to LIMIT_DB of peaks over the ceiling is limited away; beyond it, the whole is lowered
    let over = gain_to_db(survey.peak) + gain_db - PEAK_CEILING_DB;
    let held_back_db = (over - LIMIT_DB).max(0.0);
    let mut gain_db = gain_db - held_back_db;
    let mut out = limit(&scaled(sample, db_to_gain(gain_db)), ceiling);
    // The limiter takes a little off the RMS: give it back, a few times over
    if held_back_db == 0.0 {
        for _ in 0..4 {
            let short = TARGET_RMS_DB - gain_to_db(survey.rms_of(&out));
            if short.abs() < 0.01 {
                break;
            }
            gain_db += short;
            out = limit(&scaled(sample, db_to_gain(gain_db)), ceiling);
        }
    }
    let limited_db = (gain_to_db(survey.peak) + gain_db - PEAK_CEILING_DB).clamp(0.0, LIMIT_DB);
    Some(Normalized { sample: out, gain_db, limited_db, held_back_db })
}

/// A recording made `gain` times as loud.
fn scaled(sample: &SampleData, gain: f32) -> SampleData {
    let scale = |channel: &[f32]| channel.iter().map(|s| s * gain).collect::<Vec<_>>();
    if sample.is_stereo() {
        SampleData::stereo(scale(sample.left()), scale(sample.right()), sample.sample_rate())
    } else {
        SampleData::mono(scale(sample.left()), sample.sample_rate())
    }
}

/// A look-ahead peak limiter, both channels together: the gain dips
/// smoothly over 3 ms before each peak that would pass `ceiling`, and
/// recovers over about 60 ms.
fn limit(sample: &SampleData, ceiling: f32) -> SampleData {
    let rate = sample.sample_rate();
    let frames = sample.frames();
    let width = ((LIMITER_AHEAD_SECONDS * rate) as usize).max(1);
    let (left, right) = (sample.left(), sample.right());
    let needed: Vec<f32> = (0..frames).map(|i| (ceiling / left[i].abs().max(right[i].abs()).max(1e-9)).min(1.0)).collect();
    let lowest = running_min(&needed, width);
    // Averaged over the look-ahead, so the gain dips on a slope, not a step
    let half = (width - 1) / 2;
    let at = |j: isize| lowest[j.clamp(0, frames as isize - 1) as usize] as f64;
    let mut sum: f64 = (0..width).map(|k| at(half as isize - width as isize + 1 + k as isize)).sum();
    let release = rate_for_seconds(LIMITER_RELEASE_SECONDS, rate);
    let mut state = 1.0f32;
    let mut gains = Vec::with_capacity(frames);
    for i in 0..frames {
        if i > 0 {
            let i = i as isize;
            sum += at(i + half as isize) - at(i + half as isize - width as isize);
        }
        let gain = (sum / width as f64) as f32;
        state = if gain < state { gain } else { state + (gain - state) * release };
        gains.push(state);
    }
    // The average can't quite dip far enough on the sharpest edges
    let apply = |channel: &[f32]| channel.iter().zip(&gains).map(|(s, g)| (s * g).clamp(-ceiling, ceiling)).collect::<Vec<_>>();
    if sample.is_stereo() {
        SampleData::stereo(apply(left), apply(right), rate)
    } else {
        SampleData::mono(apply(left), rate)
    }
}

/// The least of `values` within `width` either side of each one.
fn running_min(values: &[f32], width: usize) -> Vec<f32> {
    let n = values.len();
    let mut out = Vec::with_capacity(n);
    let mut window: VecDeque<usize> = VecDeque::new();
    let mut next = 0;
    for i in 0..n {
        while next < n && next <= i + width {
            while window.back().is_some_and(|&j| values[j] >= values[next]) {
                window.pop_back();
            }
            window.push_back(next);
            next += 1;
        }
        while window.front().is_some_and(|&j| j + width < i) {
            window.pop_front();
        }
        out.push(values[window[0]]);
    }
    out
}

/// The recording as one channel, with the rumble under 70 Hz taken out.
fn measured_mix(sample: &SampleData) -> Vec<f64> {
    let mut filter = HighPass::new(RUMBLE_HZ, sample.sample_rate() as f64);
    sample.left().iter().zip(sample.right()).map(|(&l, &r)| filter.process((l as f64 + r as f64) / 2.0)).collect()
}

/// The RMS level, in dB, of each whole frame.
fn frame_levels(mix: &[f64], frame: usize) -> Vec<f32> {
    mix.chunks_exact(frame)
        .map(|chunk| {
            let power = chunk.iter().map(|s| s * s).sum::<f64>() / frame as f64;
            (10.0 * power.max(1e-18).log10()) as f32
        })
        .collect()
}

/// The first and last frames of the sound, as `record.py` finds its words:
/// see the module's notes. `None` if nothing stands out of the room.
fn sound_frames(levels: &[f32], loudest: f32, room: f32) -> Option<(usize, usize)> {
    let floor = (loudest - UNDER_LOUDEST_DB).max(room + OVER_ROOM_DB);
    let loud: Vec<bool> = levels.iter().map(|&level| level > floor).collect();
    let heard = runs(&loud, BRIDGE_FRAMES);
    let sustained: Vec<_> = heard.iter().filter(|run| run.len() >= SUSTAINED_FRAMES).cloned().collect();
    let sustained = if sustained.is_empty() { heard } else { sustained };
    let first = sustained.first()?.start;
    let last = sustained.last()?.end - 1;
    // Back through what leads into it, while the level keeps falling
    let (mut quietest, mut i) = (first, first);
    // back to the room (in a quiet room, that's where the sound began)
    while i > first.saturating_sub(LEAD_IN_FRAMES) && levels[quietest] > room + LEAD_IN_RISE_DB && levels[i - 1] < levels[quietest] + LEAD_IN_RISE_DB {
        i -= 1;
        if levels[i] < levels[quietest] {
            quietest = i;
        }
    }
    Some((quietest, last))
}

/// Each run of `true`, joined across gaps shorter than `bridge`.
fn runs(mask: &[bool], bridge: usize) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        if !mask[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < mask.len() && mask[i] {
            i += 1;
        }
        match out.last_mut() {
            Some(last) if start - last.end < bridge => last.end = i,
            _ => out.push(start..i),
        }
    }
    out
}

/// The `p`th percentile of `values`, between the two nearest (as numpy has it).
fn percentile(values: &[f32], p: f32) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let place = p / 100.0 * (sorted.len() - 1) as f32;
    let below = place.floor() as usize;
    let above = (below + 1).min(sorted.len() - 1);
    sorted[below] + (sorted[above] - sorted[below]) * (place - below as f32)
}

/// A second-order Butterworth high-pass, as scipy's `butter(2, …)` makes.
struct HighPass {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl HighPass {
    fn new(cutoff: f64, rate: f64) -> Self {
        let w = std::f64::consts::TAU * cutoff / rate;
        let alpha = w.sin() / std::f64::consts::SQRT_2;
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let b = [(1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0, (1.0 + cos) / 2.0 / a0];
        Self { b, a: [-2.0 * cos / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48000.0;

    /// A tone at `db` dBFS for `seconds`.
    fn tone(seconds: f32, db: f32, hz: f32) -> Vec<f32> {
        let amplitude = db_to_gain(db) * std::f32::consts::SQRT_2;
        (0..(seconds * RATE) as usize).map(|i| amplitude * (std::f32::consts::TAU * hz * i as f32 / RATE).sin()).collect()
    }

    /// Hiss at `db` dBFS RMS, the same every time.
    fn room(seconds: f32, db: f32) -> Vec<f32> {
        let mut seed = 12345u32;
        let amplitude = db_to_gain(db) * 3f32.sqrt();
        (0..(seconds * RATE) as usize)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                amplitude * ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    fn padded(before: f32, sound: &[f32], after: f32) -> SampleData {
        let mut take = room(before, -70.0);
        take.extend(sound.iter().zip(room(1.0, -70.0).iter().cycle()).map(|(s, r)| s + r));
        take.extend(room(after, -70.0));
        SampleData::mono(take, RATE)
    }

    #[test]
    fn trims_the_silence_at_both_ends() {
        let take = padded(1.0, &tone(1.5, -20.0, 220.0), 2.0);
        let survey = Survey::of(&take);
        let sound = survey.sound.clone().unwrap();
        // The tone starts at 1 s: the start walks back no further into the room
        assert!((sound.start as f32 / RATE - 1.0).abs() <= 0.011, "{sound:?}");
        // and ends at 2.5 s, plus the 300 ms tail
        assert!((sound.end as f32 / RATE - 2.8).abs() <= 0.011, "{sound:?}");
        let trimmed = trim(&take, &survey).unwrap();
        assert_eq!(trimmed.frames(), sound.len());
        // Faded in at the cut, and out to nothing
        assert_eq!(trimmed.left()[0], 0.0);
        assert!(trimmed.left().last().unwrap().abs() < 1e-6);
        // Trimmed, there's nothing more to cut
        assert!(!Survey::of(&trimmed).trims(trimmed.frames()));
    }

    #[test]
    fn a_thump_before_the_sound_is_left_out_but_the_breath_kept() {
        // A 40 ms thump, a gap of room, a breath rising into a sustained tone
        let mut sound = tone(0.04, -12.0, 80.0);
        sound.extend(vec![0.0; (0.3 * RATE) as usize]);
        let breath: Vec<f32> = (0..(0.15 * RATE) as usize).map(|i| i as f32 / (0.15 * RATE)).collect();
        sound.extend(tone(0.15, -24.0, 300.0).iter().zip(&breath).map(|(s, b)| s * b * b));
        sound.extend(tone(1.0, -14.0, 300.0));
        let take = padded(0.5, &sound, 0.5);
        let start = Survey::of(&take).sound.unwrap().start as f32 / RATE;
        // After the thump's gap, and into the breath, not at the tone
        assert!(start > 0.6 && start < 0.95, "{start}");
    }

    /// Chris's take for Soba Speaks, as `record.py` first made it: its
    /// first 0.22 s are kept as a fixture, the rest is the example's sample.
    fn chris_take() -> SampleData {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let read = |path: &str| crate::persistence::sample_files::read_file(&root.join(path)).unwrap().sample;
        let head = read("tests/fixtures/soba-speaks-chris-head.wav");
        let rest = read("patches/samples/soba-speaks-chris.wav");
        // The example's first 10 ms were faded in when it was cut
        let mut take = head.left().to_vec();
        take.extend_from_slice(&rest.left()[take.len() - 9261..]);
        SampleData::mono(take, head.sample_rate())
    }

    #[test]
    fn chris_take_starts_where_record_py_found_it() {
        let take = chris_take();
        assert_eq!(take.frames(), 192_717);
        let sound = Survey::of(&take).sound.unwrap();
        // record.py: 0.22 s in, after a thump and on the breath before "Hello",
        // and 300 ms after "sines", 4.35 s in
        let (start, end) = (sound.start as f32 / take.sample_rate(), sound.end as f32 / take.sample_rate());
        assert!((start - 0.22).abs() < 0.005, "{start}");
        assert!((end - 4.35).abs() < 0.005, "{end}");
        // And trimmed, it's its sound: nothing more to cut
        let trimmed = trim(&take, &Survey::of(&take)).unwrap();
        assert!(!Survey::of(&trimmed).trims(trimmed.frames()), "{:?}", Survey::of(&trimmed).sound);
    }

    #[test]
    fn silence_has_no_sound_and_a_drone_is_all_sound() {
        let silent = SampleData::mono(vec![0.0; RATE as usize], RATE);
        let survey = Survey::of(&silent);
        assert_eq!(survey.sound, None);
        assert_eq!(survey.gain_db(Level::Peak), None);
        assert!(trim(&silent, &survey).is_none());
        let drone = SampleData::mono(tone(2.0, -10.0, 110.0), RATE);
        let survey = Survey::of(&drone);
        assert_eq!(survey.sound, Some(0..drone.frames()));
        assert!(trim(&drone, &survey).is_none());
    }

    #[test]
    fn peak_normalizes_exactly() {
        let take = padded(0.3, &tone(1.0, -18.0, 440.0), 0.3);
        let survey = Survey::of(&take);
        let out = normalize(&take, &survey, Level::Peak).unwrap();
        let peak = out.sample.left().iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((gain_to_db(peak) - PEAK_CEILING_DB).abs() < 0.01, "{}", gain_to_db(peak));
        // The tone peaks 3 dB over its RMS
        assert!((out.gain_db - 14.0).abs() < 0.1, "{}", out.gain_db);
    }

    #[test]
    fn rms_normalizes_within_a_tenth_of_a_db_with_peaks_limited() {
        // A tone with a few sharp clicks on it, 9 dB over its own peaks
        let mut sound = tone(2.0, -30.0, 300.0);
        for at in [0.3, 0.9, 1.6] {
            let i = (at * RATE) as usize;
            sound[i] = 0.25;
            sound[i + 1] = -0.2;
        }
        let take = SampleData::stereo(sound.clone(), sound.iter().map(|s| s * 0.5).collect(), RATE);
        let survey = Survey::of(&take);
        let out = normalize(&take, &survey, Level::Rms).unwrap();
        let rms = gain_to_db(survey.rms_of(&out.sample));
        assert!((rms - TARGET_RMS_DB).abs() < 0.1, "{rms}");
        assert_eq!(out.held_back_db, 0.0);
        assert!(out.limited_db > 0.0);
        let peak = out.sample.left().iter().chain(out.sample.right()).fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(gain_to_db(peak) <= PEAK_CEILING_DB + 0.001, "{}", gain_to_db(peak));
        // Stereo stays stereo, the sides in the same proportion away from the clicks
        assert!(out.sample.is_stereo());
        let i = (1.2 * RATE) as usize;
        assert!((out.sample.right()[i] - 0.5 * out.sample.left()[i]).abs() < 1e-4);
    }

    #[test]
    fn a_peak_too_loud_to_limit_holds_the_level_back() {
        let mut sound = tone(1.0, -40.0, 300.0);
        sound[1000] = 0.9;
        let take = SampleData::mono(sound, RATE);
        let survey = Survey::of(&take);
        let out = normalize(&take, &survey, Level::Rms).unwrap();
        assert!(out.held_back_db > 1.0, "{}", out.held_back_db);
        assert!((out.limited_db - LIMIT_DB).abs() < 0.01);
    }

    #[test]
    fn running_min_is_the_least_within_reach() {
        let values = [5.0, 3.0, 4.0, 9.0, 1.0, 7.0, 8.0, 6.0];
        let brute: Vec<f32> = (0..values.len())
            .map(|i| values[i.saturating_sub(2)..(i + 3).min(values.len())].iter().cloned().fold(f32::INFINITY, f32::min))
            .collect();
        assert_eq!(running_min(&values, 2), brute);
    }

    #[test]
    fn percentiles_interpolate_as_numpy_does() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&values, 10.0), 1.4);
        assert_eq!(percentile(&values, 50.0), 3.0);
        assert_eq!(percentile(&values, 100.0), 5.0);
    }

    #[test]
    fn decibels_read_with_a_true_minus() {
        assert_eq!(minus(-16.0), "−16");
        assert_eq!(minus(-1.0), "−1");
        assert_eq!(Level::Rms.describe(), "RMS to −16 dBFS");
        assert_eq!(signed_db(6.84), "+6.8 dB");
        assert_eq!(signed_db(-2.1), "−2.1 dB");
    }
}
