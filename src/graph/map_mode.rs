//! Map Mode (#426): the canvas as a map of everything MIDI can turn.
//!
//! While it's on, every control MIDI Learn can take (knobs, toggles,
//! dropdowns, a Looper's footswitches, a Sampler's Rec and the pads of a
//! Scenes module) wears a soft purple outline, and a click on one starts
//! MIDI Learn on it instead of turning, flipping or pressing it. A catch
//! laid over the control takes the click: egui hands the pointer to the
//! topmost widget, and the catch is added after the control. The control's
//! right-click menu hangs on the catch meanwhile, so it still opens.
//!
//! The Controller Map lights a mapping's control while its row is hovered,
//! with Map Mode on or off.

use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, StrokeKind};

use super::{SynthGraphState, SynthResponse};
use crate::app::theme;
use crate::engine::NodeId as EngineNodeId;

/// A control MIDI can be mapped to: a module's parameter, or one of a
/// Scenes module's pads (whose `param_index` is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MapKey {
    pub node: EngineNodeId,
    pub param_index: usize,
    pub scene: Option<u8>,
}

impl MapKey {
    /// The control a MIDI Learn response starts on.
    pub fn of(learn: &SynthResponse) -> Option<Self> {
        match *learn {
            SynthResponse::MidiLearnStart { engine_node_id, param_index, scene, .. } => {
                Some(Self { node: engine_node_id, param_index: if scene.is_some() { 0 } else { param_index }, scene })
            }
            _ => None,
        }
    }

    /// Is MIDI Learn waiting for this control?
    fn learning(self, user_state: &SynthGraphState) -> bool {
        match self.scene {
            Some(slot) => user_state.midi_learn_scene == Some((self.node, slot as usize)),
            None => user_state.midi_learn_target == Some((self.node, self.param_index)),
        }
    }
}

/// Map Mode as the canvas draws it. The app turns it on and off.
#[derive(Clone, Debug, Default)]
pub struct MapMode {
    /// A click on a control maps it.
    pub on: bool,
    /// The control whose row the pointer is over in the Controller Map.
    pub highlight: Option<MapKey>,
    /// How many controls have been mapped since it came on.
    pub mapped: usize,
}

/// How a control's outline goes round it.
#[derive(Clone, Copy, Debug)]
pub enum Outline {
    /// A circle round a knob's dial or a footswitch.
    Ring { center: Pos2, radius: f32 },
    /// A rounded box round a toggle, a dropdown or a pad.
    Box { rect: Rect, corner: f32 },
}

impl Outline {
    /// A box a little outside `rect`, at `zoom`.
    pub fn around(rect: Rect, zoom: f32) -> Self {
        Self::Box { rect: rect.expand(2.0 * zoom), corner: 4.0 * zoom }
    }
}

/// A click Map Mode caught: the catch, which carries the control's
/// right-click menu while Map Mode is on, and MIDI Learn on the control
/// if it was clicked.
pub struct Caught {
    pub response: Response,
    pub learn: Option<SynthResponse>,
}

/// Outlines a learnable control, in Map Mode or when the Controller Map
/// lights it, and in Map Mode lays a catch over `hit` that takes its
/// clicks. Call it after drawing the control. `learn` is the response its
/// right-click menu's Learn sends. Returns the catch in Map Mode.
pub fn catch(ui: &mut egui::Ui, hit: Rect, outline: Outline, learn: &SynthResponse, user_state: &SynthGraphState) -> Option<Caught> {
    let key = MapKey::of(learn)?;
    let mode = &user_state.map_mode;
    let lit = mode.highlight == Some(key);
    if !mode.on && !lit {
        return None;
    }
    let caught = mode.on.then(|| ui.interact(hit, ui.scope_id().with(("map_mode_catch", key)), Sense::click_and_drag()));
    let hovered = caught.as_ref().is_some_and(|response| response.hovered() || response.is_pointer_button_down_on());
    let learning = key.learning(user_state);
    let look = if lit {
        Look::Lit
    } else if learning {
        crate::app::pace::animate(ui.ctx());
        Look::Learning((ui.input(|i| i.time) * 3.0).sin().abs() as f32)
    } else if hovered {
        Look::Hovered
    } else {
        Look::Quiet
    };
    paint(ui.painter(), outline, look, user_state.zoom);
    caught.map(|response| {
        let learn = response.clicked().then(|| learn.clone());
        Caught { response, learn }
    })
}

/// How strongly an outline shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Look {
    /// Learnable, and nothing more.
    Quiet,
    /// Under the pointer: a click maps it.
    Hovered,
    /// MIDI Learn waits for it, breathing between its ends (0-1).
    Learning(f32),
    /// Its row is hovered in the Controller Map.
    Lit,
}

impl Look {
    /// The line's strength, width (at zoom 1) and the fill under it.
    fn ink(self) -> (f32, f32, f32) {
        match self {
            Look::Quiet => (0.34, 1.0, 0.0),
            Look::Hovered => (0.85, 1.25, 0.07),
            Look::Learning(breath) => (0.55 + 0.45 * breath, 1.5, 0.05 + 0.06 * breath),
            Look::Lit => (1.0, 1.5, 0.10),
        }
    }
}

fn paint(painter: &egui::Painter, outline: Outline, look: Look, zoom: f32) {
    let purple: Color32 = theme::signal::MIDI;
    let (strength, width, fill) = look.ink();
    let stroke = Stroke::new(width * zoom, purple.gamma_multiply(strength));
    // A lit control glows a little beyond its line, so it can be found from afar
    let glow = (look == Look::Lit).then(|| Stroke::new(5.0 * zoom, purple.gamma_multiply(0.16)));
    match outline {
        Outline::Ring { center, radius } => {
            if fill > 0.0 {
                painter.circle_filled(center, radius, purple.gamma_multiply(fill));
            }
            if let Some(glow) = glow {
                painter.circle_stroke(center, radius + 2.0 * zoom, glow);
            }
            painter.circle_stroke(center, radius, stroke);
        }
        Outline::Box { rect, corner } => {
            if fill > 0.0 {
                painter.rect_filled(rect, corner, purple.gamma_multiply(fill));
            }
            if let Some(glow) = glow {
                painter.rect_stroke(rect.expand(2.0 * zoom), corner + 2.0 * zoom, glow, StrokeKind::Middle);
            }
            painter.rect_stroke(rect, corner, stroke, StrokeKind::Middle);
        }
    }
}

/// A thin purple line round the canvas while Map Mode is on, so the mode
/// can't be mistaken for the canvas at rest.
pub fn paint_edge(painter: &egui::Painter, canvas: Rect, user_state: &SynthGraphState) {
    if user_state.map_mode.on {
        let purple: Color32 = theme::signal::MIDI;
        painter.rect_stroke(canvas.shrink(1.0), 0.0, Stroke::new(2.0, purple.gamma_multiply(0.45)), StrokeKind::Inside);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::MidiControl;

    #[test]
    fn a_learn_names_its_control_and_a_pad_its_slot() {
        let knob = SynthResponse::midi_learn_start(7, 3, "Cutoff", 20.0, 20000.0, MidiControl::Knob);
        assert_eq!(MapKey::of(&knob), Some(MapKey { node: 7, param_index: 3, scene: None }));
        let pad = SynthResponse::MidiLearnStart {
            engine_node_id: 9,
            param_index: 5,
            param_name: "Scene B".into(),
            min_value: 0.0,
            max_value: 1.0,
            control: MidiControl::Button,
            scene: Some(1),
            into: None,
        };
        assert_eq!(MapKey::of(&pad), Some(MapKey { node: 9, param_index: 0, scene: Some(1) }));
        assert_eq!(MapKey::of(&SynthResponse::MidiLearnCancel), None);
    }

    #[test]
    fn the_outlines_grow_stronger_from_quiet_to_lit() {
        let strength = |look: Look| look.ink().0;
        assert!(strength(Look::Quiet) < strength(Look::Hovered));
        assert!(strength(Look::Learning(0.0)) < strength(Look::Learning(1.0)));
        assert_eq!(strength(Look::Lit), 1.0);
    }
}
