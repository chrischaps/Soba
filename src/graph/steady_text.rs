//! Text that keeps its size on the patch as the view zooms.
//!
//! The canvas zooms by laying itself out again with every font scaled, and
//! epaint rounds each font to whole pixels. So a row as tall as its text
//! grows in steps while everything around it grows smoothly: in patch
//! units it shrinks as the view zooms in, then jumps back as the font
//! gains a pixel, and a stack of such rows saws the node's height (#255).
//! These give a row of text the height the text would have if fonts
//! scaled smoothly, and centre the text in it.

use eframe::egui::{pos2, vec2, Color32, Context, FontId, Response, Sense, TextStyle, TextWrapMode, Ui, Vec2, Widget, WidgetText};

/// The size fonts are measured at: big enough that rounding it to whole
/// pixels doesn't matter.
const MEASURED_AT: f32 = 100.0;

/// How tall a row of `font` is, scaled smoothly from its size.
pub fn row_height(ui: &Ui, font: &FontId) -> f32 {
    let big = FontId::new(MEASURED_AT, font.family.clone());
    ui.fonts_mut(|f| f.row_height(&big)) * font.size / MEASURED_AT
}

/// The size text is measured across at. Every glyph measured is drawn
/// into the font atlas at this size, so it's kept modest: what matters is
/// that the measure is the same at every zoom.
const WIDTH_MEASURED_AT: f32 = 24.0;

/// How wide `text` is in `font`, scaled smoothly from its size.
pub fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    let measured = FontId::new(WIDTH_MEASURED_AT, font.family.clone());
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), measured, Color32::WHITE).size().x) * font.size / WIDTH_MEASURED_AT
}

/// Adds `widget` filling a slot `size` big. Its padding goes, so as long
/// as its text fits, the slot is all the room it takes, at every zoom.
pub fn slot(ui: &mut Ui, size: Vec2, widget: impl Widget) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size = size;
        ui.spacing_mut().button_padding = Vec2::ZERO;
        ui.add_sized(size, widget)
    })
    .inner
}

/// A label, as `ui.label` draws it, taking the room its text would if
/// fonts scaled smoothly. The text itself is drawn at a whole number of
/// pixels, so it can stand a hair past the room's right edge.
pub fn label(ui: &mut Ui, text: impl Into<WidgetText>, sense: Sense) -> Response {
    laid_label(ui, text.into(), TextWrapMode::Extend, sense)
}

/// A [`label`] cut short with "…" where it would run past the room left.
pub fn truncated_label(ui: &mut Ui, text: impl Into<WidgetText>, sense: Sense) -> Response {
    laid_label(ui, text.into(), TextWrapMode::Truncate, sense)
}

fn laid_label(ui: &mut Ui, text: WidgetText, wrap: TextWrapMode, sense: Sense) -> Response {
    let galley = text.into_galley(ui, Some(wrap), ui.available_width(), TextStyle::Body);
    let font = galley.job.sections.first().map_or_else(|| TextStyle::Body.resolve(ui.style()), |s| s.format.font_id.clone());
    let rows = galley.rows.len().max(1) as f32;
    // As wide as the text would be if fonts scaled smoothly, or as it's
    // been cut short to
    let width = if galley.elided { galley.size().x } else { text_width(ui, &galley.job.text, &font) };
    let (rect, response) = ui.allocate_exact_size(vec2(width, row_height(ui, &font) * rows), sense);
    if ui.is_rect_visible(rect) {
        let ink = if sense.interactive() { ui.style().interact(&response).text_color() } else { ui.visuals().text_color() };
        let at = pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(at, galley, ink);
    }
    response
}

/// Buttons and number fields in `ui` keep their height as the view zooms:
/// their padding takes up whatever their text gains or loses by being
/// drawn at a whole number of pixels. (Small buttons have no padding to
/// take it up with.)
pub fn steady_buttons(ui: &mut Ui) {
    steady_padding(ui, 0.0);
}

/// Dropdowns in `ui` keep their height as the view zooms, as
/// [`steady_buttons`] keeps buttons'. A dropdown is as tall as its text or
/// its arrow, whichever is taller.
pub fn steady_dropdowns(ui: &mut Ui) {
    steady_padding(ui, ui.spacing().icon_width);
}

fn steady_padding(ui: &mut Ui, icon: f32) {
    let font = TextStyle::Button.resolve(ui.style());
    let drawn = ui.painter().layout_no_wrap("Ag".into(), font.clone(), Color32::WHITE).size().y;
    ui.spacing_mut().button_padding.y += (row_height(ui, &font).max(icon) - drawn.max(icon)) / 2.0;
}


/// How wide a dropdown showing `selected` is, at least `least`: as wide as
/// its text would be if fonts scaled smoothly, with its arrow and padding.
/// Shown in a [`ui.allocate_ui`](Ui::allocate_ui) this wide, with
/// `ComboBox::width` set to it and `truncate()`, it keeps that width as the
/// view zooms, cutting its text short in the rare zoom it wouldn't fit.
pub fn dropdown_width(ui: &Ui, selected: &str, least: f32) -> f32 {
    let spacing = ui.spacing();
    let font = TextStyle::Button.resolve(ui.style());
    let text = text_width(ui, selected, &font) * 1.08;
    (text + spacing.icon_spacing + spacing.icon_width + 2.0 * spacing.button_padding.x).max(least)
}

/// How tall a dropdown is in `ui`, once [`steady_dropdowns`] has given it
/// its padding: as tall as its text or arrow, and that padding.
pub fn dropdown_height(ui: &Ui) -> f32 {
    let font = TextStyle::Button.resolve(ui.style());
    let drawn = ui.painter().layout_no_wrap("Ag".into(), font, Color32::WHITE).size().y;
    drawn.max(ui.spacing().icon_width) + 2.0 * ui.spacing().button_padding.y
}

/// `text` cut short with "…" to fit `width` in `font`, measured as
/// [`text_width`] measures it so the cut stays put as the view zooms. The
/// cut is found by halving, a handful of layouts at the measuring size
/// that epaint's galley cache keeps from one frame to the next.
pub fn fit(ctx: &Context, text: &str, font: &FontId, width: f32) -> String {
    let measured = FontId::new(WIDTH_MEASURED_AT, font.family.clone());
    let room = width * WIDTH_MEASURED_AT / font.size;
    let wide = |s: String| ctx.fonts_mut(|f| f.layout_no_wrap(s, measured.clone(), Color32::WHITE).size().x);
    if wide(text.to_string()) <= room {
        return text.to_string();
    }
    let shortened = |end: usize| text[..end].trim_end().to_string() + "…";
    // The ends of the first 0, 1, 2… characters: how much of `text` to keep
    let ends: Vec<usize> = std::iter::once(0).chain(text.char_indices().skip(1).map(|(i, _)| i)).collect();
    let fits = ends.partition_point(|&end| wide(shortened(end)) <= room);
    match fits {
        0 => String::new(),
        n => shortened(ends[n - 1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `check` in a frame, once fonts are loaded.
    fn in_a_frame(check: impl Fn(&Context)) {
        let ctx = Context::default();
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        ctx.run_ui(Default::default(), |ui| check(ui.ctx())).textures_delta.clear();
    }

    #[test]
    fn fit_keeps_what_fits_and_cuts_the_rest() {
        in_a_frame(|ctx| {
            let font = FontId::proportional(12.0);
            assert_eq!(fit(ctx, "Verse", &font, 1000.0), "Verse");
            assert_eq!(fit(ctx, "Verse", &font, 0.0), "");
            let cut = fit(ctx, "Chorus with strings", &font, 60.0);
            assert!(cut.ends_with('…') && cut.len() < "Chorus with strings…".len(), "{cut}");
            assert!(!cut.trim_end_matches('…').ends_with(' '), "no space before the ellipsis: {cut:?}");
        });
    }

    #[test]
    fn fit_cuts_in_the_same_place_at_every_zoom() {
        in_a_frame(|ctx| {
            let text = "Breakdown — strings only";
            let at = |zoom: f32| fit(ctx, text, &FontId::proportional(12.0 * zoom), 70.0 * zoom);
            let first = at(1.0);
            for step in 0..40 {
                assert_eq!(at(0.5 + step as f32 * 0.05), first);
            }
        });
    }

    #[test]
    fn fit_never_splits_a_character() {
        in_a_frame(|ctx| {
            let font = FontId::proportional(12.0);
            for width in 0..80 {
                let _ = fit(ctx, "ÄÖÜ — ✦✦✦ ñandú", &font, width as f32);
            }
        });
    }
}
