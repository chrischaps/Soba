//! Hands follow the selection (#429), the app's side: which knobs the
//! selection offers the hands, teaching Soba a controller from the MIDI In
//! menu, its keys (Tab skips, Enter finishes, Escape gives up), and the
//! toggle, Ctrl+Shift+L. What the hands are and how they turn is
//! `crate::app::hands`; their badges are `crate::graph::hand_badges`.
//!
//! Map Mode and MIDI Learn come first: while either is on, the hands let
//! go and the patch's mappings have the controller back.

use super::*;
use crate::app::hands::{HandTarget, SLOTS};
use crate::graph::groups::Control;

impl SynthApp {
    /// Takes hold of the selection's knobs, before the MIDI is heard, and
    /// puts the badges on them. With a group selected, the knobs pinned to
    /// its face; with several things selected, the first.
    pub(super) fn follow_hands(&mut self, now: f64) {
        let device = self.midi.selected_device().map(|d| d.name.clone());
        self.midi.hands.set_device(device.as_deref());
        self.midi.hands.held_back = self.map_mode_on() || self.is_midi_learning();
        let targets = if self.midi.hands.active() { self.hand_targets() } else { Vec::new() };
        if self.midi.hands.follow(&targets) {
            self.user_state.hands.since = now;
            self.user_state.hands.slots = self.midi.hands.badges();
        }
        self.user_state.hands.paused = self.midi.hands.paused(&self.midi.mappings);
        if let Some(news) = self.midi.hands.take_finished() {
            self.status_message = Some(news);
        }
    }

    /// The knobs the selection offers the hands, in the order its face
    /// shows them.
    pub(super) fn hand_targets(&self) -> Vec<HandTarget> {
        let graph = &self.graph_state.graph;
        let Some(node) = self.graph_state.selected_nodes.first().and_then(|&id| graph.nodes.get(id)) else {
            return Vec::new();
        };
        let knobs: Vec<(egui_node_graph2::NodeId, &str)> = match node.user_data.kind.group() {
            Some(group) => self.user_state.group_faces.get(&group).map_or_else(Vec::new, |face| {
                face.controls.iter().filter(|c| c.kind == Control::Knob).map(|c| (c.node, c.param.as_str())).collect()
            }),
            None => node.user_data.knob_params.iter().map(|k| (node.id, k.param_name.as_str())).collect(),
        };
        knobs.into_iter().filter_map(|(node_id, param)| self.hand_target(node_id, param)).take(SLOTS).collect()
    }

    /// A module's knob, as the hands would hold it.
    fn hand_target(&self, node_id: egui_node_graph2::NodeId, param_name: &str) -> Option<HandTarget> {
        let graph = &self.graph_state.graph;
        let node_id_engine = self.user_state.get_engine_node_id(node_id)?;
        let (_, input) = graph.nodes.get(node_id)?.inputs.iter().find(|(name, _)| name == param_name)?;
        let param_index = crate::graph::port_mapping::parameter_index(graph, node_id, *input)?;
        let (min_value, max_value) = graph.get_input(*input).value.range();
        Some(HandTarget { node_id: node_id_engine, param_index, param_name: param_name.to_string(), min_value, max_value })
    }

    /// Turns the hands on or off. They need a controller taught first.
    pub(super) fn toggle_hands(&mut self, ctx: &egui::Context) {
        let hands = &mut self.midi.hands;
        if !hands.on && hands.controller().is_none() {
            self.status_message = Some("Teach Soba your controller first: MIDI In ▸ Teach Your Controller…".to_string());
            return;
        }
        hands.on = !hands.on;
        self.status_message = Some(if hands.on {
            shortcuts::keys(ctx, "The encoders follow the selection (Ctrl+Shift+L lets go)").into_owned()
        } else {
            "The encoders let go: the patch's mappings are back".to_string()
        });
    }

    /// Starts teaching Soba the controller open, encoder by encoder, then
    /// fader by fader. Map Mode and MIDI Learn give way.
    pub(super) fn start_teaching(&mut self) {
        if self.map_mode_on() {
            self.user_state.map_mode.on = false;
        }
        if self.is_midi_learning() {
            self.midi.cancel_learn();
            self.show_midi_learn();
        }
        self.midi.hands.start_teaching();
    }

    /// Tab skips the control being taught, Enter finishes and Escape gives
    /// up, while Soba is being taught. Before the canvas's own Tab and
    /// Escape, which would open the palette or leave the group.
    pub(super) fn teach_keys(&mut self, ctx: &egui::Context) {
        if self.midi.hands.teaching().is_none() {
            return;
        }
        use egui::{Key, Modifiers};
        let (skip, done, cancel) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::Tab), i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape))
        });
        if cancel {
            self.midi.hands.cancel_teaching();
        } else if done {
            self.midi.hands.finish_teaching();
        } else if skip {
            self.midi.hands.skip();
        }
    }

    /// What the status bar asks for while Soba is taught: the control to
    /// move next, and the keys.
    pub(super) fn teach_status(&self) -> Option<String> {
        let teaching = self.midi.hands.teaching()?;
        let device = self.midi.selected_device().map_or("your controller", |d| d.name.as_str());
        Some(format!("Teaching {device} · {} · Tab skips, Enter finishes, Esc cancels", teaching.prompt()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::hands::{Controller, Taught};
    use crate::persistence::MidiEncoding;

    fn add(app: &mut SynthApp, module_id: &str) -> egui_node_graph2::NodeId {
        let template = SynthNodeTemplate::from_module_id(module_id).expect("registered");
        let (node_id, _) = editing::add_module(&mut app.graph_state, &mut app.user_state, template, egui::pos2(0.0, 0.0));
        node_id
    }

    /// An app whose controller (no device: a capture's) has 8 endless encoders and 4 faders.
    fn taught() -> SynthApp {
        let mut app = SynthApp::new();
        let taught = |cc| Some(Taught { cc, channel: 1, encoding: MidiEncoding::Relative64 });
        let controller = Controller { encoders: (70..78).map(taught).collect(), faders: (20..24).map(taught).collect() };
        app.midi.hands.know("", controller);
        app.midi.hands.on = true;
        app
    }

    #[test]
    fn a_selected_module_offers_its_knobs_in_the_order_its_face_shows_them() {
        let mut app = taught();
        let osc = add(&mut app, crate::modules::oscillator::ID);
        let filter = add(&mut app, crate::modules::filter::ID);
        assert!(app.hand_targets().is_empty(), "nothing selected");

        app.graph_state.selected_nodes = vec![osc];
        let names: Vec<String> = app.hand_targets().into_iter().map(|t| t.param_name).collect();
        let face: Vec<String> = app.graph_state.graph[osc].user_data.knob_params.iter().map(|k| k.param_name.clone()).take(SLOTS).collect();
        assert_eq!(names, face);

        // Several selected: the first
        app.graph_state.selected_nodes = vec![filter, osc];
        app.follow_hands(1.0);
        let filter_engine = app.user_state.get_engine_node_id(filter).unwrap();
        assert!(!app.user_state.hands.slots.is_empty());
        assert!(app.user_state.hands.slots.keys().all(|(node, _)| *node == filter_engine));
        assert_eq!(app.user_state.hands.since, 1.0, "the badges fade in");

        // Map Mode takes the controller back
        app.toggle_map_mode();
        app.follow_hands(2.0);
        assert!(app.user_state.hands.slots.is_empty());
    }

    #[test]
    fn a_selected_group_offers_the_knobs_pinned_to_its_face() {
        let mut app = taught();
        let ctx = egui::Context::default();
        let osc = add(&mut app, crate::modules::oscillator::ID);
        let filter = add(&mut app, crate::modules::filter::ID);
        app.group_selection(&ctx, &[osc, filter]);
        let pinned = app.graph_state.graph[filter].user_data.knob_params[1].param_name.clone();
        app.pin_knob(filter, &pinned, 1);
        app.prepare_level();
        let group = app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.kind.group().is_some()).map(|(id, _)| id).unwrap();
        app.graph_state.selected_nodes = vec![group];
        let targets = app.hand_targets();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].param_name, pinned);
        assert_eq!(targets[0].node_id, app.user_state.get_engine_node_id(filter).unwrap());
    }

    #[test]
    fn teaching_keys_skip_finish_and_cancel() {
        let mut app = SynthApp::new();
        app.toggle_map_mode();
        app.start_teaching();
        assert!(!app.map_mode_on(), "teaching takes the controller from Map Mode");
        assert!(app.teach_status().unwrap().contains("Turn encoder 1"));
        app.midi.hands.skip();
        assert!(app.teach_status().unwrap().contains("Encoder 1 skipped · turn encoder 2"));
        app.midi.hands.cancel_teaching();
        app.follow_hands(0.0);
        assert_eq!(app.status_message.as_deref(), Some("Teaching cancelled"));
        assert!(app.teach_status().is_none());

        // Without a controller, the toggle says what to do
        app.toggle_hands(&egui::Context::default());
        assert!(!app.midi.hands.on);
        assert!(app.status_message.as_deref().unwrap().starts_with("Teach Soba your controller first"));
    }
}
