//! The Cloud's display: the source as a waveform, and every grain a spark
//! where it's reading.
//!
//! The waveform is the file's overview, or, playing live audio, the last
//! six seconds as the engine sends them, oldest on the left and just now on
//! the right. A soft wash shows where grains may start: Position, as wide
//! as Spray scatters them. Each grain glows at its place in the source,
//! raised or lowered by where it sits in the stereo field, bright as it
//! swells and fading as its window closes, so a dense cloud is a drift of
//! sparks and a sparse one a few slow embers. Frozen, the waveform turns to
//! ice.
//!
//! Under it, the file's name (or what the live audio is doing) and a
//! button to open another file. A file dragged over the node lights the
//! panel up to take it.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::ModuleCategory;
use crate::modules::cloud::{Cloud, SHOWN};
use crate::modules::sampler::Sampler;
use crate::persistence::sample_files;

use super::display_frame::{self, with_alpha};
use super::{SynthGraph, SynthGraphState, SynthResponse, SynthValueType};

/// Frozen live audio's colour: ice.
const ICE: Color32 = Color32::from_rgb(190, 232, 255);

/// A spark at its brightest: nearly white, warmed a little.
const SPARK: Color32 = Color32::from_rgb(255, 246, 228);

/// Draws the display, and returns what the user did with it: a file asked for.
pub fn cloud_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut responses = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return responses };
    let value_of = |name: &str, default: f32| {
        node.inputs
            .iter()
            .find(|(input, _)| input == name)
            .and_then(|(_, id)| match graph.get_input(*id).value {
                SynthValueType::Number { value, .. } => Some(value),
                SynthValueType::Select { value, .. } => Some(value as f32),
                _ => None,
            })
            .unwrap_or(default)
    };
    let spray = value_of("Spray", 0.1).clamp(0.0, 1.0);

    let engine_id = user_state.get_engine_node_id(node_id);
    let readout = engine_id.and_then(|id| user_state.readouts.get(&id)).copied().unwrap_or_default();
    let values = readout.values;
    let live = values[Cloud::READOUT_LIVE] >= 0.5;
    let frozen = live && values[Cloud::READOUT_FROZEN] >= 0.5;
    let filled = values[Cloud::READOUT_FILLED].clamp(0.0, 1.0);
    let playing = values[Cloud::READOUT_COUNT].max(0.0) as usize;
    // Before the engine has said anything, where the knob is
    let position = if engine_id.is_some_and(|id| user_state.readouts.contains_key(&id)) {
        values[Cloud::READOUT_POSITION]
    } else {
        value_of("Position", 0.5)
    }
    .clamp(0.0, 1.0);

    let key = node.user_data.file.as_deref();
    let shelved = key.and_then(|key| user_state.samples.get(key));
    let loaded = shelved.and_then(|entry| entry.as_ref().ok());
    let problem: Option<&str> = shelved.and_then(|entry| entry.as_ref().err()).map(String::as_str);
    let drop_here = user_state.file_drop_target == Some(Some(node_id));

    let width = display_frame::knob_row_width(ui, 4, z);
    let height = 72.0 * z;
    let blue = ModuleCategory::Source.color();
    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    super::marks::mark(ui.ctx(), node_id, "cloud", rect);
    ui.painter().hline(rect.x_range(), rect.top() - 4.0 * z, Stroke::new(1.0 * z, with_alpha(blue, 64)));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0 * z, display_frame::PANEL);

    let plot = Rect::from_min_max(rect.min + Vec2::new(4.0 * z, 12.0 * z), rect.max - Vec2::new(4.0 * z, 4.0 * z));
    let x_at = |place: f32| plot.left() + place.clamp(0.0, 1.0) * plot.width();
    let mid = plot.center().y;
    let half = plot.height() / 2.0;

    // Where grains may start: Position, as wide as Spray scatters them
    let reach = (spray / 2.0).max(0.004);
    let (from, to) = (x_at(position - reach), x_at(position + reach));
    let wash = Rect::from_x_y_ranges(from..=to.max(from + 1.0 * z), plot.y_range());
    painter.rect_filled(wash, 2.0 * z, with_alpha(blue, 22));
    display_frame::guide(&painter, Pos2::new(x_at(position), plot.top()), Pos2::new(x_at(position), plot.bottom()), 60, z);

    // The waveform, as each column's (lowest, highest): the file's, or the live audio's
    let wave = if frozen { ICE } else { blue };
    let columns: Option<Vec<(f32, f32)>> = if live {
        engine_id
            .and_then(|id| user_state.get_scope_data(id))
            .map(|scope| scope.channel1.iter().zip(&scope.channel2).map(|(&high, &low)| (low, high)).collect())
    } else {
        loaded.map(|sample| sample.overview.clone())
    };
    if let Some(columns) = columns.filter(|c| !c.is_empty()) {
        let loudest = columns.iter().fold(0.0f32, |m, &(lo, hi)| m.max(hi.abs()).max(lo.abs())).max(0.05);
        let n = columns.len();
        let shown = (plot.width() / (1.5 * z)).max(16.0) as usize;
        for k in 0..shown {
            // The columns under this one, at their widest
            let a = k * n / shown;
            let b = ((k + 1) * n / shown).max(a + 1).min(n);
            let (lo, hi) = columns[a..b].iter().fold((0.0f32, 0.0f32), |(l, h), &(lo, hi)| (l.min(lo), h.max(hi)));
            let x = plot.left() + (k as f32 + 0.5) / shown as f32 * plot.width();
            let top = mid - (hi / loudest).clamp(-1.0, 1.0) * half * 0.9;
            let bottom = mid - (lo / loudest).clamp(-1.0, 1.0) * half * 0.9;
            // Brighter near where grains read
            let near = 1.0 - ((x - x_at(position)).abs() / plot.width() / reach.max(0.08)).min(1.0);
            let alpha = (70.0 + 80.0 * near) as u8;
            painter.line_segment([Pos2::new(x, top.min(mid - 0.5 * z)), Pos2::new(x, bottom.max(mid + 0.5 * z))], Stroke::new(1.0 * z, with_alpha(wave, alpha)));
        }
    } else {
        painter.hline(plot.x_range(), mid, Stroke::new(1.0 * z, with_alpha(blue, 50)));
    }
    if live && !frozen && filled < 1.0 {
        // Still filling: what's to come is dark
        let edge = x_at(filled);
        painter.rect_filled(Rect::from_x_y_ranges(edge..=plot.right(), plot.y_range()), 0.0, with_alpha(display_frame::PANEL, 200));
    }
    if frozen {
        painter.rect_stroke(rect.shrink(0.5 * z), 3.0 * z, Stroke::new(1.0 * z, with_alpha(ICE, 120)), egui::StrokeKind::Middle);
    }

    // Every grain a spark: its place across, its pan up and down
    let mut sparks = Vec::new();
    for k in 0..SHOWN {
        if let Some((place, level)) = Sampler::unpack_playhead(values[Cloud::READOUT_GRAINS + k]) {
            let pan = values[Cloud::READOUT_PANS + k].clamp(-1.0, 1.0);
            sparks.push((Pos2::new(x_at(place), mid - pan * half * 0.75), level));
        }
    }
    // Quietest first, so the brightest land on top
    sparks.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (at, level) in sparks {
        let glow = with_alpha(if frozen { ICE } else { blue }, (level * 70.0) as u8);
        painter.circle_filled(at, (3.0 + 4.0 * level) * z, glow);
        let core = lerp(blue, SPARK, level);
        painter.circle_filled(at, (0.8 + 1.4 * level) * z, with_alpha(core, (90.0 + 165.0 * level) as u8));
    }

    // The header: where the grains come from, and how many there are
    let source = match (live, frozen) {
        (true, true) => ("FROZEN".to_string(), ICE),
        (true, false) => ("LIVE".to_string(), theme::text::QUIET),
        _ => ("FILE".to_string(), theme::text::QUIET),
    };
    let count = (playing > 0).then(|| (format!("{playing} grain{}", if playing == 1 { "" } else { "s" }), theme::text::QUIET));
    display_frame::header(&painter, rect, z, Some(source), count);

    if drop_here {
        painter.rect_stroke(rect.shrink(1.0 * z), 3.0 * z, Stroke::new(1.5 * z, theme::text::ACCENT), egui::StrokeKind::Middle);
    }
    if response.clicked() && !live {
        responses.push(SynthResponse::OpenSample(node_id));
    }
    if response.hovered() {
        let tip = match (live, frozen, key) {
            (true, true, _) => "Frozen: playing a held moment of the live audio. Let go of Freeze to listen again".to_string(),
            (true, false, _) => "Playing the last six seconds of what's patched into In, just now on the right".to_string(),
            (false, _, Some(key)) => match loaded {
                Some(sample) => format!("{key}\n{}\nClick to open another sound file", duration(sample.source.seconds())),
                None => format!("{key}\n{}", problem.filter(|p| !sample_files::is_loading(p)).unwrap_or("Loading…")),
            },
            (false, _, None) => "Click to open a WAV, MP3, FLAC or Ogg file, or drag one onto the node; or patch live audio into In".to_string(),
        };
        response.on_hover_text(tip);
    }

    // The file's name, or what the live audio is doing, and Open…
    let row_height = 14.0 * z;
    let (row, _) = ui.allocate_exact_size(Vec2::new(width, row_height), Sense::hover());
    let small = egui::FontId::proportional(8.0 * z);
    let (label, color) = if live {
        let text = if frozen { "Live input, frozen".to_string() } else { format!("Live input, last {} s", crate::modules::cloud::LIVE_SECONDS as u32) };
        (text, theme::text::SECONDARY)
    } else {
        let name = key.map(sample_files::file_name).unwrap_or("No file");
        let color = if problem.is_some_and(|p| !sample_files::is_loading(p)) {
            theme::accent::WARNING
        } else if key.is_some() {
            theme::text::SECONDARY
        } else {
            theme::text::DISABLED
        };
        let text = match loaded {
            Some(sample) => format!("{}  {}", truncate(name, 18), duration(sample.source.seconds())),
            None => truncate(name, 28),
        };
        (text, color)
    };
    ui.painter().text(Pos2::new(row.left() + 2.0 * z, row.center().y), egui::Align2::LEFT_CENTER, label, small.clone(), color);
    let button = Rect::from_min_size(Pos2::new(row.right() - 34.0 * z, row.top() + 1.0 * z), Vec2::new(34.0 * z, row_height - 2.0 * z));
    let open = ui.interact(button, ui.id().with(("cloud_open", node_id)), Sense::click());
    super::marks::mark(ui.ctx(), node_id, "file", button);
    let fill = if open.hovered() { theme::background::WIDGET_HOVERED } else { theme::background::WIDGET };
    ui.painter().rect(button, 3.0 * z, fill, Stroke::new(1.0 * z, theme::background::GRID_MAJOR), egui::StrokeKind::Middle);
    ui.painter().text(button.center(), egui::Align2::CENTER_CENTER, "Open…", small, theme::text::PRIMARY);
    if open.on_hover_text("Open a WAV, MP3, FLAC or Ogg file for this Cloud").clicked() {
        responses.push(SynthResponse::OpenSample(node_id));
    }
    responses
}

/// `a` turned toward `b` by `t`, 0 to 1.
fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// A length to read at a glance: "450 ms", "2.4 s", "3:05".
fn duration(seconds: f32) -> String {
    if seconds < 1.0 {
        format!("{} ms", (seconds * 1000.0).round())
    } else if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{}:{:02}", (seconds / 60.0) as u32, seconds as u32 % 60)
    }
}

/// `text`, cut to `chars` characters with an ellipsis if longer.
fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(chars - 1).collect::<String>())
    }
}
