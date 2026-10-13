//! Takes in the editor: keeping what a Sampler records with the patch.
//!
//! A Sampler records into room set aside here, off the audio thread: a
//! minute of stereo, handed to each Sampler with something patched into
//! In, and again whenever a take has used it up (see
//! [`crate::modules::sampler`]). When a take ends the Sampler plays it at
//! once, from that room, and the engine shares it with this side.
//!
//! Here it's written as a WAV beside the patch, in `<patch name> samples`,
//! and the Sampler's file then names it, as one undo step, so undo brings
//! back the file it had before. A patch with no home yet (new, or an
//! example) keeps its takes in a folder of the session's own, and Save As
//! gathers them beside it, as it does any sample from elsewhere. Last, the
//! Sampler is handed the same recording made to measure, which it swaps
//! for the room without a break.
//!
//! The browser has no folder beside the patch: a take lives in the session,
//! as a file dropped there does.

use std::sync::Arc;
use std::time::Duration;

use web_time::Instant;

use crate::dsp::SampleData;
use crate::engine::{EngineCommand, NodeId as EngineNodeId};
use crate::modules::sampler::{Sampler, SAMPLER_ID, TAKE_ROOM_SECONDS};
use crate::persistence::sample_files::{self, Decoded};
use super::SynthApp;

/// How long room handed to a Sampler has to show in its readout before
/// it's handed again.
const ROOM_PATIENCE: Duration = Duration::from_secs(1);

impl SynthApp {
    /// Keeps the takes Samplers have just recorded, and hands room to
    /// those that can record and have none. Call once a frame, before
    /// [`SynthApp::sync_samples`].
    pub(super) fn tend_takes(&mut self) {
        self.keep_takes();
        self.give_rooms();
    }

    /// Writes each take just recorded, points its Sampler at it, and
    /// swaps the Sampler's room for it.
    fn keep_takes(&mut self) {
        let mut takes = Vec::new();
        if let Some(handle) = self.ui_handle.as_mut() {
            while let Some(take) = handle.take_finished_take() {
                takes.push(take);
            }
        }
        for (engine_id, count, room) in takes {
            let measured = room.to_measure();
            // The room is the Sampler's until it's swapped for the copy
            drop(room);
            let Some(node_id) = self.user_state.node_id_map.iter().find(|(_, &id)| id == engine_id).map(|(&node_id, _)| node_id) else { continue };
            let seconds = measured.seconds();
            match self.keep_take(measured) {
                Ok(key) => {
                    let name = sample_files::file_name(&key).to_string();
                    self.forget_slice_markers(node_id);
                    if let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) {
                        node.user_data.file = Some(key.clone());
                    }
                    self.history.name_next(format!("Record {name}"));
                    let rate = self.ui_handle.as_ref().map(|handle| handle.graph().sample_rate());
                    if let Some(sample) = rate.and_then(|rate| self.user_state.samples.playing(&key, rate)) {
                        self.send_command(EngineCommand::SettleTake { node_id: engine_id, take: count, sample });
                    }
                    self.status_message = Some(format!("Recorded {name}, {seconds:.1} s"));
                }
                Err(e) => self.status_message = Some(format!("Couldn't keep the take: {e}. The Sampler plays it until it's given another file")),
            }
        }
    }

    /// Keeps a take on the shelf, and on disk beside the patch where there
    /// is one, under a name of its own. Returns its key.
    fn keep_take(&mut self, sample: SampleData) -> Result<String, String> {
        self.keep_beside_patch(sample, |n| format!("Take {n}.wav"))
    }

    /// Keeps a recording made in the editor (a take, or a file trimmed) on
    /// the shelf, and on disk beside the patch where there is one, under the
    /// first name `name(1)`, `name(2)`… that's free. Returns its key.
    pub(super) fn keep_beside_patch(&mut self, sample: SampleData, name: impl Fn(usize) -> String) -> Result<String, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let folder = match &self.current_patch_path {
                Some(path) => sample_files::samples_folder(path),
                None => std::env::temp_dir().join(format!("Soba takes {}", std::process::id())),
            };
            std::fs::create_dir_all(&folder).map_err(|e| format!("couldn't make {}: {e}", folder.display()))?;
            let path = (1..).map(|n| folder.join(name(n))).find(|path| !path.exists()).expect("some name is free");
            // Written whole beside it first, so a failed write leaves nothing half done
            let partial = path.with_extension("wav.partial");
            std::fs::write(&partial, sample_files::encode_wav(&sample)).map_err(|e| format!("{}: {e}", path.display()))?;
            std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
            let key = path.to_string_lossy().into_owned();
            self.user_state.samples.insert(&key, Decoded { sample, truncated: false });
            Ok(key)
        }
        #[cfg(target_arch = "wasm32")]
        {
            use crate::graph::sample_shelf::DROPPED_PREFIX;
            let key = (1..)
                .map(|n| format!("{DROPPED_PREFIX}{}", name(n)))
                .find(|key| self.user_state.samples.get(key).is_none())
                .expect("some name is free");
            self.user_state.samples.insert(&key, Decoded { sample, truncated: false });
            Ok(key)
        }
    }

    /// Hands room to record into to each Sampler with something patched
    /// into In that has none, once it's on the audio thread.
    fn give_rooms(&mut self) {
        let Some(handle) = self.ui_handle.as_ref() else { return };
        let graph = handle.graph();
        let rate = graph.sample_rate();
        let now = Instant::now();
        let wanting: Vec<EngineNodeId> = self
            .graph_state
            .graph
            .nodes
            .iter()
            .filter(|(_, node)| node.user_data.module_id == SAMPLER_ID)
            .filter(|(_, node)| {
                ["In L", "In R"]
                    .iter()
                    .any(|name| node.get_input(name).is_ok_and(|input| self.graph_state.graph.connection(input).is_some()))
            })
            .filter_map(|(node_id, _)| self.user_state.get_engine_node_id(node_id))
            .filter(|&engine_id| graph.contains_module(engine_id) && !graph.is_fresh(engine_id))
            .filter(|engine_id| {
                // Its readout says it has none, and none is on its way
                let holds = self.user_state.readouts.get(engine_id).map(|r| r.values[Sampler::READOUT_ROOM] > 0.0);
                holds == Some(false) && self.rooms_given.get(engine_id).is_none_or(|&given| now.duration_since(given) > ROOM_PATIENCE)
            })
            .collect();
        for engine_id in wanting {
            let room = Arc::new(SampleData::with_room((TAKE_ROOM_SECONDS * rate) as usize, rate));
            self.send_command(EngineCommand::GiveRoom { node_id: engine_id, room });
            self.rooms_given.insert(engine_id, now);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::modules::sampler::RecState;
    use crate::persistence::{load_from_file, save_to_file, ConnectionData, NodeData, Patch};
    use egui_node_graph2::NodeId;
    use std::path::Path;

    /// Runs the app's per-frame engine work, without a window, for `seconds`.
    fn pump(app: &mut SynthApp, seconds: f32) {
        let end = Instant::now() + Duration::from_secs_f32(seconds);
        while Instant::now() < end {
            app.sync_parameters();
            app.tend_takes();
            app.sync_samples();
            if let Some(handle) = app.ui_handle.as_mut() {
                handle.flush();
            }
            app.process_engine_events();
            app.sync_history();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn the_sampler(app: &SynthApp) -> (NodeId, EngineNodeId) {
        let (node_id, _) = app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.module_id == SAMPLER_ID).expect("a Sampler");
        (node_id, app.user_state.get_engine_node_id(node_id).unwrap())
    }

    fn file(app: &SynthApp) -> Option<String> {
        app.graph_state.graph.nodes[the_sampler(app).0].user_data.file.clone()
    }

    fn state(app: &SynthApp) -> RecState {
        let (_, engine_id) = the_sampler(app);
        RecState::from_code(app.user_state.readouts.get(&engine_id).map_or(0.0, |r| r.values[Sampler::READOUT_REC_STATE]))
    }

    /// Presses the Sampler's Rec on its face.
    fn press_rec(app: &mut SynthApp) {
        let (node_id, _) = the_sampler(app);
        app.play_parameter(node_id, "Rec Button", 1.0);
        pump(app, 0.08);
        app.play_parameter(node_id, "Rec Button", 0.0);
        pump(app, 0.05);
    }

    /// The whole round, on the real audio device: a sine recorded into a
    /// Sampler with no file dialog, written beside the patch, undone and
    /// redone, saved and opened again. The patch has no output module, so
    /// it makes no sound.
    #[test]
    #[ignore = "needs an audio device; run with --ignored"]
    fn takes_are_kept_with_the_patch() {
        let mut app = SynthApp::new(false);
        if app.audio_engine.is_err() {
            eprintln!("no audio device: skipped");
            return;
        }
        let folder = std::env::temp_dir().join(format!("soba-takes-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("Song.json");

        // A sine into a Sampler's In L, opened as a patch file
        let mut patch = Patch::new("Song");
        patch.nodes.push(NodeData::new(1, "osc.sine", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, SAMPLER_ID, (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 2, "In L"));
        save_to_file(&patch, &path).unwrap();
        app.open_file(&path);
        app.is_playing = true;
        app.user_state.is_playing = true;
        app.send_command(EngineCommand::SetPlaying(true));
        pump(&mut app, 0.3);
        let (_, engine_id) = the_sampler(&app);
        assert!(app.user_state.readouts[&engine_id].values[Sampler::READOUT_ROOM] >= TAKE_ROOM_SECONDS - 0.1, "room arrives");
        assert!(!app.has_unsaved_changes());

        // About half a second, recorded from the face
        press_rec(&mut app);
        assert_eq!(state(&app), RecState::Recording);
        pump(&mut app, 0.4);
        press_rec(&mut app);
        pump(&mut app, 0.2);
        assert_eq!(state(&app), RecState::Idle);

        // Beside the patch, named by the Sampler, which plays it, and room for the next
        let key = file(&app).expect("the Sampler names its take");
        assert_eq!(Path::new(&key), folder.join("Song samples").join("Take 1.wav"));
        let take = sample_files::read_wav_file(Path::new(&key)).unwrap().sample;
        assert!((0.4..0.8).contains(&take.seconds()), "{}", take.seconds());
        assert!(!take.is_stereo(), "one side patched: mono");
        assert!(take.left().iter().any(|&s| s.abs() > 0.5), "the sine");
        let held = app.ui_handle.as_ref().unwrap().graph().sample(engine_id).cloned().unwrap();
        assert_eq!(held.as_ref(), &take, "settled to the file's recording");
        assert!(app.user_state.readouts[&engine_id].values[Sampler::READOUT_ROOM] > 0.0, "room again");
        assert!(app.has_unsaved_changes());

        // Undo brings back no file; redo the take
        app.undo();
        pump(&mut app, 0.1);
        assert_eq!(file(&app), None);
        assert!(app.ui_handle.as_ref().unwrap().graph().sample(engine_id).is_none());
        app.redo();
        pump(&mut app, 0.1);
        assert_eq!(file(&app).as_deref(), Some(key.as_str()));

        // Saved, the patch names it beside itself; opened again, it plays it
        assert!(app.quick_save());
        let saved = load_from_file(&path).unwrap();
        let named = saved.nodes.iter().find(|n| n.module_id == SAMPLER_ID).and_then(|n| n.file.clone());
        assert_eq!(named.as_deref(), Some("Song samples/Take 1.wav"));
        app.open_file(&path);
        pump(&mut app, 0.3);
        assert_eq!(file(&app).as_deref(), Some(key.as_str()));
        let (_, engine_id) = the_sampler(&app);
        assert_eq!(app.ui_handle.as_ref().unwrap().graph().sample(engine_id).map(|s| s.as_ref()), Some(&take));

        // A second take is a second file
        press_rec(&mut app);
        pump(&mut app, 0.2);
        press_rec(&mut app);
        pump(&mut app, 0.2);
        assert_eq!(file(&app).map(|key| sample_files::file_name(&key).to_string()).as_deref(), Some("Take 2.wav"));

        app.send_command(EngineCommand::SetPlaying(false));
        pump(&mut app, 0.05);
        std::fs::remove_dir_all(&folder).ok();
    }
}
