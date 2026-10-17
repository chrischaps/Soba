//! Vertical fader widget for audio parameters.
//!
//! Provides a vertical slider with track groove and 3D thumb,
//! with fine control via Shift+drag.

use eframe::egui::{self, Color32, Pos2, Rect, Response, Rounding, Sense, Stroke, Ui, Vec2};
use std::ops::RangeInclusive;

use crate::app::theme;
use super::knob::ParamFormat;

/// Configuration for the Fader widget.
#[derive(Clone)]
pub struct FaderConfig {
    /// Width of the fader.
    pub width: f32,
    /// Height of the fader track.
    pub height: f32,
    /// Value range.
    pub range: RangeInclusive<f32>,
    /// Default value (for double-click reset).
    pub default: f32,
    /// Display format for the value.
    pub format: ParamFormat,
    /// Whether to use logarithmic scaling.
    pub logarithmic: bool,
    /// Label shown above/below the fader.
    pub label: Option<String>,
    /// Show value display.
    pub show_value: bool,
    /// Show scale markers on the side.
    pub show_markers: bool,
    /// Fine control multiplier when Shift is held.
    pub fine_multiplier: f32,
}

impl Default for FaderConfig {
    fn default() -> Self {
        Self {
            width: 30.0,
            height: 120.0,
            range: 0.0..=1.0,
            default: 0.5,
            format: ParamFormat::default(),
            logarithmic: false,
            label: None,
            show_value: true,
            show_markers: true,
            fine_multiplier: 0.1,
        }
    }
}

impl FaderConfig {
    /// Create a volume fader configuration.
    pub fn volume() -> Self {
        Self {
            range: 0.0..=1.0,
            default: 0.8,
            format: ParamFormat::Percent,
            ..Default::default()
        }
    }

    /// Create a pan fader configuration.
    pub fn pan() -> Self {
        Self {
            range: -1.0..=1.0,
            default: 0.0,
            format: ParamFormat::Raw { decimals: 2 },
            ..Default::default()
        }
    }

    /// Create a decibel fader configuration.
    pub fn decibels(min: f32, max: f32, default: f32) -> Self {
        Self {
            range: min..=max,
            default,
            format: ParamFormat::Decibels,
            ..Default::default()
        }
    }

    /// Set the label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the dimensions.
    pub fn with_size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Disable markers.
    pub fn without_markers(mut self) -> Self {
        self.show_markers = false;
        self
    }
}

/// A vertical fader widget for audio parameter control.
///
/// Features:
/// - Vertical track with groove effect
/// - 3D thumb/cap
/// - Scale markers on the side
/// - Click on track to jump to value
/// - Drag for smooth adjustment
/// - Shift+drag for fine control
/// - Double-click to reset to default
pub fn fader(ui: &mut Ui, value: &mut f32, config: &FaderConfig) -> Response {
    // Calculate total dimensions
    let marker_width = if config.show_markers { 15.0 } else { 0.0 };
    let total_width = config.width + marker_width;
    let total_height = config.height
        + if config.show_value { 18.0 } else { 0.0 }
        + if config.label.is_some() { 16.0 } else { 0.0 };

    let (rect, response) = ui.allocate_exact_size(Vec2::new(total_width, total_height), Sense::click_and_drag());

    // Handle double-click to reset
    if response.double_clicked() {
        *value = config.default;
    }

    // Calculate track area
    let track_left = rect.left() + marker_width;
    let track_top = rect.top() + if config.label.is_some() { 16.0 } else { 0.0 };
    let track_rect = Rect::from_min_size(
        Pos2::new(track_left, track_top),
        Vec2::new(config.width, config.height),
    );

    // Handle drag and click
    if response.dragged() || response.clicked() {
        let pointer_pos = response.interact_pointer_pos();
        if let Some(pos) = pointer_pos {
            // Calculate normalized position (0 at bottom, 1 at top)
            let thumb_height = 20.0;
            let usable_height = track_rect.height() - thumb_height;
            let track_top = track_rect.top() + thumb_height / 2.0;

            let y_pos = pos.y.clamp(track_top, track_top + usable_height);
            let raw_normalized = 1.0 - (y_pos - track_top) / usable_height;

            // Apply fine control modifier
            if response.dragged() {
                let delta_normalized = response.drag_delta().y / usable_height;
                let sensitivity = if ui.input(|i| i.modifiers.shift) {
                    config.fine_multiplier
                } else {
                    1.0
                };

                // Get current normalized value
                let current_normalized = if config.logarithmic {
                    let min = *config.range.start();
                    let max = *config.range.end();
                    let log_min = min.ln();
                    let log_max = max.ln();
                    (value.ln() - log_min) / (log_max - log_min)
                } else {
                    (*value - config.range.start()) / (config.range.end() - config.range.start())
                };

                let new_normalized =
                    (current_normalized - delta_normalized * sensitivity).clamp(0.0, 1.0);

                // Convert back to value
                if config.logarithmic {
                    let min = *config.range.start();
                    let max = *config.range.end();
                    let log_min = min.ln();
                    let log_max = max.ln();
                    *value = (log_min + new_normalized * (log_max - log_min)).exp();
                } else {
                    *value = config.range.start()
                        + new_normalized * (config.range.end() - config.range.start());
                }
            } else {
                // Click to jump
                if config.logarithmic {
                    let min = *config.range.start();
                    let max = *config.range.end();
                    let log_min = min.ln();
                    let log_max = max.ln();
                    *value = (log_min + raw_normalized * (log_max - log_min)).exp();
                } else {
                    *value = config.range.start()
                        + raw_normalized * (config.range.end() - config.range.start());
                }
            }
        }
    }

    *value = value.clamp(*config.range.start(), *config.range.end());

    // Draw the fader
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        // Draw label at top if present
        if let Some(label) = &config.label {
            painter.text(
                Pos2::new(rect.center().x, rect.top() + 8.0),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(10.0),
                theme::text::SECONDARY,
            );
        }

        // Draw scale markers
        if config.show_markers {
            draw_scale_markers(painter, track_rect, config);
        }

        // Draw track groove (recessed look)
        let groove_width = 8.0;
        let groove_rect = Rect::from_center_size(
            track_rect.center(),
            Vec2::new(groove_width, track_rect.height() - 10.0),
        );

        // Track shadow (inner)
        painter.rect_filled(
            groove_rect.translate(Vec2::new(1.0, 1.0)),
            Rounding::same(3.0),
            Color32::from_rgba_unmultiplied(0, 0, 0, 80),
        );

        // Track background
        painter.rect_filled(
            groove_rect,
            Rounding::same(3.0),
            Color32::from_rgb(25, 25, 35),
        );

        // Track highlight (left edge for 3D effect)
        painter.rect_filled(
            Rect::from_min_size(
                groove_rect.min,
                Vec2::new(1.5, groove_rect.height()),
            ),
            Rounding::same(1.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 15),
        );

        // Calculate thumb position
        let normalized = if config.logarithmic {
            let min = *config.range.start();
            let max = *config.range.end();
            let log_min = min.ln();
            let log_max = max.ln();
            (value.ln() - log_min) / (log_max - log_min)
        } else {
            (*value - config.range.start()) / (config.range.end() - config.range.start())
        };

        let thumb_height = 20.0;
        let thumb_width = config.width - 4.0;
        let usable_height = track_rect.height() - thumb_height;
        let thumb_y = track_rect.top() + thumb_height / 2.0 + (1.0 - normalized) * usable_height;

        let thumb_rect = Rect::from_center_size(
            Pos2::new(track_rect.center().x, thumb_y),
            Vec2::new(thumb_width, thumb_height),
        );

        // Draw filled portion of track
        let fill_rect = Rect::from_min_max(
            Pos2::new(groove_rect.left(), thumb_y),
            groove_rect.max,
        );
        if fill_rect.height() > 0.0 {
            painter.rect_filled(
                fill_rect,
                Rounding::same(3.0),
                theme::accent::PRIMARY.gamma_multiply(0.5),
            );
        }

        // Draw thumb shadow
        painter.rect_filled(
            thumb_rect.translate(Vec2::new(2.0, 3.0)),
            Rounding::same(4.0),
            Color32::from_rgba_unmultiplied(0, 0, 0, 60),
        );

        // Draw thumb body
        let thumb_color = if response.hovered() || response.dragged() {
            theme::background::WIDGET_HOVERED
        } else {
            theme::background::WIDGET
        };
        painter.rect_filled(thumb_rect, Rounding::same(4.0), thumb_color);

        // Thumb highlight (top edge)
        painter.rect_filled(
            Rect::from_min_size(
                thumb_rect.min + Vec2::new(2.0, 1.0),
                Vec2::new(thumb_rect.width() - 4.0, 2.0),
            ),
            Rounding::same(1.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 40),
        );

        // Thumb grip lines (center)
        let grip_y = thumb_rect.center().y;
        for i in -1..=1 {
            let line_y = grip_y + i as f32 * 4.0;
            painter.line_segment(
                [
                    Pos2::new(thumb_rect.left() + 6.0, line_y),
                    Pos2::new(thumb_rect.right() - 6.0, line_y),
                ],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 40)),
            );
            painter.line_segment(
                [
                    Pos2::new(thumb_rect.left() + 6.0, line_y + 1.0),
                    Pos2::new(thumb_rect.right() - 6.0, line_y + 1.0),
                ],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 20)),
            );
        }

        // Thumb border
        painter.rect_stroke(
            thumb_rect,
            Rounding::same(4.0),
            Stroke::new(
                1.0,
                if response.has_focus() || response.dragged() {
                    theme::accent::PRIMARY
                } else {
                    theme::node::BODY_STROKE
                },
            ),
        );

        // Draw value text at bottom
        if config.show_value {
            let value_y = track_rect.bottom() + 10.0;
            let value_text = config.format.format(*value);
            painter.text(
                Pos2::new(rect.center().x, value_y),
                egui::Align2::CENTER_CENTER,
                value_text,
                egui::FontId::proportional(10.0),
                theme::text::PRIMARY,
            );
        }
    }

    response
}

/// Draw scale markers on the left side of the fader track.
fn draw_scale_markers(painter: &egui::Painter, track_rect: Rect, _config: &FaderConfig) {
    let marker_x = track_rect.left() - 8.0;
    let thumb_height = 20.0;
    let usable_height = track_rect.height() - thumb_height;
    let top = track_rect.top() + thumb_height / 2.0;

    // Draw markers at key positions
    let marker_positions = [0.0, 0.25, 0.5, 0.75, 1.0];

    for &normalized in &marker_positions {
        let y = top + (1.0 - normalized) * usable_height;

        // Major marker (longer line)
        let line_length = if normalized == 0.5 { 6.0 } else { 4.0 };
        painter.line_segment(
            [
                Pos2::new(marker_x - line_length, y),
                Pos2::new(marker_x, y),
            ],
            Stroke::new(1.0, theme::text::DISABLED),
        );
    }
}

/// A compact horizontal fader for tight layouts.
pub fn horizontal_fader(ui: &mut Ui, value: &mut f32, config: &FaderConfig) -> Response {
    // Swap width and height for horizontal layout
    let total_width = config.height; // Use height as width
    let total_height = config.width + if config.show_value { 14.0 } else { 0.0 };

    let (rect, response) = ui.allocate_exact_size(Vec2::new(total_width, total_height), Sense::click_and_drag());

    // Handle double-click to reset
    if response.double_clicked() {
        *value = config.default;
    }

    // Calculate track area
    let track_rect = Rect::from_min_size(
        rect.min,
        Vec2::new(config.height, config.width),
    );

    // Handle drag and click (horizontal version)
    if response.dragged() || response.clicked() {
        let pointer_pos = response.interact_pointer_pos();
        if let Some(pos) = pointer_pos {
            let thumb_width = 20.0;
            let usable_width = track_rect.width() - thumb_width;
            let track_left = track_rect.left() + thumb_width / 2.0;

            let x_pos = pos.x.clamp(track_left, track_left + usable_width);
            let raw_normalized = (x_pos - track_left) / usable_width;

            if response.dragged() {
                let delta_normalized = response.drag_delta().x / usable_width;
                let sensitivity = if ui.input(|i| i.modifiers.shift) {
                    config.fine_multiplier
                } else {
                    1.0
                };

                let current_normalized = if config.logarithmic {
                    let min = *config.range.start();
                    let max = *config.range.end();
                    let log_min = min.ln();
                    let log_max = max.ln();
                    (value.ln() - log_min) / (log_max - log_min)
                } else {
                    (*value - config.range.start()) / (config.range.end() - config.range.start())
                };

                let new_normalized =
                    (current_normalized + delta_normalized * sensitivity).clamp(0.0, 1.0);

                if config.logarithmic {
                    let min = *config.range.start();
                    let max = *config.range.end();
                    let log_min = min.ln();
                    let log_max = max.ln();
                    *value = (log_min + new_normalized * (log_max - log_min)).exp();
                } else {
                    *value = config.range.start()
                        + new_normalized * (config.range.end() - config.range.start());
                }
            } else if config.logarithmic {
                let min = *config.range.start();
                let max = *config.range.end();
                let log_min = min.ln();
                let log_max = max.ln();
                *value = (log_min + raw_normalized * (log_max - log_min)).exp();
            } else {
                *value = config.range.start()
                    + raw_normalized * (config.range.end() - config.range.start());
            }
        }
    }

    *value = value.clamp(*config.range.start(), *config.range.end());

    // Draw the horizontal fader
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        // Draw track groove
        let groove_height = 8.0;
        let groove_rect = Rect::from_center_size(
            track_rect.center(),
            Vec2::new(track_rect.width() - 10.0, groove_height),
        );

        painter.rect_filled(
            groove_rect.translate(Vec2::new(1.0, 1.0)),
            Rounding::same(3.0),
            Color32::from_rgba_unmultiplied(0, 0, 0, 80),
        );
        painter.rect_filled(
            groove_rect,
            Rounding::same(3.0),
            Color32::from_rgb(25, 25, 35),
        );

        // Calculate thumb position
        let normalized = if config.logarithmic {
            let min = *config.range.start();
            let max = *config.range.end();
            let log_min = min.ln();
            let log_max = max.ln();
            (value.ln() - log_min) / (log_max - log_min)
        } else {
            (*value - config.range.start()) / (config.range.end() - config.range.start())
        };

        let thumb_width = 16.0;
        let thumb_height = config.width - 4.0;
        let usable_width = track_rect.width() - thumb_width;
        let thumb_x = track_rect.left() + thumb_width / 2.0 + normalized * usable_width;

        let thumb_rect = Rect::from_center_size(
            Pos2::new(thumb_x, track_rect.center().y),
            Vec2::new(thumb_width, thumb_height),
        );

        // Draw filled portion
        let fill_rect = Rect::from_min_max(
            groove_rect.min,
            Pos2::new(thumb_x, groove_rect.max.y),
        );
        if fill_rect.width() > 0.0 {
            painter.rect_filled(
                fill_rect,
                Rounding::same(3.0),
                theme::accent::PRIMARY.gamma_multiply(0.5),
            );
        }

        // Draw thumb
        let thumb_color = if response.hovered() || response.dragged() {
            theme::background::WIDGET_HOVERED
        } else {
            theme::background::WIDGET
        };
        painter.rect_filled(
            thumb_rect.translate(Vec2::new(2.0, 2.0)),
            Rounding::same(3.0),
            Color32::from_rgba_unmultiplied(0, 0, 0, 60),
        );
        painter.rect_filled(thumb_rect, Rounding::same(3.0), thumb_color);
        painter.rect_stroke(
            thumb_rect,
            Rounding::same(3.0),
            Stroke::new(
                1.0,
                if response.has_focus() || response.dragged() {
                    theme::accent::PRIMARY
                } else {
                    theme::node::BODY_STROKE
                },
            ),
        );

        // Draw value text below
        if config.show_value {
            let value_text = config.format.format(*value);
            painter.text(
                Pos2::new(rect.center().x, track_rect.bottom() + 8.0),
                egui::Align2::CENTER_CENTER,
                value_text,
                egui::FontId::proportional(10.0),
                theme::text::PRIMARY,
            );
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fader_config_default() {
        let config = FaderConfig::default();
        assert_eq!(config.width, 30.0);
        assert_eq!(config.height, 120.0);
        assert_eq!(*config.range.start(), 0.0);
        assert_eq!(*config.range.end(), 1.0);
        assert!(config.show_markers);
    }

    #[test]
    fn test_fader_config_volume() {
        let config = FaderConfig::volume();
        assert_eq!(config.default, 0.8);
        assert_eq!(config.format, ParamFormat::Percent);
    }

    #[test]
    fn test_fader_config_pan() {
        let config = FaderConfig::pan();
        assert_eq!(*config.range.start(), -1.0);
        assert_eq!(*config.range.end(), 1.0);
        assert_eq!(config.default, 0.0);
    }

    #[test]
    fn test_fader_config_with_label() {
        let config = FaderConfig::default().with_label("Volume");
        assert_eq!(config.label, Some("Volume".to_string()));
    }

    #[test]
    fn test_fader_config_with_size() {
        let config = FaderConfig::default().with_size(40.0, 150.0);
        assert_eq!(config.width, 40.0);
        assert_eq!(config.height, 150.0);
    }

    #[test]
    fn test_fader_config_without_markers() {
        let config = FaderConfig::default().without_markers();
        assert!(!config.show_markers);
    }
}
