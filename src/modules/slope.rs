//! Slope module: a rise and a fall at set rates, after the slope generator
//! of a Make Noise Maths.
//!
//! One circuit does four jobs. Patch a signal into **In** and Out follows it,
//! rising at the Rise rate and falling at the Fall rate: slew, glide, lag.
//! A gate into **In** makes that an attack-release envelope. A trigger runs
//! one whole rise to 1 and fall back, however short the trigger: a function
//! generator. Turn on **Cycle** and each fall starts the next rise: an LFO
//! whose two halves are set apart.
//!
//! Inside, a *charge* moves at a constant rate, one unit per Rise (or Fall)
//! time. Out is the charge bent by the Shape curve over 0 to 1, so the
//! curve never changes how long a stage takes. Beyond 0 to 1, where a pitch
//! or a bipolar signal can take it, Out is the charge itself.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    ParameterDisplay, SignalType,
};

/// The shortest and longest Rise and Fall on the knobs, in seconds.
pub const MIN_TIME: f32 = 0.001;
pub const MAX_TIME: f32 = 20.0;

/// How far CV can push a time, in seconds. Below a sample, a stage takes
/// one sample.
const CV_MIN_TIME: f32 = 0.0001;
const CV_MAX_TIME: f32 = 120.0;

/// How curved the ends of the Shape knob are: the curve is
/// (e^(k·x) − 1) / (e^k − 1), with k from −this to +this.
const SHAPE_K: f32 = 6.0;

/// How long the EOC pulse lasts, in seconds, unless the cycle is shorter.
const EOC_SECONDS: f32 = 0.001;

/// Gate threshold for Trig and Cycle.
const GATE_THRESHOLD: f32 = 0.5;

/// The Shape curve, from charge to level and back.
///
/// Over 0 to 1 it is (e^(k·x) − 1) / (e^k − 1): log-like (fast, then easing
/// in) for k below 0, a straight line at 0, exponential (slow, then
/// accelerating) above. The same curve serves both stages, so on the
/// exponential side a rise starts slowly and a fall drops fast and tails
/// off, as a struck sound decays. Outside 0 to 1 it is a straight line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    /// The bend, k. Zero is a straight line.
    k: f64,
    /// e^k − 1.
    span: f64,
}

impl Curve {
    /// The straight line.
    pub const LINEAR: Curve = Curve { k: 0.0, span: 0.0 };

    /// The curve for a Shape knob value, −1 (log) to 1 (exponential).
    pub fn from_shape(shape: f32) -> Self {
        let k = f64::from(shape.clamp(-1.0, 1.0) * SHAPE_K);
        if k.abs() < 1e-3 {
            Self::LINEAR
        } else {
            Self { k, span: k.exp_m1() }
        }
    }

    /// The level for a charge.
    #[inline]
    pub fn level(&self, charge: f64) -> f32 {
        if self.k == 0.0 || !(0.0..=1.0).contains(&charge) {
            charge as f32
        } else {
            ((self.k * charge).exp_m1() / self.span) as f32
        }
    }

    /// The charge for a level: the inverse of [`Curve::level`].
    #[inline]
    pub fn charge(&self, level: f32) -> f64 {
        let level = f64::from(level);
        if self.k == 0.0 || !(0.0..=1.0).contains(&level) {
            level
        } else {
            (level * self.span).ln_1p() / self.k
        }
    }
}

/// What the slope is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Following In.
    Follow,
    /// A run, started by a trigger or by Cycle: rising to 1.
    Rise,
    /// A run: falling back to In.
    Fall,
}

/// Which way Out was slewing at the end of the last sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    /// On its target.
    Rest,
    Up,
    Down,
}

/// A Slope module: slew, trigger envelopes and cycling, as on Maths.
///
/// # Ports
///
/// **Inputs:**
/// - **In** (Control): a signal to follow, at the Rise rate going up and the
///   Fall rate going down.
/// - **Trig** (Gate): a rising edge runs one full rise to 1 and fall back.
/// - **Rise**, **Fall** (Control): exponential CV around each knob; +1
///   halves the time.
/// - **Cycle** (Gate): high makes it cycle, in place of the toggle.
///
/// **Outputs:**
/// - **Out** (Control): the slope.
/// - **EOR** (Gate): high from the top of a rise until the fall ends.
/// - **EOC** (Gate): a short pulse each time a fall ends.
///
/// # Parameters
///
/// - **Rise**, **Fall** (1 ms – 20 s): the time to move from 0 to 1.
/// - **Cycle** (toggle): start a new rise at the end of every fall.
/// - **Shape** (−1 to 1): log, through linear, to exponential.
pub struct Slope {
    /// Where the slope is, before the Shape curve: one unit per stage time.
    /// Double precision, so ten minutes of cycling doesn't drift.
    charge: f64,
    stage: Stage,
    motion: Motion,
    /// The End of Rise gate.
    eor: bool,
    /// Samples left of the EOC pulse.
    eoc_left: u32,
    /// Trig was high on the last sample.
    trig_high: bool,
    /// The Shape curve in use, so turning the knob can keep Out where it is.
    curve: Curve,
    /// The last In value, and its charge, to save inverting the curve each
    /// sample while In holds still.
    last_in: f32,
    in_charge: f64,
    /// The stage times and Cycle on the last sample, for the display.
    rise_seconds: f32,
    fall_seconds: f32,
    cycling: bool,
    /// The last value written to Out.
    level: f32,
    sample_rate: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Slope {
    /// Creates a Slope at rest at 0.
    pub fn new() -> Self {
        Self {
            charge: 0.0,
            stage: Stage::Follow,
            motion: Motion::Rest,
            eor: false,
            eoc_left: 0,
            trig_high: false,
            curve: Curve::LINEAR,
            last_in: 0.0,
            in_charge: 0.0,
            rise_seconds: Self::DEFAULT_RISE,
            fall_seconds: Self::DEFAULT_FALL,
            cycling: false,
            level: 0.0,
            sample_rate: 44100.0,
            ports: vec![
                PortDefinition::input_with_default("in", "In", SignalType::Control, 0.0)
                    .describe("A signal to follow: Out moves toward it at the Rise rate going up and the Fall rate going down. A gate gives an attack-release envelope"),
                PortDefinition::input_with_default("trig", "Trig", SignalType::Gate, 0.0)
                    .describe("A rising edge runs one full rise to 1 and fall back, however short the trigger"),
                PortDefinition::input_with_default("rise_cv", "Rise", SignalType::Control, 0.0)
                    .describe("CV for the rise time around the knob; +1 halves it, -1 doubles it"),
                PortDefinition::input_with_default("fall_cv", "Fall", SignalType::Control, 0.0)
                    .describe("CV for the fall time around the knob; +1 halves it, -1 doubles it"),
                PortDefinition::input_with_default("cycle", "Cycle", SignalType::Gate, 0.0)
                    .describe("While high, every fall starts a new rise; patched, it takes over from the switch"),
                PortDefinition::output("out", "Out", SignalType::Control)
                    .describe("The slope: 0 to 1 from a trigger, or following In in In's own range"),
                PortDefinition::output("eor", "EOR", SignalType::Gate)
                    .describe("End of rise: high from the top of each rise until the fall ends"),
                PortDefinition::output("eoc", "EOC", SignalType::Gate)
                    .describe("End of cycle: a short pulse each time a fall ends"),
            ],
            parameters: vec![
                ParameterDefinition::new("rise", "Rise", MIN_TIME, MAX_TIME, Self::DEFAULT_RISE, ParameterDisplay::logarithmic("s"))
                    .describe("Time to rise from 0 to 1"),
                ParameterDefinition::new("fall", "Fall", MIN_TIME, MAX_TIME, Self::DEFAULT_FALL, ParameterDisplay::logarithmic("s"))
                    .describe("Time to fall from 1 to 0"),
                // Cycle before Shape: its jack must come before any knob-only row
                ParameterDefinition::toggle("cycle", "Cycle", false)
                    .describe("Start a new rise at the end of every fall, so it runs as an LFO at 1 / (Rise + Fall)"),
                ParameterDefinition::new("shape", "Shape", -1.0, 1.0, 0.0, ParameterDisplay::linear(""))
                    .describe("Curve of both stages: -1 log (fast, then easing in), 0 linear, 1 exponential (a slow rise, a fall that drops and tails off)"),
            ],
        }
    }

    const DEFAULT_RISE: f32 = 0.1;
    const DEFAULT_FALL: f32 = 0.3;

    /// Input port indices.
    const PORT_IN: usize = 0;
    const PORT_TRIG: usize = 1;
    const PORT_RISE_CV: usize = 2;
    const PORT_FALL_CV: usize = 3;
    const PORT_CYCLE: usize = 4;

    /// Output port indices.
    pub const OUT: usize = 0;
    pub const EOR: usize = 1;
    pub const EOC: usize = 2;

    /// Parameter indices.
    pub const PARAM_RISE: usize = 0;
    pub const PARAM_FALL: usize = 1;
    pub const PARAM_CYCLE: usize = 2;
    pub const PARAM_SHAPE: usize = 3;

    /// Readout value indices: see [`DspModule::readout`].
    pub const READOUT_CHARGE: usize = 0;
    /// 1 while rising, −1 while falling, 0 at rest.
    pub const READOUT_MOTION: usize = 1;
    pub const READOUT_EOR: usize = 2;
    pub const READOUT_RISE: usize = 3;
    pub const READOUT_FALL: usize = 4;
    pub const READOUT_CYCLING: usize = 5;
    pub const READOUT_LEVEL: usize = 6;

    /// A stage time after its CV: +1 halves it.
    #[inline]
    fn time(knob: f32, cv: Option<f32>) -> f32 {
        match cv {
            Some(cv) => (knob * (-cv).exp2()).clamp(CV_MIN_TIME, CV_MAX_TIME),
            None => knob,
        }
    }

    /// Moves the charge toward `target` with up to `budget` of this sample,
    /// at `up` or `down` per sample. Returns whether it arrived, and spends
    /// from the budget only the part of the sample it took, so the next
    /// stage starts where this one ended, between samples.
    #[inline]
    fn approach(&mut self, target: f64, up: f64, down: f64, budget: &mut f64) -> bool {
        let distance = target - self.charge;
        if distance == 0.0 {
            self.motion = Motion::Rest;
            return true;
        }
        let rate = if distance > 0.0 { up } else { down };
        let needed = distance.abs() / rate;
        if needed > *budget {
            self.charge += rate * *budget * distance.signum();
            *budget = 0.0;
            self.motion = if distance > 0.0 { Motion::Up } else { Motion::Down };
            false
        } else {
            self.charge = target;
            *budget -= needed;
            self.motion = Motion::Rest;
            true
        }
    }

    /// Whether the slope is on its way up (for the display, and the tests).
    fn rising(&self) -> bool {
        self.stage == Stage::Rise || (self.stage == Stage::Follow && self.motion == Motion::Up)
    }

    /// Whether the slope is on its way down.
    fn falling(&self) -> bool {
        self.motion == Motion::Down
    }
}

impl Default for Slope {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Slope {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "mod.slope",
            name: "Slope",
            category: ModuleCategory::Modulation,
            description: "Rises and falls at set rates: slew, glide, portamento, lag, AR envelope and function generator, or a cycling LFO with skew",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let input = connected_input(inputs, Self::PORT_IN);
        let trig = connected_input(inputs, Self::PORT_TRIG);
        let rise_cv = connected_input(inputs, Self::PORT_RISE_CV);
        let fall_cv = connected_input(inputs, Self::PORT_FALL_CV);
        let cycle_in = connected_input(inputs, Self::PORT_CYCLE);
        let rise_knob = params[Self::PARAM_RISE].clamp(MIN_TIME, MAX_TIME);
        let fall_knob = params[Self::PARAM_FALL].clamp(MIN_TIME, MAX_TIME);
        let cycle_switch = params[Self::PARAM_CYCLE] > 0.5;

        // A new Shape keeps Out where it is: only the road ahead bends
        let curve = Curve::from_shape(params[Self::PARAM_SHAPE]);
        if curve != self.curve {
            self.charge = curve.charge(self.curve.level(self.charge));
            self.in_charge = curve.charge(self.last_in);
            self.curve = curve;
        }

        let sample = |buffer: Option<&SignalBuffer>, i: usize| buffer.and_then(|b| b.samples.get(i).copied());
        let samples_per_second = f64::from(self.sample_rate);
        for i in 0..context.block_size {
            let target = sample(input, i).unwrap_or(0.0);
            if target != self.last_in {
                self.last_in = target;
                self.in_charge = self.curve.charge(target);
            }
            let trig_high = sample(trig, i).unwrap_or(0.0) >= GATE_THRESHOLD;
            let triggered = trig_high && !self.trig_high;
            self.trig_high = trig_high;
            let cycling = match cycle_in {
                Some(_) => sample(cycle_in, i).unwrap_or(0.0) >= GATE_THRESHOLD,
                None => cycle_switch,
            };
            let rise = Self::time(rise_knob, sample(rise_cv, i));
            let fall = Self::time(fall_knob, sample(fall_cv, i));
            self.rise_seconds = rise;
            self.fall_seconds = fall;
            self.cycling = cycling;
            // Charge per sample; a stage is never shorter than a sample
            let up = (1.0 / (f64::from(rise) * samples_per_second)).min(1.0);
            let down = (1.0 / (f64::from(fall) * samples_per_second)).min(1.0);

            if triggered {
                // From wherever it is, so a trigger mid-fall doesn't jump
                self.stage = Stage::Rise;
            }

            // Spend the sample, a stage at a time: a stage that ends
            // between samples hands what's left of the sample to the next,
            // so a cycle's period is exact and can't drift
            let mut budget = 1.0;
            let mut end_of_cycle = false;
            for _ in 0..4 {
                match self.stage {
                    Stage::Rise => {
                        if !self.approach(1.0, up, down, &mut budget) {
                            self.eor = false;
                            break;
                        }
                        self.stage = Stage::Fall;
                        self.eor = true;
                    }
                    Stage::Fall => {
                        if !self.approach(self.in_charge, up, down, &mut budget) {
                            break;
                        }
                        self.stage = Stage::Follow;
                        self.eor = false;
                        end_of_cycle = true;
                        if !cycling {
                            break;
                        }
                        self.stage = Stage::Rise;
                    }
                    Stage::Follow => {
                        let was = self.motion;
                        if !self.approach(self.in_charge, up, down, &mut budget) {
                            if self.motion == Motion::Up {
                                self.eor = false;
                            }
                            break;
                        }
                        // A rise or a fall the rate held back has caught In
                        match was {
                            Motion::Up => self.eor = true,
                            Motion::Down => {
                                self.eor = false;
                                end_of_cycle = true;
                            }
                            Motion::Rest => {}
                        }
                        if !cycling {
                            break;
                        }
                        self.stage = Stage::Rise;
                    }
                }
            }

            if end_of_cycle {
                // A millisecond, or less if the cycle is quicker, so every
                // pulse has its own edge
                let cycle = (rise + fall) * self.sample_rate;
                self.eoc_left = (EOC_SECONDS * self.sample_rate).min(cycle * 0.5).max(1.0) as u32;
            }

            // At rest on In, Out is In exactly
            self.level = if self.stage == Stage::Follow && self.motion == Motion::Rest {
                target
            } else {
                self.curve.level(self.charge)
            };
            outputs[Self::OUT].samples[i] = self.level;
            outputs[Self::EOR].samples[i] = if self.eor { 1.0 } else { 0.0 };
            outputs[Self::EOC].samples[i] = if self.eoc_left > 0 { 1.0 } else { 0.0 };
            self.eoc_left = self.eoc_left.saturating_sub(1);
        }
    }

    fn reset(&mut self) {
        self.charge = 0.0;
        self.stage = Stage::Follow;
        self.motion = Motion::Rest;
        self.eor = false;
        self.eoc_left = 0;
        self.trig_high = false;
        self.last_in = 0.0;
        self.in_charge = 0.0;
        self.level = 0.0;
    }

    /// Where the slope is, which way it's going, and the times it's running
    /// at after CV, for the display.
    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_CHARGE] = self.charge as f32;
        readout.values[Self::READOUT_MOTION] = if self.rising() {
            1.0
        } else if self.falling() {
            -1.0
        } else {
            0.0
        };
        readout.values[Self::READOUT_EOR] = if self.eor { 1.0 } else { 0.0 };
        readout.values[Self::READOUT_RISE] = self.rise_seconds;
        readout.values[Self::READOUT_FALL] = self.fall_seconds;
        readout.values[Self::READOUT_CYCLING] = if self.cycling { 1.0 } else { 0.0 };
        readout.values[Self::READOUT_LEVEL] = self.level;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Poly;

    const BLOCK: usize = 512;

    /// Knob values, in parameter order (Rise, Fall, Cycle, Shape).
    fn params(rise: f32, fall: f32, shape: f32, cycle: bool) -> [f32; 4] {
        [rise, fall, if cycle { 1.0 } else { 0.0 }, shape]
    }

    /// What the slope wrote, sample by sample.
    #[derive(Default)]
    struct Run {
        out: Vec<f32>,
        eor: Vec<f32>,
        eoc: Vec<f32>,
    }

    /// Plays `samples` of In and Trig (each `None` for unpatched) through a
    /// slope at `rate`, in blocks.
    fn run(slope: &mut Slope, rate: f32, samples: usize, params: &[f32], input: impl Fn(usize) -> Option<f32>, trig: impl Fn(usize) -> Option<f32>) -> Run {
        slope.prepare(rate, BLOCK);
        let mut result = Run::default();
        let mut start = 0;
        while start < samples {
            let len = BLOCK.min(samples - start);
            let ctx = ProcessContext::new(rate, len);
            let buffer = |f: &dyn Fn(usize) -> Option<f32>, kind| {
                if f(start).is_none() {
                    return SignalBuffer::unconnected(len, kind);
                }
                let mut b = SignalBuffer::new(len, kind);
                for i in 0..len {
                    b.samples[i] = f(start + i).unwrap_or(0.0);
                }
                b
            };
            let in_buf = buffer(&input, SignalType::Control);
            let trig_buf = buffer(&trig, SignalType::Gate);
            let none = SignalBuffer::unconnected(len, SignalType::Control);
            let mut outputs = vec![SignalBuffer::control(len), SignalBuffer::gate(len), SignalBuffer::gate(len)];
            slope.process(&[&in_buf, &trig_buf, &none, &none, &none], &mut outputs, params, &ctx);
            result.out.extend_from_slice(&outputs[0].samples[..len]);
            result.eor.extend_from_slice(&outputs[1].samples[..len]);
            result.eoc.extend_from_slice(&outputs[2].samples[..len]);
            start += len;
        }
        result
    }

    /// A one-sample trigger at sample `at`.
    fn pulse(at: usize) -> impl Fn(usize) -> Option<f32> {
        move |i| Some(if i == at { 1.0 } else { 0.0 })
    }

    fn unpatched(_: usize) -> Option<f32> {
        None
    }

    /// The fractional sample where `signal` first crosses `level` at or
    /// after `from`, going the way it's going there.
    fn crossing(signal: &[f32], level: f32, from: usize) -> f32 {
        let rising = signal[from + 1] > signal[from];
        for i in from.max(1)..signal.len() {
            let (a, b) = (signal[i - 1], signal[i]);
            let crossed = if rising { a < level && b >= level } else { a > level && b <= level };
            if crossed {
                return (i - 1) as f32 + (level - a) / (b - a);
            }
        }
        panic!("never crossed {level}");
    }

    /// Where a signal peaks: the top of a rise, which may end between
    /// samples, so its sample can already be on the way down.
    fn peak_at(signal: &[f32]) -> usize {
        (0..signal.len()).fold(0, |best, i| if signal[i] > signal[best] { i } else { best })
    }

    /// Indices where a gate goes high.
    fn edges(gate: &[f32]) -> Vec<usize> {
        (0..gate.len()).filter(|&i| gate[i] > 0.5 && (i == 0 || gate[i - 1] <= 0.5)).collect()
    }

    #[test]
    fn test_info_ports_and_parameters() {
        let slope = Slope::new();
        assert_eq!(slope.info().id, "mod.slope");
        assert_eq!(slope.info().category, ModuleCategory::Modulation);
        let names: Vec<&str> = slope.ports().iter().map(|p| p.name).collect();
        assert_eq!(names, ["In", "Trig", "Rise", "Fall", "Cycle", "Out", "EOR", "EOC"]);
        let params: Vec<&str> = slope.parameters().iter().map(|p| p.name).collect();
        assert_eq!(params, ["Rise", "Fall", "Cycle", "Shape"]);
        for word in ["slew", "glide", "portamento", "lag", "AR envelope", "function generator"] {
            assert!(slope.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn test_curve_round_trips_and_bends_the_right_way() {
        for shape in [-1.0, -0.4, 0.0, 0.3, 1.0] {
            let curve = Curve::from_shape(shape);
            assert_eq!(curve.level(0.0), 0.0);
            assert!((curve.level(1.0) - 1.0).abs() < 1e-6);
            for x in [0.0, 0.1, 0.5, 0.9, 1.0] {
                assert!((curve.charge(curve.level(x)) - x).abs() < 1e-5, "shape {shape} at {x}");
            }
            assert_eq!(curve.charge(0.0), 0.0);
            // Straight beyond 0 to 1
            assert_eq!(curve.level(-0.5), -0.5);
            assert_eq!(curve.level(2.0), 2.0);
        }
        assert!(Curve::from_shape(1.0).level(0.5) < 0.1, "exponential sags");
        assert!(Curve::from_shape(-1.0).level(0.5) > 0.9, "log bows");
        assert_eq!(Curve::from_shape(0.0).level(0.37), 0.37);
    }

    #[test]
    fn test_rise_and_fall_times_match_the_knobs() {
        // Measured on the linear shape between 10% and 90%, to the
        // fraction of a sample
        for rate in [44100.0, 48000.0] {
            for time in [0.001, 0.0137, 0.1, 1.0, 20.0] {
                for (rise, fall) in [(time, 0.01), (0.01, time)] {
                    let length = ((rise + fall) * rate) as usize + 1000;
                    let r = run(&mut Slope::new(), rate, length, &params(rise, fall, 0.0, false), unpatched, pulse(10));
                    let up = (crossing(&r.out, 0.9, 10) - crossing(&r.out, 0.1, 10)) / 0.8 / rate;
                    let top = peak_at(&r.out);
                    let down = (crossing(&r.out, 0.1, top) - crossing(&r.out, 0.9, top)) / 0.8 / rate;
                    assert!((up / rise - 1.0).abs() < 0.01, "rise {rise} at {rate}: measured {up}");
                    assert!((down / fall - 1.0).abs() < 0.01, "fall {fall} at {rate}: measured {down}");
                }
            }
        }
    }

    #[test]
    fn test_shape_leaves_the_times_alone() {
        // EOR goes high at the top, and EOC marks the end of the fall
        let rate = 48000.0;
        for shape in [-1.0, -0.5, 0.5, 1.0] {
            let r = run(&mut Slope::new(), rate, 48000, &params(0.1, 0.25, shape, false), unpatched, pulse(0));
            let top = edges(&r.eor)[0] as f32 / rate;
            let end = edges(&r.eoc)[0] as f32 / rate;
            assert!((top - 0.1).abs() < 0.001 * 0.1 + 1.0 / rate, "shape {shape}: top at {top}");
            assert!((end - 0.35).abs() < 0.001 * 0.35 + 1.0 / rate, "shape {shape}: end at {end}");
        }
    }

    #[test]
    fn test_a_one_sample_trigger_runs_the_whole_shape() {
        let r = run(&mut Slope::new(), 48000.0, 48000, &params(0.05, 0.2, 0.5, false), unpatched, pulse(100));
        let peak = r.out.iter().copied().fold(0.0, f32::max);
        assert!(peak > 0.999, "rises all the way: {peak}");
        assert_eq!(*r.out.last().unwrap(), 0.0, "and falls all the way back");
        assert_eq!(edges(&r.eor).len(), 1);
        assert_eq!(edges(&r.eoc).len(), 1);
        // EOR covers the fall: from the top until the fall ends
        let top = edges(&r.eor)[0];
        let end = edges(&r.eoc)[0];
        assert!((top as f32 - (100.0 + 0.05 * 48000.0)).abs() <= 1.0, "top at {top}");
        assert!(r.out[top - 1..=top].contains(&peak));
        assert!(r.eor[top..end].iter().all(|&g| g == 1.0) && r.eor[end] == 0.0);
    }

    #[test]
    fn test_a_trigger_mid_fall_rises_from_where_it_is() {
        let rate = 48000.0;
        let (rise, fall) = (0.01, 0.1);
        // The second trigger lands halfway down
        let second = 100 + (0.01 * rate) as usize + (0.05 * rate) as usize;
        let r = run(&mut Slope::new(), rate, 24000, &params(rise, fall, 0.0, false), unpatched, move |i| {
            Some(if i == 100 || i == second { 1.0 } else { 0.0 })
        });
        let before = r.out[second - 1];
        assert!((before - 0.5).abs() < 0.01, "halfway down: {before}");
        let largest_step = r.out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(largest_step <= 1.0 / (rise * rate) + 1e-5, "no jump: largest step {largest_step}");
        assert!(r.out[second] > before, "turns round on the trigger");
        assert_eq!(edges(&r.eoc).len(), 1, "the interrupted fall never ended");
        assert_eq!(edges(&r.eor).len(), 2);
    }

    #[test]
    fn test_slews_a_step_at_a_constant_rate_and_holds_exactly() {
        let rate = 44100.0;
        let rise = 0.2;
        let held = 0.8125;
        let r = run(&mut Slope::new(), rate, 22050, &params(rise, 0.5, 0.0, false), |i| Some(if i < 100 { 0.0 } else { held }), unpatched);
        // A step of 0.8125 takes 0.8125 of the Rise time
        let arrive = r.out.iter().position(|&v| v == held).unwrap();
        let expected = 100.0 + held * rise * rate;
        assert!((arrive as f32 - expected).abs() <= 1.0, "arrived at {arrive}, expected {expected}");
        // Linear: every step the same
        let rate_per_sample = 1.0 / (rise * rate);
        for w in r.out[100..arrive - 1].windows(2) {
            assert!(((w[1] - w[0]) - rate_per_sample).abs() < 1e-5);
        }
        // Holding: Out is In, to the bit
        assert!(r.out[arrive..].iter().all(|&v| v == held));
        assert_eq!(edges(&r.eor), [arrive], "EOR goes high when the rise ends");

        // A held odd value through a curved shape holds exactly too
        let r = run(&mut Slope::new(), rate, 22050, &params(0.01, 0.01, 0.7, false), |_| Some(0.3333), unpatched);
        assert!(r.out[2000..].iter().all(|&v| v == 0.3333));
    }

    #[test]
    fn test_follows_beyond_zero_to_one() {
        // A pitch two octaves up, then one down: one unit per Rise or Fall
        let rate = 48000.0;
        let r = run(&mut Slope::new(), rate, 48000, &params(0.1, 0.05, 0.0, false), |i| Some(if i < 24000 { 2.0 } else { -1.0 }), unpatched);
        let up = r.out.iter().position(|&v| v == 2.0).unwrap();
        assert!((up as f32 - 0.2 * rate).abs() <= 1.0);
        let down = r.out.iter().position(|&v| v == -1.0).unwrap();
        assert!(((down - 24000) as f32 - 0.15 * rate).abs() <= 1.0);
        assert_eq!(edges(&r.eoc), [down], "the fall ended");
    }

    #[test]
    fn test_a_gate_on_in_is_an_attack_release_envelope() {
        let rate = 48000.0;
        let r = run(&mut Slope::new(), rate, 48000, &params(0.01, 0.1, 0.0, false), |i| Some(if (100..20000).contains(&i) { 1.0 } else { 0.0 }), unpatched);
        assert!(r.out[1000..20000].iter().all(|&v| v == 1.0), "holds while the gate is high");
        let end = edges(&r.eoc)[0];
        assert!(((end - 20000) as f32 - 0.1 * rate).abs() <= 1.0);
        assert!(r.eor[1000..end].iter().all(|&g| g == 1.0));
        assert_eq!(r.eor[end], 0.0);
    }

    #[test]
    fn test_cycle_period_is_rise_plus_fall_without_drift() {
        // Ten minutes, with EOC on the sample each fall ends in
        let rate = 48000.0;
        let (rise, fall) = (0.0217, 0.0519);
        let total = 600 * rate as usize;
        let mut slope = Slope::new();
        slope.prepare(rate, BLOCK);
        let ctx = ProcessContext::new(rate, BLOCK);
        let none = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::control(BLOCK), SignalBuffer::gate(BLOCK), SignalBuffer::gate(BLOCK)];
        let mut eoc = Vec::new();
        let mut was_high = false;
        for block in 0..total / BLOCK {
            slope.process(&[&none, &none, &none, &none, &none], &mut outputs, &params(rise, fall, 1.0, true), &ctx);
            for (i, &g) in outputs[2].samples.iter().enumerate() {
                if g > 0.5 && !was_high {
                    eoc.push(block * BLOCK + i);
                }
                was_high = g > 0.5;
            }
        }
        // Rise and fall as the knobs hold them, in f32
        let period = (f64::from(rise) + f64::from(fall)) * f64::from(rate);
        let expected = ((total / BLOCK * BLOCK) as f64 / period) as usize;
        assert!(eoc.len().abs_diff(expected) <= 1, "{} cycles, expected {expected}", eoc.len());
        for (n, &at) in eoc.iter().enumerate() {
            // The fall ends between samples, (n + 1) periods from the start
            // of sample 0: within sample `at`
            let ends = (n + 1) as f64 * period;
            assert!(ends > at as f64 - 1e-3 && ends <= (at + 1) as f64 + 1e-3, "cycle {n} ends at {ends}, EOC on {at}");
        }
    }

    #[test]
    fn test_cycle_starts_from_rest_and_stops_after_the_fall() {
        let rate = 48000.0;
        let mut slope = Slope::new();
        // A 30 ms cycle, switched off partway up its fourth rise
        let on = run(&mut slope, rate, 4700, &params(0.01, 0.02, 0.0, true), unpatched, unpatched);
        assert!(on.out[100] > 0.0, "starts at once");
        // Switched off mid-rise, it finishes the run and rests
        let off = run(&mut slope, rate, 4800, &params(0.01, 0.02, 0.0, false), unpatched, unpatched);
        assert_eq!(edges(&off.eoc).len(), 1);
        assert_eq!(*off.out.last().unwrap(), 0.0);
    }

    #[test]
    fn test_cycle_input_takes_over_from_the_switch() {
        let rate = 48000.0;
        let mut slope = Slope::new();
        slope.prepare(rate, BLOCK);
        let ctx = ProcessContext::new(rate, BLOCK);
        let none = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let low = SignalBuffer::gate(BLOCK);
        let mut outputs = vec![SignalBuffer::control(BLOCK), SignalBuffer::gate(BLOCK), SignalBuffer::gate(BLOCK)];
        // The switch is on, but a low cable holds it still
        slope.process(&[&none, &none, &none, &none, &low], &mut outputs, &params(0.001, 0.001, 0.0, true), &ctx);
        assert!(outputs[0].samples.iter().all(|&v| v == 0.0));
        let mut high = SignalBuffer::gate(BLOCK);
        high.fill(1.0);
        slope.process(&[&none, &none, &none, &none, &high], &mut outputs, &params(0.001, 0.001, 0.0, false), &ctx);
        assert!(outputs[0].samples.iter().any(|&v| v > 0.5), "and a high one cycles it");
    }

    #[test]
    fn test_rise_cv_halves_the_time() {
        let rate = 48000.0;
        let mut slope = Slope::new();
        slope.prepare(rate, BLOCK);
        let ctx = ProcessContext::new(rate, BLOCK);
        let none = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut input = SignalBuffer::control(BLOCK);
        input.fill(1.0);
        let mut cv = SignalBuffer::control(BLOCK);
        cv.fill(1.0);
        let mut outputs = vec![SignalBuffer::control(BLOCK), SignalBuffer::gate(BLOCK), SignalBuffer::gate(BLOCK)];
        // A 20 ms rise at +1 takes 10 ms: 480 samples
        slope.process(&[&input, &none, &cv, &none, &none], &mut outputs, &params(0.02, 0.1, 0.0, false), &ctx);
        assert!((outputs[0].samples[239] - 0.5).abs() < 0.003);
        assert_eq!(outputs[0].samples[480], 1.0);
        assert!((slope.readout(&[]).unwrap().values[Slope::READOUT_RISE] - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_turning_shape_never_jumps() {
        let rate = 48000.0;
        let mut slope = Slope::new();
        let first = run(&mut slope, rate, 2400, &params(0.1, 0.1, -1.0, false), unpatched, pulse(0));
        let second = run(&mut slope, rate, 2400, &params(0.1, 0.1, 1.0, false), unpatched, unpatched);
        let before = *first.out.last().unwrap();
        assert!((second.out[0] - before).abs() < 0.01, "{before} then {}", second.out[0]);
        assert!(second.out[0] > before, "and still rising");
    }

    #[test]
    fn test_poly_voices_slew_on_their_own() {
        let rate = 48000.0;
        let mut poly = Poly::<Slope>::default();
        poly.prepare(rate, BLOCK);
        let ctx = ProcessContext::new(rate, BLOCK);
        let mut input = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        input.set_channels(3);
        let chord = [0.25, 0.5833, -0.5];
        for (voice, &pitch) in chord.iter().enumerate() {
            input.channel_mut(voice).fill(pitch);
        }
        let none = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
            SignalBuffer::polyphonic(BLOCK, SignalType::Gate),
            SignalBuffer::polyphonic(BLOCK, SignalType::Gate),
        ];
        // 10 ms per unit: the farthest voice arrives in 6 ms
        poly.process(&[&input, &none, &none, &none, &none], &mut outputs, &params(0.01, 0.01, 0.0, false), &ctx);
        assert_eq!(outputs[0].channels(), 3);
        for (voice, &pitch) in chord.iter().enumerate() {
            let out = &outputs[0].voice(voice).samples;
            let arrive = out.iter().position(|&v| v == pitch).unwrap();
            assert!((arrive as f32 - pitch.abs() * 480.0).abs() <= 1.0, "voice {voice} arrived at {arrive}");
        }
    }

    #[test]
    fn test_registered_as_polyphonic() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create("mod.slope").expect("Slope is registered");
        assert!(module.polyphonic());
    }
}
