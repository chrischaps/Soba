//! The Sampler's face while it records: its Rec button, the take drawn as
//! it comes in, and a Sampler waiting to start one.
//!
//! A take draws in red, growing from the left across a span that doubles
//! whenever the take outgrows it (or across its bars, when it's in bars),
//! with seconds ticked along the foot and the time so far at the head.
//! What the Sampler held before stays where it was until the take is done.
//!
//! Waiting for a sound, the panel dims and an input meter runs along its
//! foot, with the threshold marked, so it's plain how loud to be. Armed
//! for the next bar, it says so.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::Readout;
use crate::modules::sampler::{RecState, Sampler, NO_THRESHOLD_DB};

use super::state::ScopeData;
use super::{SynthGraphState, SynthResponse};

/// How long a press from the face holds Rec down, at least: long enough
/// for the audio thread to see it.
pub const MIN_PRESS_SECONDS: f64 = 0.06;

/// The face's Rec button parameter.
pub const REC_BUTTON: &str = "Rec Button";

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

/// Draws a take as it comes in, across `plot`.
pub fn draw_take(painter: &egui::Painter, plot: Rect, view: &RecView, overview: Option<&ScopeData>, label: &str, z: f32) {
    let red = theme::accent::ERROR;
    let x_at = |seconds: f32| plot.left() + (seconds / view.scale).clamp(0.0, 1.0) * plot.width();
    let mid = plot.center().y;

    // Seconds along the foot, a tick for each, or every few on a long span
    let every = [1.0, 2.0, 4.0, 8.0, 16.0].into_iter().find(|step| view.scale / step <= 16.0).unwrap_or(30.0);
    let mut tick = every;
    while tick < view.scale {
        let x = x_at(tick);
        painter.vline(x, (plot.bottom() - 3.0 * z)..=plot.bottom(), Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.12)));
        tick += every;
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
    let text = painter.layout_no_wrap(label.to_string(), font, Color32::WHITE);
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

/// The Rec button, in `rect`: pressed from the face, it holds the Rec
/// Button parameter down for a moment. Right-click learns a MIDI
/// footswitch for it.
#[allow(clippy::too_many_arguments)]
pub fn rec_button(
    ui: &mut egui::Ui,
    rect: Rect,
    node_id: NodeId,
    user_state: &SynthGraphState,
    view: &RecView,
    down: bool,
    blocked: Option<RecBlock>,
    z: f32,
) -> Vec<SynthResponse> {
    let mut responses = Vec::new();
    let red = theme::accent::ERROR;
    let id = ui.id().with(("sampler_rec", node_id));
    let button = ui.interact(rect, id, Sense::click());
    let engine_node_id = user_state.get_engine_node_id(node_id);
    // Busy, it can always be pressed again to let go
    let usable = blocked.is_none() || (view.is_busy() && blocked != Some(RecBlock::Jack));

    // Down for at least MIN_PRESS_SECONDS
    let now = ui.input(|i| i.time);
    let pressed_at: Option<f64> = ui.ctx().data(|d| d.get_temp(id));
    let holding = usable && (button.is_pointer_button_down_on() || button.clicked());
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

    // A learned MIDI footswitch shows as a purple badge
    let param_index = Sampler::PARAM_REC_BUTTON;
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
            if let Some(m) = mapping {
                tip.push_str(&format!("\nMIDI CC {} presses it", m.cc_number));
            }
            tip
        }
    };
    let button = button.on_hover_text(format!("{tip}\nRight-click to learn a MIDI footswitch"));
    button.context_menu(|ui| {
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
