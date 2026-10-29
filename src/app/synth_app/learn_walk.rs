//! Learn in order (#481), the app's side: a learn walked across a group's
//! Pads or Controls (#483), each step an ordinary learn into a module.
//! Starting the walk starts the first one's learn; once the MIDI heard says it took a hit,
//! the next one's starts, past the hits already taken. Tab (or Skip) leaves
//! one as it is and Escape stops. A learn cancelled or started elsewhere
//! stops it too.
//!
//! The walk is one undo step: the history waits while it goes, as it waits
//! for a drag, and records it, named, when it ends. Undo or redo ends it
//! first. Which modules a walk takes and in what order, and how the faces
//! show it, is `crate::graph::learn_walk`.

use super::*;
use crate::graph::learn_walk::{self, LearnWalk};
use crate::graph::{SynthResponse, WalkAction};

impl SynthApp {
    /// Does what a face or a menu asked of a walk.
    pub(super) fn learn_walk(&mut self, action: WalkAction) {
        match action {
            WalkAction::Start(group_node) => self.start_learn_walk(group_node),
            WalkAction::Skip => {
                if let Some(walk) = self.user_state.learn_walk.as_mut() {
                    walk.skip();
                    self.walk_on();
                }
            }
            WalkAction::Stop => self.end_learn_walk(),
        }
    }

    /// Starts learning the group `group_node`'s Pads and Controls in order,
    /// top row first.
    fn start_learn_walk(&mut self, group_node: egui_node_graph2::NodeId) {
        self.end_learn_walk();
        let graph = &self.graph_state.graph;
        let Some(node) = graph.nodes.get(group_node) else { return };
        let Some(id) = node.user_data.kind.group() else { return };
        let name = node.user_data.display_name.clone();
        let positions = &self.graph_state.node_positions;
        let steps = learn_walk::steps(graph, |n| positions.get(n).copied(), id, self.graph_state.pan_zoom.zoom);
        let Some(walk) = LearnWalk::new(id, name.clone(), steps) else {
            self.status_message = Some(format!("Nothing in {name} learns in order: it has no Pads or Controls"));
            return;
        };
        // Map Mode and teaching the controller give way, as for any learn
        if self.map_mode_on() {
            self.user_state.map_mode.on = false;
        }
        self.user_state.learn_walk = Some(walk);
        self.walk_on();
    }

    /// Starts the learn of the module waiting, passing any gone from the
    /// patch, or ends the walk once none is left.
    fn walk_on(&mut self) {
        loop {
            let Some(walk) = self.user_state.learn_walk.as_mut() else { return };
            let graph = &self.graph_state.graph;
            walk.pass_gone(|n| graph.nodes.contains_key(n));
            let Some(node) = walk.current() else {
                self.end_learn_walk();
                return;
            };
            let taken = walk.taken().to_vec();
            match learn_walk::learn_start(graph, &self.user_state, node) {
                Some(SynthResponse::MidiLearnStart { engine_node_id, param_index, param_name, min_value, max_value, control, scene, into }) => {
                    let target = MidiLearnTarget {
                        node_id: engine_node_id,
                        param_index,
                        param_name,
                        min_value,
                        max_value,
                        control,
                        scene,
                        into: into.map(|into| *into),
                    };
                    self.midi.start_learn_past(target, taken);
                    self.show_midi_learn();
                    return;
                }
                // Nothing it can learn into (not in the engine yet): on past it
                _ => {
                    if let Some(walk) = self.user_state.learn_walk.as_mut() {
                        walk.skip();
                    }
                }
            }
        }
    }

    /// Writes what the learns into modules heard this frame, then follows
    /// the walk.
    pub(super) fn learned_into_modules(&mut self, learned_into: &[(u64, usize, f32)]) {
        for &(node_id, param_index, value) in learned_into {
            self.learn_into_module(node_id, param_index, value);
        }
        self.follow_learn_walk(learned_into);
    }

    /// Follows the walk after the MIDI heard this frame, whose learns into
    /// modules wrote `learned_into`: the module waiting took a hit, so on
    /// to the next; or its learn was cancelled, or another took its place,
    /// so the walk stops.
    fn follow_learn_walk(&mut self, learned_into: &[(u64, usize, f32)]) {
        let Some(current) = self.user_state.learn_walk.as_ref().and_then(LearnWalk::current) else {
            self.end_learn_walk();
            return;
        };
        let engine_id = self.user_state.get_engine_node_id(current);
        let writes: Vec<(usize, f32)> =
            learned_into.iter().filter(|&&(node, ..)| Some(node) == engine_id).map(|&(_, param, value)| (param, value)).collect();
        if let (false, Some(walk)) = (writes.is_empty(), self.user_state.learn_walk.as_mut()) {
            walk.take(writes);
            self.walk_on();
            return;
        }
        let waiting = self.midi.learning().is_some_and(|target| Some(target.node_id) == engine_id && target.into.is_some());
        if !waiting {
            self.end_learn_walk();
        }
    }

    /// Ends the walk, if one is going, keeping what it learned: its learn
    /// stops, and the walk becomes one undo step.
    pub(super) fn end_learn_walk(&mut self) {
        let Some(walk) = self.user_state.learn_walk.take() else { return };
        let engine_id = walk.current().and_then(|n| self.user_state.get_engine_node_id(n));
        if self.midi.learning().is_some_and(|target| Some(target.node_id) == engine_id) {
            self.midi.cancel_learn();
            self.show_midi_learn();
        }
        let (learned, (_, of)) = (walk.learned(), walk.progress());
        let name = &walk.name;
        self.status_message = Some(match (walk.is_over(), learned) {
            (true, n) if n == of => format!("Learned all {of} in {name}, in order"),
            (_, 0) => format!("Stopped learning {name} in order: nothing learned"),
            (true, n) => format!("Learned {name} in order: {n} of {of}, the rest skipped"),
            (false, n) => format!("Stopped learning {name} in order: {n} of {of} learned"),
        });
        if learned > 0 {
            self.history.name_next(format!("Learn {name} in order"));
        }
        self.history.record(&self.graph_state, &self.user_state, false, Instant::now());
    }

    /// Tab skips the module waiting and Escape stops, while a walk goes.
    /// Before the canvas's own Tab and Escape, which would open the palette
    /// or the group, or leave it.
    pub(super) fn walk_keys(&mut self, ctx: &egui::Context) {
        if self.user_state.learn_walk.is_none() {
            return;
        }
        use egui::{Key, Modifiers};
        let (skip, stop) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Tab), i.consume_key(Modifiers::NONE, Key::Escape)));
        if stop {
            self.end_learn_walk();
        } else if skip {
            self.learn_walk(WalkAction::Skip);
        }
    }

    /// What the status bar says while a walk goes: how far it has come,
    /// what it waits for, and the keys.
    pub(super) fn walk_status(&self) -> Option<String> {
        let walk = self.user_state.learn_walk.as_ref()?;
        let (at, of) = walk.progress();
        let target = self.midi.learning()?;
        let ask = target.into.as_ref().map_or_else(String::new, |into| into.prompt());
        Some(format!("Learning {} in order, {} of {of} · {ask} for {} · Tab skips, Esc stops", walk.name, at + 1, target.param_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MidiEvent;
    use crate::graph::learn_walk::Mark;
    use crate::graph::{groups, pad_listen, SynthNodeTemplate};
    use egui_node_graph2::NodeId;

    /// An app holding eight Pads in a group named Pads, two rows of four,
    /// added in no particular order: the group's node, and the Pads in the
    /// order they sit.
    fn pads_x8() -> (SynthApp, NodeId, Vec<NodeId>) {
        let mut app = SynthApp::new();
        let template = SynthNodeTemplate::from_module_id(crate::modules::pad::ID).unwrap();
        let mut placed: Vec<(usize, NodeId)> = [5, 2, 7, 0, 3, 6, 1, 4]
            .into_iter()
            .map(|n| {
                let at = egui::pos2((n % 4) as f32 * 250.0, (n / 4) as f32 * 290.0 + (n % 3) as f32 * 9.0);
                (n, editing::add_module(&mut app.graph_state, &mut app.user_state, template, at).0)
            })
            .collect();
        placed.sort();
        let pads: Vec<NodeId> = placed.into_iter().map(|(_, node)| node).collect();
        app.group_selection(&egui::Context::default(), &pads);
        app.user_state.renaming = None;
        let group = app.graph_state.selected_nodes[0];
        groups::rename(&mut app.graph_state.graph, group, "Pads");
        app.prepare_level();
        // The frame that made it ends
        app.history.record(&app.graph_state, &app.user_state, false, Instant::now());
        (app, group, pads)
    }

    /// A pad hit on the controller, heard as the app hears a frame's MIDI.
    fn hit(app: &mut SynthApp, channel: u8, note: u8) {
        let mut heard = crate::app::midi_session::Heard::default();
        app.midi.hear(MidiEvent::NoteOn { channel, note, velocity: 100 }, &mut heard, &|_, _| None);
        app.learned_into_modules(&heard.learned_into);
    }

    fn notes(app: &SynthApp, pads: &[NodeId]) -> Vec<(u8, u8)> {
        pads.iter().map(|&pad| pad_listen(&app.graph_state.graph, pad)).map(|l| (l.note, l.channel)).collect()
    }

    #[test]
    fn a_group_of_eight_learns_eight_pads_in_one_walk_and_one_undo() {
        let (mut app, group, pads) = pads_x8();
        assert_eq!(app.user_state.group_faces.values().next().map(|face| face.learnable), Some(8));
        let before = notes(&app, &pads);

        app.learn_walk(WalkAction::Start(group));
        let walk = app.user_state.learn_walk.as_ref().unwrap();
        assert_eq!(walk.mark(pads[0]), Some(Mark::Now), "the top left Pad first");
        assert_eq!(walk.mark(pads[4]), Some(Mark::Next(5)), "the second row after the first");
        let status = app.walk_status().unwrap();
        assert!(status.starts_with("Learning Pads in order, 1 of 8 · Hit a pad or key for Pad"), "{status}");

        // A pad on channel 3 for each, the one before bouncing first
        for n in 0..8u8 {
            if n > 0 {
                hit(&mut app, 2, 47 + n);
            }
            hit(&mut app, 2, 48 + n);
        }
        assert!(app.user_state.learn_walk.is_none(), "over once the last is learned");
        assert!(!app.is_midi_learning());
        assert_eq!(notes(&app, &pads), (48..56).map(|note| (note, 3)).collect::<Vec<_>>());
        assert_eq!(app.status_message.as_deref(), Some("Learned all 8 in Pads, in order"));

        // One step takes it all back
        assert_eq!(app.history.undo_label(), Some("Learn Pads in order"));
        app.undo();
        assert_eq!(notes(&app, &pads), before);
    }

    #[test]
    fn tab_skips_one_and_escape_stops_keeping_what_was_learned() {
        let (mut app, group, pads) = pads_x8();
        let before = notes(&app, &pads);
        app.learn_walk(WalkAction::Start(group));
        app.learn_walk(WalkAction::Skip);
        hit(&mut app, 9, 36);
        let walk = app.user_state.learn_walk.as_ref().unwrap();
        assert_eq!(walk.mark(pads[0]), Some(Mark::Skipped));
        assert_eq!(walk.mark(pads[1]), Some(Mark::Learned));
        assert_eq!(walk.mark(pads[2]), Some(Mark::Now));

        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        ctx.run_ui(input, |ui| app.walk_keys(ui.ctx())).textures_delta.clear();
        assert!(app.user_state.learn_walk.is_none());
        assert!(!app.is_midi_learning());
        assert_eq!(app.status_message.as_deref(), Some("Stopped learning Pads in order: 1 of 8 learned"));
        let after = notes(&app, &pads);
        assert_eq!(after[0], before[0], "the skipped one keeps its note");
        assert_eq!(after[1], (36, 10));
        assert_eq!(after[2..], before[2..]);
    }

    #[test]
    fn duplicating_the_pad_on_40_gives_one_on_41() {
        let mut app = SynthApp::new();
        let template = SynthNodeTemplate::from_module_id(crate::modules::pad::ID).unwrap();
        let (pad, _) = editing::add_module(&mut app.graph_state, &mut app.user_state, template, egui::pos2(0.0, 0.0));
        let note = app.graph_state.graph[pad].get_input("Note").unwrap();
        app.graph_state.graph.inputs[note].value.set_actual_value(40.0);
        app.duplicate_selection(&editing::Selection::modules(&[pad]));
        let copy = app.graph_state.selected_nodes[0];
        assert_ne!(copy, pad);
        assert_eq!(pad_listen(&app.graph_state.graph, copy).note, 41);
        // ...and a fresh one beside them, the next
        let (fresh, _) = editing::add_module(&mut app.graph_state, &mut app.user_state, template, egui::pos2(0.0, 300.0));
        assert_eq!(pad_listen(&app.graph_state.graph, fresh).note, 42);
    }

    #[test]
    fn a_learn_cancelled_or_started_elsewhere_stops_the_walk() {
        let (mut app, group, _) = pads_x8();
        app.learn_walk(WalkAction::Start(group));
        app.cancel_midi_learn();
        app.learned_into_modules(&[]);
        assert!(app.user_state.learn_walk.is_none());

        // A group without Pads has nothing to walk
        let osc = SynthNodeTemplate::from_module_id(crate::modules::oscillator::ID).unwrap();
        let (osc, _) = editing::add_module(&mut app.graph_state, &mut app.user_state, osc, egui::pos2(0.0, 900.0));
        app.group_selection(&egui::Context::default(), &[osc]);
        let other = app.graph_state.selected_nodes[0];
        app.learn_walk(WalkAction::Start(other));
        assert!(app.user_state.learn_walk.is_none());
        assert!(app.status_message.as_deref().unwrap().contains("has no Pads or Controls"));
    }

    #[test]
    fn a_group_of_controls_learns_each_fader_moved_in_turn() {
        // Four Controls in a row (#483): move four faders, each one a while
        let mut app = SynthApp::new();
        let template = SynthNodeTemplate::from_module_id(crate::modules::control::ID).unwrap();
        let controls: Vec<NodeId> =
            (0..4).map(|n| editing::add_module(&mut app.graph_state, &mut app.user_state, template, egui::pos2(n as f32 * 210.0, 0.0)).0).collect();
        app.group_selection(&egui::Context::default(), &controls);
        app.user_state.renaming = None;
        let group = app.graph_state.selected_nodes[0];
        groups::rename(&mut app.graph_state.graph, group, "Faders");
        app.prepare_level();
        app.history.record(&app.graph_state, &app.user_state, false, Instant::now());

        app.learn_walk(WalkAction::Start(group));
        let status = app.walk_status().unwrap();
        assert!(status.starts_with("Learning Faders in order, 1 of 4 · Move a knob or fader"), "{status}");
        let frame = |app: &mut SynthApp, events: &[(u8, u8)]| {
            let mut heard = crate::app::midi_session::Heard::default();
            for &(controller, value) in events {
                app.midi.hear(MidiEvent::ControlChange { channel: 0, controller, value }, &mut heard, &|_, _| None);
            }
            app.learned_into_modules(&heard.learned_into);
        };
        // Each fader is still moving as the next Control waits
        for (n, cc) in [82u8, 83, 85, 17].into_iter().enumerate() {
            frame(&mut app, &[(cc, 10), (cc, 12)]);
            frame(&mut app, &[(cc, 14), (cc, 16)]);
            assert_eq!(app.user_state.learn_walk.as_ref().map(|walk| walk.progress().0), (n < 3).then_some(n + 1));
        }
        let heard: Vec<u8> = controls.iter().map(|&n| crate::graph::control_listen(&app.graph_state.graph, n).cc).collect();
        assert_eq!(heard, [82, 83, 85, 17]);
        assert_eq!(app.status_message.as_deref(), Some("Learned all 4 in Faders, in order"));
        assert_eq!(app.history.undo_label(), Some("Learn Faders in order"));
    }
}
