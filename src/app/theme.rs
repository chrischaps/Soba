//! Theme definitions for the Soba UI
//!
//! Provides color constants, styling utilities, and theme configuration
//! for a dark, audio-software aesthetic.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Vec2};

/// Background colors
pub mod background {
    use super::Color32;

    /// Main window background - deep dark blue
    pub const MAIN: Color32 = Color32::from_rgb(26, 26, 46);

    /// Grid line color - subtle
    pub const GRID: Color32 = Color32::from_rgb(36, 36, 58);

    /// Every fifth grid line, a little brighter, so distances read at a glance
    pub const GRID_MAJOR: Color32 = Color32::from_rgb(46, 46, 74);

    /// Panel background - slightly lighter than main
    pub const PANEL: Color32 = Color32::from_rgb(35, 35, 55);

    /// Widget background (buttons, inputs)
    pub const WIDGET: Color32 = Color32::from_rgb(45, 45, 70);

    /// Widget background when hovered
    pub const WIDGET_HOVERED: Color32 = Color32::from_rgb(55, 55, 85);

    /// Widget background when active/pressed
    pub const WIDGET_ACTIVE: Color32 = Color32::from_rgb(65, 65, 100);
}

/// Signal type colors, used for cables, jacks and signal displays. These are
/// [`SignalType::color`], so a waveform is drawn in its cable's colour.
pub mod signal {
    use super::Color32;
    use crate::dsp::SignalType;

    /// Audio signal - blue
    pub const AUDIO: Color32 = SignalType::Audio.color();

    /// Control/CV signal - orange
    pub const CONTROL: Color32 = SignalType::Control.color();

    /// Gate signal - green
    pub const GATE: Color32 = SignalType::Gate.color();

    /// MIDI signal - purple
    pub const MIDI: Color32 = SignalType::Midi.color();

    /// Mixer bus - pale steel
    pub const BUS: Color32 = SignalType::Bus.color();
}

/// Module header colors by category. These are [`ModuleCategory::color`].
pub mod module {
    use super::Color32;
    use crate::dsp::ModuleCategory;

    /// Source modules (oscillators) - blue
    pub const SOURCE: Color32 = ModuleCategory::Source.color();

    /// Filter modules - teal
    pub const FILTER: Color32 = ModuleCategory::Filter.color();

    /// Modulation modules (envelopes, LFOs) - orange
    pub const MODULATION: Color32 = ModuleCategory::Modulation.color();

    /// Output modules - purple
    pub const OUTPUT: Color32 = ModuleCategory::Output.color();

    /// Utility modules - gray
    pub const UTILITY: Color32 = ModuleCategory::Utility.color();

    /// Effect modules - cyan
    pub const EFFECT: Color32 = ModuleCategory::Effect.color();

    /// Groups, and the modules you've made from them - rose, the one header
    /// colour no built-in module wears
    pub const GROUP: Color32 = Color32::from_rgb(236, 127, 169);
}

/// Node styling constants
pub mod node {
    use super::Color32;

    /// Node body background color (dark)
    pub const BODY_FILL: Color32 = Color32::from_rgb(35, 38, 48);

    /// Node body border color
    pub const BODY_STROKE: Color32 = Color32::from_rgb(55, 60, 75);

    /// Node body border when selected
    pub const SELECTED_STROKE: Color32 = Color32::from_rgb(100, 150, 255);

    /// Node shadow color (use shadow() function for alpha)
    pub const SHADOW_BASE: Color32 = Color32::from_rgb(0, 0, 0);

    /// Get shadow color with alpha
    pub fn shadow() -> Color32 {
        Color32::from_rgba_unmultiplied(0, 0, 0, 80)
    }

    /// Port connector size
    pub const PORT_RADIUS: f32 = 6.0;

    /// Port connector highlight (metallic effect)
    pub const PORT_HIGHLIGHT: Color32 = Color32::from_rgb(200, 210, 220);

    /// Port connector shadow
    pub const PORT_SHADOW: Color32 = Color32::from_rgb(40, 45, 55);
}

/// Cable styling constants
pub mod cable {
    /// Cable thickness
    pub const THICKNESS: f32 = 3.0;

    /// Cable glow radius
    pub const GLOW_RADIUS: f32 = 6.0;

    /// Cable curvature factor (0.0 = straight, 1.0 = very curved)
    pub const CURVATURE: f32 = 0.5;
}

/// Text colors
pub mod text {
    use super::Color32;

    /// Primary text - bright white
    pub const PRIMARY: Color32 = Color32::from_rgb(240, 240, 245);

    /// Secondary text - dimmed
    pub const SECONDARY: Color32 = Color32::from_rgb(160, 160, 175);

    /// Quiet text: information that steps back without fading out, such as bar and lane
    /// numbers, column titles, units and hints. About 4.5:1 on a node body (#329)
    pub const QUIET: Color32 = Color32::from_rgb(140, 140, 155);

    /// Disabled text: controls that can't be used and things that are off or absent.
    /// Too faint (about 2.6:1) for anything that must be read
    pub const DISABLED: Color32 = Color32::from_rgb(100, 100, 115);

    /// Accent/highlight text
    pub const ACCENT: Color32 = Color32::from_rgb(130, 180, 255);
}

/// UI accent colors
pub mod accent {
    use super::Color32;

    /// Primary accent - blue
    pub const PRIMARY: Color32 = Color32::from_rgb(66, 165, 245);

    /// Success/active - green
    pub const SUCCESS: Color32 = Color32::from_rgb(129, 199, 132);

    /// Warning - orange
    pub const WARNING: Color32 = Color32::from_rgb(255, 183, 77);

    /// Error - red
    pub const ERROR: Color32 = Color32::from_rgb(239, 83, 80);
}

/// Grid spacing for the background pattern, in unzoomed points
pub const GRID_SPACING: f32 = 20.0;

/// Every this many grid lines is a major line
pub const GRID_MAJOR_EVERY: i64 = 5;

/// Standard rounding for UI elements
pub const ROUNDING: CornerRadius = CornerRadius::same(6);

/// Smaller rounding for compact elements
pub const ROUNDING_SMALL: CornerRadius = CornerRadius::same(4);

/// Module titles are set in Space Grotesk SemiBold: heavy enough to hold its
/// own on a bright header, with a little of an instrument panel's character
const TITLE_FONT: &[u8] = include_bytes!("../../assets/fonts/SpaceGrotesk-SemiBold.ttf");

/// The family module titles are set in
pub const TITLE_FAMILY: &str = "Title";

/// The text style module titles are set in
pub fn title_text_style() -> egui::TextStyle {
    egui::TextStyle::Name(TITLE_FAMILY.into())
}

/// Installs the title face, and a fallback for the filled status dot. Fonts
/// take a frame to arrive, so this runs when the app is created, before
/// anything asks for the family. The title family falls back to the default
/// faces for any glyph its Latin subset lacks.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert(TITLE_FAMILY.to_owned(), std::sync::Arc::new(egui::FontData::from_static(TITLE_FONT)));
    // None of the default proportional faces has a filled circle (●, U+25CF)
    // used for status dots; the bundled monospace face does, so it goes last
    let proportional = fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap();
    proportional.push("Hack".to_owned());
    let mut family = vec![TITLE_FAMILY.to_owned()];
    family.extend(fonts.families[&egui::FontFamily::Proportional].iter().cloned());
    fonts.families.insert(egui::FontFamily::Name(TITLE_FAMILY.into()), family);
    ctx.set_fonts(fonts);
}

/// Apply the dark synth theme to an egui context
pub fn apply_theme(ctx: &egui::Context) {
    // Always dark, whatever the system's preference: a browser in light
    // mode would otherwise start this from egui's light style
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.global_style()).clone();

    // Labels are names, not text to select. Selectable labels also take
    // clicks, so right-clicking a port's name wouldn't reach its module
    style.interaction.selectable_labels = false;

    // egui 0.35 doubled its animations to 0.2 s. Menus and hovers keep the
    // snap they had, and the capture scripts' stills land where they did
    style.animation_time = 0.1;

    // Visuals
    let visuals = &mut style.visuals;
    visuals.dark_mode = true;

    // Window styling
    visuals.window_fill = background::PANEL;
    visuals.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(60, 60, 80));
    visuals.window_corner_radius = ROUNDING;

    // Panel styling
    visuals.panel_fill = background::MAIN;

    // Widget styling
    visuals.widgets.noninteractive.bg_fill = background::WIDGET;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text::SECONDARY);
    visuals.widgets.noninteractive.corner_radius = ROUNDING_SMALL;

    visuals.widgets.inactive.bg_fill = background::WIDGET;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text::PRIMARY);
    visuals.widgets.inactive.corner_radius = ROUNDING_SMALL;

    visuals.widgets.hovered.bg_fill = background::WIDGET_HOVERED;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text::PRIMARY);
    visuals.widgets.hovered.corner_radius = ROUNDING_SMALL;

    visuals.widgets.active.bg_fill = background::WIDGET_ACTIVE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, accent::PRIMARY);
    visuals.widgets.active.corner_radius = ROUNDING_SMALL;

    visuals.widgets.open.bg_fill = background::WIDGET_ACTIVE;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, text::PRIMARY);
    visuals.widgets.open.corner_radius = ROUNDING_SMALL;

    // Selection styling
    visuals.selection.bg_fill = accent::PRIMARY.gamma_multiply(0.3);
    visuals.selection.stroke = Stroke::new(1.0_f32, accent::PRIMARY);

    // Hyperlink color
    visuals.hyperlink_color = text::ACCENT;

    // Extreme background (for things like text edit backgrounds)
    visuals.extreme_bg_color = Color32::from_rgb(20, 20, 35);

    // Faint background for code/monospace
    visuals.code_bg_color = Color32::from_rgb(35, 35, 50);

    // Spacing
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(12);
    // egui 0.35 made each row 20 points: the faces are laid out at 18
    style.spacing.interact_size.y = 18.0;

    style.text_styles.insert(
        title_text_style(),
        egui::FontId::new(14.0, egui::FontFamily::Name(TITLE_FAMILY.into())),
    );

    ctx.set_global_style(style);

    // egui 0.31 snaps rects and straight lines to whole pixels. The patch
    // zooms smoothly, and parts of a face snapping one by one would shift
    // against each other a pixel at a time as it does (#255), so shapes are
    // drawn where they fall, as they were before
    ctx.tessellation_options_mut(|options| {
        options.round_rects_to_pixels = false;
        options.round_line_segments_to_pixels = false;
    });
}

/// Draws the editor background: the main fill and a grid that pans and
/// zooms with the patch. `origin` is where the patch's (0, 0) is on screen.
pub fn draw_grid_background(painter: &egui::Painter, rect: egui::Rect, origin: egui::Pos2, zoom: f32) {
    painter.rect_filled(rect, 0.0, background::MAIN);

    let spacing = GRID_SPACING * zoom;
    if spacing < 1.0 {
        return;
    }
    // Minor lines fade out as zooming out crowds them together
    let minor = background::GRID.gamma_multiply(((spacing - 6.0) / 10.0).clamp(0.0, 1.0));
    let pixel = 1.0 / painter.ctx().pixels_per_point();
    // Centred on a pixel, so a 1px line stays crisp instead of smearing over two
    let snap = |v: f32| (v / pixel).floor() * pixel + pixel * 0.5;

    // Positions of the lines across one axis, each with its colour
    let lines = |from: f32, to: f32, origin: f32| {
        let first = ((from - origin) / spacing).ceil() as i64;
        let last = ((to - origin) / spacing).floor() as i64;
        (first..=last).filter_map(move |k| {
            let color = if k.rem_euclid(GRID_MAJOR_EVERY) == 0 { background::GRID_MAJOR } else { minor };
            (color.a() > 0).then(|| (snap(origin + k as f32 * spacing), color))
        })
    };

    for (x, color) in lines(rect.left(), rect.right(), origin.x) {
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], Stroke::new(pixel, color));
    }
    for (y, color) in lines(rect.top(), rect.bottom(), origin.y) {
        painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], Stroke::new(pixel, color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_colors_are_distinct() {
        // Ensure signal colors are visually distinct
        assert_ne!(signal::AUDIO, signal::CONTROL);
        assert_ne!(signal::AUDIO, signal::GATE);
        assert_ne!(signal::AUDIO, signal::MIDI);
        assert_ne!(signal::CONTROL, signal::GATE);
        assert_ne!(signal::CONTROL, signal::MIDI);
        assert_ne!(signal::GATE, signal::MIDI);
    }

    #[test]
    fn module_colors_are_distinct() {
        // Module category colors should all be visually distinct
        let colors = [
            module::SOURCE,
            module::FILTER,
            module::MODULATION,
            module::EFFECT,
            module::UTILITY,
            module::OUTPUT,
        ];

        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(colors[i], colors[j], "Module colors should be unique");
            }
        }
    }

    #[test]
    fn node_styling_constants_exist() {
        // Verify node styling constants are defined and reasonable
        const {
            assert!(node::PORT_RADIUS > 0.0);
            assert!(cable::THICKNESS > 0.0);
            assert!(cable::GLOW_RADIUS > cable::THICKNESS);
        }
    }

    #[test]
    fn grid_spacing_is_reasonable() {
        const { assert!(GRID_SPACING >= 10.0 && GRID_SPACING <= 50.0) };
    }

    /// WCAG contrast ratio of two opaque colours
    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |c: Color32| {
            let linear = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn quiet_text_is_readable_where_it_is_drawn() {
        for back in [node::BODY_FILL, background::PANEL, background::MAIN] {
            assert!(contrast(text::QUIET, back) >= 4.5, "{back:?}: {:.2}", contrast(text::QUIET, back));
        }
        // ...and still sits between disabled and secondary
        assert!(contrast(text::DISABLED, node::BODY_FILL) < contrast(text::QUIET, node::BODY_FILL));
        assert!(contrast(text::QUIET, node::BODY_FILL) < contrast(text::SECONDARY, node::BODY_FILL));
    }
}
