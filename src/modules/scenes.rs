//! Scenes: the knobs of the patch, or of the group it sits in, kept as
//! scenes A to H, and a Morph knob to glide from one scene to another.
//!
//! The editor keeps the scenes (see `graph/scenes.rs` and
//! `app/synth_app/scenes.rs`) and hands the engine a table of them: each
//! knob a scene holds, where each scene has it, and how it turns. The
//! module follows Morph, From and To (and the CV patched into Morph), and
//! as they move the plan turns the knobs (see
//! [`GraphPlan`](crate::engine::GraphPlan)), so the render tool, the plugin
//! and the browser hear a morph with no editor drawing it. The editor turns
//! the same knobs on screen by the same [`blend`].

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    ParameterDisplay, SignalType,
};
use crate::engine::NodeId;
use crate::widgets::knob::KnobConfig;

/// The module's ID, as patches name it.
pub const ID: &str = "util.scenes";

/// The scenes' names, in slot order.
pub const SLOT_LABELS: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];

/// How far Morph has to move before the knobs follow.
pub const MORPH_EPSILON: f32 = 1e-4;

/// How a knob gets from one scene to another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glide {
    /// Round its travel, as a hand would turn it: a frequency by octaves.
    Turn { min: f32, max: f32, logarithmic: bool },
    /// Holds until halfway, then flips: whole numbers, switches, dropdowns.
    Flip,
}

/// The value `morph` of the way from `from` to `to`, gliding as `glide`.
pub fn blend(glide: Glide, from: f32, to: f32, morph: f32) -> f32 {
    let morph = morph.clamp(0.0, 1.0);
    match glide {
        Glide::Turn { min, max, logarithmic } => {
            let config = KnobConfig { range: min..=max, logarithmic, ..Default::default() };
            let (a, b) = (config.travel(from.clamp(min, max)), config.travel(to.clamp(min, max)));
            config.value_at_travel(a + (b - a) * morph).clamp(min, max)
        }
        Glide::Flip => {
            if morph < 0.5 { from } else { to }
        }
    }
}

/// A knob a Scenes module's scenes hold, as the engine turns it.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneKnob {
    /// Its module.
    pub node: NodeId,
    /// Its parameter's index.
    pub param: usize,
    pub glide: Glide,
    /// Where each scene has it, by slot, or NaN where a scene doesn't hold it.
    pub values: [f32; SLOT_LABELS.len()],
}

impl SceneKnob {
    /// Where it stands `morph` of the way from scene `from` to scene `to`,
    /// or `None` unless both scenes hold it.
    pub fn at(&self, morph: f32, from: usize, to: usize) -> Option<f32> {
        let (a, b) = (*self.values.get(from)?, *self.values.get(to)?);
        (!a.is_nan() && !b.is_nan()).then(|| blend(self.glide, a, b, morph))
    }
}

/// Keeps scenes of knobs and glides between two of them.
pub struct Scenes {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    /// Morph, From and To when the knobs last followed them, or `None`
    /// before the first block: a patch just opened or started keeps its
    /// knobs where they are.
    followed: Option<(f32, usize, usize)>,
    /// Where they moved to, until the plan has turned the knobs.
    moved: Option<(f32, usize, usize)>,
}

impl Scenes {
    pub fn new() -> Self {
        Self {
            ports: vec![PortDefinition::input("morph", "Morph", SignalType::Control)
                .describe("0 to 1 glides from the From scene to the To scene: an LFO, an envelope or an Arranger lane")],
            parameters: vec![
                ParameterDefinition::new("morph", "Morph", 0.0, 1.0, 0.0, ParameterDisplay::linear(""))
                    .describe("How far from the From scene to the To scene the knobs sit. Switches flip halfway"),
                ParameterDefinition::choice("from", "From", &SLOT_LABELS, 0)
                    .describe("The scene Morph starts from, at 0"),
                ParameterDefinition::choice("to", "To", &SLOT_LABELS, 1)
                    .describe("The scene Morph arrives at, at 1"),
            ],
            followed: None,
            moved: None,
        }
    }

    pub const PARAM_MORPH: usize = 0;
    pub const PARAM_FROM: usize = 1;
    pub const PARAM_TO: usize = 2;
}

impl Default for Scenes {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Scenes {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Scenes",
            category: ModuleCategory::Utility,
            description: "Keep the knobs as scenes, and glide between two",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn process(&mut self, inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], params: &[f32], _context: &ProcessContext) {
        // The CV where the block ends, or the knob
        let morph = connected_input(inputs, 0)
            .and_then(|cv| cv.samples.last().copied())
            .unwrap_or(params[Self::PARAM_MORPH])
            .clamp(0.0, 1.0);
        let slot = |index: usize| (params[index].round().max(0.0) as usize).min(SLOT_LABELS.len() - 1);
        let at = (morph, slot(Self::PARAM_FROM), slot(Self::PARAM_TO));
        let Some((was, was_from, was_to)) = self.followed else {
            self.followed = Some(at);
            return;
        };
        if (morph - was).abs() >= MORPH_EPSILON || at.1 != was_from || at.2 != was_to {
            self.followed = Some(at);
            self.moved = Some(at);
        }
    }

    fn reset(&mut self) {
        self.followed = None;
        self.moved = None;
    }

    fn take_morph(&mut self) -> Option<(f32, usize, usize)> {
        self.moved.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morph_pairs_with_its_jack() {
        let scenes = Scenes::new();
        assert_eq!(scenes.info().id, ID);
        assert_eq!(scenes.ports()[0].name, scenes.parameters()[Scenes::PARAM_MORPH].name);
        assert_eq!(scenes.parameters()[Scenes::PARAM_TO].default, 1.0);
    }

    #[test]
    fn the_knobs_follow_morph_only_once_it_moves() {
        let mut scenes = Scenes::new();
        let context = ProcessContext::default();
        let mut params = [0.3, 0.0, 1.0];
        let unpatched = SignalBuffer::unconnected(4, SignalType::Control);
        let run = |scenes: &mut Scenes, params: &[f32]| {
            scenes.process(&[&unpatched], &mut [], params, &context);
            scenes.take_morph()
        };
        // The first block only notes where Morph stands
        assert_eq!(run(&mut scenes, &params), None);
        assert_eq!(run(&mut scenes, &params), None);
        params[0] = 0.6;
        assert_eq!(run(&mut scenes, &params), Some((0.6, 0, 1)));
        assert_eq!(run(&mut scenes, &params), None);
        params[2] = 3.0;
        assert_eq!(run(&mut scenes, &params), Some((0.6, 0, 3)));
        scenes.reset();
        params[0] = 0.9;
        assert_eq!(run(&mut scenes, &params), None);
    }

    #[test]
    fn a_knob_only_one_scene_holds_stays_put() {
        let mut values = [f32::NAN; SLOT_LABELS.len()];
        values[0] = 200.0;
        values[1] = 3200.0;
        let knob = SceneKnob { node: 1, param: 0, glide: Glide::Turn { min: 20.0, max: 20000.0, logarithmic: true }, values };
        // Halfway between 200 Hz and 3200 Hz, four octaves apart, is two up
        assert!((knob.at(0.5, 0, 1).unwrap() - 800.0).abs() < 1.0);
        assert_eq!(knob.at(0.5, 0, 2), None);
        assert_eq!(blend(Glide::Flip, -2.0, 2.0, 0.49), -2.0);
        assert_eq!(blend(Glide::Flip, -2.0, 2.0, 0.5), 2.0);
    }
}
