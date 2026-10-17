//! Recorded audio a module plays from: a sample loaded from a file.
//!
//! [`SampleData`] is built off the audio thread (decoded, cut to length and
//! resampled to the engine's rate there) and handed to the module that
//! plays it as an `Arc`, so the audio thread only ever swaps a pointer.
//! When it's replaced, the module hands the old one back to be dropped on
//! the UI side, so freeing a long file never happens in the audio callback.

use super::primitives::hermite;

/// The longest sample a module keeps, in seconds: five minutes, about
/// 115 MB of stereo at 48 kHz. A longer file is cut to this.
pub const MAX_SAMPLE_SECONDS: f32 = 300.0;

/// A mono or stereo recording at a known sample rate.
#[derive(Clone, PartialEq)]
pub struct SampleData {
    /// The left channel, or the only one.
    left: Vec<f32>,
    /// The right channel, or `None` for a mono recording, which plays on
    /// both sides.
    right: Option<Vec<f32>>,
    /// Frames per second the samples were recorded (or resampled) at.
    sample_rate: f32,
}

impl std::fmt::Debug for SampleData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the samples: there may be millions
        f.debug_struct("SampleData")
            .field("frames", &self.frames())
            .field("stereo", &self.is_stereo())
            .field("sample_rate", &self.sample_rate)
            .finish()
    }
}

impl SampleData {
    /// A mono recording. NaN and infinity become silence, and anything past
    /// [`SAMPLE_CEILING`](super::finite::SAMPLE_CEILING) is held to it, so a
    /// damaged file can't poison the patch.
    pub fn mono(mut samples: Vec<f32>, sample_rate: f32) -> Self {
        super::finite::sanitize_all(&mut samples);
        Self { left: samples, right: None, sample_rate: sample_rate.max(1.0) }
    }

    /// A stereo recording. The longer channel is cut to the shorter, and
    /// its samples made finite as in [`mono`](Self::mono).
    pub fn stereo(mut left: Vec<f32>, mut right: Vec<f32>, sample_rate: f32) -> Self {
        let frames = left.len().min(right.len());
        left.truncate(frames);
        right.truncate(frames);
        super::finite::sanitize_all(&mut left);
        super::finite::sanitize_all(&mut right);
        Self { left, right: Some(right), sample_rate: sample_rate.max(1.0) }
    }

    /// How many frames it holds.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Whether it holds no audio at all.
    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }

    /// Whether it has a right channel of its own.
    pub fn is_stereo(&self) -> bool {
        self.right.is_some()
    }

    /// The rate its frames are at, in Hz.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// How long it plays at its own rate, in seconds.
    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.sample_rate
    }

    /// The left channel, or the only one.
    pub fn left(&self) -> &[f32] {
        &self.left
    }

    /// The right channel; a mono recording's one channel.
    pub fn right(&self) -> &[f32] {
        self.right.as_deref().unwrap_or(&self.left)
    }

    /// Cuts it to at most `seconds` long. Returns true if anything was cut.
    pub fn truncate_seconds(&mut self, seconds: f32) -> bool {
        let keep = (seconds.max(0.0) * self.sample_rate) as usize;
        if keep >= self.frames() {
            return false;
        }
        self.left.truncate(keep);
        if let Some(right) = &mut self.right {
            right.truncate(keep);
        }
        true
    }

    /// The frame at `position` (in frames, fractional), read between
    /// samples on a Hermite curve, as (left, right). Outside the recording
    /// it holds the first or last sample, so a curve near either end is
    /// still smooth.
    ///
    /// REAL-TIME SAFE.
    #[inline]
    pub fn read(&self, position: f64) -> (f32, f32) {
        let frames = self.frames();
        if frames == 0 {
            return (0.0, 0.0);
        }
        let last = frames as i64 - 1;
        let base = position.floor();
        let t = (position - base) as f32;
        let i = base as i64;
        let at = |k: i64| (i + k).clamp(0, last) as usize;
        let (a, b, c, d) = (at(-1), at(0), at(1), at(2));
        let curve = |x: &[f32]| hermite(x[a], x[b], x[c], x[d], t);
        let left = curve(&self.left);
        let right = match &self.right {
            Some(right) => curve(right),
            None => left,
        };
        (left, right)
    }

    /// The same recording at another sample rate, read between samples on
    /// a Hermite curve, so a voice can play it back one frame per frame.
    pub fn resampled(&self, sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(1.0);
        if sample_rate == self.sample_rate || self.is_empty() {
            return Self { sample_rate, ..self.clone() };
        }
        let step = self.sample_rate as f64 / sample_rate as f64;
        let frames = ((self.frames() as f64) / step).floor().max(1.0) as usize;
        let mut left = Vec::with_capacity(frames);
        let mut right = self.right.as_ref().map(|_| Vec::with_capacity(frames));
        for n in 0..frames {
            let (l, r) = self.read(n as f64 * step);
            left.push(l);
            if let Some(right) = &mut right {
                right.push(r);
            }
        }
        Self { left, right, sample_rate }
    }

    /// An empty stereo recording with room set aside for `frames` more, to
    /// record a take into on the audio thread (see [`push`](Self::push)).
    /// Off the audio thread: this allocates.
    pub fn with_room(frames: usize, sample_rate: f32) -> Self {
        Self::stereo(Vec::with_capacity(frames), Vec::with_capacity(frames), sample_rate)
    }

    /// How many frames it has room for, recorded or not.
    pub fn room(&self) -> usize {
        self.left.capacity().min(self.right.as_ref().map_or(usize::MAX, Vec::capacity))
    }

    /// Empties it, keeping its room.
    ///
    /// REAL-TIME SAFE.
    pub fn clear(&mut self) {
        self.left.clear();
        if let Some(right) = &mut self.right {
            right.clear();
        }
    }

    /// Adds a frame at the end, if there's room. Returns whether it fitted.
    ///
    /// REAL-TIME SAFE: never grows past its room.
    #[inline]
    pub fn push(&mut self, [left, right]: [f32; 2]) -> bool {
        if self.frames() >= self.room() {
            return false;
        }
        self.left.push(left);
        if let Some(r) = &mut self.right {
            r.push(right);
        }
        true
    }

    /// Writes over frame `i`, if it has one.
    ///
    /// REAL-TIME SAFE.
    #[inline]
    pub fn set(&mut self, i: usize, [left, right]: [f32; 2]) {
        if let Some(l) = self.left.get_mut(i) {
            *l = left;
            if let Some(r) = self.right.as_mut().and_then(|r| r.get_mut(i)) {
                *r = right;
            }
        }
    }

    /// Turns its frames round, so frame `mid` comes first: a ring of
    /// pre-roll put in order.
    ///
    /// REAL-TIME SAFE.
    pub fn rotate_left(&mut self, mid: usize) {
        let frames = self.frames();
        if frames == 0 {
            return;
        }
        self.left.rotate_left(mid % frames);
        if let Some(right) = &mut self.right {
            right.rotate_left(mid % frames);
        }
    }

    /// A copy of what it holds and no more room than that: mono if both
    /// sides are the same throughout, which a mono source makes.
    pub fn to_measure(&self) -> Self {
        match &self.right {
            Some(right) if *right != self.left => Self::stereo(self.left.clone(), right.clone(), self.sample_rate),
            _ => Self::mono(self.left.clone(), self.sample_rate),
        }
    }

    /// Its loudest and quietest sample in each of `columns` equal slices,
    /// both channels together, for drawing it as a waveform.
    pub fn overview(&self, columns: usize) -> Vec<(f32, f32)> {
        let frames = self.frames();
        if frames == 0 || columns == 0 {
            return Vec::new();
        }
        (0..columns)
            .map(|column| {
                let start = column * frames / columns;
                let end = ((column + 1) * frames / columns).max(start + 1).min(frames);
                let mut low = f32::INFINITY;
                let mut high = f32::NEG_INFINITY;
                for channel in [self.left(), self.right()] {
                    for &sample in &channel[start..end] {
                        low = low.min(sample);
                        high = high.max(sample);
                    }
                }
                (low, high)
            })
            .collect()
    }
}

/// How a [`Snapshot`] came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// Not finished yet.
    Copying,
    /// Every frame is copied.
    Done,
    /// The module holds nothing to keep (a Looper with no loop).
    Empty,
    /// The recording is longer than the room set aside for it: this many frames.
    NoRoom(usize),
    /// The recording changed length while it was being copied.
    Changed,
    /// The module is still loading a recording, so what it holds isn't settled.
    Busy,
    /// The module keeps no recording, or it isn't in the patch any more.
    Unsupported,
}

/// A copy of a recording a module holds (a Looper's loop), made on the
/// audio thread a piece at a time into room the UI set aside beforehand,
/// so copying never allocates there and a long loop doesn't stall one
/// callback.
///
/// The module copies as many frames as it's allowed each callback (see
/// [`DspModule::fill_snapshot`](super::DspModule::fill_snapshot)), and the
/// snapshot goes back to the UI once [`is_finished`](Self::is_finished).
#[derive(Debug)]
pub struct Snapshot {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Frames the recording has, once copying has begun.
    frames: usize,
    sample_rate: f32,
    /// The module's count of changes to what it holds, as copying began.
    edits: u32,
    outcome: SnapshotOutcome,
}

impl Snapshot {
    /// Room for a stereo recording `frames` long. Off the audio thread:
    /// this allocates.
    pub fn with_capacity(frames: usize) -> Self {
        Self {
            left: Vec::with_capacity(frames),
            right: Vec::with_capacity(frames),
            frames: 0,
            sample_rate: 0.0,
            edits: 0,
            outcome: SnapshotOutcome::Copying,
        }
    }

    /// How many frames there's room for.
    pub fn capacity(&self) -> usize {
        self.left.capacity().min(self.right.capacity())
    }

    /// Whether copying has begun.
    pub fn has_begun(&self) -> bool {
        self.frames > 0
    }

    /// Starts copying a recording `frames` long at `sample_rate`, as of the
    /// module's `edits`th change. Returns false, and finishes as
    /// [`NoRoom`](SnapshotOutcome::NoRoom), if it doesn't fit.
    ///
    /// REAL-TIME SAFE.
    pub fn begin(&mut self, frames: usize, sample_rate: f32, edits: u32) -> bool {
        self.left.clear();
        self.right.clear();
        if frames > self.capacity() {
            self.outcome = SnapshotOutcome::NoRoom(frames);
            return false;
        }
        self.frames = frames;
        self.sample_rate = sample_rate;
        self.edits = edits;
        self.outcome = if frames == 0 { SnapshotOutcome::Empty } else { SnapshotOutcome::Copying };
        true
    }

    /// Frames the recording has, once copying has begun.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Frames copied so far.
    pub fn copied(&self) -> usize {
        self.left.len()
    }

    /// Adds the next frame. Past the end it's ignored; the last one
    /// finishes the snapshot.
    ///
    /// REAL-TIME SAFE: within the room set aside.
    #[inline]
    pub fn push(&mut self, [left, right]: [f32; 2]) {
        if self.left.len() < self.frames {
            self.left.push(left);
            self.right.push(right);
            if self.left.len() == self.frames {
                self.outcome = SnapshotOutcome::Done;
            }
        }
    }

    /// Finishes without a recording, saying why.
    ///
    /// REAL-TIME SAFE.
    pub fn finish(&mut self, outcome: SnapshotOutcome, edits: u32) {
        self.left.clear();
        self.right.clear();
        self.edits = edits;
        self.outcome = outcome;
    }

    /// Whether there's nothing more to copy.
    pub fn is_finished(&self) -> bool {
        self.outcome != SnapshotOutcome::Copying
    }

    /// How it came out.
    pub fn outcome(&self) -> SnapshotOutcome {
        self.outcome
    }

    /// The module's count of changes to what it holds, as copying began:
    /// a snapshot with the same count holds the same recording.
    pub fn edits(&self) -> u32 {
        self.edits
    }

    /// The recording, once [`Done`](SnapshotOutcome::Done): mono if both
    /// sides are the same throughout, which a mono source makes.
    pub fn into_sample(self) -> Option<SampleData> {
        if self.outcome != SnapshotOutcome::Done {
            return None;
        }
        Some(if self.left == self.right {
            SampleData::mono(self.left, self.sample_rate)
        } else {
            SampleData::stereo(self.left, self.right, self.sample_rate)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: f32, seconds: f32) -> Vec<f32> {
        (0..(rate * seconds) as usize).map(|n| (std::f32::consts::TAU * hz * n as f32 / rate).sin()).collect()
    }

    #[test]
    fn reads_whole_frames_exactly() {
        let sample = SampleData::stereo(vec![0.0, 1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0, 7.0], 48000.0);
        assert_eq!(sample.read(2.0), (2.0, 6.0));
        // Past either end it holds the end
        assert_eq!(sample.read(-3.0), (0.0, 4.0));
        assert_eq!(sample.read(10.0), (3.0, 7.0));
    }

    #[test]
    fn mono_plays_on_both_sides() {
        let sample = SampleData::mono(vec![0.25; 8], 44100.0);
        assert_eq!(sample.read(3.5), (0.25, 0.25));
        assert_eq!(sample.right(), sample.left());
    }

    #[test]
    fn resampling_keeps_length_and_pitch() {
        let source = SampleData::mono(sine(1000.0, 44100.0, 1.0), 44100.0);
        let resampled = source.resampled(48000.0);
        assert_eq!(resampled.sample_rate(), 48000.0);
        assert!((resampled.seconds() - 1.0).abs() < 0.001);
        // Count rising zero crossings: a 1 kHz tone still has a thousand a second
        let crossings = resampled.left().windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((999..=1001).contains(&crossings), "{crossings}");
        // Matches the sine it was cut from
        for n in (100..47000).step_by(997) {
            let expected = (std::f32::consts::TAU * 1000.0 * n as f32 / 48000.0).sin();
            assert!((resampled.left()[n] - expected).abs() < 0.01, "frame {n}");
        }
    }

    #[test]
    fn truncating_cuts_both_channels() {
        let mut sample = SampleData::stereo(vec![0.0; 1000], vec![0.0; 1000], 100.0);
        assert!(sample.truncate_seconds(2.5));
        assert_eq!(sample.frames(), 250);
        assert_eq!(sample.right().len(), 250);
        assert!(!sample.truncate_seconds(60.0));
    }

    #[test]
    fn overview_holds_each_slices_extremes() {
        let mut samples = vec![0.0; 100];
        samples[10] = 0.9;
        samples[60] = -0.5;
        let overview = SampleData::mono(samples, 1000.0).overview(4);
        assert_eq!(overview.len(), 4);
        assert_eq!(overview[0], (0.0, 0.9));
        assert_eq!(overview[2], (-0.5, 0.0));
        assert_eq!(overview[3], (0.0, 0.0));
    }

    #[test]
    fn a_take_records_within_its_room_and_comes_out_to_measure() {
        let mut take = SampleData::with_room(4, 48000.0);
        assert_eq!((take.frames(), take.room()), (0, 4));
        for n in 0..4 {
            assert!(take.push([n as f32, n as f32]));
        }
        assert!(!take.push([9.0, 9.0]), "full");
        assert_eq!(take.room(), 4, "never grows");
        // A ring of pre-roll, written round from frame 2, put in order
        take.set(0, [4.0, 4.0]);
        take.set(1, [5.0, 5.0]);
        take.rotate_left(2);
        assert_eq!(take.left(), &[2.0, 3.0, 4.0, 5.0]);
        // Both sides the same: mono, and only as long as it is
        let measured = take.to_measure();
        assert!(!measured.is_stereo());
        assert_eq!(measured.left().len(), 4);
        take.set(3, [5.0, -5.0]);
        assert!(take.to_measure().is_stereo());
        take.clear();
        assert_eq!((take.frames(), take.room()), (0, 4));
    }

    #[test]
    fn a_snapshot_fills_within_its_room() {
        let mut snapshot = Snapshot::with_capacity(4);
        assert!(!snapshot.is_finished());
        assert!(snapshot.begin(3, 48000.0, 7));
        snapshot.push([0.1, 0.2]);
        snapshot.push([0.3, 0.4]);
        assert!(!snapshot.is_finished());
        snapshot.push([0.5, 0.6]);
        snapshot.push([9.0, 9.0]);
        assert_eq!(snapshot.outcome(), SnapshotOutcome::Done);
        assert_eq!(snapshot.edits(), 7);
        let sample = snapshot.into_sample().unwrap();
        assert_eq!(sample.left(), &[0.1, 0.3, 0.5]);
        assert_eq!(sample.right(), &[0.2, 0.4, 0.6]);
        assert_eq!(sample.sample_rate(), 48000.0);
    }

    #[test]
    fn a_snapshot_says_when_it_has_no_room_and_keeps_mono_mono() {
        let mut small = Snapshot::with_capacity(2);
        assert!(!small.begin(3, 48000.0, 1));
        assert_eq!(small.outcome(), SnapshotOutcome::NoRoom(3));
        assert!(small.into_sample().is_none());

        let mut mono = Snapshot::with_capacity(2);
        mono.begin(2, 44100.0, 0);
        mono.push([0.5, 0.5]);
        mono.push([-0.5, -0.5]);
        assert!(!mono.into_sample().unwrap().is_stereo());
    }
}
