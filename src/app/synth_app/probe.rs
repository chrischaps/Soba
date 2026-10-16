//! The cable probe (#296): hold Alt over a cable to hear what it carries,
//! with its waveform beside the pointer. The cable spotlight shows *where*
//! a signal goes; the probe lets you hear *what* it is.
//!
//! This side picks the cable, follows it through any groups to the module
//! output it really carries, and tells the engine (see
//! [`crate::engine::probe`]), which plays it beside the patch. The scope
//! it draws reads what the engine heard.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
use egui_node_graph2::InputId;

use crate::app::theme;
use crate::dsp::SignalType;
use crate::engine::{NodeId as EngineNodeId, Probe, ProbeDuck, ProbeScope, SCOPE_WINDOW};
use crate::graph::groups::{self, GroupIndex};
use crate::widgets::{oscilloscope_display, OscilloscopeConfig};
use super::SynthApp;

/// How much of the window an audio probe's scope shows, from a rising zero
/// crossing so a steady tone stands still: about 20 ms.
const AUDIO_SPAN: usize = SCOPE_WINDOW / 2;

/// The scope's size, and how far it sits from the pointer.
const SCOPE_SIZE: Vec2 = Vec2::new(176.0, 72.0);
const FROM_POINTER: Vec2 = Vec2::new(18.0, 20.0);

/// Where the ducking is kept between launches.
const DUCK_AUDIO_KEY: &str = "probe_duck_audio";
const DUCK_SIGNAL_KEY: &str = "probe_duck_signal";

/// The deepest the ducking sliders go, in dB. All the way there, the
/// patch is silenced and the cable soloed.
const DUCK_DB_RANGE: f32 = 40.0;

/// The cable being probed.
pub(super) struct CableProbe {
    /// Where the engine copies what it hears. Kept for as long as the app
    /// runs, so the audio thread never frees it.
    scope: Arc<ProbeScope>,
    /// The cable heard: the jack it plugs into, and what it carries.
    heard: Option<(InputId, Carried)>,
    /// How many samples the scope had kept when this cable's probe began:
    /// those are another cable's, so they aren't drawn.
    since: usize,
    /// The scope's samples, read each frame.
    window: Box<[f32; SCOPE_WINDOW]>,
    /// How far the patch is turned down beneath it, set in the Cables menu,
    /// and what the engine was last told.
    duck: ProbeDuck,
    told: ProbeDuck,
}

impl Default for CableProbe {
    fn default() -> Self {
        Self {
            scope: Arc::new(ProbeScope::new()),
            heard: None,
            since: 0,
            window: Box::new([0.0; SCOPE_WINDOW]),
            duck: ProbeDuck::default(),
            told: ProbeDuck::default(),
        }
    }
}

/// What a cable carries: a module's output, as the engine knows it.
#[derive(Clone, PartialEq)]
struct Carried {
    node: EngineNodeId,
    output_index: usize,
    signal: SignalType,
    /// "Module · Output", for the scope's caption.
    name: String,
}

impl SynthApp {
    /// Once the graph is drawn: probes the cable under the pointer while
    /// Alt is held, and lets go when Alt is released or the pointer leaves
    /// the cable.
    pub(super) fn tend_probe(&mut self, ctx: &egui::Context, hovered_cable: Option<InputId>) {
        let alt = ctx.input(|i| i.modifiers.alt);
        let wanted = hovered_cable.filter(|_| alt && self.is_playing).and_then(|jack| {
            // Still on the same cable: no need to follow it again
            match &self.probe.heard {
                Some((heard, carried)) if *heard == jack => Some((jack, carried.clone())),
                _ => self.carried(jack).map(|carried| (jack, carried)),
            }
        });

        let same = |a: &Option<(InputId, Carried)>, b: &Option<(InputId, Carried)>| {
            a.as_ref().map(|(_, c)| (c.node, c.output_index)) == b.as_ref().map(|(_, c)| (c.node, c.output_index))
        };
        let moved = !same(&self.probe.heard, &wanted);
        if moved {
            self.probe.since = self.probe.scope.written();
        }
        // A new cable, or the ducking changed while one is heard
        if moved || (wanted.is_some() && self.probe.duck != self.probe.told) {
            if let Some(handle) = self.ui_handle.as_mut() {
                let scope = self.probe.scope.clone();
                let duck = self.probe.duck;
                handle.probe(wanted.as_ref().map(|(_, carried)| Probe {
                    node_id: carried.node,
                    output_index: carried.output_index,
                    scope,
                    duck,
                }));
            }
            self.probe.told = self.probe.duck;
        }
        self.probe.heard = wanted;

        if self.probe.heard.is_some() {
            if let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) {
                self.probe.draw(ctx, pointer);
            }
            ctx.request_repaint();
        }
    }

    /// The module output a cable into `jack` carries, through any groups,
    /// or `None` for a MIDI cable (which has no sound to hear) or one whose
    /// trail ends at an empty group jack.
    fn carried(&self, jack: InputId) -> Option<Carried> {
        let graph = &self.graph_state.graph;
        let output = *graph.connections.get(jack)?.first()?;
        let leaf = groups::leaf_source(graph, &GroupIndex::of(graph), output)?;
        let port = graph.get_output(leaf);
        let signal = port.typ.0;
        if signal == SignalType::Midi {
            return None;
        }
        let node = graph.nodes.get(port.node)?;
        let output_name = node.outputs.iter().find(|(_, id)| *id == leaf).map_or("", |(name, _)| name.as_str());
        Some(Carried {
            node: self.user_state.get_engine_node_id(port.node)?,
            output_index: graph.get_output_index(leaf)?,
            signal,
            name: format!("{} · {}", node.label, output_name),
        })
    }
}

impl CableProbe {
    /// The ducking chosen last time, if there was one.
    pub(super) fn load(&mut self, storage: &dyn eframe::Storage) {
        let read = |key| storage.get_string(key).and_then(|v| v.parse::<f32>().ok()).map(|v| v.clamp(0.0, 1.0));
        self.duck.audio = read(DUCK_AUDIO_KEY).unwrap_or(self.duck.audio);
        self.duck.signal = read(DUCK_SIGNAL_KEY).unwrap_or(self.duck.signal);
    }

    pub(super) fn store(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(DUCK_AUDIO_KEY, self.duck.audio.to_string());
        storage.set_string(DUCK_SIGNAL_KEY, self.duck.signal.to_string());
    }

    /// The Cables menu's ducking: how far the patch is turned down beneath
    /// an audio cable, and beneath a control or gate cable, in dB.
    pub(super) fn duck_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Patch under a probe (Alt)").color(theme::text::SECONDARY));
        duck_slider(ui, &mut self.duck.audio, "audio", "How far the rest of the patch is turned down while you hear an audio cable");
        duck_slider(ui, &mut self.duck.signal, "control and gate", "How far the rest of the patch is turned down while you hear a control or gate cable");
    }
    /// Draws what's heard beside the pointer: a small scope in the cable's
    /// colour, captioned with the output it comes from.
    fn draw(&mut self, ctx: &egui::Context, pointer: Pos2) {
        let Some((_, carried)) = &self.heard else { return };
        // A scope switched on just now: what came before it is flat
        let fresh = self.scope.read(&mut self.window).wrapping_sub(self.since);
        let stale = SCOPE_WINDOW.saturating_sub(fresh);
        self.window[..stale].fill(0.0);
        let samples: &[f32] = match self.scope.kind() {
            // A slow signal shows the last second or so as it is
            Some(SignalType::Control | SignalType::Gate) => &self.window[..],
            // Audio from its last rising crossing that leaves a whole span
            _ => {
                let latest = SCOPE_WINDOW - AUDIO_SPAN;
                let start = (stale.max(1)..=latest).rev().find(|&n| self.window[n - 1] <= 0.0 && self.window[n] > 0.0).unwrap_or(latest);
                &self.window[start..start + AUDIO_SPAN]
            }
        };
        let color = carried.signal.color();
        let config = OscilloscopeConfig::new(SCOPE_SIZE.x, SCOPE_SIZE.y)
            .with_channel1_color(color)
            .with_trigger_indicator(false);

        // Kept on screen, flipping to the pointer's other side near an edge
        let screen = ctx.screen_rect();
        let size = SCOPE_SIZE + Vec2::new(16.0, 36.0);
        let mut at = pointer + FROM_POINTER;
        if at.x + size.x > screen.right() {
            at.x = pointer.x - FROM_POINTER.x - size.x;
        }
        if at.y + size.y > screen.bottom() {
            at.y = pointer.y - FROM_POINTER.y - size.y;
        }

        egui::Area::new(egui::Id::new("cable probe"))
            .order(egui::Order::Tooltip)
            .fixed_pos(at)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(theme::node::BODY_FILL.gamma_multiply(0.96))
                    .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
                    .rounding(theme::ROUNDING_SMALL)
                    .inner_margin(egui::Margin::same(8.0))
                    .shadow(egui::epaint::Shadow { offset: Vec2::new(0.0, 4.0), blur: 12.0, spread: 0.0, color: Color32::from_black_alpha(110) })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, color);
                            ui.label(egui::RichText::new(&carried.name).size(12.0).color(theme::text::PRIMARY));
                        });
                        ui.add_space(4.0);
                        oscilloscope_display(ui, samples, &[], &config);
                    });
            });
    }
}

/// One of the ducking sliders, in whole dB from Off to Solo.
fn duck_slider(ui: &mut egui::Ui, amount: &mut f32, name: &str, hint: &str) {
    let mut db = duck_to_db(*amount);
    let slider = egui::Slider::new(&mut db, 0.0..=DUCK_DB_RANGE)
        .step_by(1.0)
        .text(name)
        .custom_formatter(|db, _| duck_label(db as f32))
        .custom_parser(parse_duck);
    if ui.add(slider).on_hover_text(hint).changed() {
        *amount = db_to_duck(db);
    }
}

/// How far the patch is turned down, 0 to 1, as dB: all the way is the
/// slider's end.
fn duck_to_db(duck: f32) -> f32 {
    if duck >= 1.0 {
        DUCK_DB_RANGE
    } else {
        (-20.0 * (1.0 - duck).log10()).clamp(0.0, DUCK_DB_RANGE)
    }
}

/// ...and back: the slider's end silences the patch.
fn db_to_duck(db: f32) -> f32 {
    if db >= DUCK_DB_RANGE {
        1.0
    } else {
        1.0 - 10f32.powf(-db.max(0.0) / 20.0)
    }
}

fn duck_label(db: f32) -> String {
    if db >= DUCK_DB_RANGE {
        "Solo".to_string()
    } else if db < 0.5 {
        "Off".to_string()
    } else {
        format!("\u{2212}{db:.0} dB")
    }
}

/// What's typed into a ducking slider: "Off", "Solo", or dB with or
/// without the sign and unit.
fn parse_duck(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("off") {
        Some(0.0)
    } else if text.eq_ignore_ascii_case("solo") {
        Some(DUCK_DB_RANGE as f64)
    } else {
        let number = text.trim_end_matches(['d', 'D', 'b', 'B']).trim().trim_start_matches(['-', '\u{2212}']);
        number.parse::<f64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ducking_reads_in_db_and_its_ends_are_off_and_solo() {
        let duck = ProbeDuck::default();
        assert_eq!(duck_label(duck_to_db(duck.audio)), "\u{2212}20 dB");
        assert_eq!(duck_label(duck_to_db(duck.signal)), "\u{2212}6 dB");
        assert_eq!(duck_label(duck_to_db(0.0)), "Off");
        assert_eq!(duck_label(duck_to_db(1.0)), "Solo");
        assert_eq!(db_to_duck(DUCK_DB_RANGE), 1.0);
        assert_eq!(db_to_duck(0.0), 0.0);
        for db in [3.0, 12.0, 20.0, 39.0] {
            assert!((duck_to_db(db_to_duck(db)) - db).abs() < 1e-3, "{db}");
        }
        assert_eq!(parse_duck("\u{2212}20 dB"), Some(20.0));
        assert_eq!(parse_duck("12"), Some(12.0));
        assert_eq!(parse_duck("solo"), Some(DUCK_DB_RANGE as f64));
    }

    #[test]
    fn ducking_is_remembered() {
        #[derive(Default)]
        struct Memory(std::collections::HashMap<String, String>);
        impl eframe::Storage for Memory {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_string(), value);
            }
            fn flush(&mut self) {}
        }

        // Nothing kept yet: the defaults
        let mut storage = Memory::default();
        let mut probe = CableProbe::default();
        probe.load(&storage);
        assert_eq!(probe.duck, ProbeDuck::default());

        probe.duck = ProbeDuck { audio: 1.0, signal: 0.25 };
        probe.store(&mut storage);
        let mut next = CableProbe::default();
        next.load(&storage);
        assert_eq!(next.duck, probe.duck);
    }
}
