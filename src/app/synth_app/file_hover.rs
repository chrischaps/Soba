//! Hearing a sound file dragged over a Sampler before it's dropped, so files
//! can be auditioned straight from the file manager. The file is read as
//! soon as it's over the window (see [`crate::graph::hovered_file`]), plays
//! once from Start to End each time it comes over a Sampler or Chopper, and
//! stops when it leaves the node or the drag is given up. Over empty canvas
//! it plays whole, as the new Sampler a drop there would, and a ghost of
//! that Sampler follows the pointer with the file's waveform in it. Dropped
//! where it's playing, it plays on. Desktop only: the browser says what's
//! dragged only when it's dropped.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke};

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::graph::hovered_file::{self, HoveredFile};
use crate::graph::sample_shelf::ShelvedSample;
use crate::persistence::sample_files;
use super::SynthApp;

impl SynthApp {
    /// Once a frame, after drops: follows the file dragged over the window,
    /// and plays it from the Sampler it's over, or as a new one over canvas.
    pub(super) fn tend_hovered_file(&mut self, ctx: &egui::Context) {
        let key = ctx.input(|i| {
            i.raw
                .hovered_files
                .iter()
                .filter_map(|file| file.path.as_ref())
                .find(|path| path.file_name().is_some_and(|name| sample_files::is_sound_file(&name.to_string_lossy())))
                .map(|path| path.to_string_lossy().into_owned())
        });
        // Another file, or none: the last one is let go
        if self.user_state.hovered_file.as_ref().map(|hovered| &hovered.key) != key.as_ref() {
            self.stop_hovered_cue();
            let rate = self.engine_rate();
            self.user_state.hovered_file = key.map(|key| HoveredFile::read(key, rate));
        }
        let over = self.user_state.file_drop_target;
        if let Some(hovered) = self.user_state.hovered_file.as_mut() {
            hovered.poll();
        }
        if over == Some(None) {
            self.draw_ghost_sampler(ctx);
        }
        let rate = self.engine_rate();
        let Some(hovered) = self.user_state.hovered_file.as_mut() else { return };
        if hovered.heard_on == over {
            return;
        }
        // Not read yet: it plays once it is
        let (Some(sample), Some(rate)) = (hovered.sample_mut(), rate) else {
            ctx.request_repaint();
            return;
        };
        let sample = sample.at_rate(rate);
        let key = hovered.key.clone();
        hovered.heard_on = over;
        self.stop_hovered_cue();
        match over {
            Some(Some(node_id)) => {
                let (from, to) = self.start_to_end(node_id);
                self.start_cue(node_id, key, sample, from, to, true);
            }
            Some(None) => self.start_canvas_cue(key, sample),
            None => {}
        }
    }

    /// Draws the Sampler a drop would make, faintly, where it would land
    /// under the pointer: the file's waveform in it, lit as it's heard.
    fn draw_ghost_sampler(&self, ctx: &egui::Context) {
        let Some(pointer) = super::samples::drop_pointer(ctx, self.capture.is_some()) else { return };
        let Some(hovered) = self.user_state.hovered_file.as_ref() else { return };
        if !self.editor_rect.contains(pointer) {
            return;
        }
        let z = self.graph_state.pan_zoom.zoom;
        // Where add_sampler_at puts the new node, at about a Sampler's size
        let card = Rect::from_min_size(pointer - GHOST_GRAB * z, GHOST_SIZE * z);
        let blue = ModuleCategory::Source.color();
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("ghost sampler"));
        let painter = ctx.layer_painter(layer).with_clip_rect(self.editor_rect);
        let rounding = CornerRadius::from(theme::ROUNDING.nw as f32 * z);
        painter.rect_filled(card, rounding, Color32::from_gray(63).gamma_multiply(0.75));
        let header = Rect::from_min_size(card.min, egui::vec2(card.width(), GHOST_HEADER * z));
        painter.rect_filled(header, CornerRadius { sw: 0, se: 0, ..rounding }, blue.gamma_multiply(0.35));
        let title = egui::FontId::proportional(13.0 * z);
        painter.text(header.left_center() + egui::vec2(8.0 * z, 0.0), egui::Align2::LEFT_CENTER, "Sampler", title, Color32::WHITE.gamma_multiply(0.7));
        painter.rect_stroke(card, rounding, Stroke::new(1.0 * z, blue.gamma_multiply(0.6)), egui::StrokeKind::Middle);

        let panel = Rect::from_min_size(card.min + GHOST_PANEL.min.to_vec2() * z, GHOST_PANEL.size() * z);
        let plot = panel.shrink2(egui::vec2(4.0 * z, 3.0 * z));
        painter.rect_filled(panel, 3.0 * z, Color32::from_rgb(20, 22, 30).gamma_multiply(0.9));
        match hovered.sample() {
            Some(sample) => {
                let cue = self.user_state.cue.as_ref().filter(|cue| cue.hovered && cue.node_id.is_none());
                hovered_file::draw(&painter, panel, plot, (&hovered.key, sample), (0.0, 1.0), cue, z);
            }
            // Still being read: an empty panel, as a Sampler's is before it's loaded
            None => {
                painter.rect_stroke(plot, 2.0 * z, Stroke::new(1.0 * z, blue.gamma_multiply(0.4)), egui::StrokeKind::Middle);
            }
        }
    }

    /// Stops the cue of a file dragged over a Sampler, if one plays.
    fn stop_hovered_cue(&mut self) {
        if self.user_state.cue.as_ref().is_some_and(|cue| cue.hovered) {
            self.stop_cue();
        }
    }

    /// The file dragged over the window, as read, if it's the one `key`
    /// names: a drop takes it rather than read it again.
    pub(super) fn take_hovered_file(&mut self, key: &str) -> Option<ShelvedSample> {
        self.user_state.hovered_file.take_if(|hovered| hovered.key == key).and_then(HoveredFile::into_sample)
    }

    /// Whether the cue playing is a file dragged over `node_id` that's now
    /// been dropped there as `key`, or over empty canvas when `node_id` is
    /// the new Sampler the drop made: if so, it plays on as the node's own.
    pub(super) fn keep_dropped_cue(&mut self, node_id: egui_node_graph2::NodeId, key: &str) -> bool {
        // A Sampler playing all of the file, untuned, plays what the canvas did
        let as_canvas = self.start_to_end(node_id) == (0.0, 1.0) && self.semitones(node_id) == 0.0;
        match self.user_state.cue.as_mut() {
            Some(cue) if cue.hovered && cue.key == key && (cue.node_id == Some(node_id) || (cue.node_id.is_none() && as_canvas)) => {
                cue.node_id = Some(node_id);
                cue.hovered = false;
                true
            }
            _ => false,
        }
    }
}

/// The ghost of a new Sampler, in patch units: where the pointer holds it
/// (as `add_sampler_at` places the node), its size, its header's height,
/// and where its waveform panel is in it.
const GHOST_GRAB: egui::Vec2 = egui::vec2(111.0, 60.0);
const GHOST_SIZE: egui::Vec2 = egui::vec2(231.0, 784.0);
const GHOST_HEADER: f32 = 30.0;
const GHOST_PANEL: Rect = Rect { min: Pos2::new(16.0, 369.0), max: Pos2::new(207.0, 440.0) };
