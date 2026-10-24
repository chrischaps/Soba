//! Handing the session to the next copy across a restart: the patch, with
//! any unsaved changes, the devices, the view and whether it was playing.
//!
//! The old copy writes it to a file and keeps the file locked until it has
//! exited and let go of the audio device (an ASIO driver serves one program
//! at a time). The new copy, started with `--resume <file>`, waits for the
//! lock before it opens anything.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::super::session::Autosave;

/// The command-line option a restarted copy is given, followed by the file.
pub const OPTION: &str = "--resume";

/// How long the new copy waits for the old one to close.
const PATIENCE: Duration = Duration::from_secs(15);

/// What the next copy picks up.
#[derive(Debug, Serialize, Deserialize)]
pub struct Resume {
    /// The patch as it stands, with its file or example.
    pub(crate) patch: Autosave,
    /// Whether it has changes its file doesn't.
    pub(crate) unsaved: bool,
    /// The view: zoom, and the patch point at the middle of the canvas.
    pub(crate) view: Option<(f32, [f32; 2])>,
    pub(crate) output_device: Option<String>,
    pub(crate) input_device: Option<String>,
    /// The first MIDI input open, for a copy from before #435 to read.
    pub(crate) midi_device: Option<String>,
    /// Every MIDI input open (#435).
    #[serde(default)]
    pub(crate) midi_devices: Vec<String>,
    pub(crate) playing: bool,
    /// The version that handed over, so the new one can say what changed.
    pub(crate) from_version: String,
}

impl Resume {
    /// Writes the handover and locks it until this process exits. Returns
    /// the file, to pass to the next copy after [`OPTION`].
    pub fn hand_over(&self) -> Result<PathBuf, String> {
        let path = std::env::temp_dir().join(format!("soba-resume-{}.json", std::process::id()));
        // Held, and so locked, until the process ends
        std::mem::forget(self.write_locked(&path)?);
        Ok(path)
    }

    /// Writes the handover to `path`, returning the file, locked.
    fn write_locked(&self, path: &Path) -> Result<std::fs::File, String> {
        let json = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let mut file = std::fs::File::create(path).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        file.lock().map_err(|e| format!("couldn't lock {}: {e}", path.display()))?;
        file.write_all(&json).and_then(|()| file.sync_all()).map_err(|e| e.to_string())?;
        Ok(file)
    }

    /// Waits for the copy that wrote `path` to close, then reads and
    /// removes it. `None` if it's missing or unreadable.
    pub fn take(path: &Path) -> Option<Self> {
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path).ok()?;
        let started = Instant::now();
        while file.try_lock().is_err() {
            if started.elapsed() > PATIENCE {
                eprintln!("resume: the previous copy is still running; going ahead");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut json = String::new();
        let read = file.read_to_string(&mut json);
        drop(file);
        let _ = std::fs::remove_file(path);
        read.ok()?;
        serde_json::from_str(&json).map_err(|e| eprintln!("resume: {e}")).ok()
    }

    /// The file named after [`OPTION`] on a command line.
    pub fn file_from_args(args: &[String]) -> Option<PathBuf> {
        let at = args.iter().position(|a| a == OPTION)?;
        args.get(at + 1).map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handover_round_trips_once() {
        let patch = crate::persistence::examples::first_sound().patch().unwrap();
        let resume = Resume {
            patch: Autosave::new(&patch, Some(PathBuf::from("song.json")), None).unwrap(),
            unsaved: true,
            view: Some((0.5, [120.0, -40.0])),
            output_device: Some("Speakers".into()),
            input_device: None,
            midi_device: Some("Keystep".into()),
            midi_devices: vec!["Keystep".into(), "Fantom-06".into()],
            playing: true,
            from_version: "0.3.1".into(),
        };
        let path = std::env::temp_dir().join(format!("soba-resume-test-{}.json", std::process::id()));
        let held = resume.write_locked(&path).unwrap();
        // The writer lets go a moment later, as the old copy does by exiting
        let started = Instant::now();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let args: Vec<String> = ["soba", OPTION, path.to_str().unwrap()].map(String::from).to_vec();
        assert_eq!(Resume::file_from_args(&args), Some(path.clone()));

        let back = Resume::take(&path).expect("the handover");
        assert!(started.elapsed() >= Duration::from_millis(300), "it waited for the lock");
        writer.join().unwrap();
        assert_eq!(back.view, Some((0.5, [120.0, -40.0])));
        assert_eq!(back.patch.path, Some(PathBuf::from("song.json")));
        assert_eq!(back.midi_device.as_deref(), Some("Keystep"));
        assert_eq!(back.midi_devices, ["Keystep", "Fantom-06"]);
        assert!(back.unsaved && back.playing);
        assert!(!path.exists());
        assert!(Resume::take(&path).is_none());
    }
}
