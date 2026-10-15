//! The Sampler's face while it records: its Rec button, the take drawn as
//! it comes in, and a Sampler waiting to start one. The Chopper records
//! with the same recorder, and shows it the same way.
//!
//! A take draws in red, growing from the left across a span that doubles
//! whenever the take outgrows it (or across its bars, when it's in bars),
//! with seconds ticked along the foot and the time so far at the head.
//! A take in bars is the loop it's becoming: its beats are ticked instead,
//! each bar more strongly, and the counter reads bar.beat of the bars, so
//! it's plain how much of the phrase is left to play.
//! What the Sampler held before stays where it was until the take is done.
//!
//! Waiting for a sound, the panel dims and an input meter runs along its
//! foot, with the threshold marked, so it's plain how loud to be. Armed
//! for the next bar, it says so.
//!
//! With Tidy takes on (from Rec's right-click menu), Rec wears crop marks,
//! as a trim previewed on the waveform does. A take tidied as it's kept
//! shows them round the whole panel for a moment, with what tidying did.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;
use web_time::Instant;

use crate::app::theme;
use crate::dsp::Readout;
use crate::modules::sampler::{RecState, Sampler, NO_THRESHOLD_DB};

use super::state::ScopeData;
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// How long a press from the face holds Rec down, at least: long enough
/// for the audio thread to see it.
pub const MIN_PRESS_SECONDS: f64 = 0.06;

/// The face's Rec button parameter.
pub const REC_BUTTON: &str = "Rec Button";

/// The parameter Rec's menu turns on to tidy each take as it's kept.
pub const TIDY_TAKES: &str = "Tidy Takes";

/// How long a take's face says it was tidied, the last part fading.
const TIDIED_SECONDS: f32 = 3.5;
const TIDIED_FADE_SECONDS: f32 = 1.0;

/// What the recorder is doing, as the face shows it.
#[derive(Clone, Copy, Debug)]
pub struct RecView {
    pub state: RecState,
    /// Seconds the take under way holds.
    pub seconds: f32,
    /// Seconds the live overview spans.
    pub scale: f32,
    /// How loud In is, linear.
    pub level: f32,
    /// Whether it holds room to record into.
    pub room: bool,
    /// The Thresh knob, in dB.
    pub threshold_db: f32,
    /// Seconds in a beat of a take in bars, 0 for a free take.
    pub beat: f32,
    /// Beats in that take's bar.
    pub per_bar: u32,
}

impl RecView {
    pub fn read(readout: Option<&Readout>, threshold_db: f32) -> Self {
        let value = |index: usize| readout.map_or(0.0, |r| r.values[index]);
        Self {
            state: RecState::from_code(value(Sampler::READOUT_REC_STATE)),
            seconds: value(Sampler::READOUT_REC_SECONDS),
            scale: value(Sampler::READOUT_REC_SCALE).max(0.001),
            level: value(Sampler::READOUT_REC_LEVEL),
            room: value(Sampler::READOUT_ROOM) > 0.0,
            threshold_db,
            beat: value(Sampler::READOUT_REC_BEAT),
            per_bar: (value(Sampler::READOUT_REC_BAR).round() as u32).max(1),
        }
    }

    /// The beats and bars of a take in bars, if it is one: (beats so far,
    /// bars it lasts).
    fn in_bars(&self) -> Option<(f32, u32)> {
        (self.beat > 0.0).then(|| (self.seconds / self.beat, ((self.scale / self.beat / self.per_bar as f32).round() as u32).max(1)))
    }

    /// The recorder's counter: `REC  2.3 / 4` (bar 2, beat 3, of four
    /// bars) for a take in bars, or the seconds so far.
    pub fn counter(&self) -> String {
        match self.in_bars() {
            Some((beats, bars)) => {
                // The beat under way, from 0, held on the last as the take closes
                let whole = ((beats + 1e-3).floor() as u32).min(bars * self.per_bar - 1);
                format!("REC  {}.{} / {bars}", whole / self.per_bar + 1, whole % self.per_bar + 1)
            }
            None => format!("REC  {:.1} s", self.seconds),
        }
    }

    /// Whether a press is under way: recording, or waiting to.
    pub fn is_busy(&self) -> bool {
        self.state != RecState::Idle
    }
}

/// Why Rec can't record right now, if it can't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecBlock {
    /// Nothing is patched into In.
    NoInput,
    /// The Rec jack is patched, and presses it.
    Jack,
    /// The patch isn't playing.
    Stopped,
    /// The room to record into hasn't arrived yet.
    NoRoom,
}

impl RecBlock {
    fn tip(self) -> &'static str {
        match self {
            RecBlock::NoInput => "Rec records what's patched into In L and In R: patch an Audio Input to record yourself, or the patch's own sound to resample it",
            RecBlock::Jack => "The Rec jack is patched: it records while its gate is high",
            RecBlock::Stopped => "Start the patch playing to record",
            RecBlock::NoRoom => "Getting ready to record…",
        }
    }
}

/// Whether a node has something patched into In, and why its Rec can't
/// record right now, if it can't.
pub fn rec_block(graph: &SynthGraph, node_id: NodeId, user_state: &SynthGraphState, view: &RecView) -> (bool, Option<RecBlock>) {
    let patched = |name: &str| {
        graph.nodes.get(node_id).is_some_and(|node| node.get_input(name).is_ok_and(|input| graph.connection(input).is_some()))
    };
    let in_patched = patched("In L") || patched("In R");
    let blocked = if !in_patched {
        Some(RecBlock::NoInput)
    } else if patched("Rec") {
        Some(RecBlock::Jack)
    } else if !user_state.is_playing {
        Some(RecBlock::Stopped)
    } else if !view.room {
        Some(RecBlock::NoRoom)
    } else {
        None
    };
    (in_patched, blocked)
}

/// Draws a take as it comes in, across `plot`.
pub fn draw_take(painter: &egui::Painter, plot: Rect, view: &RecView, overview: Option<&ScopeData>, z: f32) {
    let red = theme::accent::ERROR;
    let x_at = |seconds: f32| plot.left() + (seconds / view.scale).clamp(0.0, 1.0) * plot.width();
    let mid = plot.center().y;

    let foot_tick = |x: f32, height: f32, alpha: f32| {
        painter.vline(x, (plot.bottom() - height * z)..=plot.bottom(), Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(alpha)));
    };
    if let Some((_, bars)) = view.in_bars() {
        // Its beats along the foot, each bar taller and brighter, and the
        // beats left out where they'd crowd
        let beats = bars * view.per_bar;
        let roomy = plot.width() / beats as f32 >= 3.0 * z;
        for beat in 1..beats {
            let x = x_at(beat as f32 * view.beat);
            if beat % view.per_bar == 0 {
                foot_tick(x, 6.0, 0.3);
            } else if roomy {
                foot_tick(x, 3.0, 0.12);
            }
        }
    } else {
        // Seconds along the foot, a tick for each, or every few on a long span
        let every = [1.0, 2.0, 4.0, 8.0, 16.0].into_iter().find(|step| view.scale / step <= 16.0).unwrap_or(30.0);
        let mut tick = every;
        while tick < view.scale {
            foot_tick(x_at(tick), 3.0, 0.12);
            tick += every;
        }
    }
    painter.hline(plot.x_range(), mid, Stroke::new(0.5 * z, Color32::from_rgba_unmultiplied(255, 255, 255, 10)));

    // The take so far, a column at a time
    let head = x_at(view.seconds);
    if let Some(overview) = overview.filter(|o| !o.channel1.is_empty() && o.channel1.len() == o.channel2.len()) {
        let (highs, lows) = (&overview.channel1, &overview.channel2);
        let count = highs.len();
        let tallest = highs.iter().chain(lows).map(|v| v.abs()).fold(0.05, f32::max);
        let half = plot.height() / 2.0 * 0.92;
        let columns = (plot.width() / (1.0 * z)).max(16.0) as usize;
        for column in 0..columns {
            let x = plot.left() + (column as f32 + 0.5) / columns as f32 * plot.width();
            if x > head {
                break;
            }
            let from = column * count / columns;
            let to = ((column + 1) * count / columns).max(from + 1).min(count);
            let high = highs[from..to].iter().copied().fold(0.0, f32::max);
            let low = lows[from..to].iter().copied().fold(0.0, f32::min);
            let top = mid - (high / tallest) * half;
            let bottom = mid - (low / tallest) * half;
            painter.line_segment([Pos2::new(x, top), Pos2::new(x, bottom.max(top + 0.6 * z))], Stroke::new(1.0 * z, red.gamma_multiply(0.85)));
        }
    }

    // The head, writing
    painter.vline(head, plot.y_range(), Stroke::new(1.2 * z, red));
    painter.circle_filled(Pos2::new(head, mid), 3.5 * z, red.gamma_multiply(0.4));
    painter.circle_filled(Pos2::new(head, mid), 1.8 * z, Color32::WHITE);

    // The time so far, as a recorder's counter: on a dark tab of its own,
    // so the take can run under it
    let font = egui::FontId::proportional(8.0 * z);
    let text = painter.layout_no_wrap(view.counter(), font, Color32::WHITE);
    let tab = Rect::from_min_size(plot.left_top() + Vec2::new(1.0 * z, 1.0 * z), text.size() + Vec2::new(14.0 * z, 2.0 * z));
    painter.rect_filled(tab, 3.0 * z, Color32::from_black_alpha(190));
    painter.circle_filled(Pos2::new(tab.left() + 5.5 * z, tab.center().y), 2.4 * z, red);
    painter.galley(Pos2::new(tab.left() + 10.0 * z, tab.top() + 1.0 * z), text, Color32::WHITE);
}

/// Dims the panel over a Sampler waiting to record, saying for what: a
/// sound, with In's level against the threshold, or the next bar.
pub fn draw_waiting(painter: &egui::Painter, rect: Rect, plot: Rect, view: &RecView, now: f64, z: f32) {
    let red = theme::accent::ERROR;
    painter.rect_filled(rect, 3.0 * z, Color32::from_black_alpha(150));
    let pulse = 0.55 + 0.45 * (now * std::f64::consts::TAU / 1.2).cos().abs() as f32;
    let caption = match view.state {
        RecState::Armed => "Recording from the next bar",
        _ => "Waiting for a sound",
    };
    let center = plot.center() - Vec2::new(0.0, 6.0 * z);
    let font = egui::FontId::proportional(9.0 * z);
    let width = painter.layout_no_wrap(caption.to_string(), font.clone(), Color32::WHITE).size().x;
    painter.circle_filled(Pos2::new(center.x - width / 2.0 - 7.0 * z, center.y), 3.0 * z, red.gamma_multiply(pulse));
    painter.text(center, egui::Align2::CENTER_CENTER, caption, font, Color32::WHITE.gamma_multiply(0.9));

    if view.state == RecState::Waiting {
        // In's level, -60 to 0 dB, and the threshold it must pass
        let track = Rect::from_min_max(Pos2::new(plot.left() + 12.0 * z, plot.bottom() - 12.0 * z), Pos2::new(plot.right() - 12.0 * z, plot.bottom() - 8.0 * z));
        let place = |db: f32| ((db - NO_THRESHOLD_DB) / -NO_THRESHOLD_DB).clamp(0.0, 1.0);
        let level_db = 20.0 * view.level.max(1e-6).log10();
        painter.rect_filled(track, 2.0 * z, Color32::WHITE.gamma_multiply(0.08));
        let lit = Rect::from_min_max(track.min, Pos2::new(track.left() + place(level_db) * track.width(), track.bottom()));
        let over = level_db > view.threshold_db;
        painter.rect_filled(lit, 2.0 * z, if over { red } else { Color32::WHITE.gamma_multiply(0.5) });
        let x = track.left() + place(view.threshold_db) * track.width();
        painter.vline(x, (track.top() - 3.0 * z)..=(track.bottom() + 3.0 * z), Stroke::new(1.2 * z, red));
        painter.text(
            Pos2::new(x, track.top() - 3.5 * z),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.0} dB", view.threshold_db),
            egui::FontId::proportional(7.0 * z),
            Color32::WHITE.gamma_multiply(0.6),
        );
    }
}

/// Crop marks at the corners of `rect`, each arm `arm` long.
fn crop_marks(painter: &egui::Painter, rect: Rect, arm: f32, stroke: Stroke) {
    for (x, inward) in [(rect.left(), 1.0), (rect.right(), -1.0)] {
        for (y, down) in [(rect.top(), 1.0), (rect.bottom(), -1.0)] {
            painter.line_segment([Pos2::new(x, y), Pos2::new(x + inward * arm, y)], stroke);
            painter.line_segment([Pos2::new(x, y), Pos2::new(x, y + down * arm)], stroke);
        }
    }
}

/// Seconds since the face first showed the take tidied `at`, on the
/// frame clock, so a capture shows it as long as the app does.
pub fn tidied_age(ui: &egui::Ui, node_id: NodeId, at: Instant) -> f32 {
    let id = egui::Id::new(("sampler_tidied", node_id));
    let now = ui.input(|i| i.time);
    let shown = ui.data_mut(|d| match d.get_temp::<(Instant, f64)>(id) {
        Some((seen, shown)) if seen == at => shown,
        _ => {
            d.insert_temp(id, (at, now));
            now
        }
    });
    (now - shown) as f32
}

/// Says a take was just tidied as it was kept, `age` seconds ago: crop
/// marks round the panel and what tidying did, on a tab at its corner,
/// fading. Returns whether it's still showing.
pub fn draw_tidied(painter: &egui::Painter, plot: Rect, said: &str, age: f32, z: f32) -> bool {
    if age >= TIDIED_SECONDS {
        return false;
    }
    let alpha = ((TIDIED_SECONDS - age) / TIDIED_FADE_SECONDS).min(1.0);
    crop_marks(painter, plot.shrink(1.0 * z), 5.0 * z, Stroke::new(1.2 * z, Color32::WHITE.gamma_multiply(alpha)));

    let font = egui::FontId::proportional(8.0 * z);
    let text = painter.layout_no_wrap(format!("Tidied  {said}"), font, Color32::WHITE.gamma_multiply(alpha));
    let size = text.size() + Vec2::new(16.0 * z, 2.0 * z);
    let tab = Rect::from_min_size(Pos2::new(plot.right() - size.x - 8.0 * z, plot.top() + 3.0 * z), size);
    painter.rect_filled(tab, 3.0 * z, Color32::from_black_alpha((190.0 * alpha) as u8));
    let mark = Rect::from_center_size(Pos2::new(tab.left() + 6.0 * z, tab.center().y), Vec2::new(5.0 * z, 5.0 * z));
    crop_marks(painter, mark, 1.6 * z, Stroke::new(0.9 * z, Color32::WHITE.gamma_multiply(0.8 * alpha)));
    painter.galley(Pos2::new(tab.left() + 12.0 * z, tab.top() + 1.0 * z), text, Color32::WHITE);
    true
}

/// The Rec button, in `rect`: pressed from the face, it holds the Rec
/// Button parameter (the module's `param_index`) down for a moment.
/// Right-click turns Tidy takes on or off (`tidies`, where the module has
/// it), and learns a MIDI footswitch for it.
#[allow(clippy::too_many_arguments)]
pub fn rec_button(
    ui: &mut egui::Ui,
    rect: Rect,
    node_id: NodeId,
    user_state: &SynthGraphState,
    view: &RecView,
    down: bool,
    param_index: usize,
    tidies: Option<bool>,
    blocked: Option<RecBlock>,
    z: f32,
) -> Vec<SynthResponse> {
    let tidy_menu = tidies.is_some();
    let tidies = tidies.unwrap_or(false);
    let mut responses = Vec::new();
    let red = theme::accent::ERROR;
    let id = ui.id().with(("sampler_rec", node_id));
    let button = ui.interact(rect, id, Sense::click());
    super::marks::mark(ui.ctx(), node_id, "rec", rect);
    let engine_node_id = user_state.get_engine_node_id(node_id);
    // Busy, it can always be pressed again to let go
    let usable = blocked.is_none() || (view.is_busy() && blocked != Some(RecBlock::Jack));

    // Down for at least MIN_PRESS_SECONDS
    let now = ui.input(|i| i.time);
    let pressed_at: Option<f64> = ui.ctx().data(|d| d.get_temp(id));
    // The primary button only: a right-click opens the menu, not a take
    let primary = button.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    let holding = usable && (primary || button.clicked());
    match pressed_at {
        None if holding => {
            ui.ctx().data_mut(|d| d.insert_temp(id, now));
            responses.push(SynthResponse::PlayParameter { node_id, param_name: REC_BUTTON.to_string(), value: 1.0 });
        }
        Some(at) if !button.is_pointer_button_down_on() => {
            let left = MIN_PRESS_SECONDS - (now - at);
            if left <= 0.0 {
                ui.ctx().data_mut(|d| d.remove::<f64>(id));
                responses.push(SynthResponse::PlayParameter { node_id, param_name: REC_BUTTON.to_string(), value: 0.0 });
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
        }
        _ => {}
    }
    let down = down || pressed_at.is_some();

    // A dot: lit while recording, pulsing while waiting to
    let fill = match (view.state, down, button.hovered() && usable) {
        (RecState::Recording, _, _) => red.gamma_multiply(0.45),
        (_, true, _) => theme::background::WIDGET_HOVERED,
        (_, _, true) => theme::background::WIDGET_HOVERED,
        _ => theme::background::WIDGET,
    };
    ui.painter().rect(rect, 3.0 * z, fill, Stroke::new(1.0 * z, theme::background::GRID_MAJOR));
    // Recording, it's a stop button, as ▶ is while cueing
    if view.state == RecState::Recording {
        ui.painter().rect_filled(Rect::from_center_size(rect.center(), Vec2::splat(5.0 * z)), 0.5 * z, Color32::WHITE);
    }
    let dot = match view.state {
        RecState::Recording => Color32::TRANSPARENT,
        RecState::Armed | RecState::Waiting => {
            ui.ctx().request_repaint();
            red.gamma_multiply(0.45 + 0.55 * (now * std::f64::consts::TAU / 1.2).cos().abs() as f32)
        }
        RecState::Idle if usable => red,
        RecState::Idle => red.gamma_multiply(0.3),
    };
    ui.painter().circle_filled(rect.center(), 3.2 * z, dot);
    // Tidying takes, it wears crop marks
    if tidies {
        crop_marks(ui.painter(), rect.shrink(1.5 * z), 2.5 * z, Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.75)));
    }

    // A learned MIDI footswitch shows as a purple badge
    let mapping = engine_node_id.and_then(|eid| user_state.get_midi_mapping(eid, param_index));
    let learning = engine_node_id.is_some_and(|eid| user_state.is_midi_learn_target(eid, param_index));
    if mapping.is_some() || learning {
        let alpha = if learning { 0.4 + 0.6 * ((now * 4.0).sin().abs() as f32) } else { 1.0 };
        ui.painter().circle_filled(rect.right_top() + Vec2::new(-2.0 * z, 2.0 * z), 2.2 * z, theme::signal::MIDI.gamma_multiply(alpha));
        if learning {
            ui.ctx().request_repaint();
        }
    }

    let tip = match (view.state, blocked) {
        (RecState::Recording, _) => "Recording: press to stop".to_string(),
        (RecState::Armed, _) => "Recording from the next bar: press to let go".to_string(),
        (RecState::Waiting, _) => "Waiting for a sound louder than Thresh: press to let go".to_string(),
        (RecState::Idle, Some(block)) => block.tip().to_string(),
        (RecState::Idle, None) => {
            let mut tip = "Record what's patched into In: press to start, and again to stop. The take plays at once, and is kept with the patch".to_string();
            if tidies {
                tip.push_str("\nTidy takes is on: each is trimmed to its sound and set to −16 dBFS RMS as it's kept");
            }
            if let Some(m) = mapping {
                tip.push_str(&format!("\nMIDI CC {} presses it", m.cc_number));
            }
            tip
        }
    };
    let menu = if tidy_menu { "Right-click to tidy takes as they're kept, or learn a MIDI footswitch" } else { "Right-click to learn a MIDI footswitch" };
    let button = button.on_hover_text(format!("{tip}\n{menu}"));
    button.context_menu(|ui| {
        if tidy_menu {
            let mut on = tidies;
            let tidy = ui.checkbox(&mut on, "Tidy takes").on_hover_text(
                "Trim each take to its sound and set it to −16 dBFS RMS as it's kept, into one file. A take in bars keeps its length. Undo brings back the take as it came in",
            );
            if tidy.changed() {
                responses.push(SynthResponse::ParameterChanged { node_id, param_name: TIDY_TAKES.to_string(), value: if on { 1.0 } else { 0.0 } });
                ui.close_menu();
            }
            ui.separator();
        }
        let Some(eid) = engine_node_id else { return };
        if learning {
            if ui.button("Cancel MIDI Learn").clicked() {
                responses.push(SynthResponse::MidiLearnCancel);
                ui.close_menu();
            }
            return;
        }
        let learn = if mapping.is_some() { "Re-learn MIDI CC" } else { "Learn MIDI CC" };
        if ui.button(learn).clicked() {
            responses.push(SynthResponse::MidiLearnStart { engine_node_id: eid, param_index, param_name: REC_BUTTON.to_string(), min_value: 0.0, max_value: 1.0 });
            ui.close_menu();
        }
        if mapping.is_some() && ui.button("Clear MIDI mapping").clicked() {
            responses.push(SynthResponse::MidiLearnClear { engine_node_id: eid, param_index });
            ui.close_menu();
        }
    });
    responses
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(seconds: f32, scale: f32, beat: f32) -> RecView {
        RecView { state: RecState::Recording, seconds, scale, level: 0.0, room: true, threshold_db: NO_THRESHOLD_DB, beat, per_bar: 4 }
    }

    #[test]
    fn test_a_take_in_bars_counts_bars_and_beats() {
        // Four bars at 120 BPM: 8 s, half a second a beat
        assert_eq!(recording(0.0, 8.0, 0.5).counter(), "REC  1.1 / 4");
        assert_eq!(recording(2.6, 8.0, 0.5).counter(), "REC  2.2 / 4");
        assert_eq!(recording(7.99, 8.0, 0.5).counter(), "REC  4.4 / 4");
        assert_eq!(recording(8.0, 8.0, 0.5).counter(), "REC  4.4 / 4", "held on the last beat as it closes");
    }

    #[test]
    fn test_a_free_take_counts_seconds() {
        assert_eq!(recording(2.34, 4.0, 0.0).counter(), "REC  2.3 s");
    }
}
