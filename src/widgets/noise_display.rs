//! Noise display: the three colours of noise as an analyzer would read them.
//!
//! White, pink and brown fan out from one point on a log-frequency axis:
//! flat, falling 3 dB per octave, falling 6 dB per octave. Each line
//! shimmers the way a real reading of noise does, more at the low end,
//! where an analyzer averages fewer frequencies per octave, and settling
//! towards the top.

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};

use crate::app::theme;

/// The noise colours, back to front: brown, pink, white.
const LINES: [NoiseLine; 3] = [
    NoiseLine { slope_db: -6.0, color: Color32::from_rgb(46, 104, 196), label: "B" },
    NoiseLine { slope_db: -3.0, color: theme::module::SOURCE, label: "P" },
    NoiseLine { slope_db: 0.0, color: Color32::from_rgb(222, 236, 255), label: "W" },
];

struct NoiseLine {
    /// Fall per octave, in dB.
    slope_db: f32,
    color: Color32,
    label: &'static str,
}

/// Octaves across the display, 20 Hz to 20 kHz.
const OCTAVES: f32 = 10.0;
/// Points drawn per octave.
const POINTS_PER_OCTAVE: usize = 4;
/// Level range shown, in dB.
const TOP_DB: f32 = 4.0;
const BOTTOM_DB: f32 = -66.0;
/// How many fresh readings the shimmer takes each second.
const READINGS_PER_SECOND: f64 = 8.0;

/// What a Noise node's display shows.
#[derive(Clone, Copy, Debug)]
pub struct NoiseDisplayConfig {
    pub size: Vec2,
    /// The Level knob, 0 to 1. The lines sit lower as it falls.
    pub level: f32,
    /// Which outputs are patched, as (white, pink, brown). Unpatched lines
    /// dim while any other is patched.
    pub patched: [bool; 3],
    /// Whether the patch is playing. Stopped, the lines hold still.
    pub playing: bool,
}

/// Draws the noise display and, while playing, asks for the next frame of shimmer.
pub fn noise_display(ui: &mut Ui, config: &NoiseDisplayConfig) -> Response {
    let (rect, response) = ui.allocate_exact_size(config.size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let scale = config.size.y / 50.0;
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 2.0 * scale, Color32::from_rgb(20, 22, 30));

    // The lines stop short of the right edge, leaving a gutter for their names
    let plot = Rect::from_min_max(rect.min, Pos2::new(rect.right() - 11.0 * scale, rect.bottom()));
    let grid = Stroke::new(0.5 * scale, Color32::from_rgba_unmultiplied(255, 255, 255, 9));
    for octave in 1..=OCTAVES as usize {
        let x = plot.left() + plot.width() * octave as f32 / OCTAVES;
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
    }
    for db in [-20.0, -40.0, -60.0] {
        let y = db_to_y(db, rect);
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], grid);
    }

    // Between two readings the lines ease from one to the next, so the
    // shimmer breathes instead of strobing
    let time = if config.playing { ui.ctx().input(|i| i.time) * READINGS_PER_SECOND } else { 0.0 };
    let reading = time.floor() as u32;
    let blend = smoothstep(time.fract() as f32);

    let level_db = 20.0 * config.level.max(1e-4).log10();
    let any_patched = config.patched.iter().any(|&p| p);
    let fade = 0.35 + 0.65 * config.level.clamp(0.0, 1.0).sqrt();
    let points = OCTAVES as usize * POINTS_PER_OCTAVE + 1;

    for (index, line) in LINES.iter().enumerate() {
        // `patched` runs white, pink, brown; LINES runs the other way
        let lit = !any_patched || config.patched[2 - index];
        let alpha = if lit { fade } else { fade * 0.3 };

        let curve: Vec<Pos2> = (0..points)
            .map(|point| {
                let octave = point as f32 / POINTS_PER_OCTAVE as f32;
                let spread = 3.0 / (1.0 + 1.5 * octave).sqrt();
                let jitter = lerp(
                    shimmer(reading, index, point),
                    shimmer(reading + 1, index, point),
                    blend,
                );
                let db = level_db + line.slope_db * octave + spread * jitter;
                Pos2::new(plot.left() + plot.width() * octave / OCTAVES, db_to_y(db, rect))
            })
            .collect();

        if lit {
            painter.add(Shape::line(curve.clone(), Stroke::new(3.5 * scale, with_alpha(line.color, 0.18 * alpha))));
        }
        painter.add(Shape::line(curve.clone(), Stroke::new(1.2 * scale, with_alpha(line.color, alpha))));

        // Name each line in the gutter, level with its quiet end
        let end_db = level_db + line.slope_db * OCTAVES;
        let label_y = db_to_y(end_db, rect).clamp(rect.top() + 5.0 * scale, rect.bottom() - 5.0 * scale);
        painter.text(
            Pos2::new(rect.right() - 5.5 * scale, label_y),
            egui::Align2::CENTER_CENTER,
            line.label,
            egui::FontId::proportional(7.5 * scale),
            with_alpha(line.color, alpha * 0.85),
        );
    }

    painter.rect_stroke(rect, 2.0 * scale, Stroke::new(1.0 * scale, Color32::from_rgb(50, 55, 70)), egui::StrokeKind::Middle);

    if config.playing {
        crate::app::pace::animate(ui.ctx());
    }
    response
}

fn db_to_y(db: f32, rect: Rect) -> f32 {
    rect.top() + rect.height() * (TOP_DB - db) / (TOP_DB - BOTTOM_DB)
}

/// A repeatable random number in [-1, 1] for one point of one reading.
fn shimmer(reading: u32, line: usize, point: usize) -> f32 {
    let mut x = reading
        .wrapping_mul(0x9E37_79B1)
        ^ (line as u32).wrapping_mul(0x85EB_CA77)
        ^ (point as u32).wrapping_mul(0xC2B2_AE3D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    x as f32 / u32::MAX as f32 * 2.0 - 1.0
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shimmer_is_bounded_and_varied() {
        let values: Vec<f32> = (0..1000).map(|i| shimmer(i, i as usize % 3, i as usize % 41)).collect();
        assert!(values.iter().all(|v| (-1.0..=1.0).contains(v)));
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!(mean.abs() < 0.1, "shimmer mean {mean}");
        assert_eq!(shimmer(7, 1, 3), shimmer(7, 1, 3));
    }

    #[test]
    fn test_lines_fan_out_within_the_display() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(140.0, 50.0));
        // At full level the brown line's quiet end is still on screen
        let brown_end = db_to_y(LINES[0].slope_db * OCTAVES, rect);
        assert!(brown_end < rect.bottom());
        assert!(db_to_y(0.0, rect) > rect.top());
    }
}
