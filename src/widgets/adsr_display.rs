//! ADSR envelope visualization widget.
//!
//! Provides a visual display of ADSR envelope shape based on current parameter values.
//! The display shows attack, decay, sustain, and release segments drawn with
//! the envelope generator's own stage curve ([`stage_shape`]), so each segment
//! has exactly the shape the audio follows. Segment widths are log-scaled so a
//! 5 ms attack stays visible next to a 5 s release.

use eframe::egui::{self, Color32, Pos2, Response, Sense, Stroke, Ui, Vec2};

use crate::app::theme;
use crate::dsp::Readout;
use crate::modules::envelope::{curve_steepness, stage_shape, AdsrEnvelope};

/// Configuration for the ADSR display widget.
#[derive(Clone)]
pub struct AdsrConfig {
    /// Size of the display (width x height).
    pub size: Vec2,
    /// Envelope line color.
    pub color: Color32,
    /// Line thickness.
    pub line_thickness: f32,
    /// Whether to show a glow effect.
    pub glow: bool,
    /// Whether to fill below the envelope curve.
    pub filled: bool,
    /// Fill color (uses main color with reduced alpha if None).
    pub fill_color: Option<Color32>,
    /// Whether to show segment labels (A, D, S, R).
    pub show_labels: bool,
    /// Whether to show the sustain level line.
    pub show_sustain_line: bool,
    /// Whether to show grid lines.
    pub show_grid: bool,
}

impl Default for AdsrConfig {
    fn default() -> Self {
        Self {
            size: Vec2::new(140.0, 50.0),
            color: theme::signal::CONTROL,
            line_thickness: 1.5,
            glow: true,
            filled: true,
            fill_color: None,
            show_labels: true,
            show_sustain_line: true,
            show_grid: true,
        }
    }
}

impl AdsrConfig {
    /// Create a new ADSR config with the specified size.
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            size: Vec2::new(width, height),
            ..Default::default()
        }
    }

    /// Set the display size.
    pub fn with_size(mut self, width: f32, height: f32) -> Self {
        self.size = Vec2::new(width, height);
        self
    }

    /// Set the envelope color.
    pub fn with_color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }

    /// Enable or disable glow effect.
    pub fn with_glow(mut self, glow: bool) -> Self {
        self.glow = glow;
        self
    }

    /// Enable or disable fill.
    pub fn with_fill(mut self, filled: bool) -> Self {
        self.filled = filled;
        self
    }

    /// Show or hide segment labels.
    pub fn with_labels(mut self, show: bool) -> Self {
        self.show_labels = show;
        self
    }
}

/// ADSR envelope parameters for visualization.
#[derive(Clone, Copy, Debug)]
pub struct AdsrParams {
    /// Attack time in seconds (0.001 to 10.0).
    pub attack: f32,
    /// Decay time in seconds (0.001 to 10.0).
    pub decay: f32,
    /// Sustain level (0.0 to 1.0).
    pub sustain: f32,
    /// Release time in seconds (0.001 to 10.0).
    pub release: f32,
    /// Attack curve (0 = straight, 1 = deep RC).
    pub attack_curve: f32,
    /// Decay curve (0 = straight, 1 = deep RC).
    pub decay_curve: f32,
    /// Release curve (0 = straight, 1 = deep RC).
    pub release_curve: f32,
    /// Peak at the softest velocity (1.0 when velocity isn't patched).
    /// Below 1, a faint second envelope shows the softest note.
    pub softest_peak: f32,
    /// Which stages CV is moving, A, D, S, R: their letters light, and
    /// they're drawn as the envelope is running them.
    pub moved: [bool; 4],
}

impl Default for AdsrParams {
    fn default() -> Self {
        Self {
            attack: 0.01,
            decay: 0.1,
            sustain: 0.7,
            release: 0.3,
            attack_curve: 0.2,
            decay_curve: 0.5,
            release_curve: 0.5,
            softest_peak: 1.0,
            moved: [false; 4],
        }
    }
}

/// An envelope segment, for [`AdsrParams::level_at`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdsrSegment {
    Attack,
    Decay,
    Sustain,
    Release,
}

impl AdsrParams {
    /// Create new ADSR parameters.
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Self {
            attack: attack.clamp(0.001, 10.0),
            decay: decay.clamp(0.001, 10.0),
            sustain: sustain.clamp(0.0, 1.0),
            release: release.clamp(0.001, 10.0),
            ..Default::default()
        }
    }

    /// Sets the stage curves (0 = straight, 1 = deep RC).
    pub fn with_curves(mut self, attack: f32, decay: f32, release: f32) -> Self {
        self.attack_curve = attack.clamp(0.0, 1.0);
        self.decay_curve = decay.clamp(0.0, 1.0);
        self.release_curve = release.clamp(0.0, 1.0);
        self
    }

    /// Sets the peak of the softest note (1 - velocity amount).
    pub fn with_softest_peak(mut self, peak: f32) -> Self {
        self.softest_peak = peak.clamp(0.0, 1.0);
        self
    }

    /// With CV patched into a stage (`moved`, in A, D, S, R order), draws
    /// that stage as the envelope's `readout` says CV has moved it, and
    /// lights its letter. Before the engine has reported, the knob stands in.
    pub fn with_cv(mut self, moved: [bool; 4], readout: Option<&Readout>) -> Self {
        self.moved = moved;
        if let Some(values) = readout.map(|readout| readout.values) {
            let time = |index: usize| values[index].clamp(0.001, 10.0);
            let [attack, decay, sustain, release] = moved;
            if attack {
                self.attack = time(AdsrEnvelope::READOUT_ATTACK);
            }
            if decay {
                self.decay = time(AdsrEnvelope::READOUT_DECAY);
            }
            if sustain {
                self.sustain = values[AdsrEnvelope::READOUT_SUSTAIN].clamp(0.0, 1.0);
            }
            if release {
                self.release = time(AdsrEnvelope::READOUT_RELEASE);
            }
        }
        self
    }

    /// Envelope level at normalized time `u` (0-1) through a segment, for a
    /// note at full velocity. This is the curve the envelope module renders.
    pub fn level_at(&self, segment: AdsrSegment, u: f32) -> f32 {
        let shape = |curve: f32| stage_shape(curve_steepness(curve), u as f64) as f32;
        match segment {
            AdsrSegment::Attack => shape(self.attack_curve),
            AdsrSegment::Decay => 1.0 - (1.0 - self.sustain) * shape(self.decay_curve),
            AdsrSegment::Sustain => self.sustain,
            AdsrSegment::Release => self.sustain * (1.0 - shape(self.release_curve)),
        }
    }
}

/// Generate ADSR envelope curve points for display.
///
/// Returns a vector of (x, y) normalized points where:
/// - x is in range [0, 1] representing time
/// - y is in range [0, 1] representing amplitude
///
/// Each segment is drawn with [`AdsrParams::level_at`], the curve the envelope
/// module renders. Time segments are scaled to ensure visual clarity - each segment gets a minimum
/// visual width so the envelope shape is always readable.
pub fn generate_adsr_curve(params: &AdsrParams, num_points: usize) -> Vec<(f32, f32)> {
    let (attack_end, decay_end, sustain_end) = get_adsr_segment_boundaries(params);

    // Where x falls in [start, end], as 0-1
    let through = |x: f32, start: f32, end: f32| {
        if end > start {
            (x - start) / (end - start)
        } else {
            1.0
        }
    };

    (0..num_points)
        .map(|i| {
            let x = i as f32 / (num_points - 1) as f32;
            let y = if x <= attack_end {
                params.level_at(AdsrSegment::Attack, through(x, 0.0, attack_end))
            } else if x <= decay_end {
                params.level_at(AdsrSegment::Decay, through(x, attack_end, decay_end))
            } else if x <= sustain_end {
                params.level_at(AdsrSegment::Sustain, 0.0)
            } else {
                params.level_at(AdsrSegment::Release, through(x, sustain_end, 1.0))
            };
            (x, y.clamp(0.0, 1.0))
        })
        .collect()
}

/// Get the segment boundaries for drawing and label positioning.
/// Returns (attack_end, decay_end, sustain_end) as normalized x positions.
///
/// Widths grow with ln(1 + seconds), so short stages stay readable; the
/// sustain gets a fixed width since it lasts as long as the gate.
pub fn get_adsr_segment_boundaries(params: &AdsrParams) -> (f32, f32, f32) {
    let attack_log = (1.0 + params.attack).ln();
    let decay_log = (1.0 + params.decay).ln();
    let sustain_log = (1.0 + 0.15_f32).ln();
    let release_log = (1.0 + params.release).ln();
    let total_log = attack_log + decay_log + sustain_log + release_log;

    let attack_end = attack_log / total_log;
    let decay_end = (attack_log + decay_log) / total_log;
    let sustain_end = (attack_log + decay_log + sustain_log) / total_log;

    (attack_end, decay_end, sustain_end)
}

/// Display an ADSR envelope visualization.
///
/// Shows the envelope shape based on current Attack, Decay, Sustain, and Release values.
/// Features:
/// - Exponential curves matching actual envelope behavior
/// - Optional segment labels (A, D, S, R)
/// - Sustain level indicator line
/// - Fill and glow effects for visual clarity
pub fn adsr_display(ui: &mut Ui, params: &AdsrParams, config: &AdsrConfig) -> Response {
    let (rect, response) = ui.allocate_exact_size(config.size, Sense::hover());

    // Calculate zoom scale factor based on height (default 50.0)
    let zoom_scale = config.size.y / 50.0;

    if ui.is_rect_visible(rect) {
        // Use clipped painter to prevent glow effects from extending outside bounds
        let painter = ui.painter_at(rect);

        // Draw background
        painter.rect_filled(rect, 2.0 * zoom_scale, Color32::from_rgb(20, 22, 30));

        // Draw subtle grid
        if config.show_grid {
            let grid_color = Color32::from_rgba_unmultiplied(255, 255, 255, 15);

            // Vertical divisions (4)
            for i in 1..4 {
                let x = rect.left() + rect.width() * (i as f32 / 4.0);
                painter.line_segment(
                    [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                    Stroke::new(0.5 * zoom_scale, grid_color),
                );
            }

            // Horizontal divisions (4)
            for i in 1..4 {
                let y = rect.top() + rect.height() * (i as f32 / 4.0);
                painter.line_segment(
                    [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                    Stroke::new(0.5 * zoom_scale, grid_color),
                );
            }
        }

        // Generate envelope curve
        let num_points = (config.size.x as usize).max(64);
        let curve = generate_adsr_curve(params, num_points);

        // Convert to screen coordinates
        // Y is inverted (0 at bottom, 1 at top)
        let padding_top = 4.0 * zoom_scale;
        let padding_bottom = if config.show_labels { 14.0 * zoom_scale } else { 4.0 * zoom_scale };
        let draw_height = rect.height() - padding_top - padding_bottom;

        let to_screen = |(x, y): &(f32, f32), scale: f32| {
            Pos2::new(
                rect.left() + x * rect.width(),
                rect.top() + padding_top + (1.0 - y * scale) * draw_height,
            )
        };
        let points: Vec<Pos2> = curve.iter().map(|p| to_screen(p, 1.0)).collect();

        // Draw sustain level line
        if config.show_sustain_line && params.sustain > 0.01 {
            let sustain_y = rect.top() + padding_top + (1.0 - params.sustain) * draw_height;
            let sustain_color = Color32::from_rgba_unmultiplied(
                config.color.r(),
                config.color.g(),
                config.color.b(),
                40,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left(), sustain_y),
                    Pos2::new(rect.right(), sustain_y),
                ],
                Stroke::new(1.0 * zoom_scale, sustain_color),
            );
        }

        // Draw filled area if enabled using vertical quad strips
        // (convex_polygon doesn't work because ADSR shape is concave)
        if config.filled && points.len() >= 2 {
            let fill_color = config.fill_color.unwrap_or_else(|| {
                let c = config.color;
                Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 30)
            });

            let baseline_y = rect.top() + padding_top + draw_height;

            // Draw vertical quad strips between adjacent curve points
            for i in 0..points.len() - 1 {
                let p1 = points[i];
                let p2 = points[i + 1];
                // Create a quad from curve point to baseline
                let quad = vec![
                    p1,                                    // top-left
                    p2,                                    // top-right
                    Pos2::new(p2.x, baseline_y),          // bottom-right
                    Pos2::new(p1.x, baseline_y),          // bottom-left
                ];
                painter.add(egui::Shape::convex_polygon(
                    quad,
                    fill_color,
                    Stroke::NONE,
                ));
            }
        }

        // Draw glow effect
        if config.glow && points.len() >= 2 {
            let glow_color = Color32::from_rgba_unmultiplied(
                config.color.r(),
                config.color.g(),
                config.color.b(),
                50,
            );
            draw_polyline(&painter, &points, glow_color, config.line_thickness * 3.0);
        }

        // The softest note: the same shape at its lower peak, faint and
        // without glow, so the span between the two lines is the dynamics
        if params.softest_peak < 0.999 && curve.len() >= 2 {
            let ghost: Vec<Pos2> = curve.iter().map(|p| to_screen(p, params.softest_peak)).collect();
            let ghost_color = Color32::from_rgba_unmultiplied(
                config.color.r(),
                config.color.g(),
                config.color.b(),
                90,
            );
            draw_polyline(&painter, &ghost, ghost_color, config.line_thickness * 0.75);
        }

        // Draw main envelope line
        if points.len() >= 2 {
            draw_polyline(&painter, &points, config.color, config.line_thickness);
        }

        // Draw segment labels
        if config.show_labels {
            let label_y = rect.bottom() - 2.0 * zoom_scale;
            let label_color = Color32::from_rgba_unmultiplied(255, 255, 255, 120);
            let font = egui::FontId::proportional(9.0 * zoom_scale);

            // Get segment boundaries using the same logarithmic scaling as the curve
            let (attack_end, decay_end, sustain_end) = get_adsr_segment_boundaries(params);

            // Position labels at the center of each segment
            let attack_x = rect.left() + (attack_end * 0.5) * rect.width();
            let decay_x = rect.left() + ((attack_end + decay_end) * 0.5) * rect.width();
            let sustain_x = rect.left() + ((decay_end + sustain_end) * 0.5) * rect.width();
            let release_x = rect.left() + ((sustain_end + 1.0) * 0.5) * rect.width();

            // Draw labels centered under each segment; a stage CV is
            // moving lights in the envelope's colour
            let lit = Color32::from_rgba_unmultiplied(config.color.r(), config.color.g(), config.color.b(), 230);
            let segments = [(attack_x, "A"), (decay_x, "D"), (sustain_x, "S"), (release_x, "R")];
            for ((x, letter), moved) in segments.into_iter().zip(params.moved) {
                painter.text(
                    Pos2::new(x, label_y),
                    egui::Align2::CENTER_BOTTOM,
                    letter,
                    font.clone(),
                    if moved { lit } else { label_color },
                );
            }
        }

        // Draw border
        painter.rect_stroke(rect, 2.0 * zoom_scale, Stroke::new(1.0 * zoom_scale, Color32::from_rgb(50, 55, 70)));
    }

    response
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adsr_config_default() {
        let config = AdsrConfig::default();
        assert_eq!(config.size.x, 140.0);
        assert_eq!(config.size.y, 50.0);
        assert!(config.glow);
        assert!(config.filled);
        assert!(config.show_labels);
    }

    #[test]
    fn test_adsr_config_builder() {
        let config = AdsrConfig::default()
            .with_size(200.0, 100.0)
            .with_glow(false)
            .with_fill(false)
            .with_labels(false);

        assert_eq!(config.size, Vec2::new(200.0, 100.0));
        assert!(!config.glow);
        assert!(!config.filled);
        assert!(!config.show_labels);
    }

    #[test]
    fn test_adsr_params_default() {
        let params = AdsrParams::default();
        assert!((params.attack - 0.01).abs() < f32::EPSILON);
        assert!((params.decay - 0.1).abs() < f32::EPSILON);
        assert!((params.sustain - 0.7).abs() < f32::EPSILON);
        assert!((params.release - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_adsr_params_clamping() {
        let params = AdsrParams::new(-1.0, 100.0, 2.0, -5.0);
        assert_eq!(params.attack, 0.001);
        assert_eq!(params.decay, 10.0);
        assert_eq!(params.sustain, 1.0);
        assert_eq!(params.release, 0.001);
    }

    #[test]
    fn test_generate_adsr_curve_length() {
        let params = AdsrParams::default();
        let curve = generate_adsr_curve(&params, 100);
        assert_eq!(curve.len(), 100);
    }

    #[test]
    fn test_generate_adsr_curve_bounds() {
        let params = AdsrParams::default();
        let curve = generate_adsr_curve(&params, 100);

        for (x, y) in &curve {
            assert!(*x >= 0.0 && *x <= 1.0, "x={} out of bounds", x);
            assert!(*y >= 0.0 && *y <= 1.0, "y={} out of bounds", y);
        }
    }

    #[test]
    fn test_generate_adsr_curve_start_end() {
        let params = AdsrParams::default();
        let curve = generate_adsr_curve(&params, 100);

        // Should start at (0, ~0) - beginning of attack
        assert!(curve[0].0 < 0.01);
        assert!(curve[0].1 < 0.1);

        // Should end near (1, ~0) - end of release
        assert!(curve[99].0 > 0.99);
        assert!(curve[99].1 < 0.1);
    }

    #[test]
    fn test_generate_adsr_curve_peak() {
        let params = AdsrParams::default();
        let curve = generate_adsr_curve(&params, 100);

        // Should reach near 1.0 at peak (end of attack)
        let max_y = curve.iter().map(|(_, y)| *y).fold(0.0f32, f32::max);
        assert!(max_y > 0.9, "Peak should be near 1.0, got {}", max_y);
    }

    #[test]
    fn test_generate_adsr_curve_sustain_level() {
        let params = AdsrParams::new(0.01, 0.1, 0.5, 0.3);
        let curve = generate_adsr_curve(&params, 200);

        // Use helper to get segment boundaries
        let (_, decay_end, sustain_end) = get_adsr_segment_boundaries(&params);

        // Find points in the sustain region
        let sustain_points: Vec<f32> = curve
            .iter()
            .filter(|(x, _)| *x > decay_end + 0.02 && *x < sustain_end - 0.02)
            .map(|(_, y)| *y)
            .collect();

        // Sustain region should be near sustain level
        if !sustain_points.is_empty() {
            let avg_sustain: f32 = sustain_points.iter().sum::<f32>() / sustain_points.len() as f32;
            assert!(
                (avg_sustain - 0.5).abs() < 0.1,
                "Sustain region should be near 0.5, got {}",
                avg_sustain
            );
        }
    }

    #[test]
    fn test_generate_adsr_curve_zero_sustain() {
        let params = AdsrParams::new(0.01, 0.1, 0.0, 0.3);
        let curve = generate_adsr_curve(&params, 100);

        // Use helper to get segment boundaries
        let (_, decay_end, sustain_end) = get_adsr_segment_boundaries(&params);

        // With zero sustain, should decay to near zero
        let sustain_points: Vec<f32> = curve
            .iter()
            .filter(|(x, _)| *x > decay_end + 0.02 && *x < sustain_end - 0.02)
            .map(|(_, y)| *y)
            .collect();

        if !sustain_points.is_empty() {
            for y in sustain_points {
                assert!(y < 0.1, "Zero sustain should produce near-zero values");
            }
        }
    }

    #[test]
    fn test_display_matches_rendered_envelope() {
        use crate::dsp::{DspModule, ProcessContext, SignalBuffer};
        use crate::modules::AdsrEnvelope;

        // Render a note through the real module, then check every segment
        // of the display against it at the same point in each stage
        let sr = 48000.0;
        for (curves, sustain) in [((0.2, 0.5, 0.5), 0.7), ((0.0, 1.0, 0.3), 0.25), ((0.9, 0.1, 0.0), 0.5)] {
            let (a, d, r) = (0.04, 0.07, 0.11);
            let params = AdsrParams::new(a, d, sustain, r).with_curves(curves.0, curves.1, curves.2);

            let samples = |secs: f32| (secs * sr) as usize;
            let hold = samples(a + d + 0.05);
            let total = hold + samples(r) + 10;
            let mut env = AdsrEnvelope::new();
            env.prepare(sr, total);
            let mut gate = SignalBuffer::control(total);
            gate.samples[..hold].fill(1.0);
            let mut out = vec![SignalBuffer::control(total)];
            let module_params = [a, d, sustain, r, curves.0, curves.1, curves.2, 0.5];
            env.process(&[&gate], &mut out, &module_params, &ProcessContext::new(sr, total));
            let out = &out[0].samples;

            let decay_start = out.iter().position(|&s| s >= 1.0).unwrap();
            for step in 0..=20 {
                let u = step as f32 / 20.0;
                let at = |start: usize, secs: f32| out[start + (u * secs * sr).round() as usize];
                for (segment, rendered) in [
                    (AdsrSegment::Attack, at(0, a)),
                    (AdsrSegment::Decay, at(decay_start, d)),
                    (AdsrSegment::Release, at(hold, r)),
                ] {
                    let drawn = params.level_at(segment, u);
                    assert!(
                        (drawn - rendered).abs() < 2e-3,
                        "{segment:?} at {u} with curves {curves:?}: drawn {drawn}, rendered {rendered}"
                    );
                }
            }
            assert!((out[hold - 1] - params.level_at(AdsrSegment::Sustain, 0.0)).abs() < 1e-4);
        }
    }

    #[test]
    fn test_generate_adsr_curve_full_sustain() {
        let params = AdsrParams::new(0.01, 0.1, 1.0, 0.3);
        let curve = generate_adsr_curve(&params, 100);

        // Use helper to get segment boundaries with logarithmic scaling
        let (_, decay_end, sustain_end) = get_adsr_segment_boundaries(&params);

        // Filter for sustain region only (between decay_end and sustain_end)
        let sustain_points: Vec<f32> = curve
            .iter()
            .filter(|(x, _)| *x > decay_end + 0.02 && *x < sustain_end - 0.02)
            .map(|(_, y)| *y)
            .collect();

        // With full sustain, should stay at 1.0 during sustain phase
        if !sustain_points.is_empty() {
            for y in &sustain_points {
                assert!(*y > 0.9, "Full sustain should stay near 1.0, got {}", y);
            }
        }
    }
}
