//! ADSR Envelope module.
//!
//! Generates an Attack-Decay-Sustain-Release envelope in response to gate signals.
//! Fundamental for shaping sound amplitude and filter cutoff over time.
//!
//! Every stage takes exactly the time on its knob. A stage is a one-pole
//! (RC) curve aimed past its target, so it arrives on time instead of creeping
//! up on it forever. How far past is the stage's Curve: aimed far past, the
//! visible stretch is nearly a straight line; aimed just past, it is a deep
//! analog curve. [`stage_shape`] is that curve in closed form, and the ADSR
//! display draws with it, so the picture is the sound.
//!
//! Attack, Decay and Release CV stretch their stages by octaves of time: +1
//! doubles the knob's time, -1 halves it. Sustain CV adds to its knob's
//! level. CV is read every sample, so a cue that lands while a release is
//! already falling stretches the rest of it.

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, Readout, SignalType,
};

/// The shortest and longest stage times, the knobs' range, which CV stays
/// within too.
pub const MIN_TIME: f32 = 0.001;
pub const MAX_TIME: f32 = 10.0;

/// Steepness at Curve = 100%. The stage aims 1/(e^10 − 1) ≈ 0.005% of its
/// span past its target: a deep curve that is still on time.
const MAX_STEEPNESS: f64 = 10.0;

/// Steepness for a Curve knob position (0.0–1.0).
///
/// 0 is a straight line, 1 a deep RC curve. Never quite 0, so the
/// aim-past-the-target arithmetic stays finite.
#[inline]
pub fn curve_steepness(curve: f32) -> f64 {
    (curve.clamp(0.0, 1.0) as f64 * MAX_STEEPNESS).max(1e-3)
}

/// How far a stage has gone from its start level to its target, at
/// normalized time `u` (0 at the start of the stage, 1 when it lands).
///
/// Fast start, slow finish, and exactly 1 at `u = 1`. Higher `steepness`
/// is more curved; near 0 it is a straight line.
#[inline]
pub fn stage_shape(steepness: f64, u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    (-steepness * u).exp_m1() / (-steepness).exp_m1()
}

/// How far through a stage, as normalized time, the envelope is when it
/// has gone `progress` of the way to its target: the inverse of
/// [`stage_shape`].
#[inline]
pub fn stage_time(steepness: f64, progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    (-(progress * (-steepness).exp_m1()).ln_1p() / steepness).clamp(0.0, 1.0)
}

/// Envelope stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvelopeStage {
    /// Envelope is idle (output = 0).
    Idle,
    /// Attack phase: rising from the current level to the peak.
    Attack,
    /// Decay phase: falling from the peak to the sustain level.
    Decay,
    /// Sustain phase: holding at sustain level while gate is high.
    Sustain,
    /// Release phase: falling from current level to 0.
    Release,
}

/// Per-sample recursion for one stage: progress `x` (0 → 1) steps toward
/// `aim` (past 1) by `x = aim + (x - aim) * coeff`, reaching 1 after exactly
/// the stage time. This traces [`stage_shape`] one sample at a time.
#[derive(Clone, Copy, Debug)]
struct StageRate {
    coeff: f64,
    aim: f64,
}

impl StageRate {
    fn new(time_seconds: f32, curve: f32, sample_rate: f32) -> Self {
        let steepness = curve_steepness(curve);
        let samples = (time_seconds as f64 * sample_rate as f64).max(1.0);
        Self {
            coeff: (-steepness / samples).exp(),
            // 1 + 1/(e^k − 1): from here, the curve crosses 1 at u = 1
            aim: 1.0 + 1.0 / steepness.exp_m1(),
        }
    }
}

/// One stage's time as CV stretches it, and the rate that runs it.
#[derive(Clone, Copy, Debug)]
struct StageTime {
    knob: f32,
    curve: f32,
    /// The CV the rate was last worked out for: a held CV costs nothing.
    octaves: f32,
    seconds: f32,
    rate: StageRate,
}

impl StageTime {
    fn new(knob: f32, curve: f32, sample_rate: f32) -> Self {
        let knob = knob.clamp(MIN_TIME, MAX_TIME);
        Self { knob, curve, octaves: 0.0, seconds: knob, rate: StageRate::new(knob, curve, sample_rate) }
    }

    /// Follows CV of `octaves`, if patched.
    #[inline]
    fn follow(&mut self, octaves: Option<f32>, sample_rate: f32) {
        if let Some(octaves) = octaves.filter(|&octaves| octaves != self.octaves) {
            self.octaves = octaves;
            self.seconds = AdsrEnvelope::stretched(self.knob, octaves);
            self.rate = StageRate::new(self.seconds, self.curve, sample_rate);
        }
    }
}

/// ADSR Envelope generator.
///
/// Generates a control signal that follows the classic ADSR envelope shape:
/// - **Attack**: Time to rise from the current level to the peak
/// - **Decay**: Time to fall from the peak to the sustain level
/// - **Sustain**: Level to hold while gate is high, as a fraction of the peak
/// - **Release**: Time to fall from the current level to 0 after gate goes low
///
/// # Ports
///
/// - **Gate** (Gate, Input): Triggers the envelope (high = note on, low = note off).
/// - **Retrigger** (Gate, Input): Restarts attack from current level when high.
/// - **Velocity** (Control, Input): Scales the peak, read at each note on.
/// - **Attack**, **Decay**, **Release** (Control, Input): CV around each
///   knob in octaves of time; +1 doubles it, -1 halves it.
/// - **Sustain** (Control, Input): CV added to the Sustain knob.
/// - **Out** (Control, Output): The envelope output (0.0 to 1.0).
///
/// # Parameters
///
/// - **Attack**, **Decay**, **Release** (0.001-10.0s): Stage times, logarithmic scaling.
/// - **Sustain** (0.0-1.0): Sustain level, linear scaling. Glides rather than steps.
/// - **Attack/Decay/Release Curve** (0-100%): Straight line to deep RC curve.
/// - **Velocity Amount** (0-100%): How much velocity scales the peak.
pub struct AdsrEnvelope {
    /// Current envelope stage.
    stage: EnvelopeStage,
    /// Current envelope level (0.0 to 1.0).
    level: f64,
    /// Level the current stage started from.
    stage_start: f64,
    /// Progress through the current stage (0 at its start, 1 when it lands).
    progress: f64,
    /// Peak level for this note, set from velocity at note on.
    peak: f64,
    /// Sustain parameter after smoothing, so knob moves glide.
    sustain: f64,
    /// Whether `sustain` has caught a parameter value yet.
    sustain_primed: bool,
    /// One-pole coefficient for sustain smoothing.
    sustain_coeff: f64,
    /// The gate input, for its edges and its level.
    gate: Edge,
    /// The retrigger input, for its rising edges.
    retrigger: Edge,
    /// Stage times and sustain after CV, as of the last sample, for the display.
    attack_seconds: f32,
    decay_seconds: f32,
    sustain_target: f32,
    release_seconds: f32,
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl AdsrEnvelope {
    /// Creates a new ADSR envelope.
    pub fn new() -> Self {
        let mut env = Self {
            stage: EnvelopeStage::Idle,
            level: 0.0,
            stage_start: 0.0,
            progress: 0.0,
            peak: 1.0,
            sustain: 0.0,
            sustain_primed: false,
            sustain_coeff: 0.0,
            gate: Edge::LOW,
            retrigger: Edge::LOW,
            attack_seconds: 0.01,
            decay_seconds: 0.1,
            sustain_target: 0.7,
            release_seconds: 0.3,
            sample_rate: 44100.0,
            ports: vec![
                // Input ports
                PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0).describe("Gate that starts the envelope when high and releases it when low"),
                PortDefinition::input_with_default("retrigger", "Retrig", SignalType::Gate, 0.0).describe("A rising edge restarts the attack from the current level"),
                PortDefinition::input_with_default("velocity", "Velocity", SignalType::Control, 1.0).describe("Scales the peak at each note on; unpatched means full peak"),
                // A jack for every stage: the editor lays jacks before
                // knob-only parameters, so Decay's and Release's need
                // Attack's and Sustain's beside them
                PortDefinition::input_with_default("attack_cv", "Attack", SignalType::Control, 0.0)
                    .describe("CV that stretches the attack around its knob: +1 doubles it, -1 halves it"),
                PortDefinition::input_with_default("decay_cv", "Decay", SignalType::Control, 0.0)
                    .describe("CV that stretches the decay around its knob: +1 doubles it, -1 halves it. Velocity here rings harder notes longer"),
                PortDefinition::input_with_default("sustain_cv", "Sustain", SignalType::Control, 0.0)
                    .describe("CV added to the Sustain knob's level"),
                PortDefinition::input_with_default("release_cv", "Release", SignalType::Control, 0.0)
                    .describe("CV that stretches the release around its knob: +1 doubles it, -1 halves it, even mid-release. An Arranger lane here lets a last chord ring"),
                // Output port
                PortDefinition::output("out", "Out", SignalType::Control).describe("Envelope level, 0 to 1"),
            ],
            // New parameters go at the end: v1/v2 patches name their
            // positional values against this list.
            parameters: vec![
                // Attack time (1ms to 10s, logarithmic)
                ParameterDefinition::new(
                    "attack",
                    "Attack",
                    MIN_TIME,
                    MAX_TIME,
                    0.01, // 10ms default
                    ParameterDisplay::logarithmic("s"),
                ).describe("Time to rise to the peak after the gate goes high"),
                // Decay time (1ms to 10s, logarithmic)
                ParameterDefinition::new(
                    "decay",
                    "Decay",
                    MIN_TIME,
                    MAX_TIME,
                    0.1, // 100ms default
                    ParameterDisplay::logarithmic("s"),
                ).describe("Time to fall from the peak to the sustain level"),
                // Sustain level (0 to 1, linear)
                ParameterDefinition::new(
                    "sustain",
                    "Sustain",
                    0.0,
                    1.0,
                    0.7, // 70% default
                    ParameterDisplay::linear(""),
                ).describe("Level held while the gate stays high, as a fraction of the peak"),
                // Release time (1ms to 10s, logarithmic)
                ParameterDefinition::new(
                    "release",
                    "Release",
                    MIN_TIME,
                    MAX_TIME,
                    0.3, // 300ms default
                    ParameterDisplay::logarithmic("s"),
                ).describe("Time to fade to silence after the gate goes low"),
                // Attack curve: nearly straight by default, so attacks are punchy
                ParameterDefinition::new(
                    "attack_curve",
                    "Attack Curve",
                    0.0,
                    1.0,
                    0.2,
                    ParameterDisplay::linear("%"),
                ).describe("Attack shape; 0 is a straight line, 100% a deep curve"),
                // Decay and release curves: analog RC by default
                ParameterDefinition::new(
                    "decay_curve",
                    "Decay Curve",
                    0.0,
                    1.0,
                    0.5,
                    ParameterDisplay::linear("%"),
                ).describe("Decay shape; 0 is a straight line, 100% a deep curve"),
                ParameterDefinition::new(
                    "release_curve",
                    "Release Curve",
                    0.0,
                    1.0,
                    0.5,
                    ParameterDisplay::linear("%"),
                ).describe("Release shape; 0 is a straight line, 100% a deep curve"),
                // How much the Velocity input scales the peak
                ParameterDefinition::new(
                    "velocity_amount",
                    "Velocity Amount",
                    0.0,
                    1.0,
                    0.5,
                    ParameterDisplay::linear("%"),
                ).describe("How much velocity scales the peak; 0 ignores velocity"),
            ],
        };
        env.prepare(44100.0, 0);
        env
    }

    /// Port index constants.
    const PORT_GATE: usize = 0;
    const PORT_RETRIGGER: usize = 1;
    const PORT_VELOCITY: usize = 2;
    const PORT_ATTACK_CV: usize = 3;
    const PORT_DECAY_CV: usize = 4;
    const PORT_SUSTAIN_CV: usize = 5;
    const PORT_RELEASE_CV: usize = 6;
    const PORT_OUT: usize = 0;

    /// Parameter index constants.
    const PARAM_ATTACK: usize = 0;
    const PARAM_DECAY: usize = 1;
    const PARAM_SUSTAIN: usize = 2;
    const PARAM_RELEASE: usize = 3;
    const PARAM_ATTACK_CURVE: usize = 4;
    const PARAM_DECAY_CURVE: usize = 5;
    const PARAM_RELEASE_CURVE: usize = 6;
    const PARAM_VELOCITY_AMOUNT: usize = 7;

    /// Readout indices: the stage times after CV, in seconds, and the
    /// sustain level after CV.
    pub const READOUT_ATTACK: usize = 0;
    pub const READOUT_DECAY: usize = 1;
    pub const READOUT_SUSTAIN: usize = 2;
    pub const READOUT_RELEASE: usize = 3;
    /// Readout indices: the stage it's in (0 idle, 1 attack, 2 decay,
    /// 3 sustain, 4 release), how far it has gone to the stage's target
    /// (0-1), its level, and the peak of the note it's playing.
    pub const READOUT_STAGE: usize = 4;
    pub const READOUT_PROGRESS: usize = 5;
    pub const READOUT_LEVEL: usize = 6;
    pub const READOUT_PEAK: usize = 7;
    /// Readout indices: the level the stage began at and the level it's
    /// heading for. A note let go mid-decay releases from above the sustain,
    /// and one struck mid-release attacks from above zero.
    pub const READOUT_STAGE_START: usize = 8;
    pub const READOUT_STAGE_TARGET: usize = 9;

    /// Time constant for sustain knob moves.
    const SUSTAIN_SMOOTHING_SECONDS: f32 = 0.005;

    /// A stage time: the knob's `seconds`, stretched by `octaves` of CV.
    #[inline]
    fn stretched(seconds: f32, octaves: f32) -> f32 {
        (seconds * octaves.exp2()).clamp(MIN_TIME, MAX_TIME)
    }

    /// Begins a stage from the current level.
    fn enter(&mut self, stage: EnvelopeStage) {
        self.stage = stage;
        self.stage_start = self.level;
        self.progress = 0.0;
    }

    /// Note on: sets the peak from velocity and starts the attack from
    /// wherever the envelope is now.
    fn trigger(&mut self, velocity: Option<f32>, velocity_amount: f32) {
        self.peak = match velocity {
            Some(v) => {
                let amount = velocity_amount.clamp(0.0, 1.0) as f64;
                1.0 - amount + amount * v.clamp(0.0, 1.0) as f64
            }
            None => 1.0,
        };
        self.enter(EnvelopeStage::Attack);
    }

    /// Advances the current stage's progress one sample. Returns true
    /// when the stage lands on its target.
    #[inline]
    fn advance(&mut self, rate: StageRate) -> bool {
        self.progress = rate.aim + (self.progress - rate.aim) * rate.coeff;
        if self.progress >= 1.0 {
            self.progress = 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for AdsrEnvelope {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for AdsrEnvelope {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "mod.adsr",
            name: "ADSR Envelope",
            category: ModuleCategory::Modulation,
            description: "Attack-Decay-Sustain-Release envelope with exact stage times, curve shaping, velocity and CV on every stage",
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
        self.sustain_coeff =
            (-1.0 / (Self::SUSTAIN_SMOOTHING_SECONDS as f64 * sample_rate as f64)).exp();
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let sustain_knob = params[Self::PARAM_SUSTAIN];
        let velocity_amount = params[Self::PARAM_VELOCITY_AMOUNT];

        // Rates are worked out per block, so turning a knob mid-stage
        // reshapes the rest of the stage rather than waiting for the next note
        let sr = self.sample_rate;
        let mut attack = StageTime::new(params[Self::PARAM_ATTACK], params[Self::PARAM_ATTACK_CURVE], sr);
        let mut decay = StageTime::new(params[Self::PARAM_DECAY], params[Self::PARAM_DECAY_CURVE], sr);
        let mut release = StageTime::new(params[Self::PARAM_RELEASE], params[Self::PARAM_RELEASE_CURVE], sr);
        let mut sustain_target = sustain_knob.clamp(0.0, 1.0);

        if !self.sustain_primed {
            self.sustain = sustain_target as f64;
            self.sustain_primed = true;
        }

        // Get input buffers
        let gate_in = inputs.get(Self::PORT_GATE);
        let retrigger_in = inputs.get(Self::PORT_RETRIGGER);
        let velocity_in = connected_input(inputs, Self::PORT_VELOCITY);
        let attack_cv = connected_input(inputs, Self::PORT_ATTACK_CV);
        let decay_cv = connected_input(inputs, Self::PORT_DECAY_CV);
        let sustain_cv = connected_input(inputs, Self::PORT_SUSTAIN_CV);
        let release_cv = connected_input(inputs, Self::PORT_RELEASE_CV);
        let cv_at = |cv: Option<&SignalBuffer>, i: usize| cv.and_then(|buf| buf.samples.get(i).copied());

        // Get output buffer
        let output = &mut outputs[Self::PORT_OUT];

        for i in 0..context.block_size {
            let gate_rising = self.gate.rising(gate_in.map_or(0.0, |buf| buf.at(i)));
            let gate_high = self.gate.is_high();
            let retrigger_rising = self.retrigger.rising(retrigger_in.map_or(0.0, |buf| buf.at(i)));
            let velocity = velocity_in.and_then(|buf| buf.samples.get(i).copied());

            // CV stretches the stage times and moves the sustain, sample by sample
            attack.follow(cv_at(attack_cv, i), sr);
            decay.follow(cv_at(decay_cv, i), sr);
            release.follow(cv_at(release_cv, i), sr);
            if let Some(cv) = cv_at(sustain_cv, i) {
                sustain_target = (sustain_knob + cv).clamp(0.0, 1.0);
            }

            // Stage changes from the gates
            match self.stage {
                EnvelopeStage::Idle | EnvelopeStage::Release => {
                    if gate_rising || (retrigger_rising && gate_high) {
                        self.trigger(velocity, velocity_amount);
                    }
                }
                EnvelopeStage::Attack => {
                    if !gate_high {
                        self.enter(EnvelopeStage::Release);
                    }
                    // A retrigger mid-attack changes nothing: already rising
                }
                EnvelopeStage::Decay | EnvelopeStage::Sustain => {
                    if !gate_high {
                        self.enter(EnvelopeStage::Release);
                    } else if retrigger_rising {
                        self.trigger(velocity, velocity_amount);
                    }
                }
            }

            // Sustain glides toward its knob
            let sustain_param = sustain_target as f64;
            self.sustain = sustain_param + (self.sustain - sustain_param) * self.sustain_coeff;
            let sustain_level = self.sustain * self.peak;

            // Output, then move one sample on: a stage that starts at the
            // gate edge lands exactly its time later
            output.samples[i] = self.level as f32;

            match self.stage {
                EnvelopeStage::Idle => {}
                EnvelopeStage::Attack => {
                    let landed = self.advance(attack.rate);
                    self.level = self.stage_start + (self.peak - self.stage_start) * self.progress;
                    if landed {
                        self.enter(EnvelopeStage::Decay);
                    }
                }
                EnvelopeStage::Decay => {
                    // Aimed at the smoothed sustain, so a knob move mid-decay bends the curve
                    let landed = self.advance(decay.rate);
                    self.level = self.stage_start + (sustain_level - self.stage_start) * self.progress;
                    if landed {
                        self.enter(EnvelopeStage::Sustain);
                    }
                }
                EnvelopeStage::Sustain => {
                    self.level = sustain_level;
                }
                EnvelopeStage::Release => {
                    let landed = self.advance(release.rate);
                    self.level = self.stage_start * (1.0 - self.progress);
                    if landed {
                        self.level = 0.0;
                        self.stage = EnvelopeStage::Idle;
                    }
                }
            }
        }

        self.attack_seconds = attack.seconds;
        self.decay_seconds = decay.seconds;
        self.sustain_target = sustain_target;
        self.release_seconds = release.seconds;
    }

    fn reset(&mut self) {
        self.stage = EnvelopeStage::Idle;
        self.level = 0.0;
        self.stage_start = 0.0;
        self.progress = 0.0;
        self.peak = 1.0;
        self.sustain_primed = false;
        self.gate.reset();
        self.retrigger.reset();
    }

    /// The stage times and sustain CV has moved them to, and where the
    /// envelope is, for the display.
    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_ATTACK] = self.attack_seconds;
        readout.values[Self::READOUT_DECAY] = self.decay_seconds;
        readout.values[Self::READOUT_SUSTAIN] = self.sustain_target;
        readout.values[Self::READOUT_RELEASE] = self.release_seconds;
        readout.values[Self::READOUT_STAGE] = match self.stage {
            EnvelopeStage::Idle => 0.0,
            EnvelopeStage::Attack => 1.0,
            EnvelopeStage::Decay => 2.0,
            EnvelopeStage::Sustain => 3.0,
            EnvelopeStage::Release => 4.0,
        };
        readout.values[Self::READOUT_PROGRESS] = self.progress as f32;
        readout.values[Self::READOUT_LEVEL] = self.level as f32;
        readout.values[Self::READOUT_PEAK] = self.peak as f32;
        readout.values[Self::READOUT_STAGE_START] = self.stage_start as f32;
        readout.values[Self::READOUT_STAGE_TARGET] = match self.stage {
            EnvelopeStage::Attack => self.peak,
            EnvelopeStage::Decay | EnvelopeStage::Sustain => self.sustain * self.peak,
            EnvelopeStage::Idle | EnvelopeStage::Release => 0.0,
        } as f32;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    /// Full parameter list with the given A/D/S/R and default curves and velocity amount.
    fn params(attack: f32, decay: f32, sustain: f32, release: f32) -> [f32; 8] {
        [attack, decay, sustain, release, 0.2, 0.5, 0.5, 0.5]
    }

    /// Runs the envelope over a gate that is high for `gate_samples` of `total` samples.
    fn render(params: &[f32], gate_samples: usize, total: usize) -> Vec<f32> {
        let mut env = AdsrEnvelope::new();
        env.prepare(SR, total);
        let mut gate = SignalBuffer::control(total);
        gate.samples[..gate_samples].fill(1.0);
        let mut outputs = vec![SignalBuffer::control(total)];
        env.process(&[&gate], &mut outputs, params, &ProcessContext::new(SR, total));
        outputs.remove(0).samples
    }

    /// First index at or after `from` where the output reaches `level`
    /// (from below when rising, from above when falling).
    fn first_reaching(out: &[f32], from: usize, level: f32, rising: bool) -> usize {
        out[from..]
            .iter()
            .position(|&s| if rising { s >= level } else { s <= level })
            .map(|i| i + from)
            .expect("level never reached")
    }

    fn assert_within_5_percent(measured_samples: usize, knob_seconds: f32, what: &str) {
        let measured = measured_samples as f32 / SR;
        let error = (measured - knob_seconds).abs() / knob_seconds;
        assert!(
            error <= 0.05,
            "{what}: knob {knob_seconds}s, measured {measured}s ({:.1}% off)",
            error * 100.0
        );
    }

    #[test]
    fn test_adsr_info() {
        let env = AdsrEnvelope::new();
        assert_eq!(env.info().id, "mod.adsr");
        assert_eq!(env.info().name, "ADSR Envelope");
        assert_eq!(env.info().category, ModuleCategory::Modulation);
    }

    #[test]
    fn test_adsr_ports() {
        let env = AdsrEnvelope::new();
        let ports = env.ports();

        assert_eq!(ports.len(), 8);

        // Gate input
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "gate");
        assert_eq!(ports[0].signal_type, SignalType::Gate);

        // Retrigger input
        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "retrigger");
        assert_eq!(ports[1].signal_type, SignalType::Gate);

        // Velocity input
        assert!(ports[2].is_input());
        assert_eq!(ports[2].id, "velocity");
        assert_eq!(ports[2].signal_type, SignalType::Control);

        // CV for each stage, named after its knob so each shares one input
        // with its knob, in the knobs' order
        let cv: Vec<_> = ports[3..7].iter().map(|p| (p.id, p.name)).collect();
        assert_eq!(
            cv,
            [("attack_cv", "Attack"), ("decay_cv", "Decay"), ("sustain_cv", "Sustain"), ("release_cv", "Release")]
        );
        assert!(ports[3..7].iter().all(|p| p.is_input() && p.signal_type == SignalType::Control));

        // Output
        assert!(ports[7].is_output());
        assert_eq!(ports[7].id, "out");
        assert_eq!(ports[7].signal_type, SignalType::Control);
    }

    #[test]
    fn test_adsr_parameters() {
        let env = AdsrEnvelope::new();
        let params = env.parameters();

        // The first four keep their positions for v1/v2 patches
        let ids: Vec<_> = params.iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            ["attack", "decay", "sustain", "release", "attack_curve", "decay_curve", "release_curve", "velocity_amount"]
        );

        assert_eq!(params[0].min, 0.001);
        assert_eq!(params[0].max, 10.0);
        assert!((params[0].default - 0.01).abs() < f32::EPSILON);
        assert!((params[1].default - 0.1).abs() < f32::EPSILON);
        assert_eq!(params[2].min, 0.0);
        assert_eq!(params[2].max, 1.0);
        assert!((params[2].default - 0.7).abs() < f32::EPSILON);
        assert!((params[3].default - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_adsr_idle_output_zero() {
        let mut env = AdsrEnvelope::new();
        env.prepare(44100.0, 256);

        // No gate input, should output zeros
        let mut outputs = vec![SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);

        env.process(&[], &mut outputs, &params(0.01, 0.1, 0.7, 0.3), &ctx);

        // Output should be all zeros
        assert!(outputs[0].samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_stage_shape_ends() {
        for k in [1e-3, 0.5, 2.0, 5.0, 10.0] {
            assert!(stage_shape(k, 0.0).abs() < 1e-12);
            assert!((stage_shape(k, 1.0) - 1.0).abs() < 1e-12);
        }
        // Near zero steepness is a straight line
        assert!((stage_shape(curve_steepness(0.0), 0.25) - 0.25).abs() < 1e-3);
        // Steeper is further along early
        assert!(stage_shape(5.0, 0.2) > stage_shape(2.0, 0.2));
    }

    #[test]
    fn test_stage_times_match_knobs() {
        // Attack, decay and release land within 5% of their knobs, at
        // short, middling and long settings and every curve
        for &time in &[0.005, 0.1, 1.0] {
            for &curve in &[0.0, 0.2, 0.5, 1.0] {
                let p = [time, time, 0.5, time, curve, curve, curve, 0.5];
                let hold = ((time * 2.5) * SR) as usize;
                let total = hold + ((time * 1.5) * SR) as usize;
                let out = render(&p, hold, total);

                let peak = first_reaching(&out, 0, 1.0, true);
                assert_within_5_percent(peak, time, &format!("attack (curve {curve})"));

                let sustained = first_reaching(&out, peak, 0.5, false);
                assert_within_5_percent(sustained - peak, time, &format!("decay (curve {curve})"));

                let silent = first_reaching(&out, hold, 0.0, false);
                assert_within_5_percent(silent - hold, time, &format!("release (curve {curve})"));
            }
        }
    }

    #[test]
    fn test_curve_follows_stage_shape() {
        // The rendered envelope is stage_shape, sample for sample
        let (a, d, s, r) = (0.05, 0.08, 0.4, 0.12);
        let p = [a, d, s, r, 0.3, 0.6, 0.8, 0.5];
        let hold = ((a + d + 0.05) * SR) as usize;
        let out = render(&p, hold, hold + (r * SR) as usize + 100);

        let curve = |c: f32, u: f64| stage_shape(curve_steepness(c), u);
        let decay_start = first_reaching(&out, 0, 1.0, true);
        for step in 1..10 {
            let u = step as f64 / 10.0;
            let at = |start: usize, secs: f32| out[start + (u * (secs * SR) as f64).round() as usize] as f64;

            let attack = curve(0.3, u);
            let decay = 1.0 - (1.0 - s as f64) * curve(0.6, u);
            let release = s as f64 * (1.0 - curve(0.8, u));

            assert!((at(0, a) - attack).abs() < 1e-3, "attack at {u}: {} vs {attack}", at(0, a));
            assert!((at(decay_start, d) - decay).abs() < 1e-3, "decay at {u}");
            assert!((at(hold, r) - release).abs() < 1e-3, "release at {u}");
        }
    }

    #[test]
    fn test_attack_is_punchy_by_default() {
        // Halfway through the attack is about 70% up, like an RC aimed at
        // 1.2, not the 92% the old RC aimed at 1.0 gave
        let out = render(&params(0.1, 1.0, 0.7, 0.3), 48000, 48000);
        let halfway = out[(0.05 * SR) as usize];
        assert!(halfway > 0.6 && halfway < 0.8, "halfway level {halfway}");
    }

    #[test]
    fn test_adsr_attack_phase() {
        let out = render(&params(0.01, 1.0, 0.7, 0.3), 4800, 4800);
        assert!(out[0] < 0.01, "Envelope starts at zero on the gate edge");
        assert!(out[480] > 0.99, "Reaches the peak after the attack time");
        assert!(out.iter().all(|&s| s <= 1.0));
    }

    #[test]
    fn test_adsr_sustain_hold() {
        let out = render(&params(0.001, 0.001, 0.5, 0.3), 48000, 48000);
        for &sample in &out[40000..] {
            assert!((sample - 0.5).abs() < 1e-4, "Should hold at sustain level, got {}", sample);
        }
    }

    #[test]
    fn test_sustain_changes_glide() {
        let mut env = AdsrEnvelope::new();
        env.prepare(SR, 4800);
        let mut gate = SignalBuffer::control(4800);
        gate.fill(1.0);
        let ctx = ProcessContext::new(SR, 4800);
        let mut outputs = vec![SignalBuffer::control(4800)];
        env.process(&[&gate], &mut outputs, &params(0.001, 0.001, 0.8, 0.3), &ctx);
        assert!((outputs[0].samples[4799] - 0.8).abs() < 1e-4);

        // Drop the sustain knob: no step, but there within ~30 ms
        env.process(&[&gate], &mut outputs, &params(0.001, 0.001, 0.2, 0.3), &ctx);
        let out = &outputs[0].samples;
        let biggest_step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(biggest_step < 0.01, "sustain stepped by {biggest_step}");
        assert!((out[1440] - 0.2).abs() < 0.01, "sustain should settle, got {}", out[1440]);
    }

    #[test]
    fn test_velocity_scales_peak() {
        let total = 9600;
        let run = |velocity: f32, amount: f32, connected: bool| {
            let mut env = AdsrEnvelope::new();
            env.prepare(SR, total);
            let mut gate = SignalBuffer::control(total);
            gate.fill(1.0);
            let mut vel = if connected {
                SignalBuffer::control(total)
            } else {
                SignalBuffer::unconnected(total, SignalType::Control)
            };
            vel.fill(velocity);
            let mut outputs = vec![SignalBuffer::control(total)];
            let p = [0.01, 0.01, 0.5, 0.3, 0.2, 0.5, 0.5, amount];
            env.process(&[&gate, &SignalBuffer::control(total), &vel], &mut outputs, &p, &ProcessContext::new(SR, total));
            let out = outputs.remove(0).samples;
            (out.iter().cloned().fold(0.0, f32::max), out[total - 1])
        };

        // Full amount: peak is the velocity, sustain is a fraction of the peak
        let (peak, sustain) = run(0.4, 1.0, true);
        assert!((peak - 0.4).abs() < 1e-4, "peak {peak}");
        assert!((sustain - 0.2).abs() < 1e-3, "sustain {sustain}");

        // Half amount: halfway between full and the velocity
        let (peak, _) = run(0.4, 0.5, true);
        assert!((peak - 0.7).abs() < 1e-4, "peak {peak}");

        // Nothing plugged in: full peak whatever the amount
        let (peak, _) = run(0.0, 1.0, false);
        assert!((peak - 1.0).abs() < 1e-4, "peak {peak}");
    }

    #[test]
    fn test_adsr_release_phase() {
        let mut env = AdsrEnvelope::new();
        env.prepare(SR, 4800);
        let ctx = ProcessContext::new(SR, 4800);
        let p = params(0.001, 0.001, 0.7, 0.05);

        let mut gate_on = SignalBuffer::control(4800);
        gate_on.fill(1.0);
        let mut outputs = vec![SignalBuffer::control(4800)];
        env.process(&[&gate_on], &mut outputs, &p, &ctx);

        let gate_off = SignalBuffer::control(4800);
        env.process(&[&gate_off], &mut outputs, &p, &ctx);
        let out = &outputs[0].samples;

        assert!((out[0] - 0.7).abs() < 1e-3, "Release starts from sustain level");
        assert!(out[1200] < out[600] && out[600] < out[0], "Release falls");
        assert!(out[2400] < 1e-3, "Release lands on zero at its time");
        assert_eq!(out[2402], 0.0);
        assert_eq!(env.stage, EnvelopeStage::Idle);
    }

    #[test]
    fn test_adsr_gate_off_during_attack() {
        let out = render(&params(0.5, 0.1, 0.7, 0.05), 1000, 4800);

        // Released partway up, it falls from there to zero in the release time
        let level_at_gate_off = out[1000];
        assert!(level_at_gate_off > 0.0 && level_at_gate_off < 0.5);
        assert!(out[1001] < level_at_gate_off);
        assert!(out[1000 + 2400] < 1e-3);
        assert_eq!(out[1000 + 2402], 0.0);
    }

    #[test]
    fn test_adsr_retrigger() {
        let mut env = AdsrEnvelope::new();
        env.prepare(SR, 9600);

        let mut gate = SignalBuffer::control(9600);
        gate.fill(1.0);
        let mut retrigger = SignalBuffer::control(9600);
        retrigger.samples[4800..4810].fill(1.0);

        let mut outputs = vec![SignalBuffer::control(9600)];
        let ctx = ProcessContext::new(SR, 9600);
        env.process(&[&gate, &retrigger], &mut outputs, &params(0.01, 0.05, 0.5, 0.3), &ctx);

        let out = &outputs[0].samples;
        assert!((out[4799] - 0.5).abs() < 1e-3, "At sustain before the retrigger");
        // Back up to the peak one attack time later, smoothly
        assert!((out[4800 + 480] - 1.0).abs() < 1e-3, "got {}", out[4800 + 480]);
        assert!((out[4801] - out[4800]).abs() < 0.05);
    }

    #[test]
    fn test_adsr_reset() {
        let mut env = AdsrEnvelope::new();
        env.prepare(44100.0, 256);

        // Build up some state
        let mut gate = SignalBuffer::control(256);
        gate.fill(1.0);
        let mut outputs = vec![SignalBuffer::control(256)];
        let ctx = ProcessContext::new(44100.0, 256);
        env.process(&[&gate], &mut outputs, &params(0.001, 0.1, 0.7, 0.3), &ctx);

        // Reset
        env.reset();

        // Should be back to idle
        assert_eq!(env.stage, EnvelopeStage::Idle);
        assert_eq!(env.level, 0.0);

        // Process without gate - should output zeros
        let mut outputs2 = vec![SignalBuffer::control(256)];
        env.process(&[], &mut outputs2, &params(0.01, 0.1, 0.7, 0.3), &ctx);
        assert!(outputs2[0].samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_adsr_output_range() {
        for curve in [0.0, 0.5, 1.0] {
            let p = [0.01, 0.1, 0.7, 0.3, curve, curve, curve, 0.5];
            let out = render(&p, 22050, 44100);
            for &sample in &out {
                assert!((0.0..=1.0).contains(&sample), "Output {} out of range", sample);
            }
        }
    }

    #[test]
    fn test_adsr_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AdsrEnvelope>();
    }

    #[test]
    fn test_adsr_default() {
        let env = AdsrEnvelope::default();
        assert_eq!(env.info().id, "mod.adsr");
    }

    #[test]
    fn test_adsr_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<AdsrEnvelope>();

        assert!(registry.contains("mod.adsr"));

        let module = registry.create("mod.adsr");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "mod.adsr");
        assert_eq!(module.info().name, "ADSR Envelope");
        assert_eq!(module.ports().len(), 8);
        assert_eq!(module.parameters().len(), 8);
    }

    // --- Stage CV (#182) ---

    /// CV jacks, by port index.
    const ATTACK: usize = AdsrEnvelope::PORT_ATTACK_CV;
    const DECAY: usize = AdsrEnvelope::PORT_DECAY_CV;
    const SUSTAIN: usize = AdsrEnvelope::PORT_SUSTAIN_CV;
    const RELEASE: usize = AdsrEnvelope::PORT_RELEASE_CV;

    /// Runs the envelope over a gate high for `gate_samples` of `total`, with
    /// each `(jack, cv)` patched to follow `cv` sample by sample and the
    /// other jacks unpatched. Returns the output and the envelope.
    fn render_cv(
        params: &[f32],
        gate_samples: usize,
        total: usize,
        patched: &[(usize, &dyn Fn(usize) -> f32)],
    ) -> (Vec<f32>, AdsrEnvelope) {
        let mut env = AdsrEnvelope::new();
        env.prepare(SR, total);
        let mut inputs: Vec<SignalBuffer> =
            (0..=RELEASE).map(|_| SignalBuffer::unconnected(total, SignalType::Control)).collect();
        inputs[AdsrEnvelope::PORT_GATE] = SignalBuffer::control(total);
        inputs[AdsrEnvelope::PORT_GATE].samples[..gate_samples].fill(1.0);
        for &(jack, cv) in patched {
            inputs[jack] = SignalBuffer::control(total);
            inputs[jack].samples.iter_mut().enumerate().for_each(|(i, s)| *s = cv(i));
        }
        let inputs: Vec<&SignalBuffer> = inputs.iter().collect();
        let mut outputs = vec![SignalBuffer::control(total)];
        env.process(&inputs, &mut outputs, params, &ProcessContext::new(SR, total));
        (outputs.remove(0).samples, env)
    }

    #[test]
    fn test_attack_cv_is_in_octaves() {
        let total = (0.5 * SR) as usize;
        for (octaves, seconds) in [(0.0, 0.05), (1.0, 0.1), (-1.0, 0.025)] {
            let (out, _) = render_cv(&params(0.05, 0.1, 0.5, 0.1), total, total, &[(ATTACK, &|_| octaves)]);
            assert_within_5_percent(first_reaching(&out, 0, 1.0, true), seconds, &format!("attack at {octaves:+} oct"));
        }
    }

    #[test]
    fn test_sustain_cv_adds_to_the_knob() {
        let total = (0.5 * SR) as usize;
        for (cv, level) in [(0.0, 0.4), (0.3, 0.7), (-0.6, 0.0), (1.0, 1.0)] {
            let (out, _) = render_cv(&params(0.001, 0.01, 0.4, 0.1), total, total, &[(SUSTAIN, &|_| cv)]);
            assert!((out[total - 1] - level).abs() < 1e-3, "sustain CV {cv}: {} not {level}", out[total - 1]);
        }
    }

    #[test]
    fn test_release_cv_is_in_octaves() {
        // +1 doubles the release, -1 halves it, 0 is the knob
        let hold = 4800;
        for (octaves, seconds) in [(0.0, 0.2), (1.0, 0.4), (-1.0, 0.1), (2.0, 0.8)] {
            let total = hold + (1.0 * SR) as usize;
            let (out, _) = render_cv(&params(0.01, 0.01, 0.6, 0.2), hold, total, &[(RELEASE, &|_| octaves)]);
            let silent = first_reaching(&out, hold, 0.0, false);
            assert_within_5_percent(silent - hold, seconds, &format!("release at {octaves:+} oct"));
        }
    }

    #[test]
    fn test_harder_hits_decay_longer() {
        // Velocity into Decay CV: a full-velocity note decays twice as long
        // as the knob, a silent one at the knob
        let total = (1.0 * SR) as usize;
        let decay_time = |velocity: f32| {
            let (out, _) = render_cv(&params(0.001, 0.2, 0.0, 0.3), total, total, &[(DECAY, &|_| velocity)]);
            let peak = first_reaching(&out, 0, 1.0, true);
            first_reaching(&out, peak, 0.0, false) - peak
        };
        assert_within_5_percent(decay_time(0.0), 0.2, "soft note");
        assert_within_5_percent(decay_time(1.0), 0.4, "hard note");
        assert!(decay_time(0.5) > decay_time(0.0) && decay_time(0.5) < decay_time(1.0));
    }

    #[test]
    fn test_release_cv_stretches_a_release_already_falling() {
        // Halfway through a 100 ms release, CV goes to +1: the other half
        // takes 100 ms rather than 50, and the curve doesn't jump
        let hold = 4800;
        let cue = hold + (0.05 * SR) as usize;
        let total = hold + (0.5 * SR) as usize;
        let (out, _) = render_cv(&params(0.001, 0.001, 0.8, 0.1), hold, total, &[(RELEASE, &|i| if i >= cue { 1.0 } else { 0.0 })]);
        let silent = first_reaching(&out, hold, 0.0, false);
        assert_within_5_percent(silent - hold, 0.15, "release cued halfway");
        let biggest_step = out[hold..].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(biggest_step < 0.01, "release stepped by {biggest_step}");
    }

    #[test]
    fn test_cv_stays_within_the_knob_range() {
        // A 5 s release can't be stretched past 10 s, nor 2 ms shrunk below 1 ms
        assert_eq!(AdsrEnvelope::stretched(5.0, 3.0), MAX_TIME);
        assert_eq!(AdsrEnvelope::stretched(0.002, -4.0), MIN_TIME);
        assert!((AdsrEnvelope::stretched(0.3, 1.0) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_readout_reports_the_stretched_times() {
        let total = 4800;
        let p = params(0.01, 0.25, 0.5, 0.5);
        let read = |env: AdsrEnvelope| {
            let values = env.readout(&[]).unwrap().values;
            [AdsrEnvelope::READOUT_ATTACK, AdsrEnvelope::READOUT_DECAY, AdsrEnvelope::READOUT_SUSTAIN, AdsrEnvelope::READOUT_RELEASE]
                .map(|index| values[index])
        };

        // Unpatched: the knobs
        let (_, env) = render_cv(&p, total, total, &[]);
        assert_eq!(read(env), [0.01, 0.25, 0.5, 0.5]);

        // Patched: what CV made of them
        let (_, env) = render_cv(&p, total, total, &[(ATTACK, &|_| 2.0), (DECAY, &|_| 1.0), (SUSTAIN, &|_| 0.25), (RELEASE, &|_| -1.0)]);
        let [attack, decay, sustain, release] = read(env);
        assert!((attack - 0.04).abs() < 1e-6, "attack {attack}");
        assert!((decay - 0.5).abs() < 1e-6, "decay {decay}");
        assert!((sustain - 0.75).abs() < 1e-6, "sustain {sustain}");
        assert!((release - 0.25).abs() < 1e-6, "release {release}");
    }

    #[test]
    fn test_readout_reports_where_the_envelope_is() {
        // 100 ms into a held note: 10 ms of attack, then 90 ms of a 250 ms decay
        let total = 4800;
        let p = params(0.01, 0.25, 0.5, 0.5);
        let (out, env) = render_cv(&p, total, total, &[]);
        let values = env.readout(&[]).unwrap().values;
        assert_eq!(values[AdsrEnvelope::READOUT_STAGE], 2.0);
        assert_eq!(values[AdsrEnvelope::READOUT_PEAK], 1.0);
        let level = values[AdsrEnvelope::READOUT_LEVEL];
        assert!((level - out[total - 1]).abs() < 0.01, "level {level} vs out {}", out[total - 1]);
        // The progress turns back into the time through the stage
        let progress = values[AdsrEnvelope::READOUT_PROGRESS] as f64;
        let through = stage_time(curve_steepness(0.5), progress);
        assert!((through - 0.09 / 0.25).abs() < 0.01, "{through} of the way through the decay");
    }

    #[test]
    fn test_readout_reports_where_a_stage_began() {
        // Let go 30 ms into a 250 ms decay: the release starts well above the sustain
        let gate = 1920;
        let total = gate + 480;
        let p = params(0.01, 0.25, 0.5, 0.5);
        let (out, env) = render_cv(&p, gate, total, &[]);
        let values = env.readout(&[]).unwrap().values;
        assert_eq!(values[AdsrEnvelope::READOUT_STAGE], 4.0);
        let start = values[AdsrEnvelope::READOUT_STAGE_START];
        assert!((start - out[gate]).abs() < 1e-6 && start > 0.6, "released from {start}");
        assert_eq!(values[AdsrEnvelope::READOUT_STAGE_TARGET], 0.0);
        // The level is that far from where the stage began to where it's heading
        let progress = values[AdsrEnvelope::READOUT_PROGRESS];
        let level = values[AdsrEnvelope::READOUT_LEVEL];
        assert!((level - start * (1.0 - progress)).abs() < 1e-5, "level {level}");
    }

    #[test]
    fn test_stage_time_undoes_stage_shape() {
        for curve in [0.0, 0.3, 1.0] {
            let steepness = curve_steepness(curve);
            for step in 0..=10 {
                let u = step as f64 / 10.0;
                let back = stage_time(steepness, stage_shape(steepness, u));
                assert!((back - u).abs() < 1e-6, "curve {curve} at {u}: {back}");
            }
        }
    }

    #[test]
    fn test_adsr_zero_sustain() {
        let out = render(&params(0.01, 0.01, 0.0, 0.3), 48000, 48000);
        assert!(out[40000..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_adsr_full_sustain() {
        let out = render(&params(0.01, 0.01, 1.0, 0.3), 48000, 48000);
        assert!(out[1000..].iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }
}
