//! Logic module.
//!
//! Combines two gates (AND, OR, XOR, NOT A) and turns a control voltage into
//! a gate where it crosses a threshold. B is normalled to that comparison,
//! so with nothing in B, A AND Above lets a clock through only while a CV
//! is high. Maybe lets a gate at A through only sometimes, by Chance.

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    gate::{Edge, Transition},
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

use super::noise::Pcg32;

/// The module's ID, as patches name it.
pub const ID: &str = "util.logic";

/// Combines gates, and compares a CV with a threshold.
///
/// # Ports
///
/// **Inputs:**
/// - **A** (Gate): The first gate.
/// - **B** (Gate): The second gate. With nothing patched, B is Above.
/// - **CV** (Control): The voltage Above compares with Threshold.
/// - **Chance** (Control): Added to the Chance knob.
///
/// **Outputs:**
/// - **AND**, **OR**, **XOR**, **NOT A** (Gate): A and B combined.
/// - **Above** (Gate): High while CV is above Threshold.
/// - **Maybe** (Gate): A, but each gate passes only by Chance: the dice
///   are thrown as it rises, and a gate let through is let through whole.
///
/// # Parameters
///
/// - **Threshold** (-1 to 1): Where Above switches, with a little
///   hysteresis so a noisy CV doesn't chatter.
/// - **Chance** (0-1): How likely each gate at A is to pass to Maybe.
pub struct Logic {
    /// Whether Above is high.
    above: bool,
    /// The A and B inputs, heard as gates.
    a: Edge,
    b: Edge,
    /// The last CV sample, for the display.
    last_cv: f32,
    /// The last sample's A and B, for the display.
    last_a: bool,
    last_b: bool,
    /// Whether the gate now high at A won its throw, so Maybe passes it.
    passing: bool,
    /// The dice Maybe throws.
    rng: Pcg32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Logic {
    /// Creates a Logic module.
    pub fn new() -> Self {
        Self {
            above: false,
            a: Edge::LOW,
            b: Edge::LOW,
            last_cv: 0.0,
            last_a: false,
            last_b: false,
            passing: false,
            rng: Pcg32::next_stream(),
            ports: vec![
                PortDefinition::input_with_default("a", "A", SignalType::Gate, 0.0).describe("First gate for AND, OR, XOR and NOT A"),
                PortDefinition::input_with_default("b", "B", SignalType::Gate, 0.0)
                    .describe("Second gate for AND, OR and XOR. With nothing patched it's Above, so AND passes A only while CV is high"),
                PortDefinition::input_with_default("cv", "CV", SignalType::Control, 0.0).describe("A voltage to compare with Threshold: Above is high while it's over"),
                PortDefinition::input_with_default("chance_cv", "Chance", SignalType::Control, 0.0).describe("CV added to the Chance knob"),
                PortDefinition::output("and", "AND", SignalType::Gate).describe("High while A and B are both high"),
                PortDefinition::output("or", "OR", SignalType::Gate).describe("High while A or B is high"),
                PortDefinition::output("xor", "XOR", SignalType::Gate).describe("High while exactly one of A and B is high"),
                PortDefinition::output("not_a", "NOT A", SignalType::Gate).describe("A turned upside down: high while A is low"),
                PortDefinition::output("above", "Above", SignalType::Gate).describe("High while CV is above Threshold: any control voltage made a gate"),
                PortDefinition::output("maybe", "Maybe", SignalType::Gate)
                    .describe("A, sometimes: each gate at A passes whole, or not at all, by Chance"),
            ],
            // Chance first: its jack has to come before the knob-only Threshold
            parameters: vec![
                ParameterDefinition::new("chance", "Chance", 0.0, 1.0, 0.5, ParameterDisplay::linear(""))
                    .describe("How likely each gate at A is to pass to Maybe: at 1 every one, at 0 none"),
                ParameterDefinition::new("threshold", "Threshold", -1.0, 1.0, 0.5, ParameterDisplay::linear(""))
                    .describe("Where Above switches on and off"),
            ],
        }
    }

    const PORT_A: usize = 0;
    const PORT_B: usize = 1;
    const PORT_CV: usize = 2;
    const PORT_CHANCE_CV: usize = 3;

    const OUT_AND: usize = 0;
    const OUT_OR: usize = 1;
    const OUT_XOR: usize = 2;
    const OUT_NOT_A: usize = 3;
    const OUT_ABOVE: usize = 4;
    const OUT_MAYBE: usize = 5;

    const PARAM_CHANCE: usize = 0;
    const PARAM_THRESHOLD: usize = 1;

    /// Readout slots: the CV, Above, and A and B as the logic heard them.
    pub const READOUT_CV: usize = 0;
    pub const READOUT_ABOVE: usize = 1;
    pub const READOUT_A: usize = 2;
    pub const READOUT_B: usize = 3;

    /// How far past Threshold CV must go to switch Above, either way.
    pub const HYSTERESIS: f32 = 0.01;
}

impl Default for Logic {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Logic {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Logic",
            category: ModuleCategory::Utility,
            description: "Combine gates with AND, OR, XOR and NOT, and turn a CV into a gate at a threshold",
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

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let threshold = params[Self::PARAM_THRESHOLD];
        let at = |buffer: Option<&SignalBuffer>, i: usize| buffer.map_or(0.0, |buf| buf.at(i));
        let a_in = inputs.get(Self::PORT_A).copied();
        let b_in = connected_input(inputs, Self::PORT_B);
        let cv_in = inputs.get(Self::PORT_CV).copied();
        let chance_knob = params[Self::PARAM_CHANCE];
        let chance_in = connected_input(inputs, Self::PORT_CHANCE_CV);
        let level = |high: bool| if high { 1.0 } else { 0.0 };

        for i in 0..context.block_size {
            let cv = at(cv_in, i);
            if self.above {
                self.above = cv >= threshold - Self::HYSTERESIS;
            } else {
                self.above = cv > threshold + Self::HYSTERESIS;
            }
            self.last_cv = cv;

            match self.a.tick(at(a_in, i)) {
                // Thrown once as the gate rises, so it passes whole or not at all
                Some(Transition::Rising) => {
                    let chance = (chance_knob + at(chance_in, i)).clamp(0.0, 1.0);
                    self.passing = self.rng.chance(f64::from(chance));
                }
                Some(Transition::Falling) => self.passing = false,
                None => {}
            }
            let a = self.a.is_high();
            // Normalled: an empty B jack reads Above
            let b = match b_in {
                Some(buf) => {
                    self.b.tick(buf.at(i));
                    self.b.is_high()
                }
                None => {
                    self.b.reset();
                    self.above
                }
            };
            (self.last_a, self.last_b) = (a, b);

            outputs[Self::OUT_AND].samples[i] = level(a && b);
            outputs[Self::OUT_OR].samples[i] = level(a || b);
            outputs[Self::OUT_XOR].samples[i] = level(a != b);
            outputs[Self::OUT_NOT_A].samples[i] = level(!a);
            outputs[Self::OUT_ABOVE].samples[i] = level(self.above);
            if let Some(maybe) = outputs.get_mut(Self::OUT_MAYBE) {
                maybe.samples[i] = level(self.passing);
            }
        }
    }

    fn reset(&mut self) {
        self.above = false;
        self.a.reset();
        self.b.reset();
        self.last_cv = 0.0;
        self.last_a = false;
        self.last_b = false;
        self.passing = false;
    }

    fn seed(&mut self, seed: u64) {
        self.rng = Pcg32::new(seed);
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_CV] = self.last_cv;
        readout.values[Self::READOUT_ABOVE] = if self.above { 1.0 } else { 0.0 };
        readout.values[Self::READOUT_A] = if self.last_a { 1.0 } else { 0.0 };
        readout.values[Self::READOUT_B] = if self.last_b { 1.0 } else { 0.0 };
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs A, B and CV (one value per sample; `None` leaves the jack
    /// empty) through a fresh Logic, returning its five outputs.
    fn run(a: &[f32], b: Option<&[f32]>, cv: &[f32], threshold: f32) -> Vec<Vec<f32>> {
        let len = a.len().max(cv.len()).max(b.map_or(0, |b| b.len()));
        let buffer = |values: &[f32]| {
            let mut buf = SignalBuffer::control(len);
            buf.samples[..values.len()].copy_from_slice(values);
            buf
        };
        let a = buffer(a);
        let b = b.map_or(SignalBuffer::unconnected(len, SignalType::Gate), buffer);
        let cv = buffer(cv);
        let mut outs: Vec<SignalBuffer> = (0..5).map(|_| SignalBuffer::control(len)).collect();
        let mut logic = Logic::new();
        logic.process(&[&a, &b, &cv], &mut outs, &[1.0, threshold], &ProcessContext::new(48000.0, len));
        outs.into_iter().map(|buf| buf.samples).collect()
    }

    #[test]
    fn test_logic_info() {
        let logic = Logic::new();
        assert_eq!(logic.info().id, "util.logic");
        assert_eq!(logic.info().category, ModuleCategory::Utility);
        assert_eq!(logic.ports().iter().filter(|p| p.is_input()).count(), 4);
        assert_eq!(logic.ports().iter().filter(|p| p.is_output()).count(), 6);
        assert_eq!(logic.parameters().len(), 2);
    }

    #[test]
    fn test_truth_table() {
        let out = run(&[0.0, 1.0, 0.0, 1.0], Some(&[0.0, 0.0, 1.0, 1.0]), &[], 0.5);
        assert_eq!(out[Logic::OUT_AND], [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(out[Logic::OUT_OR], [0.0, 1.0, 1.0, 1.0]);
        assert_eq!(out[Logic::OUT_XOR], [0.0, 1.0, 1.0, 0.0]);
        assert_eq!(out[Logic::OUT_NOT_A], [1.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn test_above_switches_with_hysteresis() {
        // Wobbling right at the threshold holds; crossing it clearly switches
        let cv = [0.0, 0.505, 0.52, 0.495, 0.505, 0.48, 0.495, 0.51, 0.9];
        let above = &run(&[], Some(&[]), &cv, 0.5)[Logic::OUT_ABOVE];
        assert_eq!(*above, [0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_empty_b_reads_above() {
        // A clock through AND passes only while CV is over the threshold
        let a = [1.0, 0.0, 1.0, 0.0, 1.0, 0.0];
        let cv = [-0.5, -0.5, 0.5, 0.5, -0.5, -0.5];
        let out = run(&a, None, &cv, 0.0);
        assert_eq!(out[Logic::OUT_AND], [0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(out[Logic::OUT_OR], [1.0, 0.0, 1.0, 1.0, 1.0, 0.0]);
        // Patched, B is B, whatever CV does
        let patched = run(&a, Some(&[0.0; 6]), &cv, 0.0);
        assert_eq!(patched[Logic::OUT_AND], [0.0; 6]);
    }

    #[test]
    fn test_empty_b_with_nothing_in_cv_reads_low() {
        // CV unpatched is 0, under the default threshold: B is low, as an
        // empty jack would be
        let out = run(&[1.0, 0.0], None, &[], 0.5);
        assert_eq!(out[Logic::OUT_AND], [0.0, 0.0]);
        assert_eq!(out[Logic::OUT_XOR], [1.0, 0.0]);
    }

    /// Plays `gates` gates, each 4 samples high and 4 low, into A of a
    /// seeded Logic at `chance` (plus `cv` in its Chance jack), returning A
    /// and Maybe.
    fn maybe(gates: usize, chance: f32, cv: Option<f32>, seed: u64) -> (Vec<f32>, Vec<f32>) {
        let len = gates * 8;
        let mut a = SignalBuffer::control(len);
        for (i, s) in a.samples.iter_mut().enumerate() {
            *s = if i % 8 < 4 { 1.0 } else { 0.0 };
        }
        let empty = SignalBuffer::unconnected(len, SignalType::Gate);
        let cv_in = SignalBuffer::unconnected(len, SignalType::Control);
        let chance_in = cv.map_or(SignalBuffer::unconnected(len, SignalType::Control), |cv| {
            let mut buf = SignalBuffer::control(len);
            buf.fill(cv);
            buf
        });
        let mut outs: Vec<SignalBuffer> = (0..6).map(|_| SignalBuffer::control(len)).collect();
        let mut logic = Logic::new();
        logic.seed(seed);
        logic.process(&[&a, &empty, &cv_in, &chance_in], &mut outs, &[chance, 0.5], &ProcessContext::new(48000.0, len));
        (a.samples, outs.swap_remove(Logic::OUT_MAYBE).samples)
    }

    #[test]
    fn test_maybe_passes_gates_whole_by_chance() {
        let (a, maybe) = maybe(4000, 0.25, None, 1);
        let passed = (0..4000).filter(|g| maybe[g * 8] == 1.0).count();
        assert!((900..1100).contains(&passed), "{passed} of 4000 at 25%");
        // A passed gate is all of A's gate, and Maybe is never high without A
        for gate in a.chunks(8).zip(maybe.chunks(8)) {
            let (a, maybe) = gate;
            assert!(maybe == [0.0; 8] || maybe == a, "{maybe:?}");
        }
    }

    #[test]
    fn test_chance_at_its_ends() {
        let (a, all) = maybe(200, 1.0, None, 2);
        assert_eq!(all, a);
        let (_, none) = maybe(200, 0.0, None, 2);
        assert!(none.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_chance_cv_adds_to_the_knob() {
        let (a, all) = maybe(200, 0.25, Some(0.75), 3);
        assert_eq!(all, a);
        let (_, none) = maybe(200, 0.75, Some(-1.0), 3);
        assert!(none.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_seeded_dice_repeat() {
        assert_eq!(maybe(500, 0.5, None, 7).1, maybe(500, 0.5, None, 7).1);
        assert_ne!(maybe(500, 0.5, None, 7).1, maybe(500, 0.5, None, 8).1);
    }
}
