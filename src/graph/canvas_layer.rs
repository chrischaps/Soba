//! The canvas is laid out at zoom 1 and drawn at the view's zoom (#409), so
//! nothing on it moves as the view zooms. egui rounds every step of a layout
//! to 1/32 pt; laid out afresh at each zoom, every face wobbled by up to a
//! pixel as the view zoomed, worst on the tall ones.
//!
//! The editor is drawn into a layer of its own, which egui is told is
//! scaled, as [`egui::Scene`] does: its widgets then take the pointer in
//! layout points. But egui would draw that layer by scaling its shapes,
//! and a glyph is rasterized once, at the size its text was laid out:
//! scaled up it blurs, scaled down it breaks up (egui #4813). So the
//! layer's shapes are moved onto the canvas's own layer instead, scaled
//! here, and each text is laid out again at the size it's shown.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, LayerId, Pos2, Rect, Shape, Style, Ui, UiBuilder, Vec2};
use eframe::emath::TSTransform;
use eframe::epaint::text::ByteIndex;
use egui_node_graph2::NodeId;

use super::SynthGraphEditorState;

/// The editor switched to zoom 1 for laying out, and what it takes to
/// switch it back to the view's zoom. Node positions and the pan are the
/// view's divided by its zoom, so the two agree about where everything is.
pub struct LaidOut {
    zoom: f32,
    style: Arc<Style>,
    /// The view's positions and pan, given back exactly where nothing moved
    positions: HashMap<NodeId, Pos2>,
    pan: Vec2,
    /// The same, laid out: whatever differs afterwards was moved
    laid_positions: HashMap<NodeId, Pos2>,
    laid_pan: Vec2,
}

impl LaidOut {
    /// Switches `editor` to zoom 1, its style to `style` (the canvas's, unscaled).
    pub fn switch(editor: &mut SynthGraphEditorState, style: Arc<Style>) -> Self {
        let zoom = editor.pan_zoom.zoom;
        let positions = editor.node_positions.iter().map(|(n, &at)| (n, at)).collect();
        let pan = editor.pan_zoom.pan;
        for (_, at) in editor.node_positions.iter_mut() {
            *at = (at.to_vec2() / zoom).to_pos2();
        }
        editor.pan_zoom.pan = pan / zoom;
        editor.pan_zoom.zoom = 1.0;
        let style = std::mem::replace(&mut editor.pan_zoom.zoomed_style, style);
        Self {
            zoom,
            style,
            positions,
            pan,
            laid_positions: editor.node_positions.iter().map(|(n, &at)| (n, at)).collect(),
            laid_pan: editor.pan_zoom.pan,
        }
    }

    /// The zoom the view is drawn at.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// How a point laid out in an editor whose rect starts at `editor_min`
    /// is shown: scaled about that corner, where node position (0, 0) is
    /// before the pan.
    pub fn transform(&self, editor_min: Pos2) -> TSTransform {
        TSTransform::new(editor_min.to_vec2() * (1.0 - self.zoom), self.zoom)
    }

    /// Switches `editor` back to the view's zoom, keeping what was moved
    /// while it was laid out: dragged modules, a pan, new modules.
    pub fn restore(self, editor: &mut SynthGraphEditorState) {
        let zoom = self.zoom;
        for (node, at) in editor.node_positions.iter_mut() {
            let unmoved = self.laid_positions.get(&node) == Some(at);
            *at = match self.positions.get(&node) {
                Some(&before) if unmoved => before,
                _ => (at.to_vec2() * zoom).to_pos2(),
            };
        }
        editor.pan_zoom.pan = if editor.pan_zoom.pan == self.laid_pan { self.pan } else { editor.pan_zoom.pan * zoom };
        editor.pan_zoom.zoom = zoom;
        editor.pan_zoom.zoomed_style = self.style;
    }
}

/// Lays `add_contents` out on a layer of its own, which takes the pointer
/// through `transform`, in a rect that shows as `ui`'s available rect. Its
/// shapes end up on `ui`'s layer, scaled, after whatever `ui` has drawn.
pub fn show<R>(ui: &mut Ui, transform: TSTransform, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let canvas = ui.layer_id();
    let layer = LayerId::new(canvas.order, ui.scope_id().with("canvas layer"));
    ctx.set_sublayer(canvas, layer);
    ctx.set_transform_layer(layer, transform);
    ctx.data_mut(|d| d.insert_temp(drawn_on_id(), (canvas, layer)));

    let to_layout = transform.inverse();
    let clip = ui.clip_rect();
    let mut laid = ui.new_child(UiBuilder::new().layer_id(layer).max_rect(to_layout * ui.available_rect_before_wrap()));
    laid.set_clip_rect(to_layout * clip);
    let inner = add_contents(&mut laid);

    move_scaled(&ctx, layer, canvas, transform, clip);
    inner
}

fn drawn_on_id() -> egui::Id {
    egui::Id::unique("soba canvas layer")
}

/// The layer egui finds under the pointer where `layer` is: the canvas's
/// own layer for the one the editor is laid out on, which isn't an area of
/// its own. Compare it with [`egui::Context::layer_id_at`].
pub fn shown_on(ctx: &egui::Context, layer: LayerId) -> LayerId {
    match ctx.data(|d| d.get_temp::<(LayerId, LayerId)>(drawn_on_id())) {
        Some((canvas, laid)) if laid == layer => canvas,
        _ => layer,
    }
}

/// The layer the editor is laid out on, if it's drawn onto `layer`.
pub fn canvas_on(ctx: &egui::Context, layer: LayerId) -> Option<LayerId> {
    ctx.data(|d| d.get_temp::<(LayerId, LayerId)>(drawn_on_id())).filter(|&(canvas, _)| canvas == layer).map(|(_, laid)| laid)
}

/// Where a point on screen is in `layer`: the pointer, for a face that
/// reads it from the input rather than from its response.
pub fn in_layer(ctx: &egui::Context, layer: LayerId, screen: Pos2) -> Pos2 {
    ctx.layer_transform_from_global(layer).map_or(screen, |from_global| from_global * screen)
}

/// Where a point the input gives, on screen, is in `ui`'s layer.
pub fn in_ui(ui: &Ui, screen: Option<Pos2>) -> Option<Pos2> {
    screen.map(|at| in_layer(ui.ctx(), ui.layer_id(), at))
}

/// Where a rect in `layer` is on screen.
pub fn on_screen(ctx: &egui::Context, layer: LayerId, rect: Rect) -> Rect {
    ctx.layer_transform_to_global(layer).map_or(rect, |to_global| to_global * rect)
}

/// Moves every shape on `from` to the end of `to`, scaled by `transform`
/// and clipped to `clip` as well as their own clips.
fn move_scaled(ctx: &egui::Context, from: LayerId, to: LayerId, transform: TSTransform, clip: Rect) {
    let mut shapes = ctx.graphics_mut(|g| {
        let list = g.entry(from);
        let mut shapes = Vec::with_capacity(list.next_idx().0);
        for i in 0..list.next_idx().0 {
            list.mutate_shape(egui::layers::ShapeIdx(i), |shape| {
                let taken = std::mem::replace(&mut shape.shape, Shape::Noop);
                shapes.push((shape.clip_rect, taken));
            });
        }
        shapes
    });
    if transform != TSTransform::IDENTITY {
        ctx.fonts_mut(|fonts| {
            for (clip_rect, shape) in &mut shapes {
                *clip_rect = transform * *clip_rect;
                scale_shape(shape, transform, &mut |job| fonts.layout_job(job));
            }
        });
    }
    ctx.graphics_mut(|g| {
        let list = g.entry(to);
        for (clip_rect, shape) in shapes {
            if !matches!(shape, Shape::Noop) {
                list.add(clip_rect.intersect(clip), shape);
            }
        }
    });
}

/// Scales `shape` by `transform`, its text laid out again by `layout` at
/// the size it's shown, so its glyphs are rasterized at that size.
fn scale_shape(shape: &mut Shape, transform: TSTransform, layout: &mut impl FnMut(egui::text::LayoutJob) -> Arc<egui::Galley>) {
    match shape {
        Shape::Vec(shapes) => {
            for shape in shapes {
                scale_shape(shape, transform, layout);
            }
        }
        Shape::Text(text) => {
            let scale = transform.scaling;
            text.galley = layout(scaled_job(&text.galley, scale));
            text.pos = transform * text.pos;
            text.underline.width *= scale;
        }
        shape => shape.transform(transform),
    }
}

/// `galley`'s text as it lays out `scale` times the size. Its lines break
/// where they did, and text that fitted isn't cut short: fonts don't grow
/// quite in step with the size they're asked for (#255), so wrapped afresh
/// a word could change lines, and a choice that just fitted its box would
/// be elided to "…". Text that was cut short is cut at the scaled width.
fn scaled_job(galley: &egui::Galley, scale: f32) -> egui::text::LayoutJob {
    let mut job = (*galley.job).clone();
    if galley.elided || job.justify {
        job.wrap.max_width *= scale;
    } else {
        broken_at(&mut job, &wrapped_at(galley));
        job.wrap.max_width = f32::INFINITY;
    }
    for section in &mut job.sections {
        section.leading_space *= scale;
        let format = &mut section.format;
        format.font_id.size *= scale;
        format.extra_letter_spacing *= scale;
        format.line_height = format.line_height.map(|h| h * scale);
        format.expand_bg *= scale;
        format.underline.width *= scale;
        format.strikethrough.width *= scale;
    }
    job.wrap.max_width *= scale;
    job.first_row_min_height *= scale;
    job
}

/// Where `galley`'s wrapping broke its text into lines, in bytes.
fn wrapped_at(galley: &egui::Galley) -> Vec<usize> {
    let text = &galley.job.text;
    let mut chars = 0;
    let mut breaks = Vec::new();
    for row in &galley.rows[..galley.rows.len().saturating_sub(1)] {
        chars += row.char_count_including_newline().0;
        if !row.ends_with_newline {
            breaks.push(text.char_indices().nth(chars).map_or(text.len(), |(at, _)| at));
        }
    }
    breaks
}

/// Puts a line break into `job` at each of `breaks` (bytes, in order), each
/// in the section that ends there.
fn broken_at(job: &mut egui::text::LayoutJob, breaks: &[usize]) {
    if breaks.is_empty() {
        return;
    }
    let moved = |at: usize| at + breaks.iter().take_while(|&&b| b <= at).count();
    let mut text = String::with_capacity(job.text.len() + breaks.len());
    let mut from = 0;
    for &at in breaks {
        text.push_str(&job.text[from..at]);
        text.push('\n');
        from = at;
    }
    text.push_str(&job.text[from..]);
    job.text = text;
    for section in &mut job.sections {
        let range = &mut section.byte_range;
        let start = if range.start.0 == 0 { 0 } else { moved(range.start.0) };
        *range = ByteIndex(start)..ByteIndex(moved(range.end.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2, Color32, FontId};
    use eframe::epaint::text::ByteRangeExt as _;

    #[test]
    fn laid_out_and_back_is_where_it_was() {
        let mut editor = crate::graph::create_editor_state();
        let mut add = || {
            let data = crate::graph::SynthNodeData::new("test.node", "Test", crate::dsp::ModuleCategory::Utility);
            editor.graph.add_node("Test".to_string(), data, |_, _| {})
        };
        let (a, b) = (add(), add());
        editor.node_positions.insert(a, pos2(120.3, -41.7));
        editor.node_positions.insert(b, pos2(-8.1, 333.3));
        editor.pan_zoom.pan = vec2(17.7, -3.1);
        editor.pan_zoom.zoom = 0.37;

        let laid = LaidOut::switch(&mut editor, Arc::new(Style::default()));
        assert_eq!(editor.pan_zoom.zoom, 1.0);
        let shown = laid.transform(pos2(50.0, 80.0));
        // A node is shown where the view had it: editor corner + pan + position
        let laid_at = pos2(50.0, 80.0) + editor.pan_zoom.pan + editor.node_positions[a].to_vec2();
        let view_at = pos2(50.0, 80.0) + vec2(17.7, -3.1) + vec2(120.3, -41.7);
        assert!((shown * laid_at - view_at).length() < 1e-3);

        // Nothing moved: given back to the bit. One node dragged: scaled
        editor.node_positions[b] += vec2(10.0, 0.0);
        laid.restore(&mut editor);
        assert_eq!(editor.node_positions[a], pos2(120.3, -41.7));
        assert!((editor.node_positions[b] - pos2(-8.1 + 3.7, 333.3)).length() < 1e-3);
        assert_eq!(editor.pan_zoom.pan, vec2(17.7, -3.1));
        assert_eq!(editor.pan_zoom.zoom, 0.37);
    }

    #[test]
    fn text_is_laid_out_again_at_the_size_it_is_shown() {
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx();
            let galley = ctx.fonts_mut(|f| f.layout_no_wrap("Cutoff".into(), FontId::proportional(14.0), Color32::WHITE));
            let mut shape = Shape::galley(pos2(10.0, 20.0), galley.clone(), Color32::WHITE);
            let transform = TSTransform::new(vec2(5.0, 5.0), 0.5);
            ctx.fonts_mut(|fonts| scale_shape(&mut shape, transform, &mut |job| fonts.layout_job(job)));

            let Shape::Text(text) = shape else { panic!("still text") };
            assert_eq!(text.pos, pos2(10.0, 15.0));
            assert_eq!(text.galley.job.sections[0].format.font_id.size, 7.0);
            // About half as wide, but laid out at 7 pt rather than squeezed
            let ratio = text.galley.size().x / galley.size().x;
            assert!((ratio - 0.5).abs() < 0.05, "{ratio}");

            // A choice that just fits its box, as a dropdown's does, isn't cut short
            let mut job = egui::text::LayoutJob::simple_singleline("Omni".into(), FontId::proportional(11.0), Color32::WHITE);
            job.wrap = egui::text::TextWrapping::truncate_at_width(galley_width(ctx, "Omni", 11.0));
            let fitted = ctx.fonts_mut(|f| f.layout_job(job));
            assert!(!fitted.elided);
            for scale in [0.3, 0.45, 0.7, 2.6] {
                let scaled = ctx.fonts_mut(|f| f.layout_job(scaled_job(&fitted, scale)));
                assert!(!scaled.elided, "at {scale}");
            }
        })
        .textures_delta
        .clear();
    }

    fn galley_width(ctx: &egui::Context, text: &str, size: f32) -> f32 {
        ctx.fonts_mut(|f| f.layout_no_wrap(text.into(), FontId::proportional(size), Color32::WHITE)).size().x
    }

    #[test]
    fn wrapped_text_keeps_its_lines_at_any_size() {
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx();
            let words = "Hold the low note while the pad swells, then let it go";
            let mut job = egui::text::LayoutJob::simple(words.into(), FontId::proportional(13.0), Color32::WHITE, 120.0);
            job.append("and **listen**", 4.0, egui::TextFormat::simple(FontId::monospace(11.0), Color32::RED));
            let galley = ctx.fonts_mut(|f| f.layout_job(job));
            assert!(galley.rows.len() > 2, "wraps");
            let lines = |g: &egui::Galley| g.rows.iter().map(|r| r.glyphs.iter().map(|g| g.chr).collect::<String>().trim().to_owned()).collect::<Vec<_>>();
            for scale in [0.31, 0.77, 2.3, 4.9] {
                let scaled = ctx.fonts_mut(|f| f.layout_job(scaled_job(&galley, scale)));
                assert_eq!(lines(&scaled), lines(&galley), "at {scale}");
                assert_eq!(scaled.job.sections[1].format.font_id, FontId::monospace(11.0 * scale));
                assert!(scaled.job.sections[1].byte_range.slice(&scaled.job.text).ends_with("**listen**"));
            }
        })
        .textures_delta
        .clear();
    }
}
