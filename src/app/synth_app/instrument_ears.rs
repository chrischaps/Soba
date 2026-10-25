//! Instruments listen back (#444): turn the MicroFreak's own Cutoff, and
//! the node's Cutoff turns with it, ready to save in the patch.
//!
//! An Instrument (or a Kit) hears the MIDI In of the device on its port,
//! matched by name as the Rig matches them, and only that: the same CC on
//! the same channel from another device passes it by. A knob the device
//! turns moves as a mouse drag would, so the moves of one turn of the hand
//! are one undo step, and sends nothing back to the device, which has the
//! value already. Under a cable, the knob stays the cable's, and a faint
//! tick on its rim shows where the hardware's is.
//!
//! Each node's [`Ear`](crate::hardware::hearing::Ear) notes what its knobs
//! send, a frame at a time, so a device that echoes what it's sent doesn't
//! have the knob fight the mouse.

use std::collections::HashSet;

use crate::engine::{device_names, EngineCommand};
use crate::hardware::{self, instrument};
use crate::link::midi::MIDI;
use crate::modules::instrument::SLOTS;

use super::SynthApp;

/// Whether a CC that came in on the input called `input` (`None`: Soba's
/// own) is from the device called `output`, an Instrument's port's. Soba's
/// own count only while no input is open (`none_open`).
fn from_its_device(input: Option<&str>, output: Option<&str>, none_open: bool) -> bool {
    match (input, output) {
        (Some(input), Some(output)) => device_names::in_and_out(input, output),
        (Some(_), None) => false,
        (None, _) => none_open,
    }
}

/// A CC that came in: the input it came on, its channel (from 0), its
/// controller and its value.
pub(super) type HeardCc = (u8, u8, u8, u8);

impl SynthApp {
    /// Notes what every Instrument's knobs send now, before the MIDI that
    /// came in is heard: a value the device sends straight back is Soba's
    /// own coming back. Call once a frame.
    pub(super) fn follow_instrument_knobs(&mut self, now: f64) {
        let profiles = hardware::known();
        let mut playing = HashSet::new();
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if !instrument::plays_a_profile(node.user_data.module_id) {
                continue;
            }
            let (Some(engine_id), Some(knobs)) = (self.user_state.get_engine_node_id(node_id), crate::graph::instrument_knobs(&self.graph_state.graph, node_id, &profiles)) else {
                continue;
            };
            let ear = self.instrument_ears.entry(engine_id).or_default();
            for slot in 0..SLOTS {
                let sends = knobs.knobs[slot].filter(|knob| !knob.patched).map(|knob| knobs.maps[slot].value(knob.travel));
                ear.knob(slot, sends, now);
            }
            playing.insert(engine_id);
        }
        self.instrument_ears.retain(|engine_id, _| playing.contains(engine_id));
    }

    /// Hears the CCs that came in this frame: each from an Instrument's own
    /// device turns its knobs. With no MIDI input open, Soba's own (a
    /// capture's cues) stand in for the device, so a capture can film it.
    pub(super) fn hear_instruments(&mut self, heard: &[HeardCc], now: f64) {
        if heard.is_empty() || self.instrument_ears.is_empty() {
            return;
        }
        let profiles = hardware::known();
        let input_name = |port: u8| self.midi.engine.as_ref().and_then(|engine| engine.name_of(port));
        let none_open = !self.midi.is_connected();
        let mut sends = Vec::new();
        let mut moves = Vec::new();
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if !instrument::plays_a_profile(node.user_data.module_id) {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            let (Some(ear), Some(knobs)) = (self.instrument_ears.get_mut(&engine_id), crate::graph::instrument_knobs(&self.graph_state.graph, node_id, &profiles)) else {
                continue;
            };
            let device = MIDI.chosen(knobs.port);
            for &(port, channel, controller, value) in heard {
                if from_its_device(input_name(port), device.as_deref(), none_open) {
                    ear.hear(&knobs.maps, knobs.channel, channel, controller, value);
                }
            }
            for turned in ear.take(&knobs.maps, now) {
                let Some(knob) = knobs.knobs[turned.slot] else { continue };
                if knob.patched {
                    // The cable's knob: only a ghost of where the hardware is
                    self.user_state.hardware_ghosts.insert((engine_id, knob.param_index), (turned.travel, now));
                    continue;
                }
                ear.turned(turned.slot, turned.value);
                // The knob's told first, so the value never goes back
                sends.push(EngineCommand::SlotHeard { node_id: engine_id, slot: turned.slot, value: turned.value });
                sends.push(EngineCommand::SetParameter { node_id: engine_id, param_index: knob.param_index, value: turned.travel });
                moves.push((knob.input, engine_id, knob.param_index, turned.travel));
            }
        }
        // As a mouse drag moves it: an edit, which undo gathers into one step
        for (input, engine_id, param_index, travel) in moves {
            if let Some(input) = self.graph_state.graph.inputs.get_mut(input) {
                input.value.set_actual_value(travel);
            }
            self.cached_params.insert((engine_id, param_index), travel);
        }
        for command in sends {
            self.send_command(command);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::app::editing;
    use crate::dsp::SOBA_PORT;
    use crate::graph::SynthNodeTemplate;
    use crate::hardware::Address;
    use crate::modules::instrument::{Instrument, SLOT_NAMES};
    use web_time::Instant;

    /// An app with a MicroFreak's Instrument, and its first knob's CC.
    fn freak() -> (SynthApp, egui_node_graph2::NodeId, u8) {
        let mut app = SynthApp::with_devices(Err(crate::engine::AudioError::NoOutputDevice), None);
        let template = SynthNodeTemplate::from_module_id(crate::modules::instrument::ID).expect("registered");
        let (node_id, _) = editing::add_module(&mut app.graph_state, &mut app.user_state, template, egui::pos2(0.0, 0.0));
        let profiles = hardware::known();
        let profile = &profiles.find("MicroFreak").unwrap().profile;
        let labels = &mut app.graph_state.graph[node_id].user_data.labels;
        labels.extend(instrument::choose(profile).into_iter().filter(|(_, text)| !text.is_empty()));
        let Address::Cc(cc) = profile.parameters[0].address else { panic!("the MicroFreak's first knob is a CC") };
        app.history.reset(&app.graph_state, &app.user_state);
        (app, node_id, cc)
    }

    fn knob(app: &SynthApp, node_id: egui_node_graph2::NodeId) -> f32 {
        crate::graph::instrument_knobs(&app.graph_state.graph, node_id, &hardware::known()).unwrap().knobs[0].unwrap().travel
    }

    fn set_knob(app: &mut SynthApp, node_id: egui_node_graph2::NodeId, travel: f32) {
        let input = app.graph_state.graph[node_id].inputs.iter().find(|(name, _)| name == SLOT_NAMES[0]).unwrap().1;
        app.graph_state.graph.inputs[input].value.set_actual_value(travel);
    }

    /// A frame: the knobs noted, then `ccs` heard, at `now`.
    fn frame(app: &mut SynthApp, ccs: &[HeardCc], now: f64) {
        app.follow_instrument_knobs(now);
        app.hear_instruments(ccs, now);
    }

    #[test]
    fn the_devices_knob_turns_the_nodes_and_undoes_as_one_turn() {
        let (mut app, node_id, cc) = freak();
        let start = knob(&app, node_id);
        let map = crate::graph::instrument_knobs(&app.graph_state.graph, node_id, &hardware::known()).unwrap().maps[0];
        // Turned on the device, on the Instrument's channel (1)
        for (i, value) in [90, 95, 100].into_iter().enumerate() {
            frame(&mut app, &[(SOBA_PORT, 0, cc, value)], 1.0 + i as f64 * 0.02);
            app.history.record(&app.graph_state, &app.user_state, false, Instant::now());
        }
        assert_eq!(knob(&app, node_id), map.travel(100));
        assert_eq!(app.cached_params.get(&(app.user_state.get_engine_node_id(node_id).unwrap(), Instrument::PARAM_SLOT)), Some(&map.travel(100)), "sent, not sent again");
        // Another channel's passes it by
        frame(&mut app, &[(SOBA_PORT, 5, cc, 10)], 2.0);
        assert_eq!(knob(&app, node_id), map.travel(100));
        // One turn of the hand, one step
        assert!(app.history.undo(&mut app.graph_state, &mut app.user_state).is_some());
        assert_eq!(knob(&app, node_id), start);
        assert!(app.history.undo(&mut app.graph_state, &mut app.user_state).is_none());
    }

    #[test]
    fn an_echo_of_the_mouse_doesnt_move_the_knob() {
        let (mut app, node_id, cc) = freak();
        let map = crate::graph::instrument_knobs(&app.graph_state.graph, node_id, &hardware::known()).unwrap().maps[0];
        // The mouse turns it to 70, then 72; the device sends back 70
        set_knob(&mut app, node_id, map.travel(70));
        frame(&mut app, &[], 1.0);
        set_knob(&mut app, node_id, map.travel(72));
        frame(&mut app, &[(SOBA_PORT, 0, cc, 70)], 1.03);
        assert_eq!(knob(&app, node_id), map.travel(72));
    }

    #[test]
    fn soba_hears_an_instrument_from_its_own_device_alone() {
        assert!(from_its_device(Some("MicroFreak"), Some("MicroFreak"), false));
        assert!(from_its_device(Some("MIDIIN2 (Fantom-06)"), Some("MIDIOUT2 (Fantom-06)"), false));
        assert!(from_its_device(Some("2- Digitakt"), Some("Digitakt"), false));
        assert!(!from_its_device(Some("Arturia MiniLab 3"), Some("MicroFreak"), false), "another device on the same channel");
        assert!(!from_its_device(Some("MicroFreak"), None, false), "a port with no device");
        // Soba's own (a capture's cues) only while nothing's open
        assert!(from_its_device(None, Some("MicroFreak"), true));
        assert!(!from_its_device(None, Some("MicroFreak"), false));
    }
}
