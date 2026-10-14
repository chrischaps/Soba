//! What the oscillator's display draws live: short captures of its own Out.
//!
//! A capture starts where the tuned pitch starts a cycle with the Sub high,
//! or on a hard-sync reset that does the same, so each lines up with the
//! shape drawn on the face, and the Sub's octave with it. It spans
//! [`CYCLES`] cycles of the pitch the knobs set, whatever V/Oct is doing,
//! so a note an octave up fits twice as many cycles in. The output is
//! sounded a few samples after the phase that made it, the BLEP and the
//! downsampler's delay, and the capture waits that long for each point.
//!
//! At most [`PER_SECOND`] captures are taken a second, and none while the
//! last is still waiting to be taken.

use crate::modules::oscilloscope::SCOPE_BUFFER_SIZE;

/// Points in a capture.
pub const POINTS: usize = SCOPE_BUFFER_SIZE;

/// Cycles of the knobs' pitch a capture spans: two, for the Sub's octave.
pub const CYCLES: f32 = 2.0;

/// Captures a second, at most.
const PER_SECOND: f32 = 30.0;

/// Output samples from the phase making a sound to the output sounding
/// it, at low pitches: one for the BLEP, the rest the downsampler's group
/// delay, measured by the oscillator's `the_trace_lines_up_with_the_shape`.
const LATENCY: f32 = 2.78;

pub(super) struct Trace {
    points: Box<[f32]>,
    /// Points taken in the capture under way, if there is one.
    taking: Option<usize>,
    /// Whether a finished capture waits to be taken.
    ready: bool,
    /// Output samples a point.
    stride: f32,
    /// How far into the capture the last sample sounded, in output samples.
    sounded: f32,
    last: f32,
    /// Output samples until another capture may start.
    rest: f32,
}

impl Trace {
    pub(super) fn new() -> Self {
        Self { points: vec![0.0; POINTS].into_boxed_slice(), taking: None, ready: false, stride: 1.0, sounded: 0.0, last: 0.0, rest: 0.0 }
    }

    /// A cycle began, `ago` output samples before the end of this sample, at
    /// `hz` set by the knobs. Starts a capture if one is due.
    ///
    /// REAL-TIME SAFE.
    #[inline]
    pub(super) fn cycle(&mut self, ago: f32, hz: f32, sample_rate: f32) {
        if self.taking.is_some() || self.ready || self.rest > 0.0 {
            return;
        }
        self.taking = Some(0);
        self.stride = CYCLES * sample_rate / (hz.abs().max(1.0) * POINTS as f32);
        // This sample sounds what was played `LATENCY` before the end of it
        self.sounded = ago - LATENCY;
        self.rest = sample_rate / PER_SECOND;
    }

    /// Takes the next output sample, after [`cycle`](Self::cycle).
    ///
    /// REAL-TIME SAFE.
    #[inline]
    pub(super) fn push(&mut self, sample: f32) {
        self.rest -= 1.0;
        if let Some(mut taken) = self.taking {
            // Every point due between the last sample and this one
            while taken < POINTS {
                let at = taken as f32 * self.stride;
                if at > self.sounded {
                    break;
                }
                let between = (at - (self.sounded - 1.0)).clamp(0.0, 1.0);
                self.points[taken] = self.last + (sample - self.last) * between;
                taken += 1;
            }
            if taken == POINTS {
                self.taking = None;
                self.ready = true;
            } else {
                self.taking = Some(taken);
            }
        }
        self.last = sample;
        self.sounded += 1.0;
    }

    /// The finished capture, once.
    pub(super) fn take(&mut self) -> Option<&[f32]> {
        std::mem::take(&mut self.ready).then_some(&self.points[..])
    }

    /// Drops any capture under way or waiting.
    pub(super) fn reset(&mut self) {
        self.taking = None;
        self.ready = false;
        self.rest = 0.0;
        self.last = 0.0;
    }
}
