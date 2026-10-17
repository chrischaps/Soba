//! Loops in the editor: keeping each Looper's loop with the patch.
//!
//! A Looper's loop is part of the piece, so saving the patch saves it too,
//! as a WAV in `<patch name> loops` beside it, which the node's file then
//! names, as a Sampler's does. Opening the patch loads it back (see
//! [`SynthApp::sync_samples`]), and it waits, Stopped at the top.
//!
//! The loop lives on the audio thread, so saving asks for a copy of it:
//! the UI sets the room aside, and the Looper fills it a piece per
//! callback (see [`crate::dsp::Snapshot`]). Each Looper counts its changes
//! (its `READOUT_EDITS`), and a loop that hasn't changed since it was
//! saved or opened isn't copied again, so saving a patch with a long loop
//! a second time is instant.
//!
//! The browser has no folder beside the patch, so loops aren't kept there.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

use egui_node_graph2::NodeId;

use crate::engine::NodeId as EngineNodeId;
use crate::modules::looper::{self, Looper};
#[cfg(not(target_arch = "wasm32"))]
use crate::dsp::{SampleData, Snapshot, SnapshotOutcome};
#[cfg(not(target_arch = "wasm32"))]
use crate::persistence::sample_files::{self, Decoded};
use super::SynthApp;

/// Past this much, saving asks before writing loops beside the patch:
/// about 2 min 10 s of stereo at 48 kHz.
#[cfg(not(target_arch = "wasm32"))]
const ASK_BEFORE_SAVING_LOOP_BYTES: u64 = 50 * 1024 * 1024;

/// How long saving waits for the audio thread to copy the loops, past the
/// time copying them should take.
#[cfg(not(target_arch = "wasm32"))]
const SNAPSHOT_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

/// A Looper's loop as the patch last kept it.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct KeptLoop {
    /// The Looper's change count when it was saved.
    pub edits: u32,
    /// The file it was saved to, by sample key.
    pub key: Option<String>,
}

/// What saving the loops did, for saving the patch.
#[derive(Debug, Default)]
pub(super) struct LoopsSaved {
    /// Loopers whose loops weren't written because the question was
    /// declined: the patch is saved without them.
    pub left_out: Vec<EngineNodeId>,
    /// What went wrong, to show.
    pub problems: Vec<String>,
}

/// A Looper's loop, copied off the audio thread.
#[cfg(not(target_arch = "wasm32"))]
enum LoopCopy {
    /// The loop as heard, as of the Looper's `edits`th change.
    Loop { sample: SampleData, edits: u32 },
    /// It holds no loop.
    Empty { edits: u32 },
    /// It couldn't be copied, and why.
    Failed(String),
}

impl SynthApp {
    /// Every Looper in the patch: its node, engine node and file key.
    fn loopers(&self) -> Vec<(NodeId, EngineNodeId, Option<String>)> {
        self.graph_state
            .graph
            .nodes
            .iter()
            .filter(|(_, node)| node.user_data.module_id == looper::ID)
            .filter_map(|(node_id, node)| {
                let engine_id = self.user_state.get_engine_node_id(node_id)?;
                Some((node_id, engine_id, node.user_data.file.clone()))
            })
            .collect()
    }

    /// A Looper's change count as it last reported it: 0, as made or
    /// opened, until it reports.
    fn looper_edits(&self, engine_id: EngineNodeId) -> u32 {
        self.user_state.readouts.get(&engine_id).map_or(0, |r| r.values[Looper::READOUT_EDITS] as u32)
    }

    /// Whether a Looper holds something other than what the patch keeps
    /// for it: a loop recorded, overdubbed, undone or cleared since it was
    /// saved or opened, or pointed at another file since.
    fn loop_changed(&self, engine_id: EngineNodeId, key: Option<&str>) -> bool {
        match self.kept_loops.get(&engine_id) {
            Some(kept) => self.looper_edits(engine_id) != kept.edits || kept.key.as_deref() != key,
            // As opened (or made): just the file it names, or nothing
            None => self.looper_edits(engine_id) != 0,
        }
    }

    /// Whether any Looper's loop has changed since the patch was saved or
    /// opened. In the browser loops aren't kept, so none counts.
    pub(super) fn loops_unsaved(&self) -> bool {
        !super::WEB && self.loopers().iter().any(|(_, engine_id, key)| self.loop_changed(*engine_id, key.as_deref()))
    }

    /// Starts keeping track of a just-opened patch's loops: each Looper
    /// holds what its file does.
    pub(super) fn opened_loops(&mut self) {
        self.kept_loops.clear();
        self.loop_files = self.loopers().into_iter().filter_map(|(_, _, key)| key).collect();
    }

    /// Before the patch is saved to `path`, writes each Looper's loop that
    /// has changed (or isn't beside the patch yet, on a Save As) into
    /// `<patch name> loops` beside it, and points the Looper at the file.
    /// A Looper with no loop has no file, and a file this patch wrote, or
    /// opened, that no Looper names any more is removed. Asks first if the
    /// loops come to more than 50 MB.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_loops(&mut self, path: &Path) -> LoopsSaved {
        let mut saved = LoopsSaved::default();
        let folder = sample_files::loops_folder(path);
        let loopers = self.loopers();

        // A key two Loopers share (one pasted from the other) isn't either
        // one's to write over: whichever changed gets a file of its own
        let mut sharing: HashMap<String, usize> = HashMap::new();
        for key in loopers.iter().filter_map(|(_, _, key)| key.clone()) {
            *sharing.entry(key).or_default() += 1;
        }
        let beside = |key: &str| Path::new(key).parent() == Some(folder.as_path());
        let wanted: Vec<(NodeId, EngineNodeId, Option<String>)> = loopers
            .iter()
            .filter(|(_, engine_id, key)| {
                let kept = !self.loop_changed(*engine_id, key.as_deref());
                !(kept && key.as_deref().is_none_or(beside))
            })
            .cloned()
            .collect();

        if !wanted.is_empty() {
            match self.snapshot_loops(&wanted) {
                Ok(mut copies) => {
                    let bytes: u64 = copies
                        .values()
                        .map(|copy| match copy {
                            LoopCopy::Loop { sample, .. } => sample.frames() as u64 * if sample.is_stereo() { 8 } else { 4 },
                            _ => 0,
                        })
                        .sum();
                    if bytes > ASK_BEFORE_SAVING_LOOP_BYTES && !ask_to_save_loops(bytes, &folder) {
                        saved.left_out = wanted.iter().map(|(_, engine_id, _)| *engine_id).collect();
                        return saved;
                    }
                    for (node_id, engine_id, key) in &wanted {
                        match copies.remove(engine_id) {
                            Some(LoopCopy::Loop { sample, edits }) => {
                                let own = key.clone().filter(|key| beside(key) && sharing.get(key) == Some(&1));
                                match self.write_loop(*engine_id, sample, own, &folder, &loopers) {
                                    Ok(file) => self.keep_loop(*node_id, *engine_id, edits, Some(file)),
                                    Err(e) => saved.problems.push(format!("Couldn't save a Looper's loop: {e}")),
                                }
                            }
                            Some(LoopCopy::Empty { edits }) => self.keep_loop(*node_id, *engine_id, edits, None),
                            Some(LoopCopy::Failed(problem)) => saved.problems.push(problem),
                            None => {}
                        }
                    }
                }
                Err(problem) => saved.problems.push(problem),
            }
        }
        saved.problems.sort();
        saved.problems.dedup();

        self.remove_stale_loops(&folder);
        saved
    }

    /// Copies the wanted Loopers' loops off the audio thread, waiting until
    /// they're all back. Fails as a whole only with no engine to ask.
    #[cfg(not(target_arch = "wasm32"))]
    fn snapshot_loops(&mut self, wanted: &[(NodeId, EngineNodeId, Option<String>)]) -> Result<HashMap<EngineNodeId, LoopCopy>, String> {
        let no_engine = "Loops weren't saved: there's no audio engine to copy them from".to_string();
        if self.audio.engine.is_err() || self.ui_handle.is_none() {
            return Err(no_engine);
        }
        // A Looper added or pasted this frame must reach the engine first
        self.sync_samples();
        let rate = self.ui_handle.as_ref().map_or(48000.0, |h| h.graph().sample_rate()).max(1.0);
        let room: HashMap<EngineNodeId, usize> = wanted
            .iter()
            .map(|(_, engine_id, key)| (*engine_id, self.loop_room(*engine_id, key.as_deref(), rate)))
            .collect();
        let Some(handle) = self.ui_handle.as_mut() else { return Err(no_engine) };

        // Anything left from a save that gave up waiting is stale
        while handle.take_snapshot().is_some() {}
        let mut copies = HashMap::new();
        for (&engine_id, &frames) in &room {
            if !handle.request_snapshot(engine_id, Box::new(Snapshot::with_capacity(frames))) {
                copies.insert(engine_id, LoopCopy::Failed("A Looper's loop couldn't be saved: too many copies are on their way".to_string()));
            }
        }

        // Copying runs at about four minutes of loop a second
        let total: usize = room.values().sum();
        let mut deadline = std::time::Instant::now() + SNAPSHOT_PATIENCE + std::time::Duration::from_secs_f64(total as f64 / rate as f64 / 100.0);
        while copies.len() < room.len() && std::time::Instant::now() < deadline {
            handle.flush();
            while let Some((engine_id, snapshot)) = handle.take_snapshot() {
                if !room.contains_key(&engine_id) {
                    continue;
                }
                let edits = snapshot.edits();
                let copy = match snapshot.outcome() {
                    SnapshotOutcome::Done => match snapshot.into_sample() {
                        Some(sample) => LoopCopy::Loop { sample, edits },
                        None => LoopCopy::Empty { edits },
                    },
                    SnapshotOutcome::Empty => LoopCopy::Empty { edits },
                    SnapshotOutcome::NoRoom(frames) => {
                        // It grew since it last reported: ask again, with room
                        handle.request_snapshot(engine_id, Box::new(Snapshot::with_capacity(frames)));
                        deadline += std::time::Duration::from_millis(500);
                        continue;
                    }
                    SnapshotOutcome::Busy => LoopCopy::Failed("A Looper was still loading its loop, so its file was left as it was".to_string()),
                    SnapshotOutcome::Changed => LoopCopy::Failed("A Looper's loop changed as it was being saved: save again".to_string()),
                    SnapshotOutcome::Copying | SnapshotOutcome::Unsupported => {
                        LoopCopy::Failed("A Looper's loop couldn't be copied, so its file was left as it was".to_string())
                    }
                };
                copies.insert(engine_id, copy);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for engine_id in room.keys() {
            copies
                .entry(*engine_id)
                .or_insert_with(|| LoopCopy::Failed("A Looper's loop wasn't saved: the audio engine didn't answer in time".to_string()));
        }
        Ok(copies)
    }

    /// Room for a Looper's loop, in frames: its length as last reported,
    /// or else its file's, with a little to spare. Too little is asked
    /// for again.
    #[cfg(not(target_arch = "wasm32"))]
    fn loop_room(&mut self, engine_id: EngineNodeId, key: Option<&str>, rate: f32) -> usize {
        let reported = self.user_state.readouts.get(&engine_id).map(|r| r.values[Looper::READOUT_SECONDS]);
        let seconds = match reported {
            Some(seconds) => seconds,
            None => key.and_then(|key| self.user_state.samples.load(key).ok()).map_or(0.0, |shelved| shelved.source.seconds()),
        };
        (seconds as f64 * rate as f64).ceil() as usize + 64
    }

    /// Writes a loop into `folder`, over `own` if it has a file there of
    /// its own, else under a new name. Returns the file's key.
    #[cfg(not(target_arch = "wasm32"))]
    fn write_loop(
        &mut self,
        engine_id: EngineNodeId,
        sample: SampleData,
        own: Option<String>,
        folder: &Path,
        loopers: &[(NodeId, EngineNodeId, Option<String>)],
    ) -> Result<String, String> {
        std::fs::create_dir_all(folder).map_err(|e| format!("couldn't make {}: {e}", folder.display()))?;
        let path = match own {
            Some(own) => PathBuf::from(own),
            None => self.free_loop_name(engine_id, folder, loopers),
        };
        // Written whole beside it first, so a failed write leaves the old one
        let partial = path.with_extension("wav.partial");
        std::fs::write(&partial, sample_files::encode_wav(&sample)).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;

        let key = path.to_string_lossy().into_owned();
        self.user_state.samples.insert(&key, Decoded { sample, truncated: false });
        Ok(key)
    }

    /// A name in `folder` for a Looper's new loop file, `Looper <id>.wav`,
    /// or with " 2", " 3" if another Looper or someone else's file has it.
    #[cfg(not(target_arch = "wasm32"))]
    fn free_loop_name(&self, engine_id: EngineNodeId, folder: &Path, loopers: &[(NodeId, EngineNodeId, Option<String>)]) -> PathBuf {
        let named: HashSet<PathBuf> = loopers.iter().filter_map(|(_, _, key)| key.as_ref().map(PathBuf::from)).collect();
        (1..)
            .map(|n| match n {
                1 => folder.join(format!("Looper {engine_id}.wav")),
                n => folder.join(format!("Looper {engine_id} {n}.wav")),
            })
            .find(|path| !named.contains(path) && (!path.exists() || self.loop_files.contains(&*path.to_string_lossy())))
            .expect("some name is free")
    }

    /// Points a Looper at the file its loop was just saved to (or at none),
    /// as the patch now keeps it. That's not an edit to undo, nor a file
    /// to load into the Looper, which holds it already.
    #[cfg(not(target_arch = "wasm32"))]
    fn keep_loop(&mut self, node_id: NodeId, engine_id: EngineNodeId, edits: u32, key: Option<String>) {
        if let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) {
            node.user_data.file = key.clone();
        }
        self.history.absorb_file(engine_id, key.clone());
        let rate = self.ui_handle.as_ref().map(|handle| handle.graph().sample_rate());
        let held = key.as_deref().zip(rate).and_then(|(key, rate)| self.user_state.samples.playing(key, rate));
        if let Some(handle) = self.ui_handle.as_mut() {
            handle.note_sample(engine_id, held);
        }
        if let Some(key) = &key {
            self.loop_files.insert(key.clone());
        }
        self.kept_loops.insert(engine_id, KeptLoop { edits, key });
    }

    /// Removes the loop files in `folder` this patch wrote, or opened, that
    /// no Looper names any more, and the folder if that empties it.
    #[cfg(not(target_arch = "wasm32"))]
    fn remove_stale_loops(&mut self, folder: &Path) {
        let named: HashSet<String> = self.loopers().into_iter().filter_map(|(_, _, key)| key).collect();
        let stale: Vec<String> = self
            .loop_files
            .iter()
            .filter(|key| !named.contains(*key) && Path::new(key.as_str()).parent() == Some(folder))
            .cloned()
            .collect();
        for key in stale {
            if std::fs::remove_file(&key).is_ok() || !Path::new(&key).exists() {
                self.loop_files.remove(&key);
                self.user_state.samples.forget(&key);
            }
        }
        // Only goes if that left it empty
        let _ = std::fs::remove_dir(folder);
    }
}

/// Asks whether to write loops coming to `bytes` beside the patch. If not,
/// the patch is saved without them.
#[cfg(not(target_arch = "wasm32"))]
fn ask_to_save_loops(bytes: u64, folder: &Path) -> bool {
    let answer = rfd::MessageDialog::new()
        .set_title("Save the loops with the patch?")
        .set_description(format!(
            "The patch's loops come to {:.0} MB.\n\nSave them as WAV files in \"{}\" beside the patch? If not, the patch is saved without them, and its Loopers will open empty.",
            bytes as f64 / (1024.0 * 1024.0),
            folder.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    answer == rfd::MessageDialogResult::Yes
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::engine::EngineCommand;
    use crate::modules::looper::LoopState;
    use crate::persistence::{load_from_file, save_to_file, ConnectionData, NodeData, Patch};
    use std::time::{Duration, Instant};

    /// Runs the app's per-frame engine work, without a window, for `seconds`.
    fn pump(app: &mut SynthApp, seconds: f32) {
        let end = Instant::now() + Duration::from_secs_f32(seconds);
        while Instant::now() < end {
            app.sync_parameters();
            app.sync_samples();
            if let Some(handle) = app.ui_handle.as_mut() {
                handle.flush();
            }
            app.process_engine_events();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Presses one of the Looper's footswitches (0 Rec, 1 Stop, 2 Undo, 3 Clear).
    fn press(app: &mut SynthApp, engine_id: EngineNodeId, pedal: usize) {
        let param_index = Looper::PARAM_PEDALS + pedal;
        app.send_command(EngineCommand::SetParameter { node_id: engine_id, param_index, value: 1.0 });
        pump(app, 0.1);
        app.send_command(EngineCommand::SetParameter { node_id: engine_id, param_index, value: 0.0 });
        pump(app, 0.1);
    }

    fn the_looper(app: &SynthApp) -> (NodeId, EngineNodeId, Option<String>) {
        app.loopers().into_iter().next().expect("a Looper")
    }

    fn state(app: &SynthApp, engine_id: EngineNodeId) -> LoopState {
        LoopState::from_code(app.user_state.readouts.get(&engine_id).map_or(0.0, |r| r.values[Looper::READOUT_STATE]))
    }

    /// The loop the Looper holds now, copied off the audio thread.
    fn held(app: &mut SynthApp) -> Option<SampleData> {
        let looper = the_looper(app);
        match app.snapshot_loops(std::slice::from_ref(&looper)).unwrap().remove(&looper.1) {
            Some(LoopCopy::Loop { sample, .. }) => Some(sample),
            _ => None,
        }
    }

    fn modified(path: &str) -> std::time::SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    /// The whole round, on the real audio device: record, save, save
    /// again untouched, overdub and undo, reopen, clear. The patch has no
    /// output module, so it makes no sound.
    #[test]
    #[ignore = "needs an audio device; run with --ignored"]
    fn loops_are_kept_with_the_patch() {
        let mut app = SynthApp::new(false);
        if app.audio.engine.is_err() {
            eprintln!("no audio device: skipped");
            return;
        }
        let folder = std::env::temp_dir().join(format!("soba-loops-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("Song.json");

        // A sine into a Looper, opened as a patch file
        let mut patch = Patch::new("Song");
        patch.nodes.push(NodeData::new(1, "osc.sine", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, looper::ID, (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 2, "In L"));
        save_to_file(&patch, &path).unwrap();
        app.open_file(&path);
        app.is_playing = true;
        app.user_state.is_playing = true;
        app.send_command(EngineCommand::SetPlaying(true));
        pump(&mut app, 0.3);
        let (_, engine_id, _) = the_looper(&app);
        assert!(!app.has_unsaved_changes());

        // Record about half a second: the patch has unsaved changes
        press(&mut app, engine_id, 0);
        pump(&mut app, 0.4);
        press(&mut app, engine_id, 0);
        pump(&mut app, 0.2);
        assert_eq!(state(&app, engine_id), LoopState::Playing);
        assert!(app.has_unsaved_changes(), "a loop recorded is a change");

        // Saved beside the patch, bit for bit, and the patch names it
        assert!(app.quick_save());
        assert!(app.load_warnings.is_empty(), "{:?}", app.load_warnings);
        let (_, _, key) = the_looper(&app);
        let key = key.expect("the Looper names its file");
        assert_eq!(Path::new(&key).parent(), Some(folder.join("Song loops").as_path()));
        let first = sample_files::read_file(Path::new(&key)).unwrap().sample;
        assert_eq!(Some(&first), held(&mut app).as_ref(), "the file is the loop");
        assert!(!app.has_unsaved_changes());
        let saved = load_from_file(&path).unwrap();
        let file = saved.nodes.iter().find(|n| n.module_id == looper::ID).and_then(|n| n.file.clone());
        assert_eq!(file.as_deref(), Some(&*format!("Song loops/{}", sample_files::file_name(&key))));

        // Saving again untouched writes nothing
        let written = modified(&key);
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        assert!(app.quick_save());
        assert!(started.elapsed() < Duration::from_millis(100), "no copying");
        assert_eq!(modified(&key), written);

        // An overdub, an octave up, is saved with the loop; undone, it isn't
        let sine = app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.module_id == "osc.sine").map(|(id, _)| id).unwrap();
        let sine = app.user_state.get_engine_node_id(sine).unwrap();
        press(&mut app, engine_id, 0);
        app.send_command(EngineCommand::SetParameter { node_id: sine, param_index: 0, value: 880.0 });
        pump(&mut app, 0.8);
        press(&mut app, engine_id, 0);
        pump(&mut app, 0.1);
        assert!(app.has_unsaved_changes());
        assert!(app.quick_save());
        let layered = sample_files::read_file(Path::new(&key)).unwrap().sample;
        assert_ne!(layered, first);
        press(&mut app, engine_id, 2);
        assert!(app.has_unsaved_changes(), "an undo is a change");
        assert!(app.quick_save());
        assert_eq!(sample_files::read_file(Path::new(&key)).unwrap().sample, first, "the undone layer is left out");

        // Opened again, it waits, Stopped, holding the file, with nothing unsaved
        app.open_file(&path);
        pump(&mut app, 0.3);
        let (_, engine_id, reopened) = the_looper(&app);
        assert_eq!(reopened.as_deref(), Some(key.as_str()));
        assert_eq!(state(&app, engine_id), LoopState::Stopped);
        assert_eq!(held(&mut app).as_ref(), Some(&first));
        assert!(!app.has_unsaved_changes());
        press(&mut app, engine_id, 0);
        assert_eq!(state(&app, engine_id), LoopState::Playing, "Rec plays it");

        // Cleared and saved: the file, its name in the patch and the folder go
        press(&mut app, engine_id, 3);
        pump(&mut app, 0.1);
        assert!(app.has_unsaved_changes());
        assert!(app.quick_save());
        assert_eq!(the_looper(&app).2, None);
        assert!(!Path::new(&key).exists());
        assert!(!folder.join("Song loops").exists());
        let saved = load_from_file(&path).unwrap();
        assert!(saved.nodes.iter().all(|n| n.file.is_none()));

        // A loop file gone missing is a warning, and the Looper opens empty
        let mut patch = Patch::new("Song");
        let mut looper = NodeData::new(2, looper::ID, (0.0, 0.0));
        looper.file = Some("Song loops/gone.wav".to_string());
        patch.nodes.push(looper);
        save_to_file(&patch, &path).unwrap();
        app.open_file(&path);
        pump(&mut app, 0.2);
        assert!(app.load_warnings.iter().any(|w| w.contains("Couldn't load loop Song loops/gone.wav")), "{:?}", app.load_warnings);
        assert_eq!(state(&app, the_looper(&app).1), LoopState::Empty);

        app.send_command(EngineCommand::SetPlaying(false));
        pump(&mut app, 0.05);
        std::fs::remove_dir_all(&folder).ok();
    }
}
