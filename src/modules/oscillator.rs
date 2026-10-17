//! The oscillator: a VCO with a tune section, hard sync, through-zero FM,
//! a sub-oscillator and unison.
//!
//! Every voice runs at twice the sample rate with polyBLEP steps and polyBLAMP
//! corners, then a halfband filter brings it back down. The corrections clean
//! up aliases that fold back near DC; the oversampling removes the ones that
//! would otherwise fold back near Nyquist, which two-point BLEPs barely touch.
//!
//! Its display draws short captures of Out as it plays (see [`trace`]).

mod trace;

use std::f32::consts::TAU;

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{BlepDelay, Downsampler2x},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    connected_input, ParameterDisplay, Readout, SignalType,
};

pub use trace::{CYCLES as TRACE_CYCLES, POINTS as TRACE_POINTS};
use trace::Trace;

/// Waveform shapes for the oscillator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscWaveform {
    Sine = 0,
    Saw = 1,
    Square = 2,
    Triangle = 3,
}

impl OscWaveform {
    /// Convert from parameter value (0-3) to waveform.
    pub fn from_param(value: f32) -> Self {
        match value as usize {
            0 => OscWaveform::Sine,
            1 => OscWaveform::Saw,
            2 => OscWaveform::Square,
            3 => OscWaveform::Triangle,
            _ => OscWaveform::Sine,
        }
    }

    /// Naive value at phase `p` in 0..=1. Phase 1.0 means "just before the
    /// wrap" and 0.0 "just after it", so both sides of the wrap are expressible.
    #[inline]
    pub(crate) fn value(self, p: f32, pulse_width: f32) -> f32 {
        match self {
            OscWaveform::Sine => (p * TAU).sin(),
            OscWaveform::Saw => 2.0 * p - 1.0,
            OscWaveform::Square => {
                if p < pulse_width {
                    1.0
                } else {
                    -1.0
                }
            }
            OscWaveform::Triangle => {
                if p < 0.5 {
                    4.0 * p - 1.0
                } else {
                    3.0 - 4.0 * p
                }
            }
        }
    }

    /// Slope at phase `p`, per unit of phase.
    #[inline]
    fn slope(self, p: f32) -> f32 {
        match self {
            OscWaveform::Sine => TAU * (p * TAU).cos(),
            OscWaveform::Saw => 2.0,
            OscWaveform::Square => 0.0,
            OscWaveform::Triangle => {
                if p < 0.5 {
                    4.0
                } else {
                    -4.0
                }
            }
        }
    }
}

/// Largest phase step per oversampled sample (a little under its Nyquist).
const MAX_DT: f32 = 0.45;

/// Where the phase meets breakpoint `b` between `p0` and `p1`, as the
/// unwrapped phase of the crossing. A phase of exactly 1.0 sits before the
/// wrap and 0.0 after it, so leaving either of them across the wrap counts.
#[inline]
fn crossing(p0: f32, p1: f32, b: f32) -> Option<f32> {
    if p1 > p0 {
        let mut target = b + (p0 - b).ceil();
        if target == p0 && p0 < 1.0 {
            target += 1.0;
        }
        (target <= p1).then_some(target)
    } else if p1 < p0 {
        let mut target = b + (p0 - b).floor();
        if target == p0 && p0 > 0.0 {
            target -= 1.0;
        }
        (target >= p1).then_some(target)
    } else {
        None
    }
}

/// Folds an advanced phase back into 0..=1. Landing exactly on the wrap
/// counts as having crossed it.
#[inline]
fn wrap(p: f32, dt: f32) -> f32 {
    if dt > 0.0 && p >= 1.0 {
        p - 1.0
    } else if dt < 0.0 && p <= 0.0 {
        p + 1.0
    } else {
        p
    }
}

/// Where a sync reset sends the phase: the start of the cycle in the
/// direction the phase is running.
#[inline]
fn sync_target(dt: f32) -> f32 {
    if dt < 0.0 {
        1.0
    } else {
        0.0
    }
}

/// One unison voice: a phase and its band-limiting corrections.
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    phase: f32,
    blep: BlepDelay,
}

impl Voice {
    /// Advances one oversampled sample and returns the finished previous
    /// sample. `sync` is where in this sample a hard-sync reset falls.
    #[inline]
    fn tick(&mut self, wave: OscWaveform, pulse_width: f32, dt: f32, sync: Option<f32>) -> f32 {
        match sync {
            Some(d) => {
                self.advance(wave, pulse_width, dt, 0.0, d);
                let target = sync_target(dt);
                let jump = wave.value(target, pulse_width) - wave.value(self.phase, pulse_width);
                let kink = (wave.slope(target) - wave.slope(self.phase)) * dt;
                self.blep.step(d, jump);
                self.blep.corner(d, kink);
                self.phase = target;
                self.advance(wave, pulse_width, dt, d, 1.0);
            }
            None => self.advance(wave, pulse_width, dt, 0.0, 1.0),
        }
        self.blep.push(wave.value(self.phase, pulse_width))
    }

    /// Runs the phase from time `t0` to `t1` within the sample, correcting
    /// every breakpoint it passes.
    #[inline]
    fn advance(&mut self, wave: OscWaveform, pulse_width: f32, dt: f32, t0: f32, t1: f32) {
        let p0 = self.phase;
        let p1 = p0 + dt * (t1 - t0);
        match wave {
            OscWaveform::Sine => {}
            OscWaveform::Saw => self.breakpoint(p0, p1, dt, t0, 0.0, -2.0, 0.0),
            OscWaveform::Square => {
                self.breakpoint(p0, p1, dt, t0, 0.0, 2.0, 0.0);
                self.breakpoint(p0, p1, dt, t0, pulse_width, -2.0, 0.0);
            }
            OscWaveform::Triangle => {
                self.breakpoint(p0, p1, dt, t0, 0.0, 0.0, 8.0);
                self.breakpoint(p0, p1, dt, t0, 0.5, 0.0, -8.0);
            }
        }
        self.phase = wrap(p1, dt);
    }

    /// Corrects for breakpoint `b` if the phase crosses it. `jump` and `kink`
    /// are the changes in value and in slope (per unit phase) crossing it
    /// forwards.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn breakpoint(&mut self, p0: f32, p1: f32, dt: f32, t0: f32, b: f32, jump: f32, kink: f32) {
        if let Some(target) = crossing(p0, p1, b) {
            let d = t0 + (target - p0) / dt;
            if jump != 0.0 {
                // Crossing backwards undoes the jump
                self.blep.step(d, jump * dt.signum());
            }
            if kink != 0.0 {
                // A corner bends the same way in either direction
                self.blep.corner(d, kink * dt.abs());
            }
        }
    }
}

/// The sub-oscillator: a flip-flop that changes state every time the main
/// pitch completes a cycle, giving a square one octave down.
#[derive(Clone, Copy, Debug, Default)]
struct SubOscillator {
    phase: f32,
    low: bool,
    blep: BlepDelay,
    /// Where in the last sample it went high, starting a pair of cycles.
    rose: Option<f32>,
}

impl SubOscillator {
    #[inline]
    fn tick(&mut self, dt: f32, sync: Option<f32>) -> f32 {
        self.rose = None;
        match sync {
            Some(d) => {
                self.advance(dt, 0.0, d);
                // A reset only clocks the divider when it's a real fall: right
                // after a natural wrap it would toggle twice in a row
                let far = if dt < 0.0 { self.phase <= 0.5 } else { self.phase >= 0.5 };
                if far {
                    self.toggle(d);
                }
                self.phase = sync_target(dt);
                self.advance(dt, d, 1.0);
            }
            None => self.advance(dt, 0.0, 1.0),
        }
        self.blep.push(self.level())
    }

    #[inline]
    fn advance(&mut self, dt: f32, t0: f32, t1: f32) {
        let p0 = self.phase;
        let p1 = p0 + dt * (t1 - t0);
        if let Some(target) = crossing(p0, p1, 0.0) {
            self.toggle(t0 + (target - p0) / dt);
        }
        self.phase = wrap(p1, dt);
    }

    #[inline]
    fn toggle(&mut self, d: f32) {
        let before = self.level();
        self.low = !self.low;
        self.blep.step(d, self.level() - before);
        if !self.low {
            self.rose = Some(d);
        }
    }

    #[inline]
    fn level(&self) -> f32 {
        if self.low {
            -1.0
        } else {
            1.0
        }
    }
}

/// The most unison voices.
pub const MAX_VOICES: usize = 7;

/// Detune, in cents, of the outermost unison voice at full Detune.
const MAX_DETUNE_CENTS: f32 = 100.0;

/// A multi-waveform VCO.
///
/// # Ports
///
/// **Inputs:**
/// - **V/Oct** (Control): 1 per octave pitch CV, added to the tune section.
/// - **FM** (Control): through-zero linear FM. The pitch is multiplied by
///   `1 + FM Depth × FM`, so past a depth of 1 the frequency swings through
///   zero and the waveform runs backwards.
/// - **Exp FM** (Control): exponential FM, `Exp FM Depth` octaves per unit.
/// - **PWM** (Control): pulse width modulation around Pulse Width.
/// - **Sync** (Control): hard sync. Each rising zero crossing restarts the cycle.
///
/// **Outputs:**
/// - **Out** (Audio): all voices, mono.
/// - **Sub** (Audio): square one octave below the tuned pitch.
/// - **Out L / Out R** (Audio): the voices spread across the stereo field.
///
/// # Parameters
///
/// - **Octave / Semitone / Fine**: pitch relative to C4 (261.63 Hz).
/// - **FM Depth** (0-5): linear FM index.
/// - **Exp FM Depth** (0-4 octaves): exponential FM range.
/// - **Waveform** (Sine/Saw/Square/Tri).
/// - **Pulse Width** (0.1-0.9): duty cycle of the square.
/// - **Voices** (1-7): unison voices. 7 saws is a supersaw.
/// - **Detune**: spread of the unison voices' pitches, up to ±100 cents.
/// - **Spread**: stereo width of the unison voices on Out L / Out R.
pub struct Oscillator {
    sample_rate: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    /// Smoothed tune section, in octaves above C4.
    pitch_smooth: SmoothedValue,
    fm_depth_smooth: SmoothedValue,
    exp_depth_smooth: SmoothedValue,
    pulse_width_smooth: SmoothedValue,
    voices: [Voice; MAX_VOICES],
    sub: SubOscillator,
    /// Per-voice frequency ratio and stereo gains, set once per block.
    ratios: [f32; MAX_VOICES],
    gains_left: [f32; MAX_VOICES],
    gains_right: [f32; MAX_VOICES],
    /// Last sync input sample, for edge detection.
    prev_sync: f32,
    /// Last phase step, for interpolating across the two oversampled samples.
    prev_dt: f32,
    down_mono: Downsampler2x,
    down_sub: Downsampler2x,
    down_left: Downsampler2x,
    down_right: Downsampler2x,
    /// Captures of Out for the display.
    trace: Trace,
    /// The pitch V/Oct and Exp FM have taken it to, and the pulse width
    /// PWM has, at the end of the last block.
    live_hz: f32,
    live_pulse_width: f32,
    /// The pitch in octaves above C4 that `live_hz` was worked out from, so
    /// a steady pitch skips the `exp2`.
    live_octaves: f32,
}

impl Oscillator {
    /// Frequency of C4 (MIDI note 60), the pitch at V/Oct 0.0.
    pub const C4_HZ: f32 = 261.625_58;

    const PORT_V_OCT: usize = 0;
    const PORT_FM: usize = 1;
    const PORT_EXP_FM: usize = 2;
    const PORT_PWM: usize = 3;
    const PORT_SYNC: usize = 4;

    const OUT_MONO: usize = 0;
    const OUT_SUB: usize = 1;
    const OUT_LEFT: usize = 2;
    const OUT_RIGHT: usize = 3;

    const PARAM_OCTAVE: usize = 0;
    const PARAM_SEMITONE: usize = 1;
    const PARAM_FINE: usize = 2;
    const PARAM_FM_DEPTH: usize = 3;
    const PARAM_EXP_DEPTH: usize = 4;
    const PARAM_WAVEFORM: usize = 5;
    const PARAM_PULSE_WIDTH: usize = 6;
    const PARAM_VOICES: usize = 7;
    const PARAM_DETUNE: usize = 8;
    const PARAM_SPREAD: usize = 9;

    /// The readout: the pitch it's playing before linear FM, in Hz, and the
    /// pulse width.
    pub const READOUT_HZ: usize = 0;
    pub const READOUT_PULSE_WIDTH: usize = 1;

    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let mut osc = Self {
            sample_rate,
            ports: vec![
                PortDefinition::input_with_default("v_oct", "V/Oct", SignalType::Control, 0.0).describe("Pitch in volts per octave; 1 V up is one octave up"),
                PortDefinition::input_with_default("fm", "FM", SignalType::Control, 0.0).describe("Linear FM signal, scaled by FM Depth; strong settings swing through zero"),
                PortDefinition::input_with_default("exp_fm", "Exp FM", SignalType::Control, 0.0).describe("Exponential FM signal, scaled by Exp FM Depth in octaves"),
                PortDefinition::input_with_default("pwm", "PWM", SignalType::Control, 0.0).describe("Modulates the pulse width of the square wave"),
                PortDefinition::input_with_default("sync", "Sync", SignalType::Control, 0.0).describe("Hard sync; each rising edge restarts the cycle"),
                PortDefinition::output("out", "Out", SignalType::Audio).describe("All unison voices mixed to mono"),
                PortDefinition::output("sub", "Sub", SignalType::Audio).describe("Square wave one octave below the pitch"),
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("Left side of the unison voices spread across stereo"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("Right side of the unison voices spread across stereo"),
            ],
            parameters: vec![
                ParameterDefinition::new("octave", "Octave", -4.0, 4.0, 0.0, ParameterDisplay::stepped("oct")).describe("Shifts the pitch in whole octaves from C4"),
                ParameterDefinition::new("semitone", "Semitone", -12.0, 12.0, 0.0, ParameterDisplay::stepped("st")).describe("Shifts the pitch in whole semitones"),
                ParameterDefinition::new("fine", "Fine", -100.0, 100.0, 0.0, ParameterDisplay::linear("ct")).describe("Fine tuning in cents; 100 is one semitone"),
                ParameterDefinition::new("fm_depth", "FM Depth", 0.0, 5.0, 0.0, ParameterDisplay::linear("")).describe("How strongly the FM input bends the pitch"),
                ParameterDefinition::new("exp_fm_depth", "Exp FM Depth", 0.0, 4.0, 1.0, ParameterDisplay::linear("oct")).describe("Octaves of pitch change per unit of Exp FM signal"),
                ParameterDefinition::choice("waveform", "Waveform", &["Sine", "Saw", "Square", "Tri"], 0).describe("Shape of the oscillator's sound"),
                ParameterDefinition::new("pulse_width", "Pulse Width", 0.1, 0.9, 0.5, ParameterDisplay::linear("")).describe("Width of the square's high part; 0.5 is an even square"),
                ParameterDefinition::new("voices", "Voices", 1.0, MAX_VOICES as f32, 1.0, ParameterDisplay::stepped("")).describe("Number of stacked unison voices; 7 saws is a supersaw"),
                ParameterDefinition::normalized("detune", "Detune", 0.4).describe("How far apart the unison voices are tuned, up to 100 cents"),
                ParameterDefinition::normalized("spread", "Spread", 0.5).describe("How widely the unison voices pan on Out L and Out R"),
            ],
            pitch_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            fm_depth_smooth: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            exp_depth_smooth: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            pulse_width_smooth: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            voices: [Voice::default(); MAX_VOICES],
            sub: SubOscillator::default(),
            ratios: [1.0; MAX_VOICES],
            gains_left: [1.0; MAX_VOICES],
            gains_right: [1.0; MAX_VOICES],
            prev_sync: 0.0,
            prev_dt: 0.0,
            down_mono: Downsampler2x::new(),
            down_sub: Downsampler2x::new(),
            down_left: Downsampler2x::new(),
            down_right: Downsampler2x::new(),
            trace: Trace::new(),
            live_hz: Self::C4_HZ,
            live_octaves: 0.0,
            live_pulse_width: 0.5,
        };
        osc.reset_phases();
        osc
    }

    /// The tune section as octaves above C4. Octave and Semitone click to
    /// whole steps.
    pub fn tune_octaves(octave: f32, semitone: f32, fine_cents: f32) -> f32 {
        octave.round() + semitone.round() / 12.0 + fine_cents / 1200.0
    }

    /// Splits a frequency into the nearest Octave and Semitone, with the
    /// remainder in Fine (cents). Pitches beyond the tune section's reach
    /// (about 7.7 Hz to 8.9 kHz) are clamped to its ends.
    pub fn tune_from_hz(hz: f32) -> (f32, f32, f32) {
        let semis = 12.0 * (hz.max(1e-3) as f64 / Self::C4_HZ as f64).log2();
        let nearest = semis.round();
        let octave = (nearest / 12.0).floor().clamp(-4.0, 4.0);
        let semitone = (nearest - 12.0 * octave).clamp(-12.0, 12.0);
        let fine = ((semis - 12.0 * octave - semitone) * 100.0).clamp(-100.0, 100.0);
        (octave as f32, semitone as f32, fine as f32)
    }

    /// Position of unison voice `i` of `n`, from -1 to 1.
    fn voice_position(i: usize, n: usize) -> f32 {
        if n <= 1 {
            0.0
        } else {
            -1.0 + 2.0 * i as f32 / (n - 1) as f32
        }
    }

    /// How far voice `i` of `n` is detuned, in cents, at a Detune of `detune`.
    pub(crate) fn unison_cents(i: usize, n: usize, detune: f32) -> f32 {
        let x = Self::voice_position(i, n);
        x.signum() * x.abs().powf(1.5) * MAX_DETUNE_CENTS * detune * detune
    }

    /// Lays out the unison voices: pitch ratios and stereo gains.
    ///
    /// Voices sit evenly across the stereo field, but their detune bends
    /// toward the centre (|x|^1.5), close to the JP-8000 supersaw's offsets.
    /// Uneven spacing also keeps the voices from beating in lockstep.
    fn layout_voices(&mut self, n: usize, detune: f32, spread: f32) {
        for i in 0..n {
            let x = Self::voice_position(i, n);
            self.ratios[i] = (Self::unison_cents(i, n, detune) / 1200.0).exp2();
            let pan = x * spread;
            self.gains_left[i] = (1.0 - pan).min(1.0);
            self.gains_right[i] = (1.0 + pan).min(1.0);
        }
    }

    /// Starts each voice somewhere different along its cycle (golden-ratio
    /// steps), so a unison doesn't start as one loud spike. A single voice
    /// starts at phase 0.
    fn reset_phases(&mut self) {
        const GOLDEN: f32 = 0.618_034;
        for (i, voice) in self.voices.iter_mut().enumerate() {
            *voice = Voice { phase: (i as f32 * GOLDEN).fract(), blep: BlepDelay::new() };
        }
        self.sub = SubOscillator::default();
    }
}

impl Default for Oscillator {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Oscillator {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "osc.sine",
            name: "Oscillator",
            category: ModuleCategory::Source,
            description: "Band-limited VCO with hard sync, through-zero FM, sub-oscillator and unison",
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
        self.pitch_smooth.set_sample_rate(sample_rate);
        self.fm_depth_smooth.set_sample_rate(sample_rate);
        self.exp_depth_smooth.set_sample_rate(sample_rate);
        self.pulse_width_smooth.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        self.pitch_smooth.set_target(Self::tune_octaves(
            params[Self::PARAM_OCTAVE],
            params[Self::PARAM_SEMITONE],
            params[Self::PARAM_FINE],
        ));
        self.fm_depth_smooth.set_target(params[Self::PARAM_FM_DEPTH]);
        self.exp_depth_smooth.set_target(params[Self::PARAM_EXP_DEPTH]);
        self.pulse_width_smooth.set_target(params[Self::PARAM_PULSE_WIDTH]);

        let wave = OscWaveform::from_param(params[Self::PARAM_WAVEFORM]);
        let n = (params[Self::PARAM_VOICES].round() as usize).clamp(1, MAX_VOICES);
        self.layout_voices(n, params[Self::PARAM_DETUNE], params[Self::PARAM_SPREAD]);
        // Unison voices are mostly uncorrelated, so they add in power
        let norm = 1.0 / (n as f32).sqrt();

        let input = |port: usize, i: usize| {
            inputs.get(port).and_then(|buf| buf.samples.get(i)).copied().unwrap_or(0.0)
        };
        let sync_in = connected_input(inputs, Self::PORT_SYNC);
        let inv_rate = 1.0 / (2.0 * self.sample_rate);
        let tuned_hz = Self::C4_HZ * self.pitch_smooth.target().exp2();

        // Outputs nothing hears are left silent. Out is always worked out:
        // the display draws it
        let sub_heard = context.output_heard(Self::OUT_SUB);
        let (left_heard, right_heard) = (context.output_heard(Self::OUT_LEFT), context.output_heard(Self::OUT_RIGHT));
        let stereo = left_heard || right_heard;
        // ...and their downsamplers start from silence when they're patched again
        for (heard, down) in [(sub_heard, &mut self.down_sub), (left_heard, &mut self.down_left), (right_heard, &mut self.down_right)] {
            if !heard {
                down.reset();
            }
        }

        for i in 0..context.block_size {
            let pitch = self.pitch_smooth.next();
            let fm_depth = self.fm_depth_smooth.next();
            let exp_depth = self.exp_depth_smooth.next();
            let pulse_width = (self.pulse_width_smooth.next() + input(Self::PORT_PWM, i) * 0.4).clamp(0.1, 0.9);

            let octaves = pitch + input(Self::PORT_V_OCT, i) + input(Self::PORT_EXP_FM, i) * exp_depth;
            if octaves != self.live_octaves {
                self.live_octaves = octaves;
                self.live_hz = Self::C4_HZ * octaves.exp2();
            }
            self.live_pulse_width = pulse_width;
            // Through-zero: the frequency may go negative and the phase run backwards
            let hz = self.live_hz * (1.0 + fm_depth * input(Self::PORT_FM, i));
            let dt = (hz * inv_rate).clamp(-MAX_DT, MAX_DT);

            // A rising zero crossing between the last input sample and this one
            let sync = sync_in.and_then(|buf| {
                let s = buf.at(i);
                let prev = std::mem::replace(&mut self.prev_sync, s);
                (prev <= 0.0 && s > 0.0).then(|| prev / (prev - s))
            });

            // Two oversampled steps; the first one halfway between the old pitch and the new
            let steps = [0.5 * (self.prev_dt + dt), dt];
            self.prev_dt = dt;

            let mut mono = [0.0; 2];
            let mut left = [0.0; 2];
            let mut right = [0.0; 2];
            let mut sub = [0.0; 2];
            // The reset lands in whichever oversampled step contains it
            let sync_step = sync.map(|d| if d < 0.5 { (0, 2.0 * d) } else { (1, 2.0 * d - 1.0) });
            // How long before the end of this sample the Sub last went high
            let mut rose = None;
            for (k, &step) in steps.iter().enumerate() {
                let sync_here = sync_step.filter(|&(at, _)| at == k).map(|(_, d)| d);
                for v in 0..n {
                    let s = self.voices[v].tick(wave, pulse_width, step * self.ratios[v], sync_here);
                    mono[k] += s;
                    if stereo {
                        left[k] += s * self.gains_left[v];
                        right[k] += s * self.gains_right[v];
                    }
                }
                sub[k] = self.sub.tick(step, sync_here);
                if let Some(d) = self.sub.rose {
                    rose = Some((2.0 - k as f32 - d) * 0.5);
                }
            }

            let out = self.down_mono.process(mono) * norm;
            outputs[Self::OUT_MONO].samples[i] = out;
            if let Some(ago) = rose {
                self.trace.cycle(ago, tuned_hz, self.sample_rate);
            }
            self.trace.push(out);
            if sub_heard {
                outputs[Self::OUT_SUB].samples[i] = self.down_sub.process(sub);
            }
            if left_heard {
                outputs[Self::OUT_LEFT].samples[i] = self.down_left.process(left) * norm;
            }
            if right_heard {
                outputs[Self::OUT_RIGHT].samples[i] = self.down_right.process(right) * norm;
            }
        }
    }

    fn reset(&mut self) {
        self.reset_phases();
        self.prev_sync = 0.0;
        self.prev_dt = 0.0;
        self.down_mono.reset();
        self.down_sub.reset();
        self.down_left.reset();
        self.down_right.reset();
        self.trace.reset();
        self.pitch_smooth.reset(self.pitch_smooth.target());
        self.fm_depth_smooth.reset(self.fm_depth_smooth.target());
        self.exp_depth_smooth.reset(self.exp_depth_smooth.target());
        self.pulse_width_smooth.reset(self.pulse_width_smooth.target());
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_HZ] = self.live_hz;
        readout.values[Self::READOUT_PULSE_WIDTH] = self.live_pulse_width;
        Some(readout)
    }

    fn take_scope_data(&mut self) -> Option<(&[f32], &[f32], bool)> {
        self.trace.take().map(|points| (points, &[][..], true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{rms, Spectrum};

    const SR: f32 = 48000.0;

    /// Default parameters, in definition order.
    fn defaults() -> Vec<f32> {
        Oscillator::new().parameters().iter().map(|p| p.default).collect()
    }

    fn with(mut params: Vec<f32>, index: usize, value: f32) -> Vec<f32> {
        params[index] = value;
        params
    }

    fn outputs(len: usize) -> Vec<SignalBuffer> {
        (0..4).map(|_| SignalBuffer::audio(len)).collect()
    }

    /// Runs `osc` for `len` samples with the given inputs, in blocks of 256.
    fn run(osc: &mut Oscillator, params: &[f32], inputs: &[SignalBuffer], len: usize) -> Vec<Vec<f32>> {
        const BLOCK: usize = 256;
        let mut result = (0..4).map(|_| Vec::with_capacity(len)).collect::<Vec<_>>();
        let mut done = 0;
        while done < len {
            let block = BLOCK.min(len - done);
            let slices: Vec<SignalBuffer> = inputs
                .iter()
                .map(|buf| {
                    let mut b = if buf.is_connected() {
                        SignalBuffer::control(block)
                    } else {
                        SignalBuffer::unconnected(block, SignalType::Control)
                    };
                    b.samples.copy_from_slice(&buf.samples[done..done + block]);
                    b
                })
                .collect();
            let refs: Vec<&SignalBuffer> = slices.iter().collect();
            let mut outs = outputs(block);
            osc.process(&refs, &mut outs, params, &ProcessContext::new(SR, block));
            for (r, o) in result.iter_mut().zip(&outs) {
                r.extend_from_slice(&o.samples);
            }
            done += block;
        }
        result
    }

    #[test]
    fn unheard_outputs_are_left_silent_and_out_is_unchanged() {
        // A unison, so Out L and Out R differ from Out
        let params = with(with(defaults(), Oscillator::PARAM_VOICES, 5.0), Oscillator::PARAM_WAVEFORM, 1.0);
        let all = render(&params, 1024);
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        osc.reset();
        let (mut out, mut sub, mut right) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..4 {
            let mut outs = outputs(256);
            // Out and Out R heard
            osc.process(&[], &mut outs, &params, &ProcessContext::new(SR, 256).hearing(0b1001));
            assert!(outs[Oscillator::OUT_LEFT].samples.iter().all(|&s| s == 0.0));
            out.extend_from_slice(&outs[Oscillator::OUT_MONO].samples);
            sub.extend_from_slice(&outs[Oscillator::OUT_SUB].samples);
            right.extend_from_slice(&outs[Oscillator::OUT_RIGHT].samples);
        }
        assert_eq!(out, all[Oscillator::OUT_MONO]);
        assert_eq!(right, all[Oscillator::OUT_RIGHT]);
        assert!(sub.iter().all(|&s| s == 0.0));
    }

    fn render(params: &[f32], len: usize) -> Vec<Vec<f32>> {
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        osc.reset();
        run(&mut osc, params, &[], len)
    }

    /// Input buffers with only `port` connected, holding `samples`.
    fn patched(port: usize, samples: Vec<f32>) -> Vec<SignalBuffer> {
        let len = samples.len();
        (0..=port)
            .map(|p| {
                if p == port {
                    let mut b = SignalBuffer::control(len);
                    b.samples.copy_from_slice(&samples);
                    b
                } else {
                    SignalBuffer::unconnected(len, SignalType::Control)
                }
            })
            .collect()
    }

    /// Frequency from upward zero crossings, interpolated, over the whole signal.
    fn measured_hz(samples: &[f32]) -> f64 {
        let mut crossings = Vec::new();
        for i in 1..samples.len() {
            let (a, b) = (samples[i - 1] as f64, samples[i] as f64);
            if a <= 0.0 && b > 0.0 {
                crossings.push(i as f64 - 1.0 + a / (a - b));
            }
        }
        let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
        (crossings.len() - 1) as f64 * SR as f64 / (last - first)
    }

    fn cents(f: f64, reference: f64) -> f64 {
        1200.0 * (f / reference).log2()
    }

    #[test]
    fn test_info_and_ports() {
        let osc = Oscillator::new();
        assert_eq!(osc.info().id, "osc.sine");
        assert_eq!(osc.info().name, "Oscillator");
        assert_eq!(osc.info().category, ModuleCategory::Source);
        let ids: Vec<_> = osc.ports().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["v_oct", "fm", "exp_fm", "pwm", "sync", "out", "sub", "out_l", "out_r"]);
        let names: Vec<_> = osc.parameters().iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            ["Octave", "Semitone", "Fine", "FM Depth", "Exp FM Depth", "Waveform", "Pulse Width", "Voices", "Detune", "Spread"]
        );
    }

    #[test]
    fn test_default_is_c4() {
        let out = render(&defaults(), 48000);
        let c = cents(measured_hz(&out[0][4800..]), Oscillator::C4_HZ as f64);
        assert!(c.abs() < 0.1, "{c:+.3} cents");
    }

    #[test]
    fn test_midi_note_69_is_440_hz() {
        // MIDI note 69 arrives as V/Oct (69 - 60) / 12
        let len = 96000;
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        let inputs = patched(Oscillator::PORT_V_OCT, vec![crate::modules::MidiNote::midi_to_voct(69.0); len]);
        let out = run(&mut osc, &defaults(), &inputs, len);
        let c = cents(measured_hz(&out[0][4800..]), 440.0);
        assert!(c.abs() < 0.1, "MIDI 69 measured {c:+.4} cents from 440 Hz");
    }

    #[test]
    fn test_tune_section() {
        assert_eq!(Oscillator::tune_octaves(1.0, 0.0, 0.0), 1.0);
        assert_eq!(Oscillator::tune_octaves(0.0, 12.0, 0.0), 1.0);
        assert!((Oscillator::tune_octaves(0.0, 0.0, 1200.0) - 1.0).abs() < 1e-6);
        // Octave and Semitone snap to whole steps
        assert_eq!(Oscillator::tune_octaves(0.8, 2.6, 0.0), 1.0 + 3.0 / 12.0);

        // A4 from the knobs: octave 0, +9 semitones
        let params = with(defaults(), Oscillator::PARAM_SEMITONE, 9.0);
        let out = render(&params, 48000);
        let c = cents(measured_hz(&out[0][4800..]), 440.0);
        assert!(c.abs() < 0.1, "{c:+.3} cents");
    }

    #[test]
    fn test_pitch_glides_evenly_in_octaves() {
        // Smoothed in log2(Hz): a jump up and the same jump down cross the
        // midpoint at the same time
        let mut up = Oscillator::new();
        up.prepare(SR, 1);
        up.pitch_smooth.set_target(-2.0);
        up.pitch_smooth.set_target(2.0);
        let mut down = Oscillator::new();
        down.prepare(SR, 1);
        down.pitch_smooth.set_target(2.0);
        down.pitch_smooth.set_target(-2.0);
        let up_mid = (0..2000).position(|_| up.pitch_smooth.next() >= 0.0).unwrap();
        let down_mid = (0..2000).position(|_| down.pitch_smooth.next() <= 0.0).unwrap();
        assert!(up_mid > 100, "glided to the midpoint in only {up_mid} samples");
        assert!(up_mid.abs_diff(down_mid) <= 1, "{up_mid} vs {down_mid}");
    }

    /// Length of the leak-free analysis window.
    const N: usize = 16384;

    /// Aliasing below 20 kHz, in dB relative to everything below 20 kHz, of
    /// the last N samples. `fundamental` must fit a whole number of times in
    /// N samples: then no window is needed, nothing leaks, and every bin
    /// between harmonics is genuine alias.
    fn alias_db(out: &[f32], fundamental: f64) -> f64 {
        Spectrum::of_periodic(&out[out.len() - N..], SR).alias_energy_db(fundamental, 0.5, 20000.0)
    }

    /// V/Oct that plays exactly `cycles` periods per N samples.
    fn bin_exact_voct(cycles: usize) -> (f64, f32) {
        let hz = cycles as f64 * SR as f64 / N as f64;
        (hz, (hz / Oscillator::C4_HZ as f64).log2() as f32)
    }

    #[test]
    fn test_saw_and_triangle_alias_below_minus_60_db_at_5_khz() {
        // 1707 cycles per 16384 samples at 48 kHz = 5000.98 Hz
        let (hz, voct) = bin_exact_voct(1707);
        for wave in [OscWaveform::Saw, OscWaveform::Triangle, OscWaveform::Square] {
            let params = with(defaults(), Oscillator::PARAM_WAVEFORM, wave as usize as f32);
            let mut osc = Oscillator::new();
            osc.prepare(SR, 256);
            let len = 3 * N;
            let out = run(&mut osc, &params, &patched(Oscillator::PORT_V_OCT, vec![voct; len]), len);
            let db = alias_db(&out[0], hz);
            eprintln!("{wave:?} at {hz:.1} Hz: alias {db:.1} dB");
            assert!(db < -60.0, "{wave:?} alias energy {db:.1} dB");
        }
    }

    /// A sine at exactly `cycles` periods per N samples.
    fn bin_exact_sine(cycles: usize, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (std::f64::consts::TAU * cycles as f64 * i as f64 / N as f64).sin() as f32)
            .collect()
    }

    #[test]
    fn test_hard_sync_locks_to_master() {
        // A 187.5 Hz master (period exactly 256 samples) resets a saw tuned
        // well above it; the result repeats at the master's period
        let master_hz = 64.0 * SR as f64 / N as f64;
        let len = 3 * N;
        let mut params = with(defaults(), Oscillator::PARAM_WAVEFORM, 1.0);
        params[Oscillator::PARAM_OCTAVE] = 1.0;
        params[Oscillator::PARAM_FINE] = 37.0;
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        let out = run(&mut osc, &params, &patched(Oscillator::PORT_SYNC, bin_exact_sine(64, len)), len);

        let spectrum = Spectrum::of_periodic(&out[0][len - N..], SR);
        assert!(spectrum.magnitudes[64] > 0.05, "synced saw should have the master's fundamental");
        // Periodic at the master's period, so everything sits on its harmonics
        let db = alias_db(&out[0], master_hz);
        assert!(db < -100.0, "synced saw should repeat every master cycle: {db:.1} dB");
    }

    #[test]
    fn test_sync_reset_is_band_limited() {
        // 61 master cycles per N: a period of 268.59 samples, so each reset
        // falls at a different point between samples and any alias lands
        // between the harmonics. The master is a rising ramp through zero, so
        // the interpolated crossing time is exact.
        let cycles = 61;
        let master_hz = cycles as f64 * SR as f64 / N as f64;
        let len = 3 * N;
        let master: Vec<f32> = (0..len)
            .map(|i| ((cycles as f64 * i as f64 / N as f64).fract() - 0.5) as f32)
            .collect();
        let mut params = with(defaults(), Oscillator::PARAM_WAVEFORM, 1.0);
        params[Oscillator::PARAM_OCTAVE] = 1.0;
        params[Oscillator::PARAM_FINE] = 37.0;
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        let out = run(&mut osc, &params, &patched(Oscillator::PORT_SYNC, master), len);
        let db = alias_db(&out[0], master_hz);
        eprintln!("synced saw at {master_hz:.2} Hz: alias {db:.1} dB");
        assert!(db < -60.0, "synced saw alias energy {db:.1} dB");
    }

    #[test]
    fn test_sync_tracks_master_through_sub() {
        // The sub divides the synced slave, so it lands on half the master
        let len = 3 * N;
        let params = with(defaults(), Oscillator::PARAM_OCTAVE, 1.0);
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        let out = run(&mut osc, &params, &patched(Oscillator::PORT_SYNC, bin_exact_sine(64, len)), len);
        let master_hz = 64.0 * SR as f64 / N as f64;
        let db = alias_db(&out[1], master_hz / 2.0);
        assert!(db < -50.0, "sub should repeat every two master cycles: {db:.1} dB");
    }

    #[test]
    fn test_outputs_stay_in_range() {
        for wave in 0..4 {
            let params = with(defaults(), Oscillator::PARAM_WAVEFORM, wave as f32);
            let out = render(&params, 9600);
            for (port, samples) in out.iter().enumerate() {
                // Band-limited edges overshoot: the halfband's steep cut
                // rings near 24 kHz for a few samples after each edge
                // (inaudible, about +2.6 dB of peak). Skip the start, where
                // the output leaps from silence.
                let peak = samples[200..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!(peak <= 1.4, "wave {wave} port {port}: peak {peak}");
            }
        }
    }

    #[test]
    fn test_sub_is_an_octave_down() {
        let params = with(defaults(), Oscillator::PARAM_WAVEFORM, 1.0);
        let out = render(&params, 96000);
        let main = measured_hz(&out[0][4800..]);
        let sub = measured_hz(&out[1][4800..]);
        assert!(cents(sub, main / 2.0).abs() < 0.1, "main {main:.3} Hz, sub {sub:.3} Hz");
        assert!(rms(&out[1][4800..]) > 0.9, "sub is a full-scale square");
    }

    #[test]
    fn test_through_zero_fm_runs_backwards() {
        // FM input of -2 at depth 1: the frequency is -C4, so the saw falls
        let len = 9600;
        let params = with(with(defaults(), Oscillator::PARAM_WAVEFORM, 1.0), Oscillator::PARAM_FM_DEPTH, 1.0);
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        let out = run(&mut osc, &params, &patched(Oscillator::PORT_FM, vec![-2.0; len]), len);
        let ramp = &out[0][4800..];
        let falling = ramp.windows(2).filter(|w| w[1] < w[0]).count();
        // Most samples fall; the rest are the ringing either side of each edge
        assert!(falling > ramp.len() * 3 / 4, "saw should ramp down: {falling} of {}", ramp.len());
        let f = measured_hz(&out[0][4800..]);
        assert!(cents(f, Oscillator::C4_HZ as f64).abs() < 1.0, "{f:.2} Hz");
    }

    #[test]
    fn test_through_zero_fm_is_symmetric() {
        // Sine carrier, sine modulator at the carrier frequency, index 2:
        // through-zero FM keeps the carrier's average pitch where linear FM
        // that clamps at 0 Hz would pull it sharp
        let len = 96000;
        let modulator: Vec<f32> = (0..len)
            .map(|i| (TAU as f64 * 50.0 * i as f64 / SR as f64).sin() as f32)
            .collect();
        let params = with(defaults(), Oscillator::PARAM_FM_DEPTH, 2.0);
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        let out = run(&mut osc, &params, &patched(Oscillator::PORT_FM, modulator), len);
        let spectrum = Spectrum::of(&out[0][4800..], SR);
        let carrier_bin = (Oscillator::C4_HZ as f64 / spectrum.bin_hz).round() as usize;
        let peak = spectrum.magnitudes[carrier_bin - 3..=carrier_bin + 3].iter().cloned().fold(0.0, f64::max);
        assert!(peak > 0.01, "carrier line should survive: {peak}");
        assert!(out[0].iter().all(|s| s.is_finite()));
    }

    #[test]
    fn test_exp_fm_is_octaves() {
        let len = 48000;
        let params = with(defaults(), Oscillator::PARAM_EXP_DEPTH, 2.0);
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        let out = run(&mut osc, &params, &patched(Oscillator::PORT_EXP_FM, vec![0.5; len]), len);
        let c = cents(measured_hz(&out[0][4800..]), 2.0 * Oscillator::C4_HZ as f64);
        assert!(c.abs() < 0.1, "{c:+.3} cents");
    }

    #[test]
    fn test_unison_detunes_and_spreads() {
        let mut params = with(defaults(), Oscillator::PARAM_WAVEFORM, 1.0);
        params[Oscillator::PARAM_VOICES] = 7.0;
        params[Oscillator::PARAM_DETUNE] = 0.5;
        params[Oscillator::PARAM_SPREAD] = 1.0;
        let out = render(&params, 48000);
        let (mono, left, right) = (&out[0][4800..], &out[2][4800..], &out[3][4800..]);

        // Seven voices spread over ±25 cents smear each harmonic
        let spectrum = Spectrum::of(mono, SR);
        let harmonic_bin = (8.0 * Oscillator::C4_HZ as f64 / spectrum.bin_hz).round() as usize;
        let near = spectrum.magnitudes[harmonic_bin - 2..=harmonic_bin + 2].iter().map(|m| m * m).sum::<f64>();
        let wide = spectrum.magnitudes[harmonic_bin - 40..=harmonic_bin + 40].iter().map(|m| m * m).sum::<f64>();
        assert!(near < 0.5 * wide, "8th harmonic should be smeared by the detune");

        // Left and right differ, and neither is louder than mono
        let diff: Vec<f32> = left.iter().zip(right).map(|(l, r)| l - r).collect();
        assert!(rms(&diff) > 0.1, "spread should decorrelate left and right");
        assert!(rms(left) <= rms(mono) * 1.01 && rms(right) <= rms(mono) * 1.01);
        // Loudness stays near a single voice's
        let single = render(&with(defaults(), Oscillator::PARAM_WAVEFORM, 1.0), 48000);
        let ratio = rms(mono) / rms(&single[0][4800..]);
        assert!((0.8..1.25).contains(&ratio), "unison level ratio {ratio}");
    }

    #[test]
    fn test_unison_with_no_spread_is_centred() {
        let mut params = with(defaults(), Oscillator::PARAM_VOICES, 5.0);
        params[Oscillator::PARAM_SPREAD] = 0.0;
        let out = render(&params, 4800);
        assert_eq!(out[2], out[3]);
        assert_eq!(out[0], out[2]);
    }

    #[test]
    fn test_reset_restarts_a_single_voice_at_zero() {
        let mut osc = Oscillator::new();
        osc.prepare(SR, 256);
        run(&mut osc, &defaults(), &[], 1000);
        osc.reset();
        let a = run(&mut osc, &defaults(), &[], 512);
        osc.reset();
        let b = run(&mut osc, &defaults(), &[], 512);
        assert_eq!(a, b, "reset should make the output repeatable");
    }

    #[test]
    fn test_pwm_changes_duty_cycle() {
        let mut params = with(defaults(), Oscillator::PARAM_WAVEFORM, 2.0);
        params[Oscillator::PARAM_PULSE_WIDTH] = 0.2;
        let narrow = render(&params, 9600);
        params[Oscillator::PARAM_PULSE_WIDTH] = 0.8;
        let wide = render(&params, 9600);
        let highs = |s: &[f32]| s.iter().filter(|&&x| x > 0.5).count();
        assert!(highs(&wide[0]) > 3 * highs(&narrow[0]));
    }

    #[test]
    fn test_crossing_rules() {
        // Forward across the wrap
        assert_eq!(crossing(0.9, 1.1, 0.0), Some(1.0));
        // Leaving "just before the wrap" forwards crosses it immediately
        assert_eq!(crossing(1.0, 1.05, 0.0), Some(1.0));
        // Leaving "just after the wrap" backwards crosses it immediately
        assert_eq!(crossing(0.0, -0.05, 0.0), Some(0.0));
        // ...but not the other way
        assert_eq!(crossing(0.0, 0.05, 0.0), None);
        assert_eq!(crossing(1.0, 0.95, 0.0), None);
        // Interior breakpoints
        assert_eq!(crossing(0.4, 0.6, 0.5), Some(0.5));
        assert_eq!(crossing(0.6, 0.4, 0.5), Some(0.5));
        assert_eq!(crossing(0.1, 0.3, 0.5), None);
    }

    #[test]
    fn test_registry_instantiation() {
        use crate::dsp::ModuleRegistry;
        let mut registry = ModuleRegistry::new();
        registry.register::<Oscillator>();
        let module = registry.create("osc.sine").unwrap();
        assert_eq!(module.info().name, "Oscillator");
        assert_eq!(module.ports().len(), 9);
        assert_eq!(module.parameters().len(), 10);
    }

    #[test]
    fn test_oscillator_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Oscillator>();
    }

    /// Runs `osc` for `len` samples, keeping every trace it captured.
    fn traces(osc: &mut Oscillator, params: &[f32], inputs: &[SignalBuffer], len: usize) -> Vec<Vec<f32>> {
        let mut traces = Vec::new();
        let mut done = 0;
        while done < len {
            let block = 64.min(len - done);
            let slices: Vec<SignalBuffer> = inputs
                .iter()
                .map(|buf| {
                    let mut b = SignalBuffer::control(block);
                    b.samples.copy_from_slice(&buf.samples[done..done + block]);
                    b
                })
                .collect();
            let refs: Vec<&SignalBuffer> = slices.iter().collect();
            osc.process(&refs, &mut outputs(block), params, &ProcessContext::new(SR, block));
            if let Some((points, _, _)) = osc.take_scope_data() {
                traces.push(points.to_vec());
            }
            done += block;
        }
        traces
    }

    fn fresh() -> Oscillator {
        let mut osc = Oscillator::new();
        osc.prepare(SR, 64);
        osc.reset();
        osc
    }

    /// Where a trace's first `cycles` cycles put a sine's start, as a share
    /// of a cycle late (negative: early).
    fn sine_lag(trace: &[f32], cycles: f32) -> f32 {
        let (mut sin, mut cos) = (0.0, 0.0);
        for (j, v) in trace.iter().enumerate() {
            let angle = std::f32::consts::TAU * cycles * j as f32 / trace.len() as f32;
            sin += v * angle.sin();
            cos += v * angle.cos();
        }
        -cos.atan2(sin) / std::f32::consts::TAU
    }

    #[test]
    fn the_trace_lines_up_with_the_shape() {
        // Two octaves down a sine is slow enough for the downsampler's
        // delay to be flat; the trace should start on its rising zero
        let params = with(with(defaults(), Oscillator::PARAM_OCTAVE, -2.0), Oscillator::PARAM_WAVEFORM, 0.0);
        let traces = traces(&mut fresh(), &params, &[], SR as usize);
        assert!(traces.len() >= 12, "{} traces in a second", traces.len());
        for trace in &traces[1..] {
            let lag = sine_lag(trace, TRACE_CYCLES);
            assert!(lag.abs() < 0.0005, "starts {lag} of a cycle out");
        }
    }

    #[test]
    fn a_square_trace_rises_at_its_start_and_falls_at_the_pulse_width() {
        let params = with(with(defaults(), Oscillator::PARAM_WAVEFORM, 2.0), Oscillator::PARAM_PULSE_WIDTH, 0.3);
        let traces = traces(&mut fresh(), &params, &[], SR as usize / 4);
        let trace = traces.last().unwrap();
        let per_cycle = TRACE_POINTS as f32 / TRACE_CYCLES;
        let falls: Vec<f32> = trace.windows(2).enumerate().filter(|(_, w)| w[0] > 0.0 && w[1] <= 0.0).map(|(j, _)| j as f32 / per_cycle).collect();
        assert_eq!(falls.len(), 2, "{falls:?}");
        assert!((falls[0] - 0.3).abs() < 0.02 && (falls[1] - 1.3).abs() < 0.02, "{falls:?}");
        assert!(trace[per_cycle as usize / 10] > 0.9, "high from the start");
    }

    #[test]
    fn a_note_up_an_octave_fits_twice_the_cycles() {
        let params = with(with(defaults(), Oscillator::PARAM_OCTAVE, -1.0), Oscillator::PARAM_WAVEFORM, 0.0);
        let traces = traces(&mut fresh(), &params, &patched(Oscillator::PORT_V_OCT, vec![1.0; SR as usize / 4]), SR as usize / 4);
        let trace = traces.last().unwrap();
        let rises = trace[2..].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert_eq!(rises, 2 * TRACE_CYCLES as usize - 1, "rising zeros after the start");
        assert!(sine_lag(trace, 2.0 * TRACE_CYCLES).abs() < 0.01);
    }

    #[test]
    fn traces_follow_the_sync_master() {
        // A master a fifth below resets the slave: each trace starts on a reset
        let len = SR as usize / 2;
        let master_hz = Oscillator::C4_HZ / 1.5;
        let master: Vec<f32> = (0..len).map(|n| (std::f32::consts::TAU * master_hz * n as f32 / SR).sin()).collect();
        let params = with(defaults(), Oscillator::PARAM_WAVEFORM, 1.0);
        let traces = traces(&mut fresh(), &params, &patched(Oscillator::PORT_SYNC, master), len);
        assert!(traces.len() >= 10);
        for trace in &traces[1..] {
            // A saw restarted from the bottom: it begins low, every time
            assert!(trace[2] < -0.7, "starts at {}", trace[2]);
        }
    }

    #[test]
    fn traces_come_a_few_dozen_times_a_second_and_wait_to_be_taken() {
        let mut osc = fresh();
        let params = defaults();
        // Each waits for the next pair of cycles to start
        let count = traces(&mut osc, &params, &[], SR as usize).len();
        assert!((24..=30).contains(&count), "{count} a second");
        // Untaken, a finished trace is kept rather than written over
        run(&mut osc, &params, &[], SR as usize / 10);
        let kept = osc.take_scope_data().unwrap().0.to_vec();
        assert!(osc.take_scope_data().is_none());
        assert!(sine_lag(&kept, TRACE_CYCLES).abs() < 0.01);
    }

    #[test]
    fn the_readout_is_the_pitch_played() {
        let mut osc = fresh();
        run(&mut osc, &defaults(), &patched(Oscillator::PORT_V_OCT, vec![-1.0; 512]), 512);
        let readout = osc.readout(&defaults()).unwrap();
        assert!((readout.values[Oscillator::READOUT_HZ] - Oscillator::C4_HZ / 2.0).abs() < 0.01);
        assert!((readout.values[Oscillator::READOUT_PULSE_WIDTH] - 0.5).abs() < 1e-4);
    }
}
