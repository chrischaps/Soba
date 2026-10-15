//! Which cable carries a knob's CV (#220). Hovering a knob with a cable in
//! its jack picks that cable out: it glows and the rest dim. Hovering a
//! cable lights the bead on the knob it feeds. Either way it's one jack
//! that's lit, fading in and out.
//!
//! The knobs and the cables are drawn in the same pass, so what the pointer
//! is on is noted as a frame is drawn and lit in the next.

use egui_node_graph2::InputId;

/// How long the light takes to come up or go down, in seconds
const FADE: f32 = 0.12;

#[derive(Default)]
pub struct CableSpotlight {
    /// The jack lit now, and how strongly (0 to 1)
    lit: Option<InputId>,
    strength: f32,
    /// The jack of a patched knob the pointer is on, noted while drawing
    knob: Option<InputId>,
}

impl CableSpotlight {
    /// While drawing: the pointer is on a knob whose jack has a cable in it
    pub fn hover_knob(&mut self, jack: InputId) {
        self.knob = Some(jack);
    }

    /// Once the graph is drawn: what to light next frame, from the knob the
    /// pointer was on, or else the cable it was on, `dt` seconds on
    pub fn end_frame(&mut self, cable: Option<InputId>, dt: f32) {
        let target = self.knob.take().or(cable);
        let step = dt / FADE;
        match target {
            Some(jack) => {
                // Moving straight from one jack to the next keeps the light up
                self.lit = Some(jack);
                self.strength = (self.strength + step).min(1.0);
            }
            None => {
                self.strength = (self.strength - step).max(0.0);
                if self.strength == 0.0 {
                    self.lit = None;
                }
            }
        }
    }

    /// The cable picked out, by the jack it plugs into, and how strongly
    pub fn cable(&self) -> Option<(InputId, f32)> {
        self.lit.map(|jack| (jack, self.strength))
    }

    /// How strongly the knob with this jack is lit, 0 when it isn't
    pub fn knob(&self, jack: InputId) -> f32 {
        if self.lit == Some(jack) {
            self.strength
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{SynthGraph, SynthGraphState, SynthNodeTemplate};
    use egui_node_graph2::NodeTemplateTrait;

    /// Two jacks of a filter: Cutoff and Resonance
    fn jacks() -> (InputId, InputId) {
        let mut graph = SynthGraph::default();
        let mut user_state = SynthGraphState::new();
        let template = SynthNodeTemplate::from_module_id("filter.svf").unwrap();
        let node = graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        );
        let node = &graph[node];
        (node.get_input("Cutoff").unwrap(), node.get_input("Resonance").unwrap())
    }

    #[test]
    fn nothing_is_lit_until_something_is_hovered() {
        let (a, _) = jacks();
        let mut spot = CableSpotlight::default();
        spot.end_frame(None, 1.0 / 60.0);
        assert_eq!(spot.cable(), None);
        assert_eq!(spot.knob(a), 0.0);
    }

    #[test]
    fn a_hovered_knob_lights_its_cable_and_lets_go_after() {
        let (a, b) = jacks();
        let mut spot = CableSpotlight::default();
        for _ in 0..30 {
            spot.hover_knob(a);
            spot.end_frame(None, 1.0 / 60.0);
        }
        assert_eq!(spot.cable(), Some((a, 1.0)));
        assert_eq!(spot.knob(b), 0.0);
        for _ in 0..30 {
            spot.end_frame(None, 1.0 / 60.0);
        }
        assert_eq!(spot.cable(), None);
    }

    #[test]
    fn a_hovered_cable_lights_its_knob_and_a_knob_wins_over_a_cable() {
        let (a, b) = jacks();
        let mut spot = CableSpotlight::default();
        spot.end_frame(Some(a), 1.0);
        assert_eq!(spot.knob(a), 1.0);
        spot.hover_knob(b);
        spot.end_frame(Some(a), 1.0 / 60.0);
        assert_eq!(spot.cable(), Some((b, 1.0)));
    }
}
