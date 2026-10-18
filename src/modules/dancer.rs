//! Dancer module: a stick figure who dances to the patch.
//!
//! The audio thread works out where the dance is: how far through the
//! current move it is, how long a move lasts, how loud the music is, and
//! when a big move was last asked for. The face (`graph::dancer_display`)
//! turns that into a figure, so everything here is cheap bookkeeping and the
//! choreography lives with the drawing.
//!
//! With nothing patched the Dancer follows the patch's Clock through the
//! transport. A cable into Clock takes over from the first two pulses. With
//! neither, it listens for the beat in Audio (`dsp::beat_tracker`). Every
//! way the beat is folded by powers of two into a tempo a person dances at,
//! so a sixteenth-note clock is danced on the quarters and a slow ballad is
//! danced double time.

use crate::dsp::{
    gain_to_db,
    beat_tracker::BeatTracker,
    context::ProcessContext,
    denormal::flush,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{decay_for_seconds, rate_for_seconds, Follower},
    signal::{connected_input, SignalBuffer},
    ParameterDisplay, SignalType,
};
use crate::modules::sequencer::StepTimer;

/// The dance styles, in the order of the Style dropdown.
pub const STYLES: &[&str] = &["Bounce", "Disco", "Robot", "Headbang"];

/// What the dance follows this block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Clock,
    Transport,
    /// The beat heard in Audio.
    Heard,
}

/// The module's ID, as patches name it.
pub const ID: &str = "util.dancer";

/// A stick figure who dances to the patch.
///
/// # Ports
///
/// **Inputs:**
/// - **Clock** (Gate): Each pulse is a step. Left empty, the Dancer follows
///   the patch's Clock, or with no Clock running, the beat it hears in Audio.
/// - **Audio** (Audio): The music to dance to: the louder it plays, the
///   bigger the moves.
/// - **Accent** (Gate): A rising edge asks for a big move: a jump, a spin,
///   or arms in the air.
///
/// # Parameters
///
/// - **Style**: Bounce, Disco, Robot or Headbang.
/// - **Size** (1-3): How big the face is drawn, set by dragging its corner.
pub struct Dancer {
    sample_rate: f32,
    /// The clock input, for its rising edges.
    clock_edge: Edge,
    /// The patched clock's steps.
    clock: StepTimer,
    /// Clock pulses since the first, `None` before it.
    pulses: Option<u64>,
    /// The accent input, for its rising edges.
    accent_edge: Edge,
    /// Accents so far, so the face can tell one it hasn't drawn yet.
    accent_count: u32,
    /// Samples since the last accent, `None` before the first.
    since_accent: Option<u64>,
    /// What the dance followed last, and how many beats (or pulses) it took
    /// to make one move, as a power of two.
    fold: Option<(Source, i32)>,
    /// The music's level, and fast and slow followers whose ratio marks a hit.
    level: Follower,
    fast: Follower,
    slow: Follower,
    /// The last hit, held and fading so a frame between callbacks sees it.
    punch: f32,
    /// Listens for a beat in Audio, for when nothing else gives one.
    ears: BeatTracker,
    coefficients: Coefficients,
    /// Where the dance is at the end of the block: moves since the start
    /// and a move's length in seconds. `None` when there's no beat.
    dance: Option<(f64, f32)>,
    /// Whether it's dancing by ear, or listening for a beat.
    ear: f32,
    energy: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

/// The followers' rates, per sample at the current sample rate.
#[derive(Clone, Copy, Debug, Default)]
struct Coefficients {
    level_attack: f32,
    level_release: f32,
    fast_attack: f32,
    fast_release: f32,
    slow_attack: f32,
    slow_release: f32,
    punch_decay: f32,
}

impl Coefficients {
    fn at(sample_rate: f32) -> Self {
        Self {
            level_attack: rate_for_seconds(0.01, sample_rate),
            level_release: rate_for_seconds(0.25, sample_rate),
            fast_attack: rate_for_seconds(0.001, sample_rate),
            fast_release: rate_for_seconds(0.1, sample_rate),
            slow_attack: rate_for_seconds(0.04, sample_rate),
            slow_release: rate_for_seconds(0.1, sample_rate),
            punch_decay: decay_for_seconds(0.12, sample_rate),
        }
    }
}



impl Dancer {
    /// Creates a Dancer, bouncing.
    pub fn new() -> Self {
        Self {
            sample_rate: 48000.0,
            clock_edge: Edge::LOW,
            clock: StepTimer::new(),
            pulses: None,
            accent_edge: Edge::LOW,
            accent_count: 0,
            since_accent: None,
            fold: None,
            level: Follower::default(),
            fast: Follower::default(),
            slow: Follower::default(),
            punch: 0.0,
            ears: BeatTracker::new(),
            coefficients: Coefficients::at(48000.0),
            dance: None,
            ear: 0.0,
            energy: 0.0,
            ports: vec![
                PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0)
                    .describe("Each pulse is a step; leave it empty to dance to the patch's Clock"),
                PortDefinition::input_with_default("audio", "Audio", SignalType::Audio, 0.0)
                    .describe("Patch the mix here: the louder it plays, the bigger the moves. With no Clock, it dances to the beat it hears"),
                PortDefinition::input_with_default("accent", "Accent", SignalType::Gate, 0.0)
                    .describe("A rising edge makes a big move: a jump, a spin, arms in the air"),
            ],
            parameters: vec![
                ParameterDefinition::choice("style", "Style", STYLES, 0).describe("How the Dancer dances"),
                ParameterDefinition::new("size", "Size", 1.0, MAX_SIZE, 1.0, ParameterDisplay::linear("×"))
                    .describe("How big the face is drawn; drag its corner"),
            ],
        }
    }

    const PORT_CLOCK: usize = 0;
    const PORT_AUDIO: usize = 1;
    const PORT_ACCENT: usize = 2;

    pub const PARAM_STYLE: usize = 0;
    pub const PARAM_SIZE: usize = 1;

    // Readout slots

    /// 1 while there's a beat to dance to, 0 otherwise.
    pub const READOUT_DANCING: usize = 0;
    /// Whole moves since the dance started, wrapping at [`WRAP`](Self::WRAP).
    pub const READOUT_MOVE: usize = 1;
    /// How far through the current move, 0 to 1.
    pub const READOUT_PHASE: usize = 2;
    /// How long a move lasts, in seconds.
    pub const READOUT_MOVE_SECONDS: usize = 3;
    /// How loud the music is, 0 to 1.
    pub const READOUT_ENERGY: usize = 4;
    /// A hit in the music, 0 to 1, fading over about a tenth of a second.
    pub const READOUT_PUNCH: usize = 5;
    /// Accents so far, wrapping at [`WRAP`](Self::WRAP).
    pub const READOUT_ACCENTS: usize = 6;
    /// Moves since the last accent (half-seconds when there's no beat),
    /// up to [`ACCENT_LONG_AGO`](Self::ACCENT_LONG_AGO).
    pub const READOUT_ACCENT_AGE: usize = 7;
    /// 1 while it dances to the beat it hears in Audio, 0.5 while it
    /// listens for one there (nothing else giving it a beat), 0 otherwise.
    pub const READOUT_EAR: usize = 8;
    /// While it listens, how near it is to hearing a beat, 0 to 1: up to a
    /// half as a tempo starts to stand out, the rest as that tempo holds
    /// (see [`BeatTracker::hunch`]). 1 dancing to one, 0 otherwise.
    pub const READOUT_HUNCH: usize = 9;

    /// Counters in the readout wrap here, well inside an f32's whole numbers.
    pub const WRAP: u64 = 4096;
    /// An accent this many moves ago, or none yet.
    pub const ACCENT_LONG_AGO: f32 = 16.0;

    /// A move lasts at least this long once folded, in seconds (150 BPM),
    /// and less than twice that (75 BPM).
    const MOVE_MIN: f32 = 0.4;
    /// A fold already chosen is kept while its move stays this long, so a
    /// tempo ramp doesn't make the dance lurch between speeds.
    const KEEP_MIN: f32 = 0.3;
    const KEEP_MAX: f32 = 1.1;

    /// Energy with nothing in Audio, while dancing.
    const UNPATCHED_ENERGY: f32 = 0.6;
    /// The music's level for no energy and for all of it, in dBFS.
    const QUIET_DB: f32 = -42.0;
    const LOUD_DB: f32 = -8.0;

    /// How many beats (or pulses) of `unit_seconds` make one move, as a
    /// power of two: `current` while its move stays danceable, otherwise
    /// the fold that puts a move between [`MOVE_MIN`](Self::MOVE_MIN) and
    /// twice that.
    pub fn fold_for(unit_seconds: f32, current: Option<i32>) -> i32 {
        if let Some(k) = current {
            let move_seconds = unit_seconds * 2f32.powi(k);
            if (Self::KEEP_MIN..=Self::KEEP_MAX).contains(&move_seconds) {
                return k;
            }
        }
        let k = (Self::MOVE_MIN / unit_seconds.max(1e-4)).log2().ceil() as i32;
        // Rounding at an exact power of two can land a hair short
        if unit_seconds * 2f32.powi(k) < Self::MOVE_MIN * 0.999 {
            k + 1
        } else {
            k
        }
    }

    /// How far the fast follower must lead the slow one to be a hit.
    const HIT_RATIO: f32 = 1.3;

    /// The music's level as energy, 0 to 1, eased at both ends.
    fn energy_of(level: f32) -> f32 {
        let db = gain_to_db(level);
        let t = ((db - Self::QUIET_DB) / (Self::LOUD_DB - Self::QUIET_DB)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The beat this block, at its end: the position in beats or pulses,
    /// and how long one lasts in seconds. A patched clock comes first, then
    /// the transport, then the beat heard in Audio.
    fn beat(&self, clock_patched: bool, context: &ProcessContext) -> Option<(Source, f64, f32)> {
        if clock_patched {
            if let (Some(pulses), Some(step), Some(since)) = (self.pulses, self.clock.coming_step(), self.clock.since_clock()) {
                // A clock that has stopped for two steps has stopped
                if since <= step * 2 {
                    let through = (since as f64 / step as f64).min(0.999);
                    return Some((Source::Clock, (pulses - 1) as f64 + through, step as f32 / self.sample_rate));
                }
            }
        }
        let transport = &context.transport;
        if transport.playing {
            if let Some(beat) = transport.beat_at(context.block_size, context.sample_rate) {
                let unit = 60.0 / transport.tempo_bpm.unwrap_or(120.0).max(1.0);
                return Some((Source::Transport, beat, unit));
            }
        }
        self.ears.beat().map(|(beat, unit)| (Source::Heard, beat, unit))
    }
}

/// The largest the face can be drawn, as a multiple of its first size.
pub const MAX_SIZE: f32 = 3.0;

impl Default for Dancer {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Dancer {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Dancer",
            category: ModuleCategory::Utility,
            description: "A stick figure who dances to the patch: on the Clock's beat, as hard as the music is loud",
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
        self.coefficients = Coefficients::at(sample_rate);
        self.clock.forget();
        self.ears.prepare(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        _outputs: &mut [SignalBuffer],
        _params: &[f32],
        context: &ProcessContext,
    ) {
        let clock_in = connected_input(inputs, Self::PORT_CLOCK);
        let audio_in = connected_input(inputs, Self::PORT_AUDIO);
        let accent_in = connected_input(inputs, Self::PORT_ACCENT);
        let at = |buf: Option<&SignalBuffer>, i: usize| buf.map_or(0.0, |buf| buf.at(i));
        let c = self.coefficients;

        for i in 0..context.block_size {
            self.clock.tick();
            if self.clock_edge.rising(at(clock_in, i)) {
                self.clock.clock();
                self.pulses = Some(self.pulses.map_or(1, |n| n + 1));
            }

            if let Some(since) = self.since_accent.as_mut() {
                *since = since.saturating_add(1);
            }
            if self.accent_edge.rising(at(accent_in, i)) {
                self.accent_count = self.accent_count.wrapping_add(1);
                self.since_accent = Some(0);
            }

            if audio_in.is_some() {
                let x = at(audio_in, i);
                self.level.follow(x, c.level_attack, c.level_release);
                let fast = self.fast.follow(x, c.fast_attack, c.fast_release);
                let slow = self.slow.follow(x, c.slow_attack, c.slow_release);
                // Both fall alike, so they agree on a steady sound and part
                // only while the fast one leaps ahead at a hit
                let hit = if fast > 1e-3 { ((fast / slow.max(1e-4) - Self::HIT_RATIO) / 1.5).clamp(0.0, 1.0) } else { 0.0 };
                self.punch = flush((self.punch * c.punch_decay).max(hit));
                self.ears.hear(x);
            }
        }
        if clock_in.is_none() {
            // Unpatching the clock forgets it, so a new one starts afresh
            self.pulses = None;
            self.clock.forget();
        }
        if audio_in.is_none() {
            self.level = Follower::default();
            self.punch = 0.0;
            self.ears.forget();
        }

        let beat = self.beat(clock_in.is_some(), context);
        self.ear = match beat {
            Some((Source::Heard, ..)) => 1.0,
            None if audio_in.is_some() => 0.5,
            _ => 0.0,
        };
        self.dance = beat.map(|(source, position, unit)| {
            let current = self.fold.filter(|(was, _)| *was == source).map(|(_, k)| k);
            let k = Self::fold_for(unit, current);
            self.fold = Some((source, k));
            let scale = 2f64.powi(k);
            (position / scale, unit * scale as f32)
        });
        self.energy = match (audio_in.is_some(), self.dance.is_some()) {
            (true, _) => Self::energy_of(self.level.level),
            (false, true) => Self::UNPATCHED_ENERGY,
            (false, false) => 0.0,
        };
    }

    fn reset(&mut self) {
        self.clock_edge.reset();
        self.clock.forget();
        self.pulses = None;
        self.accent_edge.reset();
        self.accent_count = 0;
        self.since_accent = None;
        self.fold = None;
        self.level = Follower::default();
        self.fast = Follower::default();
        self.slow = Follower::default();
        self.punch = 0.0;
        self.ears.forget();
        self.dance = None;
        self.ear = 0.0;
        self.energy = 0.0;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let v = &mut readout.values;
        if let Some((position, move_seconds)) = self.dance {
            let whole = position.floor();
            v[Self::READOUT_DANCING] = 1.0;
            v[Self::READOUT_MOVE] = (whole.rem_euclid(Self::WRAP as f64)) as f32;
            v[Self::READOUT_PHASE] = (position - whole) as f32;
            v[Self::READOUT_MOVE_SECONDS] = move_seconds;
        }
        v[Self::READOUT_ENERGY] = self.energy;
        v[Self::READOUT_EAR] = self.ear;
        v[Self::READOUT_HUNCH] = if self.ear > 0.0 { self.ears.hunch() } else { 0.0 };
        v[Self::READOUT_PUNCH] = self.punch;
        v[Self::READOUT_ACCENTS] = (self.accent_count as u64 % Self::WRAP) as f32;
        let move_seconds = self.dance.map_or(0.5, |(_, seconds)| seconds);
        v[Self::READOUT_ACCENT_AGE] = self.since_accent.map_or(Self::ACCENT_LONG_AGO, |samples| {
            (samples as f32 / self.sample_rate / move_seconds).min(Self::ACCENT_LONG_AGO)
        });
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;

    const SAMPLE_RATE: f32 = 48000.0;
    const BLOCK: usize = 256;
    const PARAMS: [f32; 2] = [0.0, 1.0];

    fn dancer() -> Dancer {
        let mut dancer = Dancer::new();
        dancer.prepare(SAMPLE_RATE, BLOCK);
        dancer
    }

    /// Runs `clock`, `audio` and `accent` (one value per sample; `None` is
    /// unpatched) through `dancer`, a block at a time, with `transport`.
    fn run(dancer: &mut Dancer, clock: Option<&[f32]>, audio: Option<&[f32]>, accent: Option<&[f32]>, transport: TransportState, len: usize) {
        let mut transport = transport;
        let mut outs: Vec<SignalBuffer> = Vec::new();
        for start in (0..len).step_by(BLOCK) {
            let buffer = |input: Option<&[f32]>, signal_type: SignalType| match input {
                Some(input) => {
                    let mut buf = SignalBuffer::new(BLOCK, signal_type);
                    for (i, sample) in buf.samples.iter_mut().enumerate() {
                        *sample = input.get(start + i).copied().unwrap_or(0.0);
                    }
                    buf
                }
                None => SignalBuffer::unconnected(BLOCK, signal_type),
            };
            let buffers = [buffer(clock, SignalType::Gate), buffer(audio, SignalType::Audio), buffer(accent, SignalType::Gate)];
            let refs: Vec<&SignalBuffer> = buffers.iter().collect();
            let ctx = ProcessContext::with_transport(SAMPLE_RATE, BLOCK, transport);
            dancer.process(&refs, &mut outs, &PARAMS, &ctx);
            if let (true, Some(beat), Some(bpm)) = (transport.playing, transport.beat_position, transport.tempo_bpm) {
                transport.beat_position = Some(beat + BLOCK as f64 * bpm as f64 / 60.0 / SAMPLE_RATE as f64);
            }
        }
    }

    fn value(dancer: &Dancer, slot: usize) -> f32 {
        dancer.readout(&PARAMS).unwrap().values[slot]
    }

    /// A clock: a 10 ms pulse every `period` samples, `count` times.
    fn pulses(period: usize, count: usize) -> Vec<f32> {
        (0..period * count).map(|i| if i % period < 480 { 1.0 } else { 0.0 }).collect()
    }

    fn sine(amplitude: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amplitude * (i as f32 * 220.0 * std::f32::consts::TAU / SAMPLE_RATE).sin()).collect()
    }

    fn playing(bpm: f32, beat: f64) -> TransportState {
        TransportState { beat_position: Some(beat), ..TransportState::playing_at(bpm) }
    }

    #[test]
    fn test_dancer_info() {
        let dancer = Dancer::new();
        assert_eq!(dancer.info().id, "util.dancer");
        assert_eq!(dancer.info().category, ModuleCategory::Utility);
        assert_eq!(dancer.ports().iter().filter(|p| p.is_input()).count(), 3);
        assert_eq!(dancer.ports().iter().filter(|p| p.is_output()).count(), 0);
        assert_eq!(dancer.parameters().len(), 2);
    }

    #[test]
    fn test_fold_puts_a_move_between_150_and_75_bpm() {
        for (unit, expected) in [(0.125, 2), (0.25, 1), (0.5, 0), (0.4, 0), (0.8, -1), (1.0, -1), (2.0, -2), (0.0625, 3)] {
            let k = Dancer::fold_for(unit, None);
            assert_eq!(k, expected, "a {unit} s beat");
            let move_seconds = unit * 2f32.powi(k);
            assert!((0.4..0.8).contains(&move_seconds), "a {unit} s beat folds to a {move_seconds} s move");
        }
    }

    #[test]
    fn test_fold_holds_through_a_tempo_ramp() {
        // A ritardando from 120 to 57 BPM keeps dancing on the beat...
        let mut k = Dancer::fold_for(0.5, None);
        for unit in [0.55, 0.6, 0.7, 0.8, 0.9, 1.0, 1.05] {
            k = Dancer::fold_for(unit, Some(k));
            assert_eq!(k, 0, "a {unit} s beat");
        }
        // ...until a move would take too long, when it goes double time
        assert_eq!(Dancer::fold_for(1.2, Some(k)), -1);
    }

    #[test]
    fn test_a_sixteenth_clock_dances_on_the_quarters() {
        // Sixteenths at 120 BPM: 6000 samples apart
        let mut dancer = dancer();
        let clock = pulses(6000, 41);
        run(&mut dancer, Some(&clock), None, None, TransportState::new(), clock.len());
        assert_eq!(value(&dancer, Dancer::READOUT_DANCING), 1.0);
        assert!((value(&dancer, Dancer::READOUT_MOVE_SECONDS) - 0.5).abs() < 1e-3);
        // The 41st pulse starts move 10, and the last block ended just past
        // where the 42nd would come: the dance holds at the end of that
        // pulse rather than running on past it
        assert_eq!(value(&dancer, Dancer::READOUT_MOVE), 10.0);
        let phase = value(&dancer, Dancer::READOUT_PHASE);
        assert!((phase - 0.999 / 4.0).abs() < 1e-3, "phase {phase}");
    }

    #[test]
    fn test_with_nothing_patched_it_follows_the_transport() {
        let mut dancer = dancer();
        run(&mut dancer, None, None, None, playing(120.0, 6.5), BLOCK);
        assert_eq!(value(&dancer, Dancer::READOUT_DANCING), 1.0);
        assert_eq!(value(&dancer, Dancer::READOUT_MOVE), 6.0);
        let phase = value(&dancer, Dancer::READOUT_PHASE);
        assert!((phase - (0.5 + 256.0 / 24000.0)).abs() < 1e-3, "phase {phase}");
        // With no Audio, it dances at a steady energy
        assert_eq!(value(&dancer, Dancer::READOUT_ENERGY), Dancer::UNPATCHED_ENERGY);

        // At 180 BPM a move is two beats
        let mut dancer = self::dancer();
        run(&mut dancer, None, None, None, playing(180.0, 9.0), BLOCK);
        assert_eq!(value(&dancer, Dancer::READOUT_MOVE), 4.0);
        assert!((value(&dancer, Dancer::READOUT_MOVE_SECONDS) - 2.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn test_with_no_beat_it_doesnt_dance() {
        let mut dancer = dancer();
        run(&mut dancer, None, None, None, TransportState::new(), BLOCK * 4);
        assert_eq!(value(&dancer, Dancer::READOUT_DANCING), 0.0);
        assert_eq!(value(&dancer, Dancer::READOUT_ENERGY), 0.0);

        // A clock that has stopped stops the dance
        let mut dancer = self::dancer();
        let mut clock = pulses(6000, 4);
        clock.resize(6000 * 8, 0.0);
        run(&mut dancer, Some(&clock), None, None, TransportState::new(), clock.len());
        assert_eq!(value(&dancer, Dancer::READOUT_DANCING), 0.0);
    }

    #[test]
    fn test_a_clock_before_its_second_pulse_leaves_the_transport_in_charge() {
        let mut dancer = dancer();
        let clock = pulses(6000, 1);
        run(&mut dancer, Some(&clock), None, None, playing(120.0, 2.0), BLOCK * 4);
        assert_eq!(value(&dancer, Dancer::READOUT_MOVE), 2.0);
    }

    #[test]
    fn test_each_accent_counts_once() {
        let mut dancer = dancer();
        // Two accents, the first held high across several blocks
        let mut accent = vec![0.0; 24000];
        accent[100..2000].fill(1.0);
        accent[12000..12010].fill(1.0);
        run(&mut dancer, None, None, Some(&accent), playing(120.0, 0.0), accent.len());
        assert_eq!(value(&dancer, Dancer::READOUT_ACCENTS), 2.0);
        // The last was about half a 0.5 s move ago
        let age = value(&dancer, Dancer::READOUT_ACCENT_AGE);
        assert!((age - (24064.0 - 12000.0) / 24000.0).abs() < 0.01, "age {age}");

        let fresh = self::dancer();
        assert_eq!(value(&fresh, Dancer::READOUT_ACCENT_AGE), Dancer::ACCENT_LONG_AGO);
    }

    #[test]
    fn test_louder_music_gives_more_energy() {
        let energy = |amplitude: f32| {
            let mut dancer = dancer();
            let audio = sine(amplitude, 24000);
            run(&mut dancer, None, Some(&audio), None, playing(120.0, 0.0), audio.len());
            value(&dancer, Dancer::READOUT_ENERGY)
        };
        let (silent, quiet, loud) = (energy(0.0), energy(0.03), energy(0.5));
        assert_eq!(silent, 0.0, "a patched but silent input has no energy");
        assert!(quiet > 0.0 && quiet < 0.5, "quiet {quiet}");
        assert!(loud > 0.9, "loud {loud}");
    }

    #[test]
    fn test_a_hit_punches() {
        let mut dancer = dancer();
        // A quiet bed, then a sudden loud hit
        let mut audio = sine(0.02, 24000);
        for (i, sample) in audio[23500..].iter_mut().enumerate() {
            *sample = 0.8 * (-(i as f32) / 2000.0).exp() * if i % 40 < 20 { 1.0 } else { -1.0 };
        }
        run(&mut dancer, None, Some(&audio[..23296]), None, playing(120.0, 0.0), 23296);
        assert!(value(&dancer, Dancer::READOUT_PUNCH) < 0.1);
        let mut dancer = self::dancer();
        run(&mut dancer, None, Some(&audio), None, playing(120.0, 0.0), audio.len());
        assert!(value(&dancer, Dancer::READOUT_PUNCH) > 0.5);
    }

    #[test]
    fn test_with_no_clock_it_dances_to_the_beat_it_hears() {
        let (music, _) = crate::dsp::beat_tracker::tests::four_to_the_floor(124.0, 8.0);
        let mut dancer = dancer();
        run(&mut dancer, None, Some(&music), None, TransportState::new(), music.len());
        assert_eq!(value(&dancer, Dancer::READOUT_DANCING), 1.0);
        let seconds = value(&dancer, Dancer::READOUT_MOVE_SECONDS);
        assert!((seconds - 60.0 / 124.0).abs() < 0.01, "a {seconds} s move");
        assert!(value(&dancer, Dancer::READOUT_ENERGY) > 0.5);
        assert_eq!(value(&dancer, Dancer::READOUT_EAR), 1.0);
        assert_eq!(value(&dancer, Dancer::READOUT_HUNCH), 1.0);

        // Before it's sure, it listens, nearer a beat as the hits go on
        let mut listening = self::dancer();
        let mut hunches = Vec::new();
        for half in 1..4 {
            run(&mut listening, None, Some(&music[(half - 1) * 24000..half * 24000]), None, TransportState::new(), 24000);
            hunches.push(value(&listening, Dancer::READOUT_HUNCH));
        }
        assert_eq!(value(&listening, Dancer::READOUT_DANCING), 0.0);
        assert_eq!(value(&listening, Dancer::READOUT_EAR), 0.5);
        assert!(hunches.windows(2).all(|w| w[0] < w[1]) && hunches[0] > 0.0 && hunches[2] < 1.0, "hunches {hunches:?}");

        // A held note never hits, so there's nothing to listen for
        let mut held = self::dancer();
        let note = sine(0.3, 96000);
        run(&mut held, None, Some(&note), None, TransportState::new(), note.len());
        assert_eq!(value(&held, Dancer::READOUT_EAR), 0.5);
        assert_eq!(value(&held, Dancer::READOUT_HUNCH), 0.0);

        // Unpatching the Audio forgets it
        run(&mut dancer, None, None, None, TransportState::new(), BLOCK);
        assert_eq!(value(&dancer, Dancer::READOUT_DANCING), 0.0);
    }

    #[test]
    fn test_a_clock_or_the_transport_comes_before_the_beat_heard() {
        // Music at 124 BPM, but the transport plays at 90: a 0.667 s beat
        let (music, _) = crate::dsp::beat_tracker::tests::four_to_the_floor(124.0, 8.0);
        let mut dancer = dancer();
        run(&mut dancer, None, Some(&music), None, playing(90.0, 0.0), music.len());
        assert!((value(&dancer, Dancer::READOUT_MOVE_SECONDS) - 60.0 / 90.0).abs() < 1e-3);
        assert_eq!(value(&dancer, Dancer::READOUT_EAR), 0.0);

        // ...and a patched clock, sixteenths at 120 BPM, before both
        let clock = pulses(6000, music.len() / 6000);
        let mut dancer = self::dancer();
        run(&mut dancer, Some(&clock), Some(&music), None, playing(90.0, 0.0), clock.len());
        assert!((value(&dancer, Dancer::READOUT_MOVE_SECONDS) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn test_dancer_is_in_the_registry() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create("util.dancer").expect("the Dancer is registered");
        assert_eq!(module.info().name, "Dancer");
    }
}
