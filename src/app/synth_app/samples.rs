//! Samples in the editor: the files Samplers play, and Loopers' loops.
//!
//! A Sampler node names its file by key (see
//! [`crate::persistence::sample_files`]), and that's all the editor edits:
//! opening a file, dropping one on the node, undo, paste and opening a
//! patch all just set the key. Once a frame, [`SynthApp::sync_samples`]
//! makes the engine match: each Sampler gets the recording its key names,
//! decoded and resampled to the engine's rate (on a thread of its own, see
//! [`crate::graph::sample_shelf`]), whenever that isn't what it already
//! has. A device that changes rate gets every recording again, resampled
//! from the file's own. Whatever there is to say about a file (loaded, cut
//! short, couldn't be read) is said once it's been read.
//!
//! A Looper's file is its loop as last saved (see [`super::loops`]). The
//! loop it holds is the player's, so it's only given the file when it's
//! new to the patch (opened, pasted, brought back by undo), or when a
//! change of rate has emptied it.
//!
//! Patch files keep paths relative to themselves where they can, so Save and
//! Save As copy samples from elsewhere into a folder beside the patch.

use std::collections::{HashMap, HashSet};
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use egui_node_graph2::NodeId;

use crate::engine::EngineCommand;
use crate::graph::sample_shelf::DROPPED_PREFIX;
use crate::graph::SynthNodeTemplate;
use crate::modules::looper;
use crate::modules::plays_a_sample;
use crate::modules::sampler;
use crate::persistence::sample_files::{self, SampleBase};
use crate::persistence::{Patch, StagedNode};
use super::super::editing;
use super::SynthApp;

/// Past this much, saving asks before copying samples beside the patch.
#[cfg(not(target_arch = "wasm32"))]
const ASK_BEFORE_COPYING_BYTES: u64 = 50 * 1024 * 1024;

/// What [`SynthApp::gather_samples`] does when the samples to copy come to
/// more than [`ASK_BEFORE_COPYING_BYTES`].
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum WhenLarge {
    /// Ask whether to copy them: Save As, which is asking things already.
    Ask,
    /// Leave them where they are and say so: Save, which would otherwise
    /// ask again on every Ctrl+S once the answer was no.
    Warn,
}

/// What to say about a sample file once it's been read.
#[derive(Clone, Debug)]
enum Awaiting {
    /// Chosen for a Sampler (opened or dropped): say it loaded, or why
    /// not, and play it once from the face of the node, if any, still naming it.
    Chosen { cue: Option<NodeId> },
    /// Named by a patch just opened: warn if it couldn't be read or was cut.
    Opened { saved: String, looper: bool },
}

/// The sample files still being read that there's something to say about, by key.
#[derive(Default)]
pub(super) struct Awaited(HashMap<String, Awaiting>);

impl SynthApp {
    /// Asks for a sound file and loads it into a Sampler.
    pub(super) fn open_sample_dialog(&mut self, node_id: NodeId) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut dialog = rfd::FileDialog::new().add_filter("Audio (WAV, MP3, FLAC, Ogg)", sample_files::EXTENSIONS);
            // Start beside the file it has, or else beside the patch
            let current = self.graph_state.graph.nodes.get(node_id).and_then(|n| n.user_data.file.clone());
            let folder = current
                .filter(|key| !key.starts_with(sample_files::EXAMPLE_PREFIX) && !key.starts_with(DROPPED_PREFIX))
                .and_then(|key| Path::new(&key).parent().map(Path::to_path_buf))
                .or_else(|| self.document.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)));
            if let Some(folder) = folder.filter(|f| f.is_dir()) {
                dialog = dialog.set_directory(folder);
            }
            if let Some(path) = dialog.pick_file() {
                self.load_sample_file(node_id, &path);
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = node_id;
            self.status_message = Some("Drag a WAV, MP3, FLAC or Ogg file from your computer onto the Sampler".to_string());
        }
    }

    /// Loads a file from disk into a Sampler, reading it afresh in case it
    /// changed since it was last loaded.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_sample_file(&mut self, node_id: NodeId, path: &Path) {
        let key = path.to_string_lossy().into_owned();
        self.user_state.samples.forget(&key);
        if self.check_sample(&key) {
            self.set_sample(node_id, key);
        }
    }

    /// Reads the file a key names onto the shelf, saying in the status bar
    /// if it couldn't be read or was cut short (once it has been, if it's
    /// read on a thread). Returns whether it can play, as far as is known.
    fn check_sample(&mut self, key: &str) -> bool {
        let read = self.user_state.samples.load(key).map(|shelved| shelved.truncated);
        match read {
            Err(e) if sample_files::is_loading(&e) => {
                self.awaited_samples.0.insert(key.to_string(), Awaiting::Chosen { cue: None });
                true
            }
            read => self.say_chosen_sample(key, read),
        }
    }

    /// Says in the status bar that a file chosen for a Sampler loaded (and
    /// whether it was cut short), or why it didn't. Returns whether it did.
    fn say_chosen_sample(&mut self, key: &str, read: Result<bool, String>) -> bool {
        let name = sample_files::file_name(key);
        match read {
            Ok(truncated) => {
                self.status_message = Some(if truncated {
                    format!("Loaded {name}: cut to the 5 minutes a Sampler keeps")
                } else {
                    format!("Loaded {name}")
                });
                true
            }
            Err(e) => {
                self.status_error(format!("Couldn't load {name}: {e}"));
                false
            }
        }
    }

    /// Points a Sampler at a file, as one undo step, and plays it once
    /// from its face, so you hear what you chose (or lets it play on, if it
    /// was heard while dragged over). A file still being read plays once it's come.
    fn set_sample(&mut self, node_id: NodeId, key: String) {
        let name = sample_files::file_name(&key).to_string();
        self.forget_slice_markers(node_id);
        if let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) {
            node.user_data.file = Some(key.clone());
            self.history.name_next(format!("Load {name}"));
            if let Some(Awaiting::Chosen { cue }) = self.awaited_samples.0.get_mut(&key) {
                *cue = Some(node_id);
            } else if !self.keep_dropped_cue(node_id, &key) {
                self.cue_start_to_end(node_id);
            }
        }
    }

    /// Says what there is to say about each awaited file that's been read
    /// since the last frame. Forgets those no longer on the shelf.
    fn report_read_samples(&mut self) {
        let shelf = &self.user_state.samples;
        let read: Vec<(String, Awaiting, Result<bool, String>)> = self
            .awaited_samples
            .0
            .iter()
            .filter_map(|(key, awaiting)| match shelf.get(key)? {
                Err(e) if sample_files::is_loading(e) => None,
                read => Some((key.clone(), awaiting.clone(), read.as_ref().map(|shelved| shelved.truncated).map_err(Clone::clone))),
            })
            .collect();
        self.awaited_samples.0.retain(|key, _| shelf.get(key).is_some_and(|entry| entry.as_ref().is_err_and(|e| sample_files::is_loading(e))));
        for (key, awaiting, read) in read {
            match awaiting {
                Awaiting::Chosen { cue } => {
                    let loaded = self.say_chosen_sample(&key, read);
                    let still_named = |app: &Self, node_id| app.graph_state.graph.nodes.get(node_id).is_some_and(|n| n.user_data.file.as_deref() == Some(key.as_str()));
                    if let Some(node_id) = cue.filter(|&node_id| loaded && still_named(self, node_id)) {
                        self.cue_start_to_end(node_id);
                    }
                }
                Awaiting::Opened { saved, looper } => {
                    if let Some(warning) = opening_warning(&saved, looper, read) {
                        eprintln!("Patch load warning: {warning}");
                        self.load_warnings.push(warning);
                    }
                }
            }
        }
    }

    /// Gives every Sampler (and Chopper) the recording its file names, at the engine's
    /// rate, wherever that isn't what it has, and every new Looper the loop
    /// its file holds; and lets go of recordings no node plays any more.
    /// Call once a frame, after the frame's edits.
    pub(super) fn sync_samples(&mut self) {
        let rate = self.engine_rate();
        self.user_state.samples.set_rate(rate);
        self.user_state.samples.collect_all();
        self.report_read_samples();

        let samplers: Vec<(NodeId, Option<String>)> = self
            .graph_state
            .graph
            .nodes
            .iter()
            .filter(|(_, node)| plays_a_sample(node.user_data.module_id))
            .map(|(node_id, node)| (node_id, node.user_data.file.clone()))
            .collect();
        let loopers: Vec<(NodeId, String)> = self
            .graph_state
            .graph
            .nodes
            .iter()
            .filter(|(_, node)| node.user_data.module_id == looper::ID)
            .filter_map(|(node_id, node)| Some((node_id, node.user_data.file.clone()?)))
            .collect();

        let mut loads = Vec::new();
        if let Some(handle) = self.ui_handle.as_ref() {
            let graph = handle.graph();
            let rate = graph.sample_rate();
            for (node_id, key) in &loopers {
                let Some(engine_id) = self.user_state.get_engine_node_id(*node_id) else { continue };
                let wanted = match graph.sample(engine_id) {
                    // Not yet on the audio thread: it's given the loop at once
                    None => graph.is_fresh(engine_id),
                    // A change of rate emptied it: the loop as last saved comes back
                    Some(has) => has.sample_rate() != rate,
                };
                if let Some(sample) = wanted.then(|| self.user_state.samples.playing(key, rate)).flatten() {
                    loads.push(EngineCommand::LoadSample { node_id: engine_id, sample: Some(sample) });
                }
            }
            for (node_id, key) in &samplers {
                let Some(engine_id) = self.user_state.get_engine_node_id(*node_id) else { continue };
                if !graph.contains_module(engine_id) {
                    continue;
                }
                let wanted = key.as_deref().and_then(|key| self.user_state.samples.playing(key, rate));
                // Read again (opened afresh): it plays what it has until the new one comes
                if wanted.is_none() && key.as_deref().is_some_and(|key| self.user_state.samples.is_reading(key)) {
                    continue;
                }
                let same = match (&wanted, graph.sample(engine_id)) {
                    (Some(wanted), Some(has)) => Arc::ptr_eq(wanted, has),
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    loads.push(EngineCommand::LoadSample { node_id: engine_id, sample: wanted });
                }
            }
        } else {
            // No engine to play them, but the nodes still draw them
            for key in samplers.iter().filter_map(|(_, key)| key.as_deref()) {
                let _ = self.user_state.samples.load(key);
            }
        }
        for command in loads {
            self.send_command(command);
        }

        let in_use: HashSet<String> =
            samplers.into_iter().filter_map(|(_, key)| key).chain(loopers.into_iter().map(|(_, key)| key)).collect();
        self.user_state.samples.retain(|key| in_use.contains(key));
        // ...and where those sliced by Onsets are cut
        self.sync_markers();
    }

    /// Turns the file paths of a just-opened patch's Samplers into keys,
    /// resolving relative ones from `base`, and reads each file, returning
    /// a warning for each that can't be read or was cut short. A Sampler
    /// whose file is missing loads empty, still naming it.
    pub(super) fn resolve_samples(&mut self, nodes: &[StagedNode], base: SampleBase) -> Vec<String> {
        let mut warnings = Vec::new();
        let mut checked = HashSet::new();
        // Made ready to play as they're read
        self.user_state.samples.set_rate(self.engine_rate());
        for node in nodes {
            let Some(data) = self.graph_state.graph.nodes.get_mut(node.graph_id).map(|n| &mut n.user_data) else { continue };
            let Some(saved) = data.file.clone() else { continue };
            let looper = data.module_id == looper::ID;
            let key = sample_files::resolve(&saved, base);
            data.file = Some(key.clone());
            if !checked.insert(key.clone()) {
                continue;
            }
            match self.user_state.samples.load(&key).map(|shelved| shelved.truncated) {
                // Read in a moment, and the Sampler says so meanwhile
                Err(e) if sample_files::is_loading(&e) => {
                    self.awaited_samples.0.insert(key, Awaiting::Opened { saved, looper });
                }
                read => warnings.extend(opening_warning(&saved, looper, read)),
            }
        }
        warnings
    }

    /// Saves the Loopers' loops beside `path` (see [`SynthApp::save_loops`])
    /// and returns the patch to save there, with whatever went wrong. The
    /// browser keeps no loops.
    pub(super) fn patch_with_loops(&mut self, name: &str, path: &Path) -> (Patch, Vec<String>) {
        #[cfg(not(target_arch = "wasm32"))]
        let loops = self.save_loops(path);
        #[cfg(target_arch = "wasm32")]
        let loops = super::loops::LoopsSaved::default();
        let mut patch = self.patch_for_file(name, path);
        if !loops.left_out.is_empty() {
            // Declined: saved without them
            fn leave_out(nodes: &mut [crate::persistence::NodeData], groups: &mut [crate::persistence::GroupData], ids: &[u64]) {
                for node in nodes.iter_mut().filter(|node| ids.contains(&node.id)) {
                    node.file = None;
                }
                for group in groups {
                    leave_out(&mut group.nodes, &mut group.groups, ids);
                }
            }
            leave_out(&mut patch.nodes, &mut patch.groups, &loops.left_out);
        }
        (patch, loops.problems)
    }

    /// The patch as it will be saved at `path`: sample paths beside or
    /// below the patch made relative to it.
    pub(super) fn patch_for_file(&self, name: &str, path: &Path) -> Patch {
        let mut patch = self.create_patch(name);
        let folder = path.parent().unwrap_or(Path::new(""));
        fn relocate(nodes: &mut [crate::persistence::NodeData], groups: &mut [crate::persistence::GroupData], folder: &Path) {
            for node in nodes {
                if let Some(key) = &node.file {
                    node.file = Some(sample_files::to_patch_path(key, folder));
                }
            }
            for group in groups {
                relocate(&mut group.nodes, &mut group.groups, folder);
            }
        }
        relocate(&mut patch.nodes, &mut patch.groups, folder);
        patch
    }

    /// Before a save to `path`, copies the samples from outside the
    /// patch's folder (and any that ship with an example) into
    /// `<patch name> samples` beside it, and points their Samplers at the
    /// copies, so the folder can be moved or shared whole. A sample dropped
    /// in after the first save is gathered by the next Save. If they come to
    /// more than 50 MB, `large` says whether to ask or just warn. Returns a
    /// problem to report, if any.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn gather_samples(&mut self, path: &Path, large: WhenLarge) -> Option<String> {
        let folder = path.parent().unwrap_or(Path::new(""));
        let mut outside: Vec<String> = self
            .graph_state
            .graph
            .nodes
            .iter()
            .filter_map(|(_, node)| node.user_data.file.clone())
            .filter(|key| !key.starts_with(DROPPED_PREFIX) && sample_files::is_outside(key, folder))
            .collect();
        outside.sort();
        outside.dedup();
        if outside.is_empty() {
            return None;
        }

        let size = |key: &str| match sample_files::example_bytes(key) {
            Some(bytes) => bytes.len() as u64,
            None => std::fs::metadata(key).map(|m| m.len()).unwrap_or(0),
        };
        let total: u64 = outside.iter().map(|key| size(key)).sum();
        let destination = sample_files::samples_folder(path);
        if total > ASK_BEFORE_COPYING_BYTES && large == WhenLarge::Warn {
            return Some(format!(
                "{} sample{} from other folders, {:.0} MB in all, {} outside the patch's folder. Save As offers to copy them beside it",
                outside.len(),
                if outside.len() == 1 { "" } else { "s" },
                total as f64 / (1024.0 * 1024.0),
                if outside.len() == 1 { "stays" } else { "stay" },
            ));
        }
        if total > ASK_BEFORE_COPYING_BYTES {
            let answer = rfd::MessageDialog::new()
                .set_title("Copy samples beside the patch?")
                .set_description(format!(
                    "The patch plays {} sample{} from other folders, {:.0} MB in all.\n\nCopy them into \"{}\" beside the patch, so the folder holds everything it needs? If not, the patch keeps pointing at where they are.",
                    outside.len(),
                    if outside.len() == 1 { "" } else { "s" },
                    total as f64 / (1024.0 * 1024.0),
                    destination.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                ))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if answer != rfd::MessageDialogResult::Yes {
                return None;
            }
        }

        if let Err(e) = std::fs::create_dir_all(&destination) {
            return Some(format!("Couldn't make {}: {e}", destination.display()));
        }
        let mut moved = Vec::new();
        let mut problems = Vec::new();
        for key in &outside {
            match copy_sample(key, &destination) {
                Ok(copy) => moved.push((key.clone(), copy.to_string_lossy().into_owned())),
                Err(e) => problems.push(format!("{}: {e}", sample_files::file_name(key))),
            }
        }
        let mut changed = false;
        for (from, to) in &moved {
            self.user_state.samples.rename(from, to);
            self.take_files.copied(from, to);
            for (_, node) in self.graph_state.graph.nodes.iter_mut() {
                if node.user_data.file.as_deref() == Some(from.as_str()) {
                    node.user_data.file = Some(to.clone());
                    changed = true;
                }
            }
        }
        if changed {
            self.history.name_next("Copy samples beside the patch");
        }
        (!problems.is_empty()).then(|| format!("Couldn't copy {}", problems.join(", ")))
    }

    /// Follows files dragged over the window, and takes them when dropped:
    /// a WAV dropped on a Sampler replaces its file, and one dropped
    /// anywhere else brings a new Sampler there to play it.
    pub(super) fn handle_file_drops(&mut self, ctx: &egui::Context) {
        let (hovering, dropped) = ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
        if !hovering && dropped.is_empty() {
            self.user_state.file_drop_target = None;
            return;
        }
        let pointer = drop_pointer(ctx, self.capture.is_some());
        self.user_state.file_drop_target = hovering.then(|| pointer.and_then(|at| self.sampler_at(ctx, at)));
        if hovering {
            ctx.request_repaint();
        }

        let mut placed = 0;
        for file in dropped {
            let name = file
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.name.clone());
            if !sample_files::is_sound_file(&name) {
                self.status_error(format!("{name} isn't a sound file: a Sampler plays WAV, MP3, FLAC and Ogg files"));
                continue;
            }
            // Read it first: a file that won't play doesn't make a Sampler
            // (unless it's still being read, and says so once it has been)
            let key = match (&file.path, &file.bytes) {
                (Some(path), _) => {
                    let key = path.to_string_lossy().into_owned();
                    match self.take_hovered_file(&key) {
                        // Read a moment ago, while it was dragged over
                        Some(shelved) => self.user_state.samples.shelve(&key, shelved),
                        None => self.user_state.samples.forget(&key),
                    }
                    key
                }
                (None, Some(bytes)) => {
                    let key = format!("{DROPPED_PREFIX}{name}");
                    match sample_files::decode(std::io::Cursor::new(bytes.to_vec()), &name) {
                        Ok(decoded) => {
                            self.keep_in_tab(&key, || bytes.to_vec());
                            self.user_state.samples.insert(&key, decoded);
                        }
                        Err(e) => {
                            self.status_error(format!("Couldn't load {name}: {e}"));
                            continue;
                        }
                    }
                    key
                }
                (None, None) => continue,
            };
            if !self.check_sample(&key) {
                continue;
            }

            let at = pointer.unwrap_or_else(|| self.editor_rect.center()) + egui::vec2(24.0, 24.0) * placed as f32;
            let node_id = match self.sampler_at(ctx, at) {
                Some(node_id) => node_id,
                None => {
                    placed += 1;
                    self.add_sampler_at(at)
                }
            };
            self.set_sample(node_id, key);
            self.graph_state.selected_nodes = vec![node_id];
        }
    }

    /// A new Sampler with its waveform about under `screen`. Returns its node.
    fn add_sampler_at(&mut self, screen: egui::Pos2) -> NodeId {
        let template = SynthNodeTemplate::from_module_id(sampler::ID).expect("the Sampler is registered");
        let zoom = self.graph_state.pan_zoom.zoom;
        let position = self.screen_to_node(screen - egui::vec2(110.0, 60.0) * zoom);
        let (node_id, commands) = editing::add_module(&mut self.graph_state, &mut self.user_state, template, position);
        for command in commands {
            self.send_command(command);
        }
        node_id
    }

    /// The Sampler (or Chopper) on show under a point on screen, if there is one.
    fn sampler_at(&self, ctx: &egui::Context, screen: egui::Pos2) -> Option<NodeId> {
        self.graph_state.node_order.iter().rev().copied().find(|&node_id| {
            !self.user_state.hidden.contains(&node_id)
                && self.graph_state.graph.nodes.get(node_id).is_some_and(|n| plays_a_sample(n.user_data.module_id))
                && ctx.read_response(egui::Id::new((node_id, "window"))).is_some_and(|r| r.rect.contains(screen))
        })
    }
}

/// The warning for a file a patch just opened names, `saved` as the patch
/// names it, by what reading it came to (whether it was cut short, or why
/// it couldn't be read), if there's anything to warn of.
fn opening_warning(saved: &str, looper: bool, read: Result<bool, String>) -> Option<String> {
    match read {
        Ok(true) if !looper => Some(format!("Sample {saved} is longer than the 5 minutes a Sampler keeps, and was cut")),
        Ok(_) => None,
        Err(e) if looper => Some(format!("Couldn't load loop {saved}: {e}. The Looper opens empty")),
        Err(e) => Some(format!("Couldn't load sample {saved}: {e}")),
    }
}

/// Copies a sample into `folder`, keeping its name, or adding " 2", " 3"
/// if a different file has it already. A file already there with the same
/// bytes is used as it is. Returns where the copy is.
#[cfg(not(target_arch = "wasm32"))]
fn copy_sample(key: &str, folder: &Path) -> std::io::Result<PathBuf> {
    let bytes = match sample_files::example_bytes(key) {
        Some(bytes) => bytes.to_vec(),
        None => std::fs::read(key)?,
    };
    let name = Path::new(sample_files::file_name(key));
    let stem = name.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = name.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "wav".into());
    for n in 1.. {
        let candidate = if n == 1 { folder.join(name) } else { folder.join(format!("{stem} {n}.{extension}")) };
        match std::fs::read(&candidate) {
            Ok(existing) if existing == bytes => return Ok(candidate),
            Ok(_) => continue,
            Err(_) => {
                std::fs::write(&candidate, &bytes)?;
                return Ok(candidate);
            }
        }
    }
    unreachable!("some name is free")
}

/// Where the pointer is while a file is dragged over the window, in egui
/// points. Windows sends the window no mouse moves during a drag, so egui's
/// pointer is stale; ask the system where the cursor is instead, unless a
/// capture script is dragging it.
pub(super) fn drop_pointer(ctx: &egui::Context, scripted: bool) -> Option<egui::Pos2> {
    #[cfg(windows)]
    if !scripted {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: GetCursorPos only writes the POINT it's given
        let found = unsafe { GetCursorPos(&mut point) } != 0;
        let inner = ctx.input(|i| i.viewport().inner_rect);
        if let (true, Some(inner)) = (found, inner) {
            let ppp = ctx.pixels_per_point();
            return Some(egui::pos2(point.x as f32 / ppp, point.y as f32 / ppp) - inner.min.to_vec2());
        }
    }
    let _ = scripted;
    ctx.input(|i| i.pointer.hover_pos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{load_from_file, save_to_file, NodeData};

    /// A folder of its own for one test's files.
    fn folder(test: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("soba-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A short silent WAV at `path`.
    fn write_wav(path: &Path) {
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut wav = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..480 {
            wav.write_sample(0i16).unwrap();
        }
        wav.finalize().unwrap();
    }

    #[test]
    fn save_gathers_a_sample_dropped_in_after_the_first_save() {
        let root = folder("quick-save-gathers");
        let downloads = root.join("Downloads");
        let song = root.join("Song");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&song).unwrap();
        let kick = downloads.join("kick.wav");
        write_wav(&kick);

        // Saved once with nothing to gather, then a Sampler plays a file from elsewhere
        let path = song.join("Song.json");
        let mut patch = Patch::new("Song");
        let mut sampler = NodeData::new(1, sampler::ID, (0.0, 0.0));
        sampler.file = Some(kick.to_string_lossy().into_owned());
        patch.nodes.push(sampler);
        save_to_file(&patch, &path).unwrap();
        let mut app = SynthApp::new();
        app.open_file(&path);
        assert_eq!(app.quick_save_path(), Some(path.clone()));

        assert!(app.quick_save());
        let copy = sample_files::samples_folder(&path).join("kick.wav");
        assert!(copy.is_file(), "the sample is copied beside the patch");
        let saved = load_from_file(&path).unwrap();
        let file = saved.nodes[0].file.clone().unwrap();
        assert!(!Path::new(&file).is_absolute(), "and the patch names the copy relative to itself: {file}");
        assert!(app.load_warnings.is_empty(), "{:?}", app.load_warnings);
        assert!(kick.is_file(), "the original stays where it was");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn files_read_aside_are_reported_once_they_come() {
        let root = folder("read-aside");
        let kick = root.join("kick.wav");
        write_wav(&kick);
        let path = root.join("Song.json");
        let mut patch = Patch::new("Song");
        for (id, file) in [(1, "kick.wav"), (2, "gone.wav")] {
            let mut sampler = NodeData::new(id, sampler::ID, (0.0, 0.0));
            sampler.file = Some(file.to_string());
            patch.nodes.push(sampler);
        }
        save_to_file(&patch, &path).unwrap();

        let mut app = SynthApp::new();
        app.user_state.samples = crate::graph::sample_shelf::SampleShelf::reading_aside();
        app.open_file(&path);
        assert!(app.user_state.samples.waiting(), "opening doesn't wait for its files");
        app.user_state.samples.wait_for_reads();
        app.sync_samples();
        assert!(app.load_warnings.iter().any(|w| w.contains("Couldn't load sample gone.wav")), "{:?}", app.load_warnings);
        assert_eq!(app.load_warnings.len(), 1, "{:?}", app.load_warnings);

        // A file chosen for a Sampler says it loaded once it has
        let key = kick.to_string_lossy().into_owned();
        app.user_state.samples.forget(&key);
        assert!(app.check_sample(&key), "a file still being read may play");
        app.status_message = None;
        app.user_state.samples.wait_for_reads();
        app.sync_samples();
        assert_eq!(app.status_message.as_deref(), Some("Loaded kick.wav"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
