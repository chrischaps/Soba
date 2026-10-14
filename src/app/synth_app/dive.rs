//! Stepping into a group, and back out of it, as a zoom through the
//! miniature on its face.
//!
//! Going in, the view glides onto the miniature while the rest of the level
//! fades away. Halfway there the level changes over, and the inside grows
//! out of the miniature into place, each module coming from where the
//! miniature showed it, as the miniature fades from over them. Coming out
//! is the same journey backwards: the inside shrinks into its miniature,
//! and the level around it comes back. Holding Shift steps straight there,
//! as the breadcrumb trail always does.
//!
//! The zoom is only in how a frame is drawn. The editor's own zoom and pan
//! sit where the level is framed, as an instant step leaves them, and the
//! shapes the canvas draws are scaled about the miniature once they're
//! drawn. So the zoom can go past the editor's limits both ways, and when
//! it ends, nothing is left to settle. The inside is framed to fit, so it
//! can be at a different zoom from the level outside.

use std::collections::HashMap;

use eframe::egui::{self, emath::TSTransform, layers::ShapeIdx, LayerId, Rect};
use egui_node_graph2::NodeId;

use crate::graph::groups::{self, GroupIndex, Preview};
use crate::graph::{miniature_transform, paint_miniature, paint_open_pill, GroupId};
use super::super::theme;
use super::framing::View;
use super::SynthApp;

/// How long the zoom takes, in seconds.
const DIVE_SECONDS: f64 = 0.36;
/// How far along the zoom the level changes over: halfway by ratio, where
/// the miniature has grown as far as the inside still has to shrink.
const CUT: f32 = 0.5;
/// How far along the miniature, drawn over the inside it stands for, has
/// faded away.
const MINIATURE_GONE: f32 = 0.85;
/// How far along going in the Open pill on the miniature has faded.
const PILL_GONE: f32 = 0.3;

/// Where each group's miniature sits on its node, per unit of zoom.
pub(super) type MiniaturePlaces = HashMap<GroupId, Rect>;

/// The view on its way into a group, or out of one.
pub(super) struct Dive {
    group: GroupId,
    /// The group's node, on the level outside it.
    node: NodeId,
    inward: bool,
    started: f64,
    /// The level has changed over, at the cut.
    switched: bool,
    /// The view of the level not on show: the one being gone to until the
    /// cut, then the one left.
    other_view: View,
    /// Going in from under the pointer, where the miniature showed Open.
    lit: bool,
}

/// How a frame of a dive is drawn.
pub(super) struct DiveFrame {
    /// From where the canvas drew this frame to where the zoom has got to.
    transform: TSTransform,
    /// The miniature drawn over it, on screen, and the zoom it's drawn at.
    miniature: Rect,
    zoom: f32,
    preview: Preview,
    lit: bool,
    /// How far the level around the miniature has faded, how much of the
    /// miniature is left over the inside, and of its Open pill.
    veil: f32,
    shown: f32,
    pill: f32,
}

/// Quick in the middle, gentle at both ends.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (2.0 - 2.0 * t).powi(3) / 2.0 }
}

/// 0 to 1 as `x` goes from 0 to 1, smoothly.
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The camera `u` of the way from the level outside a group (0) to its
/// inside (1), where `to_miniature` takes the inside, framed, onto the
/// miniature. It zooms by ratio about the one point both leave in place.
fn camera(to_miniature: TSTransform, u: f32) -> TSTransform {
    let s = to_miniature.scaling;
    if (1.0 - s).abs() < 1e-4 {
        return TSTransform::from_translation(-to_miniature.translation * u);
    }
    let still = to_miniature.translation / (1.0 - s);
    let scaling = s.powf(-u);
    TSTransform::new(still * (1.0 - scaling), scaling)
}

/// How far the level around the miniature has faded, and how much of the
/// miniature still shows, `u` of the way in.
fn fades(u: f32) -> (f32, f32) {
    if u <= CUT {
        (smooth(u / CUT), 1.0)
    } else {
        (1.0 - smooth((u - CUT) / (1.0 - CUT)), 1.0 - smooth((u - CUT) / (MINIATURE_GONE - CUT)))
    }
}

impl SynthApp {
    /// Steps into a group by zooming through its miniature, or straight in
    /// with Shift held, or where there's no miniature to zoom through.
    pub(super) fn dive_into(&mut self, ctx: &egui::Context, node: NodeId) {
        self.land_dive();
        let graph = &self.graph_state.graph;
        let Some(id) = graph.nodes.get(node).and_then(|n| n.user_data.kind.group()) else { return };
        let on_show = graph[node].user_data.parent == self.user_state.level;
        let view = self.framed_view(Some(id)).filter(|_| on_show && self.can_dive(ctx, id));
        let Some(view) = view else {
            self.enter_group(node);
            return;
        };
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let lit = pointer.is_some_and(|p| self.user_state.miniatures.get(&id).is_some_and(|r| r.contains(p)));
        self.start_dive(ctx, Dive { group: id, node, inward: true, started: 0.0, switched: false, other_view: view, lit });
    }

    /// A capture's `enter` cue: dives into the group a name or path stands
    /// for (see [`groups::find_group`]) if it's on the level on show, or
    /// goes straight there if it isn't.
    pub(super) fn dive_into_named(&mut self, ctx: &egui::Context, name: &str) -> Result<(), String> {
        let graph = &self.graph_state.graph;
        let position = |n| self.graph_state.node_positions.get(n).copied();
        let id = groups::find_group(graph, position, self.user_state.level, name)?;
        let node = GroupIndex::of(graph).node(id).ok_or("it has no node")?;
        if graph[node].user_data.parent == self.user_state.level {
            self.dive_into(ctx, node);
        } else {
            self.go_to_level(Some(id));
        }
        Ok(())
    }

    /// Steps out of the group on show by shrinking it into its miniature,
    /// or straight out with Shift held. Returns whether there was a group
    /// to leave.
    pub(super) fn dive_out(&mut self, ctx: &egui::Context) -> bool {
        self.land_dive();
        let Some(level) = self.user_state.level else { return false };
        let node = GroupIndex::of(&self.graph_state.graph).node(level);
        let view = self.view_out_of(level).filter(|_| self.can_dive(ctx, level));
        let (Some(node), Some(view)) = (node, view) else { return self.step_out() };
        self.start_dive(ctx, Dive { group: level, node, inward: false, started: 0.0, switched: false, other_view: view, lit: false });
        true
    }

    /// Whether stepping into or out of a group can zoom: not with Shift held,
    /// and only through a miniature that's been drawn with something in it.
    fn can_dive(&self, ctx: &egui::Context, id: GroupId) -> bool {
        !ctx.input(|i| i.modifiers.shift)
            && self.editor_rect.is_positive()
            && self.miniature_places.contains_key(&id)
            && !self.preview_of(id).boxes.is_empty()
    }

    fn start_dive(&mut self, ctx: &egui::Context, dive: Dive) {
        self.dive = Some(Dive { started: ctx.input(|i| i.time), ..dive });
        // A glide would fight it for the view
        self.glide = None;
        ctx.request_repaint();
    }

    /// Finishes a dive on the spot: the level it was going to, now.
    pub(super) fn land_dive(&mut self) {
        let Some(dive) = self.dive.take() else { return };
        if !dive.switched && self.user_state.level == self.dive_from(&dive) {
            self.switch_dive(&dive);
        }
    }

    /// The level a dive sets out from.
    fn dive_from(&self, dive: &Dive) -> Option<GroupId> {
        if dive.inward {
            self.graph_state.graph.nodes.get(dive.node).and_then(|n| n.user_data.parent)
        } else {
            Some(dive.group)
        }
    }

    fn switch_dive(&mut self, dive: &Dive) {
        if dive.inward {
            self.enter_group(dive.node);
        } else {
            self.step_out();
        }
    }

    /// Moves a dive on to this frame, changing the level over once it's
    /// halfway, and works out how the frame is drawn. Called before the
    /// canvas is drawn.
    pub(super) fn step_dive(&mut self, ui: &egui::Ui) -> Option<DiveFrame> {
        let ctx = ui.ctx();
        let dive = self.dive.as_ref()?;
        let t = ((ctx.input(|i| i.time) - dive.started) / DIVE_SECONDS) as f32;
        let u = if dive.inward { ease(t) } else { 1.0 - ease(t) };
        // Gone (undone, deleted), or the level changed some other way: let it go
        let graph = &self.graph_state.graph;
        let outer = graph.nodes.get(dive.node).and_then(|n| n.user_data.parent);
        let expected = if dive.inward != dive.switched { outer } else { Some(dive.group) };
        let here = graph.nodes.get(dive.node).is_some_and(|n| n.user_data.kind.group() == Some(dive.group));
        if !here || self.user_state.level != expected {
            self.dive = None;
            return None;
        }
        let past_cut = if dive.inward { u >= CUT } else { u <= CUT };
        if t >= 1.0 || (past_cut && !dive.switched) {
            let mut dive = self.dive.take()?;
            if !dive.switched {
                let view = self.view();
                self.switch_dive(&dive);
                // The level changed over is drawn this frame, where it's framed
                self.apply_level_view(ui);
                dive.switched = true;
                dive.other_view = view;
            }
            if t >= 1.0 {
                return None;
            }
            self.dive = Some(dive);
        }
        ctx.request_repaint();
        self.dive_frame(u)
    }

    /// How the frame `u` of the way in is drawn.
    fn dive_frame(&self, u: f32) -> Option<DiveFrame> {
        let dive = self.dive.as_ref()?;
        let shows_outer = dive.inward != dive.switched;
        let here = self.view();
        let (outer, inner) = if shows_outer { (here, dive.other_view) } else { (dive.other_view, here) };
        // Node positions, and the miniature's layout, are at the editor's zoom
        let zoom = self.graph_state.pan_zoom.zoom;
        let origin = self.history.view_origin();

        // The miniature on the level outside, where that level's view puts it
        let place = self.miniature_places.get(&dive.group)?;
        let node = self.history.to_patch(*self.graph_state.node_positions.get(dive.node)?, zoom);
        let corner = self.patch_to_screen(outer) * node;
        let miniature = Rect::from_min_size(corner + place.min.to_vec2() * outer.zoom, place.size() * outer.zoom);
        let preview = self.preview_of(dive.group);
        let to_face = miniature_transform(miniature, &preview, outer.zoom)?;
        // The inside as it's framed, from the miniature's layout (node
        // positions, unzoomed), taken onto the miniature
        let inside = self.patch_to_screen(inner) * TSTransform::from_translation(-origin / zoom);
        let to_miniature = to_face * inside.inverse();

        let camera = camera(to_miniature, u);
        let (veil, shown) = fades(u);
        Some(DiveFrame {
            transform: if shows_outer { camera } else { camera * to_miniature },
            miniature: camera * miniature,
            zoom: outer.zoom * camera.scaling,
            preview,
            lit: dive.lit,
            veil,
            shown,
            pill: if dive.lit { 1.0 - smooth(u / PILL_GONE) } else { 0.0 },
        })
    }

    /// Where, before it's moved, the canvas must draw to fill the view once
    /// it has been.
    pub(super) fn dive_canvas(frame: Option<&DiveFrame>, view: Rect) -> Rect {
        frame.map_or(view, |f| f.transform.inverse() * view)
    }

    /// Notes where each group's miniature was drawn this frame, relative to
    /// its node, so a dive out of the group knows where to shrink to.
    pub(super) fn note_miniatures(&mut self) {
        let index = GroupIndex::of(&self.graph_state.graph);
        let zoom = self.graph_state.pan_zoom.zoom;
        let origin = self.editor_rect.min.to_vec2() + self.graph_state.pan_zoom.pan;
        for (&id, rect) in &self.user_state.miniatures {
            let Some(at) = index.node(id).and_then(|n| self.graph_state.node_positions.get(n)) else { continue };
            let corner = origin + at.to_vec2();
            let place = Rect::from_min_size(((rect.min.to_vec2() - corner) / zoom).to_pos2(), rect.size() / zoom);
            self.miniature_places.insert(id, place);
        }
    }

    /// Moves what the canvas drew in `ranges` of `layer` to where the zoom
    /// has got to, then draws the miniature over it and fades the level
    /// around it.
    pub(super) fn paint_dive(&self, ui: &egui::Ui, layer: LayerId, ranges: &[(ShapeIdx, ShapeIdx)], frame: &DiveFrame) {
        let view = self.editor_rect;
        ui.ctx().graphics_mut(|g| {
            let list = g.entry(layer);
            for &(start, end) in ranges {
                list.transform_range(start, end, frame.transform);
                for i in start.0..end.0 {
                    list.mutate_shape(ShapeIdx(i), |shape| shape.clip_rect = shape.clip_rect.intersect(view));
                }
            }
        });

        let painter = ui.painter().with_clip_rect(view);
        // The level around the miniature fades to the canvas, the edges
        // tucked under the miniature's rounded corners
        let hole = frame.miniature.shrink(6.0 * frame.zoom);
        let veil = theme::background::MAIN.gamma_multiply(frame.veil);
        let around = [
            Rect::from_min_max(view.min, egui::pos2(view.max.x, hole.min.y)),
            Rect::from_min_max(egui::pos2(view.min.x, hole.max.y), view.max),
            Rect::from_min_max(egui::pos2(view.min.x, hole.min.y), egui::pos2(hole.min.x, hole.max.y)),
            Rect::from_min_max(egui::pos2(hole.max.x, hole.min.y), egui::pos2(view.max.x, hole.max.y)),
        ];
        for rect in around.map(|r| r.intersect(view)).into_iter().filter(|r| r.is_positive()) {
            painter.rect_filled(rect, 0.0, veil);
        }
        if frame.shown > 0.0 {
            let mut painter = painter.clone();
            painter.set_opacity(frame.shown);
            paint_miniature(&painter, frame.miniature, &frame.preview, frame.zoom, frame.lit);
            if frame.pill > 0.0 {
                painter.set_opacity(frame.pill);
                paint_open_pill(&painter, frame.miniature, frame.zoom);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    #[test]
    fn eases_from_start_to_end() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(0.5), 0.5);
        assert_eq!(ease(1.0), 1.0);
        assert!(ease(0.1) < 0.1, "gentle to leave");
        assert!(ease(0.9) > 0.9, "gentle to arrive");
    }

    /// A framed inside taken onto a miniature a fifth its size.
    fn to_miniature() -> TSTransform {
        TSTransform::new(vec2(300.0, 200.0), 0.2)
    }

    #[test]
    fn camera_starts_outside_and_ends_in_the_inside() {
        let m = to_miniature();
        let p = pos2(640.0, 400.0);
        assert_eq!(camera(m, 0.0) * p, p, "the level outside, as it was");
        // All the way in, the inside is drawn where it's framed
        let inside = camera(m, 1.0) * m;
        assert!((inside.scaling - 1.0).abs() < 1e-5);
        assert!(inside.translation.length() < 1e-3, "{:?}", inside.translation);
    }

    #[test]
    fn camera_zooms_about_a_still_point() {
        let m = to_miniature();
        let still = m.translation / (1.0 - m.scaling);
        for u in [0.25, 0.5, 0.75] {
            let at = camera(m, u) * still.to_pos2();
            assert!((at - still.to_pos2()).length() < 1e-3);
        }
    }

    #[test]
    fn halfway_the_miniature_and_the_inside_are_the_same_size() {
        let m = to_miniature();
        let outer = camera(m, CUT).scaling;
        let inner = (camera(m, CUT) * m).scaling;
        assert!((outer * m.scaling - inner).abs() < 1e-5);
        assert!((inner * outer - 1.0).abs() < 1e-4, "the miniature grows as far as the inside shrinks");
    }

    #[test]
    fn the_level_fades_out_and_the_inside_in() {
        assert_eq!(fades(0.0), (0.0, 1.0), "nothing faded yet, the miniature shows");
        assert_eq!(fades(CUT), (1.0, 1.0), "only the miniature at the cut");
        assert_eq!(fades(1.0), (0.0, 0.0), "the inside, as it's framed");
        assert_eq!(fades(MINIATURE_GONE).1, 0.0);
    }

    #[test]
    fn a_miniature_the_same_size_only_pans() {
        let m = TSTransform::new(vec2(40.0, -20.0), 1.0);
        let half = camera(m, 0.5);
        assert_eq!(half.scaling, 1.0);
        assert_eq!(half.translation, vec2(-20.0, 10.0));
    }
}
