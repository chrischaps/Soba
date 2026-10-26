//! Bringing things into view: F glides the view to fit the selected
//! modules, frames and notes, or everything on the level when nothing is
//! selected (Shift+F, everything either way).
//!
//! The glide is worked out afresh each frame in patch space, so modules
//! measured for the first time on the way (they were never drawn, and were
//! taken at a nominal size) only correct where it's headed.
//!
//! Going into a group, or to a level from the breadcrumb trail, frames it
//! the same way, straight away: all of it in view.

use eframe::egui::{self, emath::TSTransform, vec2, Pos2, Rect, Vec2};
use egui_node_graph2::NodeId;

use crate::app::capture_aim;
use crate::app::cues::{LookAt, LookZoom};
use crate::graph::annotations::{Annotation, AnnotationId};
use crate::graph::{groups, GroupId};
use super::grouping::{FRAME_MARGIN, TRAIL_HEIGHT};
use super::SynthApp;

/// How long the view takes to get there, in seconds.
const GLIDE_SECONDS: f64 = 0.2;
/// The closest framing the selection comes: one module is brought a
/// little past life size.
const SELECTION_ZOOM: f32 = 1.25;
/// The closest framing everything comes: life size, as a patch opens.
const ALL_ZOOM: f32 = 1.0;
/// The furthest the editor zooms out, and the closest it comes in: a
/// `look close` frames as close as this (egui_node_graph2's own limits).
const FURTHEST_ZOOM: f32 = egui_node_graph2::MIN_ZOOM;
const CLOSEST_ZOOM: f32 = egui_node_graph2::MAX_ZOOM;
/// Frames past the end of the glide spent waiting for modules still taken
/// at a nominal size to be measured.
const SETTLE_FRAMES: u8 = 6;

/// What a glide brings into view.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Target {
    /// Everything on the current level, its frames and notes included.
    All,
    Selection { nodes: Vec<NodeId>, annotations: Vec<AnnotationId> },
    /// A module or frame, as close as it fits: a `look close` cue.
    Close { nodes: Vec<NodeId>, annotations: Vec<AnnotationId> },
    /// A patch point, in the middle of the editor at a zoom: a capture's
    /// `aim` cue, which moves as a camera does, easing in and out.
    Point { at: Pos2, zoom: f32 },
}

/// The view on its way to framing something.
pub(super) struct Glide {
    target: Target,
    started: f64,
    /// How long it takes, in seconds: 0 cuts straight there.
    seconds: f64,
    from_zoom: f32,
    /// The patch point that was in the middle of the view.
    from_middle: Pos2,
    /// The zoom and pan this glide left last frame. Anything else that
    /// moves the view (a scroll, a drag) takes over from it.
    left: Option<(f32, Vec2)>,
    settle: u8,
}

/// Where the view is, in patch space: how far it's zoomed, and the patch
/// point in its middle (below the trail). Unlike the editor's pan, it
/// holds still while the view zooms elsewhere and back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct View {
    pub zoom: f32,
    pub looking_at: Pos2,
}

/// Eases out: quick to leave, gentle to arrive.
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Eases in and out, as a camera move starts and stops gently.
fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The zoom that fits `size` (in patch points) into `room` (in screen
/// points), coming no closer than `closest`.
fn zoom_to_fit(size: Vec2, room: Vec2, closest: f32) -> f32 {
    let fit = (room.x / size.x.max(1.0)).min(room.y / size.y.max(1.0));
    fit.min(closest).max(FURTHEST_ZOOM)
}

/// The patch point to put in the middle of a view `view` big to frame
/// `bounds` at `zoom`: their middle if they fit, otherwise, along a side
/// they overrun, their top left, where the signal comes in.
fn aim(bounds: Rect, zoom: f32, view: Vec2) -> Pos2 {
    let (room, middle) = (room(view), middle(view));
    let along = |extent: f32, room: f32, start: f32, centre: f32, middle: f32, inset: f32| {
        if extent * zoom <= room { centre } else { start + (middle - inset) / zoom }
    };
    Pos2::new(
        along(bounds.width(), room.x, bounds.left(), bounds.center().x, middle.x, FRAME_MARGIN),
        along(bounds.height(), room.y, bounds.top(), bounds.center().y, middle.y, TRAIL_HEIGHT + FRAME_MARGIN),
    )
}

/// The room a framing has in a view `view` big: clear of the breadcrumb
/// trail along the top, with a margin all round.
fn room(view: Vec2) -> Vec2 {
    vec2(view.x - 2.0 * FRAME_MARGIN, view.y - TRAIL_HEIGHT - 2.0 * FRAME_MARGIN).max(vec2(1.0, 1.0))
}

/// Where a framing is centred, relative to the editor's corner: the middle
/// of the view below the trail.
fn middle(view: Vec2) -> Vec2 {
    vec2(view.x / 2.0, TRAIL_HEIGHT + (view.y - TRAIL_HEIGHT) / 2.0)
}

/// Where a glide to `target` centres it, relative to the editor's corner:
/// a point aimed at goes in the middle of the editor, as a camera has it.
fn centre(target: &Target, view: Vec2) -> Vec2 {
    match target {
        Target::Point { .. } => view / 2.0,
        _ => middle(view),
    }
}

impl SynthApp {
    /// F: frames the selection, or everything on the level when nothing's
    /// selected. Shift+F frames everything regardless.
    pub(super) fn handle_framing_shortcut(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        // An open chord popover plays its chords on the letter keys, F among them
        if crate::graph::letter_keys_held(ctx) {
            return;
        }
        let (all, fit) = ctx.input_mut(|i| (i.consume_key(Modifiers::SHIFT, Key::F), i.consume_key(Modifiers::NONE, Key::F)));
        if all {
            self.start_glide(ctx, Target::All);
        } else if fit {
            let target = self.selection_target();
            self.start_glide(ctx, target);
        }
    }

    /// What F frames: what's selected, or all of the level if nothing is.
    pub(super) fn selection_target(&self) -> Target {
        let nodes = self.graph_state.selected_nodes.clone();
        let annotations: Vec<AnnotationId> = self.user_state.annotations.selected.iter().copied().collect();
        if nodes.is_empty() && annotations.is_empty() {
            Target::All
        } else {
            Target::Selection { nodes, annotations }
        }
    }

    /// Sets the view gliding to frame `target`.
    pub(super) fn start_glide(&mut self, ctx: &egui::Context, target: Target) {
        self.start_glide_over(ctx, target, GLIDE_SECONDS);
    }

    /// Sets the view gliding to frame `target` over `seconds`, or cutting
    /// straight there for 0: a capture's `fit` cue.
    pub(super) fn start_glide_over(&mut self, ctx: &egui::Context, target: Target, seconds: f64) {
        if !self.editor_rect.is_positive() {
            return;
        }
        let at = self.editor_rect.min + centre(&target, self.editor_rect.size());
        self.glide = Some(Glide {
            target,
            started: ctx.input(|i| i.time),
            seconds,
            from_zoom: self.graph_state.pan_zoom.zoom,
            from_middle: self.screen_to_patch(at),
            left: None,
            settle: SETTLE_FRAMES,
        });
        // The glide does the framing now
        self.fit_pending = 0;
        ctx.request_repaint();
    }

    /// Moves the view one frame along its glide. Called first thing in the
    /// frame, before anything is drawn from the view.
    pub(super) fn step_glide(&mut self, ui: &egui::Ui) {
        let Some(glide) = self.glide.as_ref() else { return };
        let pan_zoom = &self.graph_state.pan_zoom;
        // Scrolled or dragged since: whoever did it has the view now
        if glide.left.is_some_and(|left| left != (pan_zoom.zoom, pan_zoom.pan)) {
            self.glide = None;
            return;
        }
        let view = self.editor_rect.size();
        let Some((to_zoom, to_middle, measured)) = self.destination(&glide.target, view) else {
            self.glide = None;
            return;
        };
        let t = if glide.seconds > 0.0 { ((ui.input(|i| i.time) - glide.started) / glide.seconds) as f32 } else { 1.0 };
        let e = if matches!(glide.target, Target::Point { .. }) { ease_in_out(t) } else { ease(t) };
        // Zoom eases by ratio, so zooming far out doesn't rush its start
        let zoom = glide.from_zoom * (to_zoom / glide.from_zoom).powf(e);
        // A point aimed at is in the middle of the editor; a view's is below the trail
        let looking_at = glide.from_middle.lerp(to_middle, e) + (middle(view) - centre(&glide.target, view)) / zoom;
        self.set_view(ui, View { zoom, looking_at });

        let left = (self.graph_state.pan_zoom.zoom, self.graph_state.pan_zoom.pan);
        let Some(glide) = self.glide.as_mut() else { return };
        glide.left = Some(left);
        if t >= 1.0 {
            // There, unless modules were taken at a nominal size: they're
            // measured once drawn, and the framing follows
            if measured || glide.settle == 0 {
                self.glide = None;
                return;
            }
            glide.settle -= 1;
        }
        ui.ctx().request_repaint();
    }

    /// Where the view is now.
    pub(super) fn view(&self) -> View {
        let at = self.editor_rect.min + middle(self.editor_rect.size());
        View { zoom: self.graph_state.pan_zoom.zoom, looking_at: self.screen_to_patch(at) }
    }

    /// Puts the view there, straight away.
    pub(super) fn set_view(&mut self, ui: &egui::Ui, view: View) {
        let (zoom_before, pan_before) = (self.graph_state.pan_zoom.zoom, self.graph_state.pan_zoom.pan);
        if view.zoom != zoom_before {
            self.graph_state.zoom(ui, view.zoom / zoom_before);
            // Followed now, at the pan it was made at, before the view pans:
            // the frame's own zooms (a scroll) are followed from the new pan
            // once the graph's drawn
            self.history.follow_zoom(zoom_before, pan_before, &self.graph_state.pan_zoom);
        }
        let origin = self.history.view_origin();
        let middle = middle(self.editor_rect.size());
        self.graph_state.pan_zoom.pan = middle - origin - view.looking_at.to_vec2() * self.graph_state.pan_zoom.zoom;
    }

    /// Takes patch space to the screen, as `view` shows it.
    pub(super) fn patch_to_screen(&self, view: View) -> TSTransform {
        let middle = self.editor_rect.min.to_vec2() + middle(self.editor_rect.size());
        TSTransform::new(middle - view.looking_at.to_vec2() * view.zoom, view.zoom)
    }

    /// The view that frames everything on a level, shown or not, as F does
    /// with nothing selected.
    pub(super) fn framed_view(&self, level: Option<GroupId>) -> Option<View> {
        if !self.editor_rect.is_positive() {
            return None;
        }
        let (bounds, _) = self.level_bounds(level)?;
        let view = self.editor_rect.size();
        let zoom = zoom_to_fit(bounds.size(), room(view), ALL_ZOOM);
        Some(View { zoom, looking_at: aim(bounds, zoom, view) })
    }

    /// The view an `aim` at a patch point and zoom ends on: the point in the
    /// middle of the editor, as a camera has it.
    pub(super) fn aimed_view(&self, at: Pos2, zoom: f32) -> View {
        let zoom = zoom.max(FURTHEST_ZOOM);
        let view = self.editor_rect.size();
        let target = Target::Point { at, zoom };
        View { zoom, looking_at: at + (middle(view) - centre(&target, view)) / zoom }
    }

    /// The patch point and zoom an `aim` takes to end on `view`: what a
    /// take writes of where the view was (#489).
    pub(super) fn aim_of(&self, view: View) -> (Pos2, f32) {
        let size = self.editor_rect.size();
        let target = Target::Point { at: view.looking_at, zoom: view.zoom };
        (view.looking_at - (middle(size) - centre(&target, size)) / view.zoom, view.zoom)
    }

    /// The patch-space rectangle around everything on a level, its frames
    /// and notes included, and whether every module, frame and note in it
    /// has been measured.
    pub(super) fn level_bounds(&self, level: Option<GroupId>) -> Option<(Rect, bool)> {
        let nodes = groups::level_nodes(&self.graph_state.graph, level);
        // Frames and notes are only on the top level, and only where they
        // were last drawn while it's on show
        let notes = if level.is_none() && self.user_state.level.is_none() {
            self.user_state.annotations.iter().map(|(id, _)| id).collect()
        } else {
            Vec::new()
        };
        self.bounds_of(&nodes, &notes)
    }

    /// Where a glide to `target` ends: the zoom, the patch point in the
    /// middle of the framing, and whether every module it frames has been
    /// measured.
    fn destination(&self, target: &Target, view: Vec2) -> Option<(f32, Pos2, bool)> {
        if let Target::Point { at, zoom } = *target {
            return Some((zoom.max(FURTHEST_ZOOM), at, true));
        }
        let (bounds, measured) = self.target_bounds(target)?;
        let closest = match target {
            Target::All => ALL_ZOOM,
            Target::Close { .. } => CLOSEST_ZOOM,
            _ => SELECTION_ZOOM,
        };
        let zoom = zoom_to_fit(bounds.size(), room(view), closest);
        Some((zoom, aim(bounds, zoom, view), measured))
    }

    /// The patch-space rectangle around what's framed, and whether every
    /// module in it has been measured.
    fn target_bounds(&self, target: &Target) -> Option<(Rect, bool)> {
        match target {
            Target::All => self.level_bounds(self.user_state.level),
            Target::Selection { nodes, annotations } | Target::Close { nodes, annotations } => {
                let level = groups::level_nodes(&self.graph_state.graph, self.user_state.level);
                let nodes: Vec<NodeId> = nodes.iter().copied().filter(|n| level.contains(n)).collect();
                self.bounds_of(&nodes, annotations)
            }
            Target::Point { .. } => None,
        }
    }

    fn bounds_of(&self, nodes: &[NodeId], notes: &[AnnotationId]) -> Option<(Rect, bool)> {
        let zoom = self.graph_state.pan_zoom.zoom;
        let mut measured = true;
        let modules = nodes.iter().filter_map(|&n| {
            let at = self.history.to_patch(*self.graph_state.node_positions.get(n)?, zoom);
            let size = self.node_sizes.get(&n).copied().unwrap_or_else(|| {
                measured = false;
                groups::NOMINAL_NODE_SIZE
            });
            Some(Rect::from_min_size(at, size))
        });
        let modules: Vec<Rect> = modules.collect();
        let drawn = &self.user_state.annotations.drawn;
        // A frame or note not drawn since the level was shown is left out
        // until it is: where it was drawn before was in another view
        measured &= notes.iter().all(|id| drawn.contains_key(id));
        let notes = notes
            .iter()
            .filter_map(|id| drawn.get(id))
            .map(|r| Rect::from_min_max(self.screen_to_patch(r.min), self.screen_to_patch(r.max)));
        let bounds = modules.into_iter().chain(notes).reduce(|a, b| a.union(b))?;
        Some((bounds, measured))
    }

    /// Where a `look` cue takes the view: a module or frame on show, framed
    /// as F frames it selected or as close as it fits, or its middle at a zoom.
    pub(super) fn look_target(&self, at: &LookAt, zoom: LookZoom) -> Result<Target, String> {
        let (nodes, annotations) = match at {
            LookAt::Module(module, which) => {
                let node = capture_aim::find_module(&self.graph_state.graph, &self.user_state, module, *which)?;
                if !groups::level_nodes(&self.graph_state.graph, self.user_state.level).contains(&node) {
                    return Err("it isn't on the level on show".into());
                }
                (vec![node], Vec::new())
            }
            LookAt::Frame(title) => {
                let frame = self.user_state.annotations.iter().find_map(|(id, a)| match a {
                    Annotation::Frame(frame) if frame.title.eq_ignore_ascii_case(title) => Some(id),
                    _ => None,
                });
                match frame {
                    // Frames are only on the top level
                    Some(id) if self.user_state.level.is_none() => (Vec::new(), vec![id]),
                    Some(_) => return Err("frames are on the top level".into()),
                    None => return Err("the patch has no frame of that title".into()),
                }
            }
        };
        let zoom = match zoom {
            LookZoom::Fit => return Ok(Target::Selection { nodes, annotations }),
            LookZoom::Close => return Ok(Target::Close { nodes, annotations }),
            LookZoom::At(zoom) => zoom,
        };
        let (bounds, _) = self.bounds_of(&nodes, &annotations).ok_or("it hasn't been drawn yet")?;
        Ok(Target::Point { at: bounds.center(), zoom })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_from_start_to_end() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(2.0), 1.0);
        assert!(ease(0.5) > 0.5, "quick to leave");
    }

    #[test]
    fn fits_the_tighter_side() {
        // Twice as wide as the room is tall: the width decides
        let zoom = zoom_to_fit(vec2(2000.0, 500.0), vec2(1000.0, 1000.0), 2.0);
        assert_eq!(zoom, 0.5);
    }

    #[test]
    fn comes_no_closer_than_asked() {
        let one_module = vec2(200.0, 150.0);
        assert_eq!(zoom_to_fit(one_module, vec2(1600.0, 900.0), SELECTION_ZOOM), SELECTION_ZOOM);
        assert_eq!(zoom_to_fit(one_module, vec2(1600.0, 900.0), ALL_ZOOM), ALL_ZOOM);
    }

    #[test]
    fn zooms_out_no_further_than_the_editor() {
        let whole_song = vec2(5900.0, 5000.0);
        assert_eq!(zoom_to_fit(whole_song, vec2(1200.0, 460.0), ALL_ZOOM), FURTHEST_ZOOM);
    }

    #[test]
    fn aims_at_the_middle_of_what_fits() {
        let view = vec2(1280.0, 640.0);
        let bounds = Rect::from_min_size(Pos2::new(100.0, 200.0), vec2(400.0, 300.0));
        assert_eq!(aim(bounds, 1.0, view), bounds.center());
    }

    #[test]
    fn aims_from_the_top_left_of_what_overruns() {
        // Fits across, but not down: its top sits below the trail
        let view = vec2(1280.0, 640.0);
        let bounds = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(5000.0, 5000.0));
        let zoom = 0.2;
        let at = aim(bounds, zoom, view);
        assert_eq!(at.x, bounds.center().x);
        let top_on_screen = middle(view).y - (at.y - bounds.top()) * zoom;
        assert!((top_on_screen - (TRAIL_HEIGHT + FRAME_MARGIN)).abs() < 1e-3);
    }

    #[test]
    fn centres_below_the_trail() {
        let view = vec2(1000.0, 656.0);
        assert_eq!(middle(view), vec2(500.0, TRAIL_HEIGHT + 300.0));
    }
}
