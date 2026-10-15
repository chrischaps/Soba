//! Rotary knob widget for audio parameters.
//!
//! Provides a 3D-styled knob with value display, drag interaction,
//! and fine control via Shift+drag.

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};
use std::ops::RangeInclusive;

use crate::app::theme;

/// Parameter display format for value formatting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamFormat {
    /// Raw numeric value with optional decimal places.
    Raw { decimals: usize },
    /// Raw numeric value with a custom unit suffix.
    RawWithUnit { decimals: usize, unit: &'static str },
    /// Percentage (0-100%).
    Percent,
    /// A share either way from -1 to 1, signed: "-20%", "0%", "+20%".
    BipolarPercent,
    /// Frequency in Hz/kHz.
    Frequency,
    /// Time in ms/s, from a value in seconds.
    Time,
    /// Time in ms/s, from a value in milliseconds.
    Milliseconds,
    /// Decibels (dB).
    Decibels,
    /// Semitones.
    Semitones,
    /// Stereo position from -1 (left) to 1 (right): "L 40", "C", "R 40".
    Pan,
    /// A MIDI note number, by name: "C4", "F#2".
    Note,
    /// How many slices a recording is cut into: "Off" below 2, and
    /// "Onsets" past 64.
    Slices,
}

impl ParamFormat {
    /// Format a value according to this format.
    pub fn format(&self, value: f32) -> String {
        match self {
            ParamFormat::Raw { decimals } => {
                format!("{:.prec$}", value, prec = decimals)
            }
            ParamFormat::RawWithUnit { decimals, unit } => {
                format!("{:.prec$} {}", value, unit, prec = decimals)
            }
            ParamFormat::Percent => {
                format!("{:.0}%", value * 100.0)
            }
            ParamFormat::BipolarPercent => {
                let percent = (value * 100.0).round();
                if percent > 0.0 {
                    format!("+{percent:.0}%")
                } else {
                    // -0.004 rounds to -0, which prints "-0%"
                    format!("{:.0}%", percent + 0.0)
                }
            }
            ParamFormat::Frequency => {
                if value >= 1000.0 {
                    format!("{:.2} kHz", value / 1000.0)
                } else if value >= 100.0 {
                    format!("{:.0} Hz", value)
                } else if value >= 10.0 {
                    format!("{:.1} Hz", value)
                } else {
                    format!("{:.2} Hz", value)
                }
            }
            ParamFormat::Time => {
                // A time of nothing (no glide, no slew, no pre-delay) is off
                if value <= 0.0 {
                    "Off".to_string()
                } else if value >= 1.0 {
                    format!("{:.2} s", value)
                } else if value >= 0.01 {
                    format!("{:.0} ms", value * 1000.0)
                } else {
                    format!("{:.1} ms", value * 1000.0)
                }
            }
            ParamFormat::Milliseconds => ParamFormat::Time.format(value / 1000.0),
            ParamFormat::Decibels => {
                if value <= -60.0 {
                    "-∞ dB".to_string()
                } else {
                    format!("{:.1} dB", value)
                }
            }
            ParamFormat::Note => crate::modules::sequencer::note_to_name(value.round().clamp(0.0, 127.0) as u8),
            ParamFormat::Slices => match value.round() {
                count if count < 2.0 => "Off".to_string(),
                count if count >= crate::dsp::primitives::ONSETS => "Onsets".to_string(),
                count => format!("{count:.0}"),
            },
            ParamFormat::Pan => {
                let percent = (value * 100.0).round();
                if percent == 0.0 {
                    "C".to_string()
                } else if percent < 0.0 {
                    format!("L {:.0}", -percent)
                } else {
                    format!("R {:.0}", percent)
                }
            }
            ParamFormat::Semitones => {
                if value >= 0.0 {
                    format!("+{:.0} st", value)
                } else {
                    format!("{:.0} st", value)
                }
            }
        }
    }
}

impl Default for ParamFormat {
    fn default() -> Self {
        ParamFormat::Raw { decimals: 2 }
    }
}

/// Configuration for the Knob widget.
#[derive(Clone)]
pub struct KnobConfig {
    /// Size of the knob (diameter).
    pub size: f32,
    /// Value range.
    pub range: RangeInclusive<f32>,
    /// Default value (for double-click reset).
    pub default: f32,
    /// Display format for the value.
    pub format: ParamFormat,
    /// Whether to use logarithmic scaling.
    pub logarithmic: bool,
    /// Label shown below the knob.
    pub label: Option<String>,
    /// Show value display.
    pub show_value: bool,
    /// Text to show instead of the formatted value, for a knob something
    /// else is in charge of (a synced LFO's division, say).
    pub value_text: Option<String>,
    /// Drag sensitivity (pixels per full range).
    pub drag_sensitivity: f32,
    /// Fine control multiplier when Shift is held.
    pub fine_multiplier: f32,
    /// Click between whole numbers.
    pub stepped: bool,
    /// Colour of the value arc, usually the module's category colour.
    pub accent: Color32,
    /// How the knob is drawn.
    pub style: KnobStyle,
    /// Where CV has carried a knob that keeps its own setting while the CV
    /// modulates it: drawn as a Control-coloured arc from the knob's value to
    /// here, with a bead at its end, over a faint swath of where the CV has
    /// been lately, and read under the dial while the knob isn't in hand.
    pub modulated: Option<f32>,
    /// How strongly, from 0 to 1, the knob is picked out as the one the
    /// hovered cable carries CV to: its bead swells and glows, or, with no
    /// bead to show, a ring lights around the dial.
    pub spotlight: f32,
}

impl Default for KnobConfig {
    fn default() -> Self {
        Self {
            size: 50.0,
            range: 0.0..=1.0,
            default: 0.5,
            format: ParamFormat::default(),
            logarithmic: false,
            label: None,
            show_value: true,
            value_text: None,
            drag_sensitivity: 200.0,
            fine_multiplier: 0.1,
            stepped: false,
            accent: theme::accent::PRIMARY,
            style: KnobStyle::default(),
            modulated: None,
            spotlight: 0.0,
        }
    }
}

/// How knobs are drawn, chosen from the toolbar's Knobs menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KnobStyle {
    /// A dark encoder inside a ring of LEDs that light up to the value.
    #[default]
    LedRing,
    /// The Arc's track and value, around a shaded cap with a long pointer.
    Hybrid,
    /// Flat cap inside a full-range track, the value arc on the track.
    Arc,
    /// A turned-metal cap with knurled edge, inside a ring of scale ticks.
    Machined,
    /// The original: shaded ball, inner arc, short notch.
    Classic,
}

impl KnobStyle {
    /// Every style, in menu order.
    pub const ALL: [KnobStyle; 5] = [
        KnobStyle::LedRing,
        KnobStyle::Hybrid,
        KnobStyle::Arc,
        KnobStyle::Machined,
        KnobStyle::Classic,
    ];

    /// The name shown in the menu, and kept in app storage.
    pub fn name(self) -> &'static str {
        match self {
            KnobStyle::LedRing => "LED ring",
            KnobStyle::Hybrid => "Hybrid",
            KnobStyle::Arc => "Arc",
            KnobStyle::Machined => "Machined",
            KnobStyle::Classic => "Classic",
        }
    }
}

impl KnobConfig {
    /// Create a frequency knob configuration.
    pub fn frequency(min: f32, max: f32, default: f32) -> Self {
        Self {
            range: min..=max,
            default,
            format: ParamFormat::Frequency,
            logarithmic: true,
            ..Default::default()
        }
    }

    /// Create a time knob configuration.
    pub fn time(min: f32, max: f32, default: f32) -> Self {
        Self {
            range: min..=max,
            default,
            format: ParamFormat::Time,
            logarithmic: false,
            ..Default::default()
        }
    }

    /// Create a percentage knob configuration.
    pub fn percent(default: f32) -> Self {
        Self {
            range: 0.0..=1.0,
            default,
            format: ParamFormat::Percent,
            ..Default::default()
        }
    }

    /// Create a decibel knob configuration.
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

    /// Set the size.
    pub fn with_size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// How far round its travel (0 to 1) the knob sits at `value`.
    pub fn travel(&self, value: f32) -> f32 {
        let (min, max) = (*self.range.start(), *self.range.end());
        if !self.logarithmic {
            (value - min) / (max - min)
        } else if min > 0.0 {
            (value.ln() - min.ln()) / (max.ln() - min.ln())
        } else {
            // From zero: log above the first few percent, reaching true zero
            (1.0 + value.max(0.0) / max * (ZERO_LOG_RATIO - 1.0)).ln() / ZERO_LOG_RATIO.ln()
        }
    }

    /// The value at `travel` (0 to 1) round a logarithmic knob.
    fn value_at(&self, travel: f32) -> f32 {
        let (min, max) = (*self.range.start(), *self.range.end());
        let value = if min > 0.0 {
            (min.ln() + travel * (max.ln() - min.ln())).exp()
        } else {
            max * (ZERO_LOG_RATIO.powf(travel) - 1.0) / (ZERO_LOG_RATIO - 1.0)
        };
        value.clamp(min, max)
    }
}

/// How a logarithmic knob that starts at zero spreads its travel: the
/// value at mid-travel is about a tenth of the top (2 s puts 180 ms there).
const ZERO_LOG_RATIO: f32 = 100.0;

/// A rotary knob widget for audio parameter control.
///
/// Features:
/// - 3D appearance with highlight and shadow
/// - Value indicator arc
/// - Drag up/down to change value
/// - Shift+drag for fine control
/// - Double-click to reset to default
pub fn knob(ui: &mut Ui, value: &mut f32, config: &KnobConfig) -> Response {
    let desired_size = Vec2::splat(config.size);
    // Scale label/value heights proportionally with knob size (base 36.0 -> 16.0 ratio)
    let text_height = config.size * (16.0 / 36.0);
    let total_height = config.size
        + if config.show_value { text_height } else { 0.0 }
        + if config.label.is_some() { text_height } else { 0.0 };

    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(config.size, total_height),
        Sense::click_and_drag(),
    );

    // Handle double-click to reset
    if response.double_clicked() {
        *value = config.default;
    }

    // Handle drag
    if response.dragged() {
        let delta = response.drag_delta();
        let sensitivity = if ui.input(|i| i.modifiers.shift) {
            config.drag_sensitivity / config.fine_multiplier
        } else {
            config.drag_sensitivity
        };

        // Vertical drag: up increases, down decreases
        let delta_normalized = -delta.y / sensitivity;

        // A stepped knob drags an unrounded value kept for the length of the
        // drag, so small movements add up to a step instead of rounding away
        let drag_id = response.id.with("unrounded");
        if config.stepped {
            let unrounded = if response.drag_started() {
                *value
            } else {
                ui.data(|d| d.get_temp::<f32>(drag_id)).unwrap_or(*value)
            };
            *value = unrounded;
        }

        if config.logarithmic {
            *value = config.value_at((config.travel(*value) + delta_normalized).clamp(0.0, 1.0));
        } else {
            // Linear scaling
            let range = config.range.end() - config.range.start();
            *value = (*value + delta_normalized * range)
                .clamp(*config.range.start(), *config.range.end());
        }

        if config.stepped {
            ui.data_mut(|d| d.insert_temp(drag_id, *value));
            *value = value.round();
        }
    }

    // Where the CV has been lately, kept between frames
    let swath_id = response.id.with("cv_swath");
    let swath = match config.modulated {
        Some(to) => {
            let (now, time) = (config.travel(to).clamp(0.0, 1.0), ui.input(|i| i.time));
            let swath = ui.data(|d| d.get_temp::<Swath>(swath_id)).map_or(Swath::at(now, time), |swath| swath.follow(now, time));
            ui.data_mut(|d| d.insert_temp(swath_id, swath));
            Some(swath)
        }
        None => {
            ui.data_mut(|d| d.remove::<Swath>(swath_id));
            None
        }
    };
    let hot = response.hovered() || response.dragged();

    // Draw the knob
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let knob_rect = Rect::from_min_size(rect.min, desired_size);
        let center = knob_rect.center();

        // Scale factor relative to default size (50.0) for proportional scaling
        let scale = config.size / 50.0;
        let radius = config.size / 2.0 - 2.0 * scale;

        // Normalize value for display (0.0 to 1.0)
        let normalized = config.travel(*value);

        // Angle calculation: start from bottom-left (-225°) to bottom-right (+45°)
        // Arc spans 270 degrees
        let start_angle = -225.0_f32.to_radians();
        let end_angle = 45.0_f32.to_radians();
        let angle = start_angle + normalized * (end_angle - start_angle);

        let style = config.style;
        if style != KnobStyle::Classic {
            let (min, max) = (*config.range.start(), *config.range.end());
            // A range either side of zero sweeps out from its middle
            let zero = (!config.logarithmic && min < 0.0 && max > 0.0).then(|| -min / (max - min));
            let face = Face {
                center,
                radius: config.size / 2.0 - config.size / 36.0,
                s: config.size / 36.0,
                value: normalized.clamp(0.0, 1.0),
                zero,
                accent: config.accent,
                hot,
                dragged: response.dragged(),
            };
            match style {
                KnobStyle::Arc => paint_arc(painter, &face),
                KnobStyle::Machined => paint_machined(painter, &face),
                KnobStyle::LedRing => paint_led_ring(painter, &face),
                KnobStyle::Hybrid => paint_hybrid(painter, &face),
                KnobStyle::Classic => unreachable!(),
            }
            paint_modulation(painter, center, config, normalized, swath);
            paint_readout(painter, knob_rect, face.s, config, *value, hot);
            return response;
        }

        // Draw outer ring (shadow)
        painter.circle(
            center + Vec2::new(1.0 * scale, 2.0 * scale),
            radius + 1.0 * scale,
            Color32::from_rgba_unmultiplied(0, 0, 0, 60),
            Stroke::NONE,
        );

        // Draw knob body (3D gradient effect using multiple circles)
        let base_color = if response.hovered() || response.dragged() {
            theme::background::WIDGET_HOVERED
        } else {
            theme::background::WIDGET
        };

        // Main body
        painter.circle_filled(center, radius, base_color);

        // Highlight (top-left)
        let highlight_offset = Vec2::new(-radius * 0.3, -radius * 0.3);
        let highlight_radius = radius * 0.5;
        painter.circle_filled(
            center + highlight_offset,
            highlight_radius,
            Color32::from_rgba_unmultiplied(255, 255, 255, 20),
        );

        // Inner shadow (bottom-right)
        let shadow_offset = Vec2::new(radius * 0.2, radius * 0.2);
        painter.circle_filled(
            center + shadow_offset,
            radius * 0.6,
            Color32::from_rgba_unmultiplied(0, 0, 0, 15),
        );

        // Draw value arc
        draw_value_arc(
            painter,
            center,
            radius - 4.0 * scale,
            start_angle,
            angle,
            theme::accent::PRIMARY,
            scale,
        );

        // Draw position indicator (notch)
        let notch_inner = radius - 12.0 * scale;
        let notch_outer = radius - 4.0 * scale;
        let notch_start = Pos2::new(
            center.x + notch_inner * angle.cos(),
            center.y + notch_inner * angle.sin(),
        );
        let notch_end = Pos2::new(
            center.x + notch_outer * angle.cos(),
            center.y + notch_outer * angle.sin(),
        );
        painter.line_segment(
            [notch_start, notch_end],
            Stroke::new(2.5 * scale, theme::text::PRIMARY),
        );

        // Draw center dot
        painter.circle_filled(center, 3.0 * scale, theme::text::SECONDARY);

        paint_modulation(painter, center, config, normalized, swath);

        // Draw outer ring border
        painter.circle_stroke(
            center,
            radius,
            Stroke::new(
                1.0 * scale,
                if response.has_focus() || response.dragged() {
                    theme::accent::PRIMARY
                } else {
                    theme::node::BODY_STROKE
                },
            ),
        );

        // Draw value text
        let mut text_y = knob_rect.bottom() + 2.0 * scale;
        if config.show_value {
            let (value_text, color) = readout_text(config, *value, hot);
            painter.text(
                Pos2::new(center.x, text_y + 6.0 * scale),
                egui::Align2::CENTER_CENTER,
                value_text,
                egui::FontId::proportional(11.0 * scale),
                color,
            );
            text_y += 14.0 * scale;
        }

        // Draw label
        if let Some(label) = &config.label {
            painter.text(
                Pos2::new(center.x, text_y + 6.0 * scale),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(10.0 * scale),
                theme::text::SECONDARY,
            );
        }
    }

    response
}

/// Draw a value arc on the knob.
fn draw_value_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    color: Color32,
    scale: f32,
) {
    let segments = 32;
    let arc_span = end_angle - start_angle;

    // Draw the arc as line segments
    if arc_span.abs() > 0.01 {
        let mut points = Vec::with_capacity(segments + 1);
        for i in 0..=segments {
            let t = i as f32 / segments as f32;
            let current_angle = start_angle + t * arc_span;
            points.push(Pos2::new(
                center.x + radius * current_angle.cos(),
                center.y + radius * current_angle.sin(),
            ));
        }

        for i in 0..points.len().saturating_sub(1) {
            painter.line_segment([points[i], points[i + 1]], Stroke::new(3.0 * scale, color));
        }
    }
}

/// What a knob face needs to know to draw itself
struct Face {
    center: Pos2,
    /// Outer radius, inside the widget's square
    radius: f32,
    /// Scale against a 36 pt knob, the size a node draws at zoom 1
    s: f32,
    /// Position in the range, 0 to 1
    value: f32,
    /// Where zero sits in a range either side of it
    zero: Option<f32>,
    accent: Color32,
    hot: bool,
    dragged: bool,
}

impl Face {
    /// Where the sweep starts: bottom left, 135° clockwise from 3 o'clock
    const START: f32 = 0.75 * std::f32::consts::PI;
    /// The sweep covers 270°
    const SWEEP: f32 = 1.5 * std::f32::consts::PI;

    fn angle(&self, t: f32) -> f32 {
        Self::START + t * Self::SWEEP
    }

    fn at(&self, t: f32, radius: f32) -> Pos2 {
        polar(self.center, self.angle(t), radius)
    }

    /// The lit part of the range: from zero out to the value
    fn span(&self) -> (f32, f32) {
        let from = self.zero.unwrap_or(0.0);
        (from.min(self.value), from.max(self.value))
    }

    /// The value arc, glowing while the knob is under the pointer
    fn paint_value(&self, painter: &egui::Painter, radius: f32, width: f32) {
        let (from, to) = self.span();
        if to - from < 1e-3 {
            return;
        }
        if self.hot {
            let glow = self.accent.gamma_multiply(if self.dragged { 0.28 } else { 0.16 });
            stroke_arc(painter, self.center, radius, self.angle(from), self.angle(to), width * 2.6, glow);
        }
        stroke_arc(painter, self.center, radius, self.angle(from), self.angle(to), width, self.accent);
    }

    /// The pointer, from near the middle of the cap to just inside its edge
    fn paint_pointer(&self, painter: &egui::Painter, inner: f32, outer: f32, width: f32, color: Color32) {
        let a = self.angle(self.value);
        painter.line_segment([polar(self.center, a, inner), polar(self.center, a, outer)], Stroke::new(width, color));
        painter.circle_filled(polar(self.center, a, outer), width / 2.0, color);
        painter.circle_filled(polar(self.center, a, inner), width / 2.0, color);
    }
}

fn polar(center: Pos2, angle: f32, radius: f32) -> Pos2 {
    center + Vec2::angled(angle) * radius
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// An arc with round ends
fn stroke_arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, width: f32, color: Color32) {
    let span = to - from;
    let steps = ((span.abs() * radius / 2.0).ceil() as usize).max(2);
    let points: Vec<Pos2> = (0..=steps)
        .map(|i| polar(center, from + span * i as f32 / steps as f32, radius))
        .collect();
    painter.circle_filled(points[0], width / 2.0, color);
    painter.circle_filled(points[steps], width / 2.0, color);
    if span.abs() > 1e-3 {
        painter.add(egui::Shape::line(points, Stroke::new(width, color)));
    }
}

/// A disc lit from above: `top` at its upper edge, fading to `bottom`
fn shaded_disc(painter: &egui::Painter, center: Pos2, radius: f32, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    let n = 48;
    mesh.colored_vertex(center, mix(top, bottom, 0.5));
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(polar(center, a, radius), mix(top, bottom, (a.sin() + 1.0) / 2.0));
    }
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(egui::Shape::mesh(mesh));
    // A mesh isn't anti-aliased; a hairline in the mid tone softens its edge
    painter.circle_stroke(center, radius, Stroke::new(1.0, mix(top, bottom, 0.55)));
}

const TRACK: Color32 = Color32::from_rgb(21, 23, 31);

/// Flat and modern: a quiet cap inside a full-range track
fn paint_arc(painter: &egui::Painter, f: &Face) {
    let s = f.s;
    let track_r = f.radius - 2.0 * s;
    stroke_arc(painter, f.center, track_r, f.angle(0.0), f.angle(1.0), 3.2 * s, TRACK);
    f.paint_value(painter, track_r, 3.2 * s);

    let cap_r = f.radius - 6.5 * s;
    let cap = if f.hot { Color32::from_rgb(70, 75, 94) } else { Color32::from_rgb(58, 62, 79) };
    painter.circle_filled(f.center, cap_r, cap);
    f.paint_pointer(painter, cap_r * 0.2, cap_r - 1.8 * s, 2.0 * s, theme::text::PRIMARY);
}

/// Hardware: a turned-metal cap with a knurled edge, in a ring of scale ticks
fn paint_machined(painter: &egui::Painter, f: &Face) {
    let s = f.s;
    let (from, to) = f.span();
    for i in 0..=10 {
        let t = i as f32 / 10.0;
        let lit = t >= from - 1e-3 && t <= to + 1e-3 && to - from > 1e-3;
        let color = if lit { f.accent } else { Color32::from_rgb(74, 78, 96) };
        painter.line_segment(
            [f.at(t, f.radius - 3.4 * s), f.at(t, f.radius)],
            Stroke::new(1.5 * s, color),
        );
    }

    let cap_r = f.radius - 5.0 * s;
    painter.circle_filled(f.center + Vec2::new(0.0, 2.2 * s), cap_r + 0.8 * s, Color32::from_black_alpha(45));
    painter.circle_filled(f.center + Vec2::new(0.0, 1.2 * s), cap_r, Color32::from_black_alpha(110));
    let top = if f.hot { Color32::from_rgb(134, 138, 156) } else { Color32::from_rgb(116, 120, 138) };
    shaded_disc(painter, f.center, cap_r, top, Color32::from_rgb(42, 45, 58));

    // Knurling turns with the knob
    let a = f.angle(f.value);
    for i in 0..28 {
        let ai = a + i as f32 * std::f32::consts::TAU / 28.0;
        painter.line_segment(
            [polar(f.center, ai, cap_r - 2.2 * s), polar(f.center, ai, cap_r - 0.4 * s)],
            Stroke::new(0.9 * s, Color32::from_black_alpha(80)),
        );
    }

    // A shallow dish in the face, lit from the opposite side
    let face_r = cap_r - 2.6 * s;
    shaded_disc(painter, f.center, face_r, Color32::from_rgb(58, 61, 76), Color32::from_rgb(94, 98, 116));
    painter.circle_stroke(f.center, cap_r, Stroke::new(0.8 * s, Color32::from_white_alpha(22)));
    f.paint_pointer(painter, face_r * 0.15, face_r - 0.6 * s, 2.2 * s, Color32::from_rgb(244, 244, 248));
}

/// An encoder inside a ring of LEDs that light up to the value
fn paint_led_ring(painter: &egui::Painter, f: &Face) {
    let s = f.s;
    painter.circle_filled(f.center, f.radius, Color32::from_rgb(17, 18, 25));
    painter.circle_stroke(f.center, f.radius, Stroke::new(1.0 * s, Color32::from_rgb(46, 50, 64)));

    // Each LED lights by how much of its share of the range is covered
    let (from, to) = f.span();
    let count = 19;
    let half = 0.5 / (count - 1) as f32;
    let ring_r = f.radius - 2.8 * s;
    for i in 0..count {
        let t = i as f32 / (count - 1) as f32;
        let covered = (to.min(t + half) - from.max(t - half)).max(0.0) / (2.0 * half);
        let level = covered.min(1.0);
        let pos = f.at(t, ring_r);
        if level > 0.0 {
            painter.circle_filled(pos, 2.9 * s, f.accent.gamma_multiply(0.22 * level));
        }
        painter.circle_filled(pos, 1.25 * s, mix(Color32::from_rgb(52, 55, 70), f.accent, level));
    }

    let cap_r = f.radius - 6.2 * s;
    let top = if f.hot { Color32::from_rgb(82, 86, 104) } else { Color32::from_rgb(70, 74, 90) };
    shaded_disc(painter, f.center, cap_r, top, Color32::from_rgb(32, 34, 45));
    f.paint_pointer(painter, cap_r - 5.0 * s, cap_r - 1.8 * s, 2.0 * s, theme::text::PRIMARY);
}

/// The Arc's track and value around a shaded cap, with a bead where the value ends
fn paint_hybrid(painter: &egui::Painter, f: &Face) {
    let s = f.s;
    let track_r = f.radius - 1.8 * s;
    stroke_arc(painter, f.center, track_r, f.angle(0.0), f.angle(1.0), 2.8 * s, TRACK);
    f.paint_value(painter, track_r, 2.8 * s);
    painter.circle_filled(f.at(f.value, track_r), 1.9 * s, Color32::WHITE);

    let cap_r = f.radius - 5.6 * s;
    painter.circle_filled(f.center + Vec2::new(0.0, 1.4 * s), cap_r + 0.4 * s, Color32::from_black_alpha(100));
    let top = if f.hot { Color32::from_rgb(112, 117, 138) } else { Color32::from_rgb(98, 102, 122) };
    shaded_disc(painter, f.center, cap_r, top, Color32::from_rgb(44, 47, 61));
    painter.circle_stroke(f.center, cap_r, Stroke::new(0.8 * s, Color32::from_white_alpha(20)));
    f.paint_pointer(painter, cap_r * 0.25, cap_r - 1.6 * s, 2.2 * s, theme::text::PRIMARY);
}

/// Where CV has carried the knob: a thin Control-coloured arc just outside
/// every style's face, from the knob's own value to the bead where the CV
/// takes it, over a faint swath of where it's been lately. The knob itself
/// stays where it was set
fn paint_modulation(painter: &egui::Painter, center: Pos2, config: &KnobConfig, from: f32, swath: Option<Swath>) {
    let s = config.size / 36.0;
    let spot = config.spotlight.clamp(0.0, 1.0);
    let Some(to) = config.modulated else {
        // Patched, but with no bead to light: a ring round the dial
        if spot > 0.0 {
            let radius = config.size / 2.0 - 0.6 * s;
            painter.circle_stroke(center, radius, Stroke::new(3.0 * s, theme::signal::CONTROL.gamma_multiply(0.18 * spot)));
            painter.circle_stroke(center, radius, Stroke::new(1.2 * s, theme::signal::CONTROL.gamma_multiply(0.8 * spot)));
        }
        return;
    };
    let (from, to) = (from.clamp(0.0, 1.0), config.travel(to).clamp(0.0, 1.0));
    let radius = config.size / 2.0 - 0.6 * s;
    let angle = |t: f32| Face::START + t * Face::SWEEP;
    let color = theme::signal::CONTROL;
    if let Some(swath) = swath.filter(|swath| swath.hi - swath.lo > 1e-3) {
        stroke_arc(painter, center, radius, angle(swath.lo), angle(swath.hi), 2.8 * s, color.gamma_multiply(0.2));
    }
    if (to - from).abs() > 1e-3 {
        stroke_arc(painter, center, radius, angle(from.min(to)), angle(from.max(to)), 1.2 * s, color.gamma_multiply(0.7));
    }
    let bead = polar(center, angle(to), radius);
    if spot > 0.0 {
        painter.circle_filled(bead, 6.0 * s * spot, color.gamma_multiply(0.22 * spot));
    }
    painter.circle_filled(bead, (2.6 + 1.0 * spot) * s, color.gamma_multiply(0.25 + 0.3 * spot));
    painter.circle_filled(bead, (1.5 + 0.5 * spot) * s, mix(color, Color32::WHITE, 0.35 * spot));
}

/// How long the swath takes to let go of where the CV has been: each end
/// falls about two thirds of the way back to the bead in this many seconds
const SWATH_FADE: f32 = 0.8;

/// The stretch of a knob's travel (0 to 1) its CV has covered lately: it
/// reaches out at once to wherever the bead goes, and draws back to it
/// slowly, so an LFO too quick for the bead to follow still shows its depth
#[derive(Clone, Copy, Debug, PartialEq)]
struct Swath {
    lo: f32,
    hi: f32,
    /// When it was last brought up to date, in seconds
    time: f64,
}

impl Swath {
    fn at(now: f32, time: f64) -> Self {
        Swath { lo: now, hi: now, time }
    }

    /// The swath at `time`, with the bead at `now`
    fn follow(self, now: f32, time: f64) -> Self {
        let keep = (-((time - self.time).max(0.0) as f32) / SWATH_FADE).exp();
        let back = |end: f32| now + (end - now) * keep;
        Swath { lo: back(self.lo).min(now), hi: back(self.hi).max(now), time }
    }
}

/// What a knob reads under its dial, and in what colour: what's in charge of
/// it if something is, where its CV has carried it (in the CV's orange), or,
/// while it's in hand, its own setting, which is what the hand is moving
fn readout_text(config: &KnobConfig, value: f32, hot: bool) -> (String, Color32) {
    match (&config.value_text, config.modulated) {
        (Some(text), _) => (text.clone(), theme::text::PRIMARY),
        (None, Some(to)) if !hot => (config.format.format(to), theme::signal::CONTROL),
        _ => (config.format.format(value), theme::text::PRIMARY),
    }
}

/// Value and label under the refreshed knobs: the value in the title face so
/// numbers read at a glance, the name quieter beneath it
fn paint_readout(painter: &egui::Painter, knob_rect: Rect, s: f32, config: &KnobConfig, value: f32, hot: bool) {
    let x = knob_rect.center().x;
    let mut y = knob_rect.bottom() + 1.0 * s;
    if config.show_value {
        let (text, color) = readout_text(config, value, hot);
        painter.text(
            Pos2::new(x, y + 6.5 * s),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::new(10.0 * s, egui::FontFamily::Name(theme::TITLE_FAMILY.into())),
            color,
        );
        y += 14.5 * s;
    }
    if let Some(label) = &config.label {
        painter.text(
            Pos2::new(x, y + 6.0 * s),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(9.0 * s),
            Color32::from_rgb(178, 181, 196),
        );
    }
}

/// A compact mini-knob variant for tight layouts.
///
/// Same functionality as the full knob but smaller and without
/// value display or label.
pub fn mini_knob(ui: &mut Ui, value: &mut f32, config: &KnobConfig) -> Response {
    let mini_config = KnobConfig {
        size: 28.0,
        show_value: false,
        label: None,
        ..config.clone()
    };
    knob(ui, value, &mini_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_param_format_percent() {
        assert_eq!(ParamFormat::Percent.format(0.5), "50%");
        assert_eq!(ParamFormat::Percent.format(1.0), "100%");
        assert_eq!(ParamFormat::Percent.format(0.0), "0%");
    }

    #[test]
    fn test_param_format_bipolar_percent() {
        assert_eq!(ParamFormat::BipolarPercent.format(0.2), "+20%");
        assert_eq!(ParamFormat::BipolarPercent.format(-0.6), "-60%");
        assert_eq!(ParamFormat::BipolarPercent.format(0.0), "0%");
        assert_eq!(ParamFormat::BipolarPercent.format(-0.004), "0%");
        assert_eq!(ParamFormat::BipolarPercent.format(-1.0), "-100%");
    }

    #[test]
    fn test_param_format_frequency() {
        assert_eq!(ParamFormat::Frequency.format(440.0), "440 Hz");
        assert_eq!(ParamFormat::Frequency.format(1000.0), "1.00 kHz");
        assert_eq!(ParamFormat::Frequency.format(20000.0), "20.00 kHz");
        assert_eq!(ParamFormat::Frequency.format(50.0), "50.0 Hz");
        assert_eq!(ParamFormat::Frequency.format(5.0), "5.00 Hz");
    }

    #[test]
    fn test_param_format_time() {
        assert_eq!(ParamFormat::Time.format(1.0), "1.00 s");
        assert_eq!(ParamFormat::Time.format(0.5), "500 ms");
        assert_eq!(ParamFormat::Time.format(0.001), "1.0 ms");
        assert_eq!(ParamFormat::Milliseconds.format(500.0), "500 ms");
        assert_eq!(ParamFormat::Milliseconds.format(1500.0), "1.50 s");
        assert_eq!(ParamFormat::Time.format(0.0), "Off");
        assert_eq!(ParamFormat::Milliseconds.format(0.0), "Off");
    }

    #[test]
    fn test_param_format_slices() {
        assert_eq!(ParamFormat::Slices.format(1.0), "Off");
        assert_eq!(ParamFormat::Slices.format(2.0), "2");
        assert_eq!(ParamFormat::Slices.format(11.6), "12");
        assert_eq!(ParamFormat::Slices.format(64.0), "64");
        assert_eq!(ParamFormat::Slices.format(65.0), "Onsets");
    }

    #[test]
    fn test_param_format_decibels() {
        assert_eq!(ParamFormat::Decibels.format(0.0), "0.0 dB");
        assert_eq!(ParamFormat::Decibels.format(-6.0), "-6.0 dB");
        assert_eq!(ParamFormat::Decibels.format(-70.0), "-∞ dB");
    }

    #[test]
    fn test_param_format_pan() {
        assert_eq!(ParamFormat::Pan.format(0.0), "C");
        assert_eq!(ParamFormat::Pan.format(-0.004), "C");
        assert_eq!(ParamFormat::Pan.format(-0.4), "L 40");
        assert_eq!(ParamFormat::Pan.format(1.0), "R 100");
    }

    #[test]
    fn test_param_format_semitones() {
        assert_eq!(ParamFormat::Semitones.format(0.0), "+0 st");
        assert_eq!(ParamFormat::Semitones.format(12.0), "+12 st");
        assert_eq!(ParamFormat::Semitones.format(-7.0), "-7 st");
    }

    #[test]
    fn test_knob_config_default() {
        let config = KnobConfig::default();
        assert_eq!(config.size, 50.0);
        assert_eq!(*config.range.start(), 0.0);
        assert_eq!(*config.range.end(), 1.0);
        assert!(!config.logarithmic);
    }

    #[test]
    fn test_knob_config_frequency() {
        let config = KnobConfig::frequency(20.0, 20000.0, 440.0);
        assert_eq!(*config.range.start(), 20.0);
        assert_eq!(*config.range.end(), 20000.0);
        assert_eq!(config.default, 440.0);
        assert!(config.logarithmic);
        assert_eq!(config.format, ParamFormat::Frequency);
    }

    #[test]
    fn test_knob_config_with_label() {
        let config = KnobConfig::default().with_label("Volume");
        assert_eq!(config.label, Some("Volume".to_string()));
    }

    #[test]
    fn test_knob_config_with_size() {
        let config = KnobConfig::default().with_size(60.0);
        assert_eq!(config.size, 60.0);
    }

    #[test]
    fn test_log_travel_from_zero() {
        let config = KnobConfig { range: 0.0..=2.0, logarithmic: true, ..Default::default() };
        assert_eq!(config.travel(0.0), 0.0);
        assert!((config.travel(2.0) - 1.0).abs() < 1e-6);
        let middle = config.value_at(0.5);
        assert!((middle - 0.18).abs() < 0.01, "mid-travel is {middle} s");
        for value in [0.0, 0.01, 0.2, 1.5] {
            assert!((config.value_at(config.travel(value)) - value).abs() < 1e-5);
        }
        assert_eq!(config.value_at(0.0), 0.0, "all the way down is exactly zero");
    }

    #[test]
    fn test_swath_reaches_out_at_once_and_lets_go_slowly() {
        let swath = Swath::at(0.5, 0.0).follow(0.8, 0.02);
        assert_eq!(swath.hi, 0.8, "out to the bead in a frame");
        assert!(swath.lo < 0.51, "and only just letting go of where it was: {swath:?}");
        let swath = swath.follow(0.3, 0.04);
        assert!((swath.lo - 0.3).abs() < 1e-6 && swath.hi > 0.78, "both ends held: {swath:?}");
        let swath = swath.follow(0.3, 0.04 + SWATH_FADE as f64 * 3.0);
        assert!(swath.hi - swath.lo < 0.03, "three fades on, back to the bead: {swath:?}");
    }

    #[test]
    fn test_readout_reads_the_cv_until_the_knob_is_in_hand() {
        let config = KnobConfig { modulated: Some(0.75), ..KnobConfig::percent(0.5) };
        assert_eq!(readout_text(&config, 0.5, false), ("75%".to_string(), theme::signal::CONTROL));
        assert_eq!(readout_text(&config, 0.5, true), ("50%".to_string(), theme::text::PRIMARY));
        assert_eq!(readout_text(&KnobConfig::percent(0.5), 0.5, false).0, "50%");
    }

    #[test]
    fn test_log_travel_positive_range() {
        let config = KnobConfig::frequency(20.0, 20000.0, 1000.0);
        assert!((config.travel(632.456) - 0.5).abs() < 1e-4);
        assert!((config.value_at(0.5) - 632.456).abs() < 0.01);
    }
}
