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
use crate::engine::{NodeId as EngineNodeId, Probe, ProbeScope, SCOPE_WINDOW};
use crate::graph::groups::{self, GroupIndex};
use crate::widgets::{oscilloscope_display, OscilloscopeConfig};
use super::SynthApp;

/// How much of the window an audio probe's scope shows, from a rising zero
/// crossing so a steady tone stands still: about 20 ms.
const AUDIO_SPAN: usize = SCOPE_WINDOW / 2;

/// The scope's size, and how far it sits from the pointer.
const SCOPE_SIZE: Vec2 = Vec2::new(176.0, 72.0);
const FROM_POINTER: Vec2 = Vec2::new(18.0, 20.0);

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
}

impl Default for CableProbe {
    fn default() -> Self {
        Self { scope: Arc::new(ProbeScope::new()), heard: None, since: 0, window: Box::new([0.0; SCOPE_WINDOW]) }
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
        if !same(&self.probe.heard, &wanted) {
            self.probe.since = self.probe.scope.written();
            if let Some(handle) = self.ui_handle.as_mut() {
                let scope = self.probe.scope.clone();
                handle.probe(wanted.as_ref().map(|(_, carried)| Probe {
                    node_id: carried.node,
                    output_index: carried.output_index,
                    scope,
                }));
            }
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
