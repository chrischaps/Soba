//! Recording into a new Sampler from a right-click (see
//! [`crate::graph::resample`]): a Sampler placed beside the module, patched
//! from it, listening at −40 dB, and its Rec pressed as soon as it holds
//! room to record into, so the next sound is the take. A take in bars
//! (with a Clock running) waits for the bar line instead, at −∞, and loops.
//!
//! Placing and patching it is one undo step. The press isn't one, as a
//! press on the face isn't: it's played, like a footswitch.

use std::time::Duration;

use egui::{Pos2, Rect, Vec2};
use egui_node_graph2::NodeId;
use web_time::Instant;

use crate::graph::resample::Source;
use crate::graph::sampler_take::{MIN_PRESS_SECONDS, REC_BUTTON};
use crate::graph::SynthNodeTemplate;
use crate::graph::resample::bar_name;
use crate::modules::sampler::{self, Sampler, LOOP_MODES, NO_THRESHOLD_DB, TAKE_LENGTHS};
use super::super::editing;
use super::super::engine_sync;
use super::framing::Target;
use super::SynthApp;

/// Where a new Sampler's Thresh starts: under a voice's first word, over a
/// quiet room.
pub const LISTEN_DB: f32 = -40.0;

/// Space between the module and its new Sampler, in patch points.
const GAP: f32 = 48.0;

/// About how big a Sampler draws, in patch points, before one has been.
const SAMPLER_SIZE: Vec2 = Vec2::new(230.0, 770.0);

/// How far each try for a clear place moves on from the last, in patch points.
const STEP: f32 = 40.0;

/// A new Sampler whose Rec is to be pressed.
pub(super) struct Arming {
    node_id: NodeId,
    /// When Rec went down, holding for a moment before it's let go.
    pressed: Option<Instant>,
}

impl SynthApp {
    /// Places a Sampler beside `node_id`, patched from its `source`,
    /// listening at −40 dB, and presses its Rec once it can record; or, for
    /// a take `bars` long, with Take in bars, Thresh at −∞ (the bar line is
    /// the cue, not the sound) and Loop on Forward. Starts
    /// the transport if it's stopped, since a stopped patch hears nothing,
    /// and brings the two into view if the Sampler landed out of it.
    pub(super) fn record_into_new_sampler(&mut self, node_id: NodeId, source: &Source, bars: u32) {
        let Some(&from) = self.graph_state.node_positions.get(node_id) else { return };
        let Some(template) = SynthNodeTemplate::from_module_id(sampler::ID) else { return };
        let size = self.sampler_size() * self.graph_state.pan_zoom.zoom;
        let at = self.clear_place_beside(node_id, from, size);
        let (sampler, mut commands) = editing::add_module(&mut self.graph_state, &mut self.user_state, template, at);

        let graph = &mut self.graph_state.graph;
        let sides = [(Some(&source.left), "In L"), (source.right.as_ref(), "In R")];
        for (output, input) in sides {
            let output = output.and_then(|name| graph[node_id].get_output(name).ok());
            if let (Some(output), Ok(input)) = (output, graph[sampler].get_input(input)) {
                graph.add_connection(output, input, 0);
            }
        }
        let take = (bars > 0).then(|| TAKE_LENGTHS.iter().position(|&name| name == bar_name(bars))).flatten();
        let choice = |names: &[&str], name: &str| names.iter().position(|&n| n == name).unwrap_or(0) as f32;
        let settings = match take {
            Some(take) => vec![("Rec Threshold", NO_THRESHOLD_DB), ("Take", take as f32), ("Loop", choice(LOOP_MODES, "Forward"))],
            None => vec![("Rec Threshold", LISTEN_DB)],
        };
        for (name, value) in settings {
            if let Ok(input) = graph[sampler].get_input(name) {
                graph.inputs[input].value.set_actual_value(value);
            }
        }
        commands.extend(engine_sync::sync_cables(&self.graph_state.graph, &mut self.user_state));
        for command in commands {
            self.send_command(command);
        }

        let label = crate::graph::resample::label(&self.graph_state.graph, node_id);
        self.history.name_next(label);
        self.graph_state.selected_nodes = vec![sampler];
        self.revealing = Some((node_id, sampler));
        self.arming.push(Arming { node_id: sampler, pressed: None });
        let started = !self.is_playing;
        if started {
            self.set_playing(true);
        }
        let waiting = match take {
            Some(_) => format!("Waiting for the next bar: the take lasts {} and loops", bar_name(bars)),
            None => "Waiting for a sound: the take starts on it, and Rec stops it".to_string(),
        };
        self.status_message = Some(if started { format!("Playing. {waiting}") } else { waiting });
    }

    /// Brings a new Sampler and what it records into view, if it landed
    /// out of it, once it has been drawn and measured: framed at a guessed
    /// size, a glide would have to change course on the way.
    pub(super) fn reveal_new_sampler(&mut self, ctx: &egui::Context) {
        let Some((node_id, sampler)) = self.revealing else { return };
        if !self.graph_state.graph.nodes.contains_key(sampler) {
            self.revealing = None;
            return;
        }
        if !self.node_sizes.contains_key(&sampler) {
            return;
        }
        self.revealing = None;
        let in_view = crate::graph::annotation_ui::module_rect(ctx, sampler).is_some_and(|drawn| self.editor_rect.contains_rect(drawn));
        if !in_view {
            self.start_glide(ctx, Target::Selection { nodes: vec![node_id, sampler], annotations: Vec::new() });
        }
    }

    /// How big a Sampler draws, unzoomed: as one already has, or about.
    fn sampler_size(&self) -> Vec2 {
        let graph = &self.graph_state.graph;
        self.node_sizes
            .iter()
            .find(|(n, _)| graph.nodes.get(**n).is_some_and(|node| node.user_data.module_id == sampler::ID))
            .map_or(SAMPLER_SIZE, |(_, &size)| size)
    }

    /// Where a new Sampler `size` big (zoomed) goes beside the node at
    /// `from`, as an editor position: the first clear place to its right,
    /// below it, further down on its right, or to its left.
    fn clear_place_beside(&self, node_id: NodeId, from: Pos2, size: Vec2) -> Pos2 {
        let zoom = self.graph_state.pan_zoom.zoom;
        let size_of = |n: NodeId| self.node_sizes.get(&n).copied().unwrap_or(crate::graph::groups::NOMINAL_NODE_SIZE) * zoom;
        let others: Vec<Rect> = self
            .graph_state
            .node_order
            .iter()
            .filter(|n| !self.user_state.hidden.contains(n))
            .filter_map(|&n| Some(Rect::from_min_size(*self.graph_state.node_positions.get(n)?, size_of(n))))
            .collect();
        let beside = size_of(node_id);
        let (gap, step) = (GAP * zoom, STEP * zoom);
        let right = from + Vec2::new(beside.x + gap, 0.0);
        let below = from + Vec2::new(0.0, beside.y + gap);
        let left = from - Vec2::new(size.x + gap, 0.0);
        let further: Vec<Pos2> = (1..16).map(|k| right + Vec2::new(0.0, k as f32 * step)).collect();
        let lower: Vec<Pos2> = (1..16).map(|k| below + Vec2::new(0.0, k as f32 * step)).collect();
        [right, below].into_iter()
            .chain(further)
            .chain(std::iter::once(left))
            .chain(lower)
            .find(|&at| {
                let place = Rect::from_min_size(at, size).expand(8.0 * zoom);
                !others.iter().any(|other| other.intersects(place))
            })
            .unwrap_or(right)
    }

    /// Presses Rec on new Samplers that can record now, and lets it go a
    /// moment later. One taken away (undone) before then is let be. Call
    /// once a frame.
    pub(super) fn tend_arming(&mut self) {
        let now = Instant::now();
        for mut arming in std::mem::take(&mut self.arming) {
            let Some(engine_id) = self.user_state.get_engine_node_id(arming.node_id) else { continue };
            match arming.pressed {
                Some(at) if now.duration_since(at) >= Duration::from_secs_f64(MIN_PRESS_SECONDS) => {
                    self.play_parameter(arming.node_id, REC_BUTTON, 0.0);
                    continue;
                }
                Some(_) => {}
                None => {
                    let holds_room = self.user_state.readouts.get(&engine_id).is_some_and(|r| r.values[Sampler::READOUT_ROOM] > 0.0);
                    if holds_room {
                        self.play_parameter(arming.node_id, REC_BUTTON, 1.0);
                        arming.pressed = Some(now);
                    }
                }
            }
            self.arming.push(arming);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::engine::EngineCommand;
    use crate::modules::sampler::RecState;
    use crate::persistence::{save_to_file, NodeData, Patch};

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

    /// A patch of these modules, side by side, opened from a file.
    fn opened(app: &mut SynthApp, name: &str, modules: &[&str]) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("soba-record-into-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("Song.json");
        let mut patch = Patch::new("Song");
        for (k, module) in modules.iter().enumerate() {
            patch.nodes.push(NodeData::new(k as u64 + 1, *module, (k as f32 * 400.0, 0.0)));
        }
        save_to_file(&patch, &path).unwrap();
        app.open_file(&path);
        folder
    }

    fn node(app: &SynthApp, module_id: &str) -> NodeId {
        app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.module_id == module_id).map(|(id, _)| id).expect(module_id)
    }

    /// The modules patched into a node's input, by output name.
    fn patched_from(app: &SynthApp, node_id: NodeId, input: &str) -> Option<(NodeId, String)> {
        let graph = &app.graph_state.graph;
        let output = graph.connection(graph[node_id].get_input(input).ok()?)?;
        let from = graph.get_output(output).node;
        let name = graph[from].outputs.iter().find(|(_, id)| *id == output).map(|(n, _)| n.clone())?;
        Some((from, name))
    }

    #[test]
    fn test_a_new_sampler_lands_beside_patched_and_listening_as_one_step() {
        let mut app = SynthApp::new(false);
        let folder = opened(&mut app, "wiring", &["util.mixer", "osc.sine"]);
        let mixer = node(&app, "util.mixer");
        pump(&mut app, 0.05);

        let send = crate::graph::resample::source_of(&app.graph_state.graph, mixer, "Send R").unwrap();
        app.record_into_new_sampler(mixer, &send, 0);
        pump(&mut app, 0.05);
        let sampler = node(&app, sampler::ID);
        assert_eq!(patched_from(&app, sampler, "In L"), Some((mixer, "Send L".into())));
        assert_eq!(patched_from(&app, sampler, "In R"), Some((mixer, "Send R".into())));
        let graph = &app.graph_state.graph;
        let threshold = graph.get_input(graph[sampler].get_input("Rec Threshold").unwrap()).value.actual_value();
        assert_eq!(threshold, LISTEN_DB);
        assert_eq!(app.graph_state.selected_nodes, [sampler]);

        // Beside the Mixer, clear of it and of the oscillator to its right
        let positions = &app.graph_state.node_positions;
        let place = Rect::from_min_size(positions[sampler], SAMPLER_SIZE);
        for other in [mixer, node(&app, "osc.sine")] {
            let rect = Rect::from_min_size(positions[other], crate::graph::groups::NOMINAL_NODE_SIZE);
            assert!(!rect.intersects(place), "{place:?} clear of {rect:?}");
        }
        assert!(place.min.distance(positions[mixer]) < 800.0, "{place:?} near {:?}", positions[mixer]);

        // Undone in one step: the Sampler and its cables go together
        app.undo();
        pump(&mut app, 0.05);
        assert!(app.graph_state.graph.nodes.iter().all(|(_, n)| n.user_data.module_id != sampler::ID));
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn test_a_take_in_bars_waits_for_the_bar_and_loops() {
        let mut app = SynthApp::new(false);
        let folder = opened(&mut app, "bars", &["util.clock", "osc.sine"]);
        let sine = node(&app, "osc.sine");
        pump(&mut app, 0.05);

        let out = crate::graph::resample::source_of(&app.graph_state.graph, sine, "Out").unwrap();
        app.record_into_new_sampler(sine, &out, 2);
        pump(&mut app, 0.05);
        let sampler = node(&app, sampler::ID);
        let graph = &app.graph_state.graph;
        let value = |name: &str| graph.get_input(graph[sampler].get_input(name).unwrap()).value.actual_value();
        assert_eq!(TAKE_LENGTHS[value("Take") as usize], "2 bars");
        assert_eq!(LOOP_MODES[value("Loop") as usize], "Forward");
        assert_eq!(value("Rec Threshold"), NO_THRESHOLD_DB, "the bar line is the cue, not the sound");
        assert!(app.status_message.as_deref().is_some_and(|m| m.contains("next bar")), "{:?}", app.status_message);

        // Free, beside it, listens as before
        app.record_into_new_sampler(sine, &out, 0);
        pump(&mut app, 0.05);
        let graph = &app.graph_state.graph;
        let free = app.graph_state.selected_nodes[0];
        let value = |name: &str| graph.get_input(graph[free].get_input(name).unwrap()).value.actual_value();
        assert_eq!(TAKE_LENGTHS[value("Take") as usize], "Free");
        assert_eq!(LOOP_MODES[value("Loop") as usize], "Off");
        assert_eq!(value("Rec Threshold"), LISTEN_DB);
        std::fs::remove_dir_all(&folder).ok();
    }

    /// On the real audio device: a sine resampled from its menu, recorded
    /// from the moment the Sampler can, then stopped from the face. The
    /// patch has no output module, so it makes no sound.
    #[test]
    #[ignore = "needs an audio device; run with --ignored"]
    fn resampling_records_the_next_sound() {
        let mut app = SynthApp::new(false);
        if app.audio.engine.is_err() {
            eprintln!("no audio device: skipped");
            return;
        }
        let folder = opened(&mut app, "take", &["osc.sine"]);
        let sine = node(&app, "osc.sine");
        pump(&mut app, 0.1);
        assert!(!app.is_playing);

        let out = crate::graph::resample::source_of(&app.graph_state.graph, sine, "Out").unwrap();
        app.record_into_new_sampler(sine, &out, 0);
        assert!(app.is_playing, "a stopped patch hears nothing, so it plays");
        pump(&mut app, 0.5);
        let sampler = node(&app, sampler::ID);
        let engine_id = app.user_state.get_engine_node_id(sampler).unwrap();
        let state = |app: &SynthApp| RecState::from_code(app.user_state.readouts[&engine_id].values[Sampler::READOUT_REC_STATE]);
        assert_eq!(state(&app), RecState::Recording, "the sine is over Thresh at once");
        assert!(app.arming.is_empty(), "pressed and let go");

        // Stopped from the face: a take, named by the Sampler
        app.play_parameter(sampler, REC_BUTTON, 1.0);
        pump(&mut app, 0.08);
        app.play_parameter(sampler, REC_BUTTON, 0.0);
        pump(&mut app, 0.3);
        assert_eq!(state(&app), RecState::Idle);
        let key = app.graph_state.graph[sampler].user_data.file.clone().expect("the take");
        assert!(key.ends_with("Take 1.wav"), "{key}");

        app.send_command(EngineCommand::SetPlaying(false));
        pump(&mut app, 0.05);
        std::fs::remove_dir_all(&folder).ok();
    }

    /// On the real audio device: a bar of a sine, resampled with a Clock
    /// running, comes in and stops on its own, with no press to end it.
    #[test]
    #[ignore = "needs an audio device; run with --ignored"]
    fn a_take_in_bars_stops_itself() {
        let mut app = SynthApp::new(false);
        if app.audio.engine.is_err() {
            eprintln!("no audio device: skipped");
            return;
        }
        let folder = opened(&mut app, "bar-take", &["util.clock", "osc.sine"]);
        let sine = node(&app, "osc.sine");
        pump(&mut app, 0.1);

        let out = crate::graph::resample::source_of(&app.graph_state.graph, sine, "Out").unwrap();
        app.record_into_new_sampler(sine, &out, 1);
        let sampler = node(&app, sampler::ID);
        // Up to a bar to the line, a bar of take, at the Clock's 120 BPM
        let end = Instant::now() + Duration::from_secs(6);
        while app.graph_state.graph[sampler].user_data.file.is_none() && Instant::now() < end {
            pump(&mut app, 0.1);
        }
        let key = app.graph_state.graph[sampler].user_data.file.clone().expect("a take, ended by the bars");
        assert!(key.ends_with("Take 1.wav"), "{key}");

        app.send_command(EngineCommand::SetPlaying(false));
        pump(&mut app, 0.05);
        std::fs::remove_dir_all(&folder).ok();
    }
}
