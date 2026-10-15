//! Tape module: a tape machine for a bus.
//!
//! What a mix picks up on its way through a tape machine, after the
//! instruments: the record head's soft squash and the head bump, the wow and
//! flutter of the transport, the top end the tape can't hold, and, on an old
//! reel, dropouts and hiss.
//!
//! Each channel is recorded, then played back:
//!
//! 1. **Record.** Pre-emphasis (the speed's NAB, IEC or AES curve) lifts the
//!    treble, the drive pushes it into the Delay's record-head curve, and
//!    de-emphasis takes the treble back down, so the highs squash first, as
//!    they do on tape. Then the head bump of the speed, and the speed's top
//!    end. Saturation fades this in over the first tenth of its knob.
//! 2. **Transport.** A read head lagging the record head by the Delay's wow
//!    and flutter partials, as `A·(1 − cos)`: it never runs ahead, so the
//!    module adds no latency, and at no wobble it reads the record head
//!    exactly. The right track wobbles a little differently, by Width.
//! 3. **Playback.** Hiss, then the age of the tape: gap loss rolling off the
//!    top, and dropouts where the oxide has worn away.
//!
//! The **Stop** gate brakes the transport to a halt over the Stop Time, so the
//! whole bus falls in pitch to nothing, and winds it back up when let go (see
//! [`TapeStop`]). At rest it adds nothing, and no latency.
//!
//! With every knob at zero each stage passes its input through bit for bit.

use crate::dsp::{
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    context::ProcessContext,
    denormal::flush,
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    primitives::{prewarp, Adaa1, BiasedTanh, Curve, FracDelay, NoiseFloor, RecordHead, StopHeads, TapeStop, TapeTransport},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    connected_input, SignalType,
};

/// Cents in a pitch ratio of `1 + d`, for small `d`: 1200 / ln 2.
pub const CENTS_PER_DEVIATION: f32 = 1731.234;

/// Peak pitch swing at full Wow and full Flutter, at 15 ips.
pub const WOW_CENTS: f32 = 40.0;
pub const FLUTTER_CENTS: f32 = 12.0;

/// Longest the read head can lag, with room to spare: full wow at 7½ ips
/// pulls it back about 68 ms.
const MAX_LAG_SECONDS: f32 = 0.1;

/// The Stop Time's range, in seconds.
pub const MIN_STOP_SECONDS: f32 = 0.05;
pub const MAX_STOP_SECONDS: f32 = 4.0;

/// Room on the tape for the longest stop's lag, on top of the wobble's.
const STOP_ROOM_SECONDS: f32 = 2.5;

/// Glide on the wobble's depth. A deeper wobble lags the read head further,
/// which bends the pitch down while it happens, as a dragging reel does; this
/// keeps a knob grabbed from nothing to full to a short sag.
const WOBBLE_GLIDE_MS: f32 = 300.0;

/// How far pre-emphasis lifts the treble before the head: 1 + this, +9.5 dB.
const EMPHASIS: f32 = 2.0;

/// Drive at full Saturation.
const MAX_DRIVE_DB: f32 = 18.0;

/// 0 VU, the level the drive leaves where it was: a sine at -18 dBFS RMS.
const NOMINAL_PEAK: f32 = 0.177;

/// The head bump: a lift at the bump frequency, and less below it.
const BUMP_LIFT: f32 = 0.41; // +3 dB
const BUMP_CUT: f32 = 0.3;
const BUMP_DAMPING: f32 = 1.0; // 2R of the SVF: Q = 1

/// Tape can't record DC; this removes what the lopsided record head adds.
const DC_HZ: f32 = 10.0;

/// Hiss at full Hiss, and what full Age adds to it, RMS.
const HISS_RMS: f32 = 0.02; // -34 dBFS
const AGE_HISS_RMS: f32 = 0.004; // -48 dBFS
/// Hiss is mostly treble; below this it thins out.
const HISS_LOW_HZ: f32 = 300.0;

/// Dropouts a second at full Age.
const DROPOUTS_PER_SECOND: f32 = 0.7;
/// How fast a dropout falls and recovers.
const DROPOUT_FALL_MS: f32 = 3.0;
const DROPOUT_RISE_MS: f32 = 25.0;
/// Where a dropout's dip divides the lows, which it dips less, from the highs.
const DROPOUT_SPLIT_HZ: f32 = 1500.0;

/// Where on its cycle each partial of the right track's transport starts at
/// full Width, in turns.
const RIGHT_TRACK_TURNS: [f32; 4] = [0.28, 0.17, 0.39, 0.55];

/// A tape speed and what it does to the sound.
#[derive(Clone, Copy, Debug)]
pub struct Speed {
    /// How fast the reels and capstan turn, against 15 ips.
    pub rate: f32,
    /// How deep the wobble is, against 15 ips.
    pub wobble: f32,
    /// The head bump.
    pub bump_hz: f32,
    /// Where the record pre-emphasis starts to lift: 1/(2πτ).
    pub emphasis_hz: f32,
    /// The top of the tape's response.
    pub top_hz: f32,
    /// Where full Age leaves the top.
    pub worn_hz: f32,
}

/// The speeds, in the order of the Speed choice (patches save the index).
pub const SPEEDS: [Speed; 3] = [
    // 7½ ips, NAB 50 µs
    Speed { rate: 0.5, wobble: 1.6, bump_hz: 40.0, emphasis_hz: 3183.0, top_hz: 12000.0, worn_hz: 3500.0 },
    // 15 ips, IEC 35 µs
    Speed { rate: 1.0, wobble: 1.0, bump_hz: 70.0, emphasis_hz: 4547.0, top_hz: 18000.0, worn_hz: 5500.0 },
    // 30 ips, AES 17.5 µs
    Speed { rate: 2.0, wobble: 0.6, bump_hz: 120.0, emphasis_hz: 9095.0, top_hz: 22000.0, worn_hz: 8000.0 },
];

/// Peak pitch swing of the wow, in cents, for a Wow setting (0..1) at a speed.
pub fn wow_cents(wow: f32, speed: usize) -> f32 {
    WOW_CENTS * wow * wow * SPEEDS[speed].wobble
}

/// Peak pitch swing of the flutter, in cents.
pub fn flutter_cents(flutter: f32, speed: usize) -> f32 {
    FLUTTER_CENTS * flutter * flutter * SPEEDS[speed].wobble
}

/// Each transport partial's share of its group's pitch swing: the wow pair
/// and the flutter pair keep the Delay's proportions.
fn partial_shares() -> [f32; 4] {
    let p = TapeTransport::PARTIALS;
    let wow = p[0].1 + p[1].1;
    let flutter = p[2].1 + p[3].1;
    [p[0].1 / wow, p[1].1 / wow, p[2].1 / flutter, p[3].1 / flutter]
}

/// A one-pole lowpass, trapezoidal: `gg` is `g/(1+g)` of the prewarped gain.
#[derive(Clone, Copy, Debug, Default)]
struct OnePole {
    state: f32,
}

impl OnePole {
    #[inline]
    fn lowpass(&mut self, x: f32, gg: f32) -> f32 {
        let v = (x - self.state) * gg;
        let y = v + self.state;
        self.state = flush(y + v);
        y
    }
}

/// `g/(1+g)` for a one-pole at `g`.
fn one_pole(g: f32) -> f32 {
    g / (1.0 + g)
}

/// A state-variable filter, trapezoidal (Zavalishin).
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    s1: f32,
    s2: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct SvfCoeffs {
    g: f32,
    /// 2R, the damping: 1/Q.
    r2: f32,
    h: f32,
}

impl SvfCoeffs {
    fn new(cutoff_hz: f32, r2: f32, sample_rate: f32) -> Self {
        let g = prewarp(cutoff_hz, sample_rate);
        Self { g, r2, h: 1.0 / (1.0 + r2 * g + g * g) }
    }
}

impl Svf {
    /// Returns (lowpass, bandpass). `r2 · bandpass` has unity gain at the cutoff.
    #[inline]
    fn process(&mut self, x: f32, c: &SvfCoeffs) -> (f32, f32) {
        let hp = (x - (c.r2 + c.g) * self.s1 - self.s2) * c.h;
        let v1 = c.g * hp;
        let bp = v1 + self.s1;
        self.s1 = flush(bp + v1);
        let v2 = c.g * bp;
        let lp = v2 + self.s2;
        self.s2 = flush(lp + v2);
        (lp, bp)
    }
}

/// Filter coefficients for a block, from the speed and the Age.
#[derive(Clone, Copy, Debug, Default)]
struct Coeffs {
    /// Pre-emphasis: its highpass's pole sits (1 + EMPHASIS) above the lift.
    pre: f32,
    /// De-emphasis, the exact inverse: a lowpass at the pre's g / (1 + EMPHASIS).
    de: f32,
    dc_pole: f32,
    bump: SvfCoeffs,
    top: f32,
    hiss_low: f32,
    worn: SvfCoeffs,
    split: f32,
}

impl Coeffs {
    fn new(speed: &Speed, age: f32, sample_rate: f32) -> Self {
        let pre_g = prewarp(speed.emphasis_hz * (1.0 + EMPHASIS), sample_rate);
        // Gap loss falls from the top of hearing toward the worn cutoff
        let worn_hz = 20000.0 * (speed.worn_hz / 20000.0).powf(age.clamp(0.0, 1.0));
        Self {
            pre: one_pole(pre_g),
            de: one_pole(pre_g / (1.0 + EMPHASIS)),
            dc_pole: 1.0 - std::f32::consts::TAU * DC_HZ / sample_rate,
            bump: SvfCoeffs::new(speed.bump_hz, BUMP_DAMPING, sample_rate),
            top: one_pole(prewarp(speed.top_hz, sample_rate)),
            hiss_low: one_pole(prewarp(HISS_LOW_HZ, sample_rate)),
            worn: SvfCoeffs::new(worn_hz, 1.4, sample_rate),
            split: one_pole(prewarp(DROPOUT_SPLIT_HZ, sample_rate)),
        }
    }
}

/// One track of the tape: everything per channel.
struct Track {
    pre: OnePole,
    head: Adaa1,
    de: OnePole,
    dc_x: f32,
    dc_y: f32,
    bump: Svf,
    top: OnePole,
    /// The tape between the record head and the read head.
    line: FracDelay,
    hiss: NoiseFloor,
    hiss_low: OnePole,
    worn: Svf,
    split: OnePole,
}

impl Track {
    fn new(sample_rate: f32, hiss_seed: u32) -> Self {
        Self {
            pre: OnePole::default(),
            head: Adaa1::new(),
            de: OnePole::default(),
            dc_x: 0.0,
            dc_y: 0.0,
            bump: Svf::default(),
            top: OnePole::default(),
            line: FracDelay::new(Self::line_length(sample_rate)),
            hiss: NoiseFloor::with_seed(1.0, hiss_seed),
            hiss_low: OnePole::default(),
            worn: Svf::default(),
            split: OnePole::default(),
        }
    }

    fn line_length(sample_rate: f32) -> usize {
        ((MAX_LAG_SECONDS + STOP_ROOM_SECONDS) * sample_rate) as usize + 4
    }

    /// Records `x` onto the tape: the record path, faded in by `amount`.
    #[inline]
    fn record(&mut self, x: f32, amount: f32, drive: f32, makeup: f32, curve: &BiasedTanh, c: &Coeffs) -> f32 {
        // Pre-emphasis, x + E·highpass, so the treble meets the head first
        let emphasised = x + EMPHASIS * (x - self.pre.lowpass(x, c.pre));
        let level = RecordHead::LEVEL;
        let squashed = level * self.head.process(curve, emphasised * drive / level) * makeup;
        // De-emphasis, (y + E·lowpass)/(1 + E), undoes the lift exactly
        let flat = (squashed + EMPHASIS * self.de.lowpass(squashed, c.de)) / (1.0 + EMPHASIS);
        self.dc_y = flush(flat - self.dc_x + c.dc_pole * self.dc_y);
        self.dc_x = flat;
        let (low, band) = self.bump.process(self.dc_y, &c.bump);
        let bumped = self.dc_y + BUMP_LIFT * c.bump.r2 * band - BUMP_CUT * low;
        let taped = self.top.lowpass(bumped, c.top);
        x + amount * (taped - x)
    }

    /// Reads the tape `lag` samples behind the record head, which has just
    /// written `newest`.
    #[inline]
    fn play(&self, newest: f32, lag: f32) -> f32 {
        if lag < 1.0 {
            // The cubic needs a sample from the future here; a straight line doesn't
            newest + lag * (self.line.read(1.0) - newest)
        } else {
            self.line.read(lag)
        }
    }

    /// Plays the tape through the stop's heads: the wobble's lag `wobble`
    /// plus the stop's, fading from the old head if there is one.
    #[inline]
    fn play_stopping(&self, newest: f32, wobble: f32, heads: &StopHeads) -> f32 {
        let mut y = self.play(newest, wobble + heads.lag);
        if heads.old_weight > 0.0 {
            y += heads.old_weight * (self.play(newest, wobble + heads.old_lag) - y);
        }
        y * heads.gain
    }

    /// Hiss at `level` (peak), mostly treble.
    #[inline]
    fn hiss(&mut self, level: f32, c: &Coeffs) -> f32 {
        let white = self.hiss.sample();
        level * (white - self.hiss_low.lowpass(white, c.hiss_low))
    }

    /// The tape's age: gap loss, and a dropout dipping it to `dropout`,
    /// faded in by `amount`.
    #[inline]
    fn age(&mut self, x: f32, amount: f32, dropout: f32, c: &Coeffs) -> f32 {
        let (worn, _) = self.worn.process(x, &c.worn);
        let low = self.split.lowpass(worn, c.split);
        let dipped = low * (0.5 + 0.5 * dropout) + (worn - low) * dropout;
        x + amount * (dipped - x)
    }

    fn reset(&mut self) {
        self.pre = OnePole::default();
        self.head.reset();
        self.de = OnePole::default();
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.bump = Svf::default();
        self.top = OnePole::default();
        self.line.clear();
        self.hiss_low = OnePole::default();
        self.worn = Svf::default();
        self.split = OnePole::default();
    }
}

/// Where the oxide has worn away: brief dips at random, both tracks at once.
struct Dropouts {
    rng: NoiseFloor,
    /// Samples left in the dropout under way.
    hold: u32,
    /// The gain the dropout under way dips to.
    depth: f32,
    /// The gain now, gliding to `depth` or back to 1.
    gain: f32,
    fall: f32,
    rise: f32,
}

impl Dropouts {
    fn new(sample_rate: f32) -> Self {
        let mut dropouts = Self { rng: NoiseFloor::with_seed(1.0, 0x5eed_7a9e), hold: 0, depth: 1.0, gain: 1.0, fall: 0.0, rise: 0.0 };
        dropouts.set_sample_rate(sample_rate);
        dropouts
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fall = 1.0 - (-1000.0 / (DROPOUT_FALL_MS * sample_rate)).exp();
        self.rise = 1.0 - (-1000.0 / (DROPOUT_RISE_MS * sample_rate)).exp();
    }

    /// A uniform random number in 0..1.
    #[inline]
    fn uniform(&mut self) -> f32 {
        0.5 + 0.5 * self.rng.sample()
    }

    /// The gain for the next sample, at an Age of `age`.
    #[inline]
    fn next(&mut self, age: f32, sample_rate: f32) -> f32 {
        if self.hold > 0 {
            self.hold -= 1;
        } else if age > 0.0 && self.uniform() < DROPOUTS_PER_SECOND * age * age / sample_rate {
            // 15 to 150 ms, dipping deeper the older the tape
            self.hold = ((0.015 + 0.135 * self.uniform()) * sample_rate) as u32;
            self.depth = 1.0 - age * (0.3 + 0.6 * self.uniform());
        }
        let target = if self.hold > 0 { self.depth } else { 1.0 };
        let rate = if target < self.gain { self.fall } else { self.rise };
        self.gain += rate * (target - self.gain);
        // Settle exactly, so a tape with no dropouts plays at exactly unity
        if (self.gain - target).abs() < 1e-6 {
            self.gain = target;
        }
        self.gain
    }

    fn reset(&mut self) {
        self.hold = 0;
        self.gain = 1.0;
    }
}

/// A tape machine for a bus: wow, flutter, saturation and age.
///
/// # Ports
///
/// - **In L** / **In R** (Audio, Input): the bus; R copies L when unpatched.
/// - **Wow CV** (Control, Input): adds to Wow, for a deliberate warped moment.
/// - **Stop** (Gate, Input): brakes the tape to a halt while high, and winds
///   it back up when let go.
/// - **Out L** / **Out R** (Audio, Output): the bus, off tape.
///
/// # Parameters
///
/// - **Wow** (0-100%): the slow lurch of the reels, up to 40 cents at 15 ips.
/// - **Flutter** (0-100%): the fast shiver of the capstan, up to 12 cents.
/// - **Width** (0-100%): how differently the right track wobbles.
/// - **Saturation** (0-100%): drive into the record head, up to +18 dB, with
///   the head bump.
/// - **Age** (0-100%): from new to found in an attic: gap loss, dropouts and
///   some hiss.
/// - **Hiss** (0-100%): hiss, up to -34 dBFS.
/// - **Mix** (0-100%): dry to tape.
/// - **Speed** (7½, 15, 30 ips): slower is wobblier, darker, with a lower
///   head bump.
/// - **Stop Time** (50 ms-4 s): how long the tape takes to stop; winding
///   back up takes half as long.
pub struct Tape {
    sample_rate: f32,
    left: Track,
    right: Track,
    /// The transport, as the left track hears it, and the right track's own.
    transport_left: TapeTransport,
    transport_right: TapeTransport,
    dropouts: Dropouts,
    stop: TapeStop,
    curve: BiasedTanh,
    coeffs: Coeffs,
    /// Each partial's peak lag, in samples.
    lag: [SmoothedValue; 4],
    width: SmoothedValue,
    saturation: SmoothedValue,
    drive: SmoothedValue,
    makeup: SmoothedValue,
    /// The drive `makeup` was last worked out for.
    makeup_drive: f32,
    age: SmoothedValue,
    hiss: SmoothedValue,
    mix: SmoothedValue,
    /// For the display: the reels' turn (0..1), the pitch the left track is
    /// playing at in cents, and the hardest the head was driven in the last
    /// block, against the head's level.
    reel_turns: f32,
    pitch_cents: f32,
    drive_peak: f32,
    /// The left track's lag at the end of the last block, all of it and the
    /// wow's part alone.
    last_lag: f32,
    last_wow_lag: f32,
    /// How fast the reels turned in the last block, against the tape speed.
    reel_speed: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Tape {
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let glide = |initial| SmoothedValue::new(initial, WOBBLE_GLIDE_MS, sample_rate);
        Self {
            sample_rate,
            left: Track::new(sample_rate, 0x2545_f491),
            right: Track::new(sample_rate, 0x9e37_79b9),
            transport_left: TapeTransport::new(sample_rate),
            transport_right: TapeTransport::with_start(sample_rate, RIGHT_TRACK_TURNS),
            dropouts: Dropouts::new(sample_rate),
            stop: TapeStop::new(sample_rate, STOP_ROOM_SECONDS),
            curve: BiasedTanh::new(RecordHead::BIAS as f64),
            coeffs: Coeffs::new(&SPEEDS[1], 0.0, sample_rate),
            lag: [glide(0.0), glide(0.0), glide(0.0), glide(0.0)],
            width: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            saturation: SmoothedValue::new(0.0, 30.0, sample_rate),
            drive: SmoothedValue::new(1.0, 30.0, sample_rate),
            makeup: SmoothedValue::new(1.0, 30.0, sample_rate),
            makeup_drive: f32::NAN,
            age: SmoothedValue::new(0.0, 30.0, sample_rate),
            hiss: SmoothedValue::new(0.0, 30.0, sample_rate),
            mix: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            reel_turns: 0.0,
            pitch_cents: 0.0,
            drive_peak: 0.0,
            last_lag: 0.0,
            last_wow_lag: 0.0,
            reel_speed: 1.0,
            ports: vec![
                PortDefinition::input_with_default("in_l", "In L", SignalType::Audio, 0.0).describe("Left of the bus to put on tape"),
                PortDefinition::input_with_default("in_r", "In R", SignalType::Audio, 0.0).describe("Right of the bus; copies left when unpatched"),
                PortDefinition::input_with_default("wow_cv", "Wow CV", SignalType::Control, 0.0).describe("CV that adds to Wow, for a warped moment"),
                PortDefinition::input_with_default("stop", "Stop", SignalType::Gate, 0.0)
                    .describe("Brakes the tape to a halt while high, pitching the bus down to nothing; winds it back up when let go"),
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("Left, off tape"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("Right, off tape"),
            ],
            parameters: vec![
                ParameterDefinition::normalized("wow", "Wow", 0.25).describe("Slow lurch of the reels: up to 40 cents at 15 ips"),
                ParameterDefinition::normalized("flutter", "Flutter", 0.2).describe("Fast shiver of the capstan: up to 12 cents at 15 ips"),
                ParameterDefinition::normalized("width", "Width", 0.3).describe("How differently the right track wobbles from the left"),
                ParameterDefinition::normalized("saturation", "Saturation", 0.3).describe("Drive into the record head, up to +18 dB, with the head bump; -18 dBFS keeps its level"),
                ParameterDefinition::normalized("age", "Age", 0.15).describe("From new to found in an attic: duller, with dropouts and a little hiss"),
                ParameterDefinition::normalized("hiss", "Hiss", 0.15).describe("Tape hiss, up to -34 dBFS"),
                ParameterDefinition::normalized("mix", "Mix", 1.0).describe("Blend from dry (0) to tape only (1)"),
                ParameterDefinition::choice("speed", "Speed", &["7½ ips", "15 ips", "30 ips"], 1)
                    .describe("Tape speed: slower is wobblier and darker, with a lower head bump"),
                ParameterDefinition::new("stop_time", "Stop Time", MIN_STOP_SECONDS, MAX_STOP_SECONDS, 1.0, ParameterDisplay::logarithmic("s"))
                    .describe("How long the Stop gate takes to brake the tape to a halt; winding back up takes half as long"),
            ],
        }
    }

    const PORT_IN_L: usize = 0;
    const PORT_IN_R: usize = 1;
    const PORT_WOW_CV: usize = 2;
    const PORT_STOP: usize = 3;

    pub const PARAM_WOW: usize = 0;
    pub const PARAM_FLUTTER: usize = 1;
    pub const PARAM_WIDTH: usize = 2;
    pub const PARAM_SATURATION: usize = 3;
    pub const PARAM_AGE: usize = 4;
    pub const PARAM_HISS: usize = 5;
    pub const PARAM_MIX: usize = 6;
    pub const PARAM_SPEED: usize = 7;
    pub const PARAM_STOP_TIME: usize = 8;

    /// Readout slots.
    pub const READOUT_REEL: usize = 0;
    pub const READOUT_PITCH: usize = 1;
    pub const READOUT_DRIVE: usize = 2;
    pub const READOUT_DROPOUT: usize = 3;
    /// How fast the reels are turning against the tape speed: 1 steady, and
    /// swinging either side of it with the wow, exaggerated for the eye.
    pub const READOUT_REEL_SPEED: usize = 4;
    /// How far the Stop has slowed the transport: 0 at speed, 1 stopped.
    pub const READOUT_BRAKE: usize = 5;

    /// The drive into the record head for a Saturation setting.
    fn drive_for(saturation: f32) -> f32 {
        10f32.powf(MAX_DRIVE_DB * saturation.clamp(0.0, 1.0) / 20.0)
    }

    /// The gain after the head that keeps a 0 VU sine at its level (RMS,
    /// leaving out the DC the lopsided head adds).
    fn makeup_for(&self, drive: f32) -> f32 {
        const POINTS: usize = 64;
        let level = RecordHead::LEVEL as f64;
        let peak = NOMINAL_PEAK as f64 * drive as f64 / level;
        let (mut sum, mut sum_sq) = (0.0, 0.0);
        for n in 0..POINTS {
            let y = level * self.curve.value(peak * (std::f64::consts::TAU * n as f64 / POINTS as f64).sin());
            sum += y;
            sum_sq += y * y;
        }
        let mean = sum / POINTS as f64;
        let rms_out = (sum_sq / POINTS as f64 - mean * mean).sqrt();
        (NOMINAL_PEAK as f64 / std::f64::consts::SQRT_2 / rms_out) as f32
    }

    fn speed(params: &[f32]) -> usize {
        (params.get(Self::PARAM_SPEED).copied().unwrap_or(1.0).max(0.0) as usize).min(SPEEDS.len() - 1)
    }
}

impl Default for Tape {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Tape {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "fx.tape",
            name: "Tape",
            category: ModuleCategory::Effect,
            description: "A tape machine for a bus: wow, flutter, saturation, age and hiss",
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
        if sample_rate != self.sample_rate {
            self.left.line.allocate(Track::line_length(sample_rate));
            self.right.line.allocate(Track::line_length(sample_rate));
        }
        self.sample_rate = sample_rate;
        for value in self.lag.iter_mut().chain([
            &mut self.width,
            &mut self.saturation,
            &mut self.drive,
            &mut self.makeup,
            &mut self.age,
            &mut self.hiss,
            &mut self.mix,
        ]) {
            value.set_sample_rate(sample_rate);
        }
        self.transport_left.set_sample_rate(sample_rate);
        self.transport_right.set_sample_rate(sample_rate);
        self.dropouts.set_sample_rate(sample_rate);
        self.stop.set_sample_rate(sample_rate, STOP_ROOM_SECONDS);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let sr = self.sample_rate;
        let speed_index = Self::speed(params);
        let speed = SPEEDS[speed_index];
        let wow = params[Self::PARAM_WOW];
        let flutter = params[Self::PARAM_FLUTTER];
        let saturation = params[Self::PARAM_SATURATION].clamp(0.0, 1.0);
        let age = params[Self::PARAM_AGE].clamp(0.0, 1.0);
        let hiss = params[Self::PARAM_HISS].clamp(0.0, 1.0);
        let stop_time = params.get(Self::PARAM_STOP_TIME).copied().unwrap_or(1.0).clamp(MIN_STOP_SECONDS, MAX_STOP_SECONDS);

        // Each stage fades in over the bottom of its knob, so at zero it's bypassed exactly
        self.saturation.set_target((saturation * 10.0).min(1.0));
        let drive = Self::drive_for(saturation);
        self.drive.set_target(drive);
        if drive != self.makeup_drive {
            self.makeup_drive = drive;
            let makeup = self.makeup_for(drive);
            self.makeup.set_target(makeup);
        }
        self.age.set_target(age);
        let hiss_rms = HISS_RMS * hiss * hiss + AGE_HISS_RMS * age * age;
        self.hiss.set_target(hiss_rms * 3f32.sqrt());
        self.width.set_target(params[Self::PARAM_WIDTH].clamp(0.0, 1.0));
        self.mix.set_target(params[Self::PARAM_MIX].clamp(0.0, 1.0));

        // The speed sets how fast the reels turn, and the filters; Age moves
        // slowly enough to retune its filter once a block
        for transport in [&mut self.transport_left, &mut self.transport_right] {
            transport.set_rate_scale(speed.rate, sr);
            transport.renormalize();
        }
        self.coeffs = Coeffs::new(&speed, self.age.current(), sr);
        let c = self.coeffs;

        // Samples of lag per cent of pitch swing, for each partial
        let shares = partial_shares();
        let lag_per_cent: [f32; 4] = std::array::from_fn(|i| {
            shares[i] / CENTS_PER_DEVIATION * sr / (std::f32::consts::TAU * self.transport_left.rate(i))
        });
        let flutter_lag = flutter_cents(flutter.clamp(0.0, 1.0), speed_index);

        let in_left = inputs.get(Self::PORT_IN_L);
        let in_right = connected_input(inputs, Self::PORT_IN_R);
        let wow_cv = inputs.get(Self::PORT_WOW_CV);
        let stop_gate = inputs.get(Self::PORT_STOP);

        let (out_left, rest) = outputs.split_at_mut(1);
        let out_left = &mut out_left[0];
        let out_right = &mut rest[0];

        let mut drive_peak = 0.0f32;
        let mut lag_left = self.last_lag;
        let mut wow_left = self.last_wow_lag;
        let mut travelled = 0.0;
        for i in 0..context.block_size {
            let x_left = in_left.and_then(|buf| buf.samples.get(i).copied()).unwrap_or(0.0);
            let x_right = match in_right {
                Some(buf) => buf.samples.get(i).copied().unwrap_or(0.0),
                None => x_left,
            };

            // The wobble's depth, with Wow CV added to the knob
            let cv = wow_cv.and_then(|buf| buf.samples.get(i).copied()).unwrap_or(0.0);
            let wow_lag = wow_cents((wow + cv).clamp(0.0, 1.0), speed_index);
            let depth = [wow_lag, wow_lag, flutter_lag, flutter_lag];
            let mut lag = [0.0; 4];
            for p in 0..4 {
                self.lag[p].set_target(depth[p] * lag_per_cent[p]);
                lag[p] = self.lag[p].next();
            }
            let drag_left = self.transport_left.next_drag();
            let drag_right = self.transport_right.next_drag();
            lag_left = (0..4).map(|p| lag[p] * drag_left[p]).sum::<f32>();
            wow_left = lag[0] * drag_left[0] + lag[1] * drag_left[1];
            let lag_right_own = (0..4).map(|p| lag[p] * drag_right[p]).sum::<f32>();
            let lag_right = lag_left + self.width.next() * (lag_right_own - lag_left);

            // Record
            let amount = self.saturation.next();
            let drive = self.drive.next();
            let makeup = self.makeup.next();
            drive_peak = drive_peak.max(x_left.abs().max(x_right.abs()) * drive * amount);
            let rec_left = self.left.record(x_left, amount, drive, makeup, &self.curve, &c);
            let rec_right = self.right.record(x_right, amount, drive, makeup, &self.curve, &c);
            self.left.line.push(rec_left);
            self.right.line.push(rec_right);

            // Play back, with hiss, through the tape's age; a stopped tape plays nothing
            let gate = stop_gate.and_then(|buf| buf.samples.get(i).copied()).unwrap_or(0.0) > 0.5;
            let heads = self.stop.next(gate, stop_time);
            travelled += self.stop.speed();
            let hiss = self.hiss.next();
            let age = self.age.next();
            let age_amount = (age * 5.0).min(1.0);
            let dropout = self.dropouts.next(age, sr);
            let play_left = self.left.play_stopping(rec_left, lag_left, &heads) + heads.gain * self.left.hiss(hiss, &c);
            let play_right = self.right.play_stopping(rec_right, lag_right, &heads) + heads.gain * self.right.hiss(hiss, &c);
            let wet_left = self.left.age(play_left, age_amount, dropout, &c);
            let wet_right = self.right.age(play_right, age_amount, dropout, &c);

            let mix = self.mix.next();
            out_left.samples[i] = x_left + mix * (wet_left - x_left);
            out_right.samples[i] = x_right + mix * (wet_right - x_right);
        }

        // For the display: the pitch follows the lag, and the reels turn at
        // the speed, lurching with the wow. The flutter is the capstan's, so
        // it stays off the reels
        let block = context.block_size as f32;
        if block > 0.0 {
            self.pitch_cents = -(lag_left - self.last_lag) / block * CENTS_PER_DEVIATION;
            self.reel_speed = reel_speed(-(wow_left - self.last_wow_lag) / block);
            self.reel_turns =
                (self.reel_turns + REEL_TURNS_PER_SECOND * speed.rate * self.reel_speed * travelled / sr).fract();
        }
        self.last_lag = lag_left;
        self.last_wow_lag = wow_left;
        self.drive_peak = drive_peak / RecordHead::LEVEL;
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.transport_left.reset();
        self.transport_right.reset();
        self.dropouts.reset();
        self.stop.reset();
        for value in self.lag.iter_mut().chain([
            &mut self.width,
            &mut self.saturation,
            &mut self.drive,
            &mut self.makeup,
            &mut self.age,
            &mut self.hiss,
            &mut self.mix,
        ]) {
            value.reset(value.target());
        }
        self.last_lag = 0.0;
        self.last_wow_lag = 0.0;
        self.reel_speed = 1.0;
        self.pitch_cents = 0.0;
        self.drive_peak = 0.0;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_REEL] = self.reel_turns;
        readout.values[Self::READOUT_PITCH] = self.pitch_cents;
        readout.values[Self::READOUT_DRIVE] = self.drive_peak;
        readout.values[Self::READOUT_DROPOUT] = self.dropouts.gain;
        readout.values[Self::READOUT_REEL_SPEED] = self.reel_speed;
        readout.values[Self::READOUT_BRAKE] = 1.0 - self.stop.speed();
        Some(readout)
    }
}

/// How fast the display's reels turn at 15 ips.
pub const REEL_TURNS_PER_SECOND: f32 = 0.6;

/// How many times over the reels show the wow's swing in speed: the 22
/// cents of Wow at 75% swing them about half as fast again either way.
const REEL_LURCH: f32 = 50.0;

/// The most the reels' lurch can take from or add to their speed, so even
/// full Wow at 7½ ips never stalls them or runs them backwards.
const REEL_LURCH_MAX: f32 = 0.85;

/// How fast the reels turn against the tape speed, for the wow's swing
/// in the tape's speed (`d` in a ratio of `1 + d`): exaggerated so the eye
/// can follow it, and gently held back as it nears the limit.
fn reel_speed(deviation: f32) -> f32 {
    1.0 + REEL_LURCH_MAX * (REEL_LURCH * deviation / REEL_LURCH_MAX).tanh()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{rms, Spectrum};

    const SR: f32 = 48000.0;
    const BLOCK: usize = 480;

    /// Parameters with every knob at zero, at 15 ips.
    fn zeros() -> [f32; 8] {
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]
    }

    fn tape() -> Tape {
        let mut tape = Tape::new();
        tape.prepare(SR, BLOCK);
        tape
    }

    /// Runs stereo input through in blocks; `wow_cv` is constant if given.
    fn run_stereo(tape: &mut Tape, left: &[f32], right: Option<&[f32]>, params: &[f32], wow_cv: f32) -> (Vec<f32>, Vec<f32>) {
        let ctx = ProcessContext::new(SR, BLOCK);
        let mut cv = SignalBuffer::control(BLOCK);
        cv.fill(wow_cv);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK)];
        let (mut out_l, mut out_r) = (Vec::new(), Vec::new());
        for (n, chunk) in left.chunks(BLOCK).enumerate() {
            let mut l = SignalBuffer::audio(BLOCK);
            l.samples[..chunk.len()].copy_from_slice(chunk);
            let r = match right {
                Some(right) => {
                    let mut r = SignalBuffer::audio(BLOCK);
                    let part = &right[n * BLOCK..(n * BLOCK + chunk.len())];
                    r.samples[..chunk.len()].copy_from_slice(part);
                    r
                }
                None => SignalBuffer::unconnected(BLOCK, SignalType::Audio),
            };
            tape.process(&[&l, &r, &cv], &mut outputs, params, &ctx);
            out_l.extend_from_slice(&outputs[0].samples[..chunk.len()]);
            out_r.extend_from_slice(&outputs[1].samples[..chunk.len()]);
        }
        (out_l, out_r)
    }

    fn run(tape: &mut Tape, input: &[f32], params: &[f32]) -> Vec<f32> {
        run_stereo(tape, input, None, params, 0.0).0
    }

    fn sine(freq: f32, amplitude: f32, seconds: f32) -> Vec<f32> {
        (0..(seconds * SR) as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / SR).sin() * amplitude)
            .collect()
    }

    fn noise(seconds: f32, amplitude: f32, seed: u32) -> Vec<f32> {
        let mut rng = NoiseFloor::with_seed(amplitude, seed);
        (0..(seconds * SR) as usize).map(|_| rng.sample()).collect()
    }

    #[test]
    fn test_saturated_tape_starts_without_a_click() {
        // The head's curve is lopsided, so its antiderivative isn't zero at
        // rest; a fresh head once took it to be, and the first quiet sample
        // came off the tape as a click 14 dB over full scale. The quieter
        // the first sample, the louder the click, so this one is a whisper.
        // (Exactly zero would hide it: the head takes its antiderivative
        // from there.)
        let mut input = sine(220.0, 0.22, 0.05);
        input[0] = 1e-4;
        let mut params = zeros();
        params[Tape::PARAM_SATURATION] = 0.35;
        params[Tape::PARAM_MIX] = 1.0;
        let mut tape = tape();
        for pass in ["fresh", "reset"] {
            let out = run(&mut tape, &input, &params);
            let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak < 0.5, "{} tape's first 50 ms peak at {}", pass, peak);
            tape.reset();
        }
    }

    /// Instantaneous frequency of a sine, from its upward zero crossings,
    /// averaged over windows of `cycles`.
    fn frequencies(signal: &[f32], cycles: usize) -> Vec<f32> {
        let crossings: Vec<f32> = signal
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, w)| i as f32 + w[0] / (w[0] - w[1]))
            .collect();
        crossings
            .windows(cycles + 1)
            .step_by(cycles)
            .map(|w| cycles as f32 * SR / (w[cycles] - w[0]))
            .collect()
    }

    #[test]
    fn test_tape_info_ports_and_parameters() {
        let tape = Tape::new();
        assert_eq!(tape.info().id, "fx.tape");
        assert_eq!(tape.info().category, ModuleCategory::Effect);
        let ids: Vec<_> = tape.ports().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["in_l", "in_r", "wow_cv", "stop", "out_l", "out_r"]);
        let ids: Vec<_> = tape.parameters().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["wow", "flutter", "width", "saturation", "age", "hiss", "mix", "speed", "stop_time"]);
    }

    #[test]
    fn test_every_knob_at_zero_passes_audio_unchanged() {
        // At every speed, with Mix at 0 and at full: bit for bit, no latency
        let left = noise(1.0, 0.9, 7);
        let right = noise(1.0, 0.5, 11);
        for speed in 0..3 {
            for mix in [0.0, 1.0] {
                let mut p = zeros();
                p[Tape::PARAM_SPEED] = speed as f32;
                p[Tape::PARAM_MIX] = mix;
                let (l, r) = run_stereo(&mut tape(), &left, Some(&right), &p, 0.0);
                assert_eq!(l, left, "speed {speed}, mix {mix}: left changed");
                assert_eq!(r, right, "speed {speed}, mix {mix}: right changed");
            }
        }
    }

    #[test]
    fn test_knobs_turned_back_to_zero_pass_audio_unchanged_again() {
        // Every stage fades out as well as in: no filter is left in the path
        // (the wobble's depth glides out slowest, settling to nothing in about 5 s)
        let input = noise(8.0, 0.5, 3);
        let mut tape = tape();
        run(&mut tape, &input[..SR as usize * 3], &[0.8, 0.8, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0]);
        let mut p = zeros();
        p[Tape::PARAM_MIX] = 1.0;
        let out = run(&mut tape, &input, &p);
        let settled = SR as usize * 6;
        assert_eq!(out[settled..], input[settled..]);
    }

    /// The peak pitch swing of a 1 kHz sine through the tape, in cents,
    /// measured after the first second.
    fn measured_swing(params: &[f32], wow_cv: f32) -> f32 {
        let input = sine(1000.0, 0.1, 12.0);
        let (out, _) = run_stereo(&mut tape(), &input, None, params, wow_cv);
        let freqs = frequencies(&out[SR as usize..], 10);
        freqs.iter().map(|f| (f / 1000.0 - 1.0).abs()).fold(0.0, f32::max) * CENTS_PER_DEVIATION
    }

    /// The largest swing the wow's two partials reach in the same window:
    /// they're at unrelated rates, so they rarely peak together.
    fn expected_swing(cents: f32, speed: usize) -> f32 {
        let shares = partial_shares();
        let rate = |p: usize| TapeTransport::PARTIALS[p].0 * SPEEDS[speed].rate;
        (SR as usize..SR as usize * 12)
            .step_by(48)
            .map(|n| {
                let t = n as f32 / SR;
                (0..2).map(|p| shares[p] * (std::f32::consts::TAU * rate(p) * t).sin()).sum::<f32>().abs()
            })
            .fold(0.0, f32::max)
            * cents
    }

    #[test]
    fn test_wow_depth_in_cents() {
        // On a 1 kHz sine, the pitch swings by the cents the knob promises
        for (wow, speed) in [(0.5, 1), (1.0, 1), (0.5, 0), (1.0, 2)] {
            let mut p = zeros();
            p[Tape::PARAM_WOW] = wow;
            p[Tape::PARAM_MIX] = 1.0;
            p[Tape::PARAM_SPEED] = speed as f32;
            let promised = wow_cents(wow, speed);
            let expected = expected_swing(promised, speed);
            let measured = measured_swing(&p, 0.0);
            assert!(
                (measured / expected - 1.0).abs() < 0.05,
                "wow {wow} at speed {speed}: {measured:.2} cents, expected {expected:.2} (knob promises {promised:.2})"
            );
        }
        assert_eq!(wow_cents(1.0, 1), 40.0);
        assert_eq!(wow_cents(0.5, 1), 10.0);
    }

    #[test]
    fn test_wow_cv_adds_to_the_knob() {
        let mut p = zeros();
        p[Tape::PARAM_WOW] = 0.25;
        p[Tape::PARAM_MIX] = 1.0;
        let swelled = measured_swing(&p, 0.5);
        let expected = expected_swing(wow_cents(0.75, 1), 1);
        assert!((swelled / expected - 1.0).abs() < 0.05, "{swelled} against {expected}");
    }

    #[test]
    fn test_flutter_shivers_faster_than_wow() {
        // Flutter alone: a fast, small swing, which 1 ms windows can follow
        let mut p = zeros();
        p[Tape::PARAM_FLUTTER] = 1.0;
        p[Tape::PARAM_MIX] = 1.0;
        let input = sine(1000.0, 0.1, 3.0);
        let out = run(&mut tape(), &input, &p);
        let freqs = frequencies(&out[SR as usize..], 1);
        let swing = freqs.iter().map(|f| (f / 1000.0 - 1.0).abs()).fold(0.0, f32::max) * CENTS_PER_DEVIATION;
        assert!(swing > 8.0 && swing < 14.0, "flutter swing {swing} cents");
        // It changes direction many times a second
        let turns = freqs.windows(3).filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0).count();
        assert!(turns > 20, "only {turns} turns in 2 s");
    }

    #[test]
    fn test_width_parts_the_tracks() {
        // A mono input stays mono at Width 0 and parts at full Width
        let mut p = zeros();
        p[Tape::PARAM_WOW] = 0.6;
        p[Tape::PARAM_MIX] = 1.0;
        let input = sine(440.0, 0.3, 3.0);
        let (l, r) = run_stereo(&mut tape(), &input, None, &p, 0.0);
        assert_eq!(l, r);
        p[Tape::PARAM_WIDTH] = 1.0;
        let (l, r) = run_stereo(&mut tape(), &input, None, &p, 0.0);
        let diff: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
        assert!(rms(&diff) > 0.01, "tracks barely differ: {}", rms(&diff));
    }

    /// Total harmonic distortion of a settled 200 Hz sine.
    fn thd(out: &[f32]) -> f64 {
        let window = &out[out.len() - 8192..];
        let spectrum = Spectrum::of(window, SR);
        let band = |hz: f64| -> f64 {
            spectrum
                .magnitudes
                .iter()
                .enumerate()
                .filter(|(k, _)| (*k as f64 * spectrum.bin_hz - hz).abs() < 30.0)
                .map(|(_, m)| m * m)
                .sum()
        };
        let harmonics: f64 = (2..10).map(|h| band(200.0 * h as f64)).sum();
        (harmonics / band(200.0)).sqrt()
    }

    #[test]
    fn test_saturation_squashes_loud_and_keeps_nominal_level() {
        let mut p = zeros();
        p[Tape::PARAM_MIX] = 1.0;
        let loud = sine(200.0, 0.7, 1.0);
        let mut last = 0.0;
        for saturation in [0.2, 0.5, 1.0] {
            p[Tape::PARAM_SATURATION] = saturation;
            let out = run(&mut tape(), &loud, &p);
            let distortion = thd(&out);
            assert!(distortion > last, "saturation {saturation}: THD {distortion} not above {last}");
            last = distortion;
        }
        assert!(last > 0.05, "full saturation THD only {last}");

        // A 0 VU sine at 1 kHz (above the head bump) comes out at about its own level
        let nominal = sine(1000.0, NOMINAL_PEAK, 1.0);
        let out = run(&mut tape(), &nominal, &p);
        let gain_db = 20.0 * (rms(&out[SR as usize / 2..]) / rms(&nominal[SR as usize / 2..])).log10();
        assert!(gain_db.abs() < 1.0, "0 VU moved {gain_db} dB at full saturation");

        // and the peaks are held well under full scale
        let out = run(&mut tape(), &loud, &p);
        assert!(crate::dsp::analysis::peak(&out[SR as usize / 2..]) < 0.45);
    }

    #[test]
    fn test_saturation_brings_the_head_bump_of_the_speed() {
        // Quiet sines, so the head stays linear: a lift at the bump, lower for slower tape
        let gain_db = |hz: f32, speed: usize| {
            let mut p = zeros();
            p[Tape::PARAM_SATURATION] = 0.2;
            p[Tape::PARAM_MIX] = 1.0;
            p[Tape::PARAM_SPEED] = speed as f32;
            let input = sine(hz, 0.01, 2.0);
            let out = run(&mut tape(), &input, &p);
            let settled = SR as usize;
            20.0 * (rms(&out[settled..]) / rms(&input[settled..])).log10()
        };
        // (the drive lifts quiet signals a little: below 0 VU the head is compressing less)
        for speed in 0..3 {
            let mids = gain_db(1000.0, speed);
            assert!(mids.abs() < 1.0, "speed {speed}: mids moved {mids} dB");
            let bump = gain_db(SPEEDS[speed].bump_hz, speed) - mids;
            assert!(bump > 2.0 && bump < 4.5, "speed {speed}: {bump} dB at the bump");
        }
        // Slower tape bumps lower: at 40 Hz, 7½ ips lifts more than 30 ips
        assert!(gain_db(40.0, 0) > gain_db(40.0, 2) + 2.0);
    }

    /// Treble against mids, as a ratio of energies.
    fn brightness(out: &[f32]) -> f64 {
        let spectrum = Spectrum::of(&out[out.len() - 16384..], SR);
        let energy = |lo: f64, hi: f64| -> f64 {
            spectrum
                .magnitudes
                .iter()
                .enumerate()
                .filter(|(k, _)| (lo..hi).contains(&(*k as f64 * spectrum.bin_hz)))
                .map(|(_, m)| m * m)
                .sum()
        };
        energy(6000.0, 16000.0) / energy(200.0, 2000.0)
    }

    #[test]
    fn test_age_darkens_and_slower_tape_darker_still() {
        let input = noise(1.0, 0.3, 5);
        let mut p = zeros();
        p[Tape::PARAM_MIX] = 1.0;
        let new = brightness(&run(&mut tape(), &input, &p));
        p[Tape::PARAM_AGE] = 0.5;
        let used = brightness(&run(&mut tape(), &input, &p));
        p[Tape::PARAM_AGE] = 1.0;
        let attic = brightness(&run(&mut tape(), &input, &p));
        p[Tape::PARAM_SPEED] = 0.0;
        let attic_slow = brightness(&run(&mut tape(), &input, &p));
        assert!(used < new * 0.7 && attic < used * 0.5 && attic_slow < attic * 0.7, "{new} {used} {attic} {attic_slow}");
    }

    #[test]
    fn test_old_tape_drops_out_and_new_tape_never_does() {
        // A steady tone's level, in 5 ms windows, over a minute
        let input = sine(500.0, 0.3, 60.0);
        let dips = |age: f32| {
            let mut p = zeros();
            p[Tape::PARAM_AGE] = age;
            p[Tape::PARAM_MIX] = 1.0;
            let out = run(&mut tape(), &input, &p);
            let levels = crate::dsp::analysis::windowed_rms(&out[SR as usize..], 240);
            let steady = levels.iter().copied().fold(0.0, f32::max);
            levels.iter().filter(|&&l| l < steady * 0.7).count()
        };
        assert_eq!(dips(0.0), 0);
        let old = dips(1.0);
        // About 0.7 dropouts a second, each tens of milliseconds long
        assert!(old > 100 && old < 3000, "{old} dipped windows");
    }

    #[test]
    fn test_hiss_level_and_its_stereo() {
        let silence = vec![0.0; SR as usize * 2];
        let mut p = zeros();
        p[Tape::PARAM_HISS] = 1.0;
        p[Tape::PARAM_MIX] = 1.0;
        let (l, r) = run_stereo(&mut tape(), &silence, None, &p, 0.0);
        let level_db = 20.0 * rms(&l[SR as usize..]).log10();
        assert!((level_db + 34.0).abs() < 2.0, "full hiss at {level_db} dBFS");
        // The two tracks hiss independently
        let (l, r) = (&l[SR as usize..], &r[SR as usize..]);
        let correlation = l.iter().zip(r).map(|(a, b)| a * b).sum::<f32>() / (rms(l) * rms(r) * l.len() as f32);
        assert!(correlation.abs() < 0.05, "hiss correlated {correlation}");
        // and it's mostly treble
        assert!(brightness(l) > 3.0);
    }

    #[test]
    fn test_extremes_stay_bounded() {
        // Everything at full, a loud noise burst, with the Wow CV past the top
        let input = noise(4.0, 1.0, 13);
        for speed in 0..3 {
            let mut p = [1.0; 8];
            p[Tape::PARAM_SPEED] = speed as f32;
            let (l, r) = run_stereo(&mut tape(), &input, None, &p, 1.0);
            for out in [&l, &r] {
                assert!(out.iter().all(|s| s.is_finite()));
                assert!(crate::dsp::analysis::peak(out) < 1.2, "speed {speed}: peak {}", crate::dsp::analysis::peak(out));
            }
        }
    }

    #[test]
    fn test_reset_clears_the_tape() {
        let mut tape = tape();
        let mut p = [0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 1.0, 1.0];
        run(&mut tape, &noise(1.0, 0.8, 1), &p);
        tape.reset();
        p[Tape::PARAM_HISS] = 0.0;
        let out = run(&mut tape, &vec![0.0; BLOCK * 4], &p);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_speed_change_glides() {
        // Switching 15 to 7½ ips mid-tone bends the pitch rather than jumping the read head
        let mut tape = tape();
        let mut p = zeros();
        p[Tape::PARAM_WOW] = 1.0;
        p[Tape::PARAM_MIX] = 1.0;
        let input = sine(220.0, 0.3, 2.0);
        let mut out = run(&mut tape, &input, &p);
        p[Tape::PARAM_SPEED] = 0.0;
        out.extend(run(&mut tape, &input, &p));
        let worst_step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst_step < 0.01, "step of {worst_step}");
    }

    #[test]
    fn test_readout_turns_the_reels() {
        let mut tape = tape();
        let mut p = zeros();
        p[Tape::PARAM_SATURATION] = 1.0;
        run(&mut tape, &sine(200.0, 0.5, 0.25), &p);
        let readout = tape.readout(&p).unwrap();
        assert!(readout.values[Tape::READOUT_REEL] > 0.0);
        assert!(readout.values[Tape::READOUT_DRIVE] > 1.0);
        assert_eq!(readout.values[Tape::READOUT_DROPOUT], 1.0);
    }

    /// The slowest and fastest the reels turn, block by block, over `seconds`.
    fn reel_speeds(params: &[f32], seconds: f32) -> (f32, f32) {
        let mut tape = tape();
        let ctx = ProcessContext::new(SR, BLOCK);
        let l = SignalBuffer::audio(BLOCK);
        let r = SignalBuffer::unconnected(BLOCK, SignalType::Audio);
        let cv = SignalBuffer::control(BLOCK);
        let gate = SignalBuffer::gate(BLOCK);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK)];
        let (mut slowest, mut fastest) = (f32::MAX, f32::MIN);
        for _ in 0..(seconds * SR) as usize / BLOCK {
            tape.process(&[&l, &r, &cv, &gate], &mut outputs, params, &ctx);
            let speed = tape.readout(params).unwrap().values[Tape::READOUT_REEL_SPEED];
            slowest = slowest.min(speed);
            fastest = fastest.max(speed);
        }
        (slowest, fastest)
    }

    #[test]
    fn test_the_reels_lurch_with_the_wow() {
        // Wow at 75%: the reels swing well either side of their speed
        let mut p = zeros();
        p[Tape::PARAM_WOW] = 0.75;
        let (slowest, fastest) = reel_speeds(&p, 6.0);
        assert!(slowest < 0.7 && fastest > 1.3, "reels between {slowest} and {fastest}");

        // Without wow or flutter they turn steadily, as they did
        assert_eq!(reel_speeds(&zeros(), 2.0), (1.0, 1.0));

        // The flutter is the capstan's: it shivers the tape, not the reels
        let mut p = zeros();
        p[Tape::PARAM_FLUTTER] = 1.0;
        let (slowest, fastest) = reel_speeds(&p, 2.0);
        assert_eq!((slowest, fastest), (1.0, 1.0));

        // Full wow at 7½ ips lurches hard, but never stalls or reverses
        let mut p = zeros();
        p[Tape::PARAM_WOW] = 1.0;
        p[Tape::PARAM_SPEED] = 0.0;
        let (slowest, fastest) = reel_speeds(&p, 10.0);
        assert!(slowest > 0.1 && fastest < 1.9, "reels between {slowest} and {fastest}");
    }

    /// Runs a mono input with a Stop gate, high over `stopped` (in seconds).
    fn run_stopping(tape: &mut Tape, input: &[f32], params: &[f32], stopped: std::ops::Range<f32>) -> Vec<f32> {
        let ctx = ProcessContext::new(SR, BLOCK);
        let cv = SignalBuffer::control(BLOCK);
        let r = SignalBuffer::unconnected(BLOCK, SignalType::Audio);
        let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK)];
        let mut out = Vec::new();
        for (n, chunk) in input.chunks(BLOCK).enumerate() {
            let mut l = SignalBuffer::audio(BLOCK);
            l.samples[..chunk.len()].copy_from_slice(chunk);
            let mut gate = SignalBuffer::gate(BLOCK);
            for (i, g) in gate.samples.iter_mut().enumerate() {
                let t = (n * BLOCK + i) as f32 / SR;
                *g = if stopped.contains(&t) { 1.0 } else { 0.0 };
            }
            tape.process(&[&l, &r, &cv, &gate], &mut outputs, params, &ctx);
            out.extend_from_slice(&outputs[0].samples[..chunk.len()]);
        }
        out
    }

    /// Every knob at zero but Mix, with a Stop Time.
    fn stop_params(stop_time: f32) -> [f32; 9] {
        let mut p = [0.0; 9];
        p[Tape::PARAM_SPEED] = 1.0;
        p[Tape::PARAM_MIX] = 1.0;
        p[Tape::PARAM_STOP_TIME] = stop_time;
        p
    }

    /// The largest second difference: a click shows up here long before it's
    /// loud enough to see in the waveform.
    fn roughness(signal: &[f32]) -> f32 {
        signal.windows(3).map(|w| (w[2] - 2.0 * w[1] + w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn test_stop_pitches_a_sine_down_to_nothing_over_the_stop_time() {
        // 1 kHz, stopping at 0.5 s over a second: the pitch falls in a straight
        // line, 1000 Hz less each second, so a quarter in it's at 750 Hz
        let input = sine(1000.0, 0.3, 3.0);
        let out = run_stopping(&mut tape(), &input, &stop_params(1.0), 0.5..2.0);
        let crossings: Vec<f32> = out
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, w)| i as f32 + w[0] / (w[0] - w[1]))
            .collect();
        let mut checked = 0;
        for pair in crossings.windows(2) {
            let t = 0.5 * (pair[0] + pair[1]) / SR - 0.5;
            if (0.05..0.85).contains(&t) {
                let heard = SR / (pair[1] - pair[0]);
                let expected = 1000.0 * (1.0 - t);
                assert!((heard / expected - 1.0).abs() < 0.02, "{t:.3} s in: {heard:.1} Hz, expected {expected:.1}");
                checked += 1;
            }
        }
        assert!(checked > 300, "only {checked} cycles measured");
        // Stopped, it's silent: no frozen sample held as DC
        let stopped = &out[(1.6 * SR) as usize..(2.0 * SR) as usize];
        assert!(stopped.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_stop_and_wind_up_without_clicks() {
        // Sounding at no more than 1.25 kHz (the wind-up's peak), a sine can't
        // bend faster than this; a click would be many times over it
        let input = sine(1000.0, 0.3, 4.0);
        let limit = 0.3 * (std::f32::consts::TAU * 1250.0 / SR).powi(2) * 1.2;
        // All the way to a stop and back, then let go halfway down (the
        // crossfade), and at the shortest and longest Stop Times
        for (stop_time, stopped) in [(1.0, 0.5..2.0), (1.0, 0.5..1.0), (0.05, 0.5..0.53), (0.05, 0.5..1.0), (4.0, 0.2..1.0)] {
            let out = run_stopping(&mut tape(), &input, &stop_params(stop_time), stopped.clone());
            let rough = roughness(&out);
            assert!(rough < limit, "stop time {stop_time}, stopped {stopped:?}: second difference {rough}, limit {limit}");
        }
    }

    #[test]
    fn test_wind_up_lands_back_in_time_with_no_latency() {
        // After a stop and its wind-up, the tape plays its input again bit for bit
        let input = noise(3.0, 0.5, 21);
        for stopped in [0.5..1.8, 0.5..0.9] {
            let out = run_stopping(&mut tape(), &input, &stop_params(1.0), stopped.clone());
            let back = ((stopped.end + 0.5 + 0.05) * SR) as usize;
            assert_eq!(out[back..], input[back..], "stopped {stopped:?}");
            // and before the gate rose, nothing had changed
            let before = (stopped.start * SR) as usize;
            assert_eq!(out[..before], input[..before]);
        }
    }

    #[test]
    fn test_the_reels_slow_to_a_halt() {
        let mut tape = tape();
        let p = stop_params(0.5);
        run_stopping(&mut tape, &vec![0.0; SR as usize], &p, 0.0..1.0);
        let readout = tape.readout(&p).unwrap();
        assert_eq!(readout.values[Tape::READOUT_BRAKE], 1.0);
        let still = readout.values[Tape::READOUT_REEL];
        run_stopping(&mut tape, &vec![0.0; BLOCK * 10], &p, 0.0..1.0);
        assert_eq!(tape.readout(&p).unwrap().values[Tape::READOUT_REEL], still);
    }

    #[test]
    fn test_the_tape_has_room_for_the_longest_stop() {
        assert!(TapeStop::lag_needed(MAX_STOP_SECONDS) < STOP_ROOM_SECONDS);
    }

    #[test]
    fn test_tape_registry_instantiation() {
        use crate::dsp::ModuleRegistry;
        let mut registry = ModuleRegistry::new();
        registry.register::<Tape>();
        let module = registry.create("fx.tape").unwrap();
        assert_eq!(module.parameters().len(), 9);
    }
}
