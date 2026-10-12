//! The cue: a recording heard straight from a module's face.
//!
//! A Sampler's ▶ plays its file here, beside the patch rather than through
//! it: dry, at a fixed level, mixed into the output after the patch and its
//! limiter, whether or not the transport runs. It's how a sampler's or a
//! DAW's file preview works: you hear the file itself, not the patch's
//! effects on it.
//!
//! One cue plays at a time. A new one replaces the old over a short fade,
//! as a stop does, so neither clicks. The recording is the `Arc` the
//! Sampler already has; when the cue is done with it, it goes back to the
//! UI to be let go of there, so nothing is freed on the audio thread.

use std::sync::Arc;

use crate::dsp::SampleData;

use super::commands::NodeId;

/// How loud a cue plays: 6 dB down, so a file normalized to full scale
/// sits under the patch's peaks rather than over them.
pub const CUE_LEVEL: f32 = 0.5;

/// How long a cue takes to come in: just enough that starting mid-sound
/// doesn't click, and short enough to keep a drum's attack.
const ATTACK_SECONDS: f32 = 0.001;

/// How long a cue takes to go when it's stopped or replaced.
const FADE_SECONDS: f32 = 0.005;

/// How long before its end a cue fades to nothing, as a Sampler's voice does.
const END_FADE_SECONDS: f32 = 0.003;

/// A recording to hear from a module's face.
#[derive(Clone, Debug)]
pub struct Cue {
    /// The node it's heard from, reported back with where it's got to.
    pub node_id: NodeId,
    /// The recording, as the module plays it.
    pub sample: Arc<SampleData>,
    /// Where it plays from and to, 0 to 1 through the recording.
    pub from: f32,
    pub to: f32,
    /// Transposed by this many semitones (a Sampler's Tune and Fine).
    pub semitones: f32,
}

/// A cue playing.
struct Voice {
    node_id: NodeId,
    sample: Arc<SampleData>,
    /// Where it reads, in frames of the recording.
    position: f64,
    /// Where it stops.
    end: f64,
    /// How fast it reads against the recording's own rate: 1 is as recorded.
    pitch: f64,
    /// The envelope, 0 to 1.
    envelope: f32,
    /// What the envelope falls by each sample once it's fading, or 0.
    fall: f32,
}

impl Voice {
    fn new(cue: Cue) -> Self {
        let frames = cue.sample.frames() as f64;
        let at = |place: f32| place.clamp(0.0, 1.0) as f64 * frames;
        let (start, end) = (at(cue.from.min(cue.to)), at(cue.from.max(cue.to)));
        Self {
            node_id: cue.node_id,
            sample: cue.sample,
            position: start,
            end,
            pitch: (cue.semitones.clamp(-48.0, 48.0) as f64 / 12.0).exp2(),
            envelope: 0.0,
            fall: 0.0,
        }
    }

    /// Lets go over [`FADE_SECONDS`], from wherever the envelope is.
    fn fade(&mut self, sample_rate: f32) {
        if self.fall == 0.0 {
            self.fall = self.envelope.max(1e-6) / (FADE_SECONDS * sample_rate).max(1.0);
        }
    }

    /// Plays one frame at `rate` recorded frames per output frame, or
    /// returns `None` once it's over.
    #[inline]
    fn step(&mut self, rate: f64, rise: f32, end_fade: f64) -> Option<(f32, f32)> {
        if self.fall > 0.0 {
            self.envelope -= self.fall;
            if self.envelope <= 0.0 {
                return None;
            }
        } else if self.envelope < 1.0 {
            self.envelope = (self.envelope + rise).min(1.0);
        }
        let left_to_play = self.end - self.position;
        if left_to_play <= 0.0 {
            return None;
        }
        // Reaching nothing on the last frame before the end
        let edge = ((left_to_play / rate - 1.0) / end_fade).clamp(0.0, 1.0) as f32;
        let gain = self.envelope * edge;
        let (left, right) = self.sample.read(self.position);
        self.position += rate;
        Some((left * gain, right * gain))
    }

    /// How far through the recording it is, 0 to 1.
    fn place(&self) -> f32 {
        (self.position / self.sample.frames().max(1) as f64).clamp(0.0, 1.0) as f32
    }
}

/// Plays cues into the output, on the audio thread.
///
/// REAL-TIME SAFE: playing, replacing and stopping cues allocate nothing,
/// and recordings it's done with wait in [`take_retired`](Self::take_retired)
/// to be handed back.
#[derive(Default)]
pub struct CuePlayer {
    /// The cue playing.
    voice: Option<Voice>,
    /// The one it replaced, or that was stopped, fading out.
    tail: Option<Voice>,
    /// Recordings done with, waiting to be handed back.
    retired: [Option<Arc<SampleData>>; 3],
    /// The node whose cue ended since it was last reported.
    ended: Option<NodeId>,
}

impl CuePlayer {
    /// Starts a cue, fading out the one playing, or with `None` stops it.
    pub fn play(&mut self, cue: Option<Cue>, sample_rate: f32) {
        if let Some(voice) = self.voice.take() {
            // Still fading out of an older one: that goes now
            if let Some(older) = self.tail.take() {
                self.retire(older.sample);
            }
            let mut voice = voice;
            voice.fade(sample_rate);
            if cue.as_ref().is_none_or(|cue| cue.node_id != voice.node_id) {
                self.ended = Some(voice.node_id);
            }
            self.tail = Some(voice);
        }
        if let Some(cue) = cue {
            self.ended = self.ended.filter(|&node_id| node_id != cue.node_id);
            self.voice = Some(Voice::new(cue));
        }
    }

    /// Whether anything is sounding, or fading.
    pub fn is_playing(&self) -> bool {
        self.voice.is_some() || self.tail.is_some()
    }

    /// Adds the cue into `output` (interleaved, `channels` wide) at
    /// [`CUE_LEVEL`], duplicating to any channels beyond stereo.
    ///
    /// REAL-TIME SAFE.
    pub fn mix(&mut self, output: &mut [f32], channels: usize, sample_rate: f32) {
        if channels == 0 || !self.is_playing() {
            return;
        }
        let rise = 1.0 / (ATTACK_SECONDS * sample_rate).max(1.0);
        let end_fade = (END_FADE_SECONDS * sample_rate).max(1.0) as f64;
        for frame in output.chunks_exact_mut(channels) {
            let (mut left, mut right) = (0.0, 0.0);
            for (playing, slot) in [(true, &mut self.voice), (false, &mut self.tail)] {
                let Some(voice) = slot else { continue };
                // Each recording's rate against the engine's, so it plays in tune
                let rate = (voice.pitch * (voice.sample.sample_rate() / sample_rate) as f64).max(1e-3);
                match voice.step(rate, rise, end_fade) {
                    Some((l, r)) => {
                        left += l;
                        right += r;
                    }
                    None => {
                        let done = slot.take().expect("just matched");
                        if playing {
                            self.ended = Some(done.node_id);
                        }
                        Self::retire_into(&mut self.retired, done.sample);
                    }
                }
            }
            let (left, right) = (left * CUE_LEVEL, right * CUE_LEVEL);
            frame[0] = (frame[0] + left).clamp(-1.0, 1.0);
            if channels >= 2 {
                frame[1] = (frame[1] + right).clamp(-1.0, 1.0);
            }
            for sample in frame.iter_mut().skip(2) {
                *sample = (*sample + (left + right) * 0.5).clamp(-1.0, 1.0);
            }
            if !self.is_playing() {
                break;
            }
        }
    }

    /// What to tell the UI after a callback: the node whose cue is playing
    /// and where it is (0 to 1 through the recording), or once, the node
    /// whose cue just ended, with `None`.
    pub fn report(&mut self) -> Option<(NodeId, Option<f32>)> {
        match (&self.voice, self.ended.take()) {
            (Some(voice), _) => Some((voice.node_id, Some(voice.place()))),
            (None, Some(node_id)) => Some((node_id, None)),
            (None, None) => None,
        }
    }

    /// Hands over a recording the cue has let go of, if one is waiting.
    pub fn take_retired(&mut self) -> Option<Arc<SampleData>> {
        self.retired.iter_mut().find_map(Option::take)
    }

    fn retire(&mut self, sample: Arc<SampleData>) {
        Self::retire_into(&mut self.retired, sample);
    }

    /// Holds a recording to hand back, or drops it here if there's no room
    /// (which the three slots shouldn't allow: a voice and a tail, and one
    /// more between hand-backs).
    fn retire_into(retired: &mut [Option<Arc<SampleData>>], sample: Arc<SampleData>) {
        match retired.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(sample),
            None => {
                debug_assert!(false, "no room to retire a cue's recording");
                drop(sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48000.0;

    fn ramp(frames: usize) -> Arc<SampleData> {
        Arc::new(SampleData::mono((0..frames).map(|n| n as f32 / frames as f32).collect(), RATE))
    }

    fn cue(node_id: NodeId, sample: &Arc<SampleData>, from: f32, to: f32) -> Option<Cue> {
        Some(Cue { node_id, sample: sample.clone(), from, to, semitones: 0.0 })
    }

    /// `frames` of stereo output from a player, in one callback.
    fn render(player: &mut CuePlayer, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames * 2];
        player.mix(&mut output, 2, RATE);
        output.iter().step_by(2).copied().collect()
    }

    fn drain(player: &mut CuePlayer) -> usize {
        std::iter::from_fn(|| player.take_retired()).count()
    }

    #[test]
    fn test_plays_from_to_at_the_cue_level_then_hands_the_recording_back() {
        let sample = ramp(4800);
        let mut player = CuePlayer::default();
        // 25% to 75%: frames 1200 to 3600
        player.play(cue(7, &sample, 0.25, 0.75), RATE);
        let out = render(&mut player, 4800);
        for n in [100, 1000, 2000] {
            let expected = (1200 + n) as f32 / 4800.0 * CUE_LEVEL;
            assert!((out[n] - expected).abs() < 1e-4, "at {n}: {} vs {expected}", out[n]);
        }
        assert!(out[..2400].windows(2).all(|w| (w[1] - w[0]).abs() < 0.01), "no clicks");
        assert!(out[2400..].iter().all(|&s| s == 0.0), "silent past its end");
        assert_eq!(player.report(), Some((7, None)), "says it ended, once");
        assert_eq!(player.report(), None);
        assert_eq!(drain(&mut player), 1);
        assert_eq!(Arc::strong_count(&sample), 1, "nothing of it kept");
    }

    #[test]
    fn test_reports_where_it_is_while_it_plays() {
        let sample = ramp(48000);
        let mut player = CuePlayer::default();
        player.play(cue(3, &sample, 0.5, 1.0), RATE);
        render(&mut player, 4800);
        let (node_id, place) = player.report().unwrap();
        assert_eq!(node_id, 3);
        assert!((place.unwrap() - 0.6).abs() < 1e-3);
    }

    #[test]
    fn test_tune_transposes_and_another_rate_plays_in_tune() {
        // An octave up reads twice as fast: 0.5 s of recording in 0.25 s
        let sample = ramp(24000);
        let mut player = CuePlayer::default();
        player.play(Some(Cue { semitones: 12.0, ..cue(1, &sample, 0.0, 1.0).unwrap() }), RATE);
        let out = render(&mut player, 24000);
        assert!(out[11000] > 0.4 * CUE_LEVEL);
        assert!(out[12010..].iter().all(|&s| s == 0.0));

        // Recorded at 24 kHz, it takes as long as it lasts
        let slow = Arc::new(SampleData::mono(vec![0.5; 12000], 24000.0));
        player.play(cue(1, &slow, 0.0, 1.0), RATE);
        let out = render(&mut player, 30000);
        assert!(out[23000] > 0.24);
        assert!(out[24010..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_a_new_cue_replaces_the_old_without_a_click() {
        let first = Arc::new(SampleData::mono(vec![0.5; 48000], RATE));
        let second = Arc::new(SampleData::mono(vec![-0.5; 48000], RATE));
        let mut player = CuePlayer::default();
        player.play(cue(1, &first, 0.0, 1.0), RATE);
        let mut out = render(&mut player, 1000);
        player.play(cue(2, &second, 0.0, 1.0), RATE);
        out.extend(render(&mut player, 1000));
        let largest = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(largest < 0.01, "{largest}");
        assert!((out[1999] + 0.5 * CUE_LEVEL).abs() < 1e-4, "the new one plays");
        // The first went as soon as it had faded
        assert_eq!(drain(&mut player), 1);
        assert_eq!(Arc::strong_count(&first), 1);
        let (node_id, place) = player.report().unwrap();
        assert_eq!(node_id, 2);
        assert!((place.unwrap() - 1000.0 / 48000.0).abs() < 1e-5);
    }

    #[test]
    fn test_stopping_fades_out_and_says_so_at_once() {
        let sample = Arc::new(SampleData::mono(vec![0.8; 48000], RATE));
        let mut player = CuePlayer::default();
        player.play(cue(5, &sample, 0.0, 1.0), RATE);
        render(&mut player, 480);
        player.play(None, RATE);
        assert_eq!(player.report(), Some((5, None)));
        let out = render(&mut player, 480);
        assert!(out[0] > 0.39, "{}", out[0]);
        assert!(out[245..].iter().all(|&s| s == 0.0), "gone in 5 ms");
        assert!(!player.is_playing());
        assert_eq!(drain(&mut player), 1);
    }

    #[test]
    fn test_adds_to_the_patch_and_never_past_full_scale() {
        let sample = Arc::new(SampleData::mono(vec![1.0; 4800], RATE));
        let mut player = CuePlayer::default();
        player.play(cue(1, &sample, 0.0, 1.0), RATE);
        let mut output = vec![0.25; 200 * 3];
        output[300] = 0.9;
        player.mix(&mut output, 3, RATE);
        assert!((output[3 * 150] - 0.75).abs() < 1e-4, "{}", output[3 * 150]);
        assert!((output[3 * 150 + 2] - 0.75).abs() < 1e-4, "third channel gets both sides");
        assert_eq!(output[300], 1.0);
    }
}
