//! Mutate, from the keyboard: Ctrl+M nudges the selected modules' knobs
//! (Alt+Ctrl+M their switches too), and holding it glides them further (see
//! [`crate::app::mutate`]). Letting go keeps the variation as one undo step.
//!
//! Under the selection, a strip of chips holds the original and the last
//! eight variations, each drawn as a row of ticks, one per knob, as high as
//! the knob sits round its travel. Clicking one sets the knobs back to it,
//! which is an undo step too.

use eframe::egui::{self, pos2, vec2, Color32, Rect, Sense, Stroke};
use egui_node_graph2::NodeId;
use web_time::Instant;

use crate::app::mutate::{self, Knob, Strip, Variation};
use crate::app::theme;
use super::SynthApp;

/// A press being held: what it moves, and from when.
pub(super) struct Held {
    seed: u64,
    started: Instant,
    knobs: Vec<Knob>,
    switches: bool,
}

/// Mutate's state: the press being held, and the strip.
#[derive(Default)]
pub(super) struct Mutating {
    held: Option<Held>,
    strip: Option<Strip>,
    /// Presses so far, so two in the same instant get different seeds.
    presses: u64,
    /// Where the strip was drawn last frame, if it was.
    shown: Option<Rect>,
}

impl Mutating {
    /// Whether a press is being held, which holds back the undo step.
    pub(super) fn holding(&self) -> bool {
        self.held.is_some()
    }
}

const CHIP_HEIGHT: f32 = 22.0;
const TICK_WIDTH: f32 = 2.0;
const TICK_GAP: f32 = 1.0;
const MOST_TICKS: usize = 24;

impl SynthApp {
    /// Ctrl+M starts a variation, and holding it lets the variation grow.
    pub(super) fn handle_mutate_key(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        let (pressed, switches) = ctx.input_mut(|i| (i.consume_key(Modifiers::COMMAND, Key::M), i.modifiers.alt));
        if pressed && self.mutating.held.is_none() {
            self.start_mutation(switches);
        }
        let Some(held) = &self.mutating.held else { return };

        let amount = mutate::amount(held.started.elapsed().as_secs_f32());
        let variation = mutate::variation(&held.knobs, held.seed, amount);
        mutate::apply(&mut self.graph_state.graph, &self.user_state, &variation.values);

        let down = ctx.input(|i| i.key_down(Key::M) && i.modifiers.command);
        if down {
            ctx.request_repaint();
            return;
        }
        let held = self.mutating.held.take().expect("held");
        let what = if held.switches { "knobs and switches" } else { "knobs" };
        self.history.name_next(format!("Mutate {} {what}", held.knobs.len()));
        if let Some(strip) = &mut self.mutating.strip {
            strip.keep(variation);
        }
    }

    fn start_mutation(&mut self, switches: bool) {
        let selection = self.graph_state.selected_nodes.clone();
        if selection.is_empty() {
            self.status_message = Some("Select a module or group to mutate".to_string());
            return;
        }
        let knobs = mutate::knobs(&self.graph_state.graph, &self.user_state, &selection, switches);
        if knobs.is_empty() {
            self.status_message = Some("Nothing to mutate here: its knobs are patched, or set how loud it is".to_string());
            return;
        }
        if !self.mutating.strip.as_ref().is_some_and(|strip| strip.is_for(&selection)) {
            self.mutating.strip = Some(Strip::new(&selection, mutate::as_they_are(&knobs)));
        }
        self.mutating.presses += 1;
        self.mutating.held = Some(Held {
            seed: mutate::fresh_seed(self.mutating.presses),
            started: Instant::now(),
            knobs,
            switches,
        });
    }

    /// Forgets the strip and any press, as a patch is opened or started.
    pub(super) fn forget_mutations(&mut self) {
        self.mutating.held = None;
        self.mutating.strip = None;
    }

    /// The strip of variations, under the selection it was made from.
    pub(super) fn show_variations(&mut self, ctx: &egui::Context) {
        let shown = self.mutating.shown.take();
        let Some(strip) = &self.mutating.strip else { return };
        // The editor, drawn first, takes a click on the strip for a click
        // off it and drops the selection: the strip keeps it
        let clicked_here = ctx.input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| shown.is_some_and(|r| r.contains(p))));
        if clicked_here && !strip.is_for(&self.graph_state.selected_nodes) {
            self.graph_state.selected_nodes = strip.selection.clone();
        }
        let selection = &self.graph_state.selected_nodes;
        if strip.variations.is_empty() || !strip.is_for(selection) {
            return;
        }
        let Some(under) = self.selection_on_screen(selection) else { return };
        let at = pos2(under.left(), under.bottom() + 8.0);
        if !self.editor_rect.contains(at) {
            return;
        }

        let chips: Vec<(Option<usize>, &Variation)> = std::iter::once((None, &strip.original))
            .chain(strip.variations.iter().enumerate().map(|(i, v)| (Some(i), v)))
            .collect();
        // The chip the knobs stand at, newest first, however they got there
        let graph = &self.graph_state.graph;
        let current = chips.iter().rev().find(|(_, v)| mutate::matches(graph, &self.user_state, &v.values)).map(|(i, _)| *i);
        let mut chosen = None;
        let area = egui::Area::new(egui::Id::new("mutate_strip"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (index, variation) in &chips {
                        let response = chip(ui, variation, current == Some(*index), index.is_none());
                        let response = match index {
                            None => response.on_hover_text("The knobs as they were before the first variation"),
                            Some(i) => response.on_hover_text(format!(
                                "Variation {} · seed {:016x}\n{} knobs, up to {:.0}% of the way round",
                                i + 1,
                                variation.seed,
                                variation.values.len(),
                                variation.amount * 100.0,
                            )),
                        };
                        if response.clicked() {
                            chosen = Some(*index);
                        }
                    }
                });
            });
        self.mutating.shown = Some(area.response.rect);

        if let Some(index) = chosen {
            let strip = self.mutating.strip.as_ref().expect("strip");
            let (values, label) = match index {
                None => (strip.original.values.clone(), "Restore the original knobs".to_string()),
                Some(i) => (strip.variations[i].values.clone(), format!("Restore variation {}", i + 1)),
            };
            mutate::apply(&mut self.graph_state.graph, &self.user_state, &values);
            self.history.name_next(label);
        }
    }

    /// The screen rectangle around the selected nodes on the level on show.
    fn selection_on_screen(&self, selection: &[NodeId]) -> Option<Rect> {
        let zoom = self.graph_state.pan_zoom.zoom;
        let origin = self.editor_rect.min.to_vec2() + self.graph_state.pan_zoom.pan;
        selection
            .iter()
            .filter(|n| self.graph_state.graph.nodes.get(**n).is_some_and(|node| node.user_data.parent == self.user_state.level))
            .filter_map(|&n| {
                let at = *self.graph_state.node_positions.get(n)? + origin;
                let size = self.node_sizes.get(&n).copied()? * zoom;
                Some(Rect::from_min_size(at, size))
            })
            .reduce(|a, b| a.union(b))
    }
}

/// One chip: a row of ticks, one per knob, as high as the knob sits round
/// its travel. The one the knobs are at is ringed.
fn chip(ui: &mut egui::Ui, variation: &Variation, current: bool, original: bool) -> egui::Response {
    let ticks = variation.travels.len().clamp(1, MOST_TICKS);
    let width = 12.0 + ticks as f32 * (TICK_WIDTH + TICK_GAP);
    let (rect, response) = ui.allocate_exact_size(vec2(width, CHIP_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect.expand(2.0));
    let hovered = response.hovered();
    let fill = if hovered { theme::background::WIDGET_HOVERED } else { theme::background::PANEL };
    painter.rect_filled(rect, 6.0, fill);
    let ring = if current {
        Stroke::new(1.5_f32, theme::accent::PRIMARY)
    } else {
        Stroke::new(1.0_f32, theme::background::WIDGET_ACTIVE)
    };
    painter.rect_stroke(rect, 6.0, ring, egui::StrokeKind::Middle);

    let tick: Color32 = if original { theme::text::QUIET } else { theme::signal::CONTROL };
    let inner = rect.shrink2(vec2(6.0, 5.0));
    // Many knobs share the ticks: each shows the knobs it stands for, averaged
    let per = variation.travels.len().div_ceil(ticks).max(1);
    for (i, group) in variation.travels.chunks(per).enumerate() {
        let travel = group.iter().sum::<f32>() / group.len() as f32;
        let x = inner.left() + i as f32 * (TICK_WIDTH + TICK_GAP);
        let height = 2.0 + travel * (inner.height() - 2.0);
        let bar = Rect::from_min_max(pos2(x, inner.bottom() - height), pos2(x + TICK_WIDTH, inner.bottom()));
        painter.rect_filled(bar, 0.5, tick);
    }
    response
}
