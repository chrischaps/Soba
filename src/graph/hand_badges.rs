//! The badges of the knobs a controller's hands hold (#429): a small
//! purple pill at the dial's top left, numbered as the encoder that turns
//! it (1, 2, …) or the fader (F1, F2, …). They fade in as the selection changes,
//! so the eye is drawn to where the hands went, and then stay quiet.
//!
//! The app says which knobs are held, and which of the patch's own mapped
//! knobs are paused meanwhile (their badges dim). See
//! `crate::app::hands` for the rest.

use std::collections::{HashMap, HashSet};

use egui::{Align2, Color32, FontId, Pos2, Rect, Vec2};

use crate::app::theme;
use crate::engine::NodeId as EngineNodeId;

/// How long the badges take to fade in, in seconds.
pub const FADE_IN: f64 = 0.4;

/// The knobs the hands hold, as the canvas draws them.
#[derive(Clone, Debug, Default)]
pub struct HandBadges {
    /// Each knob held, by (engine node, parameter), and what its badge
    /// says: the encoder or fader holding it, "3" or "F1".
    pub slots: HashMap<(EngineNodeId, usize), String>,
    /// When they last took hold of something else, for the fade.
    pub since: f64,
    /// The patch's mapped knobs whose mappings are paused meanwhile.
    pub paused: HashSet<(EngineNodeId, usize)>,
}

impl HandBadges {
    /// How far the badges have faded in at `now`, 0-1.
    pub fn shown(&self, now: f64) -> f32 {
        ((now - self.since) / FADE_IN).clamp(0.0, 1.0) as f32
    }
}

/// Paints the badge of the knob `key` if the hands hold it, at the top left
/// of a dial `knob_size` across whose cell starts at `origin`.
pub fn paint(ui: &egui::Ui, key: (EngineNodeId, usize), origin: Pos2, zoom: f32, hands: &HandBadges) {
    let Some(label) = hands.slots.get(&key) else { return };
    let now = ui.input(|i| i.time);
    let shown = hands.shown(now);
    if shown < 1.0 {
        crate::app::pace::animate(ui.ctx());
    }
    // Eased, so it arrives softly
    let alpha = 1.0 - (1.0 - shown).powi(3);
    let height = 10.0 * zoom;
    // Wider for "16", and again for "F12"
    let width = height + label.chars().count().saturating_sub(1) as f32 * 5.0 * zoom;

    // Opposite the MIDI badge at the dial's top right, clear of its ring
    let center = origin + Vec2::new(-zoom, height / 2.0 - 2.0 * zoom);
    let rect = Rect::from_center_size(center, Vec2::new(width, height));
    let purple: Color32 = theme::signal::MIDI;
    let painter = ui.painter();
    painter.rect_filled(rect, height / 2.0, purple.gamma_multiply(0.8 * alpha));
    painter.text(rect.center() + Vec2::new(0.0, 0.25 * zoom), Align2::CENTER_CENTER, label, FontId::proportional(7.5 * zoom), Color32::WHITE.gamma_multiply(0.92 * alpha));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badges_fade_in_from_when_the_hands_moved() {
        let badges = HandBadges { since: 10.0, ..Default::default() };
        assert_eq!(badges.shown(10.0), 0.0);
        assert!(badges.shown(10.0 + FADE_IN / 2.0) > 0.4);
        assert_eq!(badges.shown(11.0), 1.0);
    }
}
