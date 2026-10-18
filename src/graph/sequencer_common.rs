//! What the three sequencer faces share: the Step, Chord and Trigger
//! Sequencers' pattern tab menu, walking round a pattern's steps, the popover
//! piano's reach, and the colours and sizes they agree on.

use eframe::egui::{self, Color32, RichText};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::modules::trigger_sequencer::{PATTERNS, PATTERN_NAMES};

use super::SynthResponse;

/// A lit step fires a gate, so it wears the gate cable's colour.
pub(super) const GATE_ON: Color32 = theme::signal::GATE;
pub(super) const GATE_OFF: Color32 = Color32::from_rgb(60, 60, 70);
/// The highest C the popover piano's lower octave can start on, so its two
/// octaves stay inside MIDI's 0–127.
pub(super) const TOP_BASE: u8 = 96;
/// One octave of the popover piano, in points.
pub(super) const OCTAVE_WIDTH: f32 = 140.0;
/// Room left of the pattern tabs for their captions, before zoom.
pub(super) const TAB_INSET: f32 = 34.0;

/// The step `by` steps along from `step`, round the loop.
pub(super) fn step_along(step: usize, by: isize, steps: usize) -> usize {
    (step as isize + by).rem_euclid(steps.max(1) as isize) as usize
}

/// Where the popover piano opens for a note: the note in its upper octave,
/// so there's an octave below it and most of one above.
pub(super) fn popover_base(note: i32) -> u8 {
    (note.div_euclid(12) * 12 - 12).clamp(0, TOP_BASE as i32) as u8
}

/// What a pattern tab's menu says its two actions carry over.
pub(super) struct MenuHints {
    /// What "Copy to" copies.
    pub copy: &'static str,
    /// What "Clear" clears, and what it keeps.
    pub clear: &'static str,
}

/// A pattern tab's right-click menu: copy the pattern to another, to start a
/// variation from, or clear it to rests. `copy_to(to)` and `clear()` give
/// the parameter changes, made only when their button's clicked.
pub(super) fn pattern_menu(
    ui: &mut egui::Ui,
    node_id: NodeId,
    pattern: usize,
    hints: &MenuHints,
    copy_to: impl Fn(usize) -> Vec<(String, f32)>,
    clear: impl FnOnce() -> Vec<(String, f32)>,
    edits: &mut Vec<SynthResponse>,
) {
    let from = PATTERN_NAMES[pattern];
    let edit = |label: String, changes: Vec<(String, f32)>| SynthResponse::EditParameters { node_id, label, changes };
    ui.label(RichText::new(format!("Pattern {from}")).strong());
    ui.separator();
    for to in (0..PATTERNS).filter(|&p| p != pattern) {
        if ui.button(format!("Copy to {}", PATTERN_NAMES[to])).on_hover_text(hints.copy).clicked() {
            edits.push(edit(format!("Copy pattern {from} to {}", PATTERN_NAMES[to]), copy_to(to)));
            ui.close();
        }
    }
    ui.separator();
    if ui.button(format!("Clear {from}")).on_hover_text(hints.clear).clicked() {
        edits.push(edit(format!("Clear pattern {from}"), clear()));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_along_goes_round_the_loop() {
        assert_eq!(step_along(3, 1, 8), 4);
        assert_eq!(step_along(7, 1, 8), 0);
        assert_eq!(step_along(0, -1, 5), 4);
        assert_eq!(step_along(2, 0, 5), 2);
        assert_eq!(step_along(0, 1, 0), 0);
    }

    #[test]
    fn the_piano_opens_an_octave_under_the_note() {
        assert_eq!(popover_base(60), 48); // C4 shows C3–B4
        assert_eq!(popover_base(71), 48);
        assert_eq!(popover_base(36), 24);
        assert_eq!(popover_base(5), 0);
        assert_eq!(popover_base(127), TOP_BASE);
        assert!(TOP_BASE as u32 + 23 <= 127);
    }
}
