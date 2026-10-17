//! The Oscillator display's live trace and its afterglow.
//!
//! The oscillator sends a short capture of its Out a few dozen times a
//! second (see [`crate::modules::oscillator::TRACE_POINTS`]), each starting
//! where the drawn shape starts. The display keeps the last few a moment, so
//! each fades like a scope's phosphor rather than vanishing: a steady wave
//! glows as one line, and one being moved leaves a soft smear of where it's
//! been. When the captures stop, as when the audio does, the display holds
//! the last a moment and then settles back to the shape the knobs set.

use std::collections::VecDeque;
use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect};

use crate::modules::oscillator::{TRACE_CYCLES, TRACE_POINTS};

use super::state::ScopeData;

/// How long a capture glows on after it arrives, in seconds.
const AFTERGLOW: f64 = 0.22;

/// How long the display stays live after the last capture, and then how
/// long it takes to settle, in seconds.
const HOLD: f64 = 0.12;
const SETTLE: f64 = 0.4;

/// Louder than this, a trace is scaled to fit: a stack of unison voices.
const HEADROOM: f32 = 1.15;

/// One oscillator's recent captures, newest last, with when each came.
#[derive(Clone, Default)]
struct Glow {
    /// The captures received when it last looked.
    count: u64,
    traces: VecDeque<(f64, Arc<[f32]>)>,
}

/// What the display draws live.
pub struct Live {
    /// How live it is: 1 while captures come, falling to 0 once they stop.
    pub presence: f32,
    /// The captures still glowing, oldest first, each with how brightly.
    pub traces: Vec<(f32, Arc<[f32]>)>,
}

/// Takes in the node's newest capture, if it's new, and returns what's
/// glowing now.
pub fn live(ctx: &egui::Context, id: egui::Id, data: Option<&ScopeData>) -> Live {
    let now = ctx.input(|i| i.time);
    let glow = ctx.data_mut(|d| {
        let glow = d.get_temp_mut_or_default::<Glow>(id);
        if let Some(data) = data.filter(|data| data.count != glow.count && data.channel1.len() == TRACE_POINTS) {
            glow.count = data.count;
            glow.traces.push_back((now, data.channel1.as_slice().into()));
        }
        // The newest stays until another comes; older ones fade out
        while glow.traces.len() > 1 && now - glow.traces[0].0 > AFTERGLOW {
            glow.traces.pop_front();
        }
        glow.clone()
    });

    let Some(&(newest, _)) = glow.traces.back() else { return Live { presence: 0.0, traces: Vec::new() } };
    let presence = (1.0 - ((now - newest - HOLD) / SETTLE).clamp(0.0, 1.0)) as f32;
    let last = glow.traces.len() - 1;
    let traces = glow
        .traces
        .into_iter()
        .enumerate()
        .map(|(i, (at, trace))| {
            let fade = if i == last { 1.0 } else { (1.0 - (now - at) / AFTERGLOW).clamp(0.0, 1.0) as f32 };
            (fade * fade, trace)
        })
        .collect();
    Live { presence, traces }
}

/// A capture's first `cycles` cycles across `plot`. One louder than the
/// plot holds, as a stack of unison voices can be, is scaled to fit.
pub fn trace_points(trace: &[f32], plot: Rect, cycles: f32, y_at: impl Fn(f32) -> f32) -> Vec<Pos2> {
    let span = ((trace.len() as f32 * cycles / TRACE_CYCLES).round() as usize).clamp(1, trace.len());
    // One point past the span reaches the right edge, if there is one
    let shown = &trace[..(span + 1).min(trace.len())];
    let peak = shown.iter().fold(0.0f32, |most, v| most.max(v.abs()));
    let scale = if peak > HEADROOM { 1.0 / peak } else { 1.0 };
    shown
        .iter()
        .enumerate()
        .map(|(j, v)| Pos2::new(plot.left() + j as f32 / span as f32 * plot.width(), y_at(v * scale)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    fn plot() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 40.0))
    }

    fn y_at(v: f32) -> f32 {
        20.0 - v * 20.0
    }

    #[test]
    fn one_cycle_spans_the_plot_from_half_the_capture() {
        let trace: Vec<f32> = (0..TRACE_POINTS).map(|j| j as f32 / TRACE_POINTS as f32).collect();
        let points = trace_points(&trace, plot(), 1.0, y_at);
        assert_eq!(points.len(), TRACE_POINTS / 2 + 1);
        assert_eq!(points[0].x, 0.0);
        assert!((points.last().unwrap().x - 100.0).abs() < 1e-3);
        // Both cycles: every point, the last just short of the edge
        let points = trace_points(&trace, plot(), 2.0, y_at);
        assert_eq!(points.len(), TRACE_POINTS);
        assert!(points.last().unwrap().x < 100.0);
    }

    #[test]
    fn a_loud_stack_is_scaled_to_fit_and_a_ringing_edge_is_not() {
        let mut trace = vec![0.0; TRACE_POINTS];
        trace[10] = 1.1;
        assert!((trace_points(&trace, plot(), 2.0, y_at)[10].y - y_at(1.1)).abs() < 1e-4);
        trace[10] = 2.4;
        assert!((trace_points(&trace, plot(), 2.0, y_at)[10].y - y_at(1.0)).abs() < 1e-4);
    }

    #[test]
    fn captures_glow_then_settle() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("osc");
        let at = |time: f64, data: Option<&ScopeData>| {
            let input = egui::RawInput { time: Some(time), ..Default::default() };
            let mut live = None;
            let _ = ctx.run(input, |ctx| live = Some(super::live(ctx, id, data)));
            live.unwrap()
        };
        let capture = |count| ScopeData { channel1: vec![0.5; TRACE_POINTS], channel2: Vec::new(), triggered: true, count };

        assert_eq!(at(0.0, None).presence, 0.0);
        assert_eq!(at(1.0, Some(&capture(1))).presence, 1.0);
        let both = at(1.1, Some(&capture(2)));
        assert_eq!(both.traces.len(), 2);
        assert!(both.traces[0].0 < 1.0 && both.traces[1].0 == 1.0, "the older fades");
        // Nothing new: the older has faded away, the newest holds, then settles
        let held = at(1.4, Some(&capture(2)));
        assert_eq!(held.traces.len(), 1);
        assert!(held.presence > 0.0 && held.presence < 1.0);
        assert_eq!(at(2.0, Some(&capture(2))).presence, 0.0);
        // A capture of another length (not the oscillator's) is passed over
        let other = ScopeData { channel1: vec![0.0; 7], channel2: Vec::new(), triggered: true, count: 3 };
        assert_eq!(at(2.1, Some(&other)).presence, 0.0);
    }
}
