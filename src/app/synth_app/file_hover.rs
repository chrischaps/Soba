//! Hearing a WAV file dragged over a Sampler before it's dropped, so files
//! can be auditioned straight from the file manager. The file is read as
//! soon as it's over the window (see [`crate::graph::hovered_file`]), plays
//! once from Start to End each time it comes over a Sampler or Chopper, and
//! stops when it leaves the node or the drag is given up. Dropped on the
//! node it's playing on, it plays on. Desktop only: the browser says what's
//! dragged only when it's dropped.

use eframe::egui;

use crate::graph::hovered_file::HoveredFile;
use crate::graph::sample_shelf::ShelvedSample;
use super::samples::is_wav;
use super::SynthApp;

impl SynthApp {
    /// Once a frame, after drops: follows the file dragged over the window,
    /// and plays it from the Sampler it's over.
    pub(super) fn tend_hovered_file(&mut self, ctx: &egui::Context) {
        let key = ctx.input(|i| {
            i.raw
                .hovered_files
                .iter()
                .filter_map(|file| file.path.as_ref())
                .find(|path| path.file_name().is_some_and(|name| is_wav(&name.to_string_lossy())))
                .map(|path| path.to_string_lossy().into_owned())
        });
        // Another file, or none: the last one is let go
        if self.user_state.hovered_file.as_ref().map(|hovered| &hovered.key) != key.as_ref() {
            self.stop_hovered_cue();
            let rate = self.engine_rate();
            self.user_state.hovered_file = key.map(|key| HoveredFile::read(key, rate));
        }
        let over = self.user_state.file_drop_target.flatten();
        let rate = self.engine_rate();
        let Some(hovered) = self.user_state.hovered_file.as_mut() else { return };
        hovered.poll();
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
        if let Some(node_id) = over {
            let (from, to) = self.start_to_end(node_id);
            self.start_cue(node_id, key, sample, from, to, true);
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
    /// been dropped there as `key`: if so, it plays on as the node's own.
    pub(super) fn keep_dropped_cue(&mut self, node_id: egui_node_graph2::NodeId, key: &str) -> bool {
        match self.user_state.cue.as_mut() {
            Some(cue) if cue.hovered && cue.node_id == node_id && cue.key == key => {
                cue.hovered = false;
                true
            }
            _ => false,
        }
    }
}
