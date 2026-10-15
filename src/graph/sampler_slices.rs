//! The Sampler's slice markers, sliced by Onsets: drawn over the waveform
//! where each slice begins, and moved by hand.
//!
//! Each marker is a rule with a tab at the head of the panel, where the
//! loop's flags would be (they step aside while sliced). Drag one to move
//! it; drag it off the panel, up or down, to take it away; double-click
//! the waveform to add one. Markers outside Start–End, or past the 63 that
//! cut it into 64 slices, cut nothing, and are drawn faint. Let go of one
//! with Alt held to hear the slice it now begins.
//!
//! Every edit sets the whole list, as markers placed by hand at the
//! Sampler's Sensitivity (see [`HandMarkers`]): the node keeps it, a patch
//! saves it, and undo takes it back a gesture at a time.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::dsp::onsets::HandMarkers;
use crate::dsp::primitives::Markers;

use super::SynthResponse;

/// How far past the panel's top or bottom (in points) a marker dragged
/// away is taken away.
const PULL_OFF: f32 = 14.0;

/// A marker being dragged: the others, as they were when it was picked up.
#[derive(Clone, Default)]
struct Drag {
    others: Vec<f32>,
}

/// The slice markers on one Sampler's display, and what the pointer does to them.
pub struct SliceMarkers<'a> {
    pub node_id: NodeId,
    /// Where they are, as fractions of the recording, in order.
    pub places: &'a [f32],
    /// The ones that cut Start–End.
    pub cutting: Markers,
    pub sensitivity: f32,
}

impl SliceMarkers<'_> {
    fn drag_id(&self, ui: &egui::Ui) -> egui::Id {
        ui.id().with(("sampler_slice_drag", self.node_id))
    }

    /// Whether one is being dragged.
    pub fn dragging(&self, ui: &egui::Ui) -> bool {
        // A drag that ended elsewhere (the node deleted under it) is over
        ui.input(|i| i.pointer.any_down()) && ui.ctx().data(|d| d.get_temp::<Drag>(self.drag_id(ui))).is_some()
    }

    /// The marker within `grab` points of `x`, nearest first, and how far it is.
    pub fn nearest(&self, x: f32, x_at: impl Fn(f32) -> f32, grab: f32) -> Option<(usize, f32)> {
        self.places
            .iter()
            .enumerate()
            .map(|(m, &place)| (m, (x_at(place) - x).abs()))
            .filter(|&(_, distance)| distance <= grab)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Which slice marker `m` begins: 2 for the first that cuts.
    pub fn slice_of(&self, m: usize) -> Option<usize> {
        let place = *self.places.get(m)?;
        self.cutting.as_slice().iter().position(|&cut| cut == place).map(|index| index + 2)
    }

    /// Draws the markers, `hot` lit, and the one being dragged where the
    /// pointer is (dashed, if letting go would take it away).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, ui: &egui::Ui, painter: &egui::Painter, rect: Rect, plot: Rect, x_at: impl Fn(f32) -> f32, hot: Option<usize>, z: f32) {
        for (m, &place) in self.places.iter().enumerate() {
            let cuts = self.cutting.as_slice().contains(&place);
            let lit = hot == Some(m);
            let alpha = match (lit, cuts) {
                (true, _) => 1.0,
                (false, true) => 0.7,
                (false, false) => 0.22,
            };
            tab(painter, plot, x_at(place), Color32::WHITE.gamma_multiply(alpha), lit, z);
        }
        // The one in hand
        let Some(pointer) = ui.ctx().pointer_latest_pos().filter(|_| self.dragging(ui)) else { return };
        let x = pointer.x.clamp(plot.left(), plot.right());
        if pulled_off(rect, pointer, z) {
            let line = [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())];
            painter.extend(Shape::dashed_line(&line, Stroke::new(1.0 * z, crate::app::theme::accent::WARNING), 2.0 * z, 2.0 * z));
        } else {
            tab(painter, plot, x, Color32::WHITE, true, z);
        }
    }

    /// Picks up a marker, if the press was on one: call as a drag starts.
    pub fn pick_up(&self, ui: &egui::Ui, m: usize) {
        let others = self.places.iter().enumerate().filter(|&(i, _)| i != m).map(|(_, &place)| place).collect();
        ui.ctx().data_mut(|d| d.insert_temp(self.drag_id(ui), Drag { others }));
    }

    /// The markers as the drag leaves them, while one is held: moved to
    /// `place`, or taken away if the pointer is off the panel.
    pub fn drag(&self, ui: &egui::Ui, rect: Rect, pointer: Pos2, place: f32, z: f32) -> Option<SynthResponse> {
        let drag: Drag = ui.ctx().data(|d| d.get_temp(self.drag_id(ui)))?;
        let mut places = drag.others;
        let label = if pulled_off(rect, pointer, z) {
            "Remove slice marker"
        } else {
            places.push(place);
            "Move slice marker"
        };
        Some(self.set(places, label))
    }

    /// The slice the marker in hand begins as the drag has left it, from 0
    /// as [`crate::dsp::primitives::Slices`] counts: none once it's pulled
    /// off, or where it cuts nothing. Call before [`put_down`](Self::put_down).
    pub fn in_hand_begins(&self, ui: &egui::Ui) -> Option<usize> {
        let drag: Drag = ui.ctx().data(|d| d.get_temp(self.drag_id(ui)))?;
        let place = self.places.iter().find(|place| !drag.others.contains(place))?;
        self.cutting.as_slice().iter().position(|cut| cut == place).map(|index| index + 1)
    }

    /// Puts down the marker in hand.
    pub fn put_down(&self, ui: &egui::Ui) {
        ui.ctx().data_mut(|d| d.remove::<Drag>(self.drag_id(ui)));
    }

    /// A marker added at `place`.
    pub fn add(&self, place: f32) -> SynthResponse {
        let mut places = self.places.to_vec();
        places.push(place);
        self.set(places, "Add slice marker")
    }

    fn set(&self, places: Vec<f32>, label: &str) -> SynthResponse {
        SynthResponse::SetSliceMarkers {
            node_id: self.node_id,
            label: label.to_string(),
            markers: HandMarkers::new(self.sensitivity, places),
        }
    }
}

/// Whether the pointer has pulled a marker off the panel, to take it away.
fn pulled_off(rect: Rect, pointer: Pos2, z: f32) -> bool {
    pointer.y < rect.top() - PULL_OFF * z || pointer.y > rect.bottom() + PULL_OFF * z
}

/// A marker: a tab at the head of the panel, the handle, and a rule down
/// it, faint until it's the one the pointer is on, so a break cut into
/// many slices is still a waveform rather than a fence.
fn tab(painter: &egui::Painter, plot: Rect, x: f32, color: Color32, lit: bool, z: f32) {
    let rule = if lit { Stroke::new(1.4 * z, color) } else { Stroke::new(1.0 * z, color.gamma_multiply(0.4)) };
    painter.vline(x, plot.y_range(), rule);
    let head = Rect::from_min_size(Pos2::new(x - 2.0 * z, plot.top()), Vec2::new(4.0 * z, 5.0 * z));
    painter.add(Shape::convex_polygon(
        vec![head.left_top(), head.right_top(), Pos2::new(head.right(), head.bottom() - 1.5 * z), Pos2::new(x, head.bottom()), Pos2::new(head.left(), head.bottom() - 1.5 * z)],
        color,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on a frame's panel, as a display draws on one.
    fn in_frame<R>(ctx: &egui::Context, f: impl FnOnce(&egui::Ui) -> R) -> R {
        let (mut f, mut out) = (Some(f), None);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(f) = f.take() {
                    out = Some(f(ui));
                }
            });
        });
        out.expect("the panel was drawn")
    }

    /// Markers at `places`, cutting Start 0.1 to End 0.9.
    fn markers(places: &[f32]) -> SliceMarkers<'_> {
        SliceMarkers { node_id: NodeId::default(), places, cutting: Markers::between(places, 0.1, 0.9), sensitivity: 0.5 }
    }

    #[test]
    fn a_marker_in_hand_names_the_slice_it_now_begins() {
        let ctx = egui::Context::default();
        assert_eq!(in_frame(&ctx, |ui| markers(&[0.2, 0.5, 0.7]).in_hand_begins(ui)), None, "nothing is in hand yet");
        in_frame(&ctx, |ui| markers(&[0.2, 0.5, 0.7]).pick_up(ui, 1));
        assert_eq!(in_frame(&ctx, |ui| markers(&[0.2, 0.5, 0.7]).in_hand_begins(ui)), Some(2), "picked up, it begins slice 3");

        // Dragged past the third, it begins the last of four
        assert_eq!(in_frame(&ctx, |ui| markers(&[0.2, 0.7, 0.8]).in_hand_begins(ui)), Some(3));
        // Past End it cuts nothing, and pulled off the panel it's gone
        assert_eq!(in_frame(&ctx, |ui| markers(&[0.2, 0.7, 0.95]).in_hand_begins(ui)), None);
        assert_eq!(in_frame(&ctx, |ui| markers(&[0.2, 0.7]).in_hand_begins(ui)), None);

        // Put down, there's none in hand
        in_frame(&ctx, |ui| markers(&[0.2, 0.7, 0.8]).put_down(ui));
        assert_eq!(in_frame(&ctx, |ui| markers(&[0.2, 0.7, 0.8]).in_hand_begins(ui)), None);
    }
}
