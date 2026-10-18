//! Oscilloscope display widget for real-time waveform visualization.
//!
//! Provides a dual-channel oscilloscope display with:
//! - Two trace channels in different colors
//! - 4x4 grid overlay with graticules
//! - Trigger level indicator
//! - Time and amplitude markers

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::app::theme;

/// Configuration for the oscilloscope display widget.
#[derive(Clone)]
pub struct OscilloscopeConfig {
    /// Size of the display (width x height).
    pub size: Vec2,
    /// Channel 1 trace color.
    pub channel1_color: Color32,
    /// Channel 2 trace color.
    pub channel2_color: Color32,
    /// Line thickness for traces.
    pub line_thickness: f32,
    /// Whether to show glow effect.
    pub glow: bool,
    /// Vertical scale multiplier (1.0 = full range).
    pub amplitude_scale: f32,
    /// Trigger level (-1.0 to 1.0).
    pub trigger_level: f32,
    /// Whether to show trigger level indicator.
    pub show_trigger: bool,
    /// Number of grid divisions.
    pub grid_divisions: usize,
}

impl Default for OscilloscopeConfig {
    fn default() -> Self {
        Self {
            size: Vec2::new(200.0, 120.0),
            channel1_color: theme::signal::AUDIO,
            channel2_color: theme::signal::CONTROL,
            line_thickness: 1.5,
            glow: true,
            amplitude_scale: 1.0,
            trigger_level: 0.0,
            show_trigger: true,
            grid_divisions: 4,
        }
    }
}

impl OscilloscopeConfig {
    /// Create a new oscilloscope config with the specified size.
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            size: Vec2::new(width, height),
            ..Default::default()
        }
    }

    /// Set the channel 1 trace color.
    pub fn with_channel1_color(mut self, color: Color32) -> Self {
        self.channel1_color = color;
        self
    }

    /// Set the channel 2 trace color.
    pub fn with_channel2_color(mut self, color: Color32) -> Self {
        self.channel2_color = color;
        self
    }

    /// Set the line thickness.
    pub fn with_thickness(mut self, thickness: f32) -> Self {
        self.line_thickness = thickness;
        self
    }

    /// Enable or disable glow effect.
    pub fn with_glow(mut self, glow: bool) -> Self {
        self.glow = glow;
        self
    }

    /// Set the amplitude scale.
    pub fn with_amplitude_scale(mut self, scale: f32) -> Self {
        self.amplitude_scale = scale;
        self
    }

    /// Set the trigger level.
    pub fn with_trigger_level(mut self, level: f32) -> Self {
        self.trigger_level = level;
        self
    }

    /// Show or hide trigger indicator.
    pub fn with_trigger_indicator(mut self, show: bool) -> Self {
        self.show_trigger = show;
        self
    }
}

/// An oscilloscope display widget for visualizing two channels of waveform data.
///
/// Features:
/// - Dual-channel display with different colors
/// - 4x4 grid overlay
/// - Trigger level indicator
/// - Anti-aliased line rendering with optional glow
pub fn oscilloscope_display(
    ui: &mut Ui,
    channel1: &[f32],
    channel2: &[f32],
    config: &OscilloscopeConfig,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(config.size, Sense::hover());

    // Calculate zoom scale factor based on height (default 120.0)
    let zoom_scale = config.size.y / 120.0;

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        // Draw background and grid
        draw_oscilloscope_background(painter, rect, config, zoom_scale);

        // Draw trigger level indicator
        if config.show_trigger {
            draw_trigger_indicator(painter, rect, config, zoom_scale);
        }

        // Draw channel 2 first (so channel 1 draws on top)
        if !channel2.is_empty() {
            draw_trace(painter, rect, channel2, config.channel2_color, config, zoom_scale);
        }

        // Draw channel 1
        if !channel1.is_empty() {
            draw_trace(painter, rect, channel1, config.channel1_color, config, zoom_scale);
        }

        // Draw border
        painter.rect_stroke(
            rect,
            4.0 * zoom_scale,
            Stroke::new(1.0 * zoom_scale, Color32::from_rgb(60, 65, 80)),
            egui::StrokeKind::Middle,
        );
    }

    response
}

/// Draw the oscilloscope background with grid.
fn draw_oscilloscope_background(painter: &egui::Painter, rect: Rect, config: &OscilloscopeConfig, zoom_scale: f32) {
    // Dark background with slight green tint (classic CRT look)
    painter.rect_filled(
        rect,
        4.0 * zoom_scale,
        Color32::from_rgb(10, 15, 12),
    );

    let grid_color = Color32::from_rgba_unmultiplied(100, 120, 100, 40);
    let major_grid_color = Color32::from_rgba_unmultiplied(100, 120, 100, 60);
    let divisions = config.grid_divisions;

    // Draw grid lines
    for i in 1..divisions {
        let t = i as f32 / divisions as f32;
        let color = if i == divisions / 2 { major_grid_color } else { grid_color };

        // Vertical lines
        let x = rect.left() + rect.width() * t;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(0.5 * zoom_scale, color),
        );

        // Horizontal lines
        let y = rect.top() + rect.height() * t;
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(0.5 * zoom_scale, color),
        );
    }

    // Draw center crosshairs (brighter)
    let center_x = rect.center().x;
    let center_y = rect.center().y;
    let crosshair_color = Color32::from_rgba_unmultiplied(100, 140, 100, 80);

    painter.line_segment(
        [Pos2::new(center_x, rect.top()), Pos2::new(center_x, rect.bottom())],
        Stroke::new(1.0 * zoom_scale, crosshair_color),
    );
    painter.line_segment(
        [Pos2::new(rect.left(), center_y), Pos2::new(rect.right(), center_y)],
        Stroke::new(1.0 * zoom_scale, crosshair_color),
    );

    // Draw small tick marks on center axes
    let tick_length = 3.0 * zoom_scale;
    let tick_color = Color32::from_rgba_unmultiplied(100, 140, 100, 100);
    let num_ticks = 10;

    for i in 1..num_ticks {
        let t = i as f32 / num_ticks as f32;

        // Ticks on horizontal center line
        let x = rect.left() + rect.width() * t;
        painter.line_segment(
            [Pos2::new(x, center_y - tick_length), Pos2::new(x, center_y + tick_length)],
            Stroke::new(0.5 * zoom_scale, tick_color),
        );

        // Ticks on vertical center line
        let y = rect.top() + rect.height() * t;
        painter.line_segment(
            [Pos2::new(center_x - tick_length, y), Pos2::new(center_x + tick_length, y)],
            Stroke::new(0.5 * zoom_scale, tick_color),
        );
    }
}

/// Draw the trigger level indicator on the left edge.
fn draw_trigger_indicator(painter: &egui::Painter, rect: Rect, config: &OscilloscopeConfig, zoom_scale: f32) {
    let trigger_y = rect.center().y - config.trigger_level * rect.height() * 0.5 * config.amplitude_scale;
    let trigger_y = trigger_y.clamp(rect.top(), rect.bottom());

    // Draw small arrow indicator on left edge
    let arrow_size = 6.0 * zoom_scale;
    let arrow_points = vec![
        Pos2::new(rect.left(), trigger_y),
        Pos2::new(rect.left() + arrow_size, trigger_y - arrow_size * 0.5),
        Pos2::new(rect.left() + arrow_size, trigger_y + arrow_size * 0.5),
    ];

    let trigger_color = Color32::from_rgb(255, 200, 100);
    painter.add(egui::Shape::convex_polygon(
        arrow_points,
        trigger_color,
        Stroke::NONE,
    ));

    // Draw dashed trigger level line
    let dash_length = 4.0 * zoom_scale;
    let gap_length = 4.0 * zoom_scale;
    let mut x = rect.left() + arrow_size + 2.0 * zoom_scale;
    let dash_color = Color32::from_rgba_unmultiplied(255, 200, 100, 60);

    while x < rect.right() {
        let end_x = (x + dash_length).min(rect.right());
        painter.line_segment(
            [Pos2::new(x, trigger_y), Pos2::new(end_x, trigger_y)],
            Stroke::new(0.5 * zoom_scale, dash_color),
        );
        x += dash_length + gap_length;
    }
}

/// Draw a single trace channel.
fn draw_trace(
    painter: &egui::Painter,
    rect: Rect,
    samples: &[f32],
    color: Color32,
    config: &OscilloscopeConfig,
    zoom_scale: f32,
) {
    if samples.is_empty() {
        return;
    }

    // Calculate display points
    let num_points = rect.width() as usize;
    let points = calculate_trace_points(samples, num_points, rect, config);

    if points.len() < 2 {
        return;
    }

    // Draw glow effect (wider, semi-transparent line)
    if config.glow {
        let glow_color = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40);
        draw_polyline(painter, &points, glow_color, config.line_thickness * 3.0 * zoom_scale);
    }

    // Draw main trace
    draw_polyline(painter, &points, color, config.line_thickness * zoom_scale);
}

/// Calculate display points by resampling the input buffer.
fn calculate_trace_points(
    samples: &[f32],
    num_points: usize,
    rect: Rect,
    config: &OscilloscopeConfig,
) -> Vec<Pos2> {
    let mut points = Vec::with_capacity(num_points);

    if samples.is_empty() || num_points == 0 {
        return points;
    }

    let step = samples.len() as f32 / num_points as f32;
    let center_y = rect.center().y;
    let amplitude = rect.height() * 0.5 * config.amplitude_scale;

    for i in 0..num_points {
        let sample_idx = (i as f32 * step) as usize;
        let sample_idx = sample_idx.min(samples.len() - 1);

        // Use min/max detection for better peak visualization
        let sample = if step > 1.0 {
            let start = (i as f32 * step) as usize;
            let end = ((i + 1) as f32 * step).min(samples.len() as f32) as usize;
            let slice = &samples[start.min(samples.len() - 1)..end.min(samples.len())];
            if slice.is_empty() {
                0.0
            } else {
                let min = slice.iter().cloned().fold(f32::INFINITY, f32::min);
                let max = slice.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                // Use the value with larger absolute magnitude
                if min.abs() > max.abs() { min } else { max }
            }
        } else {
            samples[sample_idx]
        };

        let x = rect.left() + (i as f32 / (num_points - 1).max(1) as f32) * rect.width();
        let y = center_y - sample.clamp(-1.0, 1.0) * amplitude;

        points.push(Pos2::new(x, y));
    }

    points
}

/// Draw a polyline with anti-aliasing.
fn draw_polyline(painter: &egui::Painter, points: &[Pos2], color: Color32, thickness: f32) {
    if points.len() < 2 {
        return;
    }

    for i in 0..points.len() - 1 {
        painter.line_segment([points[i], points[i + 1]], Stroke::new(thickness, color));
    }
}

/// Trigger mode for the oscilloscope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TriggerMode {
    /// Auto-triggers if no signal crosses threshold.
    #[default]
    Auto,
    /// Only triggers on signal crossing threshold.
    Normal,
    /// Single trigger, then holds.
    Single,
    /// Continuous sweep, no triggering.
    Free,
}

impl TriggerMode {
    /// Get the display name for this trigger mode.
    pub fn name(&self) -> &'static str {
        match self {
            TriggerMode::Auto => "Auto",
            TriggerMode::Normal => "Normal",
            TriggerMode::Single => "Single",
            TriggerMode::Free => "Free",
        }
    }

    /// Get all trigger modes.
    pub fn all() -> &'static [TriggerMode] {
        &[TriggerMode::Auto, TriggerMode::Normal, TriggerMode::Single, TriggerMode::Free]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oscilloscope_config_default() {
        let config = OscilloscopeConfig::default();
        assert_eq!(config.size.x, 200.0);
        assert_eq!(config.size.y, 120.0);
        assert!(config.glow);
        assert!(config.show_trigger);
    }

    #[test]
    fn test_oscilloscope_config_builder() {
        let config = OscilloscopeConfig::new(300.0, 200.0)
            .with_thickness(2.0)
            .with_glow(false)
            .with_amplitude_scale(0.5)
            .with_trigger_level(0.25);

        assert_eq!(config.size, Vec2::new(300.0, 200.0));
        assert_eq!(config.line_thickness, 2.0);
        assert!(!config.glow);
        assert!((config.amplitude_scale - 0.5).abs() < f32::EPSILON);
        assert!((config.trigger_level - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn test_trigger_mode_names() {
        assert_eq!(TriggerMode::Auto.name(), "Auto");
        assert_eq!(TriggerMode::Normal.name(), "Normal");
        assert_eq!(TriggerMode::Single.name(), "Single");
        assert_eq!(TriggerMode::Free.name(), "Free");
    }

    #[test]
    fn test_trigger_mode_all() {
        let modes = TriggerMode::all();
        assert_eq!(modes.len(), 4);
    }

    #[test]
    fn test_calculate_trace_points_empty() {
        let samples: &[f32] = &[];
        let config = OscilloscopeConfig::default();
        let rect = Rect::from_min_size(Pos2::ZERO, config.size);
        let points = calculate_trace_points(samples, 100, rect, &config);
        assert!(points.is_empty());
    }

    #[test]
    fn test_calculate_trace_points() {
        let samples = vec![0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5, 0.0];
        let config = OscilloscopeConfig::default();
        let rect = Rect::from_min_size(Pos2::ZERO, config.size);
        let points = calculate_trace_points(&samples, 9, rect, &config);

        assert_eq!(points.len(), 9);
        // First point should be at left edge
        assert!((points[0].x - rect.left()).abs() < 0.1);
        // Last point should be at right edge
        assert!((points[8].x - rect.right()).abs() < 0.1);
    }
}
