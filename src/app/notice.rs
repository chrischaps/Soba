//! Notes that pop up in the corner: a finished recording, a new version.
//! Each is a card with a coloured dot, a title and a close cross. It stays
//! up while the mouse is over it, and some go away on their own after a
//! while. When two are up, the later one stacks above.

use std::time::Duration;

use eframe::egui::{self, Color32, RichText};

use super::theme;

/// How far a note sits from the window's bottom-right corner.
const MARGIN: egui::Vec2 = egui::vec2(16.0, 40.0);

/// The gap between stacked notes.
const GAP: f32 = 10.0;

/// A note to draw this frame.
pub struct Notice<'a> {
    id: &'a str,
    dot: Color32,
    title: String,
    /// When it went up, in egui's clock, and how long it stays.
    lifetime: Option<(f64, f64)>,
    /// How far up it sits, above the notes below it.
    raise: f32,
}

/// What happened to a note this frame.
pub struct NoticeResponse<R> {
    pub inner: R,
    /// The cross was clicked.
    pub closed: bool,
    /// Its time ran out.
    pub expired: bool,
    /// The height it took, to stack the next one above.
    pub height: f32,
}

impl<'a> Notice<'a> {
    pub fn new(id: &'a str, dot: Color32, title: impl Into<String>) -> Self {
        Self { id, dot, title: title.into(), lifetime: None, raise: 0.0 }
    }

    /// Goes away `seconds` after `shown_at`, unless the mouse is over it.
    pub fn lifetime(mut self, shown_at: f64, seconds: f64) -> Self {
        self.lifetime = Some((shown_at, seconds));
        self
    }

    /// Sits above notes `height` tall already drawn below.
    pub fn above(mut self, height: f32) -> Self {
        self.raise = if height > 0.0 { height + GAP } else { 0.0 };
        self
    }

    /// Draws the card: the title row, with `title_extra` after the title,
    /// then `body`.
    pub fn show<R>(
        self,
        ctx: &egui::Context,
        title_extra: impl FnOnce(&mut egui::Ui),
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> NoticeResponse<R> {
        let mut closed = false;
        let area = egui::Area::new(egui::Id::new(self.id))
            .anchor(egui::Align2::RIGHT_BOTTOM, -MARGIN - egui::vec2(0.0, self.raise))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                card(ui).show(ui, |ui| {
                    ui.set_max_width(340.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(self.dot));
                        ui.label(RichText::new(&self.title).color(theme::text::PRIMARY).strong());
                        title_extra(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if close_button(ui).on_hover_text("Dismiss").clicked() {
                                closed = true;
                            }
                        });
                    });
                    body(ui)
                })
                .inner
            });

        let mut expired = false;
        if let Some((shown_at, seconds)) = self.lifetime {
            let now = ctx.input(|i| i.time);
            if area.response.contains_pointer() {
                // Reading it holds it up
                ctx.request_repaint();
            } else if now - shown_at > seconds {
                expired = true;
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(seconds - (now - shown_at) + 0.05));
            }
        }
        NoticeResponse { inner: area.inner, closed, expired, height: area.response.rect.height() }
    }
}

/// A note's card: the panel colour, a hairline edge, rounded.
fn card(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::popup(ui.style())
        .fill(theme::background::PANEL)
        .stroke(egui::Stroke::new(1.0_f32, theme::background::WIDGET_ACTIVE))
        .corner_radius(theme::ROUNDING)
        .inner_margin(egui::Margin::same(14))
}

/// A small painted cross (the UI font has no ✕).
pub fn close_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let color = if response.hovered() { theme::text::PRIMARY } else { theme::text::SECONDARY };
    let r = 4.0;
    let c = rect.center();
    let stroke = egui::Stroke::new(1.5_f32, color);
    ui.painter().line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
    ui.painter().line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], stroke);
    response
}

/// A thin progress bar the width of the note, in the accent colour.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))] // the browser has no updater
pub fn progress_bar(ui: &mut egui::Ui, fraction: Option<f32>) {
    let width = ui.available_width().clamp(200.0, 312.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 4.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme::background::WIDGET);
    match fraction {
        Some(f) => {
            let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * f.clamp(0.0, 1.0), rect.height()));
            painter.rect_filled(filled, 2.0, theme::accent::PRIMARY);
        }
        None => {
            // Size unknown: a short bar sweeps across
            let t = ui.input(|i| i.time) as f32;
            let x = (t * 0.7).fract() * (rect.width() + 60.0) - 60.0;
            let span = egui::Rect::from_x_y_ranges((rect.left() + x.max(0.0))..=(rect.left() + (x + 60.0).min(rect.width())), rect.y_range());
            painter.rect_filled(span, 2.0, theme::accent::PRIMARY);
        }
    }
    ui.ctx().request_repaint();
}
