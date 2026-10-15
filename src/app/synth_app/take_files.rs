//! Takes nobody keeps: tidying away the files a session wrote that no patch
//! names.
//!
//! A take is written the moment it ends, and a Trim or Normalize the moment
//! it's chosen (see `takes` and `tidy`), which is what makes undo and a
//! crash safe. But a file that's undone, replaced, or written into a patch
//! that's then let go would stay on disk for good. So the session notes
//! each file it writes, and which of them each patch it saves names, and
//! removes the rest once nothing could want them:
//!
//! - On save, those that neither the patch nor its undo history names, nor
//!   any other patch saved this session.
//! - When the patch is let go (New, Open, Quit), those that no patch saved
//!   this session names, now its history is gone. A folder that leaves
//!   empty goes too, the session's own in temp among them.
//!
//! A file the session didn't write is left alone. So is everything when
//! Soba restarts into another version, whose autosave still names them.
//!
//! An untitled patch's takes go in a folder of the session's own in temp,
//! which a crash leaves behind. Once the recovery prompt is answered, the
//! folders of sessions that are no longer running are taken over: the
//! takes the patch now open names (the recovered one, or one handed over
//! across a restart) are adopted, as if this session had written them, so
//! Save As gathers them and letting the patch go tidies them, and the rest
//! are removed. A folder whose session still runs is left alone.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::SynthApp;

/// The files a session wrote, and which of them the patches it saved name.
#[derive(Debug, Default)]
pub(in crate::app) struct TakeFiles {
    /// Every file the session wrote, by sample key, until it's removed.
    written: HashSet<String>,
    /// The written files each patch saved this session names, by its path.
    saved: HashMap<PathBuf, HashSet<String>>,
}

/// What a session's folder in temp is called, before its process id.
const TEMP_FOLDER: &str = "Soba takes ";

/// The folder in temp for this session's untitled takes.
pub(super) fn temp_folder() -> PathBuf {
    std::env::temp_dir().join(format!("{TEMP_FOLDER}{}", std::process::id()))
}

/// The folders in `temp` that sessions no longer `running` left behind.
fn left_behind(temp: &Path, running: impl Fn(u32) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(temp) else { return Vec::new() };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name();
            let pid = name.to_str().and_then(|name| name.strip_prefix(TEMP_FOLDER)).and_then(|pid| pid.parse::<u32>().ok());
            pid.is_some_and(|pid| pid != std::process::id() && !running(pid))
        })
        .map(|entry| entry.path())
        .collect();
    folders.sort();
    folders
}

/// Whether the process `pid` is still running. When it can't tell, it
/// says so: a folder is only taken over from a session that's gone.
#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is checked before use and closed after
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            // Denied is someone else's (or an elevated copy's): still there
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let still = GetExitCodeProcess(process, &mut code) == 0 || code == STILL_ACTIVE as u32;
        CloseHandle(process);
        still
    }
}

#[cfg(not(windows))]
fn running(pid: u32) -> bool {
    let proc = Path::new("/proc");
    !proc.is_dir() || proc.join(pid.to_string()).exists()
}

impl TakeFiles {
    /// Notes a file the session wrote.
    pub fn wrote(&mut self, key: &str) {
        self.written.insert(key.to_string());
    }

    /// Notes a copy of `from` made at `to` (a Save As gathering it beside
    /// the patch): if the session wrote the one, it wrote the other.
    pub fn copied(&mut self, from: &str, to: &str) {
        if self.written.contains(from) {
            self.wrote(to);
        }
    }

    /// Notes that the patch saved at `path` names the files `named`.
    fn saved<'a>(&mut self, path: &Path, named: impl IntoIterator<Item = &'a str>) {
        let kept = named.into_iter().filter(|key| self.written.contains(*key)).map(str::to_string).collect();
        self.saved.insert(path.to_path_buf(), kept);
    }

    /// The written files that no saved patch names, nor `wanted`.
    fn unkept(&self, wanted: impl Fn(&str) -> bool) -> Vec<String> {
        let mut unkept: Vec<String> = self
            .written
            .iter()
            .filter(|key| !wanted(key) && !self.saved.values().any(|named| named.contains(*key)))
            .cloned()
            .collect();
        unkept.sort();
        unkept
    }

    /// Takes over the folders in `temp` left by sessions no longer
    /// `running`: adopts each file in one that `named` gives a key for,
    /// and removes the rest, with the folder once it's empty. Returns the
    /// keys of those removed.
    fn take_over(&mut self, temp: &Path, running: impl Fn(u32) -> bool, named: impl Fn(&Path) -> Option<String>) -> Vec<String> {
        let mut unnamed = Vec::new();
        for folder in left_behind(temp, running) {
            let files = std::fs::read_dir(&folder).into_iter().flatten().filter_map(Result::ok);
            let files: Vec<PathBuf> = files.filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file())).map(|entry| entry.path()).collect();
            for file in files {
                match named(&file) {
                    Some(key) => self.wrote(&key),
                    None => unnamed.push(file.to_string_lossy().into_owned()),
                }
            }
            // One with nothing left in it at all goes too
            let _ = std::fs::remove_dir(&folder);
        }
        self.remove(unnamed)
    }

    /// Removes `keys` from disk, and each folder that leaves empty. Returns
    /// those that are gone.
    fn remove(&mut self, keys: Vec<String>) -> Vec<String> {
        let mut gone = Vec::new();
        let mut folders = HashSet::new();
        for key in keys {
            let path = Path::new(&key);
            if std::fs::remove_file(path).is_ok() || !path.exists() {
                if let Some(folder) = path.parent() {
                    folders.insert(folder.to_path_buf());
                }
                self.written.remove(&key);
                gone.push(key);
            }
        }
        for folder in folders {
            // Only goes if that left it empty
            let _ = std::fs::remove_dir(folder);
        }
        gone
    }
}

impl SynthApp {
    /// Once the patch is saved to `path`: removes the files the session
    /// wrote that neither it nor its undo history names, nor another patch
    /// saved this session.
    pub(super) fn tidy_takes_on_save(&mut self, path: &Path) {
        let named = self.files_named();
        self.take_files.saved(path, named.iter().map(String::as_str));
        let history = self.history.files();
        let unkept = self.take_files.unkept(|key| named.contains(key) || history.contains(key));
        self.forget_takes(unkept);
    }

    /// As the patch is let go, and its undo history with it: removes the
    /// files the session wrote that no patch it saved names.
    pub(super) fn tidy_takes_on_leave(&mut self) {
        let unkept = self.take_files.unkept(|_| false);
        self.forget_takes(unkept);
    }

    /// Takes over the folders in temp that sessions no longer running left
    /// behind: adopts the takes the patch names, and removes the rest.
    /// Once the recovery prompt is answered, either way.
    pub(super) fn take_over_left_takes(&mut self) {
        let named = self.files_named();
        let removed = self.take_files.take_over(&std::env::temp_dir(), running, |file| {
            named.iter().find(|key| Path::new(key.as_str()) == file).cloned()
        });
        for key in removed {
            self.user_state.samples.forget(&key);
        }
    }

    /// Every file a module in the patch plays, by sample key.
    fn files_named(&self) -> HashSet<String> {
        self.graph_state.graph.nodes.iter().filter_map(|(_, node)| node.user_data.file.clone()).collect()
    }

    /// Removes files the session wrote, and takes them off the shelf.
    fn forget_takes(&mut self, keys: Vec<String>) {
        if keys.is_empty() {
            return;
        }
        for key in self.take_files.remove(keys) {
            self.user_state.samples.forget(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("soba-take-files-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn write(folder: &Path, name: &str) -> String {
        let path = folder.join(name);
        std::fs::write(&path, b"RIFF").unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn only_written_files_no_saved_patch_names_are_unkept() {
        let mut files = TakeFiles::default();
        files.wrote("C:/Song samples/Take 1.wav");
        files.wrote("C:/Song samples/Take 2.wav");
        files.wrote("C:/Song samples/Take 3.wav");
        // Named by a saved patch, beside a file the session never wrote
        files.saved(Path::new("C:/Song.json"), ["C:/Song samples/Take 1.wav", "C:/kit/kick.wav"]);
        assert_eq!(files.unkept(|_| false), ["C:/Song samples/Take 2.wav", "C:/Song samples/Take 3.wav"]);
        // Still wanted (by the patch, or by undo)
        assert_eq!(files.unkept(|key| key.ends_with("2.wav")), ["C:/Song samples/Take 3.wav"]);
        // Saved again, it names another: the first is free to go
        files.saved(Path::new("C:/Song.json"), ["C:/Song samples/Take 3.wav"]);
        assert_eq!(files.unkept(|_| false), ["C:/Song samples/Take 1.wav", "C:/Song samples/Take 2.wav"]);
        // Unless a patch saved elsewhere names it
        files.saved(Path::new("C:/Song 2.json"), ["C:/Song samples/Take 1.wav"]);
        assert_eq!(files.unkept(|_| false), ["C:/Song samples/Take 2.wav"]);
    }

    #[test]
    fn a_copy_of_a_written_file_is_written_too() {
        let mut files = TakeFiles::default();
        files.wrote("C:/Temp/Soba takes 1/Take 1.wav");
        files.copied("C:/Temp/Soba takes 1/Take 1.wav", "C:/Song samples/Take 1.wav");
        files.copied("C:/kit/kick.wav", "C:/Song samples/kick.wav");
        assert_eq!(files.unkept(|_| false), ["C:/Song samples/Take 1.wav", "C:/Temp/Soba takes 1/Take 1.wav"]);
    }

    #[test]
    fn removing_takes_the_folder_only_once_it_is_empty() {
        let emptied = folder("emptied");
        let shared = folder("shared");
        let mut files = TakeFiles::default();
        let take = write(&emptied, "Take 1.wav");
        let other = write(&shared, "Take 1.wav");
        let theirs = write(&shared, "kick.wav");
        files.wrote(&take);
        files.wrote(&other);
        // One already gone counts as removed
        let missing = shared.join("Take 2.wav").to_string_lossy().into_owned();
        files.wrote(&missing);

        let gone = files.remove(vec![take.clone(), other.clone(), missing.clone()]);
        assert_eq!(gone, [take, other, missing]);
        assert!(!emptied.exists(), "emptied, so it goes");
        assert!(Path::new(&theirs).exists(), "a file the session didn't write stays");
        assert!(files.unkept(|_| false).is_empty());
        std::fs::remove_dir_all(&shared).ok();
    }

    #[test]
    fn a_gone_session_s_takes_are_adopted_or_removed() {
        let temp = folder("left-behind");
        let gone = temp.join("Soba takes 7");
        let running = temp.join("Soba takes 8");
        let emptied = temp.join("Soba takes 9");
        let own = temp.join(format!("Soba takes {}", std::process::id()));
        let other = temp.join("Soba takes elsewhere");
        for folder in [&gone, &running, &emptied, &own, &other] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let named = write(&gone, "Take 1.wav");
        let undone = write(&gone, "Take 2.wav");
        let half = write(&gone, "Take 3.wav.partial");
        let live = write(&running, "Take 1.wav");
        let dropped = write(&emptied, "Take 1.wav");
        let ours = write(&own, "Take 1.wav");
        let unrelated = write(&other, "Take 1.wav");

        let mut files = TakeFiles::default();
        let removed = files.take_over(&temp, |pid| pid == 8, |file| (file == Path::new(&named)).then(|| named.clone()));
        assert_eq!(removed, [undone.clone(), half.clone(), dropped.clone()]);
        assert!(Path::new(&named).exists(), "the patch names it, so it's adopted");
        assert!(!Path::new(&undone).exists() && !Path::new(&half).exists());
        assert!(!emptied.exists(), "emptied, so it goes");
        assert!(Path::new(&live).exists(), "its session still runs");
        assert!(Path::new(&ours).exists(), "this session's own is tidied as usual");
        assert!(Path::new(&unrelated).exists(), "not a session's folder");
        // Adopted: unsaved, it's tidied when the patch is let go, with its folder
        assert_eq!(files.unkept(|_| false), [named.clone()]);
        files.remove(files.unkept(|_| false));
        assert!(!gone.exists());
        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn a_process_runs_until_it_exits() {
        assert!(running(std::process::id()));
        // The test binary itself, listing its tests and gone again
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        child.wait().unwrap();
        assert!(!running(child.id()));
    }

    /// A tone a second long between two seconds of a quiet room, to trim.
    fn padded() -> crate::dsp::SampleData {
        let rate = 48000.0;
        let samples = (0..(3.0 * rate) as usize)
            .map(|i| {
                let t = i as f32 / rate;
                let room = 0.0005 * (i as f32 * 12.9898).sin();
                room + if (1.0..2.0).contains(&t) { 0.05 * (std::f32::consts::TAU * 220.0 * t).sin() } else { 0.0 }
            })
            .collect();
        crate::dsp::SampleData::mono(samples, rate)
    }

    /// The whole round on a real app, with files written by Trim and
    /// Normalize: a save keeps what undo could bring back, letting the
    /// patch go keeps only what a saved patch names, and an untitled
    /// patch's folder in temp goes with its files. Opens the audio device,
    /// but the patch has no output, so it's silent.
    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn takes_nobody_keeps_are_removed() {
        use crate::graph::Tidy;
        use crate::dsp::tidy::Level;
        use crate::modules::sampler::SAMPLER_ID;
        use crate::persistence::{sample_files, save_to_file, NodeData, Patch};

        let mut app = SynthApp::new(false);
        let folder = folder("round");
        let original = folder.join("padded.wav");
        std::fs::write(&original, sample_files::encode_wav(&padded())).unwrap();
        let path = folder.join("Song.json");
        let mut patch = Patch::new("Song");
        let mut sampler = NodeData::new(1, SAMPLER_ID, (0.0, 0.0));
        sampler.file = Some("padded.wav".to_string());
        patch.nodes.push(sampler);
        save_to_file(&patch, &path).unwrap();
        app.open_file(&path);
        app.sync_samples();
        let node_id = |app: &SynthApp| app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.module_id == SAMPLER_ID).unwrap().0;
        let file = |app: &SynthApp| app.graph_state.graph.nodes[node_id(app)].user_data.file.clone().unwrap();
        let tidy = |app: &mut SynthApp, tidy: Tidy| {
            app.tidy_sample(node_id(app), tidy);
            app.sync_history();
            app.sync_samples();
            file(app)
        };
        let exists = |key: &str| Path::new(key).exists();

        // Trimmed, then normalized, and that undone: both still on the history
        let trimmed = tidy(&mut app, Tidy::Trim);
        let normalized = tidy(&mut app, Tidy::Normalize(Level::Rms));
        app.undo();
        assert!(app.quick_save());
        assert!(exists(&trimmed) && exists(&normalized), "undo or redo could bring either back");

        // Normalized another way: the first is past redo, so a save removes it
        let peak = tidy(&mut app, Tidy::Normalize(Level::Peak));
        assert_ne!(peak, normalized);
        assert!(app.quick_save());
        assert!(!exists(&normalized), "nothing can bring it back");
        assert!(exists(&trimmed) && exists(&peak));

        // Undone and let go: the saved patch names the peak file, so it stays
        app.undo();
        app.sync_history();
        app.new_patch();
        assert!(!exists(&trimmed), "only the history named it");
        assert!(exists(&peak), "the saved patch names it");
        assert!(original.exists(), "the session didn't write it");

        // Untitled, a file is written into the session's folder in temp,
        // and that goes with it when the patch is let go
        app.open_file(&path);
        app.current_patch_path = None;
        let at = node_id(&app);
        app.graph_state.graph.nodes[at].user_data.file = Some(original.to_string_lossy().into_owned());
        app.sync_history();
        app.sync_samples();
        let untitled = tidy(&mut app, Tidy::Trim);
        let temp = Path::new(&untitled).parent().unwrap().to_path_buf();
        assert_eq!(temp, std::env::temp_dir().join(format!("Soba takes {}", std::process::id())));
        app.new_patch();
        assert!(!exists(&untitled));
        assert!(!temp.exists(), "emptied, so it goes");
        assert!(exists(&peak));

        std::fs::remove_dir_all(&folder).ok();
    }

    /// A crash left an untitled patch's take in its session's folder in
    /// temp: recovering adopts it, so letting the patch go tidies it and
    /// the folder. Opens the audio device, but the patch has no output.
    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn a_recovered_patch_s_takes_are_tidied_when_it_is_let_go() {
        use crate::app::session::Autosave;
        use crate::modules::sampler::SAMPLER_ID;
        use crate::persistence::{sample_files, NodeData, Patch};

        // A process id no session has: past what Windows or Linux hand out
        let crashed = std::env::temp_dir().join("Soba takes 4294967291");
        std::fs::create_dir_all(&crashed).unwrap();
        let take = crashed.join("Take 1.wav");
        std::fs::write(&take, sample_files::encode_wav(&padded())).unwrap();
        let undone = write(&crashed, "Take 2.wav");
        let mut patch = Patch::new("Untitled");
        let mut sampler = NodeData::new(1, SAMPLER_ID, (0.0, 0.0));
        sampler.file = Some(take.to_string_lossy().into_owned());
        patch.nodes.push(sampler);

        let mut app = SynthApp::new(false);
        app.recover(&Autosave::new(&patch, None, None).unwrap());
        app.take_over_left_takes();
        assert!(take.exists(), "the recovered patch plays it");
        assert!(!Path::new(&undone).exists(), "nothing names it");
        app.new_patch();
        assert!(!take.exists(), "adopted, and never saved");
        assert!(!crashed.exists(), "emptied, so it goes");
    }
}