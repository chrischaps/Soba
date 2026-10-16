//! The Clock's Link source: its time from an Ableton Link session.
//!
//! Each block, a [`LinkFollower`] reads the session's tempo, beat and
//! transport for the moment the block's first sample reaches the speakers,
//! and the Clock plays them out across the block. What the player does on
//! the Clock goes the other way: turning Tempo proposes the tempo to the
//! session, and switching Run starts or stops every peer (start/stop sync).
//!
//! What a peer does comes back as two counters in the Clock's readout, which
//! the app watches to move the Tempo knob and the Run switch to match (see
//! `app::synth_app::link_follow`). The knob and switch then say what the
//! session does, so turning the knob carries on from the session's tempo
//! rather than jumping back to the knob's old one.
//!
//! Everything here runs on the audio thread. The SDK's audio calls
//! (`capture_audio_session_state`, `commit_audio_session_state`,
//! `clock_micros`) never block or allocate, and the session state they
//! fill is made with the Clock, off the audio thread.

use crate::ableton_link::{self, AblLink, SessionState, QUANTUM};

/// A tempo change smaller than this is the same tempo: the knob's f32 round
/// trip, not a turn.
const SAME_TEMPO: f64 = 0.005;

/// What the session says for one block.
#[derive(Clone, Copy, Debug)]
pub struct LinkBlock {
    /// When the block's first sample reaches the speakers, on Link's clock
    /// (microseconds).
    pub host: i64,
    /// The session's tempo.
    pub tempo: f64,
    /// The session's beat at the block's first sample, on Soba's count:
    /// beat 0 is where the transport last started.
    pub beat: f64,
    /// How far the beat moves each sample.
    pub beats_per_sample: f64,
    /// Whether the session's transport is playing. The Clock runs once the
    /// beat reaches 0 too: with peers, a start waits for the next bar.
    pub playing: bool,
}

impl LinkBlock {
    /// The beat `sample` samples into the block.
    #[inline]
    pub fn beat_at(&self, sample: usize) -> f64 {
        self.beat + sample as f64 * self.beats_per_sample
    }
}

/// Keeps a Clock on a Link session.
pub struct LinkFollower {
    /// Filled by `capture_audio_session_state` each block.
    state: SessionState,
    /// Smooths the callback's jittery time into an even one.
    filter: HostTimeFilter,
    /// Samples run since the follower was made: the filter's time line.
    samples: u64,
    /// The Tempo knob and the Run switch at the last block, to notice the
    /// player moving them. `None` until the first block on Link.
    knob: Option<f64>,
    run: Option<bool>,
    /// The tempo and transport the session had, as of the last block,
    /// counting this Clock's own proposals: a difference is a peer's doing.
    tempo_seen: f64,
    playing_seen: bool,
    /// How many times a peer changed the tempo, or started or stopped.
    pub tempo_changes: u32,
    pub play_changes: u32,
    /// What the last block found.
    pub last: Option<LinkBlock>,
}

impl Default for LinkFollower {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkFollower {
    /// A follower that hasn't run yet. Allocates (the session state), so
    /// it's made with the Clock, off the audio thread.
    pub fn new() -> Self {
        Self {
            state: SessionState::new(),
            filter: HostTimeFilter::new(),
            samples: 0,
            knob: None,
            run: None,
            tempo_seen: 0.0,
            playing_seen: false,
            tempo_changes: 0,
            play_changes: 0,
            last: None,
        }
    }

    /// Forgets the knob and switch, so the next block on Link is a fresh
    /// start: as when the Clock switches to Link, or the patch restarts.
    ///
    /// REAL-TIME SAFE.
    pub fn rest(&mut self) {
        self.knob = None;
        self.run = None;
        self.last = None;
        self.filter.clear();
    }

    /// Reads the session for a block of `frames` at `sample_rate`, after
    /// proposing what the player changed on the Clock: `knob`, the Tempo
    /// knob, and `run`, the Run switch. `extra_latency` is Soba's own delay
    /// after the callback, in samples (the output limiter's look-ahead).
    ///
    /// The first block on Link sets the session's tempo and transport from
    /// the Clock if it has the session to itself, and otherwise takes the
    /// session's, as a peer's change.
    ///
    /// REAL-TIME SAFE: the SDK's audio calls and arithmetic.
    pub fn begin_block(&mut self, link: &AblLink, knob: f64, run: bool, frames: usize, sample_rate: f32, extra_latency: usize) -> LinkBlock {
        ableton_link::pulse();
        let sample_rate = sample_rate.max(1.0) as f64;
        let micros_per_sample = 1e6 / sample_rate;
        let callback = self.filter.host_time(link.clock_micros(), self.samples, micros_per_sample);
        let host = callback + ableton_link::output_latency_micros() + (extra_latency as f64 * micros_per_sample) as i64;
        self.samples += frames as u64;

        link.capture_audio_session_state(&mut self.state);
        let alone = ableton_link::peers() == 0;
        let mut commit = false;

        // The Tempo knob: turned, or the Clock's first block with the
        // session to itself
        let turned = match self.knob {
            Some(previous) => (knob - previous).abs() > f64::EPSILON,
            None => alone,
        };
        if turned && (knob - self.state.tempo()).abs() > SAME_TEMPO {
            self.state.set_tempo(knob, host);
            commit = true;
        }
        if self.knob.is_none() {
            // Whatever the session plays now is news to the Clock
            self.tempo_seen = f64::NAN;
            self.playing_seen = !self.state.is_playing();
        }
        if commit {
            self.tempo_seen = self.state.tempo();
        }
        self.knob = Some(knob);

        // The Run switch: flipped, or the first block, alone. A start
        // begins from the top: beat 0, which with peers is the next bar
        let flipped = match self.run {
            Some(previous) => previous != run,
            None => alone,
        };
        if flipped && run != self.state.is_playing() {
            if run {
                self.state.set_is_playing_and_request_beat_at_time(true, host, 0.0, QUANTUM);
            } else {
                self.state.set_is_playing(false, host);
            }
            self.playing_seen = run;
            commit = true;
        }
        self.run = Some(run);

        // A start the Clock didn't make: a peer's. Count from its downbeat,
        // as a Run of our own would
        if self.state.is_playing() && !self.playing_seen {
            self.state.request_beat_at_start_playing_time(0.0, QUANTUM);
            commit = true;
        }
        if commit {
            link.commit_audio_session_state(&self.state);
        }

        // What's left differing is a peer's doing
        let tempo = self.state.tempo();
        if (tempo - self.tempo_seen).abs() > SAME_TEMPO || self.tempo_seen.is_nan() {
            self.tempo_seen = tempo;
            self.tempo_changes = self.tempo_changes.wrapping_add(1);
        }
        let playing = self.state.is_playing();
        if playing != self.playing_seen {
            self.playing_seen = playing;
            self.play_changes = self.play_changes.wrapping_add(1);
        }

        let block = LinkBlock {
            host,
            tempo,
            beat: self.state.beat_at_time(host, QUANTUM),
            beats_per_sample: tempo / 60.0 / sample_rate,
            playing,
        };
        self.last = Some(block);
        block
    }
}

/// Smooths callback times into an even line against the sample count.
///
/// An audio callback doesn't run at quite even intervals, so the clock read
/// in it jitters by a fraction of a block. Samples, though, are even: a line
/// fitted through recent (samples, clock) pairs gives each block the time its
/// sample count says, with the jitter averaged out. The SDK's own filter
/// does the same, but allocates when it's cleared.
pub struct HostTimeFilter {
    /// Recent (samples, micros) pairs, relative to `origin`, in a ring.
    points: [(f64, f64); Self::POINTS],
    count: usize,
    head: usize,
    /// The first pair since the last clear, which the others are taken
    /// relative to, keeping the sums small enough for f64.
    origin: Option<(u64, i64)>,
}

impl Default for HostTimeFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostTimeFilter {
    /// Blocks fitted over: a few seconds at the usual block sizes.
    const POINTS: usize = 256;
    /// A reading this far off the line, in microseconds, means the stream
    /// stopped for a while (or the device changed): start the line afresh.
    const JUMP: f64 = 20_000.0;

    pub fn new() -> Self {
        Self { points: [(0.0, 0.0); Self::POINTS], count: 0, head: 0, origin: None }
    }

    /// Forgets the line.
    pub fn clear(&mut self) {
        self.count = 0;
        self.head = 0;
        self.origin = None;
    }

    /// The smoothed time, in microseconds, of the sample `samples` into the
    /// stream, read at `micros` on the clock. `micros_per_sample` is the
    /// slope to assume until there are points enough to fit one.
    pub fn host_time(&mut self, micros: i64, samples: u64, micros_per_sample: f64) -> i64 {
        let (s0, m0) = *self.origin.get_or_insert((samples, micros));
        let x = samples.wrapping_sub(s0) as f64;
        let y = (micros - m0) as f64;
        if self.count >= 2 && (self.fit(micros_per_sample, x) - y).abs() > Self::JUMP {
            self.clear();
            return self.host_time(micros, samples, micros_per_sample);
        }
        self.points[self.head] = (x, y);
        self.head = (self.head + 1) % Self::POINTS;
        self.count = (self.count + 1).min(Self::POINTS);
        m0 + self.fit(micros_per_sample, x).round() as i64
    }

    /// Where the line fitted through the points puts `x`.
    fn fit(&self, micros_per_sample: f64, x: f64) -> f64 {
        let n = self.count as f64;
        let points = &self.points[..self.count];
        let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
        let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
        let (mut covariance, mut variance) = (0.0, 0.0);
        for &(px, py) in points {
            covariance += (px - mean_x) * (py - mean_y);
            variance += (px - mean_x) * (px - mean_x);
        }
        // Until the samples span something, the sample rate is the slope
        let slope = if variance > 0.0 && self.count >= 8 { covariance / variance } else { micros_per_sample };
        mean_y + slope * (x - mean_x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;
    const FRAMES: usize = 512;

    /// A session of its own, off the network: the tests never start the
    /// app's, so the follower has it to itself.
    fn session() -> AblLink {
        let link = AblLink::new(120.0);
        link.enable_start_stop_sync(true);
        link
    }

    /// Changes the session as a peer would, from outside the audio thread.
    fn as_a_peer(link: &AblLink, change: impl FnOnce(&mut SessionState, i64)) {
        let mut state = SessionState::new();
        link.capture_app_session_state(&mut state);
        change(&mut state, link.clock_micros());
        link.commit_app_session_state(&state);
    }

    /// Runs blocks with `knob` and `run` until `done` says the session has
    /// caught up: Link passes changes between its app and audio sides a
    /// moment later. Gives up after two seconds.
    fn blocks_until(link: &AblLink, follower: &mut LinkFollower, knob: f64, run: bool, done: impl Fn(&LinkBlock) -> bool) -> LinkBlock {
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let block = follower.begin_block(link, knob, run, FRAMES, RATE, 0);
            if done(&block) {
                return block;
            }
            assert!(std::time::Instant::now() < give_up, "the session never caught up: {block:?}");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// The session as the app side sees it, once `done` says it has caught up.
    fn app_side_until(link: &AblLink, done: impl Fn(&SessionState) -> bool) -> SessionState {
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut state = SessionState::new();
        loop {
            link.capture_app_session_state(&mut state);
            if done(&state) {
                return state;
            }
            assert!(std::time::Instant::now() < give_up, "the app side never caught up");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn alone_the_clock_sets_the_session_and_starts_from_the_top() {
        let link = session();
        let mut follower = LinkFollower::new();
        let block = follower.begin_block(&link, 96.0, true, FRAMES, RATE, 0);
        assert_eq!(block.tempo, 96.0, "the knob's tempo");
        assert!(block.playing, "Run starts the session");
        assert!(block.beat.abs() < 1e-3, "from beat 0, not {}", block.beat);
        assert!((block.beats_per_sample - 96.0 / 60.0 / RATE as f64).abs() < 1e-12);

        // Turning the knob proposes the tempo; it's the Clock's own change
        let changes = follower.tempo_changes;
        let block = follower.begin_block(&link, 140.0, true, FRAMES, RATE, 0);
        assert_eq!(block.tempo, 140.0);
        assert_eq!(follower.tempo_changes, changes, "not a peer's");

        // Switching Run off stops it, and the session hears both
        let block = follower.begin_block(&link, 140.0, false, FRAMES, RATE, 0);
        assert!(!block.playing);
        app_side_until(&link, |state| !state.is_playing() && state.tempo() == 140.0);
        assert_eq!(follower.tempo_changes, changes);
    }

    #[test]
    fn a_peers_tempo_and_transport_are_counted_and_followed() {
        let link = session();
        let mut follower = LinkFollower::new();
        follower.begin_block(&link, 120.0, true, FRAMES, RATE, 0);
        app_side_until(&link, |state| state.is_playing());
        let (tempos, plays) = (follower.tempo_changes, follower.play_changes);

        // The knob doesn't fight a peer's tempo
        as_a_peer(&link, |state, now| state.set_tempo(87.5, now));
        blocks_until(&link, &mut follower, 120.0, true, |block| block.tempo == 87.5);
        assert_eq!(follower.tempo_changes, tempos + 1);

        // A peer's Stop stops the Clock, with Run still on
        as_a_peer(&link, |state, now| state.set_is_playing(false, now));
        blocks_until(&link, &mut follower, 87.5, true, |block| !block.playing);
        assert_eq!(follower.play_changes, plays + 1);

        // The app moves Run to match: nothing proposed, no change counted
        let block = follower.begin_block(&link, 87.5, false, FRAMES, RATE, 0);
        assert!(!block.playing);
        assert_eq!(follower.play_changes, plays + 1);

        // A peer's Start counts from its downbeat
        as_a_peer(&link, |state, now| state.set_is_playing(true, now));
        let block = blocks_until(&link, &mut follower, 87.5, false, |block| block.playing);
        assert_eq!(follower.play_changes, plays + 2);
        assert!((0.0..1.0).contains(&block.beat), "beat {} just after the start", block.beat);
    }

    #[test]
    fn the_latency_puts_the_block_later_on_the_session() {
        let link = session();
        let mut early = LinkFollower::new();
        let mut late = LinkFollower::new();
        early.begin_block(&link, 120.0, true, FRAMES, RATE, 0);
        late.begin_block(&link, 120.0, true, FRAMES, RATE, 0);
        let a = early.begin_block(&link, 120.0, true, FRAMES, RATE, 0);
        // 2400 samples of look-ahead at 48 kHz: 50 ms, a tenth of a beat
        let b = late.begin_block(&link, 120.0, true, FRAMES, RATE, 2400);
        let gap = (b.host - a.host) as f64 / 1e6;
        assert!((gap - 0.05).abs() < 0.005, "{gap} s later");
    }

    #[test]
    fn the_filter_evens_out_a_jittery_callback() {
        let mut filter = HostTimeFilter::new();
        let per_sample = 1e6 / 48_000.0;
        let mut worst: f64 = 0.0;
        for block in 0..1000u64 {
            let samples = block * 512;
            let truth = 5_000_000.0 + samples as f64 * per_sample;
            // The callback runs up to a millisecond late, unevenly
            let jitter = ((block * 7919) % 1000) as f64;
            let smoothed = filter.host_time((truth + jitter) as i64, samples, per_sample);
            if block > 300 {
                // Off by the average lateness at most, and steady
                worst = worst.max((smoothed as f64 - truth - 500.0).abs());
            }
        }
        assert!(worst < 150.0, "{worst} µs from the even line");
    }

    #[test]
    fn the_filter_starts_afresh_after_the_stream_stops() {
        let mut filter = HostTimeFilter::new();
        let per_sample = 1e6 / 48_000.0;
        for block in 0..100u64 {
            filter.host_time((block as f64 * 512.0 * per_sample) as i64, block * 512, per_sample);
        }
        // Ten seconds of silence, then the stream carries on
        let resumed = 10_000_000 + (100.0 * 512.0 * per_sample) as i64;
        assert_eq!(filter.host_time(resumed, 100 * 512, per_sample), resumed);
    }
}
