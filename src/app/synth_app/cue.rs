//! Hearing a Sampler's file from its face: its ▶, an Alt-click on its
//! waveform, or opening a file into it. The engine plays the recording
//! beside the patch (see [`crate::engine::cue`]); this side chooses the
//! recording and the span, follows where it's got to for the face's
//! playhead, and stops it when its node goes or names another file.

use std::time::Duration;

use eframe::egui;
use egui_node_graph2::NodeId;
use web_time::Instant;

use crate::engine::{Cue, NodeId as EngineNodeId};
use crate::graph::{SamplerCue, SynthGraphState, SynthValueType};
use super::SynthApp;

/// How long a cue can go unheard of before it's taken to be over: the
/// engine reports every callback, so only a device that's gone is this quiet.
const UNHEARD: Duration = Duration::from_millis(500);

impl SynthApp {
    /// Plays a Sampler's file from `from` to `to` (0 to 1 through it), at
    /// the Sampler's Tune and Fine, replacing any cue playing.
    pub(super) fn cue_sample(&mut self, node_id: NodeId, from: f32, to: f32) {
        let Some(key) = self.graph_state.graph.nodes.get(node_id).and_then(|node| node.user_data.file.clone()) else { return };
        let semitones = self.knob(node_id, "Tune").unwrap_or(0.0).round() + self.knob(node_id, "Fine").unwrap_or(0.0) / 100.0;
        let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { return };
        let Some(rate) = self.ui_handle.as_ref().map(|handle| handle.graph().sample_rate()) else { return };
        // The recording the Sampler plays, so nothing new is made for it
        let Some(sample) = self.user_state.samples.playing(&key, rate) else { return };
        if let Some(handle) = self.ui_handle.as_mut() {
            handle.cue(Some(Cue { node_id: engine_id, sample, from, to, semitones }));
        }
        self.user_state.cue = Some(SamplerCue { node_id, engine_id, key, from, place: from, heard: Instant::now(), begun: false });
    }

    /// Plays a Sampler's file from Start to End, as its ▶ does.
    pub(super) fn cue_start_to_end(&mut self, node_id: NodeId) {
        let start = self.knob(node_id, "Start").unwrap_or(0.0);
        let end = self.knob(node_id, "End").unwrap_or(1.0);
        self.cue_sample(node_id, start.min(end), start.max(end));
    }

    /// A knob's value on a node, by name.
    fn knob(&self, node_id: NodeId, name: &str) -> Option<f32> {
        let graph = &self.graph_state.graph;
        let (_, input) = graph.nodes.get(node_id)?.inputs.iter().find(|(input, _)| input == name)?;
        match graph.get_input(*input).value {
            SynthValueType::Number { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Stops the cue playing, if one is.
    pub(super) fn stop_cue(&mut self) {
        if self.user_state.cue.take().is_some() {
            if let Some(handle) = self.ui_handle.as_mut() {
                handle.cue(None);
            }
        }
    }

    /// Once a frame: stops a cue whose Sampler has gone or names another
    /// file, lets go of one the engine has stopped reporting, and keeps its
    /// playhead moving.
    pub(super) fn tend_cue(&mut self, ctx: &egui::Context) {
        let Some(cue) = self.user_state.cue.as_ref() else { return };
        let still_there = self.graph_state.graph.nodes.get(cue.node_id).is_some_and(|node| node.user_data.file.as_deref() == Some(&cue.key))
            && self.user_state.get_engine_node_id(cue.node_id) == Some(cue.engine_id);
        if !still_there || cue.heard.elapsed() > UNHEARD {
            self.stop_cue();
            return;
        }
        ctx.request_repaint();
    }
}

/// Follows the engine's report of where a cue is: `None` once it's over.
pub(super) fn follow(state: &mut SynthGraphState, engine_id: EngineNodeId, place: Option<f32>) {
    let Some(cue) = state.cue.as_mut().filter(|cue| cue.engine_id == engine_id) else { return };
    match place {
        Some(place) => {
            cue.place = place;
            cue.heard = Instant::now();
            cue.begun = true;
        }
        None if cue.begun => state.cue = None,
        // The end of the cue this one replaced, from the same Sampler
        None => {}
    }
}
