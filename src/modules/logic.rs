//! Logic module.
//!
//! Combines two gates (AND, OR, XOR, NOT A) and turns a control voltage into
//! a gate where it crosses a threshold. B is normalled to that comparison,
//! so with nothing in B, A AND Above lets a clock through only while a CV
//! is high.

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

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
///
/// **Outputs:**
/// - **AND**, **OR**, **XOR**, **NOT A** (Gate): A and B combined.
/// - **Above** (Gate): High while CV is above Threshold.
///
/// # Parameters
///
/// - **Threshold** (-1 to 1): Where Above switches, with a little
///   hysteresis so a noisy CV doesn't chatter.
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
            ports: vec![
                PortDefinition::input_with_default("a", "A", SignalType::Gate, 0.0).describe("First gate for AND, OR, XOR and NOT A"),
                PortDefinition::input_with_default("b", "B", SignalType::Gate, 0.0)
                    .describe("Second gate for AND, OR and XOR. With nothing patched it's Above, so AND passes A only while CV is high"),
                PortDefinition::input_with_default("cv", "CV", SignalType::Control, 0.0).describe("A voltage to compare with Threshold: Above is high while it's over"),
                PortDefinition::output("and", "AND", SignalType::Gate).describe("High while A and B are both high"),
                PortDefinition::output("or", "OR", SignalType::Gate).describe("High while A or B is high"),
                PortDefinition::output("xor", "XOR", SignalType::Gate).describe("High while exactly one of A and B is high"),
                PortDefinition::output("not_a", "NOT A", SignalType::Gate).describe("A turned upside down: high while A is low"),
                PortDefinition::output("above", "Above", SignalType::Gate).describe("High while CV is above Threshold: any control voltage made a gate"),
            ],
            parameters: vec![ParameterDefinition::new("threshold", "Threshold", -1.0, 1.0, 0.5, ParameterDisplay::linear(""))
                .describe("Where Above switches on and off")],
        }
    }

    const PORT_A: usize = 0;
    const PORT_B: usize = 1;
    const PORT_CV: usize = 2;

    const OUT_AND: usize = 0;
    const OUT_OR: usize = 1;
    const OUT_XOR: usize = 2;
    const OUT_NOT_A: usize = 3;
    const OUT_ABOVE: usize = 4;

    const PARAM_THRESHOLD: usize = 0;

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
        let level = |high: bool| if high { 1.0 } else { 0.0 };

        for i in 0..context.block_size {
            let cv = at(cv_in, i);
            if self.above {
                self.above = cv >= threshold - Self::HYSTERESIS;
            } else {
                self.above = cv > threshold + Self::HYSTERESIS;
            }
            self.last_cv = cv;

            self.a.tick(at(a_in, i));
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
        }
    }

    fn reset(&mut self) {
        self.above = false;
        self.a.reset();
        self.b.reset();
        self.last_cv = 0.0;
        self.last_a = false;
        self.last_b = false;
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
        logic.process(&[&a, &b, &cv], &mut outs, &[threshold], &ProcessContext::new(48000.0, len));
        outs.into_iter().map(|buf| buf.samples).collect()
    }

    #[test]
    fn test_logic_info() {
        let logic = Logic::new();
        assert_eq!(logic.info().id, "util.logic");
        assert_eq!(logic.info().category, ModuleCategory::Utility);
        assert_eq!(logic.ports().iter().filter(|p| p.is_input()).count(), 3);
        assert_eq!(logic.ports().iter().filter(|p| p.is_output()).count(), 5);
        assert_eq!(logic.parameters().len(), 1);
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
}
