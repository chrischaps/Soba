//! The Reverb's display: the room's decay, and where the sound is in it.
//!
//! Time runs to the right from the moment a sound goes in, on a stretched
//! axis so the first few milliseconds have room beside a long tail. Level
//! falls from 0 dB at the top to -60 dB at the bottom. After the pre-delay
//! gap come the early reflections, every path through the diffuser at this
//! Size, drawn as a spray of hairlines that thickens into the tail. The tail
//! is two slopes: the lows, darker, reaching -60 dB at the Decay time, and
//! the highs, brighter, dying sooner the more Damping takes them.
//!
//! While the room rings, a dot rides down the slope at the level the tail
//! has fallen to, and the tail still to come glows. With Mod up, eight faint
//! motes drift along the top, one per line, as the lines' lengths sway.

use std::f32::consts::TAU;

use eframe::egui::{self, Color32, Id, Pos2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::modules::Reverb;

use super::display_frame::{self as face, columns, curve, guide, header, playhead, readout, value_of, with_alpha};
use super::drum_display::fill_under;
use super::{SynthGraph, SynthGraphState};

/// The level axis reaches down this far, in dB: the RT60.
const FLOOR_DB: f32 = -60.0;

/// The colour of the lows' slope; the highs take the module's colour.
const LOWS: Color32 = Color32::from_rgb(120, 124, 255);

/// Columns the early reflections are gathered into, across the plot.
const SPRAY_BINS: usize = 96;

/// How far below its loudest the tail is drawn as gone, in dB.
const SILENT_DB: f32 = 60.0;

/// What the display remembers of the tail between frames: its loudest of
/// late (dB), and when it was last that loud (seconds on the UI clock).
#[derive(Clone, Copy, Default)]
struct Loudest {
    db: f32,
    at: f64,
}

/// The room, as the knobs set it.
#[derive(Clone, Copy, Debug)]
struct Room {
    /// Pre-delay, in seconds.
    predelay: f32,
    /// How long the lows and the highs take to fall 60 dB, in seconds.
    decay: f32,
    hf_decay: f32,
    /// The shortest feedback line, in seconds.
    room: f32,
}

impl Room {
    /// How far the axis runs, in seconds: the whole tail and a little more.
    fn span(&self) -> f32 {
        (self.predelay + self.decay) * 1.05
    }

    /// The lows' and the highs' level `t` seconds after a hit, in dB.
    fn lows_db(&self, t: f32) -> f32 {
        FLOOR_DB * ((t - self.predelay) / self.decay.max(0.01)).max(0.0)
    }

    fn highs_db(&self, t: f32) -> f32 {
        FLOOR_DB * ((t - self.predelay) / self.hf_decay.max(0.01)).max(0.0)
    }

    /// When every path through the diffuser arrives, in seconds after a hit:
    /// one tap from each step, in every combination.
    fn reflections(&self) -> Vec<f32> {
        let taps = Reverb::diffuser_taps();
        let mut times = vec![0.0f32];
        for step in taps.iter() {
            times = times.iter().flat_map(|&t| step.iter().map(move |&tap| t + tap)).collect();
        }
        times.into_iter().map(|t| self.predelay + t * self.room).collect()
    }
}

/// "450 ms", "2.0 s", "12 s".
fn seconds(s: f32) -> String {
    if s < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else if s < 10.0 {
        format!("{s:.1} s")
    } else {
        format!("{s:.0} s")
    }
}

/// Draws the display, as wide as the knob row below it.
pub fn reverb_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) {
    let z = zoom;
    let size = value_of(graph, node_id, "Size").unwrap_or(0.5);
    let decay = value_of(graph, node_id, "Decay").unwrap_or(2.0).max(0.1);
    let damping = value_of(graph, node_id, "Damping").unwrap_or(0.5);
    let modulation = value_of(graph, node_id, "Mod").unwrap_or(0.25).clamp(0.0, 1.0);
    let room = Room {
        predelay: value_of(graph, node_id, "Pre-Delay").unwrap_or(0.0).max(0.0) / 1000.0,
        decay,
        hf_decay: Reverb::hf_decay(decay, damping),
        room: Reverb::room_ms(size) / 1000.0,
    };

    let accent = ModuleCategory::Effect.color();
    let width = face::knob_row_width(ui, 4, z);
    let Some((rect, response, painter)) = face::frame(ui, width, accent, z) else { return };
    let plot = face::plot(rect, z);

    // Time stretched by its square root, so the early part has room
    let span = room.span();
    let x_at = |t: f32| plot.left() + (t / span).clamp(0.0, 1.0).sqrt() * plot.width();
    let t_at = |i: usize, n: usize| span * (i as f32 / n as f32).powi(2);
    let y_at = |db: f32| plot.top() + (db.clamp(FLOOR_DB, 0.0) / FLOOR_DB) * plot.height();

    for db in [-20.0, -40.0] {
        guide(&painter, Pos2::new(plot.left(), y_at(db)), Pos2::new(plot.right(), y_at(db)), 8, z);
    }
    let pre_x = x_at(room.predelay);
    if room.predelay > 0.0 {
        guide(&painter, Pos2::new(pre_x, plot.top()), Pos2::new(pre_x, plot.bottom()), 22, z);
    }

    // The two slopes, the lows behind the highs
    let columns = columns(plot, z);
    let slope = |level: &dyn Fn(f32) -> f32| -> Vec<Pos2> {
        let mut points = vec![Pos2::new(pre_x, plot.bottom())];
        points.extend((0..=columns).map(|i| t_at(i, columns)).filter(|&t| t >= room.predelay).map(|t| Pos2::new(x_at(t), y_at(level(t)))));
        // It ends where it meets the floor
        if let Some(end) = points.iter().skip(1).position(|p| p.y >= plot.bottom() - 0.01) {
            points.truncate(end + 2);
        }
        points
    };
    let lows = slope(&|t| room.lows_db(t));
    let highs = slope(&|t| room.highs_db(t));
    fill_under(&painter, &lows, plot.bottom(), with_alpha(LOWS, 54), with_alpha(LOWS, 6));
    curve(&painter, lows.clone(), with_alpha(LOWS, 150), 1.0 * z, false);
    fill_under(&painter, &highs, plot.bottom(), with_alpha(accent, 60), with_alpha(accent, 8));
    curve(&painter, highs, accent, 1.3 * z, true);

    // The early reflections: every path through the diffuser, gathered
    // into columns, each as bright as the paths arriving there
    let mut bins = [0u32; SPRAY_BINS];
    for t in room.reflections() {
        let along = (t / span).clamp(0.0, 1.0).sqrt();
        bins[((along * SPRAY_BINS as f32) as usize).min(SPRAY_BINS - 1)] += 1;
    }
    let most = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
    for (bin, &count) in bins.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let x = plot.left() + (bin as f32 + 0.5) / SPRAY_BINS as f32 * plot.width();
        let share = (count as f32 / most).sqrt();
        painter.line_segment(
            [Pos2::new(x, y_at(0.0)), Pos2::new(x, y_at(-24.0 * share))],
            egui::Stroke::new(0.6 * z, Color32::from_white_alpha((40.0 + 120.0 * share) as u8)),
        );
    }

    // --- The room ringing now ---
    let live = readout(user_state, node_id).filter(|_| user_state.is_playing);
    let now = ui.input(|input| input.time);
    let memory = Id::unique(("reverb_loudest", node_id));
    let mut loudest = ui.ctx().data_mut(|data| data.get_temp::<Loudest>(memory)).unwrap_or(Loudest { db: -120.0, at: now });
    let mut ringing = false;
    if let Some(r) = live {
        let tail = r.values[Reverb::READOUT_TAIL_DB];
        // The loudest holds while the tail falls from it, so the dot runs
        // down the slope at the room's own pace; past that it lets go
        if tail >= loudest.db - 0.5 || now - loudest.at > (decay + 0.5) as f64 {
            loudest = Loudest { db: tail, at: now };
        }
        let fallen = loudest.db - tail;
        if loudest.db > -100.0 && fallen < SILENT_DB {
            ringing = true;
            let t = room.predelay + decay * fallen / -FLOOR_DB;
            let at = Pos2::new(x_at(t), y_at(-fallen));
            // The tail still to come, glowing
            let ahead: Vec<Pos2> = lows.iter().copied().filter(|p| p.x >= at.x).collect();
            if ahead.len() > 1 {
                let glow = 1.0 - fallen / SILENT_DB;
                fill_under(&painter, &ahead, plot.bottom(), with_alpha(LOWS, (90.0 * glow) as u8), with_alpha(LOWS, 10));
            }
            playhead(&painter, at, with_alpha(LOWS, 80), z);
        }
        // The lines' drift, as motes along the top
        if modulation > 0.01 {
            for line in 0..Reverb::LINES {
                let phase = r.values[Reverb::READOUT_MOD_PHASE + line];
                let x = plot.left() + (line as f32 + 0.5) / Reverb::LINES as f32 * plot.width() + (phase * TAU).cos() * 3.0 * z * modulation;
                let y = plot.top() + 2.5 * z + (phase * TAU).sin() * 1.5 * z;
                painter.circle_filled(Pos2::new(x, y), 1.2 * z, Color32::from_white_alpha((30.0 + 90.0 * modulation) as u8));
            }
        }
    }
    ui.ctx().data_mut(|data| data.insert_temp(memory, loudest));

    // --- Words ---
    let left = if room.predelay > 0.0005 {
        format!("PreD {}", seconds(room.predelay))
    } else {
        format!("Size {:.0}%", size * 100.0)
    };
    header(
        &painter,
        rect,
        z,
        Some((left, accent.gamma_multiply(0.9))),
        Some((seconds(decay), theme::text::SECONDARY)),
    );

    if live.is_some() && (ringing || modulation > 0.01) {
        crate::app::pace::animate(ui.ctx());
    }

    if response.hovered() {
        let mut text = format!(
            "The lows fall 60 dB in {}, the highs in {}",
            seconds(decay),
            seconds(room.hf_decay)
        );
        if room.predelay > 0.0 {
            text.push_str(&format!("\nNothing for the first {} (Pre-Delay)", seconds(room.predelay)));
        }
        text.push_str("\nThe hairlines at the start: every early reflection through the diffuser, thickening into the tail");
        text.push_str("\nWhile the room rings, the dot rides down the slope at the level the tail has fallen to");
        if modulation > 0.01 {
            text.push_str("\nThe motes along the top drift as Mod sways the lines");
        }
        response.on_hover_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(decay: f32, damping: f32) -> Room {
        Room { predelay: 0.02, decay, hf_decay: Reverb::hf_decay(decay, damping), room: Reverb::room_ms(0.5) / 1000.0 }
    }

    #[test]
    fn the_lows_reach_the_floor_at_the_decay_time() {
        let r = room(2.0, 0.5);
        assert_eq!(r.lows_db(0.01), 0.0);
        assert!((r.lows_db(0.02 + 1.0) + 30.0).abs() < 1e-3);
        assert!((r.lows_db(0.02 + 2.0) + 60.0).abs() < 1e-3);
    }

    #[test]
    fn damping_shortens_the_highs() {
        assert_eq!(room(2.0, 0.0).hf_decay, 2.0);
        assert!((room(2.0, 1.0).hf_decay - 0.2).abs() < 1e-5);
        let r = room(2.0, 0.5);
        assert!(r.highs_db(1.0) < r.lows_db(1.0));
    }

    #[test]
    fn every_path_through_the_diffuser_arrives_after_the_predelay() {
        let r = room(2.0, 0.5);
        let times = r.reflections();
        assert_eq!(times.len(), 8usize.pow(4));
        assert!(times.iter().all(|&t| t >= r.predelay && t < r.predelay + 2.0 * r.room));
    }

    #[test]
    fn times_read_plainly() {
        assert_eq!(seconds(0.45), "450 ms");
        assert_eq!(seconds(2.0), "2.0 s");
        assert_eq!(seconds(12.0), "12 s");
    }
}
