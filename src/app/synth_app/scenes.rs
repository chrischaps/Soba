//! Scenes, from the app's side: the pads' clicks, Morph's glide, and
//! Ctrl+Shift+M, which keeps the knobs Mutate left as the next free scene.
//!
//! Storing, recalling and clearing a scene are undo steps of their own. A
//! glide isn't: the knobs it moves follow from Morph, From and To, so undo
//! takes back a turn of Morph (or of From or To) and the knobs glide back
//! after it, as MIDI CC's knobs aren't steps either. The glide only moves
//! knobs as Morph moves, so a knob turned by hand keeps its place until
//! Morph moves again.

use std::collections::HashMap;

use eframe::egui;
use egui_node_graph2::NodeId;

use crate::engine::NodeId as EngineNodeId;
use crate::graph::scenes::{self, Settings};
use crate::graph::{port_mapping, SceneAction};
use crate::modules::scenes::SCENES_ID;
use super::SynthApp;

/// How far Morph has to move before the knobs follow.
const MORPH_EPSILON: f32 = 1e-4;

/// Where each Scenes module's Morph, From and To were when its knobs last
/// followed them.
#[derive(Default)]
pub(super) struct Gliding {
    applied: HashMap<EngineNodeId, (f32, usize, usize)>,
}

impl SynthApp {
    /// A click on a Scenes pad.
    pub(super) fn scene_pad(&mut self, node_id: NodeId, slot: usize, action: SceneAction) {
        let graph = &self.graph_state.graph;
        let Some(node) = graph.nodes.get(node_id) else { return };
        let letter = scenes::letter(slot);
        match action {
            SceneAction::Store => {
                let scene = scenes::capture(graph, &self.user_state, node_id);
                let count: usize = scene.values().map(|params| params.len()).sum();
                self.graph_state.graph[node_id].user_data.scenes.insert(letter.to_string(), scene);
                self.history.name_next(format!("Keep scene {letter}"));
                self.status_message = Some(format!("Kept {count} knobs as scene {letter}"));
            }
            SceneAction::Recall => {
                let Some(scene) = node.user_data.scenes.get(letter) else { return };
                let settings = scenes::recall(graph, &self.user_state, node_id, scene);
                scenes::apply(&mut self.graph_state.graph, &settings);
                self.history.name_next(format!("Recall scene {letter}"));
            }
            SceneAction::Clear => {
                self.graph_state.graph[node_id].user_data.scenes.remove(letter);
                self.history.name_next(format!("Clear scene {letter}"));
            }
        }
    }

    /// Ctrl+Shift+M keeps the knobs as they stand (after a mutation you
    /// like) in the next free scene of the Scenes module that reaches the
    /// selection, or the patch's.
    pub(super) fn handle_scene_key(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        if !ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::M)) {
            return;
        }
        let graph = &self.graph_state.graph;
        let found = match self.graph_state.selected_nodes.first() {
            Some(&node) => scenes::scenes_for(graph, node),
            None => graph.nodes.iter().find(|(_, n)| n.user_data.module_id == SCENES_ID && n.user_data.parent.is_none()).map(|(id, _)| id),
        };
        let Some(scenes_node) = found else {
            self.status_message = Some("Add a Scenes module to keep variations as scenes".to_string());
            return;
        };
        let Some(slot) = scenes::free_slot(&graph[scenes_node].user_data.scenes) else {
            self.status_message = Some("Every scene is full: Alt+click one on the Scenes module to clear it".to_string());
            return;
        };
        self.scene_pad(scenes_node, slot, SceneAction::Store);
    }

    /// Lets the knobs follow each Scenes module's Morph. Call it once a
    /// frame, before the knobs go to the engine.
    pub(super) fn glide_scenes(&mut self, ctx: &egui::Context) {
        let graph = &self.graph_state.graph;
        let mut glides: Vec<(EngineNodeId, Settings)> = Vec::new();
        let mut patched = false;
        let mut seen = Vec::new();
        for (node_id, node) in graph.nodes.iter() {
            if node.user_data.module_id != SCENES_ID {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            let Some(at @ (morph, from, to)) = scenes::morph_at(graph, &self.user_state, node_id) else { continue };
            patched |= node.get_input("Morph").is_ok_and(|input| graph.connection(input).is_some());
            seen.push(engine_id);
            // A Scenes just loaded, pasted or added starts from where its
            // knobs are, rather than snapping them to Morph
            let Some(&(was, was_from, was_to)) = self.gliding.applied.get(&engine_id) else {
                self.gliding.applied.insert(engine_id, at);
                continue;
            };
            if (morph - was).abs() < MORPH_EPSILON && from == was_from && to == was_to {
                continue;
            }
            self.gliding.applied.insert(engine_id, at);
            let bank = &node.user_data.scenes;
            let (Some(a), Some(b)) = (bank.get(scenes::letter(from)), bank.get(scenes::letter(to))) else { continue };
            glides.push((engine_id, scenes::morph(graph, &self.user_state, node_id, a, b, morph)));
        }
        self.gliding.applied.retain(|id, _| seen.contains(id));

        for (_, settings) in glides {
            for input in scenes::apply(&mut self.graph_state.graph, &settings) {
                // Not an edit: undo follows Morph, and the knobs follow it
                let graph = &self.graph_state.graph;
                let node_id = graph.get_input(input).node;
                let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
                let Some(index) = port_mapping::parameter_inputs(graph, node_id).iter().position(|&i| i == input) else { continue };
                self.history.absorb_param(engine_id, index, graph.get_input(input).value.actual_value());
            }
        }
        // CV moves Morph without anything else asking for a frame
        if patched {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// Forgets where every Morph was, as a patch is opened or started.
    pub(super) fn forget_glides(&mut self) {
        self.gliding.applied.clear();
    }
}
