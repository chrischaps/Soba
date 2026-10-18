//! The recordings the patch's Samplers play, decoded once and kept by key.
//!
//! A node names its file by key (see [`crate::persistence::sample_files`]);
//! the shelf holds what that file decoded to, its waveform overview for the
//! node to draw, and a copy resampled to the engine's rate for the module
//! to play. Each is made the first time it's wanted: on the desktop, on a
//! thread of its own, since a long file takes a while to decode, reading
//! as [`LOADING`](sample_files::LOADING) until it's done (as a sample the
//! browser is still fetching does). A file that couldn't be read is
//! remembered as such, so a missing file is reported once rather than
//! looked for every frame. A sample still on its way isn't: in the browser
//! it's asked for again until it comes.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
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

/// What a file read on a thread comes to.
type Read = Result<ShelvedSample, String>;

/// Every file the patch's Samplers have asked for, by key.
#[derive(Debug)]
pub struct SampleShelf {
    entries: HashMap<String, Result<ShelvedSample, String>>,
    /// Files being read on threads of their own, each on the shelf as
    /// [`LOADING`](sample_files::LOADING) meanwhile.
    reading: HashMap<String, Receiver<Read>>,
    /// Whether files are read on threads at all. A capture, which steps
    /// its frames, and the tests read them where they're asked for, so
    /// each frame holds what it did; so does the browser, which has the
    /// one thread.
    aside: bool,
    /// The engine's rate, which a file read aside is made ready to play at.
    rate: Option<f32>,
}

/// Whether a new shelf reads files on threads: on the desktop, outside the tests.
const READ_ASIDE: bool = cfg!(not(any(test, target_arch = "wasm32")));

impl Default for SampleShelf {
    fn default() -> Self {
        Self { entries: HashMap::new(), reading: HashMap::new(), aside: READ_ASIDE, rate: None }
    }
}

impl SampleShelf {
    /// What's on the shelf for a key: the recording, or why it couldn't be
    /// read. `None` if it hasn't been asked for.
    pub fn get(&self, key: &str) -> Option<&Result<ShelvedSample, String>> {
        self.entries.get(key)
    }

    /// The recording a key names. The first time, its file is decoded: on
    /// a thread, reading as [`LOADING`](sample_files::LOADING) until it's
    /// done, unless files are read where they're asked for.
    pub fn load(&mut self, key: &str) -> Result<&mut ShelvedSample, String> {
        self.load_with(key, sample_files::load)
    }

    /// [`load`](Self::load), reading with `read`.
    fn load_with(&mut self, key: &str, read: impl FnOnce(&str) -> Result<Decoded, String> + Send + 'static) -> Result<&mut ShelvedSample, String> {
        self.collect(key);
        let on_its_way = self.entries.get(key).is_some_and(|entry| entry.as_ref().is_err_and(|e| sample_files::is_loading(e)));
        if on_its_way && !self.reading.contains_key(key) {
            // In the browser: ask again
            self.entries.remove(key);
        }
        if !self.entries.contains_key(key) {
            let entry = if self.aside { self.read_aside(key, read) } else { read(key).map(ShelvedSample::new) };
            self.entries.insert(key.to_string(), entry);
        }
        self.entries.get_mut(key).expect("just shelved").as_mut().map_err(|e| e.clone())
    }

    /// Starts reading a file on a thread of its own, made ready to play at
    /// the engine's rate. Returns what goes on the shelf meanwhile.
    fn read_aside(&mut self, key: &str, read: impl FnOnce(&str) -> Result<Decoded, String> + Send + 'static) -> Read {
        let (send, reading) = mpsc::channel();
        let path = key.to_string();
        let rate = self.rate;
        let spawned = std::thread::Builder::new().name("sample-file".into()).spawn(move || {
            let shelved = read(&path).map(|decoded| {
                let mut shelved = ShelvedSample::new(decoded);
                if let Some(rate) = rate {
                    shelved.at_rate(rate);
                }
                shelved
            });
            // Gone by now if the shelf let go of it
            let _ = send.send(shelved);
        });
        match spawned {
            Ok(_) => {
                self.reading.insert(key.to_string(), reading);
                Err(sample_files::LOADING.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Puts what the thread reading a key's file has read on the shelf, if it's come.
    fn collect(&mut self, key: &str) {
        let Some(reading) = self.reading.get(key) else { return };
        let read = match reading.try_recv() {
            Ok(read) => read,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("couldn't be read".to_string()),
        };
        self.reading.remove(key);
        self.entries.insert(key.to_string(), read);
    }

    /// Puts whatever the threads reading files have read on the shelf.
    /// Call once a frame, before looking at what's there.
    pub fn collect_all(&mut self) {
        let keys: Vec<String> = self.reading.keys().cloned().collect();
        for key in keys {
            self.collect(&key);
        }
    }

    /// Whether a key's file is still being read on a thread.
    pub fn is_reading(&self, key: &str) -> bool {
        self.reading.contains_key(key)
    }

    /// Reads files where they're asked for, rather than on threads, from
    /// here on, and waits for those being read: for a capture, whose
    /// frames must each hold what they did.
    pub fn read_in_place(&mut self) {
        self.aside = false;
        for (key, reading) in self.reading.drain() {
            let read = reading.recv().unwrap_or_else(|_| Err("couldn't be read".to_string()));
            self.entries.insert(key, read);
        }
    }

    /// Tells the shelf the engine's rate, so a file read aside is made
    /// ready to play at it there, rather than when first played.
    pub fn set_rate(&mut self, sample_rate: Option<f32>) {
        self.rate = sample_rate;
    }

    /// Whether a sample is still on its way (being read, or in the
    /// browser, being fetched).
    pub fn waiting(&self) -> bool {
        self.entries.values().any(|entry| entry.as_ref().is_err_and(|e| sample_files::is_loading(e)))
    }

    /// Puts a decoded recording on the shelf under `key`, replacing
    /// whatever was there: a dropped file, or one read again.
    pub fn insert(&mut self, key: &str, decoded: Decoded) {
        self.reading.remove(key);
        self.entries.insert(key.to_string(), Ok(ShelvedSample::new(decoded)));
    }

    /// Puts a recording already read on the shelf under `key`, replacing
    /// whatever was there: a file read while it was dragged over a Sampler.
    pub fn shelve(&mut self, key: &str, shelved: ShelvedSample) {
        self.reading.remove(key);
        self.entries.insert(key.to_string(), Ok(shelved));
    }

    /// Forgets a key, so the next [`load`](Self::load) reads the file again.
    pub fn forget(&mut self, key: &str) {
        self.reading.remove(key);
        self.entries.remove(key);
    }

    /// Files the same recording under a new key, as when Save As copies a
    /// sample beside the patch.
    pub fn rename(&mut self, from: &str, to: &str) {
        if let Some(entry) = self.entries.remove(from) {
            self.entries.insert(to.to_string(), entry);
        }
        if let Some(reading) = self.reading.remove(from) {
            self.reading.insert(to.to_string(), reading);
        }
    }

    /// The recording for a key at `sample_rate`, or `None` if it can't be
    /// read (or hasn't been yet).
    pub fn playing(&mut self, key: &str, sample_rate: f32) -> Option<Arc<SampleData>> {
        self.rate = Some(sample_rate);
        self.load(key).ok().map(|shelved| shelved.at_rate(sample_rate))
    }

    /// Lets go of every recording no node plays any more. Files read from
    /// disk can be read again (an undo may want one back); a file dropped
    /// into the browser can't, so it stays.
    pub fn retain(&mut self, in_use: impl Fn(&str) -> bool) {
        self.entries.retain(|key, _| key.starts_with(DROPPED_PREFIX) || in_use(key));
        self.reading.retain(|key, _| self.entries.contains_key(key));
    }

    /// Lets go of everything.
    pub fn clear(&mut self) {
        self.reading.clear();
        self.entries.clear();
    }
}

#[cfg(test)]
impl SampleShelf {
    /// A shelf that reads files on threads, as the desktop app's does.
    pub fn reading_aside() -> Self {
        Self { aside: true, ..Default::default() }
    }

    /// Collects until nothing is on its way.
    pub fn wait_for_reads(&mut self) {
        let begun = std::time::Instant::now();
        while self.waiting() {
            assert!(begun.elapsed().as_secs() < 10, "the read never came");
            std::thread::sleep(std::time::Duration::from_millis(1));
            self.collect_all();
        }
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
    fn a_file_read_aside_loads_once_it_comes_ready_to_play() {
        let mut shelf = SampleShelf::reading_aside();
        shelf.set_rate(Some(48000.0));
        let key = "C:/long.mp3";
        let (go, wait) = mpsc::channel::<()>();
        let read = move |_: &str| {
            wait.recv().unwrap();
            Ok(decoded(44100.0))
        };
        let problem = shelf.load_with(key, read).err().unwrap();
        assert!(sample_files::is_loading(&problem), "{problem}");
        assert!(shelf.waiting() && shelf.is_reading(key));
        assert!(shelf.playing(key, 48000.0).is_none(), "nothing to play while it's read");
        go.send(()).unwrap();
        shelf.wait_for_reads();
        assert!(!shelf.is_reading(key));
        // Resampled on its thread, to the engine's rate
        let shelved = shelf.load(key).unwrap();
        assert!(matches!(&shelved.playing, Some((rate, _)) if *rate == 48000.0));
    }

    #[test]
    fn a_file_read_aside_that_fails_says_why() {
        let mut shelf = SampleShelf::reading_aside();
        let key = "C:/broken.wav";
        assert!(shelf.load_with(key, |_| Err("not a WAV".to_string())).is_err());
        shelf.wait_for_reads();
        assert!(matches!(shelf.get(key), Some(Err(e)) if e == "not a WAV"));
    }

    #[test]
    fn a_file_forgotten_while_it_is_read_is_read_afresh() {
        let mut shelf = SampleShelf::reading_aside();
        let key = "C:/a.wav";
        let (_go, wait) = mpsc::channel::<()>();
        let stale = move |_: &str| {
            let _ = wait.recv();
            Ok(decoded(22050.0))
        };
        assert!(shelf.load_with(key, stale).is_err());
        shelf.forget(key);
        assert!(!shelf.waiting(), "what it was reading is let go");
        assert!(shelf.load_with(key, |_| Ok(decoded(44100.0))).is_err());
        shelf.wait_for_reads();
        assert_eq!(shelf.load(key).unwrap().source.sample_rate(), 44100.0);
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
