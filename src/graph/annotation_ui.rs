//! Drawing frames and notes under the modules, and handling the mouse on them.
//!
//! A frame is lettered like a section of a synth's front panel: its title
//! set in capitals, spaced out, with a hairline running on to the edge. Its
//! title band is its handle: drag it to move the frame and every module and
//! note inside, double-click it to rename the frame, right-click it for
//! colours. Its edges and corners resize it. Its body passes clicks through
//! to the canvas, so modules can be added and box-selected inside it.
//!
//! A note is a soft card with a margin mark. Drag it to move it, double-click
//! to edit its text, and drag its right edge to change where it wraps.

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{
    pos2, vec2, Color32, Mesh, CursorIcon, FontFamily, FontId, Id, PointerButton, Pos2, Rect, CornerRadius, Sense,
    Shape, Stroke, Ui, UiBuilder, Vec2,
};
use egui::emath::TSTransform;
use egui_node_graph2::{Backdrop, NodeId};

use crate::app::theme;
use super::annotations::{Annotation, AnnotationId, Annotations, Frame, Tint, MIN_FRAME_SIZE, NOTE_WIDTHS};
use super::annotations::emphasis_runs;

/// Maps patch space onto the screen.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Where patch (0, 0) is on screen.
    pub origin: Pos2,
    pub zoom: f32,
}

impl View {
    pub fn to_screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }

    pub fn rect_to_screen(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }

    pub fn to_patch(&self, s: Pos2) -> Pos2 {
        ((s - self.origin) / self.zoom).to_pos2()
    }
}

// Measurements in patch points, drawn scaled by the zoom
const FRAME_ROUNDING: f32 = 12.0;
/// The height of a frame's title band, its handle.
pub const FRAME_TITLE_BAND: f32 = 34.0;
const FRAME_TITLE_SIZE: f32 = 13.0;
/// Extra space between the title's capitals, as on a printed panel
const FRAME_TITLE_TRACKING: f32 = 1.8;
const FRAME_TITLE_INSET: f32 = 14.0;
const NOTE_TEXT_SIZE: f32 = 13.5;
const NOTE_PADDING: Vec2 = Vec2::new(14.0, 10.0);
const NOTE_ROUNDING: f32 = 7.0;
const NOTE_MARK_WIDTH: f32 = 3.0;

/// How near an edge the pointer can grab it, in screen points.
const GRAB: f32 = 5.0;

/// Words on the canvas are lettered over the cables, on a backing of the
/// canvas beneath them, so a cable passing by goes behind them. A cable
/// shows through the backing as strongly as through a module's face.
///
/// How far past the words the backing reaches, and how far beyond that it
/// fades away, in patch points.
const LABEL_MARGIN: Vec2 = Vec2::new(6.0, 3.0);
const LABEL_FEATHER: f32 = 7.0;

/// A note's ink, and the warmer ink its bold words are set in.
const NOTE_INK: Color32 = Color32::from_rgb(196, 198, 212);
const NOTE_EMPHASIS: Color32 = theme::text::PRIMARY;
/// The note card: paper-warm, barely there.
const NOTE_FILL: Color32 = Color32::from_rgba_premultiplied(15, 14, 12, 15);
/// The mark down a note's left side, like a pencil line in a margin.
const NOTE_MARK: Color32 = theme::accent::WARNING;

/// What a drag on a frame or note is doing. Kept in egui's memory while the
/// button is held.
#[derive(Clone, Debug)]
enum Drag {
    /// Moving a frame or note, with the modules and annotations inside a frame.
    Move { members: Vec<NodeId>, riders: Vec<AnnotationId> },
    /// Resizing a frame from the edges marked, from where it started.
    Resize { edges: Edges, start: Rect, pointer: Pos2 },
    /// Changing where a note wraps.
    Widen { start: f32, pointer: Pos2 },
}

/// Which edges of a frame a handle moves.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl Edges {
    const fn new(left: bool, right: bool, top: bool, bottom: bool) -> Self {
        Self { left, right, top, bottom }
    }

    fn cursor(self) -> CursorIcon {
        match (self.left || self.right, self.top || self.bottom) {
            (true, false) => CursorIcon::ResizeHorizontal,
            (false, true) => CursorIcon::ResizeVertical,
            _ if (self.left && self.top) || (self.right && self.bottom) => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        }
    }

    /// `start` with these edges moved by `delta` patch points, no smaller
    /// than a frame can be.
    fn resize(self, start: Rect, delta: Vec2) -> Rect {
        let mut rect = start;
        if self.left {
            rect.min.x = (start.min.x + delta.x).min(start.max.x - MIN_FRAME_SIZE.x);
        }
        if self.right {
            rect.max.x = (start.max.x + delta.x).max(start.min.x + MIN_FRAME_SIZE.x);
        }
        if self.top {
            rect.min.y = (start.min.y + delta.y).min(start.max.y - MIN_FRAME_SIZE.y);
        }
        if self.bottom {
            rect.max.y = (start.max.y + delta.y).max(start.min.y + MIN_FRAME_SIZE.y);
        }
        rect
    }
}

/// The eight handles round a frame: four edges, then the corners over them.
const HANDLES: [Edges; 8] = [
    Edges::new(true, false, false, false),
    Edges::new(false, true, false, false),
    Edges::new(false, false, true, false),
    Edges::new(false, false, false, true),
    Edges::new(true, false, true, false),
    Edges::new(false, true, true, false),
    Edges::new(true, false, false, true),
    Edges::new(false, true, false, true),
];

/// What a frame's or note's right-click menu asked for.
enum MenuAction {
    Edit(AnnotationId),
    Tint(AnnotationId, Tint),
    SelectInside(AnnotationId),
    Delete(AnnotationId),
}

fn drag_id(id: AnnotationId) -> Id {
    Id::unique(("annotation drag", id))
}

fn edit_id(id: AnnotationId) -> Id {
    Id::unique(("annotation edit", id))
}

/// The face frame titles and bold note words are set in.
fn title_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::TITLE_FAMILY.into()))
}

/// A frame's title ink: its tint, lifted toward white so it reads on the
/// dark canvas.
fn title_ink(tint: Tint) -> Color32 {
    tint.color().lerp_to_gamma(Color32::WHITE, 0.4)
}

/// Where a module was last drawn, on screen.
pub fn module_rect(ctx: &egui::Context, node_id: NodeId) -> Option<Rect> {
    ctx.read_response(Id::unique((node_id, "window"))).map(|r| r.rect)
}

/// Modules whose middle lies inside a rectangle on screen.
pub fn modules_inside(ctx: &egui::Context, screen: Rect, nodes: impl IntoIterator<Item = NodeId>) -> Vec<NodeId> {
    nodes
        .into_iter()
        .filter(|&node_id| module_rect(ctx, node_id).is_some_and(|r| screen.contains(r.center())))
        .collect()
}

/// Frames and notes lying inside a frame, which move with it.
fn riders(annotations: &Annotations, frame_id: AnnotationId, area: Rect) -> Vec<AnnotationId> {
    annotations
        .iter()
        .filter(|&(id, a)| {
            id != frame_id
                && match a {
                    Annotation::Frame(inner) => area.contains_rect(inner.rect) && inner.rect != area,
                    Annotation::Note(note) => area.contains(note.position),
                }
        })
        .map(|(id, _)| id)
        .collect()
}

/// Draws every frame, then every note, and handles the mouse on them.
/// Larger frames go first, so a frame inside another sits on top of it.
/// Returns whether the pointer is over one of their handles, where a
/// right-click is theirs rather than the canvas's.
/// `through` is how strongly a cable shows through the words it passes
/// behind, as through a module's face.
pub fn show(ui: &mut Ui, annotations: &mut Annotations, mut backdrop: Backdrop<'_>, view: View, through: f32) -> bool {
    let mut frames: Vec<(AnnotationId, f32)> = annotations
        .iter()
        .filter_map(|(id, a)| match a {
            Annotation::Frame(f) => Some((id, f.rect.area())),
            Annotation::Note(_) => None,
        })
        .collect();
    frames.sort_by(|a, b| b.1.total_cmp(&a.1));
    let notes: Vec<AnnotationId> =
        annotations.iter().filter(|(_, a)| matches!(a, Annotation::Note(_))).map(|(id, _)| id).collect();

    let mut over_handle = false;
    let mut menu_actions = Vec::new();
    for (id, _) in frames {
        over_handle |= frame_ui(ui, annotations, id, &mut backdrop, view, through, &mut menu_actions);
    }
    for id in notes {
        over_handle |= note_ui(ui, annotations, id, &mut backdrop, view, through, &mut menu_actions);
    }

    for action in menu_actions {
        match action {
            MenuAction::Edit(id) => annotations.start_editing(id, matches!(annotations.get(id), Some(Annotation::Frame(_)))),
            MenuAction::Tint(id, tint) => {
                if let Some(Annotation::Frame(frame)) = annotations.get_mut(id) {
                    frame.tint = tint;
                }
            }
            MenuAction::SelectInside(id) => {
                if let Some(Annotation::Frame(frame)) = annotations.get(id) {
                    let screen = view.rect_to_screen(frame.rect);
                    *backdrop.selected_nodes = modules_inside(ui.ctx(), screen, backdrop.node_positions.keys());
                    annotations.selected.clear();
                }
            }
            MenuAction::Delete(id) => {
                annotations.remove(id);
            }
        }
    }

    // A click anywhere else on the canvas lets go of the selection, as it
    // does for modules
    let clicked_elsewhere = ui.input(|i| i.pointer.primary_clicked() && !i.modifiers.shift)
        && !over_handle
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| ui.clip_rect().contains(pos) && ui.ctx().layer_id_at(pos) == Some(ui.layer_id()));
    if clicked_elsewhere {
        annotations.selected.clear();
    }
    over_handle
}

/// Selects a frame or note clicked on: alone, or added to (or taken from)
/// the selection with Shift held.
fn select(ui: &Ui, annotations: &mut Annotations, id: AnnotationId, backdrop: &mut Backdrop<'_>) {
    if ui.input(|i| i.modifiers.shift) {
        if !annotations.selected.remove(&id) {
            annotations.selected.insert(id);
        }
    } else {
        annotations.selected = [id].into();
        backdrop.selected_nodes.clear();
    }
}

/// The view pans when a handle is dragged with the middle button, or with
/// Ctrl, as it does when the canvas is.
fn pans_view(ui: &Ui, response: &egui::Response) -> bool {
    response.dragged() && ui.input(|i| i.pointer.middle_down() || i.modifiers.command_only())
}

fn frame_ui(
    ui: &mut Ui,
    annotations: &mut Annotations,
    id: AnnotationId,
    backdrop: &mut Backdrop<'_>,
    view: View,
    through: f32,
    menu_actions: &mut Vec<MenuAction>,
) -> bool {
    let Some(Annotation::Frame(frame)) = annotations.get(id).cloned() else { return false };
    let zoom = view.zoom;
    let screen = view.rect_to_screen(frame.rect);
    let band = Rect::from_min_size(screen.min, vec2(screen.width(), FRAME_TITLE_BAND * zoom));
    let selected = annotations.selected.contains(&id);
    let editing = annotations.editing.as_ref().is_some_and(|e| e.id == id);

    // The body and band are painted first; the handles are sensed over them
    let body_slot = ui.painter().add(Shape::Noop);

    // The title band: a handle to move the frame by
    let handle_rect = Rect::from_min_max(band.min + vec2(GRAB, GRAB), pos2(band.max.x - GRAB, band.max.y));
    let title = ui.interact(handle_rect, Id::unique(("frame title", id)), Sense::click_and_drag());
    let mut over = title.hovered();

    if title.drag_started_by(PointerButton::Primary) && !pans_view(ui, &title) {
        let members = modules_inside(ui.ctx(), screen, backdrop.node_positions.keys());
        let riders = riders(annotations, id, frame.rect);
        ui.ctx().data_mut(|d| d.insert_temp(drag_id(id), Drag::Move { members, riders }));
        if !selected {
            select(ui, annotations, id, backdrop);
        }
    }
    if pans_view(ui, &title) {
        *backdrop.pan += title.drag_delta();
    } else if title.dragged_by(PointerButton::Primary) {
        if let Some(Drag::Move { members, riders }) = ui.ctx().data(|d| d.get_temp::<Drag>(drag_id(id))) {
            let delta = title.drag_delta();
            for node_id in members {
                if let Some(position) = backdrop.node_positions.get_mut(node_id) {
                    *position += delta;
                }
            }
            for rider in riders.into_iter().chain([id]) {
                if let Some(annotation) = annotations.get_mut(rider) {
                    annotation.translate(delta / zoom);
                }
            }
        }
    }
    if title.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Drag>(drag_id(id)));
    }
    if title.clicked_by(PointerButton::Primary) {
        select(ui, annotations, id, backdrop);
    }
    if title.double_clicked() {
        annotations.start_editing(id, true);
    }
    if title.secondary_clicked() && !selected {
        select(ui, annotations, id, backdrop);
    }
    if title.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if title.hovered() && !editing {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    crate::widgets::menu::context_menu(&title, |ui| {
        ui.set_min_width(180.0);
        if ui.add(egui::Button::new("Rename").shortcut_text("Double-click")).clicked() {
            menu_actions.push(MenuAction::Edit(id));
            ui.close();
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for tint in Tint::ALL {
                if swatch(ui, tint, tint == frame.tint).clicked() {
                    menu_actions.push(MenuAction::Tint(id, tint));
                }
            }
        });
        if ui.button("Select modules inside").clicked() {
            menu_actions.push(MenuAction::SelectInside(id));
            ui.close();
        }
        ui.separator();
        if ui.add(egui::Button::new("Delete frame").shortcut_text("Del")).clicked() {
            menu_actions.push(MenuAction::Delete(id));
            ui.close();
        }
    });

    // The edges and corners resize it
    let mut resizing = false;
    for edges in HANDLES {
        let x = if edges.left { Some(screen.left()) } else if edges.right { Some(screen.right()) } else { None };
        let y = if edges.top { Some(screen.top()) } else if edges.bottom { Some(screen.bottom()) } else { None };
        let rect = match (x, y) {
            (Some(x), Some(y)) => Rect::from_center_size(pos2(x, y), Vec2::splat(GRAB * 2.5)),
            (Some(x), None) => Rect::from_x_y_ranges(x - GRAB..=x + GRAB, screen.top() + GRAB..=screen.bottom() - GRAB),
            (None, Some(y)) => Rect::from_x_y_ranges(screen.left() + GRAB..=screen.right() - GRAB, y - GRAB..=y + GRAB),
            (None, None) => continue,
        };
        let handle = ui.interact(rect, Id::unique(("frame edge", id, x.is_some(), edges.left, edges.top, y.is_some())), Sense::drag());
        over |= handle.hovered();
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(edges.cursor());
        }
        if handle.drag_started_by(PointerButton::Primary) {
            let pointer = handle.interact_pointer_pos().unwrap_or(rect.center());
            ui.ctx().data_mut(|d| d.insert_temp(drag_id(id), Drag::Resize { edges, start: frame.rect, pointer }));
        }
        if handle.dragged_by(PointerButton::Primary) {
            let held = ui.ctx().data(|d| d.get_temp::<Drag>(drag_id(id)));
            if let (Some(Drag::Resize { edges, start, pointer }), Some(now)) = (held, handle.interact_pointer_pos()) {
                if let Some(Annotation::Frame(frame)) = annotations.get_mut(id) {
                    frame.rect = edges.resize(start, (now - pointer) / zoom);
                }
            }
            resizing = true;
        }
        if handle.drag_stopped() {
            ui.ctx().data_mut(|d| d.remove::<Drag>(drag_id(id)));
        }
    }

    // Drawn where it is now, after any drag this frame
    let Some(Annotation::Frame(frame)) = annotations.get(id).cloned() else {
        ui.painter().set(body_slot, Shape::Noop);
        return over;
    };
    let screen = view.rect_to_screen(frame.rect);
    let band = Rect::from_min_size(screen.min, vec2(screen.width(), FRAME_TITLE_BAND * zoom));
    let hovered = title.hovered() || title.dragged() || resizing;
    let look = FrameLook { screen, band, zoom, selected, hovered, editing };
    paint_frame(ui, body_slot, backdrop.labels, through, &frame, look);

    if editing {
        title_editor(ui, annotations, id, band, zoom, title_ink(frame.tint));
    }
    annotations.drawn.insert(id, screen);
    over
}

/// Where a frame is drawn on screen this frame, and how it's lit.
#[derive(Clone, Copy)]
struct FrameLook {
    screen: Rect,
    band: Rect,
    zoom: f32,
    selected: bool,
    hovered: bool,
    editing: bool,
}

fn paint_frame(ui: &Ui, slot: egui::layers::ShapeIdx, labels: &mut Vec<Shape>, through: f32, frame: &Frame, look: FrameLook) {
    let FrameLook { screen, band, zoom, selected, hovered, editing } = look;
    let tint = frame.tint.color();
    let rounding = FRAME_ROUNDING * zoom;
    let mut shapes = Vec::new();

    // A faint wash of the tint, a little deeper across the title band
    let wash = tint.gamma_multiply(if selected { 0.095 } else { 0.07 });
    shapes.push(Shape::rect_filled(screen, rounding, wash));
    let r = CornerRadius::from(rounding);
    let band_rounding = CornerRadius { sw: 0, se: 0, ..r };
    let band_wash = tint.gamma_multiply(if hovered { 0.12 } else { 0.07 });
    shapes.push(Shape::rect_filled(band, band_rounding, band_wash));
    let outline = if selected {
        Stroke::new(1.5_f32, tint.gamma_multiply(0.9))
    } else if hovered {
        Stroke::new(1.0_f32, tint.gamma_multiply(0.5))
    } else {
        Stroke::new(1.0_f32, tint.gamma_multiply(0.28))
    };
    shapes.push(Shape::rect_stroke(screen, rounding, outline, egui::StrokeKind::Middle));

    // The title, in spaced capitals, and a hairline ruled on from it to the
    // edge, as sections are marked out on a printed panel. The title sits
    // over the cables, on the band as it looks beneath them
    let ink = title_ink(frame.tint);
    let inset = FRAME_TITLE_INSET * zoom;
    let mut rule_from = band.left() + inset;
    if !editing && !frame.title.is_empty() {
        let galley = ui.painter().layout_job(title_job(&frame.title, zoom, ink));
        let at = pos2(band.left() + inset, band.center().y - galley.size().y / 2.0);
        rule_from = at.x + galley.size().x + 10.0 * zoom;
        let under = laid_over(laid_over(theme::background::MAIN, wash), band_wash);
        labels.push(label_backing(Rect::from_min_size(at, galley.size()), under, through, zoom));
        labels.push(Shape::galley(at, galley, ink));
    } else if editing {
        rule_from = band.right();
    }
    let rule_to = band.right() - inset;
    if rule_to > rule_from {
        let y = band.center().y;
        shapes.push(Shape::line_segment([pos2(rule_from, y), pos2(rule_to, y)], Stroke::new(1.0_f32, tint.gamma_multiply(0.35))));
    }
    ui.painter().set(slot, Shape::Vec(shapes));
}

/// `top` laid over `bottom`, as the painter blends them.
fn laid_over(bottom: Color32, top: Color32) -> Color32 {
    let keep = 1.0 - top.a() as f32 / 255.0;
    let mix = |b: u8, t: u8| (t as f32 + b as f32 * keep).round().min(255.0) as u8;
    Color32::from_rgba_premultiplied(
        mix(bottom.r(), top.r()),
        mix(bottom.g(), top.g()),
        mix(bottom.b(), top.b()),
        mix(bottom.a(), top.a()),
    )
}

/// The backing lettering on the canvas sits on, over the cables: `under`, the
/// color of the canvas there, around `words` and fading out past them, so a
/// cable passing by dims as it goes behind them rather than stopping short.
/// `through` of the cable still shows.
fn label_backing(words: Rect, under: Color32, through: f32, zoom: f32) -> Shape {
    let inner = words.expand2(LABEL_MARGIN * zoom);
    let outer = inner.expand(LABEL_FEATHER * zoom);
    let solid = under.gamma_multiply(1.0 - through.clamp(0.0, 1.0));
    let mut mesh = Mesh::default();
    for corner in [inner.left_top(), inner.right_top(), inner.right_bottom(), inner.left_bottom()] {
        mesh.colored_vertex(corner, solid);
    }
    for corner in [outer.left_top(), outer.right_top(), outer.right_bottom(), outer.left_bottom()] {
        mesh.colored_vertex(corner, Color32::TRANSPARENT);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    // Each side fades from the inner edge to the outer
    for side in 0..4 {
        let (a, b) = (side, (side + 1) % 4);
        mesh.add_triangle(a, b, b + 4);
        mesh.add_triangle(a, b + 4, a + 4);
    }
    Shape::mesh(mesh)
}

/// A frame title as lettered: capitals, spaced out.
fn title_job(title: &str, zoom: f32, ink: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &title.to_uppercase(),
        0.0,
        TextFormat {
            font_id: title_font(FRAME_TITLE_SIZE * zoom),
            extra_letter_spacing: FRAME_TITLE_TRACKING * zoom,
            color: ink,
            ..Default::default()
        },
    );
    job
}

/// A colour swatch in a frame's menu. The current colour wears a ring.
fn swatch(ui: &mut Ui, tint: Tint, current: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    let center = rect.center();
    let fill = if response.hovered() { tint.color() } else { tint.color().gamma_multiply(0.85) };
    ui.painter().circle_filled(center, 6.0, fill);
    if current {
        ui.painter().circle_stroke(center, 8.0, Stroke::new(1.5_f32, theme::text::PRIMARY));
    }
    response.on_hover_text(tint.label())
}

/// The field a frame's title is renamed in, over the title band.
fn title_editor(ui: &mut Ui, annotations: &mut Annotations, id: AnnotationId, band: Rect, zoom: f32, ink: Color32) {
    let inset = FRAME_TITLE_INSET * zoom;
    let height = FRAME_TITLE_SIZE * zoom * 1.6;
    let rect = Rect::from_min_size(
        pos2(band.left() + inset, band.center().y - height / 2.0),
        vec2((band.width() - 2.0 * inset).max(40.0), height),
    );
    let Some(editing) = annotations.editing.as_mut() else { return };
    ui.painter().rect_filled(rect.expand(3.0 * zoom), 4.0 * zoom, theme::background::MAIN.gamma_multiply(0.8));
    let output = ui
        .scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            egui::TextEdit::singleline(&mut editing.draft)
                .id(edit_id(id))
                .font(title_font(FRAME_TITLE_SIZE * zoom))
                .text_color(ink)
                .hint_text("Name this frame")
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 2)))
                .desired_width(rect.width())
                .show(ui)
        })
        .inner;
    finish_frame_of_editing(ui, annotations, id, output);
}

/// Opens a text field the first frame it's shown, and closes the edit once
/// it lets go of the keyboard: on Enter for a title, Escape, or a click away.
fn finish_frame_of_editing(ui: &Ui, annotations: &mut Annotations, id: AnnotationId, mut output: egui::text_edit::TextEditOutput) {
    let Some(editing) = annotations.editing.as_mut() else { return };
    if !editing.opened {
        editing.opened = true;
        output.response.request_focus();
        if editing.select_all {
            let all = CCursorRange::two(CCursor::new(0), CCursor::new(editing.draft.chars().count()));
            output.state.cursor.set_char_range(Some(all));
            output.state.store(ui.ctx(), edit_id(id));
        }
    } else if output.response.lost_focus() || !output.response.has_focus() {
        annotations.finish_editing();
    }
}

/// A note's text as set: plain runs in the body face, bold ones in the
/// title face and a brighter ink. It wraps at `wrap`, and breaks its lines
/// at `breaks` too: offsets in characters into the text, without the
/// marks for bold.
fn note_job(text: &str, zoom: f32, wrap: f32, breaks: &[usize]) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let mut breaks = breaks.iter().copied().peekable();
    let mut at = 0;
    for (run, bold) in emphasis_runs(text) {
        let format = if bold {
            TextFormat { font_id: title_font(NOTE_TEXT_SIZE * zoom * 0.96), color: NOTE_EMPHASIS, ..Default::default() }
        } else {
            TextFormat { font_id: FontId::proportional(NOTE_TEXT_SIZE * zoom), color: NOTE_INK, ..Default::default() }
        };
        let mut rest = run;
        while let Some(split) = breaks.next_if(|b| *b < at + rest.chars().count()) {
            let byte = rest.char_indices().nth(split - at).map_or(rest.len(), |(i, _)| i);
            job.append(&rest[..byte], 0.0, format.clone());
            job.append("
", 0.0, format.clone());
            rest = &rest[byte..];
            at = split;
        }
        at += rest.chars().count();
        job.append(rest, 0.0, format);
    }
    job
}

/// A note's text, ready to draw at `zoom`.
struct NoteText {
    galley: std::sync::Arc<egui::Galley>,
    /// What to scale the galley by as it's drawn.
    scale: f32,
    /// Its size on screen.
    size: Vec2,
}

/// A note's text `width` wide on the patch, at `zoom`. Fonts are drawn at a
/// whole number of pixels, so text laid out at each zoom is a little wider
/// or narrower than it would be if fonts scaled smoothly, and a word could
/// jump between lines as the view zooms (#255). So the lines break where
/// they do at zoom 1, and the text is drawn scaled to the size it has
/// there: the card keeps its size and its lines as the view zooms.
fn note_text(ui: &Ui, text: &str, width: f32, zoom: f32) -> NoteText {
    let set = ui.painter().layout_job(note_job(text, 1.0, (width - 2.0 * NOTE_PADDING.x).max(1.0), &[]));
    let mut breaks = Vec::new();
    let mut at = 0;
    for row in &set.rows {
        at += row.char_count_including_newline().0;
        if !row.ends_with_newline {
            breaks.push(at);
        }
    }
    // The last line ends where the text does
    breaks.pop();
    let galley = ui.painter().layout_job(note_job(text, zoom, f32::INFINITY, &breaks));
    let size = set.size() * zoom;
    let scale = if galley.size().x > 0.0 { size.x / galley.size().x } else { 1.0 };
    NoteText { galley, scale, size }
}

fn note_ui(
    ui: &mut Ui,
    annotations: &mut Annotations,
    id: AnnotationId,
    backdrop: &mut Backdrop<'_>,
    view: View,
    through: f32,
    menu_actions: &mut Vec<MenuAction>,
) -> bool {
    let Some(Annotation::Note(note)) = annotations.get(id).cloned() else { return false };
    let zoom = view.zoom;
    let selected = annotations.selected.contains(&id);
    let editing = annotations.editing.as_ref().is_some_and(|e| e.id == id);
    let padding = NOTE_PADDING * zoom;
    let text_height = if editing {
        // The field grows as lines are typed; it's measured as it was last drawn
        annotations.drawn.get(&id).map_or(NOTE_TEXT_SIZE * zoom * 1.4, |r| r.height() - 2.0 * padding.y)
    } else {
        note_text(ui, &note.text, note.width, zoom).size.y
    };
    let card = Rect::from_min_size(view.to_screen(note.position), vec2(note.width * zoom, text_height + 2.0 * padding.y));
    let card_slot = ui.painter().add(Shape::Noop);

    // The card moves the note; while editing, the text field has the mouse
    let body = ui.interact(card, Id::unique(("note", id)), if editing { Sense::hover() } else { Sense::click_and_drag() });
    let mut over = body.hovered();
    if pans_view(ui, &body) {
        *backdrop.pan += body.drag_delta();
    } else if body.dragged_by(PointerButton::Primary) {
        if !selected && body.drag_started() {
            select(ui, annotations, id, backdrop);
        }
        if let Some(Annotation::Note(note)) = annotations.get_mut(id) {
            note.position += body.drag_delta() / zoom;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    if body.clicked_by(PointerButton::Primary) {
        select(ui, annotations, id, backdrop);
    }
    if body.double_clicked() {
        annotations.start_editing(id, false);
    }
    if body.secondary_clicked() && !selected {
        select(ui, annotations, id, backdrop);
    }
    crate::widgets::menu::context_menu(&body, |ui| {
        ui.set_min_width(160.0);
        if ui.add(egui::Button::new("Edit").shortcut_text("Double-click")).clicked() {
            menu_actions.push(MenuAction::Edit(id));
            ui.close();
        }
        ui.separator();
        if ui.add(egui::Button::new("Delete note").shortcut_text("Del")).clicked() {
            menu_actions.push(MenuAction::Delete(id));
            ui.close();
        }
    });

    // The right edge sets where the text wraps
    let edge = Rect::from_x_y_ranges(card.right() - GRAB..=card.right() + GRAB, card.top()..=card.bottom());
    let widen = ui.interact(edge, Id::unique(("note edge", id)), Sense::drag());
    over |= widen.hovered();
    if widen.hovered() || widen.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if widen.drag_started_by(PointerButton::Primary) {
        let pointer = widen.interact_pointer_pos().unwrap_or(edge.center());
        ui.ctx().data_mut(|d| d.insert_temp(drag_id(id), Drag::Widen { start: note.width, pointer }));
    }
    if widen.dragged_by(PointerButton::Primary) {
        let held = ui.ctx().data(|d| d.get_temp::<Drag>(drag_id(id)));
        if let (Some(Drag::Widen { start, pointer }), Some(now)) = (held, widen.interact_pointer_pos()) {
            if let Some(Annotation::Note(note)) = annotations.get_mut(id) {
                note.width = (start + (now.x - pointer.x) / zoom).clamp(*NOTE_WIDTHS.start(), *NOTE_WIDTHS.end());
            }
        }
    }
    if widen.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Drag>(drag_id(id)));
    }

    // Drawn where it is now
    let Some(Annotation::Note(note)) = annotations.get(id).cloned() else {
        return over;
    };
    let wrap = (note.width * zoom - 2.0 * padding.x).max(1.0);
    let at = view.to_screen(note.position);
    let (text_rect, lettered) = if editing {
        let rect = Rect::from_min_size(at + padding, vec2(wrap, text_height));
        let Some(editing) = annotations.editing.as_mut() else { return over };
        let output = ui
            .scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                egui::TextEdit::multiline(&mut editing.draft)
                    .id(edit_id(id))
                    .font(FontId::proportional(NOTE_TEXT_SIZE * zoom))
                    .text_color(NOTE_INK)
                    .hint_text("Write a note. **Bold** for emphasis")
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 2)))
                    .desired_width(wrap)
                    .desired_rows(1)
                    .show(ui)
            })
            .inner;
        let rect = output.response.rect;
        finish_frame_of_editing(ui, annotations, id, output);
        (rect, None)
    } else {
        let text = note_text(ui, &note.text, note.width, zoom);
        (Rect::from_min_size(at + padding, text.size), Some(text))
    };
    let card = Rect::from_min_max(at, pos2(at.x + note.width * zoom, text_rect.bottom() + padding.y));

    let rounding = NOTE_ROUNDING * zoom;
    let mut shapes = vec![Shape::rect_filled(card, rounding, NOTE_FILL)];
    let mark = Rect::from_min_size(
        card.min + vec2(0.0, rounding),
        vec2(NOTE_MARK_WIDTH * zoom, (card.height() - 2.0 * rounding).max(0.0)),
    );
    shapes.push(Shape::rect_filled(mark, NOTE_MARK_WIDTH * zoom * 0.5, NOTE_MARK.gamma_multiply(0.55)));
    if selected || editing {
        shapes.push(Shape::rect_stroke(card, rounding, Stroke::new(1.0_f32, NOTE_MARK.gamma_multiply(0.7)), egui::StrokeKind::Middle));
    } else if body.hovered() || widen.hovered() {
        shapes.push(Shape::rect_stroke(card, rounding, Stroke::new(1.0_f32, NOTE_INK.gamma_multiply(0.18)), egui::StrokeKind::Middle));
    }
    if let Some(text) = lettered {
        // A note is lettered over the cables, as a frame's title is
        backdrop.labels.push(label_backing(text_rect, laid_over(theme::background::MAIN, NOTE_FILL), through, zoom));
        backdrop.labels.extend(shapes);
        let mut letters = Shape::galley(text_rect.min, text.galley, NOTE_INK);
        letters.transform(TSTransform::new(text_rect.min.to_vec2() * (1.0 - text.scale), text.scale));
        backdrop.labels.push(letters);
    } else {
        // The text field is drawn under the cables, so the card is too
        ui.painter().set(card_slot, Shape::Vec(shapes));
    }
    annotations.drawn.insert(id, card);
    over
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backing_matches_the_canvas_it_is_laid_on() {
        let canvas = theme::background::MAIN;
        assert_eq!(laid_over(canvas, Color32::TRANSPARENT), canvas);
        assert_eq!(laid_over(canvas, Color32::WHITE), Color32::WHITE);
        // A wash lightens the canvas toward its tint, and stays opaque
        let washed = laid_over(canvas, Tint::ALL[0].color().gamma_multiply(0.07));
        assert_eq!(washed.a(), 255);
        assert_ne!(washed, canvas);
    }

    /// Each line of a galley, as text.
    fn lines(galley: &egui::Galley) -> Vec<String> {
        galley.rows.iter().map(|row| row.glyphs.iter().map(|g| g.chr).collect::<String>().trim().to_string()).collect()
    }

    #[test]
    fn a_note_keeps_its_lines_and_size_as_the_view_zooms() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let text = "The **harp** and the break come in at Groove II, under the kit: a long line to wrap, twice over at least.";
        let mut seen = Vec::new();
        for _ in 0..2 {
            let input = egui::RawInput { viewport_id: egui::ViewportId::ROOT, ..Default::default() };
            ctx.set_pixels_per_point(1.25);
            ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    seen.clear();
                    for step in 0..60 {
                        let zoom = 0.3 * (2.0f32 / 0.3).powf(step as f32 / 59.0);
                        seen.push((zoom, note_text(ui, text, 250.0, zoom)));
                    }
                });
            }).textures_delta.clear();
        }
        // The note as it's set, at zoom 1
        let (one, at_one) = seen.iter().min_by(|a, b| (a.0 - 1.0).abs().total_cmp(&(b.0 - 1.0).abs())).unwrap();
        let (set_lines, set_size) = (lines(&at_one.galley), at_one.size / *one);
        assert!(set_lines.len() >= 3, "{set_lines:?}");
        for (zoom, note) in &seen {
            assert_eq!(lines(&note.galley), set_lines, "lines at zoom {zoom}");
            assert!((note.size / *zoom - set_size).length() < 0.01, "size at zoom {zoom}: {:?}", note.size / *zoom);
            // Drawn within a few percent of the size its font has
            assert!((0.85..1.15).contains(&note.scale), "scale {} at zoom {zoom}", note.scale);
        }
    }
}
