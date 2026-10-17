//! Looper module.
//!
//! Works like a looper pedal. Play into it, tap Rec to start recording, tap
//! again to close the loop, and it plays back while you play over it. Keep
//! tapping to overdub layers, undo the last one, or stop and clear.
//!
//! # The tape
//!
//! The loop lives in two banks of stereo frames, allocated when the module
//! is prepared (two minutes each), and two bitsets over them. `base` says
//! which bank holds each frame as it was before the last overdub layer, and
//! `layer` which frames that layer wrote, into the other bank. Undo hides
//! the layer and redo shows it again, so both are a flag, however long the
//! loop. Starting the next layer folds the visible one into `base`, a pass
//! over the bits rather than the audio.
//!
//! # Timing
//!
//! A loop's frames are counted from the moment the take began. Live input
//! reaches the patch a round trip after the player played it, along with
//! what they heard, so every write lands that many frames back
//! ([`ProcessContext::input_latency`], plus the Offset knob).
//!
//! The playhead counts half frames, so Speed ×½, ×1 and ×2, backwards or
//! forwards, all land exactly on the grid. Overdubbing at another speed
//! writes at that speed too: an overdub at ×½ plays back an octave up at ×1.
//!
//! # Seams
//!
//! A take keeps recording for a few milliseconds past where it closed, and
//! that pre-roll is crossfaded over the loop's first frames, so the wrap
//! goes on into what was really played next. Every overdub punches in and
//! out over the same few milliseconds, and stopping, clearing and undoing
//! fade rather than cut.
//!
//! # Keeping the loop
//!
//! A patch saves each Looper's loop as a WAV beside it. Saving copies the
//! loop as heard off the audio thread a piece per callback, into room the
//! UI set aside ([`DspModule::fill_snapshot`]), and opening the patch hands
//! the file back through [`DspModule::load_sample`], copied into the tape
//! a piece per callback too ([`DspModule::background`]), or all at once
//! before the module reaches the audio thread. A loaded loop waits,
//! Stopped at the top. The Looper counts every change to what it holds
//! (the [`READOUT_EDITS`](Looper::READOUT_EDITS) readout), so the UI can
//! tell a loop that needs saving again from one that doesn't.

use std::sync::Arc;

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::{hermite, seam_weight},
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SampleData, SignalType, Snapshot, SnapshotOutcome,
};

/// The Looper's module ID.
pub const LOOPER_ID: &str = "util.looper";

/// The longest loop, in seconds. Recording past it closes the loop there.
pub const MAX_LOOP_SECONDS: f32 = 120.0;

/// How long every punch, seam, stop, clear and undo crossfades over.
pub const FADE_SECONDS: f32 = 0.005;

/// A Rec tap this soon after a clock pulse counts as on that pulse.
pub const LATE_TAP_SECONDS: f32 = 0.05;

/// How long the Start pulse lasts.
const START_PULSE_SECONDS: f32 = 0.001;

/// The furthest the Offset knob moves overdubs, in milliseconds.
pub const MAX_OFFSET_MS: f32 = 50.0;

/// How many pieces the overview divides the loop into, for the display.
pub const OVERVIEW_SEGMENTS: usize = 256;

/// How often the overview goes to the display, in seconds.
const OVERVIEW_INTERVAL_SECONDS: f32 = 1.0 / 30.0;

/// The most frames the overview rescans in one block.
const SCAN_BUDGET: usize = 16384;

/// How long a free take's ring takes to go round before it closes, at first:
/// it doubles each time the take outgrows it.
const FREE_SCALE_SECONDS: f32 = 4.0;

/// How far a loaded loop may be from a whole number of bars of the clock
/// and still be trimmed or padded to it, in seconds: resampling a file
/// saved at another rate leaves it a frame or two off.
const SNAP_SECONDS: f32 = 0.005;

/// The change count wraps below this, so a readout's `f32` holds it exactly.
const EDITS_WRAP: u32 = (1 << 24) - 1;

/// The Speed menu, in saved order.
pub const SPEEDS: &[&str] = &["½×", "1×", "2×"];

/// Half frames the playhead moves per sample at each Speed.
const SPEED_STEPS: [u64; 3] = [1, 2, 4];

/// The Bars menu, in saved order.
pub const BARS: &[&str] = &["Free", "1", "2", "4", "8", "16"];

/// Bars a clocked take lasts at each Bars setting (0 for Free).
const BAR_COUNTS: [u64; 6] = [0, 1, 2, 4, 8, 16];

/// Where the Looper is in its cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopState {
    /// Nothing recorded.
    Empty,
    /// Rec was tapped while clocked: recording starts on the next pulse.
    Armed,
    /// The first take, before its loop closes.
    Recording,
    Playing,
    Overdubbing,
    /// Holding a loop, silent.
    Stopped,
}

impl LoopState {
    const ALL: [LoopState; 6] = [
        LoopState::Empty,
        LoopState::Armed,
        LoopState::Recording,
        LoopState::Playing,
        LoopState::Overdubbing,
        LoopState::Stopped,
    ];

    /// The state as a readout value.
    pub fn code(self) -> f32 {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0) as f32
    }

    /// The state a readout value stands for.
    pub fn from_code(code: f32) -> Self {
        Self::ALL.get(code.round().max(0.0) as usize).copied().unwrap_or(LoopState::Empty)
    }
}

/// A bit per frame.
struct Bits {
    words: Vec<u64>,
}

impl Bits {
    fn with_len(len: usize) -> Self {
        Self { words: vec![0; len.div_ceil(64)] }
    }

    #[inline]
    fn get(&self, i: usize) -> bool {
        (self.words[i >> 6] >> (i & 63)) & 1 != 0
    }

    #[inline]
    fn set(&mut self, i: usize) {
        self.words[i >> 6] |= 1 << (i & 63);
    }

    /// Clears the bits of the first `len` frames.
    fn clear(&mut self, len: usize) {
        let words = len.div_ceil(64).min(self.words.len());
        self.words[..words].fill(0);
    }

    /// Flips the bits of the first `len` frames that are set in `other`.
    fn toggle(&mut self, other: &Bits, len: usize) {
        let words = len.div_ceil(64).min(self.words.len());
        for (word, flips) in self.words[..words].iter_mut().zip(&other.words) {
            *word ^= flips;
        }
    }
}

/// The loop's memory: two banks, and which one holds each frame.
struct Tape {
    banks: [Vec<[f32; 2]>; 2],
    /// Which bank holds each frame as it was before the last layer.
    base: Bits,
    /// The frames the last layer wrote, into the bank `base` doesn't name.
    layer: Bits,
    /// Whether the last layer is heard (false once undone).
    layer_visible: bool,
    /// Whether the last layer wrote anything, so there's something to undo.
    has_layer: bool,
    /// Frames that may hold set bits: everything past is known clear.
    used: usize,
}

impl Tape {
    fn empty() -> Self {
        Self {
            banks: [Vec::new(), Vec::new()],
            base: Bits::with_len(0),
            layer: Bits::with_len(0),
            layer_visible: true,
            has_layer: false,
            used: 0,
        }
    }

    /// A tape `frames` long, silent. Off the audio thread: this allocates.
    fn with_capacity(frames: usize) -> Self {
        Self {
            banks: [vec![[0.0; 2]; frames], vec![[0.0; 2]; frames]],
            base: Bits::with_len(frames),
            layer: Bits::with_len(frames),
            layer_visible: true,
            has_layer: false,
            used: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.banks[0].len()
    }

    /// Frame `i`, with the last layer heard or not.
    #[inline]
    fn read(&self, i: usize, layer_visible: bool) -> [f32; 2] {
        let bank = self.base.get(i) ^ (layer_visible && self.layer.get(i));
        self.banks[bank as usize][i]
    }

    /// Writes frame `i` of a first take. The bits are clear then, so it
    /// goes to the first bank.
    #[inline]
    fn record(&mut self, i: usize, frame: [f32; 2]) {
        self.banks[0][i] = frame;
        self.used = self.used.max(i + 1);
    }

    /// Fades frame `i` of a first take toward `frame` by `amount` (0 keeps
    /// it, 1 replaces it).
    #[inline]
    fn blend(&mut self, i: usize, frame: [f32; 2], amount: f32) {
        let cell = &mut self.banks[0][i];
        for side in 0..2 {
            cell[side] = frame[side] * amount + cell[side] * (1.0 - amount);
        }
    }

    /// Overdubs frame `i` for the first time this pass: what's there, scaled
    /// by `keep`, plus `add`, into the layer.
    #[inline]
    fn overdub(&mut self, i: usize, keep: f32, add: [f32; 2]) {
        let old = self.read(i, true);
        let bank = !self.base.get(i) as usize;
        self.banks[bank][i] = [old[0] * keep + add[0], old[1] * keep + add[1]];
        self.layer.set(i);
        self.has_layer = true;
    }

    /// Adds to a frame this pass has already overdubbed.
    #[inline]
    fn overdub_more(&mut self, i: usize, add: [f32; 2]) {
        let bank = !self.base.get(i) as usize;
        let cell = &mut self.banks[bank][i];
        cell[0] += add[0];
        cell[1] += add[1];
    }

    /// Starts a new layer: the last one, if heard, becomes part of the
    /// loop, and can no longer be undone.
    fn begin_layer(&mut self) {
        if self.layer_visible && self.has_layer {
            self.base.toggle(&self.layer, self.used);
        }
        self.layer.clear(self.used);
        self.layer_visible = true;
        self.has_layer = false;
    }

    /// Forgets everything recorded.
    fn clear(&mut self) {
        self.base.clear(self.used);
        self.layer.clear(self.used);
        self.layer_visible = true;
        self.has_layer = false;
        self.used = 0;
    }
}

/// Records, overdubs and plays back live layers, like a looper pedal.
///
/// # Inputs
/// - **In L**, **In R** (Audio): what to loop. Mono in plays on both sides.
/// - **Rec** (Gate): the footswitch. Empty → Record → Play (the tap that
///   closes the loop sets its length) → Overdub → Play → Overdub…
/// - **Stop** (Gate): stops playback; again restarts the loop from the top.
/// - **Undo** (Gate): takes back the last overdub layer, or brings it back.
/// - **Clear** (Gate): empties the loop.
/// - **Clock** (Gate): when patched, recording starts on a pulse and its
///   loop closes on a whole bar.
///
/// # Outputs
/// - **Out L**, **Out R** (Audio): the input and the loop.
/// - **Loop L**, **Loop R** (Audio): the loop alone.
/// - **Start** (Gate): a 1 ms pulse each time the loop comes round.
/// - **Phase** (Control): how far round the loop is, 0 to 1.
///
/// # Parameters
/// - **Speed** (½×, 1×, 2×) and **Reverse**: how it plays, not what's kept.
/// - **Bars** (Free, 1–16): with a Clock, how many bars a take lasts.
/// - **Latency** (Off / Auto): take the round trip measured for live input
///   off overdubs fed by an Audio Input.
/// - **Feedback** (0–1): how much of the loop each overdub pass keeps.
/// - **Loop Level**, **Dry Level** (0–1).
/// - **Offset** (±50 ms): moves overdubs earlier (+) or later (−), on top
///   of Latency.
/// - **Pedal Rec**, **Pedal Stop**, **Pedal Undo**, **Pedal Clear**: the
///   footswitches on the node, which a MIDI controller can press too.
pub struct Looper {
    tape: Tape,
    sample_rate: f32,
    /// The longest loop, in frames.
    max_frames: usize,
    /// Crossfade length, in frames.
    fade_frames: usize,
    state: LoopState,
    /// The loop's length in frames: 0 while empty or still recording.
    len: usize,
    /// The playhead, in half frames.
    head: u64,

    // The take
    /// Frames since the take began (or, while empty and clocked, since the
    /// last pulse: a take a late tap claims has been recording since then).
    elapsed: u64,
    /// Frames each write lands back, fixed for the take.
    take_offset: i64,
    /// Whether frames are being captured from the last pulse, in case a
    /// late tap claims them.
    speculating: bool,
    /// When the take closes itself (a frame count of `elapsed`).
    close_at: Option<u64>,
    /// Whether to stop rather than play once the take closes.
    stop_on_close: bool,
    /// After the loop closes, the take still writes its last frames and
    /// the pre-roll that smooths the seam.
    finishing: bool,
    /// A Rec tap that came while the take was finishing, waiting for it.
    pending_rec: bool,
    /// Frames a full turn of the ring stands for while the take records.
    scale: u64,

    // Overdubs
    /// How much of the input an overdub writes: ramps in and out.
    dub_gain: f32,
    /// Frames overdubs land back, fixed for the layer.
    dub_offset: i64,
    /// The frame overdubbed last, so a pass scales each frame by Feedback once.
    last_frame: Option<usize>,
    /// The overview segment overdubbed last, to count passes over each.
    last_segment: Option<usize>,

    // Output
    /// How loud the loop plays: ramps in and out.
    play_gain: f32,
    /// What's left of the crossfade after an undo or redo, 1 to 0.
    undo_fade: f32,
    /// A Clear waiting for the loop to fade out.
    clear_pending: bool,
    /// Samples left of the Start pulse.
    start_pulse: usize,
    /// The loop came round: Start pulses from the next sample, where the
    /// top of the loop plays.
    start_due: bool,

    // Gates
    prev_gates: [bool; 4],
    prev_pedals: Option<[bool; 4]>,
    prev_clock: bool,
    /// Frames since the clock last rose.
    since_pulse: Option<u64>,
    /// The clock's last period, in frames.
    pulse_period: Option<u64>,
    /// A bar of the clock, in frames, as of the last block, while clocked.
    bar: Option<f64>,

    // The display
    /// The loudest frame in each overview segment.
    peaks: [f32; OVERVIEW_SEGMENTS],
    /// Overdub passes over each segment before the last layer.
    rings_base: [u16; OVERVIEW_SEGMENTS],
    /// Overdub passes over each segment in the last layer.
    rings_layer: [u16; OVERVIEW_SEGMENTS],
    /// Layers folded into the loop for good.
    layers: u32,
    /// Segments whose peaks need reading again.
    dirty: [u64; OVERVIEW_SEGMENTS / 64],
    /// Where the rescan picks up.
    scan_from: usize,
    /// The overview as sent: peaks, then passes.
    overview: [[f32; OVERVIEW_SEGMENTS]; 2],
    /// Samples until the overview is sent again.
    overview_due: i64,
    overview_ready: bool,

    // Keeping the loop
    /// Changes to the loop as heard since it was created or last loaded
    /// (0): a take, a close, an overdub begun or ended, an undo, a clear.
    edits: u32,
    /// A loop being copied in, a piece at a time.
    loading: Option<Arc<SampleData>>,
    /// Frames of it copied so far.
    loaded: usize,
    /// A loop finished loading, to go back and be dropped off the audio thread.
    retired: Option<Arc<SampleData>>,
    /// A loaded loop not yet trimmed to whole bars of the clock.
    snap_pending: bool,

    loop_level: SmoothedValue,
    dry_level: SmoothedValue,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Looper {
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            tape: Tape::empty(),
            sample_rate: 0.0,
            max_frames: 0,
            fade_frames: 1,
            state: LoopState::Empty,
            len: 0,
            head: 0,
            elapsed: 0,
            take_offset: 0,
            speculating: false,
            close_at: None,
            stop_on_close: false,
            finishing: false,
            pending_rec: false,
            scale: 1,
            dub_gain: 0.0,
            dub_offset: 0,
            last_frame: None,
            last_segment: None,
            play_gain: 0.0,
            undo_fade: 0.0,
            clear_pending: false,
            start_pulse: 0,
            start_due: false,
            prev_gates: [false; 4],
            prev_pedals: None,
            prev_clock: false,
            since_pulse: None,
            pulse_period: None,
            bar: None,
            peaks: [0.0; OVERVIEW_SEGMENTS],
            rings_base: [0; OVERVIEW_SEGMENTS],
            rings_layer: [0; OVERVIEW_SEGMENTS],
            layers: 0,
            dirty: [0; OVERVIEW_SEGMENTS / 64],
            scan_from: 0,
            overview: [[0.0; OVERVIEW_SEGMENTS]; 2],
            overview_due: 0,
            overview_ready: false,
            edits: 0,
            loading: None,
            loaded: 0,
            retired: None,
            snap_pending: false,
            loop_level: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            dry_level: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            ports: vec![
                PortDefinition::input("in_l", "In L", SignalType::Audio).describe("What to loop. Mono in plays on both sides"),
                PortDefinition::input("in_r", "In R", SignalType::Audio).describe("Right input"),
                PortDefinition::input_with_default("rec", "Rec", SignalType::Gate, 0.0)
                    .describe("The footswitch: record, then close the loop, then overdub and play in turn"),
                PortDefinition::input_with_default("stop", "Stop", SignalType::Gate, 0.0)
                    .describe("Stops playback; again restarts the loop from the top"),
                PortDefinition::input_with_default("undo", "Undo", SignalType::Gate, 0.0)
                    .describe("Takes back the last overdub layer, or brings it back"),
                PortDefinition::input_with_default("clear", "Clear", SignalType::Gate, 0.0).describe("Empties the loop"),
                PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0)
                    .describe("Patched, recording starts on a pulse and the loop closes on a whole bar"),
                PortDefinition::output("out_l", "Out L", SignalType::Audio).describe("The input and the loop, left"),
                PortDefinition::output("out_r", "Out R", SignalType::Audio).describe("The input and the loop, right"),
                PortDefinition::output("loop_l", "Loop L", SignalType::Audio).describe("The loop alone, left, to send through its own effects"),
                PortDefinition::output("loop_r", "Loop R", SignalType::Audio).describe("The loop alone, right"),
                PortDefinition::output("start", "Start", SignalType::Gate).describe("A pulse each time the loop comes round"),
                PortDefinition::output("phase", "Phase", SignalType::Control).describe("How far round the loop is, 0 to 1"),
            ],
            parameters: vec![
                ParameterDefinition::choice("speed", "Speed", SPEEDS, 1)
                    .describe("Playback speed, as on tape: ½× is an octave down. The loop itself is kept"),
                ParameterDefinition::new("reverse", "Reverse", 0.0, 1.0, 0.0, ParameterDisplay::toggle("Fwd", "Rev"))
                    .describe("Plays the loop backwards"),
                ParameterDefinition::choice("bars", "Bars", BARS, 0)
                    .describe("With a Clock patched, how many bars a take lasts; Free closes on the nearest bar to your tap"),
                ParameterDefinition::new("latency", "Latency", 0.0, 1.0, 1.0, ParameterDisplay::toggle("Off", "Auto"))
                    .describe("Auto takes the measured round trip off overdubs fed by an Audio Input, so layers land where they were played"),
                ParameterDefinition::new("feedback", "Feedback", 0.0, 1.0, 1.0, ParameterDisplay::linear("%"))
                    .describe("How much of the loop each overdub pass keeps: below 100% old layers fade away"),
                ParameterDefinition::new("loop_level", "Loop Level", 0.0, 1.0, 1.0, ParameterDisplay::linear(""))
                    .describe("How loud the loop plays"),
                ParameterDefinition::new("dry_level", "Dry Level", 0.0, 1.0, 1.0, ParameterDisplay::linear(""))
                    .describe("How loud the input passes through"),
                ParameterDefinition::new("offset", "Offset", -MAX_OFFSET_MS, MAX_OFFSET_MS, 0.0, ParameterDisplay::linear("ms"))
                    .describe("Moves recording earlier (+) or later (−), on top of Latency: trim by ear"),
                ParameterDefinition::new("pedal_rec", "Pedal Rec", 0.0, 1.0, 0.0, ParameterDisplay::on_off())
                    .describe("The Rec footswitch on the node"),
                ParameterDefinition::new("pedal_stop", "Pedal Stop", 0.0, 1.0, 0.0, ParameterDisplay::on_off())
                    .describe("The Stop footswitch on the node"),
                ParameterDefinition::new("pedal_undo", "Pedal Undo", 0.0, 1.0, 0.0, ParameterDisplay::on_off())
                    .describe("The Undo footswitch on the node"),
                ParameterDefinition::new("pedal_clear", "Pedal Clear", 0.0, 1.0, 0.0, ParameterDisplay::on_off())
                    .describe("The Clear footswitch on the node"),
            ],
        }
    }

    const PORT_IN_L: usize = 0;
    const PORT_IN_R: usize = 1;
    const PORT_REC: usize = 2;
    const PORT_CLOCK: usize = 6;

    const OUT_L: usize = 0;
    const OUT_R: usize = 1;
    const OUT_LOOP_L: usize = 2;
    const OUT_LOOP_R: usize = 3;
    const OUT_START: usize = 4;
    const OUT_PHASE: usize = 5;

    pub const PARAM_SPEED: usize = 0;
    pub const PARAM_REVERSE: usize = 1;
    pub const PARAM_BARS: usize = 2;
    pub const PARAM_LATENCY: usize = 3;
    pub const PARAM_FEEDBACK: usize = 4;
    pub const PARAM_LOOP_LEVEL: usize = 5;
    pub const PARAM_DRY_LEVEL: usize = 6;
    pub const PARAM_OFFSET: usize = 7;
    /// The footswitches, in the order Rec, Stop, Undo, Clear.
    pub const PARAM_PEDALS: usize = 8;

    /// The footswitches' names, in order: the gate inputs share their
    /// order, and the parameters are these with "Pedal " in front.
    pub const PEDALS: [&'static str; 4] = ["Rec", "Stop", "Undo", "Clear"];

    /// Readout slots.
    pub const READOUT_STATE: usize = 0;
    /// The loop's length in seconds, or the take's so far.
    pub const READOUT_SECONDS: usize = 1;
    pub const READOUT_PHASE: usize = 2;
    /// The loop's length in bars when clocked, or -1.
    pub const READOUT_BARS: usize = 3;
    /// Layers overdubbed and heard.
    pub const READOUT_LAYERS: usize = 4;
    /// 1 if Undo would take a layer back, 2 if it would bring one back, else 0.
    pub const READOUT_UNDO: usize = 5;
    /// While recording, the seconds a full turn of the ring stands for.
    pub const READOUT_SCALE: usize = 6;
    /// How far writes land back, in milliseconds.
    pub const READOUT_OFFSET_MS: usize = 7;
    /// Changes to the loop since the module was made or last given a loop
    /// to load: 0 means it holds just what it was given.
    pub const READOUT_EDITS: usize = 8;

    const GATE_THRESHOLD: f32 = 0.5;

    /// Where the Looper is in its cycle.
    pub fn state(&self) -> LoopState {
        self.state
    }

    /// The loop's length in frames, 0 until a take closes.
    pub fn loop_frames(&self) -> usize {
        self.len
    }

    /// Frame `i` of the loop as it's heard.
    pub fn loop_frame(&self, i: usize) -> [f32; 2] {
        self.tape.read(i, self.tape.layer_visible)
    }

    /// Changes to the loop since the module was made or last loaded.
    pub fn edits(&self) -> u32 {
        self.edits
    }

    /// Whether a loop it was given is still being copied in.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Notes a change to the loop as heard.
    fn bump(&mut self) {
        self.edits = self.edits % EDITS_WRAP + 1;
    }

    fn sample_frames(&self, seconds: f32) -> usize {
        (seconds * self.sample_rate).round().max(1.0) as usize
    }

    /// A bar of the patch's clock, in frames: from the transport's tempo
    /// and time signature, or four pulses of the Clock input without one.
    fn bar_frames(&self, context: &ProcessContext) -> Option<f64> {
        let transport = &context.transport;
        match transport.tempo_bpm.filter(|&bpm| bpm > 0.0) {
            Some(bpm) => Some(context.sample_rate as f64 * 60.0 / bpm as f64 * transport.time_sig_numerator.max(1) as f64),
            None => self.pulse_period.map(|period| 4.0 * period as f64),
        }
    }

    /// How far writes land back, in frames: the round trip with Latency on,
    /// plus Offset.
    fn write_offset(&self, params: &[f32], context: &ProcessContext) -> i64 {
        let latency = if params[Self::PARAM_LATENCY] >= 0.5 { context.input_latency as f32 } else { 0.0 };
        let offset = params[Self::PARAM_OFFSET].clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS) * 0.001 * self.sample_rate;
        (latency + offset).round() as i64
    }

    // ------------------------------------------------------------------
    // The cycle
    // ------------------------------------------------------------------

    /// Starts a take now.
    fn start_take(&mut self, params: &[f32], context: &ProcessContext, clocked: bool) {
        self.finish_clear();
        self.cancel_load();
        self.bump();
        self.state = LoopState::Recording;
        self.speculating = false;
        self.elapsed = 0;
        self.take_offset = self.write_offset(params, context);
        // Writing later than played skips the take's first frames: they
        // must be silent, not what an old take left there
        let skipped = (-self.take_offset).max(0) as usize;
        for i in 0..skipped.min(self.tape.capacity()) {
            self.tape.record(i, [0.0; 2]);
        }
        self.close_at = None;
        self.stop_on_close = false;
        self.scale = self.sample_frames(FREE_SCALE_SECONDS) as u64;
        if let Some(bar) = self.bar_frames(context).filter(|_| clocked) {
            let bars = BAR_COUNTS[(params[Self::PARAM_BARS].round().max(0.0) as usize).min(BAR_COUNTS.len() - 1)];
            if bars > 0 {
                let length = (bars as f64 * bar).round() as u64;
                self.close_at = Some(length.clamp(1, self.max_frames as u64));
                self.scale = length.max(1);
            } else {
                self.scale = (4.0 * bar).round().max(1.0) as u64;
            }
        }
        self.mark_all_dirty();
    }

    /// The Rec footswitch.
    fn tap_rec(&mut self, params: &[f32], context: &ProcessContext, clocked: bool) {
        match self.state {
            LoopState::Empty => {
                if !clocked {
                    self.start_take(params, context, clocked);
                } else if self.speculating && self.elapsed <= self.sample_frames(LATE_TAP_SECONDS) as u64 {
                    // A late foot: the take began on the pulse just gone,
                    // and has been recording since
                    let since = self.elapsed;
                    let offset = self.take_offset;
                    self.start_take(params, context, clocked);
                    self.elapsed = since;
                    self.take_offset = offset;
                } else {
                    self.state = LoopState::Armed;
                }
            }
            LoopState::Armed => self.state = LoopState::Empty,
            LoopState::Recording => self.request_close(context, clocked),
            LoopState::Playing if self.finishing => self.pending_rec = true,
            LoopState::Playing => self.begin_overdub(params, context),
            LoopState::Overdubbing => {
                self.state = LoopState::Playing;
                self.bump();
            }
            LoopState::Stopped => self.restart(),
        }
    }

    /// Ends the take: on a tap in free time, or on the nearest whole bar
    /// when clocked (late or early), at least long enough to crossfade.
    fn request_close(&mut self, context: &ProcessContext, clocked: bool) {
        let elapsed = self.elapsed;
        let shortest = 2 * self.fade_frames as u64;
        let length = match self.bar_frames(context).filter(|_| clocked) {
            Some(bar) => {
                let bars = (elapsed as f64 / bar).round().max(1.0);
                (bars * bar).round() as u64
            }
            None => elapsed,
        };
        let length = length.clamp(shortest, self.max_frames as u64);
        if length <= elapsed {
            self.close(length as usize, elapsed - length);
        } else {
            self.close_at = Some(length);
        }
    }

    /// Closes the loop at `length` frames, with the playhead `into` frames
    /// round it.
    fn close(&mut self, length: usize, into: u64) {
        self.len = length.max(1);
        // A tap that closed the loop on a bar gone by has already recorded
        // some of the pre-roll: fade it over the loop's first frames now,
        // and the rest as it comes
        let len = self.len;
        for k in 0..self.fade_frames.min(self.tape.used.saturating_sub(len)) {
            let frame = self.tape.banks[0][len + k];
            self.tape.blend(k, frame, seam_weight(k as f32, self.fade_frames as f32));
        }
        self.head = (2 * into) % (2 * self.len as u64);
        self.close_at = None;
        self.finishing = true;
        self.state = if self.stop_on_close { LoopState::Stopped } else { LoopState::Playing };
        self.stop_on_close = false;
        if self.head == 0 && self.state == LoopState::Playing {
            self.start_pulse = self.sample_frames(START_PULSE_SECONDS);
        }
        self.bump();
        self.mark_all_dirty();
    }

    fn begin_overdub(&mut self, params: &[f32], context: &ProcessContext) {
        self.commit_layer();
        self.tape.begin_layer();
        self.dub_offset = self.write_offset(params, context);
        self.last_frame = None;
        self.last_segment = None;
        self.state = LoopState::Overdubbing;
        self.bump();
    }

    /// Folds the last layer's passes into the display's rings, once it can
    /// no longer be undone.
    fn commit_layer(&mut self) {
        if self.tape.has_layer && self.tape.layer_visible {
            for (base, layer) in self.rings_base.iter_mut().zip(&self.rings_layer) {
                *base = base.saturating_add(*layer);
            }
            self.layers += 1;
        }
        self.rings_layer = [0; OVERVIEW_SEGMENTS];
    }

    /// Plays from the top.
    fn restart(&mut self) {
        if self.len > 0 {
            self.state = LoopState::Playing;
            self.head = 0;
            self.start_pulse = self.sample_frames(START_PULSE_SECONDS);
        }
    }

    fn tap_stop(&mut self, context: &ProcessContext, clocked: bool) {
        match self.state {
            LoopState::Empty => {}
            LoopState::Armed => self.state = LoopState::Empty,
            LoopState::Recording => {
                self.stop_on_close = true;
                self.request_close(context, clocked);
            }
            LoopState::Playing => self.state = LoopState::Stopped,
            LoopState::Overdubbing => {
                self.state = LoopState::Stopped;
                self.bump();
            }
            LoopState::Stopped => self.restart(),
        }
    }

    fn tap_undo(&mut self) {
        if self.state == LoopState::Overdubbing {
            // The layer goes as it ends: no punch-out to write
            self.state = LoopState::Playing;
            self.dub_gain = 0.0;
            self.last_frame = None;
            self.last_segment = None;
            self.bump();
        }
        if self.len == 0 || !self.tape.has_layer {
            return;
        }
        self.bump();
        self.tape.layer_visible = !self.tape.layer_visible;
        self.undo_fade = 1.0;
        for s in 0..OVERVIEW_SEGMENTS {
            if self.rings_layer[s] > 0 {
                self.dirty[s / 64] |= 1 << (s % 64);
            }
        }
    }

    fn tap_clear(&mut self) {
        self.cancel_load();
        match self.state {
            LoopState::Empty => {}
            LoopState::Armed | LoopState::Recording => self.clear_now(),
            _ => {
                // Fade the loop out, then forget it
                self.bump();
                self.state = LoopState::Empty;
                self.clear_pending = true;
                self.dub_gain = 0.0;
            }
        }
    }

    /// Finishes a Clear still fading out.
    fn finish_clear(&mut self) {
        if self.clear_pending {
            self.clear_now();
        }
    }

    fn clear_now(&mut self) {
        if (self.len > 0 && !self.clear_pending) || self.state == LoopState::Recording {
            self.bump();
        }
        self.tape.clear();
        self.state = LoopState::Empty;
        self.len = 0;
        self.head = 0;
        self.elapsed = 0;
        self.speculating = false;
        self.close_at = None;
        self.stop_on_close = false;
        self.finishing = false;
        self.pending_rec = false;
        self.dub_gain = 0.0;
        self.last_frame = None;
        self.last_segment = None;
        self.play_gain = 0.0;
        self.undo_fade = 0.0;
        self.clear_pending = false;
        self.start_due = false;
        self.peaks = [0.0; OVERVIEW_SEGMENTS];
        self.rings_base = [0; OVERVIEW_SEGMENTS];
        self.rings_layer = [0; OVERVIEW_SEGMENTS];
        self.layers = 0;
        self.dirty = [0; OVERVIEW_SEGMENTS / 64];
        self.snap_pending = false;
    }

    // ------------------------------------------------------------------
    // Loading
    // ------------------------------------------------------------------

    /// Drops a loop still being copied in: something was recorded instead.
    fn cancel_load(&mut self) {
        if let Some(sample) = self.loading.take() {
            self.retired = Some(sample);
            self.loaded = 0;
        }
    }

    /// The loop being loaded is all copied in: it waits, Stopped at the top,
    /// as it was given.
    fn finish_load(&mut self) {
        self.retired = self.loading.take();
        self.len = self.loaded.max(1);
        self.tape.used = self.len;
        self.loaded = 0;
        self.state = LoopState::Stopped;
        self.head = 0;
        self.edits = 0;
        self.snap_pending = true;
        self.mark_all_dirty();
    }

    /// Trims or pads a loaded loop to the nearest whole number of bars of
    /// `bar` frames, if it's within a few milliseconds of it: a file saved
    /// at another rate comes back a frame or two off when resampled.
    fn snap_to_bar(&mut self, bar: f64) {
        let len = self.len;
        let bars = (len as f64 / bar).round().max(1.0);
        let target = ((bars * bar).round() as usize).clamp(1, self.max_frames);
        if target == len || target.abs_diff(len) > self.sample_frames(SNAP_SECONDS) {
            return;
        }
        // Frames added hold the last one, which a wrap would play next anyway
        let last = self.tape.read(len - 1, false);
        for i in len..target {
            self.tape.record(i, last);
        }
        self.len = target;
        self.tape.used = self.tape.used.max(target);
        self.mark_all_dirty();
    }

    // ------------------------------------------------------------------
    // The overview
    // ------------------------------------------------------------------

    /// Frames the ring's full turn stands for.
    fn ring_frames(&self) -> u64 {
        if self.len > 0 {
            self.len as u64
        } else {
            self.scale.max(1)
        }
    }

    #[inline]
    fn segment_of(&self, frame: usize) -> usize {
        ((frame as u64 * OVERVIEW_SEGMENTS as u64 / self.ring_frames()) as usize).min(OVERVIEW_SEGMENTS - 1)
    }

    #[inline]
    fn mark_dirty(&mut self, frame: usize) {
        let s = self.segment_of(frame);
        self.dirty[s / 64] |= 1 << (s % 64);
    }

    fn mark_all_dirty(&mut self) {
        self.dirty = [u64::MAX; OVERVIEW_SEGMENTS / 64];
    }

    /// Reads the loudest frame of dirty segments again, up to a budget.
    fn rescan(&mut self) {
        let frames = self.ring_frames();
        // While recording, only what's been written counts
        let written = if self.len > 0 { self.len } else { self.tape.used };
        let mut budget = SCAN_BUDGET;
        for step in 0..OVERVIEW_SEGMENTS {
            let s = (self.scan_from + step) % OVERVIEW_SEGMENTS;
            if self.dirty[s / 64] & (1 << (s % 64)) == 0 {
                continue;
            }
            let start = (s as u64 * frames / OVERVIEW_SEGMENTS as u64) as usize;
            let end = (((s + 1) as u64 * frames / OVERVIEW_SEGMENTS as u64) as usize).min(written);
            let mut peak = 0.0_f32;
            for i in start..end.min(self.tape.capacity()) {
                let [l, r] = self.loop_frame(i);
                peak = peak.max(l.abs()).max(r.abs());
            }
            self.peaks[s] = peak;
            self.dirty[s / 64] &= !(1 << (s % 64));
            budget = budget.saturating_sub(end.saturating_sub(start).max(1));
            if budget == 0 {
                self.scan_from = (s + 1) % OVERVIEW_SEGMENTS;
                return;
            }
        }
    }

    /// Fills the overview to send.
    fn publish_overview(&mut self) {
        let visible = self.tape.layer_visible;
        for s in 0..OVERVIEW_SEGMENTS {
            self.overview[0][s] = self.peaks[s];
            let layer = if visible { self.rings_layer[s] } else { 0 };
            self.overview[1][s] = self.rings_base[s].saturating_add(layer) as f32;
        }
        self.overview_ready = true;
    }

    // ------------------------------------------------------------------
    // Reading
    // ------------------------------------------------------------------

    /// The loop at half frame `half`, with the last layer heard or not.
    #[inline]
    fn frame_at(&self, half: u64, layer_visible: bool) -> [f32; 2] {
        let len = self.len;
        let i = (half / 2) as usize % len;
        if half.is_multiple_of(2) {
            return self.tape.read(i, layer_visible);
        }
        // Halfway between two frames
        let at = |n: usize| self.tape.read(n % len, layer_visible);
        let (x0, x1, x2, x3) = (at(i + len - 1), at(i), at(i + 1), at(i + 2));
        [hermite(x0[0], x1[0], x2[0], x3[0], 0.5), hermite(x0[1], x1[1], x2[1], x3[1], 0.5)]
    }

    /// What the loop plays at the playhead, crossfading after an undo.
    #[inline]
    fn heard(&self) -> [f32; 2] {
        let visible = self.tape.layer_visible;
        let now = self.frame_at(self.head, visible);
        if self.undo_fade <= 0.0 {
            return now;
        }
        let before = self.frame_at(self.head, !visible);
        let f = self.undo_fade;
        [now[0] * (1.0 - f) + before[0] * f, now[1] * (1.0 - f) + before[1] * f]
    }
}

impl Default for Looper {
    fn default() -> Self {
        Self::new()
    }
}

/// A ramp's next value toward `target`, `step` at a time.
#[inline]
fn ramp(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

impl DspModule for Looper {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: LOOPER_ID,
            name: "Looper",
            category: ModuleCategory::Utility,
            description: "Record, overdub and undo live layers, like a looper pedal, in time with the Clock",
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
            self.sample_rate = sample_rate;
            self.max_frames = (MAX_LOOP_SECONDS * sample_rate).round() as usize;
            self.fade_frames = self.sample_frames(FADE_SECONDS);
            // Room past the longest loop for the pre-roll the seam fades into
            self.tape = Tape::with_capacity(self.max_frames + self.fade_frames + 1);
            // A loop on its way in is at the old rate: it's sent again at the new one
            self.cancel_load();
            self.clear_now();
            self.since_pulse = None;
            self.pulse_period = None;
        }
        self.loop_level.set_sample_rate(sample_rate);
        self.dry_level.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        if self.tape.capacity() == 0 {
            // Never prepared: nothing to record into
            return;
        }
        let at = |port: usize, i: usize| inputs.get(port).map_or(0.0, |buf| buf.samples.get(i).copied().unwrap_or(0.0));
        let left = connected_input(inputs, Self::PORT_IN_L);
        let right = connected_input(inputs, Self::PORT_IN_R);
        // One side patched plays on both
        let (left, right) = match (left, right) {
            (Some(l), None) => (Some(l), Some(l)),
            (None, Some(r)) => (Some(r), Some(r)),
            sides => sides,
        };
        let clocked = connected_input(inputs, Self::PORT_CLOCK).is_some();
        self.bar = if clocked { self.bar_frames(context) } else { None };
        if self.snap_pending {
            if self.state != LoopState::Stopped {
                self.snap_pending = false;
            } else if let Some(bar) = self.bar.filter(|&bar| bar >= 1.0) {
                self.snap_to_bar(bar);
                self.snap_pending = false;
            }
        }

        let step = SPEED_STEPS[(params[Self::PARAM_SPEED].round().max(0.0) as usize).min(SPEED_STEPS.len() - 1)];
        let forward = params[Self::PARAM_REVERSE] < 0.5;
        let feedback = params[Self::PARAM_FEEDBACK].clamp(0.0, 1.0);
        self.loop_level.set_target(params[Self::PARAM_LOOP_LEVEL].clamp(0.0, 1.0));
        self.dry_level.set_target(params[Self::PARAM_DRY_LEVEL].clamp(0.0, 1.0));
        let fade_step = 1.0 / self.fade_frames as f32;
        let late_window = self.sample_frames(LATE_TAP_SECONDS) as u64;

        // The footswitches on the node press on the block's first sample
        let mut pedals = [false; 4];
        for (n, pedal) in pedals.iter_mut().enumerate() {
            *pedal = params.get(Self::PARAM_PEDALS + n).is_some_and(|&v| v >= 0.5);
        }
        let prev_pedals = self.prev_pedals.unwrap_or(pedals);
        self.prev_pedals = Some(pedals);

        for i in 0..context.block_size {
            let input = [
                left.map_or(0.0, |buf| buf.samples[i]),
                right.map_or(0.0, |buf| buf.samples[i]),
            ];

            // Taps: a rising gate, or a footswitch pressed
            let mut taps = [false; 4];
            for (n, tap) in taps.iter_mut().enumerate() {
                let gate = at(Self::PORT_REC + n, i) > Self::GATE_THRESHOLD;
                *tap = gate && !self.prev_gates[n];
                self.prev_gates[n] = gate;
                if i == 0 && pedals[n] && !prev_pedals[n] {
                    *tap = true;
                }
            }

            // The clock
            let clock = at(Self::PORT_CLOCK, i) > Self::GATE_THRESHOLD;
            let pulse = clock && !self.prev_clock;
            self.prev_clock = clock;
            if pulse {
                if let Some(since) = self.since_pulse {
                    self.pulse_period = Some(since.max(1));
                }
                self.since_pulse = Some(0);
            }

            if pulse && clocked {
                match self.state {
                    LoopState::Armed => self.start_take(params, context, clocked),
                    LoopState::Empty if !self.clear_pending && self.loading.is_none() => {
                        // Capture from this pulse, in case a late tap wants it
                        self.speculating = true;
                        self.elapsed = 0;
                        self.take_offset = self.write_offset(params, context);
                        self.tape.clear();
                    }
                    _ => {}
                }
            }

            let [rec, stop, undo, clear] = taps;
            if clear {
                self.tap_clear();
            }
            if rec {
                self.tap_rec(params, context, clocked);
            }
            if stop {
                self.tap_stop(context, clocked);
            }
            if undo {
                self.tap_undo();
            }

            // The take: written where the player played it
            let capturing = self.state == LoopState::Recording || self.finishing || (self.state == LoopState::Empty && self.speculating);
            if capturing {
                let frame = self.elapsed as i64 - self.take_offset;
                if frame >= 0 {
                    let frame = frame as usize;
                    if self.len == 0 {
                        // Still recording: grow the ring as the take outgrows it
                        if frame < self.tape.capacity() {
                            self.tape.record(frame, input);
                            while frame as u64 >= self.scale && self.scale < self.max_frames as u64 {
                                self.scale *= 2;
                                self.mark_all_dirty();
                            }
                            self.mark_dirty(frame);
                        }
                    } else if frame < self.len {
                        self.tape.record(frame, input);
                        self.mark_dirty(frame);
                    } else if frame < self.len + self.fade_frames {
                        // The pre-roll: what came after the loop closed fades
                        // over its first frames, so the wrap runs on into it
                        let k = frame - self.len;
                        self.tape.blend(k, input, seam_weight(k as f32, self.fade_frames as f32));
                        self.mark_dirty(k);
                    } else {
                        self.finishing = false;
                        // The seam is written: the loop has changed since it closed
                        self.bump();
                        if self.pending_rec {
                            self.pending_rec = false;
                            self.tap_rec(params, context, clocked);
                        }
                    }
                }
                self.elapsed += 1;
                if self.state == LoopState::Empty && self.elapsed > late_window + self.take_offset.max(0) as u64 {
                    self.speculating = false;
                }
                if self.state == LoopState::Recording {
                    if self.close_at == Some(self.elapsed) {
                        let length = self.elapsed as usize;
                        self.close(length, 0);
                    } else if self.elapsed as usize >= self.max_frames {
                        self.close(self.max_frames, 0);
                    }
                }
            }

            // The loop
            if self.start_due {
                self.start_due = false;
                self.start_pulse = self.sample_frames(START_PULSE_SECONDS);
            }
            let mut heard = [0.0; 2];
            let mut phase = 0.0;
            if self.len > 0 {
                phase = self.head as f32 / (2 * self.len) as f32;
                let target = if matches!(self.state, LoopState::Playing | LoopState::Overdubbing) { 1.0 } else { 0.0 };
                let audible = self.play_gain > 0.0 || target > 0.0;
                if audible {
                    let frame = self.heard();
                    heard = [frame[0] * self.play_gain, frame[1] * self.play_gain];
                    self.play_gain = ramp(self.play_gain, target, fade_step);
                    if self.undo_fade > 0.0 {
                        self.undo_fade = (self.undo_fade - fade_step).max(0.0);
                    }
                }

                // Overdub after reading, so the loop doesn't play the input
                // back on top of itself
                let dub_target = if self.state == LoopState::Overdubbing { 1.0 } else { 0.0 };
                if self.dub_gain > 0.0 || dub_target > 0.0 {
                    self.dub_gain = ramp(self.dub_gain, dub_target, fade_step);
                    let g = self.dub_gain;
                    let keep = 1.0 - g * (1.0 - feedback);
                    let add = [input[0] * g * 0.5, input[1] * g * 0.5];
                    let span = 2 * self.len as u64;
                    // The player heard this sample's frame one round trip ago,
                    // when the playhead was that far back
                    let back = (self.dub_offset * step as i64).rem_euclid(span as i64) as u64;
                    let write = if forward { (self.head + span - back) % span } else { (self.head + back) % span };
                    for k in 0..step {
                        let half = if forward { (write + k) % span } else { (write + span - 1 - k) % span };
                        let frame = (half / 2) as usize;
                        if self.last_frame != Some(frame) {
                            self.tape.overdub(frame, keep, add);
                            let segment = self.segment_of(frame);
                            if self.last_segment != Some(segment) {
                                self.rings_layer[segment] = self.rings_layer[segment].saturating_add(1);
                                self.last_segment = Some(segment);
                            }
                            self.mark_dirty(frame);
                        } else {
                            self.tape.overdub_more(frame, add);
                        }
                        self.last_frame = Some(frame);
                    }
                    if self.dub_gain == 0.0 {
                        self.last_frame = None;
                        self.last_segment = None;
                    }
                }

                if audible {
                    let span = 2 * self.len as u64;
                    let next = if forward { (self.head + step) % span } else { (self.head + span - step) % span };
                    let wrapped = if forward { next < self.head } else { next > self.head };
                    self.head = next;
                    if wrapped && target > 0.0 {
                        self.start_due = true;
                    }
                }

                if self.clear_pending && self.play_gain == 0.0 {
                    self.clear_now();
                }
            }

            let loop_level = self.loop_level.next();
            let dry_level = self.dry_level.next();
            let looped = [heard[0] * loop_level, heard[1] * loop_level];
            outputs[Self::OUT_L].samples[i] = input[0] * dry_level + looped[0];
            outputs[Self::OUT_R].samples[i] = input[1] * dry_level + looped[1];
            outputs[Self::OUT_LOOP_L].samples[i] = looped[0];
            outputs[Self::OUT_LOOP_R].samples[i] = looped[1];
            outputs[Self::OUT_START].samples[i] = if self.start_pulse > 0 { 1.0 } else { 0.0 };
            self.start_pulse = self.start_pulse.saturating_sub(1);
            outputs[Self::OUT_PHASE].samples[i] = phase;

            if let Some(since) = self.since_pulse.as_mut() {
                *since = since.saturating_add(1);
            }
        }

        self.rescan();
        self.overview_due -= context.block_size as i64;
        if self.overview_due <= 0 {
            self.overview_due = (OVERVIEW_INTERVAL_SECONDS * self.sample_rate) as i64;
            self.publish_overview();
        }
    }

    /// The transport stopped: a loop is kept, stopped at the top; a take
    /// still recording is dropped.
    fn reset(&mut self) {
        match self.state {
            LoopState::Empty | LoopState::Armed | LoopState::Recording => self.clear_now(),
            _ => {
                self.finish_clear();
                if self.len > 0 {
                    self.state = LoopState::Stopped;
                    self.head = 0;
                }
            }
        }
        self.finishing = false;
        self.pending_rec = false;
        self.dub_gain = 0.0;
        self.last_frame = None;
        self.last_segment = None;
        self.play_gain = 0.0;
        self.undo_fade = 0.0;
        self.start_pulse = 0;
        self.start_due = false;
        self.prev_gates = [false; 4];
        self.prev_clock = false;
        self.since_pulse = None;
        self.speculating = false;
        self.loop_level.reset(self.loop_level.target());
        self.dry_level.reset(self.dry_level.target());
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let v = &mut readout.values;
        let rate = self.sample_rate.max(1.0);
        v[Self::READOUT_STATE] = self.state.code();
        v[Self::READOUT_SECONDS] = match self.state {
            LoopState::Recording => self.elapsed as f32 / rate,
            _ => self.len as f32 / rate,
        };
        v[Self::READOUT_PHASE] = if self.len > 0 { self.head as f32 / (2 * self.len) as f32 } else { 0.0 };
        v[Self::READOUT_BARS] = -1.0;
        v[Self::READOUT_LAYERS] = self.layers as f32 + if self.tape.has_layer && self.tape.layer_visible { 1.0 } else { 0.0 };
        v[Self::READOUT_UNDO] = match (self.tape.has_layer && self.len > 0, self.tape.layer_visible) {
            (false, _) => 0.0,
            (true, true) => 1.0,
            (true, false) => 2.0,
        };
        v[Self::READOUT_SCALE] = self.ring_frames() as f32 / rate;
        let offset = if self.state == LoopState::Overdubbing { self.dub_offset } else { self.take_offset };
        v[Self::READOUT_OFFSET_MS] = offset as f32 / rate * 1000.0;
        v[Self::READOUT_EDITS] = self.edits as f32;
        if let Some(bar) = self.bar.filter(|&bar| bar >= 1.0) {
            let frames = if self.state == LoopState::Recording { self.elapsed } else { self.len as u64 };
            v[Self::READOUT_BARS] = (frames as f64 / bar) as f32;
        }
        Some(readout)
    }

    fn take_scope_data(&mut self) -> Option<(&[f32], &[f32], bool)> {
        if !self.overview_ready {
            return None;
        }
        self.overview_ready = false;
        let [peaks, rings] = &self.overview;
        Some((peaks, rings, false))
    }

    /// Takes a loop to hold, at the engine's rate: it's copied into the
    /// tape a piece at a time (see [`background`](DspModule::background)),
    /// and then waits, Stopped. Whatever was held is gone. `None` leaves
    /// the loop alone: it's the player's, not the file's.
    fn load_sample(&mut self, sample: Option<Arc<SampleData>>) -> Option<Arc<SampleData>> {
        let sample = sample?;
        if sample.is_empty() || sample.sample_rate() != self.sample_rate || self.tape.capacity() == 0 {
            return Some(sample);
        }
        // Stop at once: what was playing isn't the loop any more
        self.clear_now();
        self.loaded = 0;
        self.loading.replace(sample)
    }

    fn take_retired_sample(&mut self) -> Option<Arc<SampleData>> {
        self.retired.take()
    }

    /// Copies the next piece of a loop being loaded.
    fn background(&mut self, budget: &mut usize) {
        let Some(sample) = self.loading.as_ref() else { return };
        let frames = sample.frames().min(self.max_frames);
        let end = frames.min(self.loaded + *budget);
        let (left, right) = (sample.left(), sample.right());
        for i in self.loaded..end {
            self.tape.record(i, [left[i], right[i]]);
        }
        *budget -= end - self.loaded;
        self.loaded = end;
        if self.loaded == frames {
            self.finish_load();
        }
    }

    /// Copies the loop as heard, the last layer with it unless it's undone.
    /// A take still recording isn't a loop yet, so there's nothing to keep.
    /// The loop is copied as it stands when each piece is read, so one
    /// snapshotted mid-overdub has the overdub as far as it had got.
    fn fill_snapshot(&mut self, snapshot: &mut Snapshot, budget: &mut usize) -> bool {
        if self.loading.is_some() {
            snapshot.finish(SnapshotOutcome::Busy, self.edits);
            return true;
        }
        if !snapshot.has_begun() {
            let len = if self.state == LoopState::Empty { 0 } else { self.len };
            if !snapshot.begin(len, self.sample_rate, self.edits) || len == 0 {
                return true;
            }
        }
        if self.len != snapshot.frames() || self.state == LoopState::Empty {
            snapshot.finish(SnapshotOutcome::Changed, self.edits);
            return true;
        }
        let start = snapshot.copied();
        let count = (snapshot.frames() - start).min(*budget);
        for i in start..start + count {
            snapshot.push(self.loop_frame(i));
        }
        *budget -= count;
        snapshot.is_finished()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::TransportState;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 64;

    /// The knobs at their defaults, Latency off.
    fn params() -> Vec<f32> {
        let mut params: Vec<f32> = Looper::new().parameters().iter().map(|p| p.default).collect();
        params[Looper::PARAM_LATENCY] = 0.0;
        params
    }

    /// Which gates rise on a sample, and the clock.
    #[derive(Clone, Copy, Default)]
    struct Gates {
        rec: bool,
        stop: bool,
        undo: bool,
        clear: bool,
    }

    /// A test rig: a Looper fed sample by sample, gates held for 2 samples.
    struct Rig {
        looper: Looper,
        params: Vec<f32>,
        /// Frames run so far.
        now: usize,
        /// Taps due: (frame, gates).
        taps: Vec<(usize, Gates)>,
        clock: Option<Box<dyn Fn(usize) -> bool>>,
        transport: TransportState,
        latency: usize,
    }

    /// What came out: Out L, Out R, Loop L, Loop R, Start, Phase.
    #[derive(Default)]
    struct Heard {
        out: [Vec<f32>; 2],
        looped: [Vec<f32>; 2],
        start: Vec<f32>,
        phase: Vec<f32>,
    }

    impl Rig {
        fn new() -> Self {
            let mut looper = Looper::new();
            looper.prepare(SR, BLOCK);
            Self { looper, params: params(), now: 0, taps: Vec::new(), clock: None, transport: TransportState::new(), latency: 0 }
        }

        fn tap(&mut self, at: usize, gates: Gates) {
            self.taps.push((at, gates));
        }

        fn rec(&mut self, at: usize) {
            self.tap(at, Gates { rec: true, ..Default::default() });
        }

        /// Runs `frames` frames of `input` (given each frame's index).
        fn run(&mut self, frames: usize, input: impl Fn(usize) -> [f32; 2]) -> Heard {
            let mut heard = Heard::default();
            let mut buffers: Vec<SignalBuffer> = (0..7).map(|_| SignalBuffer::audio(BLOCK)).collect();
            if self.clock.is_none() {
                buffers[Looper::PORT_CLOCK] = SignalBuffer::unconnected(BLOCK, SignalType::Gate);
            }
            let mut outs: Vec<SignalBuffer> = (0..6).map(|_| SignalBuffer::audio(BLOCK)).collect();
            let mut done = 0;
            while done < frames {
                let n = BLOCK.min(frames - done);
                for i in 0..BLOCK {
                    let t = self.now + i;
                    let [l, r] = if i < n { input(t) } else { [0.0; 2] };
                    buffers[0].samples[i] = l;
                    buffers[1].samples[i] = r;
                    let held = |pick: fn(&Gates) -> bool| {
                        self.taps.iter().any(|(at, g)| pick(g) && t >= *at && t < at + 2)
                    };
                    buffers[2].samples[i] = held(|g| g.rec) as u8 as f32;
                    buffers[3].samples[i] = held(|g| g.stop) as u8 as f32;
                    buffers[4].samples[i] = held(|g| g.undo) as u8 as f32;
                    buffers[5].samples[i] = held(|g| g.clear) as u8 as f32;
                    if let Some(clock) = &self.clock {
                        buffers[6].samples[i] = clock(t) as u8 as f32;
                    }
                }
                let refs: Vec<&SignalBuffer> = buffers.iter().collect();
                let ctx = ProcessContext::with_transport(SR, n, self.transport).with_input_latency(self.latency);
                self.looper.process(&refs, &mut outs, &self.params, &ctx);
                for side in 0..2 {
                    heard.out[side].extend_from_slice(&outs[side].samples[..n]);
                    heard.looped[side].extend_from_slice(&outs[2 + side].samples[..n]);
                }
                heard.start.extend_from_slice(&outs[4].samples[..n]);
                heard.phase.extend_from_slice(&outs[5].samples[..n]);
                self.now += n;
                done += n;
            }
            heard
        }

        /// The stored loop, left side.
        fn stored(&self) -> Vec<f32> {
            (0..self.looper.loop_frames()).map(|i| self.looper.loop_frame(i)[0]).collect()
        }
    }

    fn sine(hz: f32, amp: f32) -> impl Fn(usize) -> [f32; 2] {
        move |t| {
            let s = (t as f32 * hz / SR * std::f32::consts::TAU).sin() * amp;
            [s, s]
        }
    }

    fn silence(_: usize) -> [f32; 2] {
        [0.0; 2]
    }

    /// The worst a signal departs from a pure sine at `hz` over `range`:
    /// what a two-pole predictor tuned to `hz` can't explain. A pure sine
    /// leaves nothing; a click leaves its size.
    fn click_size(signal: &[f32], hz: f32, range: std::ops::Range<usize>) -> f32 {
        let c = 2.0 * (hz / SR * std::f32::consts::TAU).cos();
        range.map(|n| (signal[n] - c * signal[n - 1] + signal[n - 2]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn test_state_codes_round_trip() {
        for state in LoopState::ALL {
            assert_eq!(LoopState::from_code(state.code()), state);
        }
    }

    /// Record 1.000 s of a sine: it repeats every 48 000 samples, and the
    /// seam doesn't click.
    #[test]
    fn test_one_second_loop_repeats_exactly_without_a_click() {
        let hz = 110.3; // not a whole number of cycles in a second
        let mut rig = Rig::new();
        rig.rec(1000);
        rig.rec(1000 + 48000);
        let heard = rig.run(1000 + 4 * 48000, sine(hz, 0.5));
        assert_eq!(rig.looper.state(), LoopState::Playing);
        assert_eq!(rig.looper.loop_frames(), 48000);

        let looped = &heard.looped[0];
        let close = 1000 + 48000;
        // Period exactly 48 000 from the second pass on
        for n in (close + 48000)..(close + 2 * 48000) {
            assert_eq!(looped[n], looped[n + 48000], "sample {n}");
        }
        // Across the second wrap (the first is the pre-roll's own frames
        // being written), nothing a sine can't explain above -60 dBFS
        let seam = close + 2 * 48000;
        let click = click_size(looped, hz, seam - 200..seam + 400);
        assert!(click < 0.001, "seam click {click}");

        // Without the pre-roll fold the seam would click: the stored loop's
        // last frame doesn't lead on to its first
        let stored = rig.stored();
        let raw: Vec<f32> = (0..48000).map(|n| sine(hz, 0.5)(1000 + n)[0]).collect();
        let naive: Vec<f32> = raw[47000..].iter().chain(&raw[..1000]).copied().collect();
        assert!(click_size(&naive, hz, 900..1100) > 0.01, "the test sine must click when looped raw");
        assert_eq!(stored[1000..], raw[1000..], "only the first 5 ms are touched by the fold");
    }

    #[test]
    fn test_overdub_sums_layers_and_feedback_fades_the_old_ones() {
        let len = 24000;
        for (feedback, expect) in [(1.0_f32, 1.0_f32), (0.5, 0.5)] {
            let mut rig = Rig::new();
            rig.params[Looper::PARAM_FEEDBACK] = feedback;
            rig.rec(0);
            rig.rec(len);
            rig.run(len + 1000, |_| [0.25, 0.25]);
            let first = rig.stored();
            assert!((first[len / 2] - 0.25).abs() < 1e-6);

            // One silent pass of overdub, punched in at the top of the loop
            let top = 2 * len;
            rig.run(top - rig.now, |_| [0.0; 2]);
            rig.rec(top);
            rig.rec(top + len);
            rig.run(len + 2000, silence);
            let after = rig.stored();
            let mid = len / 2;
            let ratio = after[mid] / first[mid];
            assert!((ratio - expect).abs() < 1e-5, "feedback {feedback}: {ratio}");

            // Sum: a second pass adding 0.25 doubles it at 100%
            if feedback == 1.0 {
                let at = rig.now.next_multiple_of(len);
                rig.run(at - rig.now, silence);
                rig.rec(at);
                rig.rec(at + len);
                rig.run(len + 2000, |_| [0.25, 0.25]);
                assert!((rig.stored()[mid] - 0.5).abs() < 1e-5, "{}", rig.stored()[mid]);
            }
        }
    }

    #[test]
    fn test_undo_restores_bit_exactly_and_redo_brings_it_back() {
        let len = 12000;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        rig.run(2 * len, sine(220.0, 0.4));
        let before = rig.stored();

        rig.rec(2 * len + 100);
        rig.rec(3 * len + 3000);
        rig.run(2 * len, sine(330.0, 0.3));
        let layered = rig.stored();
        assert_ne!(layered, before);

        let undo = Gates { undo: true, ..Default::default() };
        rig.tap(rig.now + 10, undo);
        rig.run(1000, silence);
        assert_eq!(rig.stored(), before, "undo is bit-exact");
        assert_eq!(rig.looper.readout(&rig.params).unwrap().values[Looper::READOUT_UNDO], 2.0);

        rig.tap(rig.now + 10, undo);
        rig.run(1000, silence);
        assert_eq!(rig.stored(), layered, "redo is bit-exact");

        // A new layer makes the last one permanent; undo then takes back
        // only the new one
        rig.rec(rig.now + 10);
        rig.rec(rig.now + 5000);
        rig.run(8000, sine(500.0, 0.2));
        rig.tap(rig.now + 10, undo);
        rig.run(1000, silence);
        assert_eq!(rig.stored(), layered);
    }

    #[test]
    fn test_undo_crossfades() {
        let len = 12000;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        rig.run(2 * len, |_| [0.0; 2]);
        rig.rec(2 * len);
        rig.rec(3 * len);
        rig.run(len + 1000, |_| [0.5, 0.5]);
        let undo_at = rig.now + 100;
        rig.tap(undo_at, Gates { undo: true, ..Default::default() });
        let heard = rig.run(2000, silence);
        let jumps = heard.looped[0].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(jumps < 0.5 / 200.0, "undo stepped by {jumps}");
        assert!(heard.looped[0][1999].abs() < 1e-6, "the layer is gone");
    }

    #[test]
    fn test_start_fires_once_per_wrap_and_phase_ramps() {
        let len = 9600;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        let heard = rig.run(len + 3 * len + 100, |_| [0.1, 0.1]);
        let rises: Vec<usize> =
            (1..heard.start.len()).filter(|&n| heard.start[n] > 0.5 && heard.start[n - 1] <= 0.5).collect();
        assert_eq!(rises, vec![len, 2 * len, 3 * len, 4 * len]);
        assert_eq!(heard.start[len..].iter().filter(|&&s| s > 0.5).count(), 4 * 48, "1 ms each");

        let phase = &heard.phase[len..2 * len];
        assert_eq!(phase[0], 0.0, "the top of the loop plays as Start fires");
        assert!(phase.windows(2).all(|w| w[1] > w[0]), "rises across the loop");
        assert!((phase[len - 1] - (len - 1) as f32 / len as f32).abs() < 1e-6);
        assert_eq!(heard.phase[2 * len], 0.0);
    }

    #[test]
    fn test_speed_and_reverse_leave_the_loop_alone() {
        let len = 4800;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        rig.run(2 * len, sine(300.0, 0.5));
        let kept = rig.stored();

        rig.params[Looper::PARAM_SPEED] = 0.0;
        let half = rig.run(4 * len, silence);
        rig.params[Looper::PARAM_REVERSE] = 1.0;
        rig.params[Looper::PARAM_SPEED] = 2.0;
        rig.run(4 * len, silence);
        assert_eq!(rig.stored(), kept);

        // At ½× the loop takes twice as long to come round: it was at the top
        // as the run began, and comes round once in it
        let rises: Vec<usize> = (1..half.start.len()).filter(|&n| half.start[n] > 0.5 && half.start[n - 1] <= 0.5).collect();
        assert!(half.start[0] > 0.5);
        assert_eq!(rises, vec![2 * len]);
    }

    #[test]
    fn test_overdub_at_half_speed_plays_back_an_octave_up() {
        // An empty-sounding loop, overdubbed with a 200 Hz tone at ½×,
        // heard at 1× as 400 Hz
        let len = 48000;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        rig.run(len + 1000, silence);
        rig.params[Looper::PARAM_SPEED] = 0.0;
        let top = rig.now.next_multiple_of(2 * len) + 2 * len; // half speed: a turn takes 2 len
        rig.run(top - rig.now, silence);
        rig.rec(top + 5000);
        rig.rec(top + 5000 + 30000);
        rig.run(40000, sine(200.0, 0.5));
        rig.params[Looper::PARAM_SPEED] = 1.0;
        let stored = rig.stored();
        // Count zero crossings over a stretch inside what was overdubbed
        let start = stored.iter().position(|x| x.abs() > 0.1).unwrap() + 2000;
        let stretch = &stored[start..start + 4800];
        let crossings = stretch.windows(2).filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0)).count();
        // 0.1 s at 400 Hz crosses zero 80 times
        assert!((78..=82).contains(&crossings), "{crossings}");
    }

    fn clocked_rig(bpm: f32) -> Rig {
        let mut rig = Rig::new();
        let beat = (SR * 60.0 / bpm).round() as usize;
        rig.clock = Some(Box::new(move |t| t % beat < 100));
        rig.transport = TransportState::playing_at(bpm);
        rig.transport.beat_position = Some(0.0);
        rig
    }

    #[test]
    fn test_a_late_tap_starts_on_the_pulse_just_gone() {
        // 120 BPM: a pulse every 24 000 samples
        let mut rig = clocked_rig(120.0);
        let late = 24000 + 960; // 20 ms after the second pulse
        rig.rec(late);
        // Mark each frame with its own time
        rig.run(late + 100, |t| [t as f32 / 1e6, 0.0]);
        assert_eq!(rig.looper.state(), LoopState::Recording);
        assert!((rig.looper.loop_frame(0)[0] - 24000.0 / 1e6).abs() < 1e-9, "frame 0 is the pulse");
    }

    #[test]
    fn test_an_early_tap_waits_for_the_pulse() {
        let mut rig = clocked_rig(120.0);
        let early = 48000 - 9600; // 200 ms before the third pulse
        rig.rec(early);
        rig.run(48000 - 10, |t| [t as f32 / 1e6, 0.0]);
        assert_eq!(rig.looper.state(), LoopState::Armed);
        rig.run(100, |t| [t as f32 / 1e6, 0.0]);
        assert_eq!(rig.looper.state(), LoopState::Recording);
        assert!((rig.looper.loop_frame(0)[0] - 48000.0 / 1e6).abs() < 1e-9);
    }

    #[test]
    fn test_bars_close_the_take_on_exactly_two_bars() {
        let mut rig = clocked_rig(120.0);
        rig.params[Looper::PARAM_BARS] = 2.0; // "2"
        rig.rec(24000 + 100);
        rig.run(24000 + 2 * 96000 + 1000, sine(220.0, 0.3));
        assert_eq!(rig.looper.state(), LoopState::Playing);
        assert_eq!(rig.looper.loop_frames(), 2 * 96000, "two bars of 4/4 at 120 BPM");
    }

    #[test]
    fn test_a_clocked_tap_snaps_to_the_nearest_bar() {
        let bar = 96000;
        // Late by 30 ms: closes back on the bar, the playhead already past it
        let mut rig = clocked_rig(120.0);
        rig.rec(24000);
        rig.rec(24000 + bar + 1440);
        rig.run(24000 + bar + 2000, sine(220.0, 0.3));
        assert_eq!(rig.looper.loop_frames(), bar);
        let phase = rig.looper.readout(&rig.params).unwrap().values[Looper::READOUT_PHASE];
        assert!((phase - 2000.0 / bar as f32).abs() < 1e-6, "{phase}");
        // The pre-roll it had already recorded was folded over the top
        let first = rig.looper.loop_frame(0)[0];
        assert!((first - sine(220.0, 0.3)(24000 + bar)[0]).abs() < 1e-6);

        // Early by a quarter bar: records on to the bar
        let mut rig = clocked_rig(120.0);
        rig.rec(24000);
        rig.rec(24000 + 2 * bar - bar / 4);
        rig.run(24000 + 2 * bar - 10, sine(220.0, 0.3));
        assert_eq!(rig.looper.state(), LoopState::Recording);
        rig.run(20, sine(220.0, 0.3));
        assert_eq!(rig.looper.loop_frames(), 2 * bar);
    }

    /// With a 256-sample round trip, overdubbing what the loop played
    /// (heard 256 samples later) lands on the frames it came from.
    #[test]
    fn test_latency_lines_overdubs_up_with_what_was_heard() {
        let len = 9600;
        let trip = 256;
        let mut rig = Rig::new();
        rig.params[Looper::PARAM_LATENCY] = 1.0;
        let noise = |t: usize| {
            let x = ((t as u32).wrapping_mul(2654435761) >> 8) as f32 / (1 << 24) as f32 - 0.5;
            [x, x]
        };
        // The take, made with no latency to take off
        let mut history: Vec<f32> = Vec::new();
        rig.rec(0);
        rig.rec(len);
        history.extend(rig.run(len + 1000, noise).looped[0].iter());
        let original = rig.stored();

        // Overdub the loop's own output as a player would play along with
        // it, arriving one round trip after it was played
        rig.latency = trip;
        let top = 2 * len;
        history.extend(rig.run(top - rig.now, silence).looped[0].iter());
        let (punch_in, punch_out) = (top + 2000, top + 2000 + len / 2);
        rig.rec(punch_in);
        rig.rec(punch_out);
        while rig.now < punch_out + 2000 {
            let played = history.clone();
            let heard = rig.run(BLOCK, move |t| [played[t - trip]; 2]);
            history.extend(heard.looped[0].iter());
        }
        let after = rig.stored();
        // Away from the punches, every frame doubled exactly where it was
        for i in 2000 + 300..2000 + len / 2 - trip - 300 {
            assert!((after[i] - 2.0 * original[i]).abs() < 1e-5, "frame {i}: {} vs {}", after[i], original[i]);
        }
    }

    #[test]
    fn test_stop_restart_and_clear() {
        let len = 4800;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        rig.run(len + 2000, |_| [0.3, 0.3]);
        let stop = Gates { stop: true, ..Default::default() };
        rig.tap(rig.now + 5, stop);
        let heard = rig.run(1000, silence);
        assert_eq!(rig.looper.state(), LoopState::Stopped);
        assert!(heard.looped[0][900].abs() < 1e-9, "silent once stopped");
        assert!(heard.looped[0].windows(2).all(|w| (w[1] - w[0]).abs() < 0.3 / 200.0), "fades out");

        rig.tap(rig.now + 5, stop);
        let heard = rig.run(500, silence);
        assert_eq!(rig.looper.state(), LoopState::Playing);
        assert!(heard.start[5] > 0.5, "restarts from the top");

        rig.tap(rig.now + 5, Gates { clear: true, ..Default::default() });
        rig.run(1000, silence);
        assert_eq!(rig.looper.state(), LoopState::Empty);
        assert_eq!(rig.looper.loop_frames(), 0);
    }

    #[test]
    fn test_pedals_tap_like_gates() {
        let mut rig = Rig::new();
        rig.run(BLOCK, silence);
        rig.params[Looper::PARAM_PEDALS] = 1.0;
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.state(), LoopState::Recording);
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.state(), LoopState::Recording, "a held pedal is one tap");
        rig.params[Looper::PARAM_PEDALS] = 0.0;
        rig.run(BLOCK * 10, silence);
        rig.params[Looper::PARAM_PEDALS] = 1.0;
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.state(), LoopState::Playing);
    }

    #[test]
    fn test_a_pedal_saved_down_is_not_a_tap() {
        // The first block sees it already down: not a press
        let mut rig = Rig::new();
        rig.params[Looper::PARAM_PEDALS] = 1.0;
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.state(), LoopState::Empty);
    }

    #[test]
    fn test_recording_past_the_limit_closes_there() {
        let mut rig = Rig::new();
        // As if two minutes were 0.2 s, so the test runs quickly
        let max = 9600;
        rig.looper.max_frames = max;
        rig.rec(0);
        rig.run(max + 10, |_| [0.01, 0.01]);
        assert_eq!(rig.looper.state(), LoopState::Playing);
        assert_eq!(rig.looper.loop_frames(), max);
    }

    #[test]
    fn test_transport_stop_keeps_the_loop() {
        let len = 4800;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        rig.run(len + 1000, |_| [0.2, 0.2]);
        rig.looper.reset();
        assert_eq!(rig.looper.state(), LoopState::Stopped);
        assert_eq!(rig.looper.loop_frames(), len);

        // A take still recording is dropped
        let mut rig = Rig::new();
        rig.rec(0);
        rig.run(1000, |_| [0.2, 0.2]);
        rig.looper.reset();
        assert_eq!(rig.looper.state(), LoopState::Empty);
    }

    #[test]
    fn test_overview_shows_the_loop_and_its_layers() {
        let len = 48000;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        // Loud in the first half, quiet in the second
        rig.run(len + 2000, |t| if t % len < len / 2 { [0.8, 0.8] } else { [0.1, 0.1] });
        // One overdub pass
        let top = 2 * len;
        rig.run(top - rig.now, silence);
        rig.rec(top);
        rig.rec(top + len);
        rig.run(len + 4000, silence);
        rig.looper.publish_overview();
        let (peaks, rings, _) = rig.looper.take_scope_data().unwrap();
        assert!((peaks[10] - 0.8).abs() < 1e-5 && (peaks[200] - 0.1).abs() < 1e-5, "{} {}", peaks[10], peaks[200]);
        assert!(rings[10..250].iter().all(|&r| r == 1.0), "one pass over the whole loop");
    }

    /// Copies the loop through a snapshot, `piece` frames per call as the
    /// audio thread would, counting the calls.
    fn snapshot_of(looper: &mut Looper, piece: usize) -> (Snapshot, usize) {
        let mut snapshot = Snapshot::with_capacity(looper.loop_frames() + 10);
        let mut calls = 1;
        loop {
            let mut budget = piece;
            if looper.fill_snapshot(&mut snapshot, &mut budget) {
                return (snapshot, calls);
            }
            assert_eq!(budget, 0, "a call stops short only when its budget runs out");
            calls += 1;
        }
    }

    #[test]
    fn test_a_snapshot_is_the_loop_as_heard_without_an_undone_layer() {
        let len = 12000;
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(len);
        rig.run(2 * len, |t| [(t as f32 * 0.01).sin() * 0.4, (t as f32 * 0.013).cos() * 0.3]);
        let base: Vec<[f32; 2]> = (0..len).map(|i| rig.looper.loop_frame(i)).collect();
        rig.rec(2 * len + 100);
        rig.rec(3 * len + 3000);
        rig.run(2 * len, sine(330.0, 0.3));
        let layered: Vec<[f32; 2]> = (0..len).map(|i| rig.looper.loop_frame(i)).collect();
        assert_ne!(layered, base);

        // A piece at a time, and the same as the loop, bit for bit
        let (snapshot, calls) = snapshot_of(&mut rig.looper, 5000);
        assert_eq!(calls, 3);
        assert_eq!(snapshot.outcome(), SnapshotOutcome::Done);
        assert_eq!(snapshot.edits(), rig.looper.edits());
        let sample = snapshot.into_sample().unwrap();
        assert!(sample.is_stereo());
        assert_eq!(sample.sample_rate(), SR);
        let saved: Vec<[f32; 2]> = (0..len).map(|i| [sample.left()[i], sample.right()[i]]).collect();
        assert_eq!(saved, layered);

        // Undone, the layer is left out
        rig.tap(rig.now + 10, Gates { undo: true, ..Default::default() });
        rig.run(1000, silence);
        let sample = snapshot_of(&mut rig.looper, usize::MAX).0.into_sample().unwrap();
        let saved: Vec<[f32; 2]> = (0..len).map(|i| [sample.left()[i], sample.right()[i]]).collect();
        assert_eq!(saved, base);
    }

    #[test]
    fn test_nothing_to_snapshot_until_a_loop_closes() {
        let mut rig = Rig::new();
        assert_eq!(snapshot_of(&mut rig.looper, 100).0.outcome(), SnapshotOutcome::Empty);
        rig.rec(0);
        rig.run(1000, sine(220.0, 0.5));
        assert_eq!(rig.looper.state(), LoopState::Recording);
        assert_eq!(snapshot_of(&mut rig.looper, 100).0.outcome(), SnapshotOutcome::Empty, "a take is not a loop yet");

        // Too little room says how much it needed
        rig.rec(rig.now);
        rig.run(1000, silence);
        let mut small = Snapshot::with_capacity(10);
        assert!(rig.looper.fill_snapshot(&mut small, &mut 100));
        assert_eq!(small.outcome(), SnapshotOutcome::NoRoom(1000));
    }

    #[test]
    fn test_edits_count_changes_to_the_loop_and_nothing_else() {
        let len = 4800;
        let mut rig = Rig::new();
        let edits = |rig: &Rig| rig.looper.readout(&rig.params).unwrap().values[Looper::READOUT_EDITS];
        assert_eq!(edits(&rig), 0.0);

        rig.rec(0);
        rig.run(100, sine(220.0, 0.5));
        let recording = edits(&rig);
        assert!(recording > 0.0, "a take is a change");
        rig.rec(len);
        // Past the close, and the seam written after it
        rig.run(len + 1000, sine(220.0, 0.5));
        let closed = edits(&rig);
        assert!(closed > recording);

        // Playing, stopping, restarting and the transport stopping change nothing
        rig.run(3 * len, silence);
        let stop = Gates { stop: true, ..Default::default() };
        rig.tap(rig.now + 10, stop);
        rig.tap(rig.now + 500, stop);
        rig.run(2 * len, silence);
        rig.looper.reset();
        rig.run(len, silence);
        assert_eq!(edits(&rig), closed);
        rig.looper.reset();
        assert_eq!(rig.looper.edits() as f32, closed, "nor does stopping a stopped Looper");

        // Rec plays a stopped loop, which changes nothing; then an overdub
        // is a change, its end is, an undo is, and a clear is
        rig.rec(rig.now + 10);
        rig.run(100, silence);
        assert_eq!(rig.looper.state(), LoopState::Playing);
        assert_eq!(edits(&rig), closed);
        rig.rec(rig.now + 10);
        rig.run(100, silence);
        let dubbing = edits(&rig);
        assert!(dubbing > closed);
        rig.rec(rig.now + 10);
        rig.run(100, silence);
        let dubbed = edits(&rig);
        assert!(dubbed > dubbing);
        rig.tap(rig.now + 10, Gates { undo: true, ..Default::default() });
        rig.run(100, silence);
        let undone = edits(&rig);
        assert!(undone > dubbed);
        rig.tap(rig.now + 10, Gates { clear: true, ..Default::default() });
        rig.run(1000, silence);
        assert!(edits(&rig) > undone);
        assert_eq!(rig.looper.state(), LoopState::Empty);
    }

    /// A stereo recording at the test rate, each frame distinct.
    fn recording(frames: usize) -> Arc<SampleData> {
        let left: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.003).sin() * 0.5).collect();
        let right: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.002).cos() * 0.25).collect();
        Arc::new(SampleData::stereo(left, right, SR))
    }

    #[test]
    fn test_a_loaded_loop_waits_stopped_and_plays_from_the_top() {
        let frames = 30000;
        let loop_in = recording(frames);
        let mut rig = Rig::new();
        rig.rec(0);
        rig.rec(5000);
        rig.run(10000, sine(440.0, 0.5));
        assert!(rig.looper.edits() > 0);

        // A piece per call, as on the audio thread
        assert!(rig.looper.load_sample(Some(Arc::clone(&loop_in))).is_none());
        assert!(rig.looper.is_loading());
        assert_eq!(snapshot_of(&mut rig.looper, 100).0.outcome(), SnapshotOutcome::Busy);
        let mut calls = 0;
        while rig.looper.is_loading() {
            rig.looper.background(&mut 4096);
            calls += 1;
        }
        assert_eq!(calls, frames.div_ceil(4096));
        let back = rig.looper.take_retired_sample().expect("the loop goes back to be dropped");
        assert!(Arc::ptr_eq(&back, &loop_in));

        assert_eq!(rig.looper.state(), LoopState::Stopped);
        assert_eq!(rig.looper.loop_frames(), frames);
        assert_eq!(rig.looper.edits(), 0, "it holds just what it was given");
        let readout = rig.looper.readout(&rig.params).unwrap();
        assert_eq!(readout.values[Looper::READOUT_UNDO], 0.0, "no layer to undo");
        assert_eq!(readout.values[Looper::READOUT_LAYERS], 0.0);
        let saved = snapshot_of(&mut rig.looper, usize::MAX).0.into_sample().unwrap();
        assert_eq!(&saved, loop_in.as_ref(), "and saves it again bit for bit");

        // Silent until Rec, then the loop from its top, after the fade in
        let quiet = rig.run(2000, silence);
        assert!(quiet.looped[0].iter().all(|&s| s == 0.0));
        rig.rec(rig.now + 10);
        let heard = rig.run(frames + 1000, silence);
        assert_eq!(rig.looper.state(), LoopState::Playing);
        let fade = (FADE_SECONDS * SR) as usize;
        for i in fade + 1..frames {
            assert_eq!(heard.looped[0][10 + i], loop_in.left()[i], "frame {i}");
            assert_eq!(heard.looped[1][10 + i], loop_in.right()[i], "frame {i}");
        }
        assert_eq!(rig.looper.edits(), 0, "playing it is not a change");
    }

    #[test]
    fn test_rec_while_loading_drops_the_load() {
        let mut rig = Rig::new();
        rig.looper.load_sample(Some(recording(48000)));
        rig.looper.background(&mut 1000);
        rig.rec(5);
        rig.run(100, |_| [0.5, 0.5]);
        assert!(!rig.looper.is_loading());
        assert!(rig.looper.take_retired_sample().is_some());
        assert_eq!(rig.looper.state(), LoopState::Recording);
        assert_eq!(rig.looper.loop_frame(50), [0.5, 0.5]);
    }

    #[test]
    fn test_a_loop_at_another_rate_is_handed_back() {
        let mut rig = Rig::new();
        let other = Arc::new(SampleData::mono(vec![0.5; 1000], 44100.0));
        assert!(rig.looper.load_sample(Some(other)).is_some());
        assert!(!rig.looper.is_loading());
        // And taking the file away leaves the loop alone
        assert!(rig.looper.load_sample(None).is_none());
    }

    #[test]
    fn test_a_loaded_loop_snaps_to_whole_bars_when_clocked() {
        // 120 BPM, 4/4: a bar is 96 000 frames. A file saved at 44.1 kHz and
        // resampled comes back a frame short of two bars
        let mut rig = clocked_rig(120.0);
        rig.looper.load_sample(Some(recording(2 * 96000 - 1)));
        rig.looper.background(&mut { usize::MAX });
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.loop_frames(), 2 * 96000);
        assert_eq!(rig.looper.edits(), 0);
        let last = rig.looper.loop_frame(2 * 96000 - 2);
        assert_eq!(rig.looper.loop_frame(2 * 96000 - 1), last, "the frame added holds the last");

        // One well off a bar is a free loop, and keeps its length
        let mut rig = clocked_rig(120.0);
        rig.looper.load_sample(Some(recording(100000)));
        rig.looper.background(&mut { usize::MAX });
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.loop_frames(), 100000);

        // Unclocked, a loop is left as it is
        let mut rig = Rig::new();
        rig.looper.load_sample(Some(recording(2 * 96000 - 1)));
        rig.looper.background(&mut { usize::MAX });
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.loop_frames(), 2 * 96000 - 1);
    }

    #[test]
    fn test_a_loop_saved_at_another_rate_comes_back_in_tune_and_on_the_bar() {
        // Two bars at 97 BPM, saved at 44.1 kHz, opened at 48 kHz: resampling
        // leaves it a frame off, and the clock puts it back on the bar
        let bpm = 97.0;
        let bar_at = |rate: f64| rate * 60.0 / bpm as f64 * 4.0;
        let frames = (2.0 * bar_at(44100.0)).round() as usize;
        let hz = 441.0;
        let tone: Vec<f32> = (0..frames).map(|n| (n as f32 * hz / 44100.0 * std::f32::consts::TAU).sin() * 0.5).collect();
        let resampled = SampleData::mono(tone, 44100.0).resampled(SR);
        let whole = (2.0 * bar_at(SR as f64)).round() as usize;
        assert_ne!(resampled.frames(), whole, "the test needs a loop that comes back off the bar");

        let mut rig = clocked_rig(bpm);
        rig.looper.load_sample(Some(Arc::new(resampled)));
        rig.looper.background(&mut { usize::MAX });
        rig.run(BLOCK, silence);
        assert_eq!(rig.looper.loop_frames(), whole);
        // Still 441 Hz: count rising zero crossings over a second
        let stored = rig.stored();
        let crossings = stored[..48000].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((440..=442).contains(&crossings), "{crossings}");
    }
}
