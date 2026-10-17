//! Recording straight into the Sampler, or the Chopper: their In jacks and
//! Rec.
//!
//! # Room
//!
//! A take is written into room the UI set aside off the audio thread (see
//! [`DspModule::give_room`](crate::dsp::DspModule::give_room)): an empty
//! recording a minute long, held by nothing else, so the Sampler can write
//! it here and then play it as it stands. The UI hands room to a Sampler
//! with something patched into In, and again after each take uses it up.
//! Unpatched, the Sampler lets its room go.
//!
//! # Starting and stopping
//!
//! Rec on the face starts a take and stops it again; patched, the Rec jack
//! takes over, recording while it's high. A take starts at once, or:
//!
//! - with **Thresh** above -∞ dB, it waits until the input is that loud,
//!   keeping the last few milliseconds before, so the take starts on the
//!   first word and not on reaching for the mouse;
//! - with **Take** set to a number of bars and a Clock running, it starts
//!   on the next bar and lasts exactly that long, so it's already a loop.
//!   Rec pressed just after a bar line counts as on it.
//!
//! It stops on Rec, the gate falling, the bars running out, or the room.
//! A take that ends while a Clock runs is measured in its beats, so the
//! Chopper can loop it at the patch's tempo: a take in bars is exactly
//! their beats.
//!
//! # Timing
//!
//! Live input reaches the patch a round trip after the player played it,
//! along with what they heard ([`ProcessContext::input_latency`]), so a
//! take's frames are written that many frames late: frame 0 of a clocked
//! take is what was played on the bar line. A stop is late by as much, so
//! the take runs on for the round trip after Rec, to what was played as
//! it was pressed.

use std::sync::Arc;

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, Transition},
    signal::SignalBuffer,
    FinishedTake, SampleData,
};

/// The longest take, in seconds: the room the UI sets aside for one.
pub const TAKE_ROOM_SECONDS: f32 = 60.0;

/// The Take menu, in saved order.
pub const TAKE_LENGTHS: &[&str] = &["Free", "1 bar", "2 bars", "4 bars", "8 bars", "16 bars"];

/// Bars a take lasts at each Take setting (0 for Free).
const BAR_COUNTS: [u32; 6] = [0, 1, 2, 4, 8, 16];

/// The Thresh knob's lowest setting, shown as -∞ dB: no threshold, so a
/// take starts as soon as Rec is pressed.
pub const NO_THRESHOLD_DB: f32 = -60.0;

/// A press this soon after a bar line counts as on it.
pub const LATE_TAP_SECONDS: f32 = 0.05;

/// How much of what came before the sound a take waiting for it keeps.
pub const PRE_ROLL_SECONDS: f32 = 0.03;

/// A take shorter than this is a slip of the finger, and is dropped.
const SHORTEST_TAKE_SECONDS: f32 = 0.01;

/// How many columns the live overview of a take has.
pub const TAKE_COLUMNS: usize = 256;

/// How long the live overview spans at first: it doubles each time the
/// take outgrows it.
const FIRST_SCALE_SECONDS: f32 = 4.0;

/// How often the live overview goes to the display, in seconds.
const OVERVIEW_INTERVAL_SECONDS: f32 = 1.0 / 30.0;

/// How quickly the level a take waits for rises to a sound: fast enough
/// for a consonant, slow enough that one stray sample doesn't count.
const ONSET_ATTACK_SECONDS: f32 = 0.001;

/// How quickly that level falls away again.
const ONSET_RELEASE_SECONDS: f32 = 0.05;

/// How quickly the input meter falls.
const METER_RELEASE_SECONDS: f32 = 0.3;

/// The take count wraps below this, so a readout's `f32` holds it exactly.
const TAKES_WRAP: u32 = (1 << 24) - 1;

/// Where the recorder is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecState {
    #[default]
    Idle,
    /// Rec was pressed with a Take in bars: recording starts on the next bar.
    Armed,
    /// Rec was pressed with a threshold: recording starts on a sound.
    Waiting,
    Recording,
}

impl RecState {
    const ALL: [RecState; 4] = [RecState::Idle, RecState::Armed, RecState::Waiting, RecState::Recording];

    /// The state as a readout value.
    pub fn code(self) -> f32 {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0) as f32
    }

    pub fn from_code(code: f32) -> Self {
        Self::ALL.get(code.round().max(0.0) as usize).copied().unwrap_or_default()
    }
}

/// What the recorder hears and is asked this block.
pub struct Controls<'a> {
    /// The In jacks, where patched.
    pub left: Option<&'a SignalBuffer>,
    pub right: Option<&'a SignalBuffer>,
    /// The Rec jack, where patched: it takes over from the button.
    pub gate: Option<&'a SignalBuffer>,
    /// The Rec button on the face, down or up.
    pub button: bool,
    /// The Thresh knob, in dB.
    pub threshold_db: f32,
    /// The Take menu.
    pub length: f32,
}

/// Records takes into room the UI sets aside.
pub struct Recorder {
    room: Option<Arc<SampleData>>,
    /// Room it has let go of, for the Sampler to hand back.
    let_go: Option<Arc<SampleData>>,
    /// The take just finished, for the UI to keep.
    finished: Option<FinishedTake>,
    state: RecState,
    sample_rate: f32,
    /// Frames since the moment the take began.
    elapsed: u64,
    /// Frames of round trip: writes begin this far in.
    offset: u64,
    /// The take ends once it holds this many frames.
    close_at: Option<usize>,
    /// Where the next frame of pre-roll goes, while waiting for a sound.
    ring_head: usize,
    /// Recording from the bar line just gone, in case a late press wants it.
    speculating: bool,
    prev_button: Option<bool>,
    /// The Rec jack, for its edges.
    gate: Edge,
    /// The bar the last frame was in, while a Clock runs.
    prev_bar: Option<i64>,
    /// Frames in a beat, while a Clock runs (the last it ran at, after).
    beat_frames: Option<f64>,
    /// Beats in a bar, as the Clock last counted them.
    per_bar: u32,
    /// Frames in a beat of the take under way, if it's in bars.
    bar_take: Option<f64>,
    /// How loud the input is, for a take waiting for a sound.
    onset: f32,
    /// The input's peak, falling slowly, for the display.
    meter: f32,
    takes: u32,
    /// The take so far: the loudest and quietest sample in each column.
    highs: [f32; TAKE_COLUMNS],
    lows: [f32; TAKE_COLUMNS],
    /// Frames the columns span.
    scale: usize,
    overview_due: i64,
    overview_ready: bool,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            room: None,
            let_go: None,
            finished: None,
            state: RecState::Idle,
            sample_rate: 44100.0,
            elapsed: 0,
            offset: 0,
            close_at: None,
            ring_head: 0,
            speculating: false,
            prev_button: None,
            gate: Edge::LOW,
            prev_bar: None,
            beat_frames: None,
            per_bar: 4,
            bar_take: None,
            onset: 0.0,
            meter: 0.0,
            takes: 0,
            highs: [0.0; TAKE_COLUMNS],
            lows: [0.0; TAKE_COLUMNS],
            scale: 1,
            overview_due: 0,
            overview_ready: false,
        }
    }
}

impl Recorder {
    /// Runs at `sample_rate`: room for another rate is let go of, and a
    /// take in hand is dropped. Off the audio thread.
    pub fn prepare(&mut self, sample_rate: f32) {
        if sample_rate != self.sample_rate {
            self.cancel();
            self.prev_bar = None;
            if let Some(room) = self.room.take() {
                self.let_go = Some(room);
            }
        }
        self.sample_rate = sample_rate;
    }

    pub fn state(&self) -> RecState {
        self.state
    }

    /// Takes recorded so far.
    pub fn takes(&self) -> u32 {
        self.takes
    }

    /// How many frames the room it holds has, or 0 with none.
    pub fn room(&self) -> usize {
        self.room.as_ref().map_or(0, |room| room.room())
    }

    /// How many beats long the take just finished is, if a Clock ran (see
    /// [`FinishedTake::beats`]); `None` once the UI has it.
    pub fn finished_beats(&self) -> Option<u32> {
        self.finished.as_ref().and_then(|take| take.beats)
    }

    /// Frames the take in hand holds.
    pub fn recorded(&self) -> usize {
        match self.state {
            RecState::Recording => self.room.as_ref().map_or(0, |room| room.frames()),
            _ => 0,
        }
    }

    /// The input's peak, falling slowly, 0 to 1 and over.
    pub fn meter(&self) -> f32 {
        self.meter
    }

    /// Seconds the live overview spans.
    pub fn scale_seconds(&self) -> f32 {
        self.scale as f32 / self.sample_rate
    }

    /// Seconds in a beat of the take under way, if it's in bars (0 if not),
    /// and beats in its bar: the face counts it in bars and beats.
    pub fn take_meter(&self) -> (f32, u32) {
        (self.bar_take.map_or(0.0, |frames| (frames / self.sample_rate as f64) as f32), self.per_bar)
    }

    /// Takes room to record into, if it fits: at this rate, held by nothing
    /// else, and not while a take is under way. Returns room to hand back.
    ///
    /// REAL-TIME SAFE.
    pub fn give_room(&mut self, mut room: Arc<SampleData>) -> Option<Arc<SampleData>> {
        let fits = room.sample_rate() == self.sample_rate && Arc::get_mut(&mut room).is_some();
        let busy = self.state != RecState::Idle || self.speculating;
        if !fits || busy {
            return Some(room);
        }
        self.room.replace(room)
    }

    /// Room it has let go of since the last call.
    pub fn take_let_go(&mut self) -> Option<Arc<SampleData>> {
        self.let_go.take()
    }

    /// The take finished since the last call.
    pub fn take_finished(&mut self) -> Option<FinishedTake> {
        self.finished.take()
    }

    /// The live overview, when there's a new one to show: the take's
    /// loudest and quietest sample in each column.
    pub fn take_overview(&mut self) -> Option<(&[f32], &[f32])> {
        if !std::mem::take(&mut self.overview_ready) {
            return None;
        }
        Some((&self.highs, &self.lows))
    }

    /// Listens to a block. Returns a take that finished in it, for the
    /// Sampler to play from now on.
    ///
    /// REAL-TIME SAFE: writes only within the room it was given.
    pub fn run(&mut self, controls: &Controls, context: &ProcessContext) -> Option<Arc<SampleData>> {
        let rate = self.sample_rate;
        let pressed = controls.button && !self.prev_button.unwrap_or(controls.button);
        self.prev_button = Some(controls.button);
        let (left, right) = match (controls.left, controls.right) {
            (Some(l), None) => (Some(l), Some(l)),
            (None, Some(r)) => (Some(r), Some(r)),
            sides => sides,
        };
        if left.is_none() {
            // Nothing to record: a take under way ends, and the room goes back
            let done = if self.state == RecState::Recording { self.finish() } else { self.cancel_none() };
            if let Some(room) = self.room.take() {
                self.let_go = Some(room);
            }
            self.gate.reset();
            return done;
        }

        let bars = BAR_COUNTS[(controls.length.round().max(0.0) as usize).min(BAR_COUNTS.len() - 1)];
        let transport = &context.transport;
        let beats = transport.beat_position.zip(transport.tempo_bpm.filter(|&bpm| bpm > 0.0));
        let per_bar = transport.time_sig_numerator.max(1) as f64;
        let bar = beats.map(|(_, bpm)| rate as f64 * 60.0 / bpm as f64 * per_bar);
        if let Some(bar) = bar {
            self.beat_frames = Some(bar / per_bar);
            self.per_bar = per_bar as u32;
        }
        // A take in bars, with a Clock to count them
        let clocked = bar.filter(|_| bars > 0).map(|bar| (bars as f64 * bar).round().max(1.0) as usize);
        let on_sound = controls.threshold_db > NO_THRESHOLD_DB + 0.05;
        let threshold = 10f32.powf(controls.threshold_db / 20.0);
        let latency = context.input_latency as u64;
        let late = (LATE_TAP_SECONDS * rate) as u64;
        let pre_roll = ((PRE_ROLL_SECONDS * rate) as usize).max(1);
        let rise = 1.0 - (-1.0 / (ONSET_ATTACK_SECONDS * rate)).exp();
        let fall = (-1.0 / (ONSET_RELEASE_SECONDS * rate)).exp();
        self.meter *= (-(context.block_size as f32) / (METER_RELEASE_SECONDS * rate)).exp();
        let beats_per_frame = beats.map_or(0.0, |(_, bpm)| bpm as f64 / 60.0 / rate as f64);

        let mut done = None;
        for i in 0..context.block_size {
            let sum = |buffer: &SignalBuffer| (0..buffer.channels()).map(|c| buffer.voice(c).at(i)).sum::<f32>();
            let frame = [left.map_or(0.0, sum), right.map_or(0.0, sum)];
            let level = frame[0].abs().max(frame[1].abs());
            self.onset = if level > self.onset { self.onset + (level - self.onset) * rise } else { self.onset * fall };
            self.meter = self.meter.max(level);

            // The bar lines, while a Clock runs
            let bar_line = match beats {
                Some((first, _)) => {
                    let place = (first + i as f64 * beats_per_frame) / per_bar;
                    let index = (place + 1e-9).floor() as i64;
                    let line = match self.prev_bar {
                        Some(prev) => index > prev,
                        None => place - (index as f64) < 1e-6,
                    };
                    self.prev_bar = Some(index);
                    line
                }
                None => {
                    self.prev_bar = None;
                    false
                }
            };

            // Rec: the jack's edges, or the button's press on the block's first sample
            let (start, stop) = match controls.gate {
                Some(gate) => {
                    let edge = self.gate.tick(gate.at(i));
                    (edge == Some(Transition::Rising), edge == Some(Transition::Falling))
                }
                None => {
                    self.gate.reset();
                    let tap = pressed && i == 0;
                    (tap && self.state == RecState::Idle, tap && self.state != RecState::Idle)
                }
            };

            match self.state {
                RecState::Idle => {
                    if bar_line && clocked.is_some() && !on_sound && self.room.is_some() {
                        // Capture from this bar, in case a late press wants it
                        self.begin(clocked, latency);
                        self.speculating = true;
                    }
                    if start && self.room.is_some() {
                        if on_sound {
                            self.wait();
                        } else if clocked.is_some() {
                            if self.speculating && self.elapsed <= late {
                                // A late press: the take began on the bar just gone
                                self.speculating = false;
                                self.state = RecState::Recording;
                            } else {
                                self.speculating = false;
                                self.state = RecState::Armed;
                            }
                        } else {
                            self.begin(None, latency);
                            self.state = RecState::Recording;
                        }
                    }
                }
                RecState::Armed => {
                    if stop {
                        self.cancel();
                    } else if bar_line || clocked.is_none() {
                        // On the bar; or at once if the Clock has stopped
                        self.begin(clocked, latency);
                        self.state = RecState::Recording;
                    }
                }
                RecState::Waiting => {
                    if stop {
                        self.cancel();
                    }
                }
                RecState::Recording => {
                    if stop {
                        // What was played as Rec went is still a round trip away
                        let recorded = self.room.as_ref().map_or(0, |room| room.frames());
                        let to = recorded + latency as usize;
                        self.close_at = Some(self.close_at.map_or(to, |at| at.min(to)));
                        if recorded >= to {
                            done = done.or_else(|| self.finish());
                        }
                    }
                }
            }

            if self.state == RecState::Waiting {
                if self.onset > threshold {
                    self.sound_began(clocked, pre_roll);
                } else {
                    self.pre_roll(frame, pre_roll);
                }
            }
            let capturing = self.state == RecState::Recording || (self.state == RecState::Idle && self.speculating);
            if capturing {
                if self.elapsed >= self.offset && !self.write(frame) {
                    // Out of room
                    if self.state == RecState::Recording {
                        done = done.or_else(|| self.finish());
                    }
                    self.speculating = false;
                }
                self.elapsed += 1;
                if self.speculating && self.elapsed > late + self.offset {
                    self.speculating = false;
                }
                let recorded = self.room.as_ref().map_or(0, |room| room.frames());
                if self.state == RecState::Recording && self.close_at.is_some_and(|at| recorded >= at) {
                    done = done.or_else(|| self.finish());
                }
            }
        }

        if self.state == RecState::Recording {
            self.overview_due -= context.block_size as i64;
            if self.overview_due <= 0 {
                self.overview_due = (OVERVIEW_INTERVAL_SECONDS * rate) as i64;
                self.overview_ready = true;
            }
        }
        done
    }

    /// The transport stopped: a take under way ends there, and a press
    /// waiting to start one is let go.
    ///
    /// REAL-TIME SAFE.
    pub fn stop(&mut self) -> Option<Arc<SampleData>> {
        self.prev_bar = None;
        match self.state {
            RecState::Recording => self.finish(),
            _ => self.cancel_none(),
        }
    }

    /// Starts a take's clock, from now: `clocked` frames long if it's in bars.
    fn begin(&mut self, clocked: Option<usize>, latency: u64) {
        if let Some(room) = self.room.as_mut().and_then(Arc::get_mut) {
            room.clear();
        }
        self.elapsed = 0;
        self.offset = latency;
        self.close_at = clocked;
        self.speculating = false;
        self.start_overview(clocked);
    }

    /// Waits for a sound, keeping what comes before it.
    fn wait(&mut self) {
        if let Some(room) = self.room.as_mut().and_then(Arc::get_mut) {
            room.clear();
        }
        self.ring_head = 0;
        self.speculating = false;
        self.state = RecState::Waiting;
    }

    /// Keeps a frame of what came before the sound, round a ring.
    fn pre_roll(&mut self, frame: [f32; 2], pre_roll: usize) {
        let Some(room) = self.room.as_mut().and_then(Arc::get_mut) else { return };
        if room.frames() < pre_roll {
            room.push(frame);
        } else {
            room.set(self.ring_head, frame);
        }
        self.ring_head = (self.ring_head + 1) % pre_roll;
    }

    /// The sound a take waited for has come: the take runs from the
    /// pre-roll before it, or, in bars, from the sound itself, so the bars
    /// count from it.
    fn sound_began(&mut self, clocked: Option<usize>, pre_roll: usize) {
        let Some(room) = self.room.as_mut().and_then(Arc::get_mut) else { return };
        if clocked.is_some() {
            room.clear();
        } else if room.frames() == pre_roll {
            room.rotate_left(self.ring_head);
        }
        let held = room.frames();
        self.state = RecState::Recording;
        // Written as it comes: the sound is where the take begins
        self.offset = 0;
        self.elapsed = held as u64;
        self.close_at = clocked;
        self.start_overview(clocked);
        for n in 0..held {
            let (l, r) = self.room.as_ref().map_or((0.0, 0.0), |room| (room.left()[n], room.right()[n]));
            self.mark(n, [l, r]);
        }
    }

    /// Writes the next frame of the take. Returns false if the room is full.
    fn write(&mut self, frame: [f32; 2]) -> bool {
        let Some(room) = self.room.as_mut().and_then(Arc::get_mut) else { return false };
        if !room.push(frame) {
            return false;
        }
        let at = room.frames() - 1;
        self.mark(at, frame);
        true
    }

    /// Ends the take, if there's enough of it, handing it over.
    fn finish(&mut self) -> Option<Arc<SampleData>> {
        let shortest = (SHORTEST_TAKE_SECONDS * self.sample_rate) as usize;
        if self.room.as_ref().is_none_or(|room| room.frames() < shortest) {
            return self.cancel_none();
        }
        self.state = RecState::Idle;
        self.speculating = false;
        self.close_at = None;
        let take = self.room.take()?;
        self.takes = self.takes % TAKES_WRAP + 1;
        // Overwriting one the UI never took would drop it here; it always has
        debug_assert!(self.finished.is_none(), "a take the UI never heard of");
        let beats = self.beat_frames.map(|beat| ((take.frames() as f64 / beat).round() as u32).max(1));
        self.finished = Some(FinishedTake { count: self.takes, sample: Arc::clone(&take), beats });
        Some(take)
    }

    /// Lets go of a press, and whatever it recorded.
    fn cancel(&mut self) {
        self.state = RecState::Idle;
        self.speculating = false;
        self.close_at = None;
        if let Some(room) = self.room.as_mut().and_then(Arc::get_mut) {
            room.clear();
        }
    }

    fn cancel_none(&mut self) -> Option<Arc<SampleData>> {
        self.cancel();
        None
    }

    /// Clears the overview for a take `clocked` frames long, or free.
    fn start_overview(&mut self, clocked: Option<usize>) {
        self.highs = [0.0; TAKE_COLUMNS];
        self.lows = [0.0; TAKE_COLUMNS];
        self.scale = clocked.unwrap_or((FIRST_SCALE_SECONDS * self.sample_rate) as usize).max(TAKE_COLUMNS);
        self.bar_take = clocked.and(self.beat_frames);
        self.overview_due = 0;
    }

    /// Adds frame `at` to the overview, widening it to fit.
    fn mark(&mut self, at: usize, [l, r]: [f32; 2]) {
        while at >= self.scale {
            // Twice as long: each pair of columns becomes one
            for k in 0..TAKE_COLUMNS / 2 {
                self.highs[k] = self.highs[2 * k].max(self.highs[2 * k + 1]);
                self.lows[k] = self.lows[2 * k].min(self.lows[2 * k + 1]);
            }
            self.highs[TAKE_COLUMNS / 2..].fill(0.0);
            self.lows[TAKE_COLUMNS / 2..].fill(0.0);
            self.scale *= 2;
        }
        let column = at * TAKE_COLUMNS / self.scale;
        self.highs[column] = self.highs[column].max(l).max(r);
        self.lows[column] = self.lows[column].min(l).min(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{context::TransportState, DspModule, SignalType};
    use crate::modules::sampler::Sampler;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 64;

    /// What a player played at frame `p` of their own time: the frame
    /// itself, small enough to be read back exactly.
    fn played(p: usize) -> f32 {
        p as f32 / 1_000_000.0
    }

    /// A Sampler with In patched, run a block at a time from frame 0.
    struct Rig {
        sampler: Sampler,
        params: Vec<f32>,
        now: usize,
        /// The input's round trip, in frames.
        latency: usize,
        /// A Clock running at this tempo from frame 0, if any.
        tempo: Option<f32>,
        /// The Rec jack, where patched: high over these frames.
        gate: Option<Vec<(usize, usize)>>,
        /// What In carries at each engine frame.
        input: Box<dyn Fn(usize) -> f32>,
        /// What L played, frame by frame.
        heard: Vec<f32>,
    }

    impl Rig {
        fn new(room_seconds: f32) -> Self {
            let mut sampler = Sampler::new();
            sampler.prepare(SR, BLOCK);
            let mut params: Vec<f32> = sampler.parameters().iter().map(|p| p.default).collect();
            params[Sampler::PARAM_LEVEL] = 1.0;
            let mut rig = Self { sampler, params, now: 0, latency: 0, tempo: None, gate: None, input: Box::new(|_| 0.0), heard: Vec::new() };
            rig.give_room(room_seconds);
            rig
        }

        fn give_room(&mut self, seconds: f32) {
            let room = Arc::new(SampleData::with_room((seconds * SR) as usize, SR));
            assert!(self.sampler.give_room(room).is_none(), "taken");
        }

        /// Plays what the player played, a round trip late, as live input is.
        fn playing_along(mut self, latency: usize) -> Self {
            self.latency = latency;
            self.input = Box::new(move |t| if t >= latency { played(t - latency) } else { 0.0 });
            self
        }

        fn readout(&self, index: usize) -> f32 {
            self.sampler.readout(&self.params).unwrap().values[index]
        }

        fn state(&self) -> RecState {
            RecState::from_code(self.readout(Sampler::READOUT_REC_STATE))
        }

        /// Runs to frame `to`, a block at a time.
        fn run_to(&mut self, to: usize) {
            assert_eq!(to % BLOCK, 0);
            let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
            let mut gate = SignalBuffer::gate(BLOCK);
            let mut input = SignalBuffer::audio(BLOCK);
            let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio), SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
            while self.now < to {
                for i in 0..BLOCK {
                    let t = self.now + i;
                    input.samples[i] = (self.input)(t);
                    let high = self.gate.as_ref().is_some_and(|spans| spans.iter().any(|&(from, to)| t >= from && t < to));
                    gate.samples[i] = if high { 1.0 } else { 0.0 };
                }
                let transport = match self.tempo {
                    Some(bpm) => TransportState { beat_position: Some(self.now as f64 * bpm as f64 / 60.0 / SR as f64), ..TransportState::playing_at(bpm) },
                    None => TransportState::new(),
                };
                let ctx = ProcessContext::with_transport(SR, BLOCK, transport).with_input_latency(self.latency);
                let rec: &SignalBuffer = if self.gate.is_some() { &gate } else { &unpatched };
                let inputs = [&unpatched, &unpatched, &unpatched, &unpatched, &unpatched, &unpatched, &input, &unpatched, rec];
                self.sampler.process(&inputs, &mut outputs, &self.params, &ctx);
                self.heard.extend_from_slice(&outputs[0].samples);
                self.now += BLOCK;
            }
        }

        /// Presses Rec on the face at frame `at`, for a block.
        fn press(&mut self, at: usize) {
            self.run_to(at);
            self.params[Sampler::PARAM_REC_BUTTON] = 1.0;
            self.run_to(at + BLOCK);
            self.params[Sampler::PARAM_REC_BUTTON] = 0.0;
        }

        /// The take just finished, which the UI hears of once.
        fn take(&mut self) -> (u32, Arc<SampleData>) {
            let take = self.sampler.take_finished_take().expect("a take");
            assert!(self.sampler.take_finished_take().is_none());
            (take.count, take.sample)
        }
    }

    /// Asserts a take holds what was played from frame `from`, `frames` long.
    fn assert_played(take: &SampleData, from: usize, frames: usize) {
        assert_eq!(take.frames(), frames, "length");
        for k in [0, 1, frames / 2, frames - 1] {
            assert_eq!(take.left()[k], played(from + k), "frame {k}");
        }
    }

    #[test]
    fn test_a_free_take_holds_what_was_played_between_the_presses() {
        // A 10 ms round trip: the take is what was played, not what arrived
        let mut rig = Rig::new(2.0).playing_along(480);
        rig.press(6400);
        assert_eq!(rig.state(), RecState::Recording);
        rig.press(30016);
        rig.run_to(32000);
        assert_eq!(rig.state(), RecState::Idle);
        assert_eq!(rig.sampler.recorder.finished_beats(), None, "no Clock to count them");
        let (count, take) = rig.take();
        assert_eq!(count, 1);
        assert_played(&take, 6400, 30016 - 6400);
        assert_eq!(rig.readout(Sampler::READOUT_TAKES), 1.0);
        assert_eq!(rig.readout(Sampler::READOUT_ROOM), 0.0, "the room is the take now");
    }

    #[test]
    fn test_a_take_in_bars_starts_on_the_bar_and_lines_up_with_the_clock() {
        // 120 BPM: a bar is 2 s. Pressed mid-bar, it waits for the next
        let mut rig = Rig::new(3.0).playing_along(480);
        rig.tempo = Some(120.0);
        rig.params[Sampler::PARAM_TAKE] = 1.0;
        rig.press(30016);
        assert_eq!(rig.state(), RecState::Armed);
        rig.run_to(96000);
        assert_eq!(rig.state(), RecState::Armed);
        rig.run_to(96000 + BLOCK);
        assert_eq!(rig.state(), RecState::Recording);
        // Counted in its beats as it comes in: half a second each, four to the bar
        assert_eq!(rig.readout(Sampler::READOUT_REC_BEAT), 0.5);
        assert_eq!(rig.readout(Sampler::READOUT_REC_BAR), 4.0);
        // Two bars on, and the round trip after them: a bar, four beats
        rig.run_to(4 * 48000 + 512);
        assert_eq!(rig.sampler.recorder.finished_beats(), Some(4));
        let (_, take) = rig.take();
        assert_played(&take, 96000, 96000);
    }

    #[test]
    fn test_a_late_press_starts_on_the_bar_just_gone() {
        // 21 ms after the bar line, with a 10 ms round trip: nothing is lost
        let mut rig = Rig::new(3.0).playing_along(480);
        rig.tempo = Some(120.0);
        rig.params[Sampler::PARAM_TAKE] = 1.0;
        rig.press(96000 + 1024);
        assert_eq!(rig.state(), RecState::Recording);
        rig.run_to(4 * 48000 + 512);
        let (_, take) = rig.take();
        assert_played(&take, 96000, 96000);

        // Too late, and it waits for the next bar
        let mut rig = Rig::new(3.0).playing_along(480);
        rig.tempo = Some(120.0);
        rig.params[Sampler::PARAM_TAKE] = 1.0;
        rig.press(96000 + 4800);
        assert_eq!(rig.state(), RecState::Armed);
    }

    #[test]
    fn test_rec_again_ends_a_take_in_bars_early() {
        let mut rig = Rig::new(3.0);
        rig.input = Box::new(played);
        rig.tempo = Some(120.0);
        rig.params[Sampler::PARAM_TAKE] = 1.0;
        rig.press(BLOCK);
        rig.press(48000);
        rig.run_to(50048);
        let (_, take) = rig.take();
        assert_played(&take, 0, 48000);
    }

    #[test]
    fn test_a_take_waiting_for_a_sound_starts_just_before_it() {
        // Silence, then a steady tone from frame 20000
        let mut rig = Rig::new(2.0);
        rig.input = Box::new(|t| if t >= 20000 { 0.5 } else { 0.0 });
        rig.params[Sampler::PARAM_REC_THRESHOLD] = -30.0;
        rig.press(BLOCK);
        rig.run_to(19968);
        assert_eq!(rig.state(), RecState::Waiting);
        rig.run_to(20032);
        assert_eq!(rig.state(), RecState::Recording);
        rig.press(40000);
        rig.run_to(41024);
        let (_, take) = rig.take();
        let pre_roll = (PRE_ROLL_SECONDS * SR) as usize;
        let first = take.left().iter().position(|&s| s > 0.0).unwrap();
        // The sound is found a few frames in: the pre-roll comes before that
        assert!((pre_roll - 8..=pre_roll).contains(&first), "{first}");
        assert_eq!(take.frames(), first + (40000 - 20000));
    }

    #[test]
    fn test_the_rec_jack_records_while_high_and_the_button_steps_aside() {
        let mut rig = Rig::new(2.0);
        rig.input = Box::new(played);
        rig.gate = Some(vec![(1000, 25000)]);
        rig.press(BLOCK);
        assert_eq!(rig.state(), RecState::Idle, "the button does nothing while the jack is patched");
        rig.run_to(25600);
        let (_, take) = rig.take();
        assert_played(&take, 1000, 24000);
    }

    #[test]
    fn test_recording_stops_when_the_room_is_full() {
        let mut rig = Rig::new(0.5);
        rig.input = Box::new(played);
        rig.press(BLOCK);
        rig.run_to(48000);
        let (_, take) = rig.take();
        assert_eq!(take.frames(), take.room());
        assert!(take.frames() >= 24000);
        assert_eq!(rig.state(), RecState::Idle);
    }

    #[test]
    fn test_without_room_or_a_cable_rec_does_nothing() {
        let mut rig = Rig::new(1.0);
        rig.input = Box::new(played);
        rig.press(BLOCK);
        rig.press(4800);
        rig.run_to(9600);
        rig.take();
        // The room went with the take: nothing more until there's more
        rig.press(9600);
        assert_eq!(rig.state(), RecState::Idle);
        assert!(rig.sampler.take_finished_take().is_none());

        // Unpatched, a Sampler lets its room go, to be dropped off the audio thread
        let mut sampler = Sampler::new();
        sampler.prepare(SR, BLOCK);
        let room = Arc::new(SampleData::with_room(4800, SR));
        assert!(sampler.give_room(Arc::clone(&room)).is_some(), "not while anything else holds it");
        assert!(sampler.give_room(room).is_none());
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio), SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let params: Vec<f32> = sampler.parameters().iter().map(|p| p.default).collect();
        sampler.process(&[&unpatched; 9], &mut outputs, &params, &ProcessContext::new(SR, BLOCK));
        assert!(sampler.take_retired_sample().is_some(), "let go");
        assert_eq!(sampler.readout(&params).unwrap().values[Sampler::READOUT_ROOM], 0.0);

        // Room for another rate isn't taken
        assert!(sampler.give_room(Arc::new(SampleData::with_room(4800, 44100.0))).is_some());
    }

    #[test]
    fn test_a_take_plays_at_once_and_settles_without_a_break() {
        let mut rig = Rig::new(1.0);
        rig.input = Box::new(|t| 0.25 + 0.25 * (t as f32 * 0.01).sin());
        rig.press(BLOCK);
        rig.press(24000);
        rig.run_to(24064);
        let (count, take) = rig.take();

        // Played straight away, looping so it's still playing when settled
        rig.params[Sampler::PARAM_LOOP] = 1.0;
        rig.params[Sampler::PARAM_MODE] = 1.0;
        let played_note = play_note(&mut rig, 9600);
        assert!(played_note[4800..].iter().any(|&s| s > 0.2), "it plays");

        // Made to measure, the same sound: no break, and the room comes back
        let measured = Arc::new(take.to_measure());
        assert!(rig.sampler.settle_take(count + 1, Arc::clone(&measured)).is_some(), "another take's is refused");
        let back = rig.sampler.settle_take(count, measured).expect("the room comes back");
        assert!(Arc::ptr_eq(&back, &take));
        let mut heard = played_note;
        heard.extend(play_note(&mut rig, 9600));
        let steps = heard[4800..].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(steps < 0.01, "{steps}");
        assert!(rig.sampler.take_retired_sample().is_none(), "nothing faded out");
    }

    /// Holds the Sampler's Gate high for `frames`, from where the rig is.
    fn play_note(rig: &mut Rig, frames: usize) -> Vec<f32> {
        let unpatched = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut gate = SignalBuffer::gate(BLOCK);
        gate.fill(1.0);
        let mut outputs = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio), SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let mut heard = Vec::new();
        for _ in 0..frames / BLOCK {
            let inputs = [&gate, &unpatched, &unpatched, &unpatched, &unpatched, &unpatched, &unpatched, &unpatched, &unpatched];
            rig.sampler.process(&inputs, &mut outputs, &rig.params, &ProcessContext::new(SR, BLOCK));
            heard.extend_from_slice(&outputs[0].samples);
        }
        heard
    }

    #[test]
    fn test_the_transport_stopping_ends_a_take_and_a_new_rate_lets_go_of_the_room() {
        let mut rig = Rig::new(1.0);
        rig.input = Box::new(played);
        rig.press(BLOCK);
        rig.run_to(9600);
        rig.sampler.reset();
        let (_, take) = rig.take();
        assert_played(&take, BLOCK, 9600 - BLOCK);

        let mut rig = Rig::new(1.0);
        rig.sampler.prepare(44100.0, BLOCK);
        assert_eq!(rig.readout(Sampler::READOUT_ROOM), 0.0);
    }

    #[test]
    fn test_the_overview_grows_with_the_take() {
        let mut rig = Rig::new(10.0);
        rig.input = Box::new(|t| if t < 48000 { 0.5 } else { -0.25 });
        rig.press(BLOCK);
        rig.run_to(5 * 48000);
        // Past four seconds, it spans eight
        assert_eq!(rig.readout(Sampler::READOUT_REC_SCALE), 8.0);
        assert!((rig.readout(Sampler::READOUT_REC_SECONDS) - 5.0).abs() < 0.01);
        assert_eq!(rig.readout(Sampler::READOUT_REC_BEAT), 0.0, "a free take is counted in seconds");
        let (highs, lows) = rig.sampler.recorder.take_overview().map(|(h, l)| (h.to_vec(), l.to_vec())).expect("an overview to show");
        // The first second is the first eighth
        assert_eq!(highs[0], 0.5);
        assert_eq!(lows[TAKE_COLUMNS / 8 + 1], -0.25);
        assert_eq!(highs[TAKE_COLUMNS - 1], 0.0, "not yet recorded");
    }
}
