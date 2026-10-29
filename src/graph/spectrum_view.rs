//! What the Spectrum module's face draws, worked out from its tap.
//!
//! Each frame, every Spectrum on screen gives up the newest window of its
//! two inputs. Both go through one FFT, as a filter's input and output do
//! (see [`super::filter_spectrum`]), padded to twice the window so a
//! partial's peak comes out round instead of cut square at a bin. The
//! window is a Blackman-Harris rather than the filters' Hann: a partial is
//! a little wider for it, but its skirts sink below anything the face
//! draws, so nothing stands between the harmonics that isn't there. The bins
//! are gathered into narrow columns on the 20 Hz to 20 kHz axis, each the
//! loudest bin in it, so a harmonic stands as a line rather than spreading
//! across a band. A column reads as a sine's amplitude: a full-scale sine
//! stands at 0 dB, and a saw's harmonics step down by their number.
//!
//! Each column rises at once and sinks smoothly, and a second line holds
//! every peak for the Hold time before letting it fall, so a pluck's
//! harmonics stay long enough to read. The strongest partial, and below it
//! any prominent peak it's a whole multiple of, name the fundamental: the
//! note whose harmonics the face numbers. Noise has no peak standing far
//! enough above its neighbours, so it names none.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::spectrum_tap::{SpectrumTap, WINDOW};
use crate::engine::NodeId as EngineNodeId;

use super::filter_spectrum::{between_bins, Fft, HIGHEST_HZ, LOWEST_HZ};

/// Columns across the axis: a sixtieth of an octave each.
pub const COLUMNS: usize = 600;

/// What a column reads when nothing is there, in dB.
pub const SILENCE_DB: f32 = -150.0;

/// The FFT's length: the window, padded with as much silence again.
const LENGTH: usize = 2 * WINDOW;

/// How fast a column rises to a louder reading and settles to a quieter
/// one, as time constants in seconds, and the fastest it falls, in dB a
/// second, so a sound that stops drains away instead of vanishing.
const RISE_SECONDS: f32 = 0.01;
const FALL_SECONDS: f32 = 0.12;
const FALL_DB_PER_SECOND: f32 = 60.0;

/// How fast a held peak falls once its Hold is up, in dB a second.
const HELD_FALL_DB_PER_SECOND: f32 = 20.0;

/// Below this a trace has nothing to show, in dB; it's fully there 20 dB up.
const QUIET_DB: f32 = -100.0;

/// The partials a fundamental is looked for among, in Hz.
const LOWEST_FUNDAMENTAL_HZ: f32 = 25.0;
const HIGHEST_PARTIAL_HZ: f32 = 8_000.0;

/// The quietest partial that names a note, in dB.
const PITCH_FLOOR_DB: f32 = -80.0;

/// How far a partial must stand above the lowest point either side of it,
/// within [`PROMINENCE_REACH_HZ`], to count as one rather than noise.
const PROMINENCE_DB: f32 = 15.0;
const PROMINENCE_REACH_HZ: f32 = 70.0;

/// How much quieter than the strongest partial the fundamental may be, in
/// dB, and the highest harmonic the strongest may be of it.
const FUNDAMENTAL_REACH_DB: f32 = 45.0;
const HIGHEST_DIVISOR: usize = 8;

/// How the named note follows the sound: a slide of less than this, in
/// octaves, is followed smoothly over [`GLIDE_SECONDS`]; a jump further is
/// taken once the new note has lasted [`SETTLE_SECONDS`].
const SAME_NOTE_OCTAVES: f32 = 0.6 / 12.0;
const GLIDE_SECONDS: f32 = 0.04;
const SETTLE_SECONDS: f32 = 0.08;

/// How long a note must last, heard from nothing, before it's named: long
/// enough that noise, whose loudest peak wanders, never is.
const FIRST_SETTLE_SECONDS: f32 = 0.15;

/// How fast the numbers appear when a note is heard, and fade once it has
/// gone for longer than the Hold, in seconds.
const APPEAR_SECONDS: f32 = 0.06;
const FADE_SECONDS: f32 = 0.5;

/// A column at silence, for when the tap has stopped.
const SILENT: [f32; COLUMNS] = [SILENCE_DB; COLUMNS];

/// How long the tap can go without anything new before its sound counts as
/// stopped, in seconds. Frames can come faster than the audio's blocks.
const STOPPED_SECONDS: f32 = 0.1;

/// Where on the axis a frequency falls, in columns from the left edge.
pub fn column_of(hz: f32) -> f32 {
    (hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln() * COLUMNS as f32
}

/// The frequency `column` columns from the left edge.
pub fn hz_at(column: f32) -> f32 {
    LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(column / COLUMNS as f32)
}

/// One input's spectrum, in dB, column by column from the bottom of the axis.
pub struct Trace {
    /// What it's playing now.
    pub live: Box<[f32; COLUMNS]>,
    /// Each column's peak, held and then falling.
    pub held: Box<[f32; COLUMNS]>,
    /// How long since each held peak was last pushed up, in seconds.
    held_for: Box<[f32; COLUMNS]>,
    /// How much there is to see, 0 (silence) to 1.
    pub presence: f32,
    /// The note it's playing.
    pub pitch: Pitch,
}

impl Trace {
    fn new() -> Self {
        Self {
            live: Box::new(SILENT),
            held: Box::new(SILENT),
            held_for: Box::new([0.0; COLUMNS]),
            presence: 0.0,
            pitch: Pitch::default(),
        }
    }

    /// Moves on by `dt` seconds toward a fresh reading of `columns`, with
    /// `heard` its fundamental, holding peaks for `hold` seconds.
    fn advance(&mut self, columns: &[f32; COLUMNS], heard: Option<f32>, hold: f32, dt: f32) {
        let rise = 1.0 - (-dt / RISE_SECONDS).exp();
        let fall = 1.0 - (-dt / FALL_SECONDS).exp();
        let most = FALL_DB_PER_SECOND * dt;
        let mut loudest = SILENCE_DB;
        for column in 0..COLUMNS {
            let (target, live) = (columns[column], &mut self.live[column]);
            if target > *live {
                *live += (target - *live) * rise;
            } else {
                *live -= ((*live - target) * fall).min(most);
            }
            let held = &mut self.held[column];
            if *live >= *held {
                *held = *live;
                self.held_for[column] = 0.0;
            } else {
                self.held_for[column] += dt;
                if self.held_for[column] > hold {
                    *held = (*held - HELD_FALL_DB_PER_SECOND * dt).max(*live);
                }
            }
            loudest = loudest.max(*held);
        }
        self.presence = ((loudest - QUIET_DB) / 20.0).clamp(0.0, 1.0);
        self.pitch.follow(heard, hold, dt);
    }

    /// The level at `hz`: the loudest of the held columns around it.
    pub fn held_at(&self, hz: f32) -> f32 {
        let at = column_of(hz).round() as isize;
        (at - 1..=at + 1)
            .filter_map(|column| usize::try_from(column).ok())
            .filter_map(|column| self.held.get(column))
            .fold(SILENCE_DB, |loudest, &db| loudest.max(db))
    }
}

/// The note a trace is playing, as the face names it: steady while the
/// sound wavers, following a slide, taking a new note once it's lasted.
#[derive(Default)]
pub struct Pitch {
    /// The fundamental, in Hz, once one has been heard.
    hz: Option<f32>,
    /// How surely it's shown, 0 to 1: up while heard, fading once gone.
    shown: f32,
    /// How long since it was last heard, in seconds.
    since: f32,
    /// A different note heard lately, and for how long.
    pending: Option<(f32, f32)>,
}

impl Pitch {
    /// The fundamental and how surely it's shown, while it's shown at all.
    pub fn fundamental(&self) -> Option<(f32, f32)> {
        self.hz.filter(|_| self.shown > 0.0).map(|hz| (hz, self.shown))
    }

    fn follow(&mut self, heard: Option<f32>, hold: f32, dt: f32) {
        let Some(hz) = heard else {
            self.since += dt;
            self.pending = None;
            if self.since > hold {
                self.shown = (self.shown - dt / FADE_SECONDS).max(0.0);
            }
            return;
        };
        self.since = 0.0;
        match self.hz {
            Some(shown) if self.shown > 0.05 => {
                let apart = (hz / shown).log2();
                if apart.abs() < SAME_NOTE_OCTAVES {
                    self.hz = Some(shown * (apart * (1.0 - (-dt / GLIDE_SECONDS).exp())).exp2());
                    self.pending = None;
                } else {
                    let lasted = self.lasted(hz, dt);
                    if lasted >= SETTLE_SECONDS {
                        self.hz = Some(hz);
                        self.pending = None;
                    } else {
                        self.pending = Some((hz, lasted));
                    }
                }
            }
            _ => {
                // A note from nothing, once it has lasted
                let lasted = self.lasted(hz, dt);
                if lasted < FIRST_SETTLE_SECONDS {
                    self.pending = Some((hz, lasted));
                    self.shown = (self.shown - dt / FADE_SECONDS).max(0.0);
                    return;
                }
                self.hz = Some(hz);
                self.pending = None;
            }
        }
        self.shown += (1.0 - self.shown) * (1.0 - (-dt / APPEAR_SECONDS).exp());
    }

    /// How long `hz` has been heard as the pending note, this frame included.
    fn lasted(&self, hz: f32, dt: f32) -> f32 {
        match self.pending {
            Some((pending, lasted)) if (hz / pending).log2().abs() < SAME_NOTE_OCTAVES => lasted + dt,
            _ => 0.0,
        }
    }
}

/// One Spectrum's two traces: In 1, then In 2.
pub struct SpectrumView {
    pub traces: [Trace; 2],
    /// How many samples the tap had written when last read, and how long
    /// ago that changed, in seconds.
    written: usize,
    stale: f32,
}

/// Every Spectrum's view, and what it takes to work them out.
#[derive(Default)]
pub struct SpectrumViews {
    views: HashMap<EngineNodeId, SpectrumView>,
    /// The Spectrums drawn since the last update, with their Hold. Only
    /// these are analysed, so the filters' taps cost nothing here.
    asked: RefCell<HashMap<EngineNodeId, f32>>,
    analyser: Option<Analyser>,
}

impl SpectrumViews {
    /// A Spectrum's view, if it has been heard.
    pub fn get(&self, node_id: EngineNodeId) -> Option<&SpectrumView> {
        self.views.get(&node_id)
    }

    /// Notes that a Spectrum is on screen, holding peaks for `hold` seconds,
    /// so it's kept up to date.
    pub fn ask(&self, node_id: EngineNodeId, hold: f32) {
        self.asked.borrow_mut().insert(node_id, hold);
    }

    /// Reads the tap of each Spectrum drawn since last time and moves its
    /// view on by `dt` seconds. Those gone from the graph are forgotten.
    pub fn listen<'a>(&mut self, taps: impl Iterator<Item = (EngineNodeId, &'a Arc<SpectrumTap>)>, dt: f32) {
        let asked = std::mem::take(&mut *self.asked.borrow_mut());
        let mut heard = Vec::new();
        for (node_id, tap) in taps {
            heard.push(node_id);
            let Some(&hold) = asked.get(&node_id) else { continue };
            let view = self.views.entry(node_id).or_insert_with(|| SpectrumView { traces: [Trace::new(), Trace::new()], written: 0, stale: 0.0 });
            let analyser = self.analyser.get_or_insert_with(Analyser::new);
            if analyser.read(tap, &mut view.written) {
                view.stale = 0.0;
                for (index, trace) in view.traces.iter_mut().enumerate() {
                    trace.advance(&analyser.columns[index], analyser.fundamentals[index], hold, dt);
                }
            } else {
                // Nothing new for a while, as when the audio has stopped: let it drain
                view.stale += dt;
                if view.stale > STOPPED_SECONDS {
                    for trace in &mut view.traces {
                        trace.advance(&SILENT, None, hold, dt);
                    }
                }
            }
        }
        self.views.retain(|node_id, _| heard.contains(node_id));
    }

    pub fn clear(&mut self) {
        self.views.clear();
    }
}

/// Where a column's level is read from, in bins.
#[derive(Clone, Copy)]
enum Column {
    /// The loudest of the bins whose centres lie in the column.
    Loudest(usize, usize),
    /// A column narrower than a bin, read between the two bins around its
    /// centre: this far from the first toward the second.
    Between(usize, f32),
}

/// The FFT and its scratch room, shared by every Spectrum.
struct Analyser {
    fft: Fft,
    window: Vec<f32>,
    /// Turns a bin's power into a sine's amplitude squared.
    scale: f32,
    in1: Box<[f32; WINDOW]>,
    in2: Box<[f32; WINDOW]>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Each input's bins, as a sine's amplitude squared.
    power: [Vec<f32>; 2],
    /// One input's bins in dB, for finding its note.
    db: Vec<f32>,
    /// The last reading: each input's columns in dB, and its fundamental.
    columns: [Box<[f32; COLUMNS]>; 2],
    fundamentals: [Option<f32>; 2],
    /// The bins each column reads, for the sample rate they were laid out at.
    layout: Vec<Column>,
    sample_rate: f32,
}

impl Analyser {
    fn new() -> Self {
        // Blackman-Harris: its skirts fall below -92 dB, so the floor
        // between partials, and under the fundamental, is clean
        let window: Vec<f32> = (0..WINDOW)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / WINDOW as f32;
                0.35875 - 0.48829 * phase.cos() + 0.14128 * (2.0 * phase).cos() - 0.01168 * (3.0 * phase).cos()
            })
            .collect();
        let sum: f32 = window.iter().sum();
        let bins = LENGTH / 2 + 1;
        Self {
            fft: Fft::new(LENGTH),
            scale: 4.0 / (sum * sum),
            window,
            in1: Box::new([0.0; WINDOW]),
            in2: Box::new([0.0; WINDOW]),
            re: vec![0.0; LENGTH],
            im: vec![0.0; LENGTH],
            power: [vec![0.0; bins], vec![0.0; bins]],
            db: vec![0.0; bins],
            columns: [Box::new(SILENT), Box::new(SILENT)],
            fundamentals: [None; 2],
            layout: Vec::new(),
            sample_rate: 0.0,
        }
    }

    /// Reads `tap` into `columns` and `fundamentals`, if it has anything
    /// newer than `written`, which it updates. Returns whether it had.
    fn read(&mut self, tap: &SpectrumTap, written: &mut usize) -> bool {
        let now = tap.read(&mut self.in1, &mut self.in2);
        if now == *written {
            return false;
        }
        *written = now;
        let sample_rate = tap.sample_rate();
        if sample_rate != self.sample_rate {
            self.layout = lay_out_columns(sample_rate);
            self.sample_rate = sample_rate;
        }

        // In 1 as the real part, In 2 as the imaginary, padded with silence
        for n in 0..WINDOW {
            self.re[n] = self.in1[n] * self.window[n];
            self.im[n] = self.in2[n] * self.window[n];
        }
        self.re[WINDOW..].fill(0.0);
        self.im[WINDOW..].fill(0.0);
        self.fft.transform(&mut self.re, &mut self.im);

        // Unpacked, bin by bin: each input's power
        for k in 0..=LENGTH / 2 {
            let (a, b) = (self.re[k], self.im[k]);
            let mirror = (LENGTH - k) % LENGTH;
            let (c, d) = (self.re[mirror], self.im[mirror]);
            self.power[0][k] = ((a + c) * (a + c) + (b - d) * (b - d)) * 0.25 * self.scale;
            self.power[1][k] = ((b + d) * (b + d) + (a - c) * (a - c)) * 0.25 * self.scale;
        }

        let bin_hz = sample_rate / LENGTH as f32;
        for index in 0..2 {
            column_levels(&self.power[index], &self.layout, &mut self.columns[index]);
            self.fundamentals[index] = fundamental(&self.power[index], bin_hz, &mut self.db);
        }
        true
    }
}

/// Which bins each column reads, at `sample_rate`.
fn lay_out_columns(sample_rate: f32) -> Vec<Column> {
    let bin_hz = sample_rate / LENGTH as f32;
    let top_bin = LENGTH / 2;
    (0..COLUMNS)
        .map(|column| {
            let first = (hz_at(column as f32) / bin_hz).ceil() as usize;
            let past = ((hz_at(column as f32 + 1.0) / bin_hz).ceil() as usize).min(top_bin + 1);
            if first < past {
                Column::Loudest(first, past)
            } else {
                let at = (hz_at(column as f32 + 0.5) / bin_hz).min(top_bin as f32 - 1.0);
                Column::Between(at.floor() as usize, at.fract())
            }
        })
        .collect()
}

/// Each column's level in dB, as the amplitude of a sine.
fn column_levels(power: &[f32], layout: &[Column], levels: &mut [f32; COLUMNS]) {
    for (level, column) in levels.iter_mut().zip(layout) {
        let power = match *column {
            Column::Loudest(first, past) => power[first..past].iter().copied().fold(0.0, f32::max),
            Column::Between(below, t) => between_bins(power, below, t),
        };
        *level = (10.0 * power.max(1e-16).log10()).max(SILENCE_DB);
    }
}

/// The fundamental of what `power` holds, if it holds a note: the strongest
/// partial, or the lowest prominent peak it's a whole multiple of.
fn fundamental(power: &[f32], bin_hz: f32, db: &mut [f32]) -> Option<f32> {
    for (db, &power) in db.iter_mut().zip(power) {
        *db = 10.0 * power.max(1e-16).log10();
    }
    let db = &*db;
    let reach = (PROMINENCE_REACH_HZ / bin_hz).round().max(2.0) as usize;
    let lowest = ((LOWEST_FUNDAMENTAL_HZ / bin_hz).ceil() as usize).max(1);
    let highest = ((HIGHEST_PARTIAL_HZ / bin_hz) as usize).min(db.len() - 2);
    let loudest = |from: usize, to: usize| (from..=to).max_by(|&a, &b| db[a].total_cmp(&db[b]));

    let strongest = loudest(lowest, highest)?;
    if db[strongest] < PITCH_FLOOR_DB || prominence(db, strongest, reach) < PROMINENCE_DB {
        return None;
    }
    let top = peak_hz(db, strongest, bin_hz);
    for divisor in (2..=HIGHEST_DIVISOR).rev() {
        let centre = top / divisor as f32 / bin_hz;
        if centre * bin_hz < LOWEST_FUNDAMENTAL_HZ {
            continue;
        }
        let spread = (centre * 0.03).max(2.0);
        let from = ((centre - spread).floor() as usize).max(1);
        let to = ((centre + spread).ceil() as usize).min(db.len() - 2);
        let Some(bin) = loudest(from, to) else { continue };
        let is_peak = db[bin] >= db[bin - 1] && db[bin] >= db[bin + 1];
        if is_peak && db[bin] > db[strongest] - FUNDAMENTAL_REACH_DB && prominence(db, bin, reach) >= PROMINENCE_DB {
            return Some(peak_hz(db, bin, bin_hz));
        }
    }
    Some(top)
}

/// How far bin `at` stands above the lowest point on its higher side,
/// within `reach` bins: a partial stands well clear, noise's peaks don't.
fn prominence(db: &[f32], at: usize, reach: usize) -> f32 {
    let lowest = |bins: &[f32]| bins.iter().copied().fold(f32::MAX, f32::min);
    let left = lowest(&db[at.saturating_sub(reach)..at]);
    let right = lowest(&db[at + 1..(at + reach + 1).min(db.len())]);
    db[at] - left.max(right)
}

/// Where the peak at bin `at` really is, in Hz: the top of a parabola
/// through it and its neighbours, in dB.
fn peak_hz(db: &[f32], at: usize, bin_hz: f32) -> f32 {
    let (a, b, c) = (db[at - 1], db[at], db[at + 1]);
    let curve = a - 2.0 * b + c;
    let offset = if curve < 0.0 { (0.5 * (a - c) / curve).clamp(-0.5, 0.5) } else { 0.0 };
    (at as f32 + offset) * bin_hz
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::SignalBuffer;

    const SR: f32 = 48_000.0;
    const DT: f32 = 1.0 / 60.0;

    /// Partials as (Hz, amplitude), summed.
    fn partials(partials: Vec<(f32, f32)>) -> impl Fn(usize) -> f32 {
        move |n| partials.iter().map(|&(hz, a)| a * (std::f32::consts::TAU * hz * n as f32 / SR).sin()).sum()
    }

    /// A saw's first `count` harmonics of `hz`: harmonic n at 1/n.
    fn saw(hz: f32, count: usize) -> impl Fn(usize) -> f32 {
        partials((1..=count).map(|n| (hz * n as f32, 0.5 / n as f32)).collect())
    }

    /// Writes a window more of `in1` and `in2` to `tap`, starting `at`.
    fn feed(tap: &SpectrumTap, in1: &impl Fn(usize) -> f32, in2: &impl Fn(usize) -> f32, at: usize) {
        let (mut a, mut b) = (SignalBuffer::audio(WINDOW), SignalBuffer::audio(WINDOW));
        for n in 0..WINDOW {
            a.channel_mut(0)[n] = in1(at + n);
            b.channel_mut(0)[n] = in2(at + n);
        }
        tap.write(&a, &b, SR);
    }

    /// One reading of `in1` and `in2`.
    fn analyse(in1: impl Fn(usize) -> f32, in2: impl Fn(usize) -> f32) -> Analyser {
        let tap = SpectrumTap::new();
        feed(&tap, &in1, &in2, 0);
        let mut analyser = Analyser::new();
        assert!(analyser.read(&tap, &mut 0));
        analyser
    }

    /// A column's level near `hz`: the loudest within a column of it.
    fn level_at(columns: &[f32; COLUMNS], hz: f32) -> f32 {
        let at = column_of(hz).round() as usize;
        columns[at - 1..=at + 1].iter().copied().fold(SILENCE_DB, f32::max)
    }

    #[test]
    fn a_sine_stands_at_its_amplitude_alone() {
        let analyser = analyse(partials(vec![(440.0, 0.5)]), |_| 0.0);
        let db = level_at(&analyser.columns[0], 440.0);
        assert!((db - -6.0).abs() < 1.0, "half scale reads -6 dB, not {db:.1}");
        assert!(level_at(&analyser.columns[0], 880.0) < -60.0, "and nothing an octave up");
        assert!(level_at(&analyser.columns[0], 220.0) < -60.0, "or an octave down");
    }

    #[test]
    fn the_two_inputs_come_apart_from_one_fft() {
        let analyser = analyse(partials(vec![(220.0, 0.5)]), partials(vec![(3_000.0, 0.25)]));
        assert!(level_at(&analyser.columns[0], 220.0) > -7.5);
        assert!(level_at(&analyser.columns[0], 3_000.0) < -70.0, "In 2's tone stays out of In 1");
        assert!((level_at(&analyser.columns[1], 3_000.0) - -12.0).abs() < 1.0);
        assert!(level_at(&analyser.columns[1], 220.0) < -70.0, "In 1's tone stays out of In 2");
    }

    #[test]
    fn a_saws_harmonics_step_down_by_their_number() {
        let analyser = analyse(saw(110.0, 40), |_| 0.0);
        let first = level_at(&analyser.columns[0], 110.0);
        for n in [2.0f32, 3.0, 5.0, 8.0, 13.0] {
            let expected = first - 20.0 * n.log10();
            let db = level_at(&analyser.columns[0], 110.0 * n);
            assert!((db - expected).abs() < 1.5, "harmonic {n} at {db:.1} dB, not {expected:.1}");
        }
        // Between harmonics, the floor falls well away: each is a line
        assert!(level_at(&analyser.columns[0], 110.0 * 4.5) < first - 40.0);
    }

    #[test]
    fn a_saw_names_its_note() {
        let analyser = analyse(saw(110.0, 40), |_| 0.0);
        let hz = analyser.fundamentals[0].expect("a note");
        assert!((hz / 110.0 - 1.0).abs() < 0.005, "110 Hz, not {hz:.1}");
        assert_eq!(analyser.fundamentals[1], None, "nothing in In 2");
    }

    #[test]
    fn a_quiet_fundamental_still_names_the_note() {
        // Filtered from below: the second harmonic is the loudest
        let sound = partials(vec![(110.0, 0.02), (220.0, 0.5), (330.0, 0.3), (440.0, 0.2), (550.0, 0.1)]);
        let hz = analyse(sound, |_| 0.0).fundamentals[0].expect("a note");
        assert!((hz / 110.0 - 1.0).abs() < 0.005, "110 Hz, not {hz:.1}");
    }

    #[test]
    fn a_low_note_names_itself() {
        let hz = analyse(saw(41.2, 60), |_| 0.0).fundamentals[0].expect("E1");
        assert!((hz / 41.2 - 1.0).abs() < 0.01, "41.2 Hz, not {hz:.1}");
    }

    #[test]
    fn noise_names_no_note() {
        let mut state = 0x2545_f491u32;
        let noise: Vec<f32> = (0..WINDOW * 40)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        let analyser = analyse(|n| noise[n], |_| 0.0);
        assert!(level_at(&analyser.columns[0], 1_000.0) > -60.0, "it's there to see");

        // Read a sixtieth of a second apart, as the face reads it, windows
        // overlapping: its loudest peak never stays put long enough to name
        let tap = Arc::new(SpectrumTap::new());
        let mut views = SpectrumViews::default();
        let step = SR as usize / 60;
        for frame in 0..(noise.len() - WINDOW) / step {
            feed(&tap, &|n| noise[n], &|_| 0.0, frame * step);
            views.ask(1, 1.0);
            views.listen(std::iter::once((1, &tap)), DT);
            assert_eq!(views.get(1).unwrap().traces[0].pitch.fundamental(), None, "frame {frame}");
        }
    }

    #[test]
    fn peaks_are_held_then_fall_while_the_sound_drains_away() {
        let tap = Arc::new(SpectrumTap::new());
        let tone = partials(vec![(1_000.0, 0.5)]);
        let mut views = SpectrumViews::default();
        let frame = |views: &mut SpectrumViews| {
            views.ask(9, 1.0);
            views.listen(std::iter::once((9, &tap)), DT);
        };
        for i in 0..20 {
            feed(&tap, &tone, &|_| 0.0, i * WINDOW);
            frame(&mut views);
        }
        let trace = &views.get(9).unwrap().traces[0];
        assert!((trace.held_at(1_000.0) - -6.0).abs() < 1.0);
        assert!(trace.pitch.fundamental().is_some_and(|(hz, shown)| (hz - 1_000.0).abs() < 5.0 && shown > 0.9));
        assert_eq!(trace.presence, 1.0);

        // The tone stops: half a second on, it's draining but the peak holds
        for _ in 0..30 {
            frame(&mut views);
        }
        let trace = &views.get(9).unwrap().traces[0];
        assert!(level_at(&trace.live, 1_000.0) < -30.0, "the sound drains: {:.1}", level_at(&trace.live, 1_000.0));
        assert!((trace.held_at(1_000.0) - -6.0).abs() < 1.0, "its peak is held");
        assert!(trace.pitch.fundamental().is_some(), "and so is its note");

        // A second on, the peak is falling; seven on, it's all gone
        for _ in 0..60 {
            frame(&mut views);
        }
        let held = views.get(9).unwrap().traces[0].held_at(1_000.0);
        assert!(held < -10.0 && held > -40.0, "falling: {held:.1}");
        for _ in 0..360 {
            frame(&mut views);
        }
        let trace = &views.get(9).unwrap().traces[0];
        assert_eq!(trace.presence, 0.0);
        assert_eq!(trace.pitch.fundamental(), None);
    }

    #[test]
    fn a_frame_between_blocks_changes_nothing() {
        let tap = Arc::new(SpectrumTap::new());
        let tone = saw(220.0, 20);
        let mut views = SpectrumViews::default();
        for i in 0..12 {
            feed(&tap, &tone, &|_| 0.0, i * WINDOW);
            views.ask(2, 1.0);
            views.listen(std::iter::once((2, &tap)), DT);
            // Drawn again before the next block comes
            views.ask(2, 1.0);
            views.listen(std::iter::once((2, &tap)), DT / 2.0);
        }
        let trace = &views.get(2).unwrap().traces[0];
        assert!(trace.pitch.fundamental().is_some_and(|(hz, _)| (hz - 220.0).abs() < 1.0), "the note is named");
        assert!((level_at(&trace.live, 220.0) - trace.held_at(220.0)).abs() < 0.5, "and nothing has sunk");
    }

    #[test]
    fn only_spectrums_drawn_are_analysed_and_gone_ones_forgotten() {
        let tap = Arc::new(SpectrumTap::new());
        feed(&tap, &partials(vec![(500.0, 0.5)]), &|_| 0.0, 0);
        let mut views = SpectrumViews::default();
        // Never drawn (a filter's tap, say): nothing kept for it
        views.listen(std::iter::once((4, &tap)), DT);
        assert!(views.get(4).is_none());
        views.ask(4, 1.0);
        views.listen(std::iter::once((4, &tap)), DT);
        assert!(views.get(4).unwrap().traces[0].presence > 0.0);
        // Gone from the graph: forgotten
        views.listen(std::iter::empty(), DT);
        assert!(views.get(4).is_none());
    }

    #[test]
    fn the_note_glides_with_a_slide_and_jumps_once_a_new_one_lasts() {
        let mut pitch = Pitch::default();
        // Heard from nothing, it's named once it has lasted
        for _ in 0..8 {
            pitch.follow(Some(220.0), 1.0, DT);
        }
        assert_eq!(pitch.fundamental(), None);
        for _ in 0..4 {
            pitch.follow(Some(220.0), 1.0, DT);
        }
        // A wobble a few cents off is followed smoothly
        pitch.follow(Some(222.0), 1.0, DT);
        let (hz, _) = pitch.fundamental().unwrap();
        assert!(hz > 220.0 && hz < 222.0, "{hz}");
        // A new note: not on the first frame, but once it's lasted
        pitch.follow(Some(330.0), 1.0, DT);
        assert!(pitch.fundamental().unwrap().0 < 225.0);
        for _ in 0..6 {
            pitch.follow(Some(330.0), 1.0, DT);
        }
        assert_eq!(pitch.fundamental().unwrap().0, 330.0);
    }

    #[test]
    fn columns_and_frequencies_agree() {
        assert!((hz_at(column_of(440.0)) - 440.0).abs() < 0.01);
        assert_eq!(column_of(LOWEST_HZ), 0.0);
        assert!((column_of(HIGHEST_HZ) - COLUMNS as f32).abs() < 1e-3);
    }
}
