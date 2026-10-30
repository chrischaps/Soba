//! Pad lights (#430): a controller's pads glow in the colour of what they
//! play in the patch, so the hardware says what each pad does.
//!
//! - A pad something listens to is lit in its target's colour, dim at
//!   rest: a Pad in the colour of the module its Gate is cabled into (a
//!   Drum's blue, an Envelope's orange), or Gate green with nothing on it;
//!   a pad mapped to a knob in its module's colour.
//! - A hit flashes it, brighter the harder it was struck, and the flash
//!   fades as the Pad's face does.
//! - A latched or held pad, and a Scenes pad's scene while it's the
//!   current one, glows steadily.
//! - A pad nothing listens to is dark.
//!
//! What to send each pad is [`Lights`]' (the controller's file); this
//! works out each pad's colour and which have changed. A port sends no
//! more often than [`GAP`], and every pad is sent again each [`REFRESH`],
//! so a controller unplugged and back, or switched to another mode, comes
//! back lit. Kept apart from the frame loop, with no egui context, so it
//! can be tested; the frame's side is `synth_app::pad_lights`.

use std::collections::HashMap;

use eframe::egui::{ecolor::Hsva, Color32};

use crate::engine::device_names::same;
use crate::hardware::lights::{LitPad, Lights};

/// How bright a pad is at rest, of its brightest.
pub const REST: f32 = 0.08;
/// How bright a pad held open (latched, held down, the current scene) is.
pub const OPEN: f32 = 0.45;
/// How long a hit's flash takes to fade, in seconds: its time constant,
/// as the Pad's face has it.
pub const FLASH: f32 = 0.12;
/// The shortest time between two sends to one port, in seconds: a flash
/// fades in steps of this, and a busy patch can't flood the controller.
pub const GAP: f64 = 1.0 / 30.0;
/// How often every pad is sent again, lit or not, in seconds.
pub const REFRESH: f64 = 2.0;
/// How long the test's rainbow runs, in seconds.
pub const TEST_SECONDS: f64 = 4.0;

/// Something in the patch that a pad plays: a Pad module, or a mapping
/// learned from a pad.
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    /// The device it hears, by name, or `None` for any.
    pub device: Option<String>,
    /// The channel it hears (0-15), or `None` for any.
    pub channel: Option<u8>,
    /// The note it hears, or `None` for any (a Pad on Any Note).
    pub note: Option<u8>,
    /// What it plays, as its colour.
    pub colour: Color32,
    /// Held open: latched, held down, the current scene.
    pub open: bool,
    /// How much of a hit's flash is left, 0 to 1.
    pub flash: f32,
}

impl Player {
    /// Does pad `pad` of the controller called `device` play it?
    pub fn played_by(&self, device: &str, pad: LitPad) -> bool {
        self.device.as_deref().is_none_or(|heard| same(heard, device) || crate::hardware::lights::of_device(heard).is_some_and(|l| l.is(device)))
            && self.channel.is_none_or(|channel| channel + 1 == pad.channel)
            && self.note.is_none_or(|note| note == pad.note)
    }

    /// How bright it lights its pad, 0 to 1.
    pub fn level(&self) -> f32 {
        let open = if self.open { OPEN } else { 0.0 };
        REST.max(open).max(self.flash.clamp(0.0, 1.0))
    }
}

/// How much of a hit of `velocity` (0 to 1) is left `quiet` seconds after
/// it, as the Pad's face flashes.
pub fn flash(velocity: f32, quiet: f32) -> f32 {
    velocity.max(0.0).powf(0.8) * (-quiet.max(0.0) / FLASH).exp()
}

/// `colour` at `level`, as a pad's red, green and blue, each 0 to `max`.
/// A colour lit at all keeps a little of each of its channels, so a dim
/// orange stays orange rather than rounding to red.
pub fn glow(colour: Color32, level: f32, max: u8) -> [u8; 3] {
    let level = level.clamp(0.0, 1.0);
    [colour.r(), colour.g(), colour.b()].map(|c| {
        let lit = c as f32 / 255.0 * level * max as f32;
        if c > 0 && level > 0.0 { (lit.round() as u8).max(1) } else { 0 }
    })
}

/// Each pad's colour on the controller called `device`: the brightest of
/// what it plays, or dark.
pub fn targets(lights: &Lights, device: &str, players: &[Player]) -> Vec<[u8; 3]> {
    lights
        .pads
        .iter()
        .map(|&pad| {
            players
                .iter()
                .filter(|player| player.played_by(device, pad))
                .max_by(|a, b| a.level().total_cmp(&b.level()))
                .map_or([0; 3], |player| glow(player.colour, player.level(), lights.max))
        })
        .collect()
}

/// The test: a rainbow turning across the pads, `t` seconds in, so the
/// lights can be checked by eye with nothing in the patch.
pub fn rainbow(lights: &Lights, t: f64) -> Vec<[u8; 3]> {
    let count = lights.pads.len().max(1) as f32;
    (0..lights.pads.len())
        .map(|i| {
            let hue = (i as f32 / count.min(8.0) + t as f32 * 0.25).fract();
            glow(Color32::from(Hsva::new(hue, 1.0, 1.0, 1.0)), 0.8, lights.max)
        })
        .collect()
}

/// What one port's controller was last sent.
#[derive(Debug, Clone)]
struct Lit {
    device: String,
    /// Each pad's colour as sent, or `None` until it is.
    sent: Vec<Option<[u8; 3]>>,
    /// When it was last sent anything, and when every pad.
    at: f64,
    whole_at: f64,
    /// Whether its `enter` messages have gone.
    entered: bool,
}

/// What each port's lights were sent, and the test running on one.
#[derive(Debug, Default)]
pub struct PadLights {
    ports: HashMap<usize, Lit>,
    /// The port running the test, and when it began.
    test: Option<(usize, f64)>,
}

/// What a port is to be sent now.
#[derive(Debug, Default, PartialEq)]
pub struct Due {
    pub messages: Vec<Vec<u8>>,
    /// A change it's holding back until [`GAP`] has passed.
    pub waiting: bool,
}

impl PadLights {
    /// What port `port`, playing `device`, is to be sent at `now` for its
    /// pads to show `targets`: the pads that changed, or every pad once
    /// [`REFRESH`] has passed. A device the port didn't play before
    /// starts afresh.
    pub fn due(&mut self, port: usize, device: &str, lights: &Lights, now: f64, targets: &[[u8; 3]]) -> Due {
        let fresh = || Lit { device: device.to_string(), sent: vec![None; lights.pads.len()], at: f64::NEG_INFINITY, whole_at: now, entered: false };
        let lit = self.ports.entry(port).or_insert_with(fresh);
        if lit.device != device || lit.sent.len() != lights.pads.len() {
            *lit = fresh();
        }
        let changed = |lit: &Lit| lit.sent.iter().zip(targets).any(|(sent, target)| *sent != Some(*target));
        if now - lit.at < GAP {
            return Due { messages: Vec::new(), waiting: changed(lit) };
        }
        if now - lit.whole_at >= REFRESH {
            lit.sent.fill(None);
            lit.whole_at = now;
        }
        let mut messages = Vec::new();
        if !lit.entered {
            messages.extend(lights.enter.iter().cloned());
            lit.entered = true;
        }
        for ((pad, sent), &target) in lights.pads.iter().zip(&mut lit.sent).zip(targets) {
            if *sent != Some(target) {
                messages.push(lights.message(pad.id, target));
                *sent = Some(target);
            }
        }
        if !messages.is_empty() {
            lit.at = now;
        }
        Due { messages, waiting: false }
    }

    /// Forgets what port `port` was sent, as when a send failed: it's all
    /// sent again.
    pub fn unsent(&mut self, port: usize) {
        self.ports.remove(&port);
    }

    /// Keeps only the ports `lit` says still play a controller with lights.
    pub fn keep(&mut self, lit: impl Fn(usize) -> bool) {
        self.ports.retain(|&port, _| lit(port));
        if self.test.is_some_and(|(port, _)| !lit(port)) {
            self.test = None;
        }
    }

    /// Runs the test's rainbow on port `port` from `now`.
    pub fn start_test(&mut self, port: usize, now: f64) {
        self.test = Some((port, now));
    }

    /// How far into the test port `port` is, in seconds, while it runs.
    pub fn testing(&mut self, port: usize, now: f64) -> Option<f64> {
        let (testing, from) = self.test?;
        if now - from >= TEST_SECONDS {
            self.test = None;
            return None;
        }
        (testing == port).then_some(now - from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::ModuleCategory;
    use crate::hardware::lights::of_device;

    const MINILAB: &str = "Minilab3 MIDI";

    fn minilab() -> &'static Lights {
        of_device(MINILAB).unwrap()
    }

    /// A player on pad `note`, channel 10, of any device.
    fn on(note: u8, colour: Color32) -> Player {
        Player { device: None, channel: Some(9), note: Some(note), colour, open: false, flash: 0.0 }
    }

    #[test]
    fn a_pad_glows_dim_in_its_targets_colour_and_the_rest_are_dark() {
        let drum = ModuleCategory::Source.color();
        let targets = targets(minilab(), MINILAB, &[on(37, drum)]);
        assert_eq!(targets[0], [0; 3], "pad 1: nothing plays it");
        assert_eq!(targets[1], glow(drum, REST, 127), "pad 2: a drum's blue, dim");
        let [r, g, b] = targets[1];
        assert!(b > g && g > r && b <= 12, "blue, and dim: {:?}", targets[1]);
        assert!(targets[2..].iter().all(|t| *t == [0; 3]));
    }

    #[test]
    fn a_hit_flashes_and_a_latch_stays_lit() {
        let gate = crate::app::theme::signal::GATE;
        let mut hit = on(36, gate);
        hit.flash = flash(1.0, 0.0);
        let mut latched = on(37, gate);
        latched.open = true;
        let mut faded = on(38, gate);
        faded.flash = flash(1.0, 1.0);
        let targets = targets(minilab(), MINILAB, &[hit, latched, faded]);
        let brightness = |rgb: [u8; 3]| rgb.iter().map(|&c| c as u32).sum::<u32>();
        assert!(brightness(targets[0]) > brightness(targets[1]), "a hit is brighter than a latch");
        assert!(brightness(targets[1]) > brightness(targets[2]), "a latch is brighter than at rest");
        assert_eq!(targets[2], glow(gate, REST, 127), "a second on, the flash is gone");
    }

    #[test]
    fn the_brightest_of_two_on_one_pad_wins() {
        let mut scene = on(36, ModuleCategory::Modulation.color());
        scene.open = true;
        let knob = on(36, ModuleCategory::Filter.color());
        assert_eq!(targets(minilab(), MINILAB, &[knob, scene.clone()])[0], glow(scene.colour, OPEN, 127));
    }

    #[test]
    fn a_player_hears_its_device_channel_and_note() {
        let pad = minilab().pads[0];
        let mut player = on(36, Color32::WHITE);
        assert!(player.played_by(MINILAB, pad));
        player.device = Some("Digitakt".into());
        assert!(!player.played_by(MINILAB, pad), "another device's");
        player.device = Some("Minilab3 MIDI".into());
        assert!(player.played_by("MIDIOUT2 (Minilab3 MIDI)", pad), "heard on one MiniLab port, lit on its other");
        player.channel = Some(0);
        assert!(!player.played_by(MINILAB, pad), "channel 1 isn't the pads' 10");
        player.channel = None;
        player.note = None;
        assert!(player.played_by(MINILAB, pad) && player.played_by(MINILAB, minilab().pads[15]), "any note, any channel: every pad");
    }

    #[test]
    fn a_dim_colour_keeps_each_of_its_channels() {
        assert_eq!(glow(Color32::from_rgb(255, 183, 77), 0.01, 127), [1, 1, 1]);
        assert_eq!(glow(Color32::from_rgb(255, 0, 0), 0.01, 127), [1, 0, 0]);
        assert_eq!(glow(Color32::WHITE, 0.0, 127), [0; 3]);
        assert_eq!(glow(Color32::WHITE, 1.0, 127), [127; 3]);
    }

    #[test]
    fn only_what_changed_is_sent_and_no_faster_than_the_gap() {
        let lights = minilab();
        let mut pad_lights = PadLights::default();
        let mut targets = vec![[0; 3]; 16];
        let first = pad_lights.due(0, MINILAB, lights, 0.0, &targets);
        assert_eq!(first.messages.len(), 16, "every pad, the first time");
        assert_eq!(pad_lights.due(0, MINILAB, lights, 0.5, &targets), Due::default(), "nothing changed");

        targets[3] = [0, 40, 0];
        let sent = pad_lights.due(0, MINILAB, lights, 0.6, &targets);
        assert_eq!(sent.messages, vec![lights.message(lights.pads[3].id, [0, 40, 0])]);
        targets[3] = [0, 80, 0];
        let held = pad_lights.due(0, MINILAB, lights, 0.6 + GAP / 2.0, &targets);
        assert!(held.messages.is_empty() && held.waiting, "held back, and says so");
        assert_eq!(pad_lights.due(0, MINILAB, lights, 0.6 + GAP * 1.01, &targets).messages.len(), 1);
    }

    #[test]
    fn every_pad_is_sent_again_after_the_refresh_and_on_a_new_device() {
        let lights = minilab();
        let mut pad_lights = PadLights::default();
        let targets = vec![[5; 3]; 16];
        pad_lights.due(0, MINILAB, lights, 0.0, &targets);
        assert!(pad_lights.due(0, MINILAB, lights, REFRESH - 0.1, &targets).messages.is_empty());
        assert_eq!(pad_lights.due(0, MINILAB, lights, REFRESH, &targets).messages.len(), 16, "refreshed");
        assert_eq!(pad_lights.due(0, "MIDIOUT2 (Minilab3 MIDI)", lights, REFRESH + 1.0, &targets).messages.len(), 16, "another port of it");
        pad_lights.unsent(0);
        assert_eq!(pad_lights.due(0, "MIDIOUT2 (Minilab3 MIDI)", lights, REFRESH + 1.01, &targets).messages.len(), 16, "a failed send is sent again");
    }

    #[test]
    fn the_test_runs_its_rainbow_then_stops() {
        let mut pad_lights = PadLights::default();
        pad_lights.start_test(2, 10.0);
        assert_eq!(pad_lights.testing(1, 11.0), None, "another port");
        assert_eq!(pad_lights.testing(2, 11.0), Some(1.0));
        assert_eq!(pad_lights.testing(2, 10.0 + TEST_SECONDS), None, "over");
        let rainbow = rainbow(minilab(), 0.0);
        assert_eq!(rainbow.len(), 16);
        assert_ne!(rainbow[0], rainbow[4], "each pad its own hue");
        assert_eq!(rainbow[0], rainbow[8], "bank B as bank A");
    }
}
