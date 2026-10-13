//! A WAV file dragged over the window, before it's dropped.
//!
//! Only a drag from the desktop says what it carries while it hovers (the
//! browser hands over a file's bytes only once it's dropped). The file is
//! read on a thread of its own, since a long one takes a moment, and kept
//! here rather than on the [shelf](super::sample_shelf), which lets go each
//! frame of what no node plays. Over a Sampler or Chopper it's heard (see
//! [`crate::engine::cue`]) and drawn on the face in place of the file the
//! node has; dropped there, what was read goes on the shelf as it is.

use std::sync::mpsc::{self, Receiver, TryRecvError};
#[cfg(test)]
use std::sync::Arc;

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Shape, Stroke};
use egui_node_graph2::NodeId;

use super::sample_shelf::ShelvedSample;
use super::SamplerCue;
use crate::app::theme;
use crate::persistence::sample_files;

/// The file being dragged, and what reading it has come to.
pub struct HoveredFile {
    /// Its sample key: the path it would be loaded from.
    pub key: String,
    /// The Sampler (or Chopper) it was last heard on, while it's still over it.
    pub heard_on: Option<NodeId>,
    /// Read on a thread; `None` once it's come.
    reading: Option<Receiver<Result<ShelvedSample, String>>>,
    /// What came: the recording, or why it couldn't be read.
    read: Option<Result<ShelvedSample, String>>,
}

impl HoveredFile {
    /// Starts reading the file a key names, made ready to play at
    /// `sample_rate` where there's an engine to play it.
    pub fn read(key: String, sample_rate: Option<f32>) -> Self {
        let (send, reading) = mpsc::channel();
        let path = key.clone();
        let spawned = std::thread::Builder::new().name("hovered-file".into()).spawn(move || {
            let shelved = sample_files::load(&path).map(|decoded| {
                let mut shelved = ShelvedSample::new(decoded);
                if let Some(rate) = sample_rate {
                    shelved.at_rate(rate);
                }
                shelved
            });
            // Gone by now if the drag moved on
            let _ = send.send(shelved);
        });
        let read = spawned.err().map(|e| Err(e.to_string()));
        Self { key, heard_on: None, reading: read.is_none().then_some(reading), read }
    }

    /// Takes what the thread has read, if it's come.
    pub fn poll(&mut self) {
        let Some(reading) = &self.reading else { return };
        match reading.try_recv() {
            Ok(read) => self.read = Some(read),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => self.read = Some(Err("couldn't be read".to_string())),
        }
        self.reading = None;
    }

    /// The recording, once it's read.
    pub fn sample(&self) -> Option<&ShelvedSample> {
        self.read.as_ref().and_then(|read| read.as_ref().ok())
    }

    pub fn sample_mut(&mut self) -> Option<&mut ShelvedSample> {
        self.read.as_mut().and_then(|read| read.as_mut().ok())
    }

    /// The recording, for the shelf, if it's been read.
    pub fn into_sample(self) -> Option<ShelvedSample> {
        self.read.and_then(Result::ok)
    }
}

/// Draws a hovered file across a face's waveform panel: its waveform, the
/// span the node would play (`lo` to `hi`) bright against the rest, the cue
/// playing it, and its name.
pub fn draw(painter: &Painter, rect: Rect, plot: Rect, sample: &ShelvedSample, key: &str, span: (f32, f32), cue: Option<&SamplerCue>, z: f32) {
    let blue = crate::dsp::ModuleCategory::Source.color();
    let x_at = |place: f32| plot.left() + place.clamp(0.0, 1.0) * plot.width();
    let mid = plot.center().y;
    let (lo, hi) = span;

    // The waveform, a column at a time, scaled to fill the panel
    let columns = (plot.width() / (1.0 * z)).max(16.0) as usize;
    let overview = &sample.overview;
    let tallest = overview.iter().map(|(low, high)| low.abs().max(high.abs())).fold(0.05, f32::max);
    let half = plot.height() / 2.0 * 0.92;
    let played = cue.map(|cue| (cue.from.min(cue.place), cue.from.max(cue.place)));
    for column in 0..columns {
        let from = column * overview.len() / columns;
        let to = ((column + 1) * overview.len() / columns).max(from + 1).min(overview.len());
        if from >= to {
            continue;
        }
        let (low, high) = overview[from..to].iter().fold((0.0f32, 0.0f32), |(l, h), &(a, b)| (l.min(a), h.max(b)));
        let place = (column as f32 + 0.5) / columns as f32;
        let x = plot.left() + place * plot.width();
        // What's been heard is lit, as the ear has had it
        let color = if !(lo..=hi).contains(&place) {
            Color32::from_rgb(70, 80, 100).gamma_multiply(0.6)
        } else if played.is_some_and(|(a, b)| (a..=b).contains(&place)) {
            blue.lerp_to_gamma(Color32::WHITE, 0.3)
        } else {
            blue.gamma_multiply(0.75)
        };
        let top = mid - (high / tallest) * half;
        let bottom = mid - (low / tallest) * half;
        painter.line_segment([Pos2::new(x, top), Pos2::new(x, bottom.max(top + 0.6 * z))], Stroke::new(1.0 * z, color));
    }
    painter.rect_filled(rect, 3.0 * z, blue.gamma_multiply(0.08));

    // Its name, on a tab, so it's clear which file this is
    let font = egui::FontId::proportional(8.5 * z);
    let name = sample_files::file_name(key);
    let galley = painter.layout_no_wrap(name.to_string(), font, Color32::WHITE);
    let width = galley.size().x.min(plot.width() - 8.0 * z);
    let tab = Rect::from_min_size(Pos2::new(plot.left() + 1.0 * z, plot.top() + 1.0 * z), egui::vec2(width + 8.0 * z, galley.size().y + 3.0 * z));
    painter.rect_filled(tab, 2.0 * z, theme::background::PANEL.gamma_multiply(0.9));
    painter.with_clip_rect(tab.shrink2(egui::vec2(4.0 * z, 0.0))).galley(tab.min + egui::vec2(4.0 * z, 1.5 * z), galley, Color32::WHITE);

    // Where the cue is
    if let Some(cue) = cue {
        let x = x_at(cue.place);
        painter.vline(x, plot.y_range(), Stroke::new(1.2 * z, Color32::WHITE));
        for (y, inward) in [(plot.top(), 1.0), (plot.bottom(), -1.0)] {
            painter.add(Shape::convex_polygon(
                vec![Pos2::new(x - 3.0 * z, y), Pos2::new(x + 3.0 * z, y), Pos2::new(x, y + inward * 4.0 * z)],
                Color32::WHITE,
                Stroke::NONE,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_through(key: &str, rate: Option<f32>) -> HoveredFile {
        let mut hovered = HoveredFile::read(key.to_string(), rate);
        let begun = std::time::Instant::now();
        while hovered.reading.is_some() {
            assert!(begun.elapsed().as_secs() < 30, "still reading");
            std::thread::sleep(std::time::Duration::from_millis(5));
            hovered.poll();
        }
        hovered
    }

    #[test]
    fn a_hovered_file_is_read_aside_and_made_ready_to_play() {
        let key = concat!(env!("CARGO_MANIFEST_DIR"), "/patches/samples/fm-bell-c4.wav");
        let mut hovered = read_through(key, Some(96000.0));
        let source = hovered.sample().expect("read").source.clone();
        assert_ne!(source.sample_rate(), 96000.0);
        // Resampled on the thread, so nothing's left to do when it's heard
        let playing = hovered.sample_mut().unwrap().at_rate(96000.0);
        assert_eq!(playing.sample_rate(), 96000.0);
        let mut shelf = super::super::sample_shelf::SampleShelf::default();
        shelf.shelve(key, hovered.into_sample().unwrap());
        assert!(Arc::ptr_eq(&playing, &shelf.playing(key, 96000.0).unwrap()));
    }

    #[test]
    fn a_file_that_cant_be_read_says_so() {
        let key = std::env::temp_dir().join("soba-no-such-hovered.wav").to_string_lossy().into_owned();
        let hovered = read_through(&key, None);
        assert!(hovered.sample().is_none());
        assert!(matches!(hovered.read, Some(Err(_))));
    }
}
