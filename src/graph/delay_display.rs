//! The Stereo Delay's display: the train of echoes, and each sound running
//! down it.
//!
//! Time runs to the right from the moment a sound goes in. The first bar,
//! at the left edge, is the dry sound; after it come the echoes, one delay
//! time apart, each as loud as the feedback leaves it, on a dB scale so a
//! long tail stays readable. The left side stands above the centre line and
//! the right hangs below it. Every repeat passes the loop's filters once
//! more, so each bar is a little warmer than the one before (High Cut, and
//! the tape's own loss in Tape mode) and a little thinner (Low Cut).
//!
//! The guide is what the knobs promise; over it, the sound coming in lights
//! each echo as it arrives, so a hit visibly walks down the train and fades.
//! With Ping-Pong on, the echoes take turns: each side's second echo is
//! the other side's first, come back across. In Tape mode the bars blur and
//! sway with the wow, and past unity the feedback runs away in orange.

use eframe::egui::{self, Color32, Pos2, Stroke};
use egui_node_graph2::{NodeId, SignalTrace};

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::modules::StereoDelay;

use super::display_frame::{self as face, header, incoming_trace, input_patched, readout, value_of, with_alpha};
use super::{SynthGraph, SynthGraphState};

/// Echoes drawn after the dry sound.
const ECHOES: usize = 6;

/// The level axis, in dB below full: a bar this far down has no height.
const RANGE_DB: f32 = 42.0;

/// How the wow's pull on the read head (ms) is shown: many times over, so
/// the eye can follow what the ear hears as a slow sway.
const WOW_EXAGGERATION: f32 = 12.0;

/// The frequencies whose loss each repeat's colour and width follow, in Hz.
const BRIGHT_HZ: f32 = 3000.0;
const BODY_HZ: f32 = 200.0;

/// How long an echo glows on after the sound has passed it, in seconds,
/// and how many looks back that takes.
const AFTERGLOW: f32 = 0.25;
const AFTERGLOW_LOOKS: usize = 8;

/// Below this, the sound coming in is drawn as quiet as it is, not scaled up.
const LOUDEST_FLOOR: f32 = 0.02;

/// The colour a repeat warms toward as its highs go.
const WARM: Color32 = Color32::from_rgb(255, 170, 96);

/// What the display needs to know, from the knobs and the engine.
#[derive(Clone, Copy, Debug)]
struct Train {
    /// What each repeat keeps of the one before.
    loop_gain: f32,
    mix: f32,
    /// How much of 3 kHz and of 200 Hz survive one pass of the loop.
    bright: f32,
    body: f32,
    ping_pong: bool,
}

impl Train {
    /// How loud echo `n` (1 is the first) is against what went in, as
    /// gain; echo 0 is the dry sound.
    fn level(&self, n: usize) -> f32 {
        if n == 0 {
            1.0 - self.mix
        } else {
            self.mix * self.loop_gain.min(1.0).powi(n as i32 - 1)
        }
    }

    /// Whether echo `n` on one side came in on the other side.
    fn crossed(&self, n: usize) -> bool {
        self.ping_pong && n >= 2 && n.is_multiple_of(2)
    }
}

/// How much of `hz` a one-pole lowpass at `cutoff` lets through.
fn lowpass(hz: f32, cutoff: f32) -> f32 {
    1.0 / (1.0 + (hz / cutoff.max(1.0)).powi(2)).sqrt()
}

/// How much of `hz` a one-pole highpass at `cutoff` lets through.
fn highpass(hz: f32, cutoff: f32) -> f32 {
    1.0 / (1.0 + (cutoff / hz.max(1.0)).powi(2)).sqrt()
}

/// A gain as a share of a half's height.
fn height_of(gain: f32) -> f32 {
    if gain <= 0.0 {
        return 0.0;
    }
    ((20.0 * gain.log10() + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0)
}

/// "450 ms", "1.20 s".
fn milliseconds(ms: f32) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

/// Mixes `a` toward `b` by `t`.
fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Draws the display, as wide as the knob row below it.
pub fn delay_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let knob_time = value_of(graph, node_id, "Time").unwrap_or(500.0);
    let knob_feedback = value_of(graph, node_id, "Feedback").unwrap_or(0.5);
    let sync = value_of(graph, node_id, "Sync").unwrap_or(0.0).max(0.0) as usize;
    let tape_on = value_of(graph, node_id, "Tape").unwrap_or(0.0) > 0.5;
    let high_cut = value_of(graph, node_id, "High Cut").unwrap_or(10_000.0);
    let low_cut = value_of(graph, node_id, "Low Cut").unwrap_or(20.0);

    // The engine's word on time and feedback once it has spoken: it knows the
    // tempo Sync follows and what the CV is doing
    let live = readout(user_state, node_id).filter(|r| r.values[StereoDelay::READOUT_TIME_MS] > 0.0);
    let time_ms = live.map_or(knob_time, |r| r.values[StereoDelay::READOUT_TIME_MS]);
    let plain_gain = knob_feedback.clamp(0.0, 0.95);
    let loop_gain = live.map_or(plain_gain, |r| r.values[StereoDelay::READOUT_LOOP_GAIN]);
    let tape = live.map_or(if tape_on { 1.0 } else { 0.0 }, |r| r.values[StereoDelay::READOUT_TAPE]);
    let wow_ms = live.map_or(0.0, |r| r.values[StereoDelay::READOUT_WOW_MS]);

    let tape_loss = lowpass(BRIGHT_HZ, StereoDelay::tape_loss_hz(time_ms));
    let train = Train {
        loop_gain,
        mix: value_of(graph, node_id, "Mix").unwrap_or(0.5).clamp(0.0, 1.0),
        bright: lowpass(BRIGHT_HZ, high_cut) * (1.0 - tape + tape * tape_loss),
        body: highpass(BODY_HZ, low_cut),
        ping_pong: value_of(graph, node_id, "Ping-Pong").unwrap_or(0.0) > 0.5,
    };
    let runaway = loop_gain >= 1.0;

    let accent = ModuleCategory::Effect.color();
    let orange = theme::signal::CONTROL;
    let width = face::knob_row_width(ui, 5, z);
    let Some((rect, response, painter)) = face::frame(ui, width, accent, z) else { return };
    let plot = face::plot(rect, z);
    let middle = plot.center().y;
    let half = plot.height() / 2.0 - 1.0 * z;
    painter.hline(plot.x_range(), middle, Stroke::new(0.5 * z, Color32::from_white_alpha(22)));

    // Where echo `n` sits: the dry sound at the left, then one delay time apart
    let step = (plot.width() - 6.0 * z) / ECHOES as f32;
    let x_of = |n: usize| plot.left() + 3.0 * z + n as f32 * step + if n > 0 { (wow_ms * WOW_EXAGGERATION / time_ms.max(1.0)) * step * n as f32 } else { 0.0 };

    // The sound coming in, each side
    let left_in = incoming_trace(graph, node_id, user_state, "In L");
    let right_in = if input_patched(graph, node_id, "In R") { incoming_trace(graph, node_id, user_state, "In R") } else { left_in };
    // How loud the sound was `age` ago, or lately if louder, fading with time
    let heard = |trace: &Option<SignalTrace>, age: f32| -> f32 {
        let Some(trace) = trace.as_ref().filter(|_| user_state.is_playing) else { return 0.0 };
        (0..AFTERGLOW_LOOKS)
            .map(|look| {
                let back = look as f32 / AFTERGLOW_LOOKS as f32;
                trace.at(age + back * AFTERGLOW).map_or(0.0, f32::abs) * (1.0 - back)
            })
            .fold(0.0, f32::max)
    };
    // The loudest the sound has come in over the train's length: an echo of
    // that fills its bar, so the light reads the same for a quiet source
    let span = ECHOES as f32 * time_ms / 1000.0 + AFTERGLOW;
    let loudest = |trace: &Option<SignalTrace>| {
        trace.as_ref().map_or(0.0, |trace| (0..=(span * 60.0) as usize).filter_map(|k| trace.at(k as f32 / 60.0)).fold(0.0, |m, v| v.abs().max(m)))
    };
    let reference = loudest(&left_in).max(loudest(&right_in)).max(LOUDEST_FLOOR);
    let mut sounding = false;

    for n in 0..=ECHOES {
        let x = x_of(n);
        if x > plot.right() {
            break;
        }
        let level = train.level(n);
        let color = if n == 0 {
            Color32::from_gray(200)
        } else if runaway {
            lerp(accent, orange, n as f32 / ECHOES as f32)
        } else {
            lerp(accent, WARM, 1.0 - train.bright.powi(n as i32))
        };
        let bar = 3.2 * z * if n == 0 { 1.0 } else { 0.55 + 0.45 * train.body.powi(n as i32) };
        let blur = tape * 4.0 * z;
        let age = n as f32 * time_ms / 1000.0;

        for (side, sign) in [(0usize, -1.0f32), (1, 1.0)] {
            // The promise: as loud as the knobs leave this echo
            let guide = height_of(level) * half;
            let tip = Pos2::new(x, middle + sign * guide);
            if blur > 0.0 {
                painter.line_segment([Pos2::new(x, middle), tip], Stroke::new(bar + blur, with_alpha(color, 18)));
            }
            painter.line_segment([Pos2::new(x, middle), tip], Stroke::new(bar, with_alpha(color, 34)));

            // What's ringing there now: what came in this long ago, as loud as
            // the train leaves it
            let source = if train.crossed(n) == (side == 0) { &right_in } else { &left_in };
            let now = (heard(source, age) / reference).min(1.0) * level;
            if now > 0.0 {
                let lit = height_of(now) * half;
                if lit > 0.5 * z {
                    sounding = true;
                    let top = Pos2::new(x, middle + sign * lit);
                    painter.line_segment([Pos2::new(x, middle), top], Stroke::new(bar * 2.6 + blur, with_alpha(color, 50)));
                    painter.line_segment([Pos2::new(x, middle), top], Stroke::new(bar, lerp(color, Color32::WHITE, 0.35)));
                }
            }
        }
    }

    // The tops of the guide, joined: how the train dies away
    let tops: Vec<Pos2> = (1..=ECHOES).map(|n| Pos2::new(x_of(n), middle - height_of(train.level(n)) * half)).filter(|p| p.x <= plot.right()).collect();
    if tops.len() > 1 {
        painter.add(egui::Shape::line(tops, Stroke::new(0.6 * z, with_alpha(accent, 50))));
    }

    // --- Words ---
    let swept = (time_ms / knob_time - 1.0).abs() > 0.01 && sync == 0;
    let time_text = match StereoDelay::SYNC_LABELS.get(sync).filter(|_| sync > 0) {
        Some(division) => format!("{division} · {}", milliseconds(time_ms)),
        None => milliseconds(time_ms),
    };
    let mut right = vec![if runaway { "runaway".to_string() } else { format!("FB {:.0}%", loop_gain * 100.0) }];
    if train.ping_pong {
        right.push("P-P".to_string());
    }
    if tape_on {
        right.push("Tape".to_string());
    }
    header(
        &painter,
        rect,
        z,
        Some((time_text, if swept { orange } else { accent.gamma_multiply(0.9) })),
        Some((right.join(" · "), if runaway { orange } else { theme::text::SECONDARY })),
    );

    if user_state.is_playing && (sounding || tape > 0.0) {
        crate::app::pace::animate(ui.ctx());
    }

    if response.hovered() {
        let mut text = format!(
            "Echoes {} apart, each keeping {:.0}% of the one before",
            milliseconds(time_ms),
            loop_gain.min(1.0) * 100.0
        );
        if sync > 0 {
            text.push_str(&format!("\nSynced to {} of the tempo", StereoDelay::SYNC_LABELS[sync.min(StereoDelay::SYNC_LABELS.len() - 1)]));
        } else if swept {
            text.push_str(&format!("\nCV has moved the time from {}", milliseconds(knob_time)));
        }
        if runaway {
            text.push_str("\nPast unity: each repeat comes back louder, into the tape's saturation");
        }
        text.push_str("\nThe left side above the line, the right below; the first bar is the dry sound");
        text.push_str("\nEach repeat warms as High Cut takes its highs, and thins as Low Cut takes its lows");
        if train.ping_pong {
            text.push_str("\nPing-Pong: each side's second echo comes back from the other side");
        }
        text.push_str("\nThe sound coming in lights each echo as it arrives");
        response.on_hover_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn train(loop_gain: f32, mix: f32) -> Train {
        Train { loop_gain, mix, bright: 1.0, body: 1.0, ping_pong: false }
    }

    #[test]
    fn each_echo_keeps_the_feedback_of_the_one_before() {
        let t = train(0.5, 1.0);
        assert_eq!(t.level(0), 0.0);
        assert_eq!(t.level(1), 1.0);
        assert!((t.level(2) - 0.5).abs() < 1e-6);
        assert!((t.level(4) - 0.125).abs() < 1e-6);
    }

    #[test]
    fn the_mix_shares_the_dry_and_the_echoes() {
        let t = train(0.5, 0.25);
        assert!((t.level(0) - 0.75).abs() < 1e-6);
        assert!((t.level(1) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn past_unity_the_guide_holds_level() {
        let t = train(1.1, 1.0);
        assert_eq!(t.level(5), 1.0);
    }

    #[test]
    fn ping_pong_crosses_every_second_echo() {
        let mut t = train(0.5, 1.0);
        assert!(!t.crossed(2));
        t.ping_pong = true;
        assert!(!t.crossed(1) && t.crossed(2) && !t.crossed(3) && t.crossed(4));
    }

    #[test]
    fn heights_follow_decibels() {
        assert_eq!(height_of(1.0), 1.0);
        assert_eq!(height_of(0.0), 0.0);
        assert!((height_of(10f32.powf(-21.0 / 20.0)) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn filters_pass_their_own_side() {
        assert!(lowpass(3000.0, 20_000.0) > 0.98 && lowpass(3000.0, 1000.0) < 0.35);
        assert!(highpass(200.0, 20.0) > 0.99 && highpass(200.0, 1000.0) < 0.2);
    }
}
