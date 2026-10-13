//! Vocoder: one sound's spectral envelope worn by another.
//!
//! The **modulator** (usually a voice) is split into a bank of band-passes,
//! and a follower on each band tracks how loud that part of the spectrum is.
//! The **carrier** (a synth chord) runs through a matching bank, and each of
//! its bands is turned up and down by its twin's follower, so the chord
//! takes on the voice's vowels.
//!
//! - **Formant** slides the carrier's bank against the voice's, by up to an
//!   octave either way: a smaller or larger throat.
//! - **Sibilance** lets the voice's own highs (above the bank) through, so
//!   consonants stay readable.
//! - **Unvoiced** swaps the carrier for noise while the voice is hissing,
//!   so an "s" or a "t" comes through even over a dull carrier, or none.
//! - The bands alternate left and right, as far apart as **Width** says.
//! - **Freeze** holds every band's follower where it is, so a vowel lasts as
//!   long as the chord does.
//! - **Flatten** evens out the carrier's own spectrum before the voice's is
//!   put on it, so a dull carrier speaks as clearly as a bright one.
//! - **Level** sets how loud it all comes out. Flattened, the output follows
//!   the carrier's level less closely, so this is where its loudness is set.
//!
//! Each band is two cascaded second-order sections (TPT state-variable
//! band-passes), so neighbours cross at -3 dB and the bank sums flat. The
//! whole bank is fixed-size: nothing is allocated, even in `prepare`.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, SQRT_2};

use crate::dsp::{
    context::ProcessContext,
    denormal::flush,
    dynamics::db_to_gain,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::noise::NoiseFloor,
    signal::{connected_input, SignalBuffer},
    smoothed_value::SmoothedValue,
    ParameterDisplay, Readout, SignalType, MAX_READOUT,
};

/// The most bands the bank runs.
pub const MAX_BANDS: usize = 24;

/// The choices of the Bands parameter.
pub const BAND_COUNTS: [usize; 3] = [8, 16, 24];

/// Centre of the lowest band, in Hz.
pub const LOWEST_BAND: f32 = 100.0;

/// The bank spans this many octaves, lowest centre to highest: 100 Hz to
/// 6.4 kHz, from a low voice's fundamental to the top of its "s".
pub const BANK_OCTAVES: f32 = 6.0;

/// Where Sibilance's high-pass opens, in Hz.
const SIBILANCE_HZ: f32 = 6000.0;

/// The unvoiced detector compares the voice's level above this with its
/// whole level.
const HISS_HZ: f32 = 4000.0;

/// How much of the voice's level must lie above [`HISS_HZ`] before the
/// detector starts calling it unvoiced, and by how much it's sure.
const HISS_FROM: f32 = 0.3;
const HISS_TO: f32 = 0.6;

/// Peak level of the noise Unvoiced swaps in (about -11 dBFS RMS).
const NOISE_LEVEL: f32 = 0.5;

/// Wet level: the bank's output with every follower at full scale would be
/// about the carrier's own level divided by √bands, so this times √bands
/// brings a voice at a speaking level up to sit with the carrier.
const MAKEUP: f32 = 4.0;

/// Sibilance at full is the voice's highs at this gain, level with the
/// vocoded voice from an even carrier.
const SIBILANCE_GAIN: f32 = 2.0;

/// How long the bank fades out, and back in, to change its band count.
const SWITCH_MS: f32 = 6.0;

/// The bank's synthesis tuning follows Formant in steps this many samples
/// apart.
const RETUNE_EVERY: usize = 16;

/// Flatten's carrier followers catch a band's peak at once, so a band a
/// new note fills is turned down before it can blast, and let go as fast as
/// the voice's bands do by default, so the bands an old chord leaves soon
/// stop holding up the level the rest are brought to.
const EVEN_ATTACK_MS: f32 = 0.5;
const EVEN_RELEASE_MS: f32 = 40.0;

/// Flatten at full turns the bank up this much (+3 dB): evening the carrier
/// out takes away what its loud low partials leaked into the voice's bands
/// through the filters' skirts, and this puts it back. Measured, like
/// [`MAKEUP`], with speech and with formant vowels on saw and triangle chords.
const FLATTEN_MAKEUP: f32 = 1.41;

/// How long Flatten listens to the voice to weigh the bands, in ms.
const VOICE_SPECTRUM_MS: f32 = 1000.0;

/// Flatten lifts a carrier band at most this far (-40 dB) below the
/// carrier's average band, so an empty band isn't raised into noise...
const EVEN_FLOOR: f32 = 0.01;

/// ...and nothing below this level (-100 dBFS) is lifted at all.
const EVEN_SILENCE: f32 = 1e-5;

/// A band rising this far past its peak follower between two aims has its
/// lift held back.
const EVEN_GUARD: f32 = 1.25;

/// Flatten tips the carrier up 6 dB an octave from 200 Hz, by up to this
/// much (+30 dB) at the top of the bank: a first-order shelf.
const EMPHASIS: f32 = 31.0;
const EMPHASIS_HZ: f32 = 6400.0;

/// How long the voice's own highs take to fade out on Freeze, and back in.
const FREEZE_FADE_MS: f32 = 3.0;

/// A gate at Freeze is high from here.
const GATE_THRESHOLD: f32 = 0.5;

/// Two cascaded sections each this much sharper than the pair should be
/// give the pair the bandwidth wanted (√(√2 - 1)).
const CASCADE_Q: f32 = 0.643_594_3;

/// Centre of band `band` of `bands`, before Formant, in Hz.
pub fn band_centre(band: usize, bands: usize) -> f32 {
    LOWEST_BAND * 2f32.powf(BANK_OCTAVES * band as f32 / (bands - 1).max(1) as f32)
}

/// The Q of each section of a band, for `bands` bands: the pair's -3 dB
/// edges land halfway to the neighbouring centres.
fn section_q(bands: usize) -> f32 {
    let ratio = 2f32.powf(BANK_OCTAVES / (bands - 1).max(1) as f32);
    CASCADE_Q * ratio.sqrt() / (ratio - 1.0)
}

/// The band count a Bands parameter value selects.
fn band_count(param: f32) -> usize {
    BAND_COUNTS[(param.round().max(0.0) as usize).min(BAND_COUNTS.len() - 1)]
}

/// Coefficients of a TPT state-variable filter (Zavalishin; Simper's form).
#[derive(Clone, Copy, Debug, Default)]
struct SvfCoefficients {
    a1: f32,
    a2: f32,
    a3: f32,
    /// 1/Q, which also scales the band-pass to a peak of unity.
    k: f32,
}

impl SvfCoefficients {
    fn new(cutoff_hz: f32, q: f32, sample_rate: f32) -> Self {
        let g = (PI * cutoff_hz.clamp(10.0, sample_rate * 0.45) / sample_rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2, k }
    }
}

/// One state-variable section's state.
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Advances the filter by one sample: (band-pass at unity peak, high-pass).
    #[inline]
    fn tick(&mut self, c: &SvfCoefficients, x: f32) -> (f32, f32) {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        (c.k * v1, x - c.k * v1 - v2)
    }

    #[inline]
    fn band(&mut self, c: &SvfCoefficients, x: f32) -> f32 {
        self.tick(c, x).0
    }

    #[inline]
    fn high(&mut self, c: &SvfCoefficients, x: f32) -> f32 {
        self.tick(c, x).1
    }
}

/// Two identical band-pass sections in a row.
#[inline]
fn band_pair(sections: &mut [Svf; 2], c: &SvfCoefficients, x: f32) -> f32 {
    let [first, second] = sections;
    second.band(c, first.band(c, x))
}

/// An envelope follower: a rectifier into a one-pole that rises at one rate
/// and falls at another, as on an analog vocoder's band.
#[derive(Clone, Copy, Debug, Default)]
struct Follower {
    level: f32,
}

impl Follower {
    #[inline]
    fn follow(&mut self, x: f32, attack: f32, release: f32) -> f32 {
        let x = x.abs();
        let rate = if x > self.level { attack } else { release };
        self.level = flush(self.level + rate * (x - self.level));
        self.level
    }
}

/// The per-sample coefficient of a one-pole that covers 63% of a step in
/// `ms` milliseconds.
fn one_pole(ms: f32, sample_rate: f32) -> f32 {
    1.0 - (-1000.0 / (ms.max(0.01) * sample_rate)).exp()
}

/// One band: the voice's side and the carrier's side.
#[derive(Clone, Copy, Debug)]
struct Band {
    analysis: SvfCoefficients,
    synthesis: SvfCoefficients,
    voice: [Svf; 2],
    carrier: [Svf; 2],
    follower: Follower,
    /// How loud the carrier is in this band, for Flatten.
    carrier_level: Follower,
    /// Flatten's gain on the carrier's band, and its step per sample toward
    /// the next one.
    evening: f32,
    evening_step: f32,
    /// How far the pre-emphasis lifts this band's centre: Flatten's gain
    /// on what the carrier itself has here is `evening · tipped`.
    tipped: f32,
    /// The voice's level in this band over the last second or so.
    voice_spectrum: f32,
    /// The loudest the band may be before its lift is held back.
    ceiling: f32,
}

impl Default for Band {
    fn default() -> Self {
        Self {
            analysis: SvfCoefficients::default(),
            synthesis: SvfCoefficients::default(),
            voice: [Svf::default(); 2],
            carrier: [Svf::default(); 2],
            follower: Follower::default(),
            carrier_level: Follower::default(),
            evening: 1.0,
            evening_step: 0.0,
            tipped: 1.0,
            ceiling: 0.0,
            voice_spectrum: 0.0,
        }
    }
}

/// The slope of `levels` (amplitudes, one per band of a bank of `bands`) in
/// dB per octave: a least-squares line through the bands loud enough to hear.
fn tilt(levels: impl Iterator<Item = f32>, bands: usize) -> f32 {
    let octaves_per_band = BANK_OCTAVES / (bands - 1).max(1) as f32;
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for (band, level) in levels.enumerate().take(bands) {
        if level < EVEN_SILENCE {
            continue;
        }
        let (x, y) = (band as f32 * octaves_per_band, 20.0 * level.log10());
        n += 1.0;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let spread = n * sxx - sx * sx;
    if n < 2.0 || spread <= 1e-6 {
        return 0.0;
    }
    (n * sxy - sx * sy) / spread
}

/// The vocoder.
///
/// # Ports
///
/// - **Carrier** (Audio, Input): the sound that's played, a synth chord.
///   A polyphonic cable is summed.
/// - **Modulator** (Audio, Input): the sound whose spectrum it wears, a voice.
/// - **Formant** (Control, Input): adds to the Formant knob, ±1 an octave.
/// - **Freeze** (Gate, Input): high holds the bands; patched, it takes over
///   from the switch.
/// - **Out L** / **Out R** (Audio, Output): the bands, alternating sides.
///
/// # Parameters
///
/// - **Formant** (−1..+1 octave): the carrier's bands against the voice's.
/// - **Bands** (8 / 16 / 24).
/// - **Freeze** (toggle): holds every band's follower where it is.
/// - **Attack** and **Release** of the band followers.
/// - **Sibilance**, **Unvoiced**, **Width** and **Mix**.
/// - **Flatten** (0–100%): evens out the carrier's bands before the voice's
///   levels are put on them.
/// - **Level** (−24..+12 dB): the output's gain, dry and wet alike.
pub struct Vocoder {
    sample_rate: f32,
    bands: [Band; MAX_BANDS],
    /// The band count the bank is running.
    active: usize,
    /// Fades the bank out and in again around a change of band count.
    switch_gain: f32,
    /// The Formant the synthesis bank is tuned to, in octaves.
    tuned_formant: f32,
    /// Counts samples to the next retune.
    retune_in: usize,
    sibilance_filter: [Svf; 2],
    sibilance_coefficients: [SvfCoefficients; 2],
    hiss_filter: Svf,
    hiss_coefficients: SvfCoefficients,
    /// The pre-emphasis's one-pole low-pass state, and its coefficient.
    emphasis: f32,
    emphasis_coefficient: f32,
    hiss_level: Follower,
    voice_level: Follower,
    /// How unvoiced the voice is right now, 0 to 1.
    unvoiced: f32,
    /// Whether Freeze held the bands on the last sample.
    frozen: bool,
    /// How far the voice's own highs are let through: 0 while frozen, 1
    /// otherwise, gliding between.
    listening: f32,
    noise: NoiseFloor,
    formant: SmoothedValue,
    sibilance: SmoothedValue,
    unvoiced_amount: SmoothedValue,
    width: SmoothedValue,
    mix: SmoothedValue,
    flatten: SmoothedValue,
    /// Level, as a gain.
    level: SmoothedValue,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Vocoder {
    pub const PORT_CARRIER: usize = 0;
    pub const PORT_MODULATOR: usize = 1;
    pub const PORT_FORMANT: usize = 2;
    pub const PORT_FREEZE: usize = 3;
    pub const PORT_OUT_L: usize = 0;
    pub const PORT_OUT_R: usize = 1;

    pub const PARAM_FORMANT: usize = 0;
    pub const PARAM_BANDS: usize = 1;
    pub const PARAM_FREEZE: usize = 2;
    pub const PARAM_ATTACK: usize = 3;
    pub const PARAM_RELEASE: usize = 4;
    pub const PARAM_SIBILANCE: usize = 5;
    pub const PARAM_UNVOICED: usize = 6;
    pub const PARAM_WIDTH: usize = 7;
    pub const PARAM_MIX: usize = 8;
    pub const PARAM_FLATTEN: usize = 9;
    pub const PARAM_LEVEL: usize = 10;

    /// Readout: the voice's level in each band, in amplitude, from 0 up to
    /// [`MAX_BANDS`].
    pub const READOUT_BANDS: usize = 0;
    /// Readout: how many bands are running.
    pub const READOUT_COUNT: usize = MAX_BANDS;
    /// Readout: the Formant shift heard, knob and CV, in octaves.
    pub const READOUT_FORMANT: usize = MAX_BANDS + 1;
    /// Readout: how unvoiced the voice is (0 to 1), before the Unvoiced knob.
    pub const READOUT_UNVOICED: usize = MAX_BANDS + 2;
    /// Readout: the voice's level above the hiss detector's high-pass.
    pub const READOUT_HIGHS: usize = MAX_BANDS + 3;
    /// Readout: 1 while Freeze holds the bands, by the switch or the gate.
    pub const READOUT_FROZEN: usize = MAX_BANDS + 4;
    /// Readout: the carrier's own tilt across the bank, in dB per octave.
    pub const READOUT_TILT: usize = MAX_BANDS + 5;
    /// Readout: the carrier's tilt as the bank plays it, after Flatten.
    pub const READOUT_EVENED: usize = MAX_BANDS + 6;

    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let mut vocoder = Self {
            sample_rate,
            bands: [Band::default(); MAX_BANDS],
            active: 16,
            switch_gain: 1.0,
            tuned_formant: 0.0,
            retune_in: 0,
            sibilance_filter: [Svf::default(); 2],
            sibilance_coefficients: [SvfCoefficients::default(); 2],
            hiss_filter: Svf::default(),
            hiss_coefficients: SvfCoefficients::default(),
            hiss_level: Follower::default(),
            emphasis: 0.0,
            emphasis_coefficient: 0.0,
            voice_level: Follower::default(),
            unvoiced: 0.0,
            frozen: false,
            listening: 1.0,
            noise: NoiseFloor::new(NOISE_LEVEL),
            formant: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            sibilance: SmoothedValue::with_default_smoothing(0.25, sample_rate),
            unvoiced_amount: SmoothedValue::with_default_smoothing(0.25, sample_rate),
            width: SmoothedValue::with_default_smoothing(0.5, sample_rate),
            mix: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            flatten: SmoothedValue::with_default_smoothing(0.0, sample_rate),
            level: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            ports: vec![
                PortDefinition::input_with_default("carrier", "Carrier", SignalType::Audio, 0.0)
                    .describe("The sound that's played, a synth chord; a polyphonic cable is summed"),
                PortDefinition::input_with_default("modulator", "Modulator", SignalType::Audio, 0.0)
                    .describe("The sound whose spectrum the carrier wears, usually a voice"),
                PortDefinition::input_with_default("formant_cv", "Formant", SignalType::Control, 0.0)
                    .describe("Adds to the Formant knob: +1 moves the carrier's bands an octave up"),
                PortDefinition::input_with_default("freeze", "Freeze", SignalType::Gate, 0.0)
                    .describe("While high, the bands hold the voice where it is; patched, it takes over from the switch"),
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("The odd bands, toward the left as Width opens"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("The even bands, toward the right as Width opens"),
            ],
            parameters: vec![
                ParameterDefinition::new("formant", "Formant", -1.0, 1.0, 0.0, ParameterDisplay::linear("oct"))
                    .describe("Moves the carrier's bands against the voice's: up for a smaller throat, down for a larger"),
                ParameterDefinition::choice("bands", "Bands", &["8", "16", "24"], 1)
                    .describe("How many bands the spectrum is split into: fewer is rougher and more robotic, more is clearer"),
                // Freeze before Attack: its jack must come before any knob-only row
                ParameterDefinition::toggle("freeze", "Freeze", false)
                    .describe("Holds every band where it is, so a vowel lasts as long as the chord does"),
                ParameterDefinition::new("attack", "Attack", 0.5, 200.0, 4.0, ParameterDisplay::logarithmic("ms"))
                    .describe("How fast a band opens when the voice gets louder there"),
                ParameterDefinition::new("release", "Release", 5.0, 2000.0, 40.0, ParameterDisplay::logarithmic("ms"))
                    .describe("How fast a band closes again: short for speech, long for a choir smear"),
                ParameterDefinition::normalized("sibilance", "Sibilance", 0.25)
                    .describe("Lets the voice's own highs through, so consonants stay readable"),
                ParameterDefinition::normalized("unvoiced", "Unvoiced", 0.25)
                    .describe("Swaps the carrier for noise while the voice hisses, so 's' and 't' come through"),
                ParameterDefinition::normalized("width", "Width", 0.5)
                    .describe("Spreads the bands, alternating left and right"),
                ParameterDefinition::normalized("mix", "Mix", 1.0)
                    .describe("Blend from the carrier alone (0) to the vocoded sound (1)"),
                ParameterDefinition::normalized("flatten", "Flatten", 0.0)
                    .describe("Evens out the carrier's own spectrum first, so a dull pad speaks as clearly as a bright saw"),
                ParameterDefinition::new("level", "Level", -24.0, 12.0, 0.0, ParameterDisplay::linear("dB"))
                    .describe("How loud it comes out: set the part's loudness here, since Flatten makes the carrier's matter less"),
            ],
        };
        vocoder.tune_all();
        vocoder
    }

    /// Tunes both banks, the sibilance and the hiss filters for the band
    /// count and sample rate, and clears every filter.
    fn tune_all(&mut self) {
        let q = section_q(self.active);
        for (index, band) in self.bands.iter_mut().enumerate().take(self.active) {
            band.analysis = SvfCoefficients::new(band_centre(index, self.active), q, self.sample_rate);
        }
        self.retune_synthesis(self.tuned_formant);
        for band in &mut self.bands {
            band.voice = [Svf::default(); 2];
            band.carrier = [Svf::default(); 2];
            band.follower = Follower::default();
            band.carrier_level = Follower::default();
            band.evening = 1.0;
            band.evening_step = 0.0;
            band.tipped = 1.0;
            band.ceiling = 0.0;
            band.voice_spectrum = 0.0;
        }
        // A fourth-order Butterworth high-pass
        self.sibilance_coefficients = [
            SvfCoefficients::new(SIBILANCE_HZ, 0.541_196_1, self.sample_rate),
            SvfCoefficients::new(SIBILANCE_HZ, 1.306_563, self.sample_rate),
        ];
        self.hiss_coefficients = SvfCoefficients::new(HISS_HZ, FRAC_1_SQRT_2, self.sample_rate);
        self.emphasis_coefficient = 1.0 - (-2.0 * PI * EMPHASIS_HZ / self.sample_rate).exp();
    }

    /// Tunes the carrier's bank `formant` octaves from the voice's.
    fn retune_synthesis(&mut self, formant: f32) {
        let q = section_q(self.active);
        let shift = 2f32.powf(formant);
        for (index, band) in self.bands.iter_mut().enumerate().take(self.active) {
            band.synthesis = SvfCoefficients::new(band_centre(index, self.active) * shift, q, self.sample_rate);
        }
        self.tuned_formant = formant;
    }

    /// Sets each carrier band's Flatten gain gliding, over the next
    /// [`RETUNE_EVERY`] samples, `flatten` of the way (in dB) from 1 to the
    /// gain that brings the band to the carrier's average band level.
    ///
    /// The average is weighted by where the voice has been over the last
    /// second or so, so evening the carrier out keeps the vocoder about as
    /// loud as it was: a chord and a voice are both loudest low down, and an
    /// even average would turn the voice's own bands down. Frozen, the
    /// weights hold with the bands: they would otherwise go on settling
    /// towards the held vowel for a second, and the held chord would swell
    /// or fade as they did.
    ///
    /// The bands were measured after the pre-emphasis, which is taken back
    /// out here: it only keeps the bands apart, and the levels they end up
    /// at are the carrier's own, evened.
    fn aim_evening(&mut self, flatten: f32) {
        let (active, shift, sample_rate) = (self.active, 2f32.powf(self.tuned_formant), self.sample_rate);
        let bands = &mut self.bands[..active];
        let listen = if self.frozen { 0.0 } else { 1.0 - (-(RETUNE_EVERY as f32) * 1000.0 / (VOICE_SPECTRUM_MS * sample_rate)).exp() };
        for band in bands.iter_mut() {
            band.voice_spectrum = flush(band.voice_spectrum + listen * (band.follower.level - band.voice_spectrum));
        }
        let aim = |band: &mut Band, target: f32| band.evening_step = (target - band.evening) / RETUNE_EVERY as f32;
        if flatten <= 0.0 {
            for band in bands {
                band.tipped = 1.0;
                aim(band, 1.0);
            }
            return;
        }
        let tip = flatten * EMPHASIS;
        // Each band's weight is the voice's power there, plus a little for
        // every band so a silent voice weighs them all alike
        let voice_power = bands.iter().map(|b| b.voice_spectrum * b.voice_spectrum).sum::<f32>();
        let even_weight = 0.01 * voice_power / active as f32 + 1e-12;
        let (mut weighed, mut weights) = (0.0, 0.0);
        for (index, band) in bands.iter_mut().enumerate() {
            // The shelf's gain at the band's centre: |1 + tip · jw / (jw + wc)|
            let w = (band_centre(index, active) * shift).min(sample_rate * 0.45) / EMPHASIS_HZ;
            band.tipped = ((w * w * (1.0 + tip) * (1.0 + tip) + 1.0) / (w * w + 1.0)).sqrt();
            let own = band.carrier_level.level / band.tipped;
            let weight = band.voice_spectrum * band.voice_spectrum + even_weight;
            weighed += weight * own * own;
            weights += weight;
        }
        let average = (weighed / weights).sqrt();
        let floor = (average * EVEN_FLOOR).max(EVEN_SILENCE);
        for band in bands {
            let lift = average / (band.carrier_level.level / band.tipped).max(floor);
            band.ceiling = EVEN_GUARD * band.carrier_level.level;
            aim(band, (lift * FLATTEN_MAKEUP).powf(flatten) / band.tipped);
        }
    }
}

const FRAC_1_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;

impl Default for Vocoder {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Vocoder {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "fx.vocoder",
            name: "Vocoder",
            category: ModuleCategory::Effect,
            description: "Puts a voice's spectrum on a synth: a bank of band-passes and followers",
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
        for smoothed in [&mut self.formant, &mut self.sibilance, &mut self.unvoiced_amount, &mut self.width, &mut self.mix, &mut self.flatten, &mut self.level] {
            smoothed.set_sample_rate(sample_rate);
        }
        self.tune_all();
    }

    fn process(&mut self, inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let sample_rate = self.sample_rate;
        let attack = one_pole(params[Self::PARAM_ATTACK], sample_rate);
        let release = one_pole(params[Self::PARAM_RELEASE], sample_rate);
        // The hiss detector listens as fast as speech moves
        let (hiss_attack, hiss_release) = (one_pole(2.0, sample_rate), one_pole(25.0, sample_rate));
        let (even_attack, even_release) = (one_pole(EVEN_ATTACK_MS, sample_rate), one_pole(EVEN_RELEASE_MS, sample_rate));
        let switch_step = 1.0 / (SWITCH_MS * 0.001 * sample_rate);
        let listen_step = 1.0 / (FREEZE_FADE_MS * 0.001 * sample_rate);
        let freeze_in = connected_input(inputs, Self::PORT_FREEZE);
        let freeze_switch = params[Self::PARAM_FREEZE] > 0.5;
        let wanted = band_count(params[Self::PARAM_BANDS]);

        self.formant.set_target(params[Self::PARAM_FORMANT]);
        self.sibilance.set_target(params[Self::PARAM_SIBILANCE]);
        self.unvoiced_amount.set_target(params[Self::PARAM_UNVOICED]);
        self.width.set_target(params[Self::PARAM_WIDTH]);
        self.mix.set_target(params[Self::PARAM_MIX]);
        self.flatten.set_target(params[Self::PARAM_FLATTEN]);
        self.level.set_target(db_to_gain(params[Self::PARAM_LEVEL]));

        let carrier = &inputs[Self::PORT_CARRIER].samples;
        let modulator = &inputs[Self::PORT_MODULATOR].samples;
        let formant_cv = &inputs[Self::PORT_FORMANT].samples;
        let (left, right) = outputs.split_at_mut(1);
        let (out_l, out_r) = (&mut left[0].samples, &mut right[0].samples);

        for i in 0..context.block_size {
            let voice = modulator[i];
            let dry = carrier[i];

            // Fade out to change the band count, then back in
            if wanted != self.active {
                self.switch_gain -= switch_step;
                if self.switch_gain <= 0.0 {
                    self.switch_gain = 0.0;
                    self.active = wanted;
                    self.tune_all();
                }
            } else if self.switch_gain < 1.0 {
                self.switch_gain = (self.switch_gain + switch_step).min(1.0);
            }

            let formant = (self.formant.next() + formant_cv[i]).clamp(-1.0, 1.0);
            let flatten = self.flatten.next();
            if self.retune_in == 0 {
                if (formant - self.tuned_formant).abs() > 1e-4 {
                    self.retune_synthesis(formant);
                }
                self.aim_evening(flatten);
                self.retune_in = RETUNE_EVERY;
            }
            self.retune_in -= 1;

            // Frozen, every follower holds and the voice's own highs fade
            let frozen = match freeze_in {
                Some(gate) => gate.samples[i] >= GATE_THRESHOLD,
                None => freeze_switch,
            };
            self.frozen = frozen;
            self.listening = if frozen { (self.listening - listen_step).max(0.0) } else { (self.listening + listen_step).min(1.0) };

            // How much of the voice is hiss: noise stands in for the carrier
            let hissing = self.hiss_filter.high(&self.hiss_coefficients, voice);
            if !frozen {
                let hiss = self.hiss_level.follow(hissing, hiss_attack, hiss_release);
                let whole = self.voice_level.follow(voice, hiss_attack, hiss_release);
                let share = hiss / (whole + 1e-6);
                self.unvoiced = ((share - HISS_FROM) / (HISS_TO - HISS_FROM)).clamp(0.0, 1.0);
            }
            let swap = self.unvoiced * self.unvoiced_amount.next();
            // Flatten first tips the carrier up, so the bands it lifts most aren't
            // swamped by their louder neighbours through the filters' skirts
            self.emphasis = flush(self.emphasis + self.emphasis_coefficient * (dry - self.emphasis));
            let tipped = dry + flatten * EMPHASIS * (dry - self.emphasis);
            let excitation = tipped + swap * (self.noise.sample() - tipped);

            // Odd bands lean left and even bands right, at constant power
            let angle = FRAC_PI_4 * (1.0 - self.width.next());
            let (near, far) = (SQRT_2 * angle.cos(), SQRT_2 * angle.sin());

            let mut sides = [0.0f32; 2];
            for (index, band) in self.bands.iter_mut().enumerate().take(self.active) {
                let heard = band_pair(&mut band.voice, &band.analysis, voice);
                let level = if frozen { band.follower.level } else { band.follower.follow(heard, attack, release) };
                let played = band_pair(&mut band.carrier, &band.synthesis, excitation);
                band.carrier_level.follow(played, even_attack, even_release);
                band.evening += band.evening_step;
                // A band far louder than when its lift was set gets none of the
                // lift until the next aim, so a new note can't burst through it
                let mut evening = band.evening;
                if evening > 1.0 && played.abs() > band.ceiling {
                    evening = (evening * band.ceiling / played.abs()).max(1.0);
                }
                let y = played * evening * level;
                let side = index & 1;
                sides[side] += near * y;
                sides[1 - side] += far * y;
            }

            let [low_stage, high_stage] = &mut self.sibilance_filter;
            let highs = high_stage.high(&self.sibilance_coefficients[1], low_stage.high(&self.sibilance_coefficients[0], voice));
            let sibilance = SIBILANCE_GAIN * self.sibilance.next() * self.listening * highs;

            // Rectified, a band reads 2/π of its amplitude
            let wet_gain = MAKEUP * FRAC_PI_2 * (self.active as f32).sqrt() * self.switch_gain;
            let mix = self.mix.next();
            let level = self.level.next();
            out_l[i] = level * (dry * (1.0 - mix) + mix * (sides[0] * wet_gain + sibilance));
            out_r[i] = level * (dry * (1.0 - mix) + mix * (sides[1] * wet_gain + sibilance));
        }
    }

    fn reset(&mut self) {
        self.tune_all();
        self.hiss_filter = Svf::default();
        self.sibilance_filter = [Svf::default(); 2];
        self.hiss_level = Follower::default();
        self.voice_level = Follower::default();
        self.unvoiced = 0.0;
        self.frozen = false;
        self.listening = 1.0;
        self.emphasis = 0.0;
        for smoothed in [&mut self.formant, &mut self.sibilance, &mut self.unvoiced_amount, &mut self.width, &mut self.mix, &mut self.flatten, &mut self.level] {
            smoothed.reset(smoothed.target());
        }
    }

    fn key_inputs(&self) -> &'static [usize] {
        &[Self::PORT_MODULATOR]
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        for (value, band) in readout.values[Self::READOUT_BANDS..].iter_mut().zip(&self.bands).take(self.active) {
            *value = band.follower.level * FRAC_PI_2;
        }
        readout.values[Self::READOUT_COUNT] = self.active as f32;
        readout.values[Self::READOUT_FORMANT] = self.tuned_formant;
        readout.values[Self::READOUT_UNVOICED] = self.unvoiced;
        readout.values[Self::READOUT_HIGHS] = self.hiss_level.level * FRAC_PI_2;
        readout.values[Self::READOUT_FROZEN] = if self.frozen { 1.0 } else { 0.0 };
        let bands = &self.bands[..self.active];
        readout.values[Self::READOUT_TILT] = tilt(bands.iter().map(|b| b.carrier_level.level / b.tipped), self.active);
        readout.values[Self::READOUT_EVENED] = tilt(bands.iter().map(|b| b.carrier_level.level * b.evening), self.active);
        Some(readout)
    }
}

const _: () = assert!(Vocoder::READOUT_EVENED < MAX_READOUT);

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;

    /// Knobs: Formant, Bands, Freeze, Attack, Release, Sibilance, Unvoiced,
    /// Width, Mix, Flatten, Level.
    type Knobs = [f32; 11];

    /// The knobs, with Freeze off, Flatten at 0 and Level at 0 dB.
    fn knobs(formant: f32, bands: f32, sibilance: f32, unvoiced: f32, width: f32, mix: f32) -> Knobs {
        [formant, bands, 0.0, 4.0, 40.0, sibilance, unvoiced, width, mix, 0.0, 0.0]
    }

    /// Knobs with Freeze set and Flatten at `flatten`, and everything else
    /// plain: 16 bands, Sibilance and Unvoiced off, mono, all wet.
    fn knobs_with(freeze: bool, flatten: f32) -> Knobs {
        let mut k = knobs(0.0, 1.0, 0.0, 0.0, 0.0, 1.0);
        k[Vocoder::PARAM_FREEZE] = if freeze { 1.0 } else { 0.0 };
        k[Vocoder::PARAM_FLATTEN] = flatten;
        k
    }

    /// Plays `seconds` through a new vocoder at 48 kHz. See [`play_on`].
    fn play(
        seconds: f32,
        carrier: impl FnMut(usize) -> f32,
        voice: impl FnMut(usize) -> f32,
        formant_cv: f32,
        params: impl Fn(usize) -> Knobs,
    ) -> (Vec<f32>, Vec<f32>) {
        play_on(&mut Vocoder::new(), seconds, carrier, voice, formant_cv, params)
    }

    /// Plays `seconds` through `vocoder` at 48 kHz, the carrier and the voice
    /// given sample by sample, the knobs block by block. Returns (L, R).
    fn play_on(
        vocoder: &mut Vocoder,
        seconds: f32,
        carrier: impl FnMut(usize) -> f32,
        voice: impl FnMut(usize) -> f32,
        formant_cv: f32,
        params: impl Fn(usize) -> Knobs,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut stage = Stage { vocoder, freeze: None, after_block: &mut |_, _| {} };
        stage.play(seconds, carrier, voice, formant_cv, params)
    }

    /// A vocoder to play, with a gate at Freeze if it has one, and a look
    /// at it after every block.
    struct Stage<'a> {
        vocoder: &'a mut Vocoder,
        freeze: Option<&'a dyn Fn(usize) -> f32>,
        after_block: &'a mut dyn FnMut(usize, &Vocoder),
    }

    impl Stage<'_> {
        fn play(
            &mut self,
            seconds: f32,
            mut carrier: impl FnMut(usize) -> f32,
            mut voice: impl FnMut(usize) -> f32,
            formant_cv: f32,
            params: impl Fn(usize) -> Knobs,
        ) -> (Vec<f32>, Vec<f32>) {
            self.vocoder.prepare(SR, BLOCK);
            let ctx = ProcessContext::new(SR, BLOCK);
            let mut c = SignalBuffer::audio(BLOCK);
            let mut m = SignalBuffer::audio(BLOCK);
            let mut cv = SignalBuffer::control(BLOCK);
            cv.fill(formant_cv);
            let mut gate = match self.freeze {
                Some(_) => SignalBuffer::gate(BLOCK),
                None => SignalBuffer::unconnected(BLOCK, SignalType::Gate),
            };
            let mut outputs = vec![SignalBuffer::audio(BLOCK), SignalBuffer::audio(BLOCK)];
            let (mut left, mut right) = (Vec::new(), Vec::new());
            for b in 0..(seconds * SR) as usize / BLOCK {
                for j in 0..BLOCK {
                    let n = b * BLOCK + j;
                    c.samples[j] = carrier(n);
                    m.samples[j] = voice(n);
                    if let Some(freeze) = self.freeze {
                        gate.samples[j] = freeze(n);
                    }
                }
                self.vocoder.process(&[&c, &m, &cv, &gate], &mut outputs, &params(b), &ctx);
                left.extend_from_slice(&outputs[0].samples);
                right.extend_from_slice(&outputs[1].samples);
                (self.after_block)(b, self.vocoder);
            }
            (left, right)
        }
    }

    /// A bright carrier: a sawtooth at `hz`.
    fn saw(hz: f32) -> impl FnMut(usize) -> f32 {
        move |n| 0.5 * (2.0 * (n as f32 * hz / SR).fract() - 1.0)
    }

    /// An even carrier: every harmonic of `hz` up to 8 kHz at one level, so
    /// the vocoder's peaks are the voice's alone.
    fn buzz(hz: f32) -> impl FnMut(usize) -> f32 {
        let harmonics = (8000.0 / hz) as usize;
        move |n| (1..=harmonics).map(|k| 0.05 * (2.0 * PI * (k as f32 * hz * n as f32 / SR).fract()).sin()).sum()
    }

    /// White noise, uniform in ±1.
    fn white(seed: u32) -> impl FnMut() -> f32 {
        let mut x = seed;
        move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as i32 as f32 / 2_147_483_648.0
        }
    }

    /// A vowel-like voice: pink noise through band-passes at `formants`.
    fn vowel(formants: &'static [f32]) -> impl FnMut(usize) -> f32 {
        let mut noise = white(7);
        let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
        let coefficients: Vec<SvfCoefficients> = formants.iter().map(|&hz| SvfCoefficients::new(hz, 6.0, SR)).collect();
        let mut filters = vec![Svf::default(); formants.len()];
        move |_| {
            // Paul Kellett's economy pink noise
            let w = noise();
            b0 = 0.99765 * b0 + w * 0.099_046;
            b1 = 0.96300 * b1 + w * 0.296_516_4;
            b2 = 0.57000 * b2 + w * 1.052_691_3;
            let pink = 0.25 * (b0 + b1 + b2 + w * 0.1848);
            filters.iter_mut().zip(&coefficients).map(|(f, c)| f.band(c, pink)).sum()
        }
    }

    /// The level of `x` at `hz` (Goertzel), as a sine's amplitude.
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let w = 2.0 * PI * hz / SR;
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        let coeff = 2.0 * (w as f64).cos();
        for &v in x {
            let s = v as f64 + coeff * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        (2.0 * power.max(0.0).sqrt() / x.len() as f64) as f32
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn mono(left: &[f32], right: &[f32]) -> Vec<f32> {
        left.iter().zip(right).map(|(l, r)| 0.5 * (l + r)).collect()
    }

    #[test]
    fn info_ports_and_parameters() {
        let vocoder = Vocoder::new();
        assert_eq!(vocoder.info().id, "fx.vocoder");
        assert_eq!(vocoder.info().category, ModuleCategory::Effect);
        let ports = vocoder.ports();
        assert_eq!(ports.len(), 6);
        assert_eq!(ports[Vocoder::PORT_CARRIER].name, "Carrier");
        assert_eq!(ports[Vocoder::PORT_MODULATOR].name, "Modulator");
        assert_eq!(ports[Vocoder::PORT_FORMANT].name, "Formant");
        assert_eq!(ports[Vocoder::PORT_FREEZE].name, "Freeze");
        assert_eq!(ports[Vocoder::PORT_FREEZE].signal_type, SignalType::Gate);
        let names: Vec<&str> = vocoder.parameters().iter().map(|p| p.name).collect();
        assert_eq!(names, ["Formant", "Bands", "Freeze", "Attack", "Release", "Sibilance", "Unvoiced", "Width", "Mix", "Flatten", "Level"]);
    }

    #[test]
    fn bands_cover_the_bank_and_meet_at_their_edges() {
        for bands in BAND_COUNTS {
            assert!((band_centre(0, bands) - 100.0).abs() < 1e-3);
            assert!((band_centre(bands - 1, bands) - 6400.0).abs() < 0.5);
            // A tone halfway between two centres is -3 dB through each pair
            let between = (band_centre(3, bands) * band_centre(4, bands)).sqrt();
            let c = SvfCoefficients::new(band_centre(3, bands), section_q(bands), SR);
            let mut pair = [Svf::default(); 2];
            let out: Vec<f32> = (0..48000).map(|n| band_pair(&mut pair, &c, (2.0 * PI * between * n as f32 / SR).sin())).collect();
            let gain = rms(&out[24000..]) * SQRT_2;
            assert!((gain - FRAC_1_SQRT_2).abs() < 0.06, "{bands} bands: {gain} between centres");
        }
    }

    /// The output's level at each harmonic of 125 Hz up to 6 kHz, playing a
    /// buzz through a voice with formants at 500 Hz and 2 kHz.
    fn harmonics_of_a_vowel(bands: f32, formant: f32) -> Vec<(f32, f32)> {
        let (l, r) = play(2.0, buzz(125.0), vowel(&[500.0, 2000.0]), 0.0, |_| knobs(formant, bands, 0.0, 0.0, 0.0, 1.0));
        let out = mono(&l[48000..], &r[48000..]);
        (1..=48).map(|k| k as f32 * 125.0).map(|hz| (hz, level_at(&out, hz))).collect()
    }

    /// Checks the output peaks at `peaks`: the loudest harmonic is near one
    /// of them, and near each, the output is `contrast` times as loud as in
    /// the valley halfway between them.
    fn assert_peaks_at(levels: &[(f32, f32)], peaks: [f32; 2], contrast: f32, what: &str) {
        let near = |hz: f32, octaves: f32| {
            levels.iter().filter(|(h, _)| (h / hz).log2().abs() <= octaves).map(|(_, l)| *l).fold(0.0, f32::max)
        };
        let valley = near((peaks[0] * peaks[1]).sqrt(), 0.15);
        for peak in peaks {
            assert!(near(peak, 0.3) > contrast * valley, "{what}: {} near {peak} Hz against {valley} between", near(peak, 0.3));
        }
        let loudest = levels.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
        assert!(peaks.iter().any(|p| (loudest / p).log2().abs() <= 0.3), "{what}: the loudest harmonic is {loudest} Hz");
    }

    #[test]
    fn a_vowel_puts_the_carriers_peaks_on_its_formants() {
        // A voice with formants at 500 Hz and 2 kHz, on a buzz whose
        // harmonics fall on both formants and the valley between
        for bands in [0.0, 1.0, 2.0] {
            assert_peaks_at(&harmonics_of_a_vowel(bands, 0.0), [500.0, 2000.0], 3.0, &format!("bands {bands}"));
        }
    }

    #[test]
    fn formant_carries_the_peaks_up_an_octave() {
        assert_peaks_at(&harmonics_of_a_vowel(1.0, 1.0), [1000.0, 4000.0], 3.0, "up an octave");
        assert_peaks_at(&harmonics_of_a_vowel(1.0, -1.0), [250.0, 1000.0], 3.0, "down an octave");
    }

    #[test]
    fn formant_cv_adds_to_the_knob() {
        let knob_only = play(0.5, saw(125.0), vowel(&[700.0]), 0.0, |_| knobs(0.6, 1.0, 0.0, 0.0, 0.5, 1.0));
        let with_cv = play(0.5, saw(125.0), vowel(&[700.0]), 0.4, |_| knobs(0.2, 1.0, 0.0, 0.0, 0.5, 1.0));
        // Both knobs glide from 0, so compare once they've settled
        let tail = 12000..;
        let diff = knob_only.0[tail.clone()].iter().zip(&with_cv.0[tail.clone()]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(diff < 0.05 * rms(&knob_only.0[tail]), "differ by {diff}");
    }

    #[test]
    fn level_scales_both_outputs_and_nothing_else() {
        // Half wet, so the carrier and the vocoded voice are both in it, with
        // Flatten, Sibilance and Unvoiced all at work
        let at = |db: f32| {
            let mut k = knobs(0.3, 1.0, 0.5, 0.5, 0.8, 0.5);
            k[Vocoder::PARAM_FLATTEN] = 0.5;
            k[Vocoder::PARAM_LEVEL] = db;
            let mut vocoder = Vocoder::new();
            let out = play_on(&mut vocoder, 0.5, saw(125.0), vowel(&[500.0, 2000.0, 5000.0]), 0.0, |_| k);
            (out, bands_of(&vocoder))
        };
        let ((l0, r0), bands0) = at(0.0);
        assert!(rms(&l0) > 0.01 && rms(&r0) > 0.01);
        for db in [-24.0, -8.0, 12.0] {
            let ((l, r), bands) = at(db);
            let gain = db_to_gain(db);
            for (ours, unity) in l.iter().zip(&l0).chain(r.iter().zip(&r0)) {
                assert!((ours - gain * unity).abs() <= 1e-5 * gain.max(1.0), "{db} dB: {ours} against {unity}");
            }
            assert_eq!(bands, bands0, "{db} dB moved the bands");
        }
    }

    #[test]
    fn a_silent_voice_is_silence() {
        let (l, r) = play(0.5, saw(125.0), |_| 0.0, 0.0, |_| knobs(0.0, 1.0, 1.0, 1.0, 0.5, 1.0));
        assert_eq!(rms(&l), 0.0);
        assert_eq!(rms(&r), 0.0);
    }

    #[test]
    fn mix_at_zero_is_the_carrier() {
        let mut carrier = saw(220.0);
        let expected: Vec<f32> = (0..23808).map(&mut carrier).collect();
        let (l, r) = play(0.5, saw(220.0), vowel(&[700.0]), 0.0, |_| knobs(0.0, 1.0, 0.5, 0.5, 1.0, 0.0));
        // The Mix knob starts at its default, full, and glides down
        assert_eq!(&l[12000..], &expected[12000..]);
        assert_eq!(&r[12000..], &expected[12000..]);
    }

    #[test]
    fn hiss_comes_through_as_noise_only_with_unvoiced() {
        // No carrier at all, and a voice that's only an "s": noise high up
        let hiss = |seed| {
            let mut noise = white(seed);
            let c = SvfCoefficients::new(5000.0, 0.7, SR);
            let mut f = Svf::default();
            move |_| 0.3 * f.high(&c, noise())
        };
        let (l, _) = play(1.0, |_| 0.0, hiss(3), 0.0, |_| knobs(0.0, 1.0, 0.0, 1.0, 0.0, 1.0));
        assert!(rms(&l[24000..]) > 0.02, "unvoiced: {}", rms(&l[24000..]));
        let (l, _) = play(1.0, |_| 0.0, hiss(3), 0.0, |_| knobs(0.0, 1.0, 0.0, 0.0, 0.0, 1.0));
        assert!(rms(&l[24000..]) < 1e-6, "Unvoiced off: {}", rms(&l[24000..]));

        // A vowel isn't hiss: no noise for it, however far Unvoiced is up
        let mut vocoder = Vocoder::new();
        let (l, _) = play_on(&mut vocoder, 1.0, |_| 0.0, vowel(&[500.0, 1500.0]), 0.0, |_| knobs(0.0, 1.0, 0.0, 1.0, 0.0, 1.0));
        assert!(rms(&l[24000..]) < 0.002, "a vowel let noise in: {}", rms(&l[24000..]));
        assert!(vocoder.readout(&[]).unwrap().values[Vocoder::READOUT_UNVOICED] < 0.05);
    }

    #[test]
    fn sibilance_passes_the_voices_highs() {
        let tone = |n: usize| 0.3 * (2.0 * PI * 9000.0 * n as f32 / SR).sin();
        let (l, r) = play(0.5, |_| 0.0, tone, 0.0, |_| knobs(0.0, 1.0, 1.0, 0.0, 0.0, 1.0));
        let level = level_at(&l[12000..], 9000.0);
        assert!((level - SIBILANCE_GAIN * 0.3).abs() < 0.1, "9 kHz through at {level}");
        assert_eq!(l, r, "the highs sit in the middle");
        let (l, _) = play(0.5, |_| 0.0, tone, 0.0, |_| knobs(0.0, 1.0, 0.0, 0.0, 0.0, 1.0));
        assert!(rms(&l[12000..]) < 1e-4, "Sibilance off: {}", rms(&l[12000..]));
    }

    #[test]
    fn width_parts_odd_and_even_bands() {
        // Carrier and voice both on the lowest of 8 bands, which leans left
        let low = |n: usize| 0.4 * (2.0 * PI * 100.0 * n as f32 / SR).sin();
        let (l, r) = play(1.0, low, low, 0.0, |_| knobs(0.0, 1.0, 0.0, 0.0, 0.0, 1.0));
        assert!(l[12000..].iter().zip(&r[12000..]).all(|(a, b)| (a - b).abs() < 1e-6), "Width 0 is mono");
        let (l, r) = play(1.0, low, low, 0.0, |_| knobs(0.0, 0.0, 0.0, 0.0, 1.0, 1.0));
        let (l, r) = (rms(&l[24000..]), rms(&r[24000..]));
        assert!(l > 5.0 * r, "left {l}, right {r}");
    }

    #[test]
    fn levels_hold_across_band_counts() {
        let mut levels = Vec::new();
        for bands in [0.0, 1.0, 2.0] {
            let (l, r) = play(2.0, saw(110.0), vowel(&[600.0, 1200.0, 2600.0]), 0.0, |_| knobs(0.0, bands, 0.0, 0.0, 0.0, 1.0));
            levels.push(rms(&mono(&l[48000..], &r[48000..])));
        }
        let voice: Vec<f32> = (0..96000).map(vowel(&[600.0, 1200.0, 2600.0])).collect();
        println!("voice rms {}, carrier rms {}, out {levels:?}", rms(&voice), 0.5 / 3f32.sqrt());
        for level in &levels {
            assert!(*level > 0.05 && *level < 0.5, "{levels:?}");
            assert!((20.0 * (level / levels[1]).log10()).abs() < 3.0, "{levels:?}");
        }
    }

    /// The steepest step in `x` against what a tone at `hz` could make at
    /// the level around it: 1 for a tone at a steady level. A click is a
    /// jump far steeper than the sound can move on its own.
    fn steepest(x: &[f32], hz: f32) -> f32 {
        let slope = 2.0 * PI * hz / SR;
        x.chunks(240)
            .map(|w| {
                let peak = w.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                let step = w.windows(2).fold(0.0f32, |m, p| m.max((p[1] - p[0]).abs()));
                step / (slope * peak + 1e-4)
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn changing_bands_or_formant_does_not_click() {
        // Steady for half a second, then a change every quarter: the bank
        // fades out and in around each change of band count, and Formant
        // glides the carrier's bands, which sweep past the tone
        let changes = |block: usize| {
            let step = (block * BLOCK) / 12000;
            let bands = [1.0, 1.0, 2.0, 0.0, 1.0, 2.0][step.min(5)];
            let formant = [0.0, 0.0, 0.5, -0.5, 1.0, 0.0][step.min(5)];
            knobs(formant, bands, 0.0, 0.0, 0.5, 1.0)
        };
        let mut vocoder = Vocoder::new();
        let tone = |n: usize| 0.4 * (2.0 * PI * 220.0 * n as f32 / SR).sin();
        let (l, r) = play_on(&mut vocoder, 1.5, tone, vowel(&[700.0, 1100.0]), 0.0, changes);
        let steady = steepest(&l[12000..24000], 220.0).max(steepest(&r[12000..24000], 220.0));
        assert!(steady < 1.2, "steady: {steady}");
        for side in [&l, &r] {
            let changing = steepest(&side[24000..], 220.0);
            assert!(changing < 3.0, "clicked: {changing} times a tone's steepest");
        }
        assert!(l.iter().chain(&r).all(|s| s.is_finite()));
        assert_eq!(vocoder.readout(&[]).unwrap().values[Vocoder::READOUT_COUNT], 24.0);
    }

    #[test]
    fn readout_shows_the_voices_spectrum() {
        let mut vocoder = Vocoder::new();
        play_on(&mut vocoder, 1.0, |_| 0.0, vowel(&[500.0, 2000.0]), 0.0, |_| knobs(0.0, 1.0, 0.0, 0.0, 0.0, 1.0));
        let values = vocoder.readout(&[]).unwrap().values;
        assert_eq!(values[Vocoder::READOUT_COUNT], 16.0);
        // The loudest band is one nearest a formant
        let loudest = (0..16).max_by(|&a, &b| values[a].total_cmp(&values[b])).unwrap();
        let centre = band_centre(loudest, 16);
        let near = |hz: f32| (centre / hz).log2().abs() < 0.25;
        assert!(near(500.0) || near(2000.0), "loudest band at {centre} Hz");
        assert!(values[16..MAX_BANDS].iter().all(|&v| v == 0.0), "bands past 16 are silent");
    }

    /// The bands' levels in a readout, for the 16-band bank.
    fn bands_of(vocoder: &Vocoder) -> [f32; 16] {
        let values = vocoder.readout(&[]).unwrap().values;
        std::array::from_fn(|band| values[Vocoder::READOUT_BANDS + band])
    }

    #[test]
    fn freeze_holds_the_bands_exactly_and_lets_go_without_a_jump() {
        // "Ahh" for 0.4 s, frozen until 1 s while the voice says "ee" and
        // then stops, and let go at 1 s with the voice silent
        let (freeze_from, freeze_to) = (75, 188);
        let (mut ahh, mut ee) = (vowel(&[700.0, 1100.0]), vowel(&[300.0, 2300.0]));
        let voice = move |n: usize| match n {
            0..24000 => ahh(n),
            24000..36000 => ee(n),
            _ => 0.0,
        };
        let mut levels = Vec::new();
        let mut frozen = Vec::new();
        let mut vocoder = Vocoder::new();
        let tone = |n: usize| 0.4 * (2.0 * PI * 220.0 * n as f32 / SR).sin();
        let (l, r) = Stage {
            vocoder: &mut vocoder,
            freeze: None,
            after_block: &mut |_, v| {
                levels.push(bands_of(v));
                frozen.push(v.readout(&[]).unwrap().values[Vocoder::READOUT_FROZEN]);
            },
        }
        .play(1.5, tone, voice, 0.0, |b| knobs_with((freeze_from..freeze_to).contains(&b), 0.0));

        // Held to the last bit through the new vowel and the silence
        let held = levels[freeze_from - 1];
        assert!(held.iter().any(|&level| level > 0.01), "nothing to hold: {held:?}");
        for b in freeze_from..freeze_to {
            assert_eq!(levels[b], held, "block {b} moved");
            assert_eq!(frozen[b], 1.0);
        }
        assert_eq!(frozen[freeze_to], 0.0);
        // ...and still sounding, though the voice has stopped
        let before = rms(&mono(&l[14000..19000], &r[14000..19000]));
        let during = rms(&mono(&l[40000..47000], &r[40000..47000]));
        assert!(during > 0.7 * before, "frozen at {during}, against {before} before");

        // Let go, the bands fall away from where they were held at the
        // Release rate: no step, and silence soon after
        let after = levels[freeze_to];
        for (band, (&was, &now)) in held.iter().zip(&after).enumerate() {
            assert!(now < was || was == 0.0, "band {band} rose from {was} to {now}");
            assert!(now > 0.6 * was, "band {band} dropped from {was} to {now} in one block");
        }
        assert!(levels.last().unwrap().iter().all(|&level| level < 1e-4));
        for side in [&l, &r] {
            let steepest = steepest(&side[24000..60000], 220.0);
            assert!(steepest < 3.0, "clicked: {steepest} times a tone's steepest");
        }
    }

    #[test]
    fn frozen_and_flattened_a_chord_holds_its_level() {
        // A second and a half of "ooh", a quarter second of "ahh", then
        // frozen on the "ahh" as the voice stops: a held vowel on a dull
        // pad, as Held Vowels plays one. Flatten's weights were still mostly
        // "ooh" when it froze, and must hold there with the bands
        let freeze_from = (1.75 * SR) as usize / BLOCK;
        let (mut ooh, mut ahh) = (vowel(&[300.0, 870.0]), vowel(&[730.0, 1090.0]));
        let voice = move |n: usize| match n {
            0..72000 => ooh(n),
            72000..84000 => ahh(n),
            _ => 0.0,
        };
        let (l, r) = play(3.5, dull(110.0), voice, 0.0, |b| knobs_with(b >= freeze_from, 1.0));
        let out = mono(&l, &r);
        let soon = rms(&out[88000..96000]);
        let later = rms(&out[155000..167000]);
        let drift = 20.0 * (later / soon).log10();
        assert!(drift.abs() < 0.5, "the held vowel moved {drift:.2} dB");
    }

    #[test]
    fn freeze_gate_takes_over_from_the_switch() {
        // The gate high for the second quarter-second, the switch on all along
        let gate = |n: usize| if (12000..24000).contains(&n) { 1.0 } else { 0.0 };
        let mut states = Vec::new();
        let mut levels = Vec::new();
        Stage {
            vocoder: &mut Vocoder::new(),
            freeze: Some(&gate),
            after_block: &mut |_, v| {
                states.push(v.readout(&[]).unwrap().values[Vocoder::READOUT_FROZEN]);
                levels.push(bands_of(v));
            },
        }
        .play(0.75, saw(125.0), vowel(&[500.0, 2000.0]), 0.0, |_| knobs_with(true, 0.0));
        let block_at = |n: usize| n / BLOCK;
        assert_eq!(states[block_at(6000)], 0.0, "the gate is low: the switch is ignored");
        assert_ne!(levels[block_at(6000)], levels[block_at(6000) + 1], "the bands follow the voice");
        assert_eq!(states[block_at(18000)], 1.0, "the gate is high");
        assert_eq!(levels[block_at(18000)], levels[block_at(18000) + 1], "the bands hold");
        assert_eq!(states[block_at(30000)], 0.0, "the gate fell");

        // Unpatched, the switch freezes
        let mut vocoder = Vocoder::new();
        play_on(&mut vocoder, 0.25, saw(125.0), vowel(&[500.0]), 0.0, |_| knobs_with(true, 0.0));
        assert_eq!(vocoder.readout(&[]).unwrap().values[Vocoder::READOUT_FROZEN], 1.0);
    }

    #[test]
    fn freeze_mutes_the_voices_own_highs() {
        // Sibilance passes the voice's highs, but a frozen vowel is the held
        // bands alone: what the voice does next isn't heard
        let tone = |n: usize| 0.3 * (2.0 * PI * 9000.0 * n as f32 / SR).sin();
        let sibilant = |freeze| {
            let mut k = knobs(0.0, 1.0, 1.0, 0.0, 0.0, 1.0);
            k[Vocoder::PARAM_FREEZE] = if freeze { 1.0 } else { 0.0 };
            k
        };
        let (l, _) = play(0.5, |_| 0.0, tone, 0.0, |_| sibilant(false));
        assert!(rms(&l[12000..]) > 0.3);
        let (l, _) = play(0.5, |_| 0.0, tone, 0.0, |_| sibilant(true));
        assert!(rms(&l[12000..]) < 1e-4, "frozen, the highs still pass: {}", rms(&l[12000..]));
    }

    /// A dull carrier: every harmonic of `hz` up to 8 kHz falling 12 dB an
    /// octave, like a pad through a low-pass. Its 2 kHz is 48 dB below its
    /// fundamental.
    fn dull(hz: f32) -> impl FnMut(usize) -> f32 {
        let harmonics = (8000.0 / hz) as usize;
        move |n| (1..=harmonics).map(|k| 0.4 / (k * k) as f32 * (2.0 * PI * (k as f32 * hz * n as f32 / SR).fract()).sin()).sum()
    }

    /// Like [`harmonics_of_a_vowel`], on any carrier and with Flatten.
    fn harmonics_with_flatten(carrier: impl FnMut(usize) -> f32, flatten: f32) -> Vec<(f32, f32)> {
        let (l, r) = play(2.0, carrier, vowel(&[500.0, 2000.0]), 0.0, |_| knobs_with(false, flatten));
        let out = mono(&l[48000..], &r[48000..]);
        (1..=48).map(|k| k as f32 * 125.0).map(|hz| (hz, level_at(&out, hz))).collect()
    }

    /// How far the output stands above the vowel's valley at 1 kHz near
    /// `formant`.
    fn contrast(levels: &[(f32, f32)], formant: f32) -> f32 {
        let near = |hz: f32, octaves: f32| {
            levels.iter().filter(|(h, _)| (h / hz).log2().abs() <= octaves).map(|(_, l)| *l).fold(0.0, f32::max)
        };
        near(formant, 0.3) / near(1000.0, 0.15)
    }

    #[test]
    fn flatten_lets_a_dull_carrier_speak() {
        // Without it, the vowel's 2 kHz formant is lost under the carrier's
        // own slope: the loudest harmonic is the fundamental
        let plain = harmonics_with_flatten(dull(125.0), 0.0);
        let loudest = plain.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
        assert!(loudest < 300.0, "the loudest harmonic is {loudest} Hz");
        assert!(contrast(&plain, 2000.0) < 1.0, "a dull carrier's 2 kHz already stands out: {}", contrast(&plain, 2000.0));

        // Flattened, both formants stand out of the valley as far as they do
        // from a bright saw, to within 1.5 dB
        let flattened = harmonics_with_flatten(dull(125.0), 1.0);
        let saw = harmonics_with_flatten(saw(125.0), 0.0);
        let db = |levels: &[(f32, f32)]| [500.0, 2000.0].map(|f| 20.0 * contrast(levels, f).log10());
        println!("contrast in dB at 500 Hz and 2 kHz: flattened {:?}, a saw {:?}", db(&flattened), db(&saw));
        for formant in [500.0, 2000.0] {
            let (ours, theirs) = (contrast(&flattened, formant), contrast(&saw, formant));
            assert!(ours > 0.84 * theirs, "{formant} Hz: flattened {ours}, a saw {theirs}");
            assert!(ours > 1.5, "{formant} Hz: flattened {ours}");
        }
    }

    #[test]
    fn flatten_keeps_the_level_and_shows_the_tilt() {
        let mut tilts = Vec::new();
        let mut levels = Vec::new();
        for flatten in [0.0, 0.5, 1.0] {
            let mut vocoder = Vocoder::new();
            let (l, r) = play_on(&mut vocoder, 1.5, dull(110.0), vowel(&[600.0, 1200.0, 2600.0]), 0.0, |_| knobs_with(false, flatten));
            levels.push(rms(&mono(&l[48000..], &r[48000..])));
            let values = vocoder.readout(&[]).unwrap().values;
            tilts.push((values[Vocoder::READOUT_TILT], values[Vocoder::READOUT_EVENED]));
        }
        // The dull carrier falls steeply across the bank, and Flatten levels
        // it by its share
        let (own, played) = tilts[0];
        assert!(own < -6.0, "a dull carrier's tilt {own}");
        assert!((played - own).abs() < 0.01, "unflattened, played as it is: {played}");
        let (own, played) = tilts[1];
        assert!((played - own / 2.0).abs() < 0.1 * own.abs(), "half flattened: {own} played as {played}");
        assert!(tilts[2].1.abs() < 0.5, "flattened {:?}", tilts[2]);
        for level in &levels {
            assert!((20.0 * (level / levels[0]).log10()).abs() < 3.0, "{levels:?}");
        }
    }

    #[test]
    fn flatten_does_not_raise_silence_into_noise() {
        // A carrier that's silent but for a hiss at -100 dBFS stays silent
        let mut faint = white(11);
        let (l, _) = play(1.0, move |_| 1e-5 * faint(), vowel(&[700.0, 1100.0]), 0.0, |_| knobs_with(false, 1.0));
        assert!(rms(&l[24000..]) < 1e-5, "raised to {}", rms(&l[24000..]));

        // Beside a low tone, the bands it doesn't reach are lifted no more
        // than 40 dB: the hiss up there stays far below the tone
        let mut faint = white(11);
        let low = move |n: usize| 0.4 * (2.0 * PI * 200.0 * n as f32 / SR).sin() + 1e-5 * faint();
        let mut open = white(5);
        let (l, _) = play(1.0, low, move |_| 0.3 * open(), 0.0, |_| knobs_with(false, 1.0));
        let c = SvfCoefficients::new(2000.0, 0.7, SR);
        let (mut first, mut second) = (Svf::default(), Svf::default());
        let highs: Vec<f32> = l.iter().map(|&s| second.high(&c, first.high(&c, s))).collect();
        assert!(rms(&highs[24000..]) < 0.01 * rms(&l[24000..]), "hiss {} under {}", rms(&highs[24000..]), rms(&l[24000..]));
    }

    #[test]
    fn flatten_does_not_blast_a_new_chord() {
        // The bands a new chord fills were lifted while they were empty, and
        // must come down before they're heard. A vocoder swells a little on
        // any new note as its bands fill (1.35 times here). Flattened, the
        // lifted highs fill faster and it swells a little more (1.8 times),
        // which is no burst
        let swell = |flatten: f32| {
            // A dull oscillator stepping up a fifth, as one does on a new
            // note: the wave carries on from where it was
            let mut phase = 0.0f32;
            let carrier = move |n: usize| {
                let hz = if n < 36000 { 110.0 } else { 165.0 };
                phase = (phase + hz / SR).fract();
                (1..=48).map(|k| 0.4 / (k * k) as f32 * (2.0 * PI * (k as f32 * phase).fract()).sin()).sum::<f32>()
            };
            let mut open = white(5);
            let (l, r) = play(1.25, carrier, move |_| 0.3 * open(), 0.0, |_| knobs_with(false, flatten));
            assert!(l.iter().chain(&r).all(|s| s.is_finite()));
            let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let steady = peak(&l[24000..36000]).max(peak(&r[24000..36000]));
            peak(&l[36000..40800]).max(peak(&r[36000..40800])) / steady
        };
        let (plain, flattened) = (swell(0.0), swell(1.0));
        assert!(flattened < 1.5 * plain.max(1.0), "flattened, the new note swells {flattened}×; plain, {plain}×");
    }

    #[test]
    fn registry_makes_one() {
        let mut registry = crate::dsp::ModuleRegistry::new();
        registry.register::<Vocoder>();
        let module = registry.create("fx.vocoder").unwrap();
        assert_eq!(module.parameters().len(), 11);
    }
}
