//! Pad lights into the hardware (#430): each MIDI Out port playing a
//! controller Soba can light is sent its pads' colours by SysEx (#440), as
//! the patch has them now. What colour each pad is, and which to send, is
//! [`crate::app::pad_lights`]; what each pad plays,
//! [`crate::graph::pad_players`].
//!
//! The lights follow the port: choose the MiniLab as a port's device in the
//! Rig and its pads light, choose None and they stop. A port with SysEx
//! still waiting to go (a dump pacing itself) is passed over, so the
//! lights never queue behind one.

use eframe::egui;

use crate::app::pad_lights::{self, GAP};
use crate::graph::pad_players;
use crate::hardware::lights::{self, Lights};
use crate::link::midi::{MIDI, PORTS};

use super::SynthApp;

/// SysEx bytes a port may have waiting and still be sent lights: about a
/// frame's worth of pads.
const BACKLOG: u32 = 256;

impl SynthApp {
    /// Sends each controller's pads what changed of their colours.
    pub(super) fn sync_pad_lights(&mut self, ctx: &egui::Context) {
        // In the browser, SysEx waits until it's allowed (see the Rig)
        #[cfg(target_arch = "wasm32")]
        if !matches!(crate::engine::web_midi::sysex_access(), crate::engine::web_midi::SysexAccess::Allowed) {
            return;
        }
        let found = MIDI.found();
        let lit: Vec<(usize, String, &'static Lights)> = (0..PORTS)
            .filter_map(|port| {
                let device = found.get(port)?.clone()?;
                let lights = lights::of_device(&device)?;
                Some((port, device, lights))
            })
            .collect();
        self.pad_lights.keep(|port| lit.iter().any(|(lit, ..)| *lit == port));
        if lit.is_empty() {
            return;
        }

        let now = ctx.input(|i| i.time);
        let players = pad_players::players(&self.graph_state.graph, &self.user_state, self.midi.mappings.as_slice(), self.midi.held());
        let (mut moving, mut waiting) = (players.iter().any(|p| p.flash > 0.01), false);
        for (port, device, lights) in lit {
            let testing = self.pad_lights.testing(port, now);
            moving |= testing.is_some();
            if MIDI.traffic(port).sysex_waiting > BACKLOG {
                waiting = true;
                continue;
            }
            let targets = match testing {
                Some(t) => pad_lights::rainbow(lights, t),
                None => pad_lights::targets(lights, &device, &players),
            };
            let due = self.pad_lights.due(port, &device, lights, now, &targets);
            waiting |= due.waiting;
            for message in due.messages {
                // Nothing sends yet (the plugin, or MIDI out not begun): try again later
                if MIDI.send_sysex(port, &message).is_err() {
                    self.pad_lights.unsent(port);
                    break;
                }
            }
        }
        if moving {
            crate::app::pace::animate(ctx);
        } else if waiting {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(GAP));
        }
    }

    /// The Rig's test: a rainbow across port `port`'s pads for a few
    /// seconds, to see the lights work with nothing in the patch.
    pub(super) fn test_pad_lights(&mut self, ctx: &egui::Context, port: usize) {
        let now = ctx.input(|i| i.time);
        self.pad_lights.start_test(port, now);
        let device = MIDI.found().get(port).cloned().flatten().unwrap_or_default();
        let unchecked = lights::of_device(&device).is_some_and(|l| !l.confirmed);
        let ask = if unchecked { ": no rainbow means its colour message needs checking (see the MiniLab 3 page)" } else { "" };
        self.status_message = Some(format!("Lighting {device}'s pads in a rainbow{ask}"));
        crate::app::pace::animate(ctx);
    }
}
