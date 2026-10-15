//! Hearing a Sampler's file from its face: its ▶, an Alt-click on its
//! waveform, opening a file into it, or dragging one over it (see
//! [`super::file_hover`]). The engine plays the recording
//! beside the patch (see [`crate::engine::cue`]); this side chooses the
//! recording and the span, follows where it's got to for the face's
//! playhead, and stops it when its node goes or names another file.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use egui_node_graph2::NodeId;
use web_time::Instant;

use crate::dsp::SampleData;
use crate::engine::{Cue, NodeId as EngineNodeId, CANVAS_CUE};
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
        let Some(rate) = self.engine_rate() else { return };
        // The recording the Sampler plays, so nothing new is made for it
        let Some(sample) = self.user_state.samples.playing(&key, rate) else { return };
        self.start_cue(node_id, key, sample, from, to, false);
    }

    /// Plays a recording from a Sampler's face, at its Tune and Fine,
    /// replacing any cue playing. `hovered` if it's a file dragged over it.
    pub(super) fn start_cue(&mut self, node_id: NodeId, key: String, sample: Arc<SampleData>, from: f32, to: f32, hovered: bool) {
        let semitones = self.semitones(node_id);
        let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { return };
        let Some(handle) = self.ui_handle.as_mut() else { return };
        handle.cue(Some(Cue { node_id: engine_id, sample, from, to, semitones }));
        self.user_state.cue = Some(SamplerCue { node_id: Some(node_id), engine_id, key, from, place: from, heard: Instant::now(), begun: false, hovered });
    }

    /// Plays a file dragged over empty canvas, all of it and untuned, as a
    /// new Sampler would if it were dropped there, replacing any cue playing.
    pub(super) fn start_canvas_cue(&mut self, key: String, sample: Arc<SampleData>) {
        let Some(handle) = self.ui_handle.as_mut() else { return };
        handle.cue(Some(Cue { node_id: CANVAS_CUE, sample, from: 0.0, to: 1.0, semitones: 0.0 }));
        self.user_state.cue = Some(SamplerCue { node_id: None, engine_id: CANVAS_CUE, key, from: 0.0, place: 0.0, heard: Instant::now(), begun: false, hovered: true });
    }

    /// How far a Sampler transposes what it plays, by its Tune and Fine.
    pub(super) fn semitones(&self, node_id: NodeId) -> f32 {
        self.knob(node_id, "Tune").unwrap_or(0.0).round() + self.knob(node_id, "Fine").unwrap_or(0.0) / 100.0
    }

    /// The rate the engine plays at, if there's an engine.
    pub(super) fn engine_rate(&self) -> Option<f32> {
        self.ui_handle.as_ref().map(|handle| handle.graph().sample_rate())
    }

    /// Plays a Sampler's file from Start to End, as its ▶ does.
    pub(super) fn cue_start_to_end(&mut self, node_id: NodeId) {
        let (from, to) = self.start_to_end(node_id);
        self.cue_sample(node_id, from, to);
    }

    /// Where a Sampler plays from and to, 0 to 1 through its file: all of
    /// it for a Chopper, which has no Start and End.
    pub(super) fn start_to_end(&self, node_id: NodeId) -> (f32, f32) {
        let start = self.knob(node_id, "Start").unwrap_or(0.0);
        let end = self.knob(node_id, "End").unwrap_or(1.0);
        (start.min(end), start.max(end))
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
    /// file (unless it's a file dragged over it), lets go of one the engine
    /// has stopped reporting, and keeps its playhead moving.
    pub(super) fn tend_cue(&mut self, ctx: &egui::Context) {
        let Some(cue) = self.user_state.cue.as_ref() else { return };
        let still_there = match cue.node_id {
            // Over empty canvas, it plays while the file is dragged
            None => cue.hovered,
            Some(node_id) => {
                self.graph_state.graph.nodes.get(node_id).is_some_and(|node| cue.hovered || node.user_data.file.as_deref() == Some(&cue.key))
                    // Begun over the canvas, it reports as it did then
                    && (cue.engine_id == CANVAS_CUE || self.user_state.get_engine_node_id(node_id) == Some(cue.engine_id))
            }
        };
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
