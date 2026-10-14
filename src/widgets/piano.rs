//! Piano keyboard widget for visual feedback of active notes.
//!
//! Provides a one-octave piano keyboard display that shows which notes are
//! currently pressed. Used by the Keyboard and MIDI Note input modules, and
//! by the Quantizer, which lights its scale and takes clicks on the keys.

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Ui, Vec2};

/// Key layout constants - which semitones correspond to white/black keys.
const WHITE_KEY_NOTES: [u8; 7] = [0, 2, 4, 5, 7, 9, 11]; // C D E F G A B
const BLACK_KEY_NOTES: [u8; 5] = [1, 3, 6, 8, 10];       // C# D# F# G# A#
/// Black key centres, in white key widths from the left. Each sits over the
/// line between two white keys, nudged the way a real piano's are: C# and D#
/// lean apart, F# and A# lean out from G#, which sits square on its line.
const BLACK_KEY_CENTERS: [f32; 5] = [0.92, 2.08, 3.88, 5.0, 6.12];

/// Configuration for the piano keyboard widget.
#[derive(Clone, Debug)]
pub struct PianoConfig {
    /// Total width of the keyboard.
    pub width: f32,
    /// Total height of the keyboard.
    pub height: f32,
    /// Color of white keys when inactive.
    pub white_key_color: Color32,
    /// Color of black keys when inactive.
    pub black_key_color: Color32,
    /// Color of white keys when active/pressed.
    pub white_key_active: Color32,
    /// Color of black keys when active/pressed.
    pub black_key_active: Color32,
    /// Glow color for active keys.
    pub glow_color: Color32,
    /// Whether to show the octave label below.
    pub show_octave: bool,
    /// Colour of the marks on keys in the scale (see [`PianoData::scale`]).
    pub scale_color: Color32,
    /// Whether keys can be clicked (see [`piano_keys`]).
    pub clickable: bool,
}

impl Default for PianoConfig {
    fn default() -> Self {
        Self {
            width: 140.0,
            height: 45.0,
            white_key_color: Color32::from_rgb(240, 240, 235),  // Off-white
            black_key_color: Color32::from_rgb(30, 30, 35),     // Near-black
            white_key_active: Color32::from_rgb(100, 180, 255), // Blue tint
            black_key_active: Color32::from_rgb(80, 140, 200),  // Darker blue
            glow_color: Color32::from_rgb(100, 180, 255),       // Blue glow
            show_octave: true,
            scale_color: Color32::from_rgb(100, 180, 255),
            clickable: false,
        }
    }
}

impl PianoConfig {
    /// Create a keyboard-style piano config (blue tint for active keys).
    pub fn keyboard() -> Self {
        Self::default()
    }

    /// Create a MIDI-style piano config (purple tint for active keys).
    pub fn midi() -> Self {
        Self {
            white_key_active: Color32::from_rgb(180, 100, 200), // Purple tint
            black_key_active: Color32::from_rgb(140, 80, 160),  // Darker purple
            glow_color: Color32::from_rgb(180, 100, 200),       // Purple glow
            ..Default::default()
        }
    }

    /// A quantizer's scale: lit and played in `color`, keys clickable.
    pub fn scale(color: Color32) -> Self {
        Self {
            // Black keys in the scale stand out from the near-black unlit ones
            black_key_color: Color32::from_rgb(62, 62, 74),
            white_key_active: color,
            black_key_active: color.gamma_multiply(0.85),
            glow_color: color,
            scale_color: color,
            show_octave: false,
            clickable: true,
            ..Default::default()
        }
    }

    /// Makes the keys sense clicks and presses (see [`piano_keys`]).
    pub fn clickable(mut self) -> Self {
        self.clickable = true;
        self
    }

    /// Set the size of the keyboard.
    pub fn with_size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set whether to show the octave label.
    pub fn with_octave_label(mut self, show: bool) -> Self {
        self.show_octave = show;
        self
    }
}

/// Data for the piano display.
#[derive(Clone, Debug, Default)]
pub struct PianoData {
    /// MIDI note numbers currently pressed (0-127).
    pub active_notes: Vec<u8>,
    /// The base MIDI note of the displayed octave (e.g., 60 for C4).
    pub base_note: u8,
    /// Octave shift for display label (e.g., 0 for C4, 1 for C5).
    pub octave_shift: i32,
    /// Pitch classes in a scale (bit 0 = C), or `None` to light no scale.
    /// Keys outside it are drawn unlit, and keys in it carry a mark.
    pub scale: Option<u16>,
    /// The scale's root (0 = C), whose mark is ringed.
    pub root: Option<u8>,
}

impl PianoData {
    /// Create a new PianoData.
    pub fn new(active_notes: Vec<u8>, base_note: u8, octave_shift: i32) -> Self {
        Self {
            active_notes,
            base_note,
            octave_shift,
            ..Default::default()
        }
    }

    /// Whether a key (0-11) is in the scale; every key is when none is set.
    fn in_scale(&self, semitone: u8) -> bool {
        self.scale.map_or(true, |scale| scale & (1 << semitone) != 0)
    }

    /// Check if a note (0-11 semitone within octave) is active.
    /// Maps any MIDI note to its position within an octave.
    fn is_note_active(&self, semitone: u8) -> bool {
        self.active_notes.iter().any(|&note| note % 12 == semitone)
    }
}

/// A piano keyboard widget showing which notes are currently pressed.
///
/// # Example
/// ```ignore
/// let data = PianoData {
///     active_notes: vec![60, 64, 67], // C4, E4, G4
///     base_note: 60,
///     octave_shift: 0,
///     ..Default::default()
/// };
/// let config = PianoConfig::keyboard().with_size(140.0, 45.0);
/// piano(ui, &data, &config);
/// ```
pub fn piano(ui: &mut Ui, data: &PianoData, config: &PianoConfig) -> Response {
    piano_keys(ui, data, config).0
}

/// The keyboard's white and black keys, each with its semitone (0-11).
fn key_rects(keyboard: Rect) -> ([(u8, Rect); 7], [(u8, Rect); 5]) {
    let white_key_width = keyboard.width() / 7.0;
    let black_key_width = white_key_width * 0.6;
    let black_key_height = keyboard.height() * 0.6;
    let whites = std::array::from_fn(|i| {
        let min = Pos2::new(keyboard.left() + i as f32 * white_key_width, keyboard.top());
        (WHITE_KEY_NOTES[i], Rect::from_min_size(min, Vec2::new(white_key_width - 1.0, keyboard.height())))
    });
    let blacks = std::array::from_fn(|i| {
        // Less half the 1pt gap between white keys, so the line runs down the middle
        let center = keyboard.left() + BLACK_KEY_CENTERS[i] * white_key_width - 0.5;
        let x = center - black_key_width / 2.0;
        let min = Pos2::new(x, keyboard.top());
        (BLACK_KEY_NOTES[i], Rect::from_min_size(min, Vec2::new(black_key_width, black_key_height)))
    });
    (whites, blacks)
}

/// The key (0-11) under `pos`. Black keys sit on top, so they win.
fn key_at(keyboard: Rect, pos: Pos2) -> Option<u8> {
    let (whites, blacks) = key_rects(keyboard);
    blacks
        .iter()
        .chain(whites.iter())
        .find(|(_, rect)| rect.contains(pos))
        .map(|(semitone, _)| *semitone)
}

/// A piano keyboard, returning the key (0-11) under the pointer as well.
///
/// With [`PianoConfig::clickable`] set, the keys sense clicks: a click on a
/// key is `response.clicked()` with that key returned. They sense drags too,
/// so a press held on the keys (`response.is_pointer_button_down_on()`)
/// stays with the piano as it slides from key to key, rather than moving
/// the module it's on.
pub fn piano_keys(ui: &mut Ui, data: &PianoData, config: &PianoConfig) -> (Response, Option<u8>) {
    // Calculate total height including label, which grows with the keys,
    // so it zooms with them
    let unit = config.height / PianoConfig::default().height;
    let label_height = if config.show_octave { 12.0 * unit } else { 0.0 };
    let total_height = config.height + label_height;
    let sense = if config.clickable { Sense::click_and_drag() } else { Sense::hover() };

    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(config.width, total_height),
        sense,
    );
    let keyboard_rect = Rect::from_min_size(rect.min, Vec2::new(config.width, config.height));
    let hovered = response.hover_pos().and_then(|pos| key_at(keyboard_rect, pos));
    if config.clickable && hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        // Strokes and marks grow with the keyboard, which grows with zoom
        let scale = config.height / 45.0;
        let (whites, blacks) = key_rects(keyboard_rect);
        let hover_stroke = egui::Stroke::new(1.5 * scale, config.scale_color);

        // A dot at the foot of each key in the scale; the root's is ringed
        let mark = |center: Pos2, semitone: u8, on_active: bool| {
            if data.scale.is_none() || !data.in_scale(semitone) {
                return;
            }
            let color = if on_active { Color32::WHITE } else { config.scale_color };
            if data.root == Some(semitone) {
                painter.circle_filled(center, 2.4 * scale, color);
                painter.circle_stroke(center, 4.0 * scale, egui::Stroke::new(1.0 * scale, color.gamma_multiply(0.7)));
            } else {
                painter.circle_filled(center, 1.9 * scale, color);
            }
        };

        // A white key: body, border, top highlight, scale mark and hover ring
        let white_key = |semitone: u8, key_rect: Rect| {
            let is_active = data.is_note_active(semitone);
            let lit = data.in_scale(semitone);

            // Key background: keys outside the scale are left unlit
            let key_color = if is_active {
                config.white_key_active
            } else if lit {
                config.white_key_color
            } else {
                Color32::from_rgb(98, 102, 118)
            };
            painter.rect_filled(key_rect, 2.0, key_color);

            // Key border (subtle)
            painter.rect_stroke(
                key_rect,
                2.0,
                egui::Stroke::new(0.5, Color32::from_gray(if lit { 120 } else { 60 })),
            );

            // Add subtle 3D effect (top highlight)
            if !is_active && lit {
                let highlight_rect = Rect::from_min_size(
                    key_rect.min,
                    Vec2::new(key_rect.width(), 2.0),
                );
                painter.rect_filled(highlight_rect, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 80));
            }

            mark(Pos2::new(key_rect.center().x, key_rect.bottom() - 6.0 * scale), semitone, is_active);
            if config.clickable && hovered == Some(semitone) {
                painter.rect_stroke(key_rect.shrink(0.75 * scale), 2.0, hover_stroke);
            }
        };

        // A pressed key's light spills a little onto its neighbours, evenly on
        // both sides, and grows with the keyboard. It stays on the keys: the
        // glow is clipped to the keyboard rather than bleeding onto the module.
        let glow_painter = painter.with_clip_rect(painter.clip_rect().intersect(keyboard_rect));
        let glow = |key_rect: Rect, rounding: f32| {
            let shadow = egui::epaint::Shadow {
                offset: Vec2::ZERO,
                blur: 6.0 * scale,
                spread: 1.0 * scale,
                color: config.glow_color.gamma_multiply(0.75),
            };
            glow_painter.add(shadow.as_shape(key_rect, rounding));
        };

        // White keys first; then the pressed ones' glow over their neighbours,
        // and the pressed keys again on top of their own glow
        for &(semitone, key_rect) in &whites {
            white_key(semitone, key_rect);
        }
        let pressed = || whites.iter().filter(|(semitone, _)| data.is_note_active(*semitone));
        for &(_, key_rect) in pressed() {
            glow(key_rect, 2.0);
        }
        for &(semitone, key_rect) in pressed() {
            white_key(semitone, key_rect);
        }

        // Black keys on top, each pressed one's glow just beneath it
        for &(semitone, key_rect) in &blacks {
            let is_active = data.is_note_active(semitone);
            let lit = data.in_scale(semitone);

            if is_active {
                glow(key_rect, 1.5);
            }

            // Key background
            let key_color = if is_active {
                config.black_key_active
            } else if lit {
                config.black_key_color
            } else {
                Color32::from_rgb(16, 16, 20)
            };
            painter.rect_filled(key_rect, 1.5, key_color);

            // Subtle highlight on black keys
            if !is_active && lit {
                let highlight_rect = Rect::from_min_size(
                    key_rect.min + Vec2::new(1.0, 1.0),
                    Vec2::new(key_rect.width() - 2.0, 3.0),
                );
                painter.rect_filled(highlight_rect, 1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 20));
            }

            mark(Pos2::new(key_rect.center().x, key_rect.bottom() - 5.0 * scale), semitone, is_active);
            if config.clickable && hovered == Some(semitone) {
                painter.rect_stroke(key_rect, 1.5, hover_stroke);
            }
        }

        // Draw octave label below
        if config.show_octave {
            let octave_num = 4 + data.octave_shift; // Base octave is C4
            let label = format!("C{}", octave_num);
            let label_pos = Pos2::new(
                rect.center().x,
                keyboard_rect.bottom() + 6.0 * unit,
            );
            painter.text(
                label_pos,
                egui::Align2::CENTER_CENTER,
                &label,
                egui::FontId::proportional(9.0 * unit),
                Color32::from_gray(160),
            );
        }
    }

    (response, hovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_piano_config_default() {
        let config = PianoConfig::default();
        assert_eq!(config.width, 140.0);
        assert_eq!(config.height, 45.0);
        assert!(config.show_octave);
    }

    #[test]
    fn test_piano_config_keyboard() {
        let config = PianoConfig::keyboard();
        // Should have blue tint
        assert!(config.white_key_active.b() > config.white_key_active.r());
    }

    #[test]
    fn test_piano_config_midi() {
        let config = PianoConfig::midi();
        // Should have purple tint (R and B both high)
        assert!(config.white_key_active.r() > 150);
        assert!(config.white_key_active.b() > 150);
    }

    #[test]
    fn test_piano_config_with_size() {
        let config = PianoConfig::default().with_size(200.0, 60.0);
        assert_eq!(config.width, 200.0);
        assert_eq!(config.height, 60.0);
    }

    #[test]
    fn test_piano_data_is_note_active() {
        let data = PianoData {
            active_notes: vec![60, 64, 67], // C4, E4, G4
            base_note: 60,
            octave_shift: 0,
            ..Default::default()
        };

        // C (semitone 0) should be active
        assert!(data.is_note_active(0));
        // E (semitone 4) should be active
        assert!(data.is_note_active(4));
        // G (semitone 7) should be active
        assert!(data.is_note_active(7));
        // D (semitone 2) should not be active
        assert!(!data.is_note_active(2));
    }

    #[test]
    fn test_piano_data_octave_wrapping() {
        // Notes from different octaves should light up the same keys
        let data = PianoData {
            active_notes: vec![48, 60, 72], // C3, C4, C5 - all C notes
            base_note: 60,
            octave_shift: 0,
            ..Default::default()
        };

        // All should show as C (semitone 0)
        assert!(data.is_note_active(0));
        // But not other notes
        assert!(!data.is_note_active(1));
    }

    #[test]
    fn test_key_at_prefers_black_keys() {
        let keyboard = Rect::from_min_size(Pos2::ZERO, Vec2::new(140.0, 45.0));
        // Top of the C/C# boundary is C#; the foot of it is C
        assert_eq!(key_at(keyboard, Pos2::new(15.0, 5.0)), Some(1));
        assert_eq!(key_at(keyboard, Pos2::new(15.0, 40.0)), Some(0));
        assert_eq!(key_at(keyboard, Pos2::new(135.0, 40.0)), Some(11));
        assert_eq!(key_at(keyboard, Pos2::new(70.0, 60.0)), None);
    }

    #[test]
    fn test_piano_data_in_scale() {
        let mut data = PianoData::default();
        assert!(data.in_scale(1), "no scale: every key lit");
        data.scale = Some(0b1010_1011_0101); // C major
        assert!(data.in_scale(0) && data.in_scale(11));
        assert!(!data.in_scale(1) && !data.in_scale(6));
    }

    #[test]
    fn test_white_key_notes() {
        // Verify white key notes are correct (C D E F G A B)
        assert_eq!(WHITE_KEY_NOTES, [0, 2, 4, 5, 7, 9, 11]);
    }

    #[test]
    fn test_black_key_notes() {
        // Verify black key notes are correct (C# D# F# G# A#)
        assert_eq!(BLACK_KEY_NOTES, [1, 3, 6, 8, 10]);
    }
}
