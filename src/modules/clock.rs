//! Clock module.
//!
//! Generates periodic gate triggers for driving envelopes and creating
//! rhythmic patterns. The first Clock in a patch is also its transport: the
//! tempo and beat that tempo-synced modules (the LFO, the Delay) follow. It
//! keeps its own time, follows a MIDI clock from a DAW or drum machine, or
//! (in builds with the `ableton-link` feature) plays in an Ableton Link
//! session (see [`link`]).

#[cfg(feature = "ableton-link")]
pub mod link;

use crate::dsp::{
    context::{ProcessContext, TransportState},
    gate::{Edge, Pulse},
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, MidiMessage, SignalBuffer},
    tempo::{self, Division},
    ParameterDisplay, SignalType,
};

/// The Division choices: what each pulse is worth. Patches save the
/// index, so new divisions go on the end. Chopper's Pulse uses the same
/// table, so it can be set to match.
pub const DIVISIONS: [Division; 5] = [tempo::WHOLE, tempo::HALF, tempo::QUARTER, tempo::EIGHTH, tempo::SIXTEENTH];

/// The Division dropdown's labels.
pub const DIVISION_LABELS: [&str; DIVISIONS.len()] = tempo::labels(&DIVISIONS);

/// Where the Clock gets its time.
///
/// The third choice is the outside clock: an Ableton Link session in the
/// app, the DAW's transport in the plugin. So a patch made with one follows
/// the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockSource {
    /// Its own Tempo knob.
    Internal,
    /// A MIDI clock master, through the MIDI input.
    Midi,
    /// An Ableton Link session, in builds that have Link. Without a
    /// session (the renderer, the tests), the Clock keeps its own time.
    Link,
    /// The DAW's transport, when Soba is a plugin (see [`crate::hosted`]).
    /// Without the host's transport, the Clock keeps its own time.
    Host,
}

impl ClockSource {
    /// The Source choice's labels: the outside clock only where there is
    /// one, Link in builds that have it, the host in a plugin.
    pub fn labels() -> &'static [&'static str] {
        if crate::hosted::is_hosted() {
            &["Internal", "MIDI", "Host"]
        } else if cfg!(feature = "ableton-link") {
            &["Internal", "MIDI", "Link"]
        } else {
            &["Internal", "MIDI"]
        }
    }

    /// Convert from parameter value to source. The outside clock, where
    /// there isn't one, keeps its own time.
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => ClockSource::Midi,
            2 if crate::hosted::is_hosted() => ClockSource::Host,
            2 if cfg!(feature = "ableton-link") => ClockSource::Link,
            _ => ClockSource::Internal,
        }
    }
}

/// MIDI clock ticks per beat (quarter note).
pub const MIDI_TICKS_PER_BEAT: u64 = 24;

/// Tick intervals the tempo is fitted over: two beats' worth.
const TEMPO_WINDOW: usize = 48;

/// A gap between ticks longer than this, in seconds, means the master
/// stopped sending clock for a while, and the tempo fit starts over. A tick
/// at the Clock's slowest tempo (20 BPM) is 125 ms.
const TICK_GAP_SECONDS: f64 = 0.25;

/// What a MIDI message did to the transport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiClockChange {
    /// Nothing the Clock needs to act on.
    None,
    /// A tick while playing: the beat is now exactly this.
    Beat(f64),
    /// Start: back to the top, where the next tick is the downbeat.
    Restart,
    /// Song Position: the beat to hold at until playing resumes.
    Locate(f64),
}

/// Follows a MIDI clock master: its tempo from the spacing of its ticks, and
/// its position from counting them.
///
/// Counting keeps the Clock's beat on the master's: every tick puts it
/// exactly where the master says, so it can never drift, however rough the
/// tempo estimate. The tempo only paces the beat between ticks.
#[derive(Clone, Debug)]
pub struct MidiClockFollower {
    /// Samples counted since the follower started: its own time line.
    now: u64,
    /// When recent ticks arrived, on `now`, in a ring.
    arrivals: [u64; TEMPO_WINDOW + 1],
    /// How many entries of `arrivals` hold ticks.
    count: usize,
    /// Where the next arrival goes in `arrivals`.
    head: usize,
    /// Samples per tick, fitted over the window, once two ticks have come.
    samples_per_tick: Option<f64>,
    /// Whether the master is playing: after Start or Continue, before Stop.
    /// `None` until the master sends any of these, as simple clock sources
    /// never do; their ticks alone run the clock.
    playing: Option<bool>,
    /// The position the next tick marks, in ticks from the top.
    next_tick: u64,
    /// Whether a tick has come since playing (re)started. Until one does, the
    /// position holds where Start, Continue or Song Position left it.
    ticked: bool,
}

impl Default for MidiClockFollower {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiClockFollower {
    /// A follower that has heard nothing yet.
    pub fn new() -> Self {
        Self {
            now: 0,
            arrivals: [0; TEMPO_WINDOW + 1],
            count: 0,
            head: 0,
            samples_per_tick: None,
            playing: None,
            next_tick: 0,
            ticked: false,
        }
    }

    /// Handles one MIDI message at the current sample.
    pub fn handle(&mut self, message: MidiMessage, sample_rate: f32) -> MidiClockChange {
        match message {
            MidiMessage::Clock => {
                self.time_tick(sample_rate);
                if !self.playing.unwrap_or(true) {
                    return MidiClockChange::None;
                }
                let beat = self.next_tick as f64 / MIDI_TICKS_PER_BEAT as f64;
                self.next_tick += 1;
                self.ticked = true;
                MidiClockChange::Beat(beat)
            }
            MidiMessage::Start => {
                self.playing = Some(true);
                self.next_tick = 0;
                self.ticked = false;
                MidiClockChange::Restart
            }
            MidiMessage::Continue => {
                self.playing = Some(true);
                self.ticked = false;
                MidiClockChange::None
            }
            MidiMessage::Stop => {
                self.playing = Some(false);
                MidiClockChange::None
            }
            MidiMessage::SongPosition { sixteenths } => {
                self.next_tick = sixteenths as u64 * (MIDI_TICKS_PER_BEAT / 4);
                self.ticked = false;
                MidiClockChange::Locate(self.next_beat())
            }
            _ => MidiClockChange::None,
        }
    }

    /// Moves on one sample.
    #[inline]
    pub fn advance(&mut self) {
        self.now += 1;
    }

    /// Whether the beat is moving: the master is playing and has ticked.
    pub fn is_running(&self) -> bool {
        self.playing.unwrap_or(true) && self.ticked
    }

    /// Whether ticks are coming in.
    pub fn is_receiving(&self, sample_rate: f32) -> bool {
        self.count > 0 && ((self.now - self.newest()) as f64) < TICK_GAP_SECONDS * sample_rate as f64
    }

    /// The beat the next tick marks. The beat never runs past it between
    /// ticks.
    pub fn next_beat(&self) -> f64 {
        self.next_tick as f64 / MIDI_TICKS_PER_BEAT as f64
    }

    /// Samples per tick, once two ticks have come.
    pub fn samples_per_tick(&self) -> Option<f64> {
        self.samples_per_tick
    }

    /// The master's tempo, once two ticks have come.
    pub fn tempo_bpm(&self, sample_rate: f32) -> Option<f32> {
        self.samples_per_tick
            .map(|spt| (60.0 * sample_rate as f64 / (spt * MIDI_TICKS_PER_BEAT as f64)) as f32)
    }

    /// The most recent arrival. Only meaningful while `count > 0`.
    fn newest(&self) -> u64 {
        self.arrivals[(self.head + self.arrivals.len() - 1) % self.arrivals.len()]
    }

    /// Notes a tick's arrival and refits the tempo.
    fn time_tick(&mut self, sample_rate: f32) {
        if self.count > 0 {
            let gap = (self.now - self.newest()) as f64;
            // A long gap, or a tick far off the beat the fit expects, means
            // the master paused or jumped tempo: fit afresh from here, as
            // the old ticks only pull the estimate towards a stale tempo
            let paused = gap > TICK_GAP_SECONDS * sample_rate as f64;
            let jumped = self.samples_per_tick.is_some_and(|spt| !(0.5..=2.0).contains(&(gap / spt)));
            if paused || jumped {
                self.count = 0;
            }
        }

        self.arrivals[self.head] = self.now;
        self.head = (self.head + 1) % self.arrivals.len();
        self.count = (self.count + 1).min(self.arrivals.len());

        if self.count >= 2 {
            self.samples_per_tick = Some(self.fit_interval());
        }
    }

    /// The least-squares slope of arrival time over tick number across the
    /// window: the tick interval, with each tick's own timing jitter mostly
    /// averaged out (the two end ticks alone would carry all of theirs).
    fn fit_interval(&self) -> f64 {
        let n = self.count;
        let len = self.arrivals.len();
        let oldest = (self.head + len - n) % len;
        let origin = self.arrivals[oldest];
        let mean_x = (n - 1) as f64 / 2.0;
        let mean_y = (0..n).map(|k| (self.arrivals[(oldest + k) % len] - origin) as f64).sum::<f64>() / n as f64;
        let (mut covariance, mut variance) = (0.0, 0.0);
        for k in 0..n {
            let dx = k as f64 - mean_x;
            let dy = (self.arrivals[(oldest + k) % len] - origin) as f64 - mean_y;
            covariance += dx * dy;
            variance += dx * dx;
        }
        covariance / variance
    }
}

/// The module's ID, as patches name it.
pub const ID: &str = "util.clock";

/// A clock module that generates periodic gate triggers.
///
/// Essential for testing envelope modules and creating rhythmic patterns
/// without external input.
///
/// # Ports
///
/// - **Sync** (Gate, Input): External clock sync input (resets the beat on
///   rising edge).
/// - **Tempo** (Control, Input): Bends the tempo around the Tempo knob, by
///   octaves: +1 doubles it, −1 halves it. A late input, so a lane of an
///   Arranger the Clock drives can slow the song that drives it.
/// - **Gate** (Gate, Output): Periodic gate output (0.0 or 1.0).
/// - **Run** (Gate, Output): High while the clock runs.
/// - **Reset** (Gate, Output): A short pulse when the clock starts from the
///   top: Run switched on, or a MIDI Start.
///
/// # Parameters
///
/// - **Tempo** (20-300 BPM): Speed of the clock. Also sets the patch tempo
///   that tempo-synced modules, such as the Delay, follow. Tempo CV bends it
///   within the same range.
/// - **Gate Length** (1-99%): Duration of the gate high as percentage of beat.
/// - **Division** (0-4): Note division (whole, half, quarter, eighth, sixteenth).
/// - **Run** (toggle): Whether the clock is running.
/// - **Source** (choice): Internal keeps its own time at Tempo; MIDI follows
///   the MIDI clock coming in (its ticks, Start, Continue and Stop); Link
///   (in builds with Ableton Link) plays in a Link session's time, and
///   turning Tempo or switching Run changes the session's.
/// - **Swing** (50-75%): Where the second pulse of each pair lands, as a
///   share of the pair. 50% is straight; 66% is a triplet shuffle. The first
///   pulse of each pair stays on the grid, so bars keep their length.
pub struct Clock {
    /// Beats since the clock started. Kept in `f64` and counted from the
    /// start, so even the slowest clock lands every edge on its sample.
    beats: f64,
    /// Whether the beat moved in the last sample: the transport is playing.
    running: bool,
    /// The sync input, for its rising edges.
    sync: Edge,
    /// What Tempo CV multiplied the Tempo knob by at the last sample, so the
    /// transport and the readout carry the tempo the Clock is playing.
    tempo_scale: f64,
    /// Run as the last block saw it, to notice it switching on. `None`
    /// before the first block, so loading a running clock isn't a start.
    prev_run: Option<bool>,
    /// The Reset trigger.
    reset_out: Pulse,
    /// Follows the MIDI clock, when Source is MIDI.
    midi: MidiClockFollower,
    /// Follows a Link session, when Source is Link.
    #[cfg(feature = "ableton-link")]
    link: link::LinkFollower,
    /// The DAW's tempo at the last block, when Source is Host and the host
    /// reports its transport.
    host_tempo: Option<f64>,
    /// Sample rate from last prepare() call.
    sample_rate: f32,
    /// Port definitions.
    ports: Vec<PortDefinition>,
    /// Parameter definitions.
    parameters: Vec<ParameterDefinition>,
}

impl Clock {
    /// Creates a new Clock.
    pub fn new() -> Self {
        Self {
            beats: 0.0,
            running: false,
            sync: Edge::LOW,
            tempo_scale: 1.0,
            prev_run: None,
            reset_out: Pulse::IDLE,
            midi: MidiClockFollower::new(),
            #[cfg(feature = "ableton-link")]
            link: link::LinkFollower::new(),
            host_tempo: None,
            sample_rate: 44100.0,
            ports: vec![
                // Input port
                PortDefinition::input_with_default("sync", "Sync", SignalType::Gate, 0.0).describe("A rising edge restarts the beat; patch another clock or trigger"),
                // Late, so a lane of the Arranger this Clock drives can close
                // the loop: a block behind is nothing to a tempo
                PortDefinition::input_with_default("tempo_cv", "Tempo", SignalType::Control, 0.0)
                    .late()
                    .describe("CV that bends the tempo around the BPM knob: +1 doubles it, -1 halves it. An Arranger lane here slows the song into its last chord"),
                // Output ports
                PortDefinition::output("gate", "Gate", SignalType::Gate).describe("Pulse on every beat division; patch into a sequencer or envelope"),
                PortDefinition::output("run", "Run", SignalType::Gate).describe("High while the clock runs; patch into a sequencer's Run"),
                PortDefinition::output("reset", "Reset", SignalType::Gate).describe("Pulses when the clock starts from the top; patch into a sequencer's Reset"),
            ],
            parameters: vec![
                // Tempo in BPM
                ParameterDefinition::new(
                    "tempo",
                    "Tempo",
                    Self::MIN_TEMPO as f32,
                    Self::MAX_TEMPO as f32,
                    120.0,
                    ParameterDisplay::linear("BPM"),
                ).describe("Speed in beats per minute"),
                // Gate length as percentage
                ParameterDefinition::new(
                    "gate_length",
                    "Gate Length",
                    1.0,
                    99.0,
                    50.0,
                    ParameterDisplay::linear("%"),
                ).describe("How much of each pulse the gate stays high"),
                // Division (discrete: whole, half, quarter, eighth, sixteenth)
                ParameterDefinition::choice(
                    "division",
                    "Division",
                    &DIVISION_LABELS,
                    2, // Default to quarter note
                ).describe("Pulse rate relative to the beat; 1/4 is one per beat"),
                // Run toggle
                ParameterDefinition::toggle("run", "Run", true).describe("Starts and stops the clock; starting begins from the top"),
                // Where the time comes from
                ParameterDefinition::choice(
                    "source",
                    "Source",
                    ClockSource::labels(),
                    0,
                ).describe(if crate::hosted::is_hosted() {
                    "Internal keeps time at Tempo; MIDI follows a MIDI clock coming in; Host plays in time with the DAW, starting and stopping with it"
                } else if cfg!(feature = "ableton-link") {
                    "Internal keeps time at Tempo; MIDI follows the clock of a DAW or drum machine on the MIDI input; Link plays in time with Ableton Link apps on the network"
                } else {
                    "Internal keeps time at Tempo; MIDI follows the clock of a DAW or drum machine on the MIDI input"
                }),
                // Where the second pulse of each pair lands
                ParameterDefinition::new(
                    "swing",
                    "Swing",
                    50.0,
                    75.0,
                    50.0,
                    ParameterDisplay::linear("%"),
                ).describe("Delays every second pulse; 50% is straight, 66% a triplet shuffle"),
            ],
        }
    }

    /// Port index constants.
    const PORT_SYNC: usize = 0;
    const PORT_TEMPO: usize = 1;
    const PORT_GATE: usize = 0;
    const PORT_RUN: usize = 1;
    const PORT_RESET: usize = 2;

    /// Parameter index constants.
    pub const PARAM_TEMPO: usize = 0;
    const PARAM_GATE_LENGTH: usize = 1;
    const PARAM_DIVISION: usize = 2;
    pub const PARAM_RUN: usize = 3;
    const PARAM_SOURCE: usize = 4;
    const PARAM_SWING: usize = 5;

    /// The tempo range, the Tempo knob's, which Tempo CV stays within too.
    pub const MIN_TEMPO: f64 = 20.0;
    pub const MAX_TEMPO: f64 = 300.0;

    /// How long the Reset pulse stays high: long enough to see on its jack,
    /// short enough to be over before the next sixteenth at any tempo.
    const RESET_PULSE_SECONDS: f32 = 0.005;

    /// Readout value indices: see [`DspModule::readout`].
    pub const READOUT_TEMPO: usize = 0;
    pub const READOUT_BEAT: usize = 1;
    pub const READOUT_RUNNING: usize = 2;
    pub const READOUT_RECEIVING: usize = 3;
    /// Following Link: the peers in the session, how many times a peer has
    /// changed its tempo and its transport (the app moves the Tempo knob and
    /// the Run switch to match), and whether the session is playing.
    pub const READOUT_PEERS: usize = 4;
    pub const READOUT_LINK_TEMPO_CHANGES: usize = 5;
    pub const READOUT_LINK_PLAY_CHANGES: usize = 6;
    pub const READOUT_LINK_PLAYING: usize = 7;

    /// The tempo this clock keeps: the master's, when following MIDI and
    /// it has been heard, otherwise the Tempo knob's as Tempo CV bends it.
    fn tempo(&self, params: &[f32]) -> f32 {
        let knob = params.get(Self::PARAM_TEMPO).copied().unwrap_or(120.0);
        match Self::source(params) {
            ClockSource::Midi => self.midi.tempo_bpm(self.sample_rate).unwrap_or(knob),
            #[cfg(feature = "ableton-link")]
            ClockSource::Link if self.link.last.is_some() => self.link.last.map_or(knob, |block| block.tempo as f32),
            ClockSource::Host if self.host_tempo.is_some() => self.host_tempo.map_or(knob, |bpm| bpm as f32),
            ClockSource::Internal | ClockSource::Link | ClockSource::Host => Self::bent_tempo(knob as f64, self.tempo_scale) as f32,
        }
    }

    /// The Tempo knob's `knob` BPM bent by Tempo CV's `scale`, kept within
    /// the knob's own range.
    #[inline]
    fn bent_tempo(knob: f64, scale: f64) -> f64 {
        (knob * scale).clamp(Self::MIN_TEMPO, Self::MAX_TEMPO)
    }

    /// Beats in each pulse for a Division value, a quarter when it's out
    /// of range.
    pub fn division_beats(value: f32) -> f64 {
        tempo::beats(&DIVISIONS, value as usize).unwrap_or(1.0)
    }

    fn source(params: &[f32]) -> ClockSource {
        ClockSource::from_param(params.get(Self::PARAM_SOURCE).copied().unwrap_or(0.0))
    }

    /// Whether the gate is high at `beats`, for pulses `division` beats apart
    /// that stay high for `gate_length` (0-1) of each, swung by `swing`
    /// (0.5-0.75).
    ///
    /// Pulses come in pairs. The first of each pair is on the grid; the
    /// second lands at `swing` of the way through the pair. Each gate lasts
    /// `gate_length` of a straight pulse, or of the swung pulse's shorter
    /// slot, so the pulses keep the same proportion of on to off.
    #[inline]
    pub fn gate_at(beats: f64, division: f64, gate_length: f64, swing: f64) -> bool {
        let cycles = beats / division;
        // A beat a hair under a whole number of cycles, after rounding, is
        // on the edge, not a whole cycle before it
        let pulse = (cycles + 1e-9).floor();
        let phase = cycles - pulse;
        if pulse.rem_euclid(2.0) < 0.5 {
            return phase < gate_length;
        }
        // The swung pulse starts `delay` into its slot, and has the rest
        let delay = 2.0 * swing - 1.0;
        phase + 1e-9 >= delay && phase - delay < gate_length * (1.0 - delay)
    }

    /// Whether a Clock following Link runs at sample `i` of the block, with
    /// its beat placed on the session's. It runs once the session plays and
    /// the start's bar has come round, firing Reset as it starts from the
    /// top.
    #[cfg(feature = "ableton-link")]
    fn follow_link(&mut self, block: Option<&link::LinkBlock>, i: usize) -> bool {
        let Some(block) = block else { return false };
        let beat = block.beat_at(i);
        let running = block.playing && beat >= 0.0;
        if running {
            if !self.running && beat < 1.0 {
                self.restart();
            }
            self.beats = beat;
        }
        running
    }

    #[cfg(not(feature = "ableton-link"))]
    fn follow_link(&mut self, _block: Option<&()>, _i: usize) -> bool {
        false
    }

    /// Whether a Clock following the DAW runs at sample `i` of the block,
    /// with its beat on the DAW's. It runs while the DAW plays and Run is
    /// on, firing Reset as it starts, so sequencers start with the song.
    fn follow_host(&mut self, host: &crate::dsp::HostTransport, run: bool, i: usize, sample_rate: f32) -> bool {
        let running = run && host.playing;
        if running {
            if !self.running {
                self.restart();
            }
            self.beats = host.beat_after(i, sample_rate);
        }
        running
    }

    /// Restarts from the top and fires Reset.
    fn restart(&mut self) {
        self.beats = 0.0;
        self.reset_out.fire(Self::RESET_PULSE_SECONDS, self.sample_rate);
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Clock {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Clock",
            category: ModuleCategory::Utility,
            description: "Periodic gate trigger generator",
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
        let tempo = params[Self::PARAM_TEMPO] as f64;
        let gate_length = params[Self::PARAM_GATE_LENGTH] as f64 / 100.0;
        let division = Self::division_beats(params[Self::PARAM_DIVISION]);
        let run = params[Self::PARAM_RUN] > 0.5;
        let mut source = Self::source(params);
        let swing = params.get(Self::PARAM_SWING).map_or(0.5, |&swing| (swing as f64 / 100.0).clamp(0.5, 0.75));

        // Link: the session's time for this block, once the Clock's own
        // changes are proposed to it. Without a session, the Clock's own
        #[cfg(feature = "ableton-link")]
        let link_block = match (source, crate::ableton_link::session()) {
            (ClockSource::Link, Some(session)) => {
                let limiter = crate::dsp::dynamics::PeakLimiter::latency_at(self.sample_rate);
                Some(self.link.begin_block(session, tempo, run, context.block_size, self.sample_rate, limiter))
            }
            _ => {
                self.link.rest();
                None
            }
        };
        #[cfg(not(feature = "ableton-link"))]
        let link_block: Option<()> = None;
        if source == ClockSource::Link && link_block.is_none() {
            source = ClockSource::Internal;
        }
        // The DAW's transport, or the Clock's own time without one
        let host = context.host.filter(|_| source == ClockSource::Host);
        self.host_tempo = host.map(|host| host.tempo_bpm);
        if source == ClockSource::Host && host.is_none() {
            source = ClockSource::Internal;
        }

        // Switching Run on starts from the top, as a drum machine's Play does
        if source == ClockSource::Internal && run && self.prev_run == Some(false) {
            self.restart();
        }
        self.prev_run = Some(run);

        let sync_in = inputs.get(Self::PORT_SYNC);
        let tempo_cv = connected_input(inputs, Self::PORT_TEMPO);
        if tempo_cv.is_none() {
            self.tempo_scale = 1.0;
        }
        let minutes_per_sample = 1.0 / 60.0 / self.sample_rate as f64;
        let mut beats_per_sample = Self::bent_tempo(tempo, self.tempo_scale) * minutes_per_sample;
        let mut midi = context.midi.iter().peekable();

        for i in 0..context.block_size {
            // Check for sync reset
            let sync_rising = self.sync.rising(sync_in.map_or(0.0, |buf| buf.at(i)));
            // Following MIDI, the master's ticks place the beat instead
            if sync_rising && source == ClockSource::Internal {
                self.beats = 0.0;
            }

            // MIDI lands on its sample, whatever the source, so the follower
            // already knows the tempo when Source is switched to MIDI
            while let Some(event) = midi.next_if(|event| event.sample_offset as usize <= i) {
                match self.midi.handle(event.message, self.sample_rate) {
                    _ if source == ClockSource::Internal => {}
                    MidiClockChange::Beat(beat) | MidiClockChange::Locate(beat) => self.beats = beat,
                    MidiClockChange::Restart => self.restart(),
                    MidiClockChange::None => {}
                }
            }

            self.running = match source {
                ClockSource::Internal => run,
                ClockSource::Midi => run && self.midi.is_running(),
                ClockSource::Link => self.follow_link(link_block.as_ref(), i),
                ClockSource::Host => host.is_some_and(|host| self.follow_host(&host, run, i, self.sample_rate)),
            };

            let gate = self.running && Self::gate_at(self.beats, division, gate_length, swing);
            outputs[Self::PORT_GATE].samples[i] = if gate { 1.0 } else { 0.0 };
            if let Some(out) = outputs.get_mut(Self::PORT_RUN) {
                out.samples[i] = if self.running { 1.0 } else { 0.0 };
            }
            let reset = self.reset_out.sample();
            if let Some(out) = outputs.get_mut(Self::PORT_RESET) {
                out.samples[i] = reset;
            }

            // Advance the beat, at the tempo Tempo CV bends it to: by octaves,
            // so a ramp in the CV is an even slowing in the ear
            if let Some(cv) = tempo_cv {
                let octaves = cv.at(i) as f64;
                self.tempo_scale = octaves.exp2();
                beats_per_sample = Self::bent_tempo(tempo, self.tempo_scale) * minutes_per_sample;
            }
            if self.running {
                match source {
                    ClockSource::Internal => self.beats += beats_per_sample,
                    // Placed from the session's or the DAW's beat
                    ClockSource::Link | ClockSource::Host => {}
                    // Between ticks, at the master's pace, but never past the
                    // next tick: that beat is the tick's to set
                    ClockSource::Midi => {
                        if let Some(spt) = self.midi.samples_per_tick() {
                            let step = 1.0 / (spt * MIDI_TICKS_PER_BEAT as f64);
                            self.beats = (self.beats + step).min(self.midi.next_beat() - 1e-9);
                        }
                    }
                }
            }
            self.midi.advance();
        }
    }

    fn reset(&mut self) {
        self.beats = 0.0;
        self.running = false;
        self.sync.reset();
        self.tempo_scale = 1.0;
        self.prev_run = None;
        self.reset_out.clear();
        self.midi = MidiClockFollower::new();
        #[cfg(feature = "ableton-link")]
        self.link.rest();
        self.host_tempo = None;
    }

    fn transport(&self, params: &[f32]) -> Option<TransportState> {
        // Before the first block, a running clock is about to play
        let playing = match self.prev_run {
            None => params.get(Self::PARAM_RUN).is_some_and(|&run| run > 0.5) && Self::source(params) == ClockSource::Internal,
            Some(_) => self.running,
        };
        Some(TransportState {
            playing,
            tempo_bpm: Some(self.tempo(params)),
            beat_position: Some(self.beats),
            ..TransportState::new()
        })
    }

    /// The tempo it keeps, its beat within the bar, whether it's running,
    /// and whether MIDI clock is coming in.
    fn readout(&self, params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_TEMPO] = self.tempo(params);
        readout.values[Self::READOUT_BEAT] = self.beats.rem_euclid(4.0) as f32;
        readout.values[Self::READOUT_RUNNING] = if self.running { 1.0 } else { 0.0 };
        // Following the DAW, whether it's reporting its transport
        let receiving = match Self::source(params) {
            ClockSource::Host => self.host_tempo.is_some(),
            _ => self.midi.is_receiving(self.sample_rate),
        };
        readout.values[Self::READOUT_RECEIVING] = if receiving { 1.0 } else { 0.0 };
        #[cfg(feature = "ableton-link")]
        if let Some(block) = self.link.last.filter(|_| Self::source(params) == ClockSource::Link) {
            readout.values[Self::READOUT_PEERS] = crate::ableton_link::peers() as f32;
            readout.values[Self::READOUT_LINK_TEMPO_CHANGES] = self.link.tempo_changes as f32;
            readout.values[Self::READOUT_LINK_PLAY_CHANGES] = self.link.play_changes as f32;
            readout.values[Self::READOUT_LINK_PLAYING] = if block.playing { 1.0 } else { 0.0 };
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::MidiEvent;

    /// Buffers for the Clock's Gate, Run and Reset outputs.
    fn outputs(frames: usize) -> Vec<SignalBuffer> {
        vec![SignalBuffer::control(frames); 3]
    }

    #[test]
    fn test_clock_info() {
        let clock = Clock::new();
        assert_eq!(clock.info().id, "util.clock");
        assert_eq!(clock.info().name, "Clock");
        assert_eq!(clock.info().category, ModuleCategory::Utility);
    }

    #[test]
    fn test_clock_ports() {
        let clock = Clock::new();
        let ports = clock.ports();

        assert_eq!(ports.len(), 5);

        // Sync input
        assert!(ports[0].is_input());
        assert_eq!(ports[0].id, "sync");
        assert_eq!(ports[0].signal_type, SignalType::Gate);

        // Tempo CV, late so an Arranger the Clock drives can patch back
        // into it
        assert!(ports[1].is_input());
        assert_eq!(ports[1].id, "tempo_cv");
        assert_eq!(ports[1].name, "Tempo");
        assert!(ports[1].late);

        // Gate output
        assert!(ports[2].is_output());
        assert_eq!(ports[2].id, "gate");
        assert_eq!(ports[2].signal_type, SignalType::Gate);
    }

    #[test]
    fn test_clock_parameters() {
        let clock = Clock::new();
        let params = clock.parameters();

        assert_eq!(params.len(), 6);

        // Tempo
        assert_eq!(params[0].id, "tempo");
        assert_eq!(params[0].min, 20.0);
        assert_eq!(params[0].max, 300.0);
        assert_eq!(params[0].default, 120.0);

        // Gate Length
        assert_eq!(params[1].id, "gate_length");
        assert_eq!(params[1].min, 1.0);
        assert_eq!(params[1].max, 99.0);
        assert_eq!(params[1].default, 50.0);

        // Division
        assert_eq!(params[2].id, "division");
        assert_eq!(params[2].default, 2.0); // Quarter note

        // Run
        assert_eq!(params[3].id, "run");
        assert_eq!(params[3].default, 1.0); // Running by default

        // Swing: straight by default
        assert_eq!(params[5].id, "swing");
        assert_eq!(params[5].min, 50.0);
        assert_eq!(params[5].max, 75.0);
        assert_eq!(params[5].default, 50.0);
    }

    #[test]
    fn test_clock_division_beats() {
        assert_eq!(Clock::division_beats(0.0), 4.0);
        assert_eq!(Clock::division_beats(1.0), 2.0);
        assert_eq!(Clock::division_beats(2.0), 1.0);
        assert_eq!(Clock::division_beats(3.0), 0.5);
        assert_eq!(Clock::division_beats(4.0), 0.25);
        assert_eq!(Clock::division_beats(99.0), 1.0); // Out of range: a quarter
    }

    #[test]
    fn test_clock_stopped_outputs_zero() {
        let mut clock = Clock::new();
        clock.prepare(44100.0, 256);

        let mut outs = outputs(256);
        let ctx = ProcessContext::new(44100.0, 256);

        // Run = false (0.0)
        clock.process(&[], &mut outs, &[120.0, 50.0, 2.0, 0.0, 0.0], &ctx);

        // All outputs should be zero when stopped
        assert!(outs[0].samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_clock_generates_gates() {
        let mut clock = Clock::new();
        let sample_rate = 44100.0;
        clock.prepare(sample_rate, 44100);

        let mut outputs = outputs(44100);
        let ctx = ProcessContext::new(sample_rate, 44100);

        // 120 BPM, 50% gate, quarter note, running
        // At 120 BPM: 2 beats per second, so 22050 samples per beat
        clock.process(&[], &mut outputs, &[120.0, 50.0, 2.0, 1.0, 0.0], &ctx);

        // Should have both high and low values
        let has_high = outputs[0].samples.contains(&1.0);
        let has_low = outputs[0].samples.contains(&0.0);
        assert!(has_high, "Clock should output high gates");
        assert!(has_low, "Clock should output low between gates");

        // Output should only be 0.0 or 1.0
        for &sample in &outputs[0].samples {
            assert!(
                sample == 0.0 || sample == 1.0,
                "Gate output should be 0 or 1, got {}",
                sample
            );
        }
    }

    #[test]
    fn test_clock_timing_accuracy() {
        let mut clock = Clock::new();
        let sample_rate = 44100.0;
        clock.prepare(sample_rate, 44100);

        let mut outputs = outputs(44100);
        let ctx = ProcessContext::new(sample_rate, 44100);

        // 60 BPM = 1 beat per second = 44100 samples per beat
        // Quarter note division, 50% gate length
        // So gate should be high for ~22050 samples, then low for ~22050
        clock.process(&[], &mut outputs, &[60.0, 50.0, 2.0, 1.0, 0.0], &ctx);

        // Count high samples in first beat
        let high_count = outputs[0].samples[..44100]
            .iter()
            .filter(|&&s| s == 1.0)
            .count();

        // Should be approximately 50% (allowing some tolerance for edge cases)
        let expected = 22050;
        let tolerance = 100; // Allow small timing variance
        assert!(
            (high_count as i32 - expected).abs() < tolerance,
            "Expected ~{} high samples, got {}",
            expected,
            high_count
        );
    }

    #[test]
    fn test_clock_gate_length() {
        let mut clock = Clock::new();
        let sample_rate = 44100.0;
        clock.prepare(sample_rate, 44100);

        // Test with 25% gate length
        let mut outputs = outputs(44100);
        let ctx = ProcessContext::new(sample_rate, 44100);

        // 60 BPM, 25% gate, quarter note
        clock.process(&[], &mut outputs, &[60.0, 25.0, 2.0, 1.0, 0.0], &ctx);

        let high_count = outputs[0].samples[..44100]
            .iter()
            .filter(|&&s| s == 1.0)
            .count();

        // Should be approximately 25%
        let expected = 11025;
        let tolerance = 100;
        assert!(
            (high_count as i32 - expected).abs() < tolerance,
            "Expected ~{} high samples for 25% gate, got {}",
            expected,
            high_count
        );
    }

    #[test]
    fn test_clock_division_timing() {
        let mut clock = Clock::new();
        let sample_rate = 44100.0;
        clock.prepare(sample_rate, 88200); // 2 seconds

        let mut outputs = outputs(88200);
        let ctx = ProcessContext::new(sample_rate, 88200);

        // 60 BPM, eighth notes (0.5 beats)
        // At 60 BPM: 1 beat/sec, eighth = 0.5 beats = 0.5 sec = 22050 samples per cycle
        // In 2 seconds, should get 4 complete cycles
        clock.process(&[], &mut outputs, &[60.0, 50.0, 3.0, 1.0, 0.0], &ctx);

        // Count rising edges (transitions from 0 to 1)
        let mut rising_edges = 0;
        let mut prev = 0.0;
        for &sample in &outputs[0].samples {
            if sample == 1.0 && prev == 0.0 {
                rising_edges += 1;
            }
            prev = sample;
        }

        // Should have 4 rising edges (4 eighth notes in 2 seconds at 60 BPM)
        // First rising edge is at start, so we should see 4 total
        assert!(
            (3..=5).contains(&rising_edges),
            "Expected ~4 rising edges for eighth notes, got {}",
            rising_edges
        );
    }

    #[test]
    fn test_clock_sync_reset() {
        let mut clock = Clock::new();
        let sample_rate = 44100.0;
        clock.prepare(sample_rate, 1000);

        // Run clock to advance phase
        let mut outs = outputs(1000);
        let ctx = ProcessContext::new(sample_rate, 1000);
        clock.process(&[], &mut outs, &[120.0, 50.0, 2.0, 1.0, 0.0], &ctx);

        // Now send a sync pulse
        let mut sync = SignalBuffer::control(100);
        sync.samples[50] = 1.0; // Rising edge at sample 50

        let mut outputs2 = outputs(100);
        let ctx2 = ProcessContext::new(sample_rate, 100);
        clock.process(&[&sync], &mut outputs2, &[120.0, 50.0, 2.0, 1.0, 0.0], &ctx2);

        // After sync, the gate should be high (phase reset to 0, which is < gate_length)
        assert_eq!(
            outputs2[0].samples[51], 1.0,
            "Gate should be high immediately after sync"
        );
    }

    #[test]
    fn test_clock_reset() {
        let mut clock = Clock::new();
        clock.prepare(44100.0, 256);

        // Advance the clock
        let mut outs = outputs(256);
        let ctx = ProcessContext::new(44100.0, 256);
        clock.process(&[], &mut outs, &[120.0, 50.0, 2.0, 1.0, 0.0], &ctx);

        // Reset
        clock.reset();

        // Phase should be back to 0, so first output should be high (0 < 0.5 gate length)
        let mut outputs2 = outputs(1);
        let ctx2 = ProcessContext::new(44100.0, 1);
        clock.process(&[], &mut outputs2, &[120.0, 50.0, 2.0, 1.0, 0.0], &ctx2);

        assert_eq!(
            outputs2[0].samples[0], 1.0,
            "First sample after reset should be high"
        );
    }

    #[test]
    fn test_clock_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Clock>();
    }

    #[test]
    fn test_clock_default() {
        let clock = Clock::default();
        assert_eq!(clock.info().id, "util.clock");
    }

    #[test]
    fn test_clock_registry_instantiation() {
        use crate::dsp::ModuleRegistry;

        let mut registry = ModuleRegistry::new();
        registry.register::<Clock>();

        assert!(registry.contains("util.clock"));

        let module = registry.create("util.clock");
        assert!(module.is_some());

        let module = module.unwrap();
        assert_eq!(module.info().id, "util.clock");
        assert_eq!(module.info().name, "Clock");
        assert_eq!(module.ports().len(), 5);
        assert_eq!(module.parameters().len(), 6);
    }

    #[test]
    fn test_clock_fast_tempo() {
        let mut clock = Clock::new();
        let sample_rate = 44100.0;
        clock.prepare(sample_rate, 44100);

        let mut outputs = outputs(44100);
        let ctx = ProcessContext::new(sample_rate, 44100);

        // 300 BPM = 5 beats per second, sixteenth notes = 20 triggers per second
        clock.process(&[], &mut outputs, &[300.0, 50.0, 4.0, 1.0, 0.0], &ctx);

        // Count rising edges
        let mut rising_edges = 0;
        let mut prev = 0.0;
        for &sample in &outputs[0].samples {
            if sample == 1.0 && prev == 0.0 {
                rising_edges += 1;
            }
            prev = sample;
        }

        // Should have approximately 20 triggers in 1 second
        assert!(
            (18..=22).contains(&rising_edges),
            "Expected ~20 triggers at 300 BPM sixteenths, got {}",
            rising_edges
        );
    }

    #[test]
    fn test_clock_slow_tempo() {
        let mut clock = Clock::new();
        let sample_rate = 44100.0;
        clock.prepare(sample_rate, 88200); // 2 seconds

        let mut outputs = outputs(88200);
        let ctx = ProcessContext::new(sample_rate, 88200);

        // 30 BPM = 0.5 beats per second, whole notes = 1 trigger per 8 seconds
        // In 2 seconds, should see only partial first cycle
        clock.process(&[], &mut outputs, &[30.0, 50.0, 0.0, 1.0, 0.0], &ctx);

        // Count rising edges - should be just 1 (the initial start)
        let mut rising_edges = 0;
        let mut prev = 0.0;
        for &sample in &outputs[0].samples {
            if sample == 1.0 && prev == 0.0 {
                rising_edges += 1;
            }
            prev = sample;
        }

        assert!(
            rising_edges <= 2,
            "Expected 1-2 triggers at 30 BPM whole notes in 2 seconds, got {}",
            rising_edges
        );
    }

    // --- Timing over long runs (#96) ---

    /// Renders `seconds` in 512-sample blocks and returns the sample of
    /// every rising edge on the Gate.
    fn rising_edges(params: &[f32], sample_rate: f32, seconds: f64) -> Vec<u64> {
        const BLOCK: usize = 512;
        let mut clock = Clock::new();
        clock.prepare(sample_rate, BLOCK);
        let mut outs = outputs(BLOCK);
        let ctx = ProcessContext::new(sample_rate, BLOCK);
        let blocks = (seconds * sample_rate as f64 / BLOCK as f64).ceil() as u64;
        let (mut edges, mut prev) = (Vec::new(), 0.0);
        for block in 0..blocks {
            clock.process(&[], &mut outs, params, &ctx);
            for (i, &gate) in outs[0].samples.iter().enumerate() {
                if gate > 0.5 && prev < 0.5 {
                    edges.push(block * BLOCK as u64 + i as u64);
                }
                prev = gate;
            }
        }
        edges
    }

    /// Every edge within one sample of `n × period`.
    fn assert_edges_on_grid(edges: &[u64], period: f64, expected: usize) {
        assert!(edges.len() >= expected, "{} edges, expected {expected}", edges.len());
        for (n, &edge) in edges.iter().enumerate() {
            let ideal = n as f64 * period;
            assert!((edge as f64 - ideal).abs() <= 1.0, "edge {n} at {edge}, ideal {ideal}");
        }
    }

    #[test]
    fn test_slow_clock_keeps_every_edge_on_its_sample_for_ten_minutes() {
        // 24 BPM whole notes: one edge every 10 s
        let edges = rising_edges(&[24.0, 50.0, 0.0, 1.0, 0.0], 48000.0, 600.0);
        assert_edges_on_grid(&edges, 480_000.0, 60);
    }

    #[test]
    fn test_fast_clock_keeps_every_edge_on_its_sample_for_ten_minutes() {
        // 300 BPM sixteenths: one edge every 50 ms
        let edges = rising_edges(&[300.0, 50.0, 4.0, 1.0, 0.0], 48000.0, 600.0);
        assert_edges_on_grid(&edges, 2400.0, 12_000);
    }

    // --- Swing (#99) ---

    /// 120 BPM sixteenths, 50% gates, swung by `swing` percent.
    fn sixteenths(swing: f32) -> [f32; 6] {
        [120.0, 50.0, 4.0, 1.0, 0.0, swing]
    }

    #[test]
    fn test_swing_lands_even_sixteenths_at_its_share_of_the_eighth() {
        // At 48 kHz, 120 BPM: a sixteenth is 6000 samples, an eighth 12000
        for (swing, at) in [(200.0 / 3.0, 8000.0), (66.0, 7920.0), (58.0, 6960.0), (75.0, 9000.0)] {
            let edges = rising_edges(&sixteenths(swing), 48000.0, 120.0);
            assert!(edges.len() >= 960, "{} edges at {swing}%", edges.len());
            for (n, &edge) in edges.iter().enumerate() {
                let pair = (n / 2) as f64 * 12000.0;
                let ideal = if n % 2 == 0 { pair } else { pair + at };
                assert!((edge as f64 - ideal).abs() <= 1.0, "edge {n} at {edge}, ideal {ideal}, swing {swing}%");
            }
        }
    }

    #[test]
    fn test_swing_leaves_the_grid_pulses_and_bars_alone() {
        let straight = rising_edges(&sixteenths(50.0), 48000.0, 60.0);
        let swung = rising_edges(&sixteenths(66.0), 48000.0, 60.0);
        assert_eq!(straight.len(), swung.len());
        // Every first pulse of a pair, the downbeats among them, on the same
        // sample as straight
        for n in (0..straight.len()).step_by(2) {
            assert_eq!(straight[n], swung[n], "pulse {n}");
        }
    }

    #[test]
    fn test_straight_swing_is_the_clock_without_it() {
        // Patches saved before Swing have five parameters
        let before = rising_edges(&[120.0, 50.0, 4.0, 1.0, 0.0], 44100.0, 30.0);
        assert_eq!(before, rising_edges(&sixteenths(50.0), 44100.0, 30.0));
    }

    #[test]
    fn test_swung_gates_keep_their_shape_and_never_run_together() {
        // At 99% gates and the hardest swing, every pulse still has its edge
        let edges = rising_edges(&[120.0, 99.0, 4.0, 1.0, 0.0, 75.0], 48000.0, 5.99);
        assert_eq!(edges.len(), 6 * 8);

        // At 50% the grid pulse keeps its 3000 samples; the swung one gets
        // half its 3000-sample slot
        let mut clock = Clock::new();
        clock.prepare(48000.0, 24000);
        let mut outs = outputs(24000);
        clock.process(&[], &mut outs, &sixteenths(75.0), &ProcessContext::new(48000.0, 24000));
        let gate = &outs[0].samples;
        let high = |from: usize| gate[from..].iter().take_while(|&&s| s == 1.0).count();
        assert!(high(0).abs_diff(3000) <= 1, "grid gate {}", high(0));
        assert!(high(9000).abs_diff(1500) <= 1, "swung gate {}", high(9000));
        assert_eq!(gate[8999], 0.0);
    }

    #[test]
    fn test_swing_keeps_the_sequencers_end_of_cycle() {
        use crate::modules::StepSequencer;
        // A 16-step sequencer on the clock, its EOC once a bar
        let eoc = |swing: f32| {
            let mut clock = Clock::new();
            let mut seq = StepSequencer::new();
            clock.prepare(48000.0, 512);
            seq.prepare(48000.0, 512);
            let mut seq_params: Vec<f32> = seq.parameters().iter().map(|p| p.default).collect();
            seq_params[0] = 16.0; // Steps
            let mut clock_outs = outputs(512);
            let mut seq_outs = vec![SignalBuffer::control(512); 5];
            let ctx = ProcessContext::new(48000.0, 512);
            let mut eoc = Vec::new();
            for _ in 0..48000 * 10 / 512 {
                clock.process(&[], &mut clock_outs, &sixteenths(swing), &ctx);
                seq.process(&[&clock_outs[0]], &mut seq_outs, &seq_params, &ctx);
                eoc.extend_from_slice(&seq_outs[4].samples);
            }
            rises(&eoc)
        };
        let straight = eoc(50.0);
        assert_eq!(straight.len(), 4, "a bar every 2 s at 120 BPM: {straight:?}");
        assert_eq!(straight, eoc(66.0));
    }

    // --- Tempo CV (#180) ---

    /// Renders `frames` in 512-sample blocks with Tempo CV following
    /// `cv(frame)`, and returns the Gate and the Clock.
    fn bent(params: &[f32], frames: usize, cv: impl Fn(usize) -> f32) -> (Vec<f32>, Clock) {
        const BLOCK: usize = 512;
        let mut clock = Clock::new();
        clock.prepare(48000.0, BLOCK);
        let mut outs = outputs(BLOCK);
        let ctx = ProcessContext::new(48000.0, BLOCK);
        let sync = SignalBuffer::control(BLOCK);
        let mut tempo = SignalBuffer::control(BLOCK);
        let mut gate = Vec::new();
        for start in (0..frames).step_by(BLOCK) {
            for (i, sample) in tempo.samples.iter_mut().enumerate() {
                *sample = cv(start + i);
            }
            clock.process(&[&sync, &tempo], &mut outs, params, &ctx);
            gate.extend_from_slice(&outs[0].samples);
        }
        (gate, clock)
    }

    #[test]
    fn test_tempo_cv_bends_the_tempo_by_octaves() {
        // 60 BPM quarters at 48 kHz: a pulse every 48000 samples, every
        // 24000 at +1, every 96000 at -1, and every 48000 / 2^0.5 at +0.5
        let params = [60.0, 50.0, 2.0, 1.0, 0.0];
        for (cv, period) in [(0.0, 48000.0), (1.0, 24000.0), (-1.0, 96000.0), (0.5, 48000.0 / 2f64.sqrt())] {
            let (gate, clock) = bent(&params, 48000 * 8, |_| cv);
            let edges = rises(&gate);
            // The first pulse opens on sample 0, before any edge to count
            assert!(edges.len() >= 2, "CV {cv}: {edges:?}");
            for (n, &edge) in edges.iter().enumerate() {
                let ideal = (n + 1) as f64 * period;
                assert!((edge as f64 - ideal).abs() <= 1.0, "CV {cv}: edge {n} at {edge}, ideal {ideal}");
            }
            // The transport and the readout carry the tempo it's playing at
            let bpm = 60.0 * 2f32.powf(cv);
            assert!((clock.transport(&params).unwrap().tempo_bpm.unwrap() - bpm).abs() < 0.01, "CV {cv}");
            assert!((clock.readout(&params).unwrap().values[Clock::READOUT_TEMPO] - bpm).abs() < 0.01, "CV {cv}");
        }
    }

    #[test]
    fn test_tempo_cv_stays_within_the_knobs_range() {
        let tempo = |knob: f32, cv: f32| {
            let params = [knob, 50.0, 2.0, 1.0, 0.0];
            let (_, clock) = bent(&params, 512, |_| cv);
            clock.transport(&params).unwrap().tempo_bpm.unwrap()
        };
        assert_eq!(tempo(200.0, 1.0), 300.0);
        assert_eq!(tempo(30.0, -1.0), 20.0);
        // A CV that's been unpatched leaves the knob alone again
        let params = [90.0, 50.0, 2.0, 1.0, 0.0];
        let (_, mut clock) = bent(&params, 512, |_| 1.0);
        clock.process(&[], &mut outputs(512), &params, &ProcessContext::new(48000.0, 512));
        assert_eq!(clock.transport(&params).unwrap().tempo_bpm, Some(90.0));
    }

    #[test]
    fn test_tempo_cv_is_ignored_while_following_midi() {
        let (_, clock) = bent(&MIDI, 512, |_| 1.0);
        // No ticks yet: the knob, unbent
        assert_eq!(clock.transport(&MIDI).unwrap().tempo_bpm, Some(120.0));
    }

    #[test]
    fn test_a_ritardando_slows_a_sequence_with_its_gates_in_proportion() {
        use crate::modules::StepSequencer;
        // 120 BPM sixteenths into a Step Sequencer. Two bars in time, then a
        // two-bar lane from 0 down to log2(0.85): the last bars broaden to
        // 102 BPM, as a pianist slows into the last chord
        const BLOCK: usize = 512;
        let sr = 48000.0;
        let bar = 4.0 * 60.0 / 120.0 * sr as f64;
        let rit = 0.85f64.log2();
        let cv = |frame: usize| {
            let through = ((frame as f64 - 2.0 * bar) / (2.0 * bar)).clamp(0.0, 1.0);
            (through * rit) as f32
        };
        let mut clock = Clock::new();
        let mut seq = StepSequencer::new();
        clock.prepare(sr, BLOCK);
        seq.prepare(sr, BLOCK);
        let clock_params = [120.0, 50.0, 4.0, 1.0, 0.0];
        let mut seq_params: Vec<f32> = seq.parameters().iter().map(|p| p.default).collect();
        seq_params[0] = 16.0; // Steps
        let mut clock_outs = outputs(BLOCK);
        let mut seq_outs = vec![SignalBuffer::control(BLOCK); 5];
        let ctx = ProcessContext::new(sr, BLOCK);
        let sync = SignalBuffer::control(BLOCK);
        let mut tempo = SignalBuffer::control(BLOCK);
        let (mut clock_gate, mut seq_gate) = (Vec::new(), Vec::new());
        for start in (0..(5.0 * bar) as usize).step_by(BLOCK) {
            for (i, sample) in tempo.samples.iter_mut().enumerate() {
                *sample = cv(start + i);
            }
            clock.process(&[&sync, &tempo], &mut clock_outs, &clock_params, &ctx);
            seq.process(&[&clock_outs[0]], &mut seq_outs, &seq_params, &ctx);
            clock_gate.extend_from_slice(&clock_outs[0].samples);
            seq_gate.extend_from_slice(&seq_outs[1].samples);
        }

        // The steps never shorten, and broaden smoothly: 15% over 32 steps
        // is half a percent a step, and none is more than 1% longer than
        // the one before it
        let edges = rises(&clock_gate);
        let steps: Vec<f64> = edges.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
        for pair in steps.windows(2) {
            assert!(pair[1] >= pair[0] - 1.0, "a step shortened: {pair:?}");
            assert!(pair[1] <= pair[0] * 1.01 + 1.0, "a step jumped: {pair:?}");
        }
        assert!((steps[0] - 6000.0).abs() <= 1.0, "in time first: {}", steps[0]);
        let last = *steps.last().unwrap();
        assert!((last - 6000.0 / 0.85).abs() < 10.0, "102 BPM at the end: {last}");
        assert!((clock.transport(&clock_params).unwrap().tempo_bpm.unwrap() - 102.0).abs() < 0.01);

        // The sequencer plays a note on every step, each half its step long
        let notes = rises(&seq_gate);
        assert_eq!(notes, edges, "a note on every clock pulse");
        for (n, pair) in notes.windows(2).enumerate().skip(1) {
            let step = (pair[1] - pair[0]) as f64;
            let held = seq_gate[pair[0]..pair[1]].iter().filter(|&&g| g > 0.5).count() as f64;
            assert!((held / step - 0.5).abs() < 0.01, "note {n} held {held} of a {step}-sample step");
        }
    }

    // --- Run and Reset ---

    #[test]
    fn test_switching_run_on_starts_from_the_top_and_fires_reset() {
        let mut clock = Clock::new();
        clock.prepare(44100.0, 1000);
        let mut outs = outputs(1000);
        let ctx = ProcessContext::new(44100.0, 1000);

        // Running from the load fires no Reset
        clock.process(&[], &mut outs, &[120.0, 50.0, 2.0, 1.0, 0.0], &ctx);
        assert!(outs[2].samples.iter().all(|&s| s == 0.0));
        assert!(outs[1].samples.iter().all(|&s| s == 1.0), "Run is high while running");

        // Stopped: everything low, the beat holds
        clock.process(&[], &mut outs, &[120.0, 50.0, 2.0, 0.0, 0.0], &ctx);
        assert!(outs.iter().all(|out| out.samples.iter().all(|&s| s == 0.0)));
        let held = clock.transport(&[120.0, 50.0, 2.0, 0.0, 0.0]).unwrap();
        assert!(!held.playing);
        assert!(held.beat_position.unwrap() > 0.0);

        // Back on: from the downbeat, with a Reset pulse
        clock.process(&[], &mut outs, &[120.0, 50.0, 2.0, 1.0, 0.0], &ctx);
        assert_eq!(outs[0].samples[0], 1.0, "the gate opens on the downbeat");
        assert_eq!(outs[2].samples[0], 1.0, "Reset fires");
        let pulse = outs[2].samples.iter().filter(|&&s| s == 1.0).count();
        assert_eq!(pulse, (0.005f32 * 44100.0).round() as usize);
    }

    #[test]
    fn test_transport_reports_the_beat() {
        let mut clock = Clock::new();
        clock.prepare(48000.0, 24000);
        let params = [120.0, 50.0, 2.0, 1.0, 0.0];
        let before = clock.transport(&params).unwrap();
        assert!(before.playing);
        assert_eq!(before.beat_position, Some(0.0));
        assert_eq!(before.tempo_bpm, Some(120.0));

        // Half a second at 120 BPM is one beat
        clock.process(&[], &mut outputs(24000), &params, &ProcessContext::new(48000.0, 24000));
        let after = clock.transport(&params).unwrap();
        assert!((after.beat_position.unwrap() - 1.0).abs() < 1e-9);
    }

    // --- Link ---

    #[test]
    fn test_link_is_a_source_only_in_builds_with_it() {
        let clock = Clock::new();
        let source = &clock.parameters()[Clock::PARAM_SOURCE];
        let offered = source.max as usize + 1;
        assert_eq!(offered, if cfg!(feature = "ableton-link") { 3 } else { 2 });
        let expected = if cfg!(feature = "ableton-link") { ClockSource::Link } else { ClockSource::Internal };
        assert_eq!(ClockSource::from_param(2.0), expected);
        assert_eq!(ClockSource::from_param(1.0), ClockSource::Midi);
    }

    #[test]
    fn test_link_without_a_session_keeps_its_own_time() {
        // The renderer and the tests have no session: a Link Clock plays
        // as an Internal one does, at its Tempo
        let link = [100.0, 50.0, 2.0, 1.0, 2.0];
        let internal = [100.0, 50.0, 2.0, 1.0, 0.0];
        let (mut a, mut b) = (Clock::new(), Clock::new());
        a.prepare(48000.0, 512);
        b.prepare(48000.0, 512);
        let ctx = ProcessContext::new(48000.0, 512);
        for _ in 0..200 {
            let (mut out_a, mut out_b) = (outputs(512), outputs(512));
            a.process(&[], &mut out_a, &link, &ctx);
            b.process(&[], &mut out_b, &internal, &ctx);
            assert_eq!(out_a[0].samples, out_b[0].samples);
        }
        assert_eq!(a.transport(&link).unwrap().beat_position, b.transport(&internal).unwrap().beat_position);
        assert_eq!(a.transport(&link).unwrap().tempo_bpm, Some(100.0));
    }

    // --- Following MIDI clock ---

    const MIDI: [f32; 5] = [120.0, 50.0, 2.0, 1.0, 1.0];

    /// Feeds a clock `events` (frame, message) over `frames` frames in
    /// 256-sample blocks, with `params`. Returns its Gate, Run and Reset.
    fn feed(clock: &mut Clock, params: &[f32], events: &[(u64, MidiMessage)], frames: u64, sample_rate: f32) -> [Vec<f32>; 3] {
        const BLOCK: u64 = 256;
        let mut outs = outputs(BLOCK as usize);
        let mut recorded: [Vec<f32>; 3] = Default::default();
        let mut start = 0;
        while start < frames {
            let block: Vec<MidiEvent> = events
                .iter()
                .filter(|(frame, _)| (start..start + BLOCK).contains(frame))
                .map(|&(frame, message)| MidiEvent::new((frame - start) as u32, 0, message))
                .collect();
            let ctx = ProcessContext::new(sample_rate, BLOCK as usize).with_midi(&block);
            clock.process(&[], &mut outs, params, &ctx);
            for (out, record) in outs.iter().zip(&mut recorded) {
                record.extend_from_slice(&out.samples);
            }
            start += BLOCK;
        }
        recorded
    }

    /// A clock following MIDI, fed `events`: see [`feed`].
    fn follow(clock: &mut Clock, events: &[(u64, MidiMessage)], frames: u64, sample_rate: f32) -> [Vec<f32>; 3] {
        feed(clock, &MIDI, events, frames, sample_rate)
    }

    /// `count` ticks at `bpm`, from `from`, each nudged by `jitter(n)` frames.
    fn ticks(bpm: f64, sample_rate: f64, from: u64, count: u64, jitter: impl Fn(u64) -> f64) -> Vec<(u64, MidiMessage)> {
        let spacing = 60.0 * sample_rate / (bpm * MIDI_TICKS_PER_BEAT as f64);
        (0..count)
            .map(|n| ((from as f64 + n as f64 * spacing + jitter(n)).round().max(0.0) as u64, MidiMessage::Clock))
            .collect()
    }

    fn rises(signal: &[f32]) -> Vec<usize> {
        (1..signal.len()).filter(|&i| signal[i] > 0.5 && signal[i - 1] < 0.5).collect()
    }

    #[test]
    fn test_midi_ticks_at_120_bpm_read_as_120() {
        for sample_rate in [44100.0, 48000.0] {
            let mut clock = Clock::new();
            clock.prepare(sample_rate, 256);
            let events = ticks(120.0, sample_rate as f64, 100, 24 * 8, |_| 0.0);
            follow(&mut clock, &events, sample_rate as u64 * 4, sample_rate);
            let tempo = clock.transport(&MIDI).unwrap().tempo_bpm.unwrap();
            assert!((tempo - 120.0).abs() <= 0.1, "{tempo} BPM at {sample_rate} Hz");
        }
    }

    #[test]
    fn test_midi_tempo_reads_through_timing_jitter() {
        // Ticks scattered by up to half a millisecond either way, as USB
        // MIDI from a DAW can be
        let mut seed = 12345u64;
        let mut offsets = Vec::new();
        for _ in 0..24 * 8 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            offsets.push(((seed >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 48.0);
        }
        for bpm in [90.0, 120.0, 174.0] {
            let mut clock = Clock::new();
            clock.prepare(48000.0, 256);
            let events = ticks(bpm, 48000.0, 500, 24 * 8, |n| offsets[n as usize]);
            follow(&mut clock, &events, events.last().unwrap().0 + 10, 48000.0);
            let tempo = clock.transport(&MIDI).unwrap().tempo_bpm.unwrap() as f64;
            assert!((tempo - bpm).abs() <= 0.1, "{tempo} BPM, sent {bpm}");
        }
    }

    #[test]
    fn test_midi_tempo_follows_a_change() {
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        let mut events = ticks(120.0, 48000.0, 0, 24 * 4, |_| 0.0);
        let switch = events.last().unwrap().0 + 1000;
        events.extend(ticks(90.0, 48000.0, switch, 24 * 4, |_| 0.0));
        // Two beats at the new tempo fill the fit window
        let settled = switch + 2 * 32_000 + 100;
        follow(&mut clock, &events, settled, 48000.0);
        let tempo = clock.transport(&MIDI).unwrap().tempo_bpm.unwrap();
        assert!((tempo - 90.0).abs() <= 0.1, "{tempo} BPM after slowing to 90");
    }

    #[test]
    fn test_midi_start_and_stop_drive_run_and_reset() {
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        // At 120 BPM a tick is 1000 frames at 48 kHz, and a beat 24000
        let mut events = vec![(500, MidiMessage::Start)];
        events.extend(ticks(120.0, 48000.0, 1000, 48, |_| 0.0));
        events.push((48_500, MidiMessage::Stop));
        let [gate, run, reset] = follow(&mut clock, &events, 60_000, 48000.0);

        // Reset fires at Start; Run rises with the first tick, the downbeat
        assert_eq!(rises(&reset), vec![500]);
        assert_eq!(rises(&run), vec![1000]);
        // A gate on each of the two beats, each on its tick
        assert_eq!(rises(&gate), vec![1000, 25_000]);
        // Stop drops Run and the gate, and they stay down
        assert!(run[48_500..].iter().all(|&s| s == 0.0));
        assert!(gate[48_500..].iter().all(|&s| s == 0.0));
        assert!(!clock.transport(&MIDI).unwrap().playing);
    }

    #[test]
    fn test_midi_continue_and_song_position_resume_where_told() {
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        let mut events = vec![(0, MidiMessage::Start)];
        events.extend(ticks(120.0, 48000.0, 1000, 30, |_| 0.0));
        events.push((31_000, MidiMessage::Stop));
        // Locate to bar 2 (sixteenth 16), then play on from there
        events.push((40_000, MidiMessage::SongPosition { sixteenths: 16 }));
        events.push((41_000, MidiMessage::Continue));
        events.extend(ticks(120.0, 48000.0, 42_000, 2, |_| 0.0));
        follow(&mut clock, &events, 43_500, 48000.0);
        // The first tick after Continue is beat 4, the second a tick on
        let beat = clock.transport(&MIDI).unwrap().beat_position.unwrap();
        assert!((beat - (4.0 + 1.0 / 24.0)).abs() < 0.03, "beat {beat}");
    }

    #[test]
    fn test_midi_ticks_without_start_run_the_clock() {
        // Simple clock sources send only ticks
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        let events = ticks(120.0, 48000.0, 1000, 48, |_| 0.0);
        let [gate, run, _] = follow(&mut clock, &events, 48_000, 48000.0);
        assert_eq!(rises(&run), vec![1000]);
        assert_eq!(rises(&gate), vec![1000, 25_000]);
    }

    #[test]
    fn test_midi_beat_glides_between_ticks_without_passing_them() {
        let mut events = vec![(0, MidiMessage::Start)];
        events.extend(ticks(120.0, 48000.0, 1000, 24, |_| 0.0));

        // 576 frames (whole blocks) after the 24th tick: beat 23/24 plus
        // 0.576 of a tick
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        follow(&mut clock, &events, 24_576, 48000.0);
        let beat = clock.transport(&MIDI).unwrap().beat_position.unwrap();
        assert!((beat - 23.576 / 24.0).abs() < 1e-4, "beat {beat}");

        // With no more ticks, it waits at the next one rather than run on
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        follow(&mut clock, &events, 40_000, 48000.0);
        let waiting = clock.transport(&MIDI).unwrap().beat_position.unwrap();
        assert!(waiting < 1.0 && waiting > 0.999, "beat {waiting}");
    }

    #[test]
    fn test_internal_clock_ignores_midi_but_hears_its_tempo() {
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        let internal = [100.0, 50.0, 2.0, 1.0, 0.0];
        let mut events = vec![(0, MidiMessage::Start)];
        events.extend(ticks(140.0, 48000.0, 1000, 48, |_| 0.0));
        let [_, _, reset] = feed(&mut clock, &internal, &events, 48_000, 48000.0);
        assert!(reset.iter().all(|&s| s == 0.0), "MIDI Start doesn't reset an internal clock");

        // Its own tempo while Internal, the master's as soon as it's MIDI
        assert_eq!(clock.transport(&internal).unwrap().tempo_bpm, Some(100.0));
        let tempo = clock.transport(&MIDI).unwrap().tempo_bpm.unwrap();
        assert!((tempo - 140.0).abs() < 0.1);
    }

    #[test]
    fn test_readout_shows_tempo_beat_and_reception() {
        let mut clock = Clock::new();
        clock.prepare(48000.0, 256);
        let events = ticks(120.0, 48000.0, 0, 60, |_| 0.0);
        follow(&mut clock, &events, 50_000, 48000.0);
        let readout = clock.readout(&MIDI).unwrap().values;
        assert!((readout[Clock::READOUT_TEMPO] - 120.0).abs() < 0.1);
        assert!(readout[Clock::READOUT_BEAT] > 2.0 && readout[Clock::READOUT_BEAT] < 2.1);
        assert_eq!(readout[Clock::READOUT_RUNNING], 1.0);
        assert_eq!(readout[Clock::READOUT_RECEIVING], 1.0);

        // A second of silence: no longer receiving
        follow(&mut clock, &[], 48_000, 48000.0);
        assert_eq!(clock.readout(&MIDI).unwrap().values[Clock::READOUT_RECEIVING], 0.0);
    }
}
