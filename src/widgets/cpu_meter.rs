//! CPU meter widget for displaying audio engine load.
//!
//! Provides a visual indicator of how much CPU time the audio processing
//! is consuming relative to the available time budget.

use eframe::egui::{self, Color32, Rect, Response, Sense, Ui, Vec2};

use crate::app::theme;

/// Room for the "CPU:" label before the bar.
const LABEL_WIDTH: f32 = 30.0;

/// Configuration for the CPU meter widget.
#[derive(Clone, Debug)]
pub struct CpuMeterConfig {
    /// Width of the meter bar in pixels.
    pub width: f32,
    /// Height of the meter bar in pixels.
    pub height: f32,
    /// Whether to show the percentage text.
    pub show_text: bool,
    /// Whether to show warning icon when load is high.
    pub show_warning: bool,
    /// Threshold for "moderate" load (yellow).
    pub moderate_threshold: f32,
    /// Threshold for "high" load (red).
    pub high_threshold: f32,
}

impl Default for CpuMeterConfig {
    fn default() -> Self {
        Self {
            width: 60.0,
            height: 12.0,
            show_text: true,
            show_warning: true,
            moderate_threshold: 50.0,
            high_threshold: 80.0,
        }
    }
}

impl CpuMeterConfig {
    /// Creates a compact CPU meter for status bars.
    pub fn compact() -> Self {
        Self {
            width: 50.0,
            height: 10.0,
            show_text: true,
            show_warning: true,
            ..Default::default()
        }
    }

    /// Creates a larger CPU meter for panels.
    pub fn large() -> Self {
        Self {
            width: 100.0,
            height: 16.0,
            show_text: true,
            show_warning: true,
            ..Default::default()
        }
    }
}

/// Returns the color for a given CPU load percentage.
pub fn cpu_load_color(load: f32, config: &CpuMeterConfig) -> Color32 {
    if load >= config.high_threshold {
        theme::accent::ERROR
    } else if load >= config.moderate_threshold {
        theme::accent::WARNING
    } else {
        theme::accent::SUCCESS
    }
}

/// Draws a CPU meter widget.
///
/// # Arguments
/// * `ui` - The egui UI to draw into
/// * `cpu_load` - Current CPU load percentage (0-100)
/// * `config` - Configuration for the meter appearance
///
/// # Returns
/// The response from the meter widget
pub fn cpu_meter(ui: &mut Ui, cpu_load: f32, config: &CpuMeterConfig) -> Response {
    // Calculate total size: label, bar, then the readout and warning after it
    let text_width = if config.show_text { 32.0 } else { 0.0 };
    let warning_width = if config.show_warning && cpu_load >= config.high_threshold { 16.0 } else { 0.0 };
    let total_width = LABEL_WIDTH + config.width + text_width + warning_width;

    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(total_width, config.height),
        Sense::hover(),
    );

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        // Draw "CPU:" label
        let label_rect = Rect::from_min_size(
            rect.min,
            Vec2::new(28.0, config.height),
        );
        painter.text(
            label_rect.center(),
            egui::Align2::CENTER_CENTER,
            "CPU:",
            egui::FontId::proportional(config.height * 0.85),
            theme::text::SECONDARY,
        );

        // Calculate bar rect (after label)
        let bar_left = rect.min.x + LABEL_WIDTH;
        let bar_rect = Rect::from_min_size(
            egui::pos2(bar_left, rect.min.y + 1.0),
            Vec2::new(config.width, config.height - 2.0),
        );

        // Draw background (track)
        painter.rect_filled(
            bar_rect,
            2.0,
            theme::background::WIDGET,
        );

        // Draw filled portion
        let fill_width = (cpu_load / 100.0).clamp(0.0, 1.0) * bar_rect.width();
        if fill_width > 0.0 {
            let fill_rect = Rect::from_min_size(
                bar_rect.min,
                Vec2::new(fill_width, bar_rect.height()),
            );
            let fill_color = cpu_load_color(cpu_load, config);
            painter.rect_filled(fill_rect, 2.0, fill_color);
        }

        // Draw border
        painter.rect_stroke(
            bar_rect,
            2.0,
            egui::Stroke::new(1.0_f32, theme::background::WIDGET_HOVERED),
        );

        // Draw percentage text
        if config.show_text {
            let text_rect = Rect::from_min_size(
                egui::pos2(bar_left + config.width + 4.0, rect.min.y),
                Vec2::new(35.0, config.height),
            );

            let text_color = cpu_load_color(cpu_load, config);
            painter.text(
                text_rect.left_center(),
                egui::Align2::LEFT_CENTER,
                format!("{:.0}%", cpu_load),
                egui::FontId::proportional(config.height * 0.9),
                text_color,
            );
        }

        // Draw warning icon if load is high
        if config.show_warning && cpu_load >= config.high_threshold {
            let warning_pos = egui::pos2(
                bar_left + config.width + text_width,
                rect.center().y,
            );
            painter.text(
                warning_pos,
                egui::Align2::LEFT_CENTER,
                "⚠",
                egui::FontId::proportional(config.height),
                theme::accent::ERROR,
            );
        }
    }

    // Add hover tooltip with more details
    response.on_hover_text(format!(
        "Audio CPU: {:.1}%\n{}",
        cpu_load,
        if cpu_load >= config.high_threshold {
            "High load - risk of audio glitches!"
        } else if cpu_load >= config.moderate_threshold {
            "Moderate load"
        } else {
            "Healthy"
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CpuMeterConfig::default();
        assert_eq!(config.width, 60.0);
        assert_eq!(config.height, 12.0);
        assert!(config.show_text);
        assert!(config.show_warning);
        assert_eq!(config.moderate_threshold, 50.0);
        assert_eq!(config.high_threshold, 80.0);
    }

    #[test]
    fn test_compact_config() {
        let config = CpuMeterConfig::compact();
        assert_eq!(config.width, 50.0);
        assert_eq!(config.height, 10.0);
    }

    #[test]
    fn test_large_config() {
        let config = CpuMeterConfig::large();
        assert_eq!(config.width, 100.0);
        assert_eq!(config.height, 16.0);
    }

    #[test]
    fn test_cpu_load_color_low() {
        let config = CpuMeterConfig::default();
        let color = cpu_load_color(25.0, &config);
        assert_eq!(color, theme::accent::SUCCESS);
    }

    #[test]
    fn test_cpu_load_color_moderate() {
        let config = CpuMeterConfig::default();
        let color = cpu_load_color(60.0, &config);
        assert_eq!(color, theme::accent::WARNING);
    }

    #[test]
    fn test_cpu_load_color_high() {
        let config = CpuMeterConfig::default();
        let color = cpu_load_color(90.0, &config);
        assert_eq!(color, theme::accent::ERROR);
    }

    #[test]
    fn test_cpu_load_color_at_thresholds() {
        let config = CpuMeterConfig::default();
        // At moderate threshold, should be warning
        assert_eq!(cpu_load_color(50.0, &config), theme::accent::WARNING);
        // Just below moderate, should be success
        assert_eq!(cpu_load_color(49.9, &config), theme::accent::SUCCESS);
        // At high threshold, should be error
        assert_eq!(cpu_load_color(80.0, &config), theme::accent::ERROR);
        // Just below high, should be warning
        assert_eq!(cpu_load_color(79.9, &config), theme::accent::WARNING);
    }
}
