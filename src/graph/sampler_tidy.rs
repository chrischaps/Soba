//! Trim and Normalize on the Sampler's face.
//!
//! The file's name, under the waveform, opens a menu: Trim to the sound,
//! and Normalize to a peak of −1 dBFS or an RMS of −16 dBFS (see
//! [`crate::dsp::tidy`]). Each item says what it would do before it's
//! chosen: how much each end loses, or how many dB the level moves. Hovered,
//! the waveform shows it too. A trim veils what it would cut, hatched, and
//! sets crop marks at the corners of what it keeps. A level shows its change
//! across the panel, since the waveform is always drawn to fill it.
//!
//! A file kept only in the browser's tab (a take recorded there, or a file
//! tidied there) can be downloaded from the same menu, to keep it.

use eframe::egui::{self, Color32, PopupCloseBehavior, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::tidy::{minus, signed_db, Level, Survey, PEAK_CEILING_DB, TARGET_RMS_DB};

use super::sample_shelf::ShelvedSample;
use super::{SynthResponse, Tidy};

/// Changes of level smaller than this aren't worth a new file.
const NO_CHANGE_DB: f32 = 0.05;

fn popup_id(node_id: NodeId) -> egui::Id {
    egui::Id::new(("sampler_tidy_menu", node_id))
}

fn preview_id(node_id: NodeId) -> egui::Id {
    egui::Id::new(("sampler_tidy_preview", node_id))
}

/// What the menu's pointer is on, for the waveform to show.
pub fn previewing(ui: &egui::Ui, node_id: NodeId) -> Option<Tidy> {
    ui.data(|d| d.get_temp(preview_id(node_id)))
}

/// The file's name and length on the row under the waveform, as a button
/// that opens the menu. The name is cut short to leave room for the rest.
/// A file `in_tab`, kept only in the browser's tab, can be downloaded too.
#[allow(clippy::too_many_arguments)]
pub fn name_button(
    ui: &mut egui::Ui,
    rect: Rect,
    node_id: NodeId,
    name: &str,
    length: &str,
    color: Color32,
    sample: &ShelvedSample,
    in_tab: bool,
    z: f32,
) -> Vec<SynthResponse> {
    let mut responses = Vec::new();
    let id = popup_id(node_id);
    let open = ui.memory(|m| m.is_popup_open(id));
    let response = ui.interact(rect, ui.id().with(("sampler_name", node_id)), Sense::click());
    if open || response.hovered() {
        let fill = if open { theme::background::WIDGET_ACTIVE } else { theme::background::WIDGET_HOVERED };
        ui.painter().rect_filled(rect, 3.0 * z, fill.gamma_multiply(0.7));
    }
    let font = egui::FontId::proportional(8.0 * z);
    let room = rect.width() - 12.0 * z;
    let chars = name.chars().count();
    let text = (6..=chars)
        .rev()
        .map(|keep| {
            let name = if keep == chars { name.to_string() } else { format!("{}…", name.chars().take(keep - 1).collect::<String>()) };
            ui.painter().layout_no_wrap(format!("{name}  {length}"), font.clone(), color)
        })
        .find(|text| text.size().x <= room)
        .unwrap_or_else(|| ui.painter().layout_no_wrap(length.to_string(), font.clone(), color));
    let at = Pos2::new(rect.left() + 2.0 * z, rect.center().y - text.size().y / 2.0);
    let text_right = at.x + text.size().x;
    ui.painter().galley(at, text, color);
    // A caret, so the name reads as something to open
    let c = Pos2::new((text_right + 6.0 * z).min(rect.right() - 4.0 * z), rect.center().y + 0.5 * z);
    let caret = if open || response.hovered() { theme::text::PRIMARY } else { theme::text::DISABLED };
    ui.painter().add(Shape::convex_polygon(
        vec![Pos2::new(c.x - 2.5 * z, c.y - 1.5 * z), Pos2::new(c.x + 2.5 * z, c.y - 1.5 * z), Pos2::new(c.x, c.y + 1.5 * z)],
        caret,
        Stroke::NONE,
    ));
    let response = response.on_hover_text(if in_tab {
        "Trim it to its sound, normalize its level, or download it to keep it"
    } else {
        "Trim it to its sound, or normalize its level"
    });
    if response.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }

    let mut hovered = None;
    egui::popup_below_widget(ui, id, &response, PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(250.0);
        if let Some(tidy) = menu(ui, sample, &mut hovered) {
            responses.push(SynthResponse::TidySample { node_id, tidy });
            ui.memory_mut(|m| m.close_popup());
        }
        if in_tab && download_item(ui, sample) {
            responses.push(SynthResponse::DownloadSample(node_id));
            ui.memory_mut(|m| m.close_popup());
        }
    });
    let was = previewing(ui, node_id);
    let shown = hovered.filter(|_| ui.memory(|m| m.is_popup_open(id)));
    if shown != was {
        ui.data_mut(|d| match shown {
            Some(tidy) => d.insert_temp(preview_id(node_id), tidy),
            None => d.remove::<Tidy>(preview_id(node_id)),
        });
        ui.ctx().request_repaint();
    }
    responses
}

/// The menu's items, each saying what it would do. Returns the one chosen.
fn menu(ui: &mut egui::Ui, sample: &ShelvedSample, hovered: &mut Option<Tidy>) -> Option<Tidy> {
    let survey = sample.survey();
    let source = &sample.source;
    let rate = source.sample_rate();
    let mut chosen = None;

    let trims = survey.trims(source.frames());
    let detail = match (&survey.sound, trims) {
        (Some(sound), true) => format!("−{:.2} s, −{:.2} s", sound.start as f32 / rate, (source.frames() - sound.end) as f32 / rate),
        _ => String::new(),
    };
    let trim = ui.add_enabled(trims, egui::Button::new("Trim to the sound").shortcut_text(detail));
    // No tooltips while an item works: the waveform shows what it would
    // do, and a tooltip would cover the items below
    let trim = trim.on_disabled_hover_text(match survey.sound {
        Some(_) => "It's already its sound: nothing to cut",
        None => "No sound stands out of the room: nothing to trim to",
    });
    if trim.hovered() {
        *hovered = Some(Tidy::Trim);
    }
    if trim.clicked() {
        chosen = Some(Tidy::Trim);
    }

    for level in [Level::Peak, Level::Rms] {
        let gain = survey.gain_db(level);
        let changes = gain.is_some_and(|gain| gain.abs() >= NO_CHANGE_DB);
        let detail = gain.filter(|_| changes).map(signed_db).unwrap_or_default();
        let item = ui.add_enabled(changes, egui::Button::new(format!("Normalize {}", level.describe())).shortcut_text(detail));
        let item = item.on_disabled_hover_text(if gain.is_some() { "It's there already" } else { "It's silent" });
        if item.hovered() {
            *hovered = Some(Tidy::Normalize(level));
        }
        if item.clicked() {
            chosen = Some(Tidy::Normalize(level));
        }
    }
    ui.add_space(2.0);
    ui.label(egui::RichText::new("Each makes a new file: undo brings this one back").small().weak());
    chosen
}

/// Download, under the rest, for a file kept only in the browser's tab.
/// Returns whether it was chosen.
fn download_item(ui: &mut egui::Ui, sample: &ShelvedSample) -> bool {
    ui.separator();
    let source = &sample.source;
    // Written as 32-bit float, as a take is on the desktop
    let bytes = source.frames() * if source.is_stereo() { 2 } else { 1 } * 4;
    let size = format!("WAV, {:.1} MB", bytes as f32 / (1024.0 * 1024.0));
    let chosen = ui.add(egui::Button::new("Download").shortcut_text(size)).clicked();
    ui.add_space(2.0);
    ui.label(egui::RichText::new("It's kept only in this tab until it's downloaded").small().weak());
    chosen
}

/// Shows what a hovered item would do, over the waveform in `plot`.
pub fn draw_preview(painter: &egui::Painter, plot: Rect, sample: &ShelvedSample, tidy: Tidy, z: f32) {
    let survey = sample.survey();
    match tidy {
        Tidy::Trim => draw_trim(painter, plot, survey, sample.source.frames(), sample.source.sample_rate(), z),
        Tidy::Normalize(level) => draw_level(painter, plot, survey, level, z),
    }
}

/// What a trim cuts, veiled and hatched, and crop marks round what it keeps.
fn draw_trim(painter: &egui::Painter, plot: Rect, survey: &Survey, frames: usize, rate: f32, z: f32) {
    let Some(sound) = survey.sound.clone().filter(|_| survey.trims(frames)) else { return };
    let x_at = |frame: usize| plot.left() + frame as f32 / frames.max(1) as f32 * plot.width();
    let (keep_left, keep_right) = (x_at(sound.start), x_at(sound.end));
    let small = egui::FontId::proportional(7.0 * z);
    for (from, to, seconds) in [
        (plot.left(), keep_left, sound.start as f32 / rate),
        (keep_right, plot.right(), (frames - sound.end) as f32 / rate),
    ] {
        if to - from < 0.5 * z {
            continue;
        }
        let cut = Rect::from_x_y_ranges(from..=to, plot.y_range());
        painter.rect_filled(cut, 0.0, Color32::from_rgba_unmultiplied(8, 9, 14, 190));
        hatch(&painter.with_clip_rect(cut), cut, Stroke::new(0.8 * z, theme::text::SECONDARY.gamma_multiply(0.22)), 5.0 * z);
        if cut.width() >= 26.0 * z {
            painter.text(cut.center_bottom() - Vec2::new(0.0, 2.0 * z), egui::Align2::CENTER_BOTTOM, format!("−{seconds:.2} s"), small.clone(), theme::text::PRIMARY);
        }
    }
    // Crop marks at the corners of what's kept
    let arm = 5.0 * z;
    let stroke = Stroke::new(1.2 * z, Color32::WHITE);
    for (x, inward) in [(keep_left, 1.0), (keep_right, -1.0)] {
        for (y, down) in [(plot.top(), 1.0), (plot.bottom(), -1.0)] {
            painter.line_segment([Pos2::new(x, y), Pos2::new(x + inward * arm, y)], stroke);
            painter.line_segment([Pos2::new(x, y), Pos2::new(x, y + down * arm)], stroke);
        }
        let line = [Pos2::new(x, plot.top() + arm + 2.0 * z), Pos2::new(x, plot.bottom() - arm - 2.0 * z)];
        painter.extend(Shape::dashed_line(&line, Stroke::new(0.8 * z, Color32::WHITE.gamma_multiply(0.5)), 2.0 * z, 2.0 * z));
    }
}

/// A change of level, written across the panel: the waveform is drawn to
/// fill it whatever its level, so the numbers say what it can't.
fn draw_level(painter: &egui::Painter, plot: Rect, survey: &Survey, level: Level, z: f32) {
    let Some(gain) = survey.gain_db(level) else { return };
    painter.rect_filled(plot, 0.0, Color32::from_rgba_unmultiplied(8, 9, 14, 150));
    let (what, now, target) = match level {
        Level::Peak => ("peak", survey.peak, PEAK_CEILING_DB),
        Level::Rms => ("RMS", survey.rms, TARGET_RMS_DB),
    };
    let now_db = 20.0 * now.max(1e-9).log10();
    let blue = crate::dsp::ModuleCategory::Source.color();
    painter.text(plot.center() - Vec2::new(0.0, 4.0 * z), egui::Align2::CENTER_CENTER, signed_db(gain), egui::FontId::proportional(14.0 * z), blue.lerp_to_gamma(Color32::WHITE, 0.5));
    painter.text(
        plot.center() + Vec2::new(0.0, 10.0 * z),
        egui::Align2::CENTER_CENTER,
        format!("{what} {} → {} dBFS", minus((now_db * 10.0).round() / 10.0), minus(target)),
        egui::FontId::proportional(8.0 * z),
        theme::text::SECONDARY,
    );
}

/// Diagonal lines across `rect`, `gap` apart, drawn through `painter`
/// (clipped to the rectangle).
fn hatch(painter: &egui::Painter, rect: Rect, stroke: Stroke, gap: f32) {
    let height = rect.height();
    let mut x = rect.left() - height;
    while x < rect.right() {
        painter.line_segment([Pos2::new(x, rect.bottom()), Pos2::new(x + height, rect.top())], stroke);
        x += gap;
    }
}
