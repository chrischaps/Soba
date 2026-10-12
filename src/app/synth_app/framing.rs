//! Bringing things into view: F glides the view to fit the selected
//! modules, frames and notes, or everything on the level when nothing is
//! selected (Shift+F, everything either way).
//!
//! The glide is worked out afresh each frame in patch space, so modules
//! measured for the first time on the way (they were never drawn, and were
//! taken at a nominal size) only correct where it's headed.

use eframe::egui::{self, vec2, Pos2, Rect, Vec2};
use egui_node_graph2::NodeId;

use crate::graph::annotations::AnnotationId;
use crate::graph::groups;
use super::grouping::{FRAME_MARGIN, TRAIL_HEIGHT};
use super::SynthApp;

/// How long the view takes to get there, in seconds.
const GLIDE_SECONDS: f64 = 0.2;
/// The closest framing the selection comes: one module is brought a
/// little past life size.
const SELECTION_ZOOM: f32 = 1.25;
/// The closest framing everything comes: life size, as a patch opens.
const ALL_ZOOM: f32 = 1.0;
/// The furthest the editor zooms out (egui_node_graph2's own limit).
const FURTHEST_ZOOM: f32 = 0.2;
/// Frames past the end of the glide spent waiting for modules still taken
/// at a nominal size to be measured.
const SETTLE_FRAMES: u8 = 6;

/// What a glide brings into view.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Target {
    /// Everything on the current level, its frames and notes included.
    All,
    Selection { nodes: Vec<NodeId>, annotations: Vec<AnnotationId> },
}

/// The view on its way to framing something.
pub(super) struct Glide {
    target: Target,
    started: f64,
    from_zoom: f32,
    /// The patch point that was in the middle of the view.
    from_middle: Pos2,
    /// The zoom and pan this glide left last frame. Anything else that
    /// moves the view (a scroll, a drag) takes over from it.
    left: Option<(f32, Vec2)>,
    settle: u8,
}

/// Eases out: quick to leave, gentle to arrive.
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
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
        if !self.editor_rect.is_positive() {
            return;
        }
        let at = self.editor_rect.min + middle(self.editor_rect.size());
        self.glide = Some(Glide {
            target,
            started: ctx.input(|i| i.time),
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
        let Some((bounds, measured)) = self.target_bounds(&glide.target) else {
            self.glide = None;
            return;
        };
        let view = self.editor_rect.size();
        let closest = if glide.target == Target::All { ALL_ZOOM } else { SELECTION_ZOOM };
        let to_zoom = zoom_to_fit(bounds.size(), room(view), closest);
        let t = ((ui.input(|i| i.time) - glide.started) / GLIDE_SECONDS) as f32;
        let e = ease(t);
        // Zoom eases by ratio, so zooming far out doesn't rush its start
        let zoom = glide.from_zoom * (to_zoom / glide.from_zoom).powf(e);
        let looking_at = glide.from_middle.lerp(aim(bounds, to_zoom, view), e);

        let (zoom_before, pan_before) = (self.graph_state.pan_zoom.zoom, self.graph_state.pan_zoom.pan);
        self.graph_state.zoom(ui, zoom / zoom_before);
        // Followed now, at the pan it was made at, before the glide pans:
        // the frame's own zooms (a scroll that interrupts) are followed from
        // the new pan once the graph's drawn
        self.history.follow_zoom(zoom_before, pan_before, &self.graph_state.pan_zoom);
        let origin = self.history.view_origin();
        self.graph_state.pan_zoom.pan = middle(view) - origin - looking_at.to_vec2() * self.graph_state.pan_zoom.zoom;

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

    /// The patch-space rectangle around what's framed, and whether every
    /// module in it has been measured.
    fn target_bounds(&self, target: &Target) -> Option<(Rect, bool)> {
        let graph = &self.graph_state.graph;
        let level = groups::level_nodes(graph, self.user_state.level);
        let (nodes, notes): (Vec<NodeId>, Vec<AnnotationId>) = match target {
            // Frames and notes are only on the top level
            Target::All => (
                level.clone(),
                if self.user_state.level.is_none() { self.user_state.annotations.drawn.keys().copied().collect() } else { Vec::new() },
            ),
            Target::Selection { nodes, annotations } => {
                (nodes.iter().copied().filter(|n| level.contains(n)).collect(), annotations.clone())
            }
        };
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
        let notes = notes
            .iter()
            .filter_map(|id| drawn.get(id))
            .map(|r| Rect::from_min_max(self.screen_to_patch(r.min), self.screen_to_patch(r.max)));
        let bounds = modules.into_iter().chain(notes).reduce(|a, b| a.union(b))?;
        Some((bounds, measured))
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
