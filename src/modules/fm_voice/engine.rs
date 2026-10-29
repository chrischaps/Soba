//! The FM Voice's own six operators (#472): the DX7 voice it holds, played
//! in Soba, so the garden sounds in the browser, on a train, or for anyone
//! without a volca. Close enough to breed by ear; the keepers still go to
//! the hardware.
//!
//! It follows the DX7 as music-synthesizer-for-android (MSFA, Apache 2.0,
//! the engine Dexed plays) documents it, implemented here afresh:
//!
//! - **Levels** run on the DX7's log scale, in steps of 1/256 octave (a
//!   "u", about 0.023 dB). An output level is 0.75 dB a step above 19, on
//!   MSFA's table below. An envelope's level adds to its operator's output
//!   level, so an operator's loudness is one number, and its gain is
//!   2^(u / 256 − 14): 2 at the top, a modulation index of 4π.
//! - **Envelopes** move in that log domain: falling at a steady rate,
//!   rising from a jump to 1716 u in steps that shrink as it nears the top,
//!   which is the DX7's quick-then-easing attack. Rates 0 to 99 become
//!   MSFA's 0 to 63, faster up the keyboard by rate scaling.
//! - **Keyboard level scaling** works in groups of three semitones from
//!   the break point, along MSFA's exponential curve or a straight line.
//!   **Velocity** follows MSFA's measured curve, neutral near 100.
//! - **Feedback** feeds an operator the average of its last two outputs,
//!   2^(feedback − 8) of it, as the DX7 does to keep it from squealing. In
//!   algorithms 4 and 6 it runs from operator 4 back to 6, as the DX7 draws.
//! - **The LFO** runs at MSFA's measured speeds (about 0.06 Hz at 0, 5.8
//!   at 35, 24 at 99), after its delay, and is shared by every note as the
//!   DX7's is.
//!
//! What's Soba's own guess, worth comparing with a volca: the detune's
//! size (about a cent a step mid-keyboard, more low down), the pitch
//! envelope's range and speeds, the LFO's depths, and the delay's ramp.
//!
//! Everything is worked out when a voice arrives ([`Patch::of`]), by value,
//! and every note is set aside up front: nothing here allocates.

use crate::hardware::dx7::{self, global, op, Frequency, Voice, OPERATORS};

/// Samples between steps of the envelopes, the pitch envelope and the
/// LFO. Each operator's gain glides across a step, as MSFA's does across
/// its blocks.
pub const TICK: usize = 32;

/// The rate the DX7's envelope speeds are given at.
const MEASURED_AT: f32 = 44_100.0;

/// V/Oct 0, MIDI note 60.
const C4_HZ: f32 = 261.625_58;

/// Where a rising envelope jumps to first (u).
const JUMP: f32 = 1716.0;
/// The lowest an envelope's target goes (u).
const FLOOR: f32 = 16.0;
/// Below this an operator is silent (u): its gain is taken as nothing.
const SILENT: f32 = 64.0;
/// Below this a released carrier no longer counts as sounding (u), about
/// 80 dB down.
const QUIET: f32 = 256.0;

/// How far the LFO's amplitude modulation goes at its deepest (u).
const AMP_MOD_DEPTH: f32 = 1792.0;
/// How far the LFO bends the pitch at its deepest, in octaves.
const PITCH_MOD_DEPTH: f32 = 1.0;

/// MSFA's LFO speeds: Hz per unit of its rate.
const LFO_UNIT_HZ: f32 = 25_190_424.0 / 4_294_967_296.0;

/// An output level, 0 to 99, on the DX7's log scale (MSFA's
/// `scaleoutlevel`): 0.75 dB a step from 20 up, closer below.
fn scale_level(level: u8) -> i32 {
    const LOW: [u8; 20] = [0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46];
    match level.min(99) {
        level @ 20.. => 28 + level as i32,
        level => LOW[level as usize] as i32,
    }
}

/// How far keyboard level scaling moves an operator `group` groups of
/// three semitones from its break point, `depth` 0 to 99 along `curve`
/// (0 −LIN, 1 −EXP, 2 +EXP, 3 +LIN), in output-level steps. The
/// exponential curve is MSFA's, measured from a DX7.
fn scale_curve(group: i32, depth: u8, curve: u8) -> i32 {
    const EXP: [u8; 33] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 14, 16, 19, 23, 27, 33, 39, 47, 56, 66, 80, 94, 110, 126, 142, 158, 174, 190, 206, 222, 238, 250];
    let depth = depth.min(99) as i32;
    let scale = match curve {
        0 | 3 => (group * depth * 329) >> 12,
        _ => (EXP[(group.max(0) as usize).min(EXP.len() - 1)] as i32 * depth * 329) >> 15,
    };
    if curve < 2 {
        -scale
    } else {
        scale
    }
}

/// Keyboard level scaling at MIDI note `note` (MSFA's `ScaleLevel`), in
/// output-level steps.
fn keyboard_level(voice: &Voice, n: usize, note: i32) -> i32 {
    let offset = note - voice.op(n, op::BREAK_POINT) as i32 - 17;
    if offset >= 0 {
        scale_curve((offset + 1) / 3, voice.op(n, op::RIGHT_DEPTH), voice.op(n, op::RIGHT_CURVE))
    } else {
        scale_curve(-(offset - 1) / 3, voice.op(n, op::LEFT_DEPTH), voice.op(n, op::LEFT_CURVE))
    }
}

/// How far velocity `velocity` (0 to 127) moves an operator of sensitivity
/// `sensitivity` (0 to 7), in u: MSFA's curve, measured from a DX7, which
/// is about neutral at 100 and lifts a little above.
fn velocity_level(velocity: u8, sensitivity: u8) -> i32 {
    const CURVE: [u8; 64] = [
        0, 70, 86, 97, 106, 114, 121, 126, 132, 138, 142, 148, 152, 156, 160, 163, 166, 170, 173, 174, 178, 181, 184, 186, 189, 190, 194, 196, 198, 200, 202, 205, 206, 209, 211, 214, 216, 218, 220, 222, 224, 225,
        227, 229, 230, 232, 233, 235, 237, 238, 240, 241, 242, 243, 244, 246, 246, 248, 249, 250, 251, 252, 253, 254,
    ];
    let value = CURVE[(velocity.min(127) >> 1) as usize] as i32 - 239;
    ((sensitivity.min(7) as i32 * value + 7) >> 3) << 4
}

/// How much faster an operator's envelope runs at MIDI note `note`, in
/// MSFA's rate steps (`ScaleRate`).
fn rate_scaling(note: i32, sensitivity: u8) -> i32 {
    let x = (note / 3 - 7).clamp(0, 31);
    (sensitivity.min(7) as i32 * x) >> 3
}

/// A pitch envelope level, 0 to 99 with 50 the note itself, in octaves:
/// a 32nd of an octave a step near the middle, reaching four octaves at
/// the ends.
fn pitch_level(level: u8) -> f32 {
    let d = level.min(99) as f32 - 50.0;
    let a = d.abs();
    let steps = if a <= 35.0 { a } else { 35.0 + 93.0 * ((a - 35.0) / 15.0).powf(1.8) };
    steps.copysign(d) / 32.0
}

/// A pitch envelope rate, 0 to 99, in octaves a second.
fn pitch_speed(rate: u8) -> f32 {
    let r = rate.min(99) as f32;
    let steps = if r < 25.0 { 1.0 + r * 0.52 } else { 14.0 * ((r - 25.0) / 17.8).exp2() };
    steps / 21.3
}

/// The LFO's speed, 0 to 99, in Hz (MSFA's fit).
fn lfo_hz(speed: u8) -> f32 {
    let rate = match speed.min(99) as u32 {
        0 => 1,
        s => (165 * s) >> 6,
    };
    let rate = rate * if rate < 160 { 11 } else { 11 + ((rate - 160) >> 4) };
    LFO_UNIT_HZ * rate as f32
}

/// How long the LFO waits after a note starts, and then how long it takes
/// to fade in, in seconds, for its delay 0 to 99.
fn lfo_delay(delay: u8) -> (f32, f32) {
    let a = 99 - delay.min(99) as u32;
    if a == 99 {
        return (0.0, 0.0);
    }
    let wait = (16 + (a & 15)) << (1 + (a >> 4));
    let fade = (wait & 0xff80).max(0x80);
    (1.0 / (LFO_UNIT_HZ * wait as f32), 1.0 / (LFO_UNIT_HZ * fade as f32))
}

/// How many cents one step of detune moves an operator sounding at `hz`:
/// about 0.9 at middle C, so the whole ±7 spans a few cents either way, as
/// Yamaha's DX21 manual gives it (±2 cents), growing down the keyboard and
/// shrinking up it, as DX7 emulations measure.
fn detune_cents(hz: f32) -> f32 {
    (0.5 * (2000.0 / hz.max(1.0)).powf(0.3)).clamp(0.25, 2.5)
}

/// A sine of `phase` (a whole turn is 2^32): an odd polynomial, folded
/// into a quarter turn, good to a few parts in a million.
#[inline(always)]
fn sine(phase: u32) -> f32 {
    let x = (phase as i32) as f32 * (1.0 / 4_294_967_296.0);
    let x = (0.25 - (0.25 - x.abs()).abs()).copysign(x);
    let x2 = x * x;
    x * (6.283_185_5 + x2 * (-41.341_7 + x2 * (81.605_25 + x2 * (-76.705_86 + x2 * 42.058_693))))
}

/// One operator's part in an algorithm, in the order they're computed.
#[derive(Clone, Copy, Debug, Default)]
struct Step {
    /// The operator, from 0 (operator 1).
    op: usize,
    /// The bus it reads: 1 or 2, or 3 for none.
    input: usize,
    /// The bus it writes: 0 is what's heard.
    output: usize,
    /// Adds to the bus rather than starting it.
    add: bool,
    /// Hears the feedback.
    fb_in: bool,
    /// Its outputs are what feeds back.
    fb_out: bool,
}

/// One operator's settings.
#[derive(Clone, Copy, Debug, Default)]
struct OpPatch {
    rates: [u8; 4],
    levels: [u8; 4],
    /// Its output level on the log scale, before scaling.
    output: i32,
    /// A ratio of the note, or Hz when fixed.
    frequency: f32,
    fixed: bool,
    /// −7 to 7.
    detune: f32,
    rate_scaling: u8,
    velocity: u8,
    /// How far the LFO moves its level, 0 to 1.
    amp_mod: f32,
}

/// A voice, worked out for playing.
#[derive(Clone, Copy, Debug)]
pub struct Patch {
    voice: Voice,
    ops: [OpPatch; OPERATORS],
    program: [Step; OPERATORS],
    /// Which operators are heard.
    carriers: [bool; OPERATORS],
    /// What the average of the feedback's last two outputs is scaled by.
    feedback: f32,
    /// Semitones.
    transpose: f32,
    pitch_rates: [f32; 4],
    pitch_levels: [f32; 4],
    /// Whether the pitch envelope moves at all.
    pitch_moves: bool,
    lfo_hz: f32,
    lfo_wave: u8,
    lfo_sync: bool,
    lfo_wait: f32,
    lfo_fade: f32,
    /// Octaves at the deepest.
    pitch_mod: f32,
    /// 0 to 1.
    amp_mod: f32,
    osc_sync: bool,
}

impl Default for Patch {
    fn default() -> Self {
        Self::of(&Voice::init())
    }
}

impl Patch {
    /// `voice`, worked out.
    pub fn of(voice: &Voice) -> Self {
        let voice = voice.clamped();
        let ops = std::array::from_fn(|k| {
            let n = k + 1;
            let (frequency, fixed) = match voice.frequency(n) {
                Frequency::Ratio(r) => (r, false),
                Frequency::Fixed(hz) => (hz, true),
            };
            let at = Voice::op_at(n);
            OpPatch {
                rates: voice.0[at + op::RATES..at + op::RATES + 4].try_into().unwrap_or([99; 4]),
                levels: voice.0[at + op::LEVELS..at + op::LEVELS + 4].try_into().unwrap_or([99, 99, 99, 0]),
                output: scale_level(voice.op(n, op::LEVEL)),
                frequency,
                fixed,
                detune: voice.op(n, op::DETUNE) as f32 - 7.0,
                rate_scaling: voice.op(n, op::RATE_SCALING),
                velocity: voice.op(n, op::VELOCITY),
                amp_mod: [0.0, 66.0, 109.0, 255.0][voice.op(n, op::AMP_MOD).min(3) as usize] / 255.0,
            }
        });
        let buses = dx7::buses(voice.algorithm());
        // The feedback comes from the last operator computed that gives it
        // (operator 4 in algorithm 4, 5 in 6), as the DX7 draws them
        let fb_from = buses.iter().rposition(|&flags| flags & dx7::FB_OUT != 0);
        let program = std::array::from_fn(|k| {
            let flags = buses[k];
            let output = match flags & (dx7::OUT_BUS_ONE | dx7::OUT_BUS_TWO) {
                dx7::OUT_BUS_ONE => 1,
                dx7::OUT_BUS_TWO => 2,
                _ => 0,
            };
            Step {
                op: OPERATORS - 1 - k,
                input: match flags & (dx7::IN_BUS_ONE | dx7::IN_BUS_TWO) {
                    dx7::IN_BUS_ONE => 1,
                    dx7::IN_BUS_TWO => 2,
                    _ => 3,
                },
                output,
                add: output == 0 || flags & dx7::OUT_BUS_ADD != 0,
                fb_in: flags & dx7::FB_IN != 0,
                fb_out: fb_from == Some(k),
            }
        });
        let feedback = match voice.feedback() {
            0 => 0.0,
            fb => 0.5 * (fb as f32 - 8.0).exp2(),
        };
        let byte = |at: usize| voice.0[at];
        let pitch_levels: [f32; 4] = std::array::from_fn(|k| pitch_level(byte(global::PITCH_LEVELS + k)));
        let pms = [0.0, 10.0, 20.0, 33.0, 55.0, 92.0, 153.0, 255.0][byte(global::PITCH_MOD).min(7) as usize] / 255.0;
        let (lfo_wait, lfo_fade) = lfo_delay(byte(global::LFO_DELAY));
        Self {
            voice,
            ops,
            program,
            carriers: voice.carriers(),
            feedback,
            transpose: byte(global::TRANSPOSE) as f32 - 24.0,
            pitch_rates: std::array::from_fn(|k| pitch_speed(byte(global::PITCH_RATES + k))),
            pitch_levels,
            pitch_moves: pitch_levels.iter().any(|&l| l != 0.0),
            lfo_hz: lfo_hz(byte(global::LFO_SPEED)),
            lfo_wave: byte(global::LFO_WAVE),
            lfo_sync: byte(global::LFO_SYNC) != 0,
            lfo_wait,
            lfo_fade,
            pitch_mod: PITCH_MOD_DEPTH * pms * byte(global::LFO_PITCH_DEPTH) as f32 / 99.0,
            amp_mod: byte(global::LFO_AMP_DEPTH) as f32 / 99.0,
            osc_sync: byte(global::OSC_SYNC) != 0,
        }
    }

    /// The voice it was worked out from.
    pub fn voice(&self) -> &Voice {
        &self.voice
    }
}

/// An operator's envelope, in u (see the module's notes).
#[derive(Clone, Copy, Debug, Default)]
struct Env {
    level: f32,
    target: f32,
    /// u a sample, at [`MEASURED_AT`].
    rate: f32,
    /// 0 to 3, or 4 once released and done.
    stage: usize,
    rising: bool,
    down: bool,
}

impl Env {
    /// Moves on to stage `stage`: its level, and how fast it goes there.
    fn enter(&mut self, stage: usize, op: &OpPatch, output: f32, scaling: i32) {
        self.stage = stage;
        if stage < 4 {
            let level = ((scale_level(op.levels[stage]) >> 1) << 6) as f32;
            self.target = (level + output - 4256.0).max(FLOOR);
            self.rising = self.target > self.level;
            let q = (((op.rates[stage] as i32 * 41) >> 6) + scaling).min(63);
            self.rate = ((4 + (q & 3)) << (2 + (q >> 2))) as f32 / 65_536.0;
        }
    }

    fn key(&mut self, down: bool, op: &OpPatch, output: f32, scaling: i32) {
        if self.down != down {
            self.down = down;
            self.enter(if down { 0 } else { 3 }, op, output, scaling);
        }
    }

    /// Runs `samples` samples' worth (at [`MEASURED_AT`]).
    fn run(&mut self, samples: f32, op: &OpPatch, output: f32, scaling: i32) {
        if self.stage < 3 || (self.stage == 3 && !self.down) {
            if self.rising {
                self.level = self.level.max(JUMP);
                // Steps that shrink as it nears the top, a whole number of
                // octaves short of 17
                let lift = ((4352.0 - self.level) / 256.0) as i32 as f32;
                self.level += lift * self.rate * samples;
                if self.level >= self.target {
                    self.level = self.target;
                    self.enter(self.stage + 1, op, output, scaling);
                }
            } else {
                self.level -= self.rate * samples;
                if self.level <= self.target {
                    self.level = self.target;
                    self.enter(self.stage + 1, op, output, scaling);
                }
            }
        }
    }
}

/// The pitch envelope, in octaves.
#[derive(Clone, Copy, Debug, Default)]
struct PitchEnv {
    level: f32,
    stage: usize,
    down: bool,
}

impl PitchEnv {
    fn run(&mut self, seconds: f32, patch: &Patch) {
        if self.stage < 3 || (self.stage == 3 && !self.down) {
            let target = patch.pitch_levels[self.stage];
            let step = patch.pitch_rates[self.stage] * seconds;
            if (target - self.level).abs() <= step {
                self.level = target;
                self.stage += 1;
            } else {
                self.level += step.copysign(target - self.level);
            }
        }
    }
}

/// One note being played, on one channel.
#[derive(Clone, Copy, Debug, Default)]
struct Note {
    /// It's sounding, or might: when not, it's skipped.
    active: bool,
    gate: bool,
    /// MIDI note and velocity (0 to 127) as it was struck, transposed.
    note: i32,
    velocity: u8,
    phase: [u32; OPERATORS],
    inc: [u32; OPERATORS],
    env: [Env; OPERATORS],
    /// Each operator's output level with its scaling (u), and its rate
    /// scaling, for the note struck.
    output: [f32; OPERATORS],
    scaling: [i32; OPERATORS],
    /// Each operator's detune, as a ratio of its frequency.
    detune: [f32; OPERATORS],
    gain: [f32; OPERATORS],
    slope: [f32; OPERATORS],
    /// The feedback's last two outputs.
    fb: [f32; 2],
    pitch: PitchEnv,
    /// The LFO's delay: seconds since the note started.
    since: f32,
    /// Samples left in this tick.
    left: usize,
}

impl Note {
    /// Works out what the note struck makes of each operator.
    fn tune(&mut self, patch: &Patch) {
        for (k, o) in patch.ops.iter().enumerate() {
            let n = k + 1;
            let level = (o.output + keyboard_level(&patch.voice, n, self.note)).min(127) << 5;
            self.output[k] = (level + velocity_level(self.velocity, o.velocity)).max(0) as f32;
            self.scaling[k] = rate_scaling(self.note, o.rate_scaling);
            let hz = if o.fixed { o.frequency } else { o.frequency * C4_HZ * ((self.note - 60) as f32 / 12.0).exp2() };
            self.detune[k] = (o.detune * detune_cents(hz) / 1200.0).exp2();
        }
    }

    fn strike(&mut self, note: i32, velocity: u8, patch: &Patch) {
        if !self.active {
            // A note from silence starts its pitch where the envelope ends
            self.pitch.level = patch.pitch_levels[3];
        }
        self.active = true;
        self.gate = true;
        self.note = note;
        self.velocity = velocity;
        self.since = 0.0;
        self.tune(patch);
        if patch.osc_sync {
            self.phase = [0; OPERATORS];
            self.fb = [0.0; 2];
        }
        for (k, env) in self.env.iter_mut().enumerate() {
            env.key(true, &patch.ops[k], self.output[k], self.scaling[k]);
        }
        self.pitch.down = true;
        self.pitch.stage = 0;
        self.left = 0;
    }

    fn release(&mut self, patch: &Patch) {
        self.gate = false;
        for (k, env) in self.env.iter_mut().enumerate() {
            env.key(false, &patch.ops[k], self.output[k], self.scaling[k]);
        }
        self.pitch.down = false;
        self.pitch.stage = 3;
        self.left = 0;
    }

    /// Takes up a new voice mid-note: its levels, ratios and envelopes,
    /// from where each envelope is. A held note glides to the new
    /// sustain, a released one to the new end.
    fn retune(&mut self, patch: &Patch) {
        self.tune(patch);
        for (k, env) in self.env.iter_mut().enumerate() {
            let stage = if env.down { env.stage.min(2) } else { 3 };
            env.enter(stage, &patch.ops[k], self.output[k], self.scaling[k]);
        }
        self.left = 0;
    }

    /// Whether every carrier has faded away, released.
    fn faded(&self, patch: &Patch) -> bool {
        !self.gate && (0..OPERATORS).all(|k| !patch.carriers[k] || (self.env[k].level < QUIET && (self.env[k].stage == 4 || !self.env[k].rising)))
    }
}

/// The LFO every note shares.
#[derive(Clone, Copy, Debug, Default)]
struct Lfo {
    /// 0 to 1.
    phase: f32,
    held: f32,
    random: u32,
}

impl Lfo {
    /// Its value, −1 to 1.
    fn value(&self, wave: u8) -> f32 {
        let p = self.phase;
        match wave {
            0 => 1.0 - 4.0 * (p - 0.5).abs(),
            1 => 1.0 - 2.0 * p,
            2 => 2.0 * p - 1.0,
            3 => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            4 => sine((p * 4_294_967_296.0) as u64 as u32),
            _ => self.held,
        }
    }

    fn run(&mut self, seconds: f32, hz: f32) {
        self.phase += hz * seconds;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.random ^= self.random << 13;
            self.random ^= self.random >> 17;
            self.random ^= self.random << 5;
            self.held = (self.random >> 8) as f32 / (1 << 23) as f32 - 1.0;
        }
    }
}

/// The notes, one per channel, and the LFO.
pub struct Engine {
    patch: Patch,
    notes: [Note; crate::dsp::MAX_CHANNELS],
    lfo: Lfo,
    sample_rate: f32,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// What one channel hears in a block.
pub struct Played<'a> {
    /// V/Oct.
    pub pitch: &'a [f32],
    pub gate: &'a [f32],
    /// 0 to 1, at the moment a note strikes.
    pub velocity: &'a [f32],
}

impl Engine {
    pub fn new() -> Self {
        Self { patch: Patch::default(), notes: [Note::default(); crate::dsp::MAX_CHANNELS], lfo: Lfo { random: 0x9E37_79B9, ..Lfo::default() }, sample_rate: 48_000.0 }
    }

    pub fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    /// Plays `voice` from now on, in the notes sounding too.
    ///
    /// REAL-TIME SAFE: the voice comes by value.
    pub fn set_voice(&mut self, voice: &Voice) {
        if voice == self.patch.voice() {
            return;
        }
        self.patch = Patch::of(voice);
        for note in self.notes.iter_mut().filter(|n| n.active) {
            note.retune(&self.patch);
        }
    }

    pub fn voice(&self) -> &Voice {
        self.patch.voice()
    }

    /// Silences every note.
    pub fn reset(&mut self) {
        self.notes = [Note::default(); crate::dsp::MAX_CHANNELS];
        self.lfo.phase = 0.0;
    }

    /// Stops the notes from channel `from` up: their cable lost them.
    pub fn drop_from(&mut self, from: usize) {
        for note in self.notes.iter_mut().skip(from) {
            *note = Note::default();
        }
    }

    /// Whether channel `channel`'s note might sound.
    pub fn sounding(&self, channel: usize) -> bool {
        self.notes.get(channel).is_some_and(|n| n.active)
    }

    /// Moves the LFO on by `samples`, once a block for every note. Call
    /// before [`play`](Self::play).
    pub fn run_lfo(&mut self, samples: usize) {
        self.lfo.run(samples as f32 / self.sample_rate, self.patch.lfo_hz);
    }

    /// Plays channel `channel` into `out`, which it fills.
    pub fn play(&mut self, channel: usize, played: &Played, out: &mut [f32]) {
        let Some(note) = self.notes.get_mut(channel) else {
            out.fill(0.0);
            return;
        };
        let patch = &self.patch;
        let lfo = self.lfo.value(patch.lfo_wave);
        let len = out.len();
        let mut i = 0;
        while i < len {
            // A gate that changes ends the run there
            let high = played.gate.get(i).copied().unwrap_or(0.0) > crate::dsp::gate::GATE_THRESHOLD;
            if high != note.gate {
                if high {
                    let voct = played.pitch.get(i).copied().unwrap_or(0.0);
                    let midi = (60.0 + voct * 12.0 + patch.transpose).round() as i32;
                    let velocity = (played.velocity.get(i).copied().unwrap_or(0.8).clamp(0.0, 1.0) * 127.0).round() as u8;
                    note.strike(midi, velocity, patch);
                    if patch.lfo_sync {
                        self.lfo.phase = 0.0;
                    }
                } else {
                    note.release(patch);
                }
            }
            if !note.active {
                let end = played.gate.get(i..len).and_then(|g| g.iter().position(|&g| g > crate::dsp::gate::GATE_THRESHOLD)).map_or(len, |k| i + k);
                out[i..end].fill(0.0);
                i = end;
                continue;
            }
            if note.left == 0 {
                Self::tick(note, patch, lfo, played.pitch.get(i).copied().unwrap_or(0.0), self.sample_rate);
                if note.faded(patch) {
                    note.active = false;
                    continue;
                }
            }
            let mut end = i + note.left.min(len - i);
            if let Some(k) = played.gate.get(i + 1..end).and_then(|g| g.iter().position(|&g| (g > crate::dsp::gate::GATE_THRESHOLD) != note.gate)) {
                end = i + 1 + k;
            }
            Self::render(note, patch, &mut out[i..end]);
            note.left -= end - i;
            i = end;
        }
    }

    /// Steps a note's envelopes on by a tick, and works out where its
    /// gains and frequencies go over the next.
    fn tick(note: &mut Note, patch: &Patch, lfo: f32, voct: f32, sample_rate: f32) {
        let seconds = TICK as f32 / sample_rate;
        let measured = TICK as f32 * MEASURED_AT / sample_rate;
        for k in 0..OPERATORS {
            note.env[k].run(measured, &patch.ops[k], note.output[k], note.scaling[k]);
        }
        if patch.pitch_moves {
            note.pitch.run(seconds, patch);
        }
        // The LFO, faded in after its delay
        note.since += seconds;
        let fade = if note.since < patch.lfo_wait {
            0.0
        } else if patch.lfo_fade > 0.0 {
            ((note.since - patch.lfo_wait) / patch.lfo_fade).min(1.0)
        } else {
            1.0
        };
        let amp_mod = (1.0 + lfo) * 0.5 * fade * patch.amp_mod * AMP_MOD_DEPTH;
        let octaves = voct + patch.transpose / 12.0 + note.pitch.level + lfo * fade * patch.pitch_mod;
        let hz = C4_HZ * octaves.exp2();
        let per_sample = 4_294_967_296.0 / sample_rate as f64;
        let step = 1.0 / TICK as f32;
        for (k, o) in patch.ops.iter().enumerate() {
            let level = note.env[k].level - amp_mod * o.amp_mod;
            let target = if level < SILENT { 0.0 } else { (level / 256.0 - 14.0).exp2() };
            if target == 0.0 && note.gain[k].abs() < 1e-6 {
                // Silent, and skipped until it isn't
                note.gain[k] = 0.0;
            }
            note.slope[k] = (target - note.gain[k]) * step;
            let f = if o.fixed { o.frequency } else { o.frequency * hz } * note.detune[k];
            note.inc[k] = (f as f64 * per_sample) as u64 as u32;
        }
        note.left = TICK;
    }

    /// Plays a note's operators into `out`, sample by sample.
    #[inline]
    fn render(note: &mut Note, patch: &Patch, out: &mut [f32]) {
        let Note { phase, inc, gain, slope, fb, .. } = note;
        let feedback = patch.feedback;
        for sample in out.iter_mut() {
            let mut buses = [0.0f32; 4];
            for s in &patch.program {
                let k = s.op;
                let mut input = buses[s.input];
                if s.fb_in {
                    input += (fb[0] + fb[1]) * feedback;
                }
                let y = if gain[k] == 0.0 && slope[k] == 0.0 {
                    0.0
                } else {
                    sine(phase[k].wrapping_add((input * 4_294_967_296.0) as i64 as u32)) * gain[k]
                };
                gain[k] += slope[k];
                phase[k] = phase[k].wrapping_add(inc[k]);
                if s.fb_out {
                    fb[1] = fb[0];
                    fb[0] = y;
                }
                if s.add {
                    buses[s.output] += y;
                } else {
                    buses[s.output] = y;
                }
            }
            // A carrier at its loudest is 1
            *sample = buses[0] * 0.5;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::garden;

    const RATE: f32 = 48_000.0;

    /// Plays `voice` at MIDI `note` for `held` seconds and then `tail` more
    /// released, on one channel.
    fn play(voice: &Voice, note: f32, held: f32, tail: f32) -> Vec<f32> {
        let mut engine = Engine::new();
        engine.prepare(RATE);
        engine.set_voice(voice);
        let total = ((held + tail) * RATE) as usize;
        let on = (held * RATE) as usize;
        let mut out = vec![0.0; total];
        let block = 128;
        for start in (0..total).step_by(block) {
            let end = (start + block).min(total);
            let gate: Vec<f32> = (start..end).map(|i| if i < on { 1.0 } else { 0.0 }).collect();
            let pitch = vec![(note - 60.0) / 12.0; end - start];
            let velocity = vec![0.8; end - start];
            engine.run_lfo(end - start);
            engine.play(0, &Played { pitch: &pitch, gate: &gate, velocity: &velocity }, &mut out[start..end]);
        }
        out
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |m, s| m.max(s.abs()))
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    /// The frequency of a near-sine, from its upward zero crossings.
    fn hz(samples: &[f32]) -> f32 {
        let ups: Vec<f32> = samples.windows(2).enumerate().filter(|(_, w)| w[0] <= 0.0 && w[1] > 0.0).map(|(i, w)| i as f32 + w[0] / (w[0] - w[1])).collect();
        (ups.len() - 1) as f32 * RATE / (ups[ups.len() - 1] - ups[0])
    }

    #[test]
    fn the_sine_is_a_sine() {
        for k in 0..1000 {
            let turn = k as f64 / 1000.0;
            let want = (turn * std::f64::consts::TAU).sin() as f32;
            let got = sine((turn * 4_294_967_296.0) as u64 as u32);
            assert!((got - want).abs() < 1e-5, "{turn}: {got} for {want}");
        }
    }

    #[test]
    fn init_voice_is_a_sine_at_the_note() {
        let out = play(&Voice::init(), 69.0, 0.5, 0.0);
        let f = hz(&out[4800..]);
        assert!((f - 440.0).abs() < 0.2, "{f} Hz");
        let p = peak(&out[4800..]);
        assert!((p - 1.0).abs() < 0.02, "one carrier at 99, its envelope at 99: full, {p}");
    }

    #[test]
    fn levels_fall_on_the_dx7s_scale() {
        // Output level 99 to 79: twenty steps of 0.75 dB, 15 dB
        let mut quieter = Voice::init();
        quieter.set_op(1, op::LEVEL, 79);
        let (loud, soft) = (peak(&play(&Voice::init(), 60.0, 0.3, 0.0)[4800..]), peak(&play(&quieter, 60.0, 0.3, 0.0)[4800..]));
        let db = 20.0 * (soft / loud).log10();
        assert!((db + 15.0).abs() < 0.3, "{db} dB");
    }

    #[test]
    fn a_released_note_fades_and_stops() {
        let mut voice = Voice::init();
        voice.set_op(1, op::RATES + 3, 60);
        let out = play(&voice, 60.0, 0.2, 2.0);
        assert!(rms(&out[..9600]) > 0.5);
        assert!(peak(&out[out.len() - 4800..]) < 1e-3, "gone two seconds after");
        // Rate 99's release is a few milliseconds
        let out = play(&Voice::init(), 60.0, 0.2, 0.1);
        assert!(peak(&out[9600 + 960..]) < 1e-3, "rate 99 lets go within 20 ms");
    }

    #[test]
    fn faster_rates_decay_faster_and_rate_scaling_speeds_high_notes() {
        let decaying = |rate: u8, scaling: u8, note: f32| {
            let mut voice = Voice::init();
            voice.0[Voice::op_at(1) + op::LEVELS..Voice::op_at(1) + op::LEVELS + 4].copy_from_slice(&[99, 0, 0, 0]);
            voice.set_op(1, op::RATES + 1, rate);
            voice.set_op(1, op::RATE_SCALING, scaling);
            let out = play(&voice, note, 1.0, 0.0);
            rms(&out[24_000..28_800])
        };
        assert!(decaying(40, 0, 60.0) > decaying(60, 0, 60.0) * 3.0);
        assert!(decaying(40, 7, 96.0) < decaying(40, 0, 96.0) * 0.5, "rate scaling 7 shortens a high note");
    }

    #[test]
    fn a_modulator_brightens_and_feedback_roughens() {
        let harmonics = |voice: &Voice| {
            let out = play(voice, 57.0, 0.5, 0.0);
            // How much is left once the fundamental is taken out: a crude
            // brightness from the difference between neighbours
            let tail = &out[9600..];
            rms(&tail.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>()) / rms(tail)
        };
        let plain = harmonics(&Voice::init());
        let mut modulated = Voice::init();
        modulated.set_op(2, op::LEVEL, 80);
        let bright = harmonics(&modulated);
        assert!(bright > plain * 2.0, "{bright} against {plain}");
        let mut fed = Voice::init();
        fed.0[global::ALGORITHM] = 31;
        fed.set_op(1, op::LEVEL, 0);
        fed.set_op(6, op::LEVEL, 99);
        fed.0[global::FEEDBACK] = 7;
        assert!(harmonics(&fed) > plain * 3.0, "operator 6 feeding itself");
    }

    #[test]
    fn velocity_reaches_a_sensitive_operator() {
        let mut voice = Voice::init();
        voice.set_op(1, op::VELOCITY, 7);
        let at = |velocity: f32| {
            let mut engine = Engine::new();
            engine.prepare(RATE);
            engine.set_voice(&voice);
            let mut out = vec![0.0; 4800];
            engine.play(0, &Played { pitch: &[0.0; 4800], gate: &[1.0; 4800], velocity: &[velocity; 4800] }, &mut out);
            peak(&out[2400..])
        };
        assert!(at(0.3) < at(1.0) * 0.4);
        assert!((at(100.0 / 127.0) - 1.0).abs() < 0.05, "about neutral near 100");
    }

    #[test]
    fn a_voice_changed_mid_note_is_heard_at_once() {
        let mut engine = Engine::new();
        engine.prepare(RATE);
        engine.set_voice(&Voice::init());
        let mut out = vec![0.0; 4800];
        let played = Played { pitch: &[0.0; 4800], gate: &[1.0; 4800], velocity: &[0.8; 4800] };
        engine.play(0, &played, &mut out);
        let mut softer = Voice::init();
        softer.set_op(1, op::LEVEL, 70);
        engine.set_voice(&softer);
        engine.play(0, &played, &mut out);
        assert!(peak(&out[2400..]) < 0.3, "the held note took the new level: {}", peak(&out[2400..]));
    }

    #[test]
    fn keyboard_scaling_follows_its_curves() {
        assert_eq!(scale_curve(0, 99, 3), 0);
        assert!(scale_curve(10, 99, 0) < 0 && scale_curve(10, 99, 3) > 0);
        assert!(scale_curve(5, 99, 2) < scale_curve(5, 99, 3), "the exponential curve starts slower");
        assert!(scale_curve(30, 99, 2) > scale_curve(20, 99, 2));
        assert_eq!(scale_level(99), 127);
        assert_eq!(scale_level(19), 46);
    }

    #[test]
    fn the_lfo_runs_at_the_dx7s_speeds() {
        assert!((lfo_hz(0) - 0.0645).abs() < 0.01);
        assert!((lfo_hz(35) - 5.8).abs() < 0.1);
        assert!((lfo_hz(99) - 23.9).abs() < 0.3);
        assert_eq!(lfo_delay(0), (0.0, 0.0));
        assert!(lfo_delay(99).0 > lfo_delay(50).0);
    }

    #[test]
    fn every_algorithm_and_seed_plays_in_range() {
        for mut voice in garden::seeds() {
            for algorithm in 0..dx7::ALGORITHMS {
                voice.0[global::ALGORITHM] = algorithm as u8;
                voice.0[global::FEEDBACK] = 7;
                let out = play(&voice, 48.0, 0.3, 0.2);
                assert!(out.iter().all(|s| s.is_finite()), "{voice:?}");
                assert!(peak(&out) < 6.5, "six carriers at most: {voice:?} {}", peak(&out));
            }
        }
    }

    #[test]
    fn notes_skip_when_silent() {
        let mut engine = Engine::new();
        engine.prepare(RATE);
        let mut out = vec![1.0; 256];
        engine.play(3, &Played { pitch: &[0.0; 256], gate: &[0.0; 256], velocity: &[0.8; 256] }, &mut out);
        assert!(out.iter().all(|&s| s == 0.0));
        assert!(!engine.sounding(3));
    }
}
