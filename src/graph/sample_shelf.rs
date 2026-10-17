//! The recordings the patch's Samplers play, decoded once and kept by key.
//!
//! A node names its file by key (see [`crate::persistence::sample_files`]);
//! the shelf holds what that file decoded to, its waveform overview for the
//! node to draw, and a copy resampled to the engine's rate for the module
//! to play. Each is made on the UI thread, the first time it's wanted.
//! A file that couldn't be read is remembered as such, so a missing file
//! is reported once rather than looked for every frame. A sample the
//! browser is still fetching isn't: it's asked for again until it comes.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::dsp::onsets::Onsets;
use crate::dsp::tidy::Survey;
use crate::dsp::SampleData;
use crate::persistence::sample_files::{self, Decoded};

/// How many slices a recording's overview has. The node draws a column
/// from however many of these fall under it.
pub const OVERVIEW_COLUMNS: usize = 1024;

/// Keys for files dropped into the browser, which exist only in memory.
pub use crate::persistence::sample_files::DROPPED_PREFIX;

/// One decoded file.
#[derive(Debug)]
pub struct ShelvedSample {
    /// As decoded, at the file's own rate.
    pub source: Arc<SampleData>,
    /// Its loudest and quietest sample per slice, for the node's waveform.
    pub overview: Vec<(f32, f32)>,
    /// Whether it was longer than a Sampler keeps, and was cut.
    pub truncated: bool,
    /// `source` at the engine's rate, made when first wanted.
    playing: Option<(f32, Arc<SampleData>)>,
    /// Where its sound is and how loud, for Trim and Normalize, measured
    /// when first wanted.
    survey: OnceLock<Survey>,
    /// Where its hits begin, for slicing by Onsets, found when first wanted.
    onsets: OnceLock<Onsets>,
}

impl ShelvedSample {
    pub fn new(decoded: Decoded) -> Self {
        Self {
            overview: decoded.sample.overview(OVERVIEW_COLUMNS),
            source: Arc::new(decoded.sample),
            truncated: decoded.truncated,
            playing: None,
            survey: OnceLock::new(),
            onsets: OnceLock::new(),
        }
    }

    /// Where its sound is and how loud, measured the first time it's asked.
    pub fn survey(&self) -> &Survey {
        self.survey.get_or_init(|| Survey::of(&self.source))
    }

    /// Where its hits begin, found the first time it's asked.
    pub fn onsets(&self) -> &Onsets {
        self.onsets.get_or_init(|| Onsets::detect(&self.source))
    }

    /// The recording at `sample_rate`, resampled and with its octaves made
    /// (so it can be pitched up without folding) the first time it's asked
    /// for at that rate.
    pub fn at_rate(&mut self, sample_rate: f32) -> Arc<SampleData> {
        match &self.playing {
            Some((rate, playing)) if *rate == sample_rate => Arc::clone(playing),
            _ => {
                let playing = Arc::new(self.source.playable_at(sample_rate));
                self.playing = Some((sample_rate, Arc::clone(&playing)));
                playing
            }
        }
    }
}

/// Every file the patch's Samplers have asked for, by key.
#[derive(Debug, Default)]
pub struct SampleShelf {
    entries: HashMap<String, Result<ShelvedSample, String>>,
}

impl SampleShelf {
    /// What's on the shelf for a key: the recording, or why it couldn't be
    /// read. `None` if it hasn't been asked for.
    pub fn get(&self, key: &str) -> Option<&Result<ShelvedSample, String>> {
        self.entries.get(key)
    }

    /// The recording a key names, decoding the file the first time.
    pub fn load(&mut self, key: &str) -> Result<&mut ShelvedSample, String> {
        self.load_with(key, sample_files::load)
    }

    /// [`load`](Self::load), reading with `read`.
    fn load_with(&mut self, key: &str, read: impl FnOnce(&str) -> Result<Decoded, String>) -> Result<&mut ShelvedSample, String> {
        if self.entries.get(key).is_some_and(|entry| entry.as_ref().is_err_and(|e| sample_files::is_loading(e))) {
            self.entries.remove(key);
        }
        let entry = self.entries.entry(key.to_string()).or_insert_with(|| read(key).map(ShelvedSample::new));
        entry.as_mut().map_err(|e| e.clone())
    }

    /// Whether a sample is still on its way (in the browser, being fetched).
    pub fn waiting(&self) -> bool {
        self.entries.values().any(|entry| entry.as_ref().is_err_and(|e| sample_files::is_loading(e)))
    }

    /// Puts a decoded recording on the shelf under `key`, replacing
    /// whatever was there: a dropped file, or one read again.
    pub fn insert(&mut self, key: &str, decoded: Decoded) {
        self.entries.insert(key.to_string(), Ok(ShelvedSample::new(decoded)));
    }

    /// Puts a recording already read on the shelf under `key`, replacing
    /// whatever was there: a file read while it was dragged over a Sampler.
    pub fn shelve(&mut self, key: &str, shelved: ShelvedSample) {
        self.entries.insert(key.to_string(), Ok(shelved));
    }

    /// Forgets a key, so the next [`load`](Self::load) reads the file again.
    pub fn forget(&mut self, key: &str) {
        self.entries.remove(key);
    }

    /// Files the same recording under a new key, as when Save As copies a
    /// sample beside the patch.
    pub fn rename(&mut self, from: &str, to: &str) {
        if let Some(entry) = self.entries.remove(from) {
            self.entries.insert(to.to_string(), entry);
        }
    }

    /// The recording for a key at `sample_rate`, or `None` if it can't be read.
    pub fn playing(&mut self, key: &str, sample_rate: f32) -> Option<Arc<SampleData>> {
        self.load(key).ok().map(|shelved| shelved.at_rate(sample_rate))
    }

    /// Lets go of every recording no node plays any more. Files read from
    /// disk can be read again (an undo may want one back); a file dropped
    /// into the browser can't, so it stays.
    pub fn retain(&mut self, in_use: impl Fn(&str) -> bool) {
        self.entries.retain(|key, _| key.starts_with(DROPPED_PREFIX) || in_use(key));
    }

    /// Lets go of everything.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(rate: f32) -> Decoded {
        Decoded { sample: SampleData::mono(vec![0.5; 4410], rate), truncated: false }
    }

    #[test]
    fn resamples_once_per_rate() {
        let mut shelf = SampleShelf::default();
        shelf.insert("dropped:a.wav", decoded(44100.0));
        let first = shelf.playing("dropped:a.wav", 48000.0).unwrap();
        assert_eq!(first.sample_rate(), 48000.0);
        assert!(Arc::ptr_eq(&first, &shelf.playing("dropped:a.wav", 48000.0).unwrap()));
        // A new device rate makes a new copy from the source
        let second = shelf.playing("dropped:a.wav", 44100.0).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second.frames(), 4410);
    }

    #[test]
    fn a_missing_file_is_remembered_as_missing() {
        let mut shelf = SampleShelf::default();
        let key = std::env::temp_dir().join("soba-no-such.wav").to_string_lossy().into_owned();
        assert!(shelf.playing(&key, 48000.0).is_none());
        assert!(matches!(shelf.get(&key), Some(Err(_))));
        shelf.forget(&key);
        assert!(shelf.get(&key).is_none());
    }

    #[test]
    fn a_sample_on_its_way_is_asked_for_again_until_it_comes() {
        let mut shelf = SampleShelf::default();
        let key = "example:samples/bell.wav";
        assert!(shelf.load_with(key, |_| Err(sample_files::LOADING.to_string())).is_err());
        assert!(shelf.waiting());
        assert!(shelf.load_with(key, |_| Ok(decoded(48000.0))).is_ok());
        assert!(!shelf.waiting());
        // Once here, it stays: no reading again
        assert!(shelf.load_with(key, |_| panic!("read again")).is_ok());
    }

    #[test]
    fn unused_files_go_but_dropped_ones_stay() {
        let mut shelf = SampleShelf::default();
        shelf.insert("dropped:a.wav", decoded(48000.0));
        shelf.insert("C:/b.wav", decoded(48000.0));
        shelf.insert("C:/c.wav", decoded(48000.0));
        shelf.retain(|key| key == "C:/c.wav");
        assert!(shelf.get("dropped:a.wav").is_some());
        assert!(shelf.get("C:/b.wav").is_none());
        assert!(shelf.get("C:/c.wav").is_some());
        shelf.rename("C:/c.wav", "D:/c.wav");
        assert!(shelf.get("D:/c.wav").is_some());
    }
}
