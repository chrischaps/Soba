//! Hearing the beat in music as it plays: where the Dancer finds a beat to
//! dance to when no Clock gives it one.
//!
//! **How hard it hits, every 10 ms.** The music is split into the four
//! bands the Sampler's onsets use (`onsets.rs`), each followed by an
//! envelope that rises at once and falls away gently. Every 10 ms each
//! band's loudest moment is compared, in dB, with the 10 ms before, and the
//! rises are added up: a drum hit rises in several bands at once, a held
//! note hardly at all. That's the onset strength, the rough spectral flux
//! the Sampler picks its onsets from, kept here as a running signal. Each
//! rise counts as much as its band is loud, and the lowest band's twice:
//! the kick marks the beat far more often than a quiet hat does.
//!
//! **The tempo, from how the hits repeat.** The onset strength is
//! correlated with itself over the last few seconds, the older fading, so a
//! beat that comes every half second stands out at a lag of half a second.
//! Each beat from 0.4 to 0.8 s (150 to 75 BPM) is scored by its lag, half
//! of it and twice it together: a sixteenth-note hat and a kick on every
//! other beat both count toward the beat they belong to, so the tempo comes
//! out already folded into the range a person dances at. Once it's dancing
//! one, another takes over only by leading for a few seconds: three against
//! four puts the beat and the dotted eighth ahead by turns, and a dancer
//! keeps to the one it found.
//!
//! **Where the beats fall.** A comb with a tooth every beat slides along the
//! last few seconds of onset strength, and where it catches the most hits
//! is where the beats are. The tracker's own beat clock is pulled toward it
//! a little each frame, hurrying or dawdling but never stepping back, so a
//! fill or a flam doesn't make the dance jerk.
//!
//! **On the beat, not the and.** A broken beat can hit harder between the
//! beats than on them: Canopy's kick lands on one, the and of two and the
//! and of three, and its bass with it. But the snare still lands on two and
//! four, so the tracker listens for that too. A rise in the upper bands
//! that the lowest doesn't share is a snare or a clap, not a kick. Taken in
//! turns a beat apart, one beat added and the next taken away, a snare on
//! two and four adds up while a hat on every beat cancels out. The clock
//! leans toward whichever of its tooth and the one half a beat away has
//! more of that, slowly, and more slowly the less there is to hear.
//! Leaning hard enough, it turns to the beat there; leaning the other way,
//! the comb needs more to pull it onto the and. Music without a backbeat
//! leans neither way.
//!
//! **Only when there's one.** It claims a beat once the music has been
//! hitting for a couple of seconds and one tempo stands clear of the rest,
//! and lets go when it stops hitting. A drone, a pad or silence has none.
//!
//! Everything is allocated up front: it runs on the audio thread.

use super::onsets::{BandSplit, FLOOR_DB, LEAST_RISE_DB, RELEASE_SECONDS, SILENCE_DB};

/// How often the onset strength is measured: every 10 ms.
const FRAME_SECONDS: f32 = 0.01;
/// How much a rise counts in each band, from the lowest: the kick is what
/// most often marks the beat.
const BAND_WEIGHTS: [f32; 4] = [2.0, 1.0, 1.0, 1.0];
/// How many frames of onset strength are kept: 10 s, a power of two.
const HISTORY: usize = 1024;
/// The shortest beat scored, in frames (150 BPM); the longest is twice it.
const SHORTEST_BEAT: f32 = 40.0;
/// Lags the correlation keeps: up to twice the longest beat, and one more
/// to read between.
const LAGS: usize = 4 * SHORTEST_BEAT as usize + 2;
/// Beats scored across the octave, spaced evenly in pitch: about 0.7% apart.
const CANDIDATES: usize = 96;
/// How long the correlation remembers, and how long the onset strength's
/// running mean takes to settle, in seconds.
const CORRELATION_SECONDS: f32 = 4.0;
const MEAN_SECONDS: f32 = 1.0;
/// How fast the loudest level heard falls away, in dB a second, so the
/// silence floor follows the music down when it gets quieter.
const LOUDEST_FALL_DB: f32 = 6.0;

/// A beat is claimed once the music has hit for this long, in seconds...
const HEARD_SECONDS: f32 = 2.0;
/// ...if its tempo has scored at least this for [`SURE_SECONDS`] (the
/// correlation's mean at the beat, half and twice it, against none at all;
/// hits at random score under 0.2, a swung break 0.25 to 0.3, four to the
/// floor 0.6)...
const CLAIM_SCORE: f32 = 0.22;
const SURE_SECONDS: f32 = 1.0;
/// ...and let go when the score has been under this for as long, or
/// nothing has hit for [`QUIET_SECONDS`].
const LOSE_SCORE: f32 = 0.14;
const QUIET_SECONDS: f32 = 2.0;
/// Another tempo takes over from the one being followed only when it
/// scores this much better, as a lead held over [`RIVAL_SECONDS`]: at once
/// for a beat danced for less than that, and more patiently over as long
/// again.
const SWITCH: f32 = 1.15;
const RIVAL_SECONDS: f32 = 4.0;
/// How far either side of the beat being followed it looks for that beat
/// again, in candidates.
const NEARBY: usize = 4;
/// The beat being followed may drift this far from the danced range, in
/// seconds, before it's folded back into it.
const LONGEST_FOLLOWED: f32 = 1.2;
const SHORTEST_FOLLOWED: f32 = 0.3;

/// The comb's teeth: how many beats back it reaches, each worth this much
/// of the one after it.
const COMB_BEATS: usize = 12;
const COMB_FADE: f32 = 0.9;
/// The most teeth a beat can have: one a frame, for the longest beat followed.
const COMB_MOST: usize = (LONGEST_FOLLOWED / FRAME_SECONDS) as usize + 2;
/// The comb keeps to the beat it found within this fraction of a beat of
/// the clock, unless a tooth elsewhere catches this many times as much.
const STICK: f32 = 0.15;
const HOLD: f32 = 1.3;
/// The backbeat: how far the upper bands rose past this many times the
/// lowest band's rise, a snare or a clap but not a kick...
const BACKBEAT_LOW: f32 = 2.0;
/// ...taken in turns over this many beats, each worth this much of the one
/// after it.
const BACKBEAT_BEATS: usize = 12;
const BACKBEAT_FADE: f32 = 0.97;
/// How much more backbeat there is half a beat from the clock's tooth than
/// on it counts by its log, up to this many times either way...
const BACKBEAT_MOST: f32 = 4.0;
/// ...past this share of the tooth's catch, so a whisper of one isn't
/// heard as a lot more than none.
const BACKBEAT_FLOOR: f32 = 0.05;
/// The clock leans toward it, settling over this long, in seconds, once
/// the backbeat is [`BACKBEAT_CLEAR`] of the tooth's catch, and more slowly
/// as it fades: a song's snare stays where it was through a breakdown.
const LEAN_SECONDS: f32 = 4.0;
const BACKBEAT_CLEAR: f32 = 0.25;
/// Leaning past this (twice the backbeat, as a log), the clock turns to
/// the beat half a beat away.
const TURN: f32 = std::f32::consts::LN_2;
/// A hit is measured in the frame it lands in, on average half a frame
/// before that frame ends.
const LATENCY_FRAMES: f32 = 0.5;
/// How hard the beat clock is pulled toward the comb: by half its error
/// over a beat, hurrying or dawdling by at most [`MOST_PULL`]...
const PULL: f64 = 0.5;
const MOST_PULL: f64 = 0.25;
/// ...and trims the beat's length by this fraction, each frame, for each
/// beat it's late (or lengthens it, early), up to [`MOST_TRIM`] either way.
/// The tempo heard is a little coarse, and a clock that kept missing the
/// same way would otherwise settle on a steady miss.
const TRIM: f32 = 0.001;
const MOST_TRIM: f32 = 0.02;
/// Within this much of a beat of the comb, the clock is on the beat.
const LOCKED: f64 = 0.1;
/// How long the beat takes to settle on a new tempo, in seconds.
const TEMPO_SECONDS: f32 = 1.0;

/// Hears the beat in music as it plays.
pub struct BeatTracker {
    /// Samples a frame, and how far into this one.
    frame_samples: usize,
    into_frame: usize,
    split: BandSplit,
    /// The split before it has heard anything, to start afresh from.
    silent_split: BandSplit,
    /// Each band's envelope, how fast it falls, its loudest this frame, and
    /// its level last frame in dB.
    envelopes: [f32; 4],
    falls: [f32; 4],
    peaks: [f32; 4],
    levels: [f32; 4],
    /// The loudest any band has been lately, in dB.
    loudest: f32,
    /// Frames since the last hit, and since the music began hitting.
    since_hit: usize,
    heard: usize,
    /// Frames the best tempo has scored enough to claim, or (with a beat)
    /// too little to keep.
    sure: usize,
    doubt: usize,
    mean: f32,
    /// Frames since it began listening: the latest frame's place in the
    /// history, unwrapped.
    frames: usize,
    /// The beat being followed. `None` while there's no beat.
    beat: Option<Beat>,
    memory: Box<Memory>,
}

/// The tracker's history, in one allocation.
struct Memory {
    /// The onset strength, and less its running mean.
    strength: [f32; HISTORY],
    /// How much of each frame's rise sounds like a snare.
    backbeat: [f32; HISTORY],
    centered: [f32; HISTORY],
    /// The centered strength correlated with itself, lag by lag.
    correlation: [f32; LAGS],
    /// Each candidate beat's score this frame.
    scores: [f32; CANDIDATES],
}

#[derive(Clone, Copy, Debug)]
struct Beat {
    /// How long a beat lasts as heard, in frames, and the fraction the
    /// clock trims from it to stay on the music.
    frames: f32,
    trim: f32,
    /// Beats since it was claimed, at the end of the last frame.
    position: f64,
    /// How far the last frame moved it, so a block between frames can be
    /// read part way.
    step: f64,
    /// How much more backbeat lies half a beat from the clock's tooth than
    /// on it, as a log, learned while the clock is on that tooth.
    lean: f32,
    /// While it turns half a beat to the backbeat, where in the beat it's
    /// headed.
    heading: Option<f64>,
    /// How far another tempo has led this one lately, as a log, 0 while
    /// this one is the best.
    rival: f32,
}

impl Default for BeatTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl BeatTracker {
    /// How near it is to claiming a beat, 0 to 1, for a listener to show:
    /// up to a half as the best tempo's score climbs toward the claim, then
    /// the rest as the music keeps hitting and that score holds. 1 with a
    /// beat, 0 while nothing hits.
    pub fn hunch(&self) -> f32 {
        if self.beat.is_some() {
            return 1.0;
        }
        if self.heard == 0 {
            return 0.0;
        }
        let best = self.memory.scores.iter().fold(0.0f32, |a, &b| a.max(b));
        let climbing = ((best - LOSE_SCORE) / (CLAIM_SCORE - LOSE_SCORE)).clamp(0.0, 1.0);
        let heard = self.heard as f32 * FRAME_SECONDS / HEARD_SECONDS;
        let sure = self.sure as f32 * FRAME_SECONDS / SURE_SECONDS;
        0.5 * climbing + 0.5 * heard.min(sure).min(1.0)
    }
}

impl BeatTracker {
    /// A tracker that has heard nothing, at 48 kHz.
    pub fn new() -> Self {
        let mut tracker = Self {
            frame_samples: 480,
            into_frame: 0,
            split: BandSplit::new(48000.0),
            silent_split: BandSplit::new(48000.0),
            envelopes: [0.0; 4],
            falls: [0.0; 4],
            peaks: [0.0; 4],
            levels: [SILENCE_DB; 4],
            loudest: SILENCE_DB,
            since_hit: usize::MAX,
            heard: 0,
            sure: 0,
            doubt: 0,
            mean: 0.0,
            frames: 0,
            beat: None,
            memory: Box::new(Memory {
                strength: [0.0; HISTORY],
                backbeat: [0.0; HISTORY],
                centered: [0.0; HISTORY],
                correlation: [0.0; LAGS],
                scores: [0.0; CANDIDATES],
            }),
        };
        tracker.prepare(48000.0);
        tracker
    }

    /// Readies it for `sample_rate`, forgetting what it heard.
    pub fn prepare(&mut self, sample_rate: f32) {
        self.frame_samples = ((FRAME_SECONDS * sample_rate).round() as usize).max(1);
        self.silent_split = BandSplit::new(sample_rate as f64);
        self.falls = RELEASE_SECONDS.map(|release| (-1.0 / (release * sample_rate as f64)).exp() as f32);
        self.forget();
    }

    /// Forgets everything heard, as if the music had only just begun.
    pub fn forget(&mut self) {
        // A tracker that hasn't measured a frame has nothing to clear but a
        // few numbers
        if self.frames > 0 {
            let memory = &mut *self.memory;
            memory.strength.fill(0.0);
            memory.backbeat.fill(0.0);
            memory.centered.fill(0.0);
            memory.correlation.fill(0.0);
            memory.scores.fill(0.0);
        }
        self.into_frame = 0;
        self.split = self.silent_split;
        self.envelopes = [0.0; 4];
        self.peaks = [0.0; 4];
        self.levels = [SILENCE_DB; 4];
        self.loudest = SILENCE_DB;
        self.since_hit = usize::MAX;
        self.heard = 0;
        self.sure = 0;
        self.doubt = 0;
        self.mean = 0.0;
        self.frames = 0;
        self.beat = None;
    }

    /// The beat at the last sample heard: beats since it was claimed, and
    /// how long one lasts in seconds. `None` while it hears none.
    pub fn beat(&self) -> Option<(f64, f32)> {
        self.beat.map(|beat| {
            let through = self.into_frame as f64 / self.frame_samples as f64;
            (beat.position + through * beat.step, beat.length() * FRAME_SECONDS)
        })
    }

    /// Hears one sample of the music.
    #[inline]
    pub fn hear(&mut self, x: f32) {
        for (b, s) in self.split.step(x).into_iter().enumerate() {
            self.envelopes[b] = s.abs().max(self.envelopes[b] * self.falls[b]);
            self.peaks[b] = self.peaks[b].max(self.envelopes[b]);
        }
        self.into_frame += 1;
        if self.into_frame == self.frame_samples {
            self.into_frame = 0;
            self.frame();
        }
    }

    /// Measures a frame's onset strength and moves the beat on.
    fn frame(&mut self) {
        let (strength, backbeat) = self.strength();
        self.frames += 1;
        let now = self.frames % HISTORY;

        if strength >= LEAST_RISE_DB {
            self.since_hit = 0;
        } else {
            self.since_hit = self.since_hit.saturating_add(1);
        }
        let quiet = self.since_hit as f32 * FRAME_SECONDS > QUIET_SECONDS;
        self.heard = if quiet { 0 } else { self.heard + 1 };

        // The strength correlated with itself, less its mean so a busy
        // passage doesn't look like every tempo at once
        let memory = &mut *self.memory;
        memory.strength[now] = strength;
        memory.backbeat[now] = backbeat;
        // Smoothed over three frames, so hits pushed or swung a little
        // still line up
        let at = |back: usize| memory.strength[(now + HISTORY - back) % HISTORY];
        let smooth = 0.25 * at(0) + 0.5 * at(1) + 0.25 * at(2);
        self.mean += (smooth - self.mean) * (FRAME_SECONDS / MEAN_SECONDS);
        let centered = smooth - self.mean;
        let keep = (-FRAME_SECONDS / CORRELATION_SECONDS).exp();
        memory.centered[now] = centered;
        for (lag, correlation) in memory.correlation.iter_mut().enumerate() {
            let then = memory.centered[(now + HISTORY - lag) % HISTORY];
            *correlation = *correlation * keep + centered * then;
        }

        // Where the beat could be, and how sure
        let chosen = self.choose_tempo();
        let score = chosen.map_or(0.0, |(_, score, _)| score);
        self.sure = if score >= CLAIM_SCORE { self.sure + 1 } else { 0 };
        self.doubt = if score < LOSE_SCORE { self.doubt + 1 } else { 0 };
        let heard_enough = self.heard as f32 * FRAME_SECONDS >= HEARD_SECONDS;
        let sure = self.sure as f32 * FRAME_SECONDS >= SURE_SECONDS;
        let doubtful = self.doubt as f32 * FRAME_SECONDS >= SURE_SECONDS;
        self.beat = match (self.beat, chosen) {
            (None, Some((frames, ..))) if heard_enough && sure => {
                let (phase, _) = self.phase(frames, None);
                Some(Beat { frames, trim: 0.0, position: phase as f64, step: 0.0, lean: 0.0, heading: None, rival: 0.0 })
            }
            (Some(beat), Some((frames, _, taking_over))) if !quiet && !doubtful => {
                // A new tempo has beats of its own: where the clock was in the
                // old one, and how it trimmed and leaned there, say nothing
                // about them, so it finds them afresh, as a claim does, and
                // catches up
                let beat = if taking_over { Beat { heading: Some(self.phase(frames, None).0 as f64), trim: 0.0, lean: 0.0, ..beat } } else { beat };
                Some(self.follow(beat, frames))
            }
            _ => None,
        };
    }

    /// This frame's onset strength: how far the bands rose together, in dB,
    /// since the last frame. And how much of that sounds like a snare.
    fn strength(&mut self) -> (f32, f32) {
        let mut db = [0.0f32; 4];
        for (db, peak) in db.iter_mut().zip(&mut self.peaks) {
            *db = 20.0 * peak.max(1e-9).log10();
            *peak = 0.0;
        }
        let frame_loudest = db.iter().fold(SILENCE_DB, |a, &b| a.max(b));
        self.loudest = (self.loudest - LOUDEST_FALL_DB * FRAME_SECONDS).max(frame_loudest);
        let floor = (self.loudest - FLOOR_DB).max(SILENCE_DB);
        // A band's rise counts as much as the band is loud, so a quiet hat
        // on the offbeat isn't heard as a kick on the beat
        let mut rises = [0.0; 4];
        for (b, ((now, before), weight)) in db.iter().zip(&mut self.levels).zip(BAND_WEIGHTS).enumerate() {
            let loud = ((now - floor) / FLOOR_DB).clamp(0.0, 1.0);
            rises[b] = weight * loud * (now.max(floor) - before.max(floor)).max(0.0);
            *before = *now;
        }
        let [low, upper @ ..] = rises;
        (rises.iter().sum(), (upper.iter().sum::<f32>() - BACKBEAT_LOW * low).max(0.0))
    }

    /// The correlation at a lag of `lag` frames, read between whole frames,
    /// against the strength's own power.
    fn correlation(&self, lag: f32) -> f32 {
        let correlation = &self.memory.correlation;
        let whole = (lag.max(0.0) as usize).min(LAGS - 2);
        let part = (lag - whole as f32).clamp(0.0, 1.0);
        let at = correlation[whole] + part * (correlation[whole + 1] - correlation[whole]);
        at / correlation[0].max(1e-9)
    }

    /// The length of candidate `i`'s beat, in frames.
    fn candidate(i: f32) -> f32 {
        SHORTEST_BEAT * (i / CANDIDATES as f32).exp2()
    }

    /// The beat to follow, in frames, its score, and whether it takes over
    /// from the beat being followed: that beat, found again, unless another
    /// has scored clearly better for a while.
    fn choose_tempo(&mut self) -> Option<(f32, f32, bool)> {
        if self.memory.correlation[0] <= 1e-6 {
            return None;
        }
        let mut best = 0;
        for i in 0..CANDIDATES {
            let lag = Self::candidate(i as f32);
            let score = (self.correlation(lag / 2.0) + self.correlation(lag) + self.correlation(2.0 * lag)) / 3.0;
            self.memory.scores[i] = score;
            if score > self.memory.scores[best] {
                best = i;
            }
        }
        let scores = &self.memory.scores;
        let at = |i: isize| scores[i.rem_euclid(CANDIDATES as isize) as usize];
        // The candidates wrap round: 0.8 s is 0.4 s an octave down
        let mut chosen = best as isize;
        let mut same = false;
        if let Some(beat) = &mut self.beat {
            let following = (CANDIDATES as f32 * (beat.frames / SHORTEST_BEAT).log2()).round() as isize;
            let near = (following - NEARBY as isize..=following + NEARBY as isize).fold(following, |m, i| if at(i) > at(m) { i } else { m });
            // Another tempo takes over once it has led for a while, not the
            // moment it's ahead: three against four has each ahead by turns,
            // and a dancer keeps to the one it found. A beat only just
            // claimed is held lightly, though: one better at once takes over
            // at once, and it grows patient as the beat is danced. Gliding to
            // a new tempo, that one keeps leading until the glide arrives
            let lead = if scores[best] > at(near) { (scores[best] / at(near).max(1e-6)).ln().min(1.0) } else { 0.0 };
            let danced = beat.position as f32 * beat.frames * FRAME_SECONDS;
            let patience = (danced - RIVAL_SECONDS).clamp(FRAME_SECONDS, RIVAL_SECONDS);
            beat.rival = if lead > 0.0 { beat.rival + (lead - beat.rival) * (FRAME_SECONDS / patience) } else { 0.0 };
            if beat.rival <= SWITCH.ln() {
                chosen = near;
                same = true;
            }
        }
        // Between candidates, at the top of a parabola through its neighbours
        let (before, peak, after) = (at(chosen - 1), at(chosen), at(chosen + 1));
        let curve = before - 2.0 * peak + after;
        let offset = if curve < 0.0 { (0.5 * (before - after) / curve).clamp(-0.5, 0.5) } else { 0.0 };
        let mut frames = Self::candidate((chosen as f32 + offset).rem_euclid(CANDIDATES as f32));
        // The same beat stays as near the one being followed as an octave
        // allows, so a ritardando isn't danced in a sudden double time
        if let (Some(beat), true) = (self.beat, same) {
            frames *= (beat.frames / frames).log2().round().exp2();
            let seconds = frames * FRAME_SECONDS;
            if !(SHORTEST_FOLLOWED..=LONGEST_FOLLOWED).contains(&seconds) {
                frames = Self::candidate(CANDIDATES as f32 * (frames / SHORTEST_BEAT).log2().rem_euclid(1.0));
            }
        }
        Some((frames, peak, self.beat.is_some() && !same))
    }

    /// The onset strength `back` frames before now, read between frames
    /// with a tent two frames wide, so a hit a little early or late still
    /// counts.
    fn strength_back(&self, back: f32) -> f32 {
        self.back(&self.memory.strength, back)
    }

    /// `signal` `back` frames before now, read as [`Self::strength_back`] reads it.
    fn back(&self, signal: &[f32; HISTORY], back: f32) -> f32 {
        let centre = back.floor() as isize;
        let mut sum = 0.0;
        for k in centre - 1..=centre + 2 {
            if k < 0 || k as usize >= HISTORY.min(self.frames) {
                continue;
            }
            let weight = (1.0 - (k as f32 - back).abs() / 2.0).max(0.0);
            sum += weight * signal[(self.frames + HISTORY - k as usize) % HISTORY];
        }
        sum
    }

    /// How far through a beat of `frames` it is now, 0 to 1: where a comb
    /// with a tooth each beat catches the most onset strength. And, from the
    /// clock's tooth, how much more backbeat lies half a beat away (see
    /// [`Beat::lean`]) and how clearly it's heard, 0 to 1.
    ///
    /// With the clock's phase and lean `near`, the best tooth near it,
    /// unless one elsewhere catches clearly more: music with as much on the
    /// offbeat as the beat would otherwise have the dance hopping between
    /// them. But a broken beat can hit harder on the and than on the beat,
    /// and its snare still lands on the beat, so the lean has its say: past
    /// [`TURN`] it turns half a beat, and leaning the other way, a jump to
    /// the and has to catch that much more.
    fn phase(&self, frames: f32, near: Option<(f64, f32)>) -> (f32, (f32, f32)) {
        let teeth = (frames.ceil() as usize).min(COMB_MOST);
        let mut caught = [0.0f32; COMB_MOST];
        for (since, caught) in caught.iter_mut().enumerate().take(teeth) {
            let mut weight = 1.0;
            for n in 0..COMB_BEATS {
                let back = since as f32 + n as f32 * frames;
                if back + 2.0 >= HISTORY as f32 {
                    break;
                }
                *caught += weight * self.strength_back(back);
                weight *= COMB_FADE;
            }
        }
        let most = |range: &mut dyn Iterator<Item = isize>| {
            range.map(|k| k.rem_euclid(teeth as isize) as usize).fold(None, |best: Option<usize>, k| match best {
                Some(b) if caught[b] >= caught[k] => Some(b),
                _ => Some(k),
            })
        };
        let anywhere = most(&mut (0..teeth as isize)).unwrap_or(0);
        let reach = (STICK * frames).round() as isize;
        let opposite = |since: usize| {
            let half = since as isize + (frames / 2.0).round() as isize;
            most(&mut (half - reach..=half + reach)).unwrap_or(since)
        };
        // How much more backbeat there is at `there` than at `here`, as a
        // log, and how clearly it's heard, 0 to 1
        let toward = |here: usize, there: usize| {
            let catch = caught[here].max(caught[there]) + 1e-6;
            let floor = BACKBEAT_FLOOR * catch;
            let (here, there) = (self.backbeat(frames, here), self.backbeat(frames, there));
            let log = ((there + floor) / (here + floor)).ln().clamp(-BACKBEAT_MOST.ln(), BACKBEAT_MOST.ln());
            (log, (here.max(there) / (BACKBEAT_CLEAR * catch)).min(1.0))
        };
        let (since, heard) = match near {
            Some((near, lean)) => {
                let expected = (near as f32 * frames - LATENCY_FRAMES).round() as isize;
                let nearby = most(&mut (expected - reach..=expected + reach)).unwrap_or(anywhere);
                let opposite = opposite(nearby);
                // Jumping to the and takes more the more backbeat is here,
                // but never less than any other jump
                let hold = if anywhere == opposite { HOLD * (-lean).max(0.0).exp() } else { HOLD };
                let since = if lean > TURN {
                    opposite
                } else if caught[anywhere] > hold * caught[nearby] {
                    anywhere
                } else {
                    nearby
                };
                (since, toward(nearby, opposite))
            }
            // Claiming a beat, the backbeat heard so far chooses at once
            None => {
                let opposite = opposite(anywhere);
                (if toward(anywhere, opposite).0 > TURN { opposite } else { anywhere }, (0.0, 0.0))
            }
        };
        (((since as f32 + LATENCY_FRAMES) / frames).fract(), heard)
    }

    /// How much backbeat there is `since` frames back, with beats of
    /// `frames`: the snare-like rises taken in turns, one beat added and the
    /// next taken away. A snare on two and four and none on one and three
    /// adds up; a hat on every beat cancels out.
    fn backbeat(&self, frames: f32, since: usize) -> f32 {
        let (mut sum, mut weight) = (0.0f32, 1.0);
        for n in 0..BACKBEAT_BEATS {
            let back = since as f32 + n as f32 * frames;
            if back + 2.0 >= HISTORY as f32 {
                break;
            }
            sum += weight * self.back(&self.memory.backbeat, back);
            weight *= -BACKBEAT_FADE;
        }
        sum.abs()
    }

    /// Moves `beat` on a frame, settling toward the tempo heard, `heard`
    /// frames a beat, and pulled toward where the comb finds the beat.
    fn follow(&self, beat: Beat, heard: f32) -> Beat {
        let frames = beat.frames + (heard - beat.frames) * (FRAME_SECONDS / TEMPO_SECONDS);
        let length = Beat { frames, ..beat }.length();
        // Where the comb puts the beat now, against where the clock would be
        // by now if left alone. The comb has the tempo heard, not the
        // clock's: one a little long would find the beat a little late, and
        // lengthen the clock's further
        let predicted = beat.position + 1.0 / length as f64;
        // Turning to the backbeat, it follows the tooth it's headed for, not
        // the one by the clock: that one moves as the clock does
        let near = beat.heading.unwrap_or(predicted.fract());
        let lean = if beat.heading.is_some() { f32::NEG_INFINITY } else { beat.lean };
        let (target, (toward, clear)) = self.phase(frames, Some((near, lean)));
        let target = target as f64;
        let error = (target - predicted.fract() + 0.5).rem_euclid(1.0) - 0.5;
        let step = (1.0 + (PULL * error).clamp(-MOST_PULL, MOST_PULL)) / length as f64;
        // Running late (a positive error) means the beat is shorter than
        // heard. Only near the beat, though: catching up with a beat found
        // afresh is the pull's job, and trimming through it would overshoot
        let trim = if error.abs() < LOCKED { (beat.trim + TRIM * error as f32).clamp(-MOST_TRIM, MOST_TRIM) } else { beat.trim };
        let (lean, heading) = match beat.heading {
            // Turning: on the beat there, what it leaned toward is under it
            None if beat.lean > TURN => (-beat.lean, Some(target)),
            Some(_) if error.abs() >= LOCKED => (beat.lean, Some(target)),
            // Learned only on the beat, or catching up would teach it nonsense
            _ if error.abs() < LOCKED => (beat.lean + (toward - beat.lean) * clear * (FRAME_SECONDS / LEAN_SECONDS), None),
            _ => (beat.lean, None),
        };
        Beat { frames, trim, position: beat.position + step, step, lean, heading, rival: beat.rival }
    }
}

impl Beat {
    /// How long the clock's beat lasts, in frames.
    fn length(&self) -> f32 {
        self.frames * (1.0 - self.trim)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::dsp::onsets::tests::{add_hit, test_break, Hit, Noise};

    const RATE: f32 = 48000.0;

    /// A kick: a sine falling from 150 Hz to 50, dying away.
    fn kick(n: usize) -> f32 {
        let t = n as f32 / RATE;
        let phase = std::f32::consts::TAU * (50.0 * t + 100.0 * 0.03 * (1.0 - (-t / 0.03).exp()));
        phase.sin() * (-t / 0.12).exp()
    }

    /// Four to the floor at `bpm` for `seconds`, with a closed hat on the
    /// offbeats and a soft pad under it all: the music, and when each kick
    /// lands in seconds.
    pub(crate) fn four_to_the_floor(bpm: f32, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let beat = 60.0 / bpm;
        let len = (seconds * RATE) as usize;
        let mut out: Vec<f32> = (0..len).map(|n| 0.05 * (std::f32::consts::TAU * 220.0 * n as f32 / RATE).sin()).collect();
        let mut noise = Noise(0x2545_f491);
        let mut kicks = Vec::new();
        let mut at = 0.0;
        while at < seconds {
            let start = (at * RATE) as usize;
            for (n, s) in out[start..].iter_mut().enumerate().take((0.4 * RATE) as usize) {
                *s += 0.8 * kick(n);
            }
            let hat = ((at + beat / 2.0) * RATE) as usize;
            for (n, s) in out.iter_mut().skip(hat).enumerate().take((0.05 * RATE) as usize) {
                *s += 0.15 * noise.next() * (-(n as f32) / (0.01 * RATE)).exp();
            }
            kicks.push(at);
            at += beat;
        }
        (out, kicks)
    }

    /// Plays `music` to a fresh tracker, noting its beat every 10 ms.
    fn listen(music: &[f32]) -> Vec<Option<(f64, f32)>> {
        let mut tracker = BeatTracker::new();
        tracker.prepare(RATE);
        let mut heard = Vec::new();
        for (n, &x) in music.iter().enumerate() {
            tracker.hear(x);
            if n % 480 == 479 {
                heard.push(tracker.beat());
            }
        }
        heard
    }

    /// How far each beat the tracker counts in the last `seconds` lands
    /// from the nearest of `hits`, in seconds.
    fn misses(heard: &[Option<(f64, f32)>], hits: &[f32], seconds: f32) -> Vec<f32> {
        let from = heard.len() - (seconds / FRAME_SECONDS) as usize;
        let mut misses = Vec::new();
        for (i, pair) in heard.windows(2).enumerate().skip(from) {
            if let [Some((before, _)), Some((after, _))] = pair {
                if after.floor() > before.floor() {
                    // A beat fell in this frame: where in it
                    let through = (after.floor() - before) / (after - before);
                    let at = (i as f64 + 1.0 + through) as f32 * FRAME_SECONDS;
                    let nearest = hits.iter().map(|hit| at - hit).fold(f32::MAX, |m, d| if d.abs() < m.abs() { d } else { m });
                    misses.push(nearest);
                }
            }
        }
        misses
    }

    #[test]
    fn test_four_to_the_floor_is_danced_on_the_kicks() {
        let (music, mut kicks) = four_to_the_floor(124.0, 12.0);
        kicks.push(kicks.last().unwrap() + 60.0 / 124.0);
        let heard = listen(&music);
        // Found within a few seconds, and held
        let found = heard.iter().position(Option::is_some).expect("a beat is found") as f32 * FRAME_SECONDS;
        assert!(found < 4.0, "found after {found} s");
        assert!(heard[(found / FRAME_SECONDS) as usize..].iter().all(Option::is_some), "the beat is held");
        let (_, seconds) = heard.last().unwrap().unwrap();
        assert!((seconds - 60.0 / 124.0).abs() < 0.01, "a {seconds} s beat");
        // The last few seconds' beats land on the kicks
        let misses = misses(&heard, &kicks, 4.0);
        assert!(misses.len() >= 7, "{misses:?}");
        assert!(misses.iter().all(|miss| miss.abs() < 0.005), "{misses:?}");
    }

    #[test]
    fn test_fast_and_slow_music_is_folded_into_dancing_tempo() {
        for (bpm, beat) in [(174.0, 2.0 * 60.0 / 174.0), (70.0, 30.0 / 70.0), (100.0, 0.6)] {
            let (music, _) = four_to_the_floor(bpm, 10.0);
            let heard = listen(&music);
            let (_, seconds) = heard.last().unwrap().unwrap_or_else(|| panic!("a beat at {bpm} BPM"));
            assert!((seconds - beat).abs() < 0.012, "{bpm} BPM is danced at {seconds} s, not {beat} s");
        }
    }

    #[test]
    fn test_a_swung_break_is_danced_on_its_beat() {
        // The Sampler's test break: two bars at 96 BPM, swung and pushed
        let (sample, times) = test_break();
        let bar = 4.0 * 60.0 / 96.0;
        let bars = 2.0 * bar;
        let one = &sample.left()[..(bars * RATE) as usize];
        let music: Vec<f32> = one.iter().copied().cycle().take(one.len() * 6).collect();
        let heard = listen(&music);
        let (_, seconds) = heard.last().unwrap().expect("a beat");
        assert!((seconds - 0.625).abs() < 0.01, "a {seconds} s beat");
        // Its beats land on the quarter notes
        let quarters: Vec<f32> = (0..=6 * 8).map(|q| q as f32 * 0.625 + times[0]).collect();
        let misses = misses(&heard, &quarters, 4.0);
        assert!(misses.iter().all(|miss| miss.abs() < 0.012), "{misses:?}");
    }

    /// A broken beat at 116 BPM for `seconds`, as Canopy's: the kick on one
    /// and harder on the ands of two and three, the snare on two and four,
    /// and a hat on every eighth. The music, and when each beat falls.
    fn broken_beat(seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let sixteenth = 15.0 / 116.0;
        let mut out = vec![0.0; (seconds * RATE) as usize];
        let mut noise = Noise(7);
        let mut bar = 0.0;
        while bar + 16.0 * sixteenth < seconds {
            for (step, hit, level) in [(0, Hit::Kick, 0.5), (6, Hit::Kick, 0.9), (10, Hit::Kick, 0.9), (4, Hit::Snare, 0.3), (12, Hit::Snare, 0.3)] {
                add_hit(&mut out, hit, bar + step as f32 * sixteenth, level, &mut noise);
            }
            for step in (0..16).step_by(2) {
                let start = ((bar + step as f32 * sixteenth) * RATE) as usize;
                for (n, s) in out.iter_mut().skip(start).enumerate().take((0.05 * RATE) as usize) {
                    *s += 0.1 * noise.next() * (-(n as f32) / (0.01 * RATE)).exp();
                }
            }
            bar += 16.0 * sixteenth;
        }
        let beats = (0..).map(|beat| beat as f32 * 4.0 * sixteenth).take_while(|&at| at < seconds + 1.0).collect();
        (out, beats)
    }

    #[test]
    fn test_a_broken_beat_is_danced_on_its_snare_not_its_kicks() {
        let (music, beats) = broken_beat(30.0);
        let heard = listen(&music);
        let (_, seconds) = heard.last().unwrap().expect("a beat");
        assert!((seconds - 60.0 / 116.0).abs() < 0.01, "a {seconds} s beat");
        // The kicks between the beats catch more than the one on it, but the
        // last few seconds' beats land on the beats, not half a beat out
        let misses = misses(&heard, &beats, 6.0);
        assert!(misses.len() >= 10, "{misses:?}");
        assert!(misses.iter().all(|miss| miss.abs() < 0.012), "{misses:?}");
    }

    /// Three against four at 100 BPM, as Interlock plays it: a gong where
    /// they meet, every three beats, and a bell on every sixteenth that
    /// three or four divides, louder on the beat, and more so in every other
    /// three cycles than the rest. For the first `full` seconds a second bell
    /// fills the sixteenths between, as Interlock's does while its LFO lets
    /// it. The music, and when each beat falls.
    fn three_against_four(full: f32, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let sixteenth = 15.0 / 100.0;
        let mut out = vec![0.0; (seconds * RATE) as usize];
        let mut noise = Noise(3);
        let bell = |out: &mut [f32], at: f32, hz: f32, level: f32| {
            let start = (at * RATE) as usize;
            for (n, s) in out.iter_mut().skip(start).enumerate().take((0.6 * RATE) as usize) {
                let t = n as f32 / RATE;
                *s += level * (std::f32::consts::TAU * hz * t).sin() * (-t / 0.25).exp();
            }
        };
        let pitches = [523.0, 587.0, 659.0, 784.0, 880.0];
        for step in 0..(seconds / sixteenth) as usize {
            let at = step as f32 * sixteenth;
            let hz = pitches[step * 7 % pitches.len()];
            match step % 12 {
                0 => add_hit(&mut out, Hit::Kick, at, 0.6, &mut noise),
                4 | 8 => bell(&mut out, at, hz, if step / 36 % 2 == 0 { 0.6 } else { 0.4 }),
                3 | 6 | 9 => bell(&mut out, at, hz, 0.35),
                _ if at < full => bell(&mut out, at, hz * 1.5, 0.35),
                _ => {}
            }
        }
        let beats = (0..).map(|beat| beat as f32 * 4.0 * sixteenth).take_while(|&at| at < seconds + 1.0).collect();
        (out, beats)
    }

    #[test]
    fn test_three_against_four_keeps_the_beat_it_found() {
        // Once the second bell stops, the bell after each gap lands every
        // dotted eighth, and that tempo scores about as well as the beat's,
        // ahead while the accents are soft. A dancer on the beat stays on it
        let (music, beats) = three_against_four(15.0, 40.0);
        let heard = listen(&music);
        for (at, beat) in heard.iter().enumerate().skip(1500) {
            let (_, seconds) = beat.unwrap_or_else(|| panic!("no beat at {} s", at as f32 * FRAME_SECONDS));
            assert!((seconds - 0.6).abs() < 0.012, "a {seconds} s beat at {} s", at as f32 * FRAME_SECONDS);
        }
        let misses = misses(&heard, &beats, 10.0);
        assert!(misses.iter().all(|miss| miss.abs() < 0.025), "{misses:?}");
    }

    #[test]
    fn test_a_drone_noise_and_silence_have_no_beat() {
        let len = (8.0 * RATE) as usize;
        let drone: Vec<f32> = (0..len).map(|n| 0.4 * (std::f32::consts::TAU * 110.0 * n as f32 / RATE).sin()).collect();
        assert!(listen(&drone).iter().all(Option::is_none));
        let mut noise = Noise(99);
        let hiss: Vec<f32> = (0..len).map(|_| 0.2 * noise.next()).collect();
        assert!(listen(&hiss).iter().all(Option::is_none));
        assert!(listen(&vec![0.0; len]).iter().all(Option::is_none));
    }

    #[test]
    fn test_hits_at_random_have_no_beat() {
        // Kicks at no steady spacing: 0.2 to 1 s apart
        let mut noise = Noise(12345);
        let mut music = vec![0.0f32; (12.0 * RATE) as usize];
        let mut at = 0.1;
        while at < 11.5 {
            let start = (at * RATE) as usize;
            for (n, s) in music[start..].iter_mut().enumerate().take((0.4 * RATE) as usize) {
                *s += 0.8 * kick(n);
            }
            at += 0.6 + 0.4 * noise.next();
        }
        let claimed = listen(&music).iter().filter(|beat| beat.is_some()).count();
        assert!(claimed < 100, "a beat claimed for {claimed} frames");
    }

    #[test]
    fn test_when_the_music_stops_the_beat_goes() {
        let (mut music, _) = four_to_the_floor(120.0, 6.0);
        music.resize((10.0 * RATE) as usize, 0.0);
        let heard = listen(&music);
        assert!(heard[590].is_some(), "dancing while it plays");
        assert!(heard.last().unwrap().is_none(), "and not after");
    }

    #[test]
    fn test_the_beat_never_steps_back() {
        let (music, _) = four_to_the_floor(124.0, 12.0);
        let positions: Vec<f64> = listen(&music).into_iter().flatten().map(|(position, _)| position).collect();
        assert!(positions.windows(2).all(|w| w[1] >= w[0]), "the beat steps back");
    }

    #[test]
    fn test_forgetting_starts_afresh() {
        let (music, _) = four_to_the_floor(124.0, 6.0);
        let mut tracker = BeatTracker::new();
        for &x in &music {
            tracker.hear(x);
        }
        assert!(tracker.beat().is_some());
        tracker.forget();
        assert!(tracker.beat().is_none());
        assert_eq!(tracker.memory.correlation[0], 0.0);
    }
}

