//! Map Mode (#426), the app's side: Ctrl+L and the MIDI In menu turn it on
//! and off, Escape leaves it, and the status bar says what it waits for.
//! The canvas's side, the outlines and the catches over the controls, is
//! `crate::graph::map_mode`.
//!
//! A click on a control starts MIDI Learn on it, as its right-click menu's
//! Learn does. Once something on the controller moves, the control is
//! mapped, and Map Mode stays on for the next one.

use super::*;

impl SynthApp {
    /// Is Map Mode on?
    pub(super) fn map_mode_on(&self) -> bool {
        self.user_state.map_mode.on
    }

    /// Turns Map Mode on, or off.
    pub(super) fn toggle_map_mode(&mut self) {
        if self.map_mode_on() {
            self.leave_map_mode();
        } else {
            self.user_state.map_mode.on = true;
            self.user_state.map_mode.mapped = 0;
        }
    }

    /// Turns Map Mode off, and with it any MIDI Learn it was waiting on.
    pub(super) fn leave_map_mode(&mut self) {
        self.user_state.map_mode.on = false;
        if self.is_midi_learning() {
            self.midi.cancel_learn();
            self.show_midi_learn();
        }
        self.status_message = Some(match self.user_state.map_mode.mapped {
            0 => "Map Mode off".to_string(),
            1 => "Map Mode off: 1 control mapped".to_string(),
            n => format!("Map Mode off: {n} controls mapped"),
        });
    }

    /// Escape on the canvas: leaves Map Mode, or cancels MIDI Learn.
    pub(super) fn escape_midi_learn(&mut self, ctx: &egui::Context) {
        if !(self.map_mode_on() || self.is_midi_learning()) {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.map_mode_on() {
                self.leave_map_mode();
            } else {
                self.cancel_midi_learn();
            }
        }
    }

    /// A control was mapped: `was_learning` says MIDI Learn was waiting,
    /// rather than an encoder's kind coming clear after.
    pub(super) fn note_map_mode_learned(&mut self, was_learning: bool) {
        if self.map_mode_on() && was_learning && !self.is_midi_learning() {
            self.user_state.map_mode.mapped += 1;
        }
    }

    /// What the status bar says in Map Mode: what it waits for, and what
    /// was mapped last.
    pub(super) fn map_mode_status(&self, ctx: &egui::Context) -> String {
        let leave = shortcuts::keys(ctx, "Esc or Ctrl+L leaves");
        if let Some(target) = self.midi.learning() {
            let module = self.module_name(target.node_id);
            let what = match module {
                Some(module) => format!("{} on {module}", target.param_name),
                None => target.param_name.clone(),
            };
            return format!("Map Mode · move a knob or fader, or hit a pad, for {what} · {leave}");
        }
        let last = (self.user_state.map_mode.mapped > 0)
            .then(|| self.midi.mappings.as_slice().last())
            .flatten()
            .map(|m| format!("{} mapped to {} · ", crate::app::midi_mappings::info(m).text(), m.param_name));
        format!("Map Mode · {}click a control to map it · {leave}", last.unwrap_or_default())
    }

    /// The display name of the module an engine node is, if it's in the patch.
    pub(super) fn module_name(&self, engine_node_id: u64) -> Option<&str> {
        let node = self.user_state.graph_node_id(engine_node_id)?;
        self.graph_state.graph.nodes.get(node).map(|node| node.user_data.display_name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::MidiControl;

    fn cutoff(node_id: u64) -> MidiLearnTarget {
        MidiLearnTarget { node_id, param_index: 0, param_name: "Cutoff".into(), min_value: 20.0, max_value: 20000.0, control: MidiControl::Knob, scene: None, into: None }
    }

    #[test]
    fn map_mode_stays_on_from_one_control_to_the_next_until_it_is_left() {
        let mut app = SynthApp::new();
        app.toggle_map_mode();
        assert_eq!(app.status_bar_shows(), status_bar::Shown::MapMode);

        // A click on a control starts MIDI Learn on it, as its menu's Learn does
        app.start_midi_learn(cutoff(7));
        let ctx = egui::Context::default();
        assert!(app.map_mode_status(&ctx).contains("for Cutoff"));
        let mut heard = crate::app::midi_session::Heard::default();
        let event = crate::engine::MidiEvent::ControlChange { channel: 0, controller: 74, value: 10 };
        app.midi.hear(event, &mut heard, &|_, _| None);
        assert!(heard.learned);
        app.note_map_mode_learned(true);
        assert!(app.map_mode_on(), "it waits for the next control");
        assert!(app.map_mode_status(&ctx).contains("CC 74 · ch 1 mapped to Cutoff"));

        // Leaving lets go of a learn it was waiting on
        app.start_midi_learn(cutoff(8));
        app.leave_map_mode();
        assert!(!app.map_mode_on());
        assert!(!app.is_midi_learning());
        assert_eq!(app.status_message.as_deref(), Some("Map Mode off: 1 control mapped"));
    }

    #[test]
    fn a_range_set_in_the_controller_map_is_a_change_to_save() {
        let mut app = SynthApp::new();
        app.midi.mappings.learn(crate::persistence::MidiMapping::new(74, 1, 7, 0, "Cutoff", 20.0, 20000.0));
        app.midi.mark_saved();
        assert!(!app.midi.unsaved());
        assert!(app.midi.mappings.set_range(7, 0, (200.0, 8000.0), (20.0, 20000.0)));
        assert!(app.midi.unsaved());
    }
}
