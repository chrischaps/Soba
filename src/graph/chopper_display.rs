//! The Chopper's face: the break as a waveform, its slices each in a hue
//! of their own, and under it the row of steps, each tinted like the slice
//! it plays.
//!
//! Left alone, the row runs through the hues in order, a spectrum from end
//! to end; rearranged, the colours jump about, so a chopped pattern can be
//! read at a glance: which slices come back, which never play, where the
//! break doubles back on itself. A fan of faint rules joins each slice to
//! its step: straight down when the slices are equal, leaning when they're
//! cut at Onsets, where the steps stay evenly spaced but the hits don't.
//!
//! - A step that rests is an empty outline.
//! - Backwards, it carries an arrow pointing back; repeated, it splits into
//!   its hits; at another pitch, it shows the semitones; with a Chance, a
//!   die's dot in its corner, as big as the chance.
//! - While it plays, the slice heard glows on the waveform, a playhead
//!   rides it, and a tick on the row shows where the loop is in time.
//! - A row of two, three or four steps per slice wraps into a line for each
//!   pass over the break, the fan joining the slices to the first; the
//!   lines below read straight down as the same slices' later turns.
//!
//! Click a step to rest it; drag it up or down to choose its slice;
//! Shift+click repeats, Alt+click reverses, Ctrl+click sets its chance, and
//! right-click has all of those and its pitch. Every edit is one undo step.
//!
//! Sliced by Onsets, the markers drag as on the Sampler (see
//! [`super::sampler_slices`]). Alt-click a slice to hear just that slice,
//! or let go of a marker with Alt held to hear the slice it now begins.
//! Under it all: ▶ to hear the whole break, its name and the tempo it
//! plays at unchanged, Rec, and Open…. Rec records what's patched into In
//! as the Sampler's does, the take drawn in red across the panel as it
//! comes in (see [`super::sampler_take`]); once it ends, it's the break.

use eframe::egui::{self, ecolor::Hsva, Color32, CursorIcon, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::{InputId, NodeId};

use crate::app::theme;
use crate::dsp::onsets::DEFAULT_SENSITIVITY;
use crate::dsp::primitives::{Markers, Slices};
use crate::modules::chopper::{Chopper, Step, MAX_PITCH, MAX_REPEAT, MAX_STEPS};
use crate::modules::sampler::{RecState, Sampler, NO_THRESHOLD_DB};
use crate::persistence::sample_files;

use super::hovered_file;
use super::sample_shelf::ShelvedSample;
use super::sampler_slices::SliceMarkers;
use super::sampler_take::{self, RecView};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// How close (in points) the pointer must be to grab a marker.
const GRAB: f32 = 6.0;
/// The chances Ctrl+click steps through.
const CHANCES: [f32; 4] = [0.0, 25.0, 50.0, 100.0];
/// Points dragged for each slice a step moves on.
const DRAG_PER_SLICE: f32 = 7.0;

/// The hue slice `index` of `count` wears: a spectrum from the Source blue
/// round through violet, rose and gold to green, short of coming back to
/// blue, so the first slice and the last never look alike.
pub fn slice_color(index: usize, count: usize) -> Color32 {
    let along = index as f32 / count.max(1) as f32;
    Hsva::new((0.58 + 0.82 * along).fract(), 0.5, 0.92, 1.0).into()
}

/// The node's parameters, by index (see [`Chopper`]'s `PARAM_*`).
struct Params<'a> {
    graph: &'a SynthGraph,
    inputs: &'a [(String, InputId)],
}

impl<'a> Params<'a> {
    fn of(graph: &'a SynthGraph, node_id: NodeId) -> Option<Self> {
        let node = graph.nodes.get(node_id)?;
        let first = node.inputs.iter().position(|(name, _)| name == "Pulse")?;
        let inputs = &node.inputs[first..];
        (inputs.len() >= Chopper::PARAM_COUNT).then_some(Self { graph, inputs })
    }

    fn value(&self, index: usize) -> f32 {
        self.graph.get_input(self.inputs[index].1).value.actual_value()
    }

    fn name(&self, index: usize) -> String {
        self.inputs[index].0.clone()
    }

    fn step(&self, index: usize) -> Step {
        Step::decode(self.value(Chopper::step_param(index)))
    }

    fn chance(&self, index: usize) -> f32 {
        self.value(Chopper::chance_param(index)).clamp(0.0, 100.0)
    }

    fn set_step(&self, index: usize, to: Step) -> (String, f32) {
        (self.name(Chopper::step_param(index)), to.encode())
    }

    fn set_chance(&self, index: usize, to: f32) -> (String, f32) {
        (self.name(Chopper::chance_param(index)), to)
    }
}

fn edit(node_id: NodeId, label: impl Into<String>, changes: Vec<(String, f32)>) -> SynthResponse {
    SynthResponse::EditParameters { node_id, label: label.into(), changes }
}

/// What the engine says is playing.
#[derive(Clone, Copy, Default)]
struct Playing {
    /// The playhead's place in the file, and how loud.
    playhead: Option<(f32, f32)>,
    step: Option<usize>,
    slice: Option<usize>,
    /// Where the loop is, 0 to 1.
    phase: Option<f32>,
}

impl Playing {
    fn read(user_state: &SynthGraphState, node_id: NodeId) -> Self {
        let Some(readout) = user_state.get_engine_node_id(node_id).and_then(|id| user_state.readouts.get(&id)) else {
            return Self::default();
        };
        let from_one = |value: f32| (value >= 1.0).then(|| value.round() as usize - 1);
        let phase = readout.values[Chopper::READOUT_PHASE];
        Self {
            playhead: Sampler::unpack_playhead(readout.values[Chopper::READOUT_PLAYHEAD]),
            step: from_one(readout.values[Chopper::READOUT_STEP]),
            slice: from_one(readout.values[Chopper::READOUT_SLICE]),
            phase: (phase >= 0.0 && user_state.is_playing).then_some(phase),
        }
    }
}

/// Draws the face, and returns what the user did with it.
pub fn chopper_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut responses = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return responses };
    let Some(params) = Params::of(graph, node_id) else { return responses };

    let key = node.user_data.file.as_deref();
    let shelved = key.and_then(|key| user_state.samples.get(key));
    let loaded: Option<&ShelvedSample> = shelved.and_then(|entry| entry.as_ref().ok());
    let problem: Option<&str> = shelved.and_then(|entry| entry.as_ref().err()).map(String::as_str);
    let drop_here = user_state.file_drop_target == Some(Some(node_id));
    // A file dragged over, once it's read, stands in for the one it has
    let hovered = user_state.hovered_file.as_ref().filter(|_| drop_here).and_then(|hovered| Some((hovered.key.as_str(), hovered.sample()?)));
    let hovered_cue = user_state.cue.as_ref().filter(|cue| cue.hovered && cue.node_id == Some(node_id));
    let loaded = loaded.filter(|_| hovered.is_none());
    let cue = user_state.cue.as_ref().filter(|cue| cue.node_id == Some(node_id) && loaded.is_some());

    // How it's cut: equally, or where its hits begin (or where they were moved to)
    let slices_value = params.value(Chopper::PARAM_SLICES);
    let sensitivity = params.value(Chopper::PARAM_SENSITIVITY);
    let sensitivity = if sensitivity.is_finite() { sensitivity } else { DEFAULT_SENSITIVITY };
    let places = match loaded {
        Some(sample) if Slices::is_marked_param(slices_value) => sample.onsets().markers(node.user_data.slice_markers.as_ref(), sensitivity),
        _ => Vec::new(),
    };
    let cutting = Markers::between(&places, 0.0, 1.0);
    let slices = Chopper::slices(slices_value, cutting.as_slice());
    let count = slices.count();
    let slice_markers = slices.is_marked().then(|| SliceMarkers { node_id, places: &places, cutting, sensitivity });
    let span_of = |index: usize| {
        let (from, to) = slices.span(index, 0.0, 1.0);
        (from as f32, to as f32)
    };
    let playing = Playing::read(user_state, node_id);

    // Recording: what it's doing, and whether Rec can start a take now
    let engine_id = user_state.get_engine_node_id(node_id);
    let rec = RecView::read(engine_id.and_then(|id| user_state.readouts.get(&id)), NO_THRESHOLD_DB);
    let live = rec.state == RecState::Recording;
    let (in_patched, blocked) = sampler_take::rec_block(graph, node_id, user_state, &rec);
    // While a take comes in, the panel is its: the old break waits
    let shown = loaded.filter(|_| !live);

    // As wide as the knob row below: four 44-pt columns
    let gap = ui.spacing().item_spacing.x;
    let width = 4.0 * 44.0 * z + 3.0 * gap - 8.0 * z;
    let wave_height = 58.0 * z;
    let fan_height = 9.0 * z;
    // A line of steps for each pass over the break
    let passes = Chopper::passes(params.value(Chopper::PARAM_PASSES), count);
    let steps = passes * count;
    let line_height = if passes > 1 { 14.0 * z } else { 17.0 * z };
    let line_gap = 4.0 * z;
    let row_height = passes as f32 * line_height + (passes - 1) as f32 * line_gap;

    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, wave_height + fan_height + row_height), Sense::hover());
    let blue = crate::dsp::ModuleCategory::Source.color();
    ui.painter().hline(rect.x_range(), rect.top() - 4.0 * z, Stroke::new(1.0 * z, blue.gamma_multiply(0.25)));
    let wave_rect = Rect::from_min_size(rect.min, Vec2::new(width, wave_height));
    let row_rect = Rect::from_min_size(Pos2::new(rect.left(), rect.bottom() - row_height), Vec2::new(width, row_height));
    let painter = ui.painter_at(rect.expand(1.0 * z));
    painter.rect_filled(wave_rect, 3.0 * z, Color32::from_rgb(20, 22, 30));

    let plot = wave_rect.shrink2(Vec2::new(3.0 * z, 4.0 * z));
    let x_at = |place: f32| plot.left() + place.clamp(0.0, 1.0) * plot.width();
    let place_at = |x: f32| ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
    let mid = plot.center().y;
    let small = FontId::proportional(8.0 * z);
    let wave = ui.interact(wave_rect, Id::new((node_id, "chopper-wave")), Sense::click_and_drag());
    let _ = response;

    match shown {
        _ if live => {
            let scope = engine_id.and_then(|id| user_state.get_scope_data(id));
            sampler_take::draw_take(&painter, plot, &rec, scope, &format!("REC  {:.1} s", rec.seconds), z);
            ui.ctx().request_repaint();
        }
        Some(sample) => {
            // The slice heard glows
            if let (Some(slice), Some(_)) = (playing.slice.filter(|&s| s < count), playing.playhead) {
                let (from, to) = span_of(slice);
                let band = Rect::from_x_y_ranges(x_at(from)..=x_at(to), plot.y_range());
                painter.rect_filled(band, 0.0, slice_color(slice, count).gamma_multiply(0.16));
            }

            // The waveform, a column at a time, each in its slice's hue
            let columns = (plot.width() / (1.0 * z)).max(16.0) as usize;
            let overview = &sample.overview;
            let tallest = overview.iter().map(|(low, high)| low.abs().max(high.abs())).fold(0.05, f32::max);
            let half = plot.height() / 2.0 * 0.92;
            for column in 0..columns {
                let from = column * overview.len() / columns;
                let to = ((column + 1) * overview.len() / columns).max(from + 1).min(overview.len());
                if from >= to {
                    continue;
                }
                let (low, high) = overview[from..to].iter().fold((0.0f32, 0.0f32), |(l, h), &(a, b)| (l.min(a), h.max(b)));
                let place = (column as f32 + 0.5) / columns as f32;
                let x = plot.left() + place * plot.width();
                let slice = slices.at(place as f64, 0.0, 1.0).unwrap_or(0);
                let hue = slice_color(slice, count);
                let color = if playing.slice == Some(slice) && playing.playhead.is_some() { hue.lerp_to_gamma(Color32::WHITE, 0.3) } else { hue.gamma_multiply(0.72) };
                let top = mid - (high / tallest) * half;
                let bottom = mid - (low / tallest) * half;
                painter.line_segment([Pos2::new(x, top), Pos2::new(x, bottom.max(top + 0.6 * z))], Stroke::new(1.0 * z, color));
            }
            painter.hline(plot.x_range(), mid, Stroke::new(0.5 * z, Color32::from_rgba_unmultiplied(255, 255, 255, 10)));

            // Rules between equal slices (markers draw their own), numbered while there's room
            let rule = Color32::WHITE.gamma_multiply(0.08 + 0.12 * (plot.width() / count as f32 / (16.0 * z)).min(1.0));
            for index in 0..count {
                let (from, to) = span_of(index);
                if index > 0 && !slices.is_marked() {
                    painter.vline(x_at(from), plot.y_range(), Stroke::new(1.0 * z, rule));
                }
                if x_at(to) - x_at(from) >= 11.0 * z {
                    let inset = if slices.is_marked() { 4.0 * z } else { 2.0 * z };
                    painter.text(
                        Pos2::new(x_at(from) + inset, plot.top() + 1.0 * z),
                        egui::Align2::LEFT_TOP,
                        (index + 1).to_string(),
                        FontId::proportional(7.0 * z),
                        theme::text::SECONDARY.gamma_multiply(0.8),
                    );
                }
            }

            // Where it reads: as bright as it's loud
            if let Some((place, level)) = playing.playhead {
                let x = x_at(place);
                let bright = level.sqrt().clamp(0.0, 1.0);
                painter.vline(x, plot.y_range(), Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.15 + 0.6 * bright)));
                painter.circle_filled(Pos2::new(x, mid), 1.6 * z, Color32::WHITE.gamma_multiply(0.3 + 0.7 * bright));
            }

            // The cue heard from the face
            if let Some(cue) = cue {
                let (from, x) = (x_at(cue.from), x_at(cue.place));
                painter.rect_filled(Rect::from_x_y_ranges(from.min(x)..=from.max(x), plot.y_range()), 0.0, Color32::WHITE.gamma_multiply(0.07));
                painter.vline(x, plot.y_range(), Stroke::new(1.2 * z, Color32::WHITE));
            }
        }
        None if hovered.is_some() => {
            if let Some((key, sample)) = hovered {
                hovered_file::draw(&painter, wave_rect, plot, sample, key, (0.0, 1.0), hovered_cue, z);
            }
        }
        None => {
            let outline = plot.shrink(2.0 * z);
            let color = if drop_here { blue } else { theme::text::DISABLED.gamma_multiply(0.6) };
            let corners = [outline.left_top(), outline.right_top(), outline.right_bottom(), outline.left_bottom(), outline.left_top()];
            for side in corners.windows(2) {
                painter.extend(Shape::dashed_line(side, Stroke::new(1.0 * z, color), 4.0 * z, 3.2 * z));
            }
            let (hint, hint_color) = match (key, problem) {
                (Some(key), Some(problem)) if sample_files::is_loading(problem) => (format!("Loading {}…", sample_files::file_name(key)), theme::text::SECONDARY),
                (Some(key), Some(problem)) => (format!("{}: {problem}", sample_files::file_name(key)), theme::accent::WARNING),
                _ if in_patched => ("Press Rec to record a break".to_string(), if drop_here { blue } else { theme::text::SECONDARY }),
                _ => ("Drop a break here".to_string(), if drop_here { blue } else { theme::text::SECONDARY }),
            };
            let under = if in_patched && problem.is_none() { "or drop a loop here, or click to open one" } else { "an audio loop, or click to open one" };
            painter.text(outline.center() - Vec2::new(0.0, 5.0 * z), egui::Align2::CENTER_CENTER, hint, FontId::proportional(9.0 * z), hint_color);
            painter.text(outline.center() + Vec2::new(0.0, 7.0 * z), egui::Align2::CENTER_CENTER, under, small.clone(), theme::text::DISABLED);
        }
    }

    // Waiting to record, for the next bar
    if matches!(rec.state, RecState::Armed | RecState::Waiting) {
        sampler_take::draw_waiting(&painter, wave_rect, plot, &rec, ui.input(|i| i.time), z);
        ui.ctx().request_repaint();
    }

    if drop_here {
        painter.rect_stroke(wave_rect.shrink(0.5 * z), 3.0 * z, Stroke::new(1.5 * z, blue));
        if loaded.is_some() {
            painter.rect_filled(wave_rect, 3.0 * z, blue.gamma_multiply(0.11));
            painter.text(wave_rect.center(), egui::Align2::CENTER_CENTER, "Drop to replace", FontId::proportional(10.0 * z), Color32::WHITE);
        }
    }

    // The markers, sliced by Onsets: drag to move, off the panel to take away
    let pointer = wave.hover_pos().or(wave.interact_pointer_pos());
    let slice_dragging = slice_markers.as_ref().is_some_and(|markers| markers.dragging(ui));
    let slice_hot = match (&slice_markers, shown, pointer, slice_dragging) {
        (Some(markers), Some(_), Some(p), false) => markers.nearest(p.x, x_at, GRAB * z).map(|(m, _)| m),
        _ => None,
    };
    if let (Some(markers), Some(_)) = (&slice_markers, shown) {
        markers.draw(ui, &painter, wave_rect, plot, x_at, slice_hot, z);
    }
    let alt = ui.input(|i| i.modifiers.alt);
    if live {
        wave.clone().on_hover_text("Recording what's patched into In: press Rec to stop, and the take is the break from the top");
    } else if loaded.is_some() {
        if let (Some(markers), true) = (&slice_markers, wave.drag_started()) {
            let pressed = ui.input(|i| i.pointer.press_origin()).or(wave.interact_pointer_pos());
            if let Some((m, _)) = pressed.and_then(|p| markers.nearest(p.x, x_at, GRAB * z)) {
                markers.pick_up(ui, m);
            }
        }
        if let (Some(markers), Some(p), true) = (&slice_markers, wave.interact_pointer_pos(), wave.dragged()) {
            responses.extend(markers.drag(ui, wave_rect, p, place_at(p.x), z));
        }
        if let (Some(markers), true) = (&slice_markers, wave.drag_stopped()) {
            if let (true, Some(index)) = (alt, markers.in_hand_begins(ui)) {
                let (from, to) = span_of(index);
                responses.push(SynthResponse::CueSample { node_id, from, to });
            }
            markers.put_down(ui);
        }
        if let (Some(markers), Some(p), true) = (&slice_markers, wave.interact_pointer_pos(), wave.double_clicked()) {
            if !alt && slice_hot.is_none() {
                responses.push(markers.add(place_at(p.x)));
            }
        }
        // Alt-click a slice to hear just it; a marker in hand, the slice it begins
        let in_hand = slice_markers.as_ref().and_then(|markers| markers.in_hand_begins(ui));
        let pointed = in_hand.or_else(|| pointer.and_then(|p| slices.at(place_at(p.x) as f64, 0.0, 1.0)));
        if let (true, Some(index)) = (alt, pointed) {
            let (from, to) = span_of(index);
            painter.rect_stroke(Rect::from_x_y_ranges(x_at(from)..=x_at(to), plot.y_range()), 1.0 * z, Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.6)));
            if wave.clicked() {
                responses.push(SynthResponse::CueSample { node_id, from, to });
            }
        }
        if slice_dragging || (slice_hot.is_some() && !alt) {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        } else if alt && pointed.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if wave.hovered() && !slice_dragging {
            let tip = match (slice_hot, pointed) {
                (Some(m), _) => {
                    let begins = slice_markers.as_ref().and_then(|markers| markers.slice_of(m));
                    let what = begins.map_or("A marker past the 63 that cut it".to_string(), |slice| format!("Slice {slice} begins here"));
                    let heard = if begins.is_some() { "\nLet go with Alt held to hear the slice" } else { "" };
                    format!("{what}\nDrag to move it, or off the panel to take it away{heard}")
                }
                (None, Some(index)) => {
                    let mut tip = format!("Slice {} of {count}\nAlt-click to hear it", index + 1);
                    if slices.is_marked() {
                        tip.push_str(" · double-click to add a marker");
                    }
                    tip
                }
                _ => String::new(),
            };
            if !tip.is_empty() {
                wave.clone().on_hover_text(tip);
            }
        }
    } else if wave.clicked() {
        responses.push(SynthResponse::OpenSample(node_id));
    } else if !drop_here {
        wave.clone().on_hover_text("Click to open a loop (WAV, MP3 or Ogg), or drag one from your files onto the node");
    }

    // The fan: each slice to its step
    let cell_width = row_rect.width() / count as f32;
    let cell_x = |step: usize| row_rect.left() + (step % count) as f32 * cell_width;
    let line_top = |line: usize| row_rect.top() + line as f32 * (line_height + line_gap);
    // The slice playing, whichever pass it's in
    let slot_playing = playing.step.filter(|_| playing.phase.is_some()).map(|step| step % count);
    let fan_top = wave_rect.bottom();
    let fan_bottom = row_rect.top();
    for index in 0..count {
        let (from, _) = span_of(index);
        let top = Pos2::new(x_at(from), fan_top);
        let bottom = Pos2::new(cell_x(index) + 0.5 * z, fan_bottom);
        let lit = slot_playing == Some(index);
        let color = slice_color(index, count).gamma_multiply(if lit { 0.9 } else { 0.25 });
        if index > 0 || slices.is_marked() {
            painter.line_segment([top, bottom], Stroke::new(if lit { 1.2 * z } else { 0.8 * z }, color));
        }
    }

    // The steps
    let mut edits = Vec::new();
    for index in 0..steps.min(MAX_STEPS) {
        let step = params.step(index);
        let chance = params.chance(index);
        let slice = step.slice_of(index, count);
        let cell = Rect::from_min_size(Pos2::new(cell_x(index), line_top(index / count)), Vec2::new(cell_width, line_height)).shrink2(Vec2::new(0.5 * z, 0.0));
        let here = playing.step == Some(index) && playing.phase.is_some();
        draw_step(&painter, cell, step, slice, count, chance, here, z);

        let response = ui.interact(cell, Id::new((node_id, "chopper-step", index)), Sense::click_and_drag()).on_hover_cursor(CursorIcon::PointingHand);
        let what = format!("Step {}", index + 1);
        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            if modifiers.shift {
                let next = Step { plays: true, repeat: step.repeat % MAX_REPEAT + 1, ..step };
                edits.push(edit(node_id, format!("Repeat ×{} on {what}", next.repeat), vec![params.set_step(index, next)]));
            } else if modifiers.alt {
                let next = Step { plays: true, reverse: !step.reverse, ..step };
                let how = if next.reverse { "Reverse" } else { "Forwards" };
                edits.push(edit(node_id, format!("{how} on {what}"), vec![params.set_step(index, next)]));
            } else if modifiers.command {
                let at = CHANCES.iter().rposition(|&c| c <= chance).unwrap_or(0);
                let next = CHANCES[(at + 1) % CHANCES.len()];
                edits.push(edit(node_id, format!("{next}% chance on {what}"), vec![params.set_chance(index, next)]));
            } else {
                let next = Step { plays: !step.plays, ..step };
                let verb = if next.plays { "Play" } else { "Rest" };
                edits.push(edit(node_id, format!("{verb} {what}"), vec![params.set_step(index, next)]));
            }
        }

        // Dragging up or down picks its slice
        let drag_id = Id::new((node_id, "chopper-drag", index));
        if response.drag_started() {
            ui.ctx().data_mut(|data| data.insert_temp(drag_id, (slice as f32, 0.0f32)));
        }
        if response.dragged() {
            let (from, moved) = ui.ctx().data(|data| data.get_temp::<(f32, f32)>(drag_id)).unwrap_or((slice as f32, 0.0));
            let moved = moved - response.drag_delta().y / z;
            ui.ctx().data_mut(|data| data.insert_temp(drag_id, (from, moved)));
            let to = (from + (moved / DRAG_PER_SLICE).round()).rem_euclid(count as f32) as usize;
            if to != slice {
                let next = Step { plays: true, slice: own_or(index, to, count), ..step };
                edits.push(edit(node_id, format!("{what} plays slice {}", to + 1), vec![params.set_step(index, next)]));
            }
        }

        let pass = if passes > 1 { format!(" (pass {} of {passes})", index / count + 1) } else { String::new() };
        let hint = format!(
            "{what}{pass}: {}\nClick to {} · drag up or down for another slice\nShift+click repeats · Alt+click reverses · Ctrl+click chance",
            describe(step, slice, chance),
            if step.plays { "rest" } else { "play" },
        );
        let response = response.on_hover_text(hint);
        response.context_menu(|ui| step_menu(ui, node_id, &params, index, step, chance, count, steps, &mut edits));
    }

    // Where the loop is, in time, under the line of the pass it's in
    if let Some(phase) = playing.phase {
        let along = phase.clamp(0.0, 1.0) * passes as f32;
        let line = (along as usize).min(passes - 1);
        let x = row_rect.left() + (along - line as f32) * row_rect.width();
        let bottom = line_top(line) + line_height;
        painter.add(Shape::convex_polygon(
            vec![Pos2::new(x - 2.5 * z, bottom + 3.0 * z), Pos2::new(x + 2.5 * z, bottom + 3.0 * z), Pos2::new(x, bottom)],
            Color32::WHITE.gamma_multiply(0.8),
            Stroke::NONE,
        ));
    }
    if playing.phase.is_some() || cue.is_some() {
        ui.ctx().request_repaint();
    }

    // Under it all: ▶, the file's name and tempo, and Open…
    ui.add_space(3.0 * z);
    let line_height = 14.0 * z;
    let (line, _) = ui.allocate_exact_size(Vec2::new(width, line_height), Sense::hover());
    let mut name_left = line.left() + 2.0 * z;
    if loaded.is_some() {
        let play = Rect::from_min_size(Pos2::new(line.left(), line.top() + 1.0 * z), Vec2::new(18.0 * z, line_height - 2.0 * z));
        let hear = ui.interact(play, ui.id().with(("chopper_cue", node_id)), Sense::click());
        let fill = match (cue.is_some(), hear.hovered()) {
            (true, _) => blue.gamma_multiply(0.45),
            (false, true) => theme::background::WIDGET_HOVERED,
            (false, false) => theme::background::WIDGET,
        };
        ui.painter().rect(play, 3.0 * z, fill, Stroke::new(1.0 * z, theme::background::GRID_MAJOR));
        let c = play.center();
        if cue.is_some() {
            ui.painter().rect_filled(Rect::from_center_size(c, Vec2::splat(5.0 * z)), 0.5 * z, theme::text::PRIMARY);
        } else {
            let points = vec![Pos2::new(c.x - 2.5 * z, c.y - 3.5 * z), Pos2::new(c.x + 3.5 * z, c.y), Pos2::new(c.x - 2.5 * z, c.y + 3.5 * z)];
            ui.painter().add(Shape::convex_polygon(points, theme::text::PRIMARY, Stroke::NONE));
        }
        let tip = if cue.is_some() { "Stop" } else { "Hear the break as recorded, beside the patch\nAlt-click a slice to hear just it" };
        if hear.on_hover_text(tip).clicked() {
            responses.push(if cue.is_some() { SynthResponse::StopCue(node_id) } else { SynthResponse::CueSample { node_id, from: 0.0, to: 1.0 } });
        }
        name_left = play.right() + 4.0 * z;
    }
    let button_width = 34.0 * z;
    let button = Rect::from_min_size(Pos2::new(line.right() - button_width, line.top() + 1.0 * z), Vec2::new(button_width, line_height - 2.0 * z));
    let name = key.map(sample_files::file_name).unwrap_or("No file");
    let name_color = if problem.is_some_and(|p| !sample_files::is_loading(p)) {
        theme::accent::WARNING
    } else if key.is_some() {
        theme::text::SECONDARY
    } else {
        theme::text::DISABLED
    };
    // Rec, beside Open…
    let rec_rect = Rect::from_min_size(Pos2::new(button.left() - 22.0 * z, line.top() + 1.0 * z), Vec2::new(18.0 * z, line_height - 2.0 * z));
    let held = params.value(Chopper::PARAM_REC_BUTTON) >= 0.5;
    responses.extend(sampler_take::rec_button(ui, rec_rect, node_id, user_state, &rec, held, Chopper::PARAM_REC_BUTTON, None, blocked, z));
    let room = rec_rect.left() - 4.0 * z - name_left;
    let label = match loaded {
        // Its own tempo: where it plays back unchanged, as long as Beats says
        Some(sample) => {
            let bpm = Chopper::own_tempo(sample.source.seconds(), Chopper::beats(params.value(Chopper::PARAM_BEATS)));
            fit(&painter, &format!("{name} · {bpm:.0} BPM"), &small, room)
        }
        None => fit(&painter, name, &small, room),
    };
    let name_rect = Rect::from_x_y_ranges(name_left..=name_left + room, line.y_range());
    ui.painter().text(Pos2::new(name_left, line.center().y), egui::Align2::LEFT_CENTER, label, small.clone(), name_color);
    if let Some(sample) = loaded {
        let seconds = sample.source.seconds();
        let beats = Chopper::beats(params.value(Chopper::PARAM_BEATS));
        ui.interact(name_rect, Id::new((node_id, "chopper-name")), Sense::hover()).on_hover_text(format!(
            "{}\n{seconds:.2} s as {beats:.0} beats: it plays unchanged at {:.1} BPM, and keeps its pitch at any other",
            key.unwrap_or(name),
            Chopper::own_tempo(seconds, beats),
        ));
    }
    let open = ui.interact(button, ui.id().with(("chopper_open", node_id)), Sense::click());
    let fill = if open.hovered() { theme::background::WIDGET_HOVERED } else { theme::background::WIDGET };
    ui.painter().rect(button, 3.0 * z, fill, Stroke::new(1.0 * z, theme::background::GRID_MAJOR));
    ui.painter().text(button.center(), egui::Align2::CENTER_CENTER, "Open…", small, theme::text::PRIMARY);
    if open.on_hover_text("Open a loop (WAV, MP3 or Ogg) for this Chopper").clicked() {
        responses.push(SynthResponse::OpenSample(node_id));
    }

    responses.extend(edits);
    responses
}

/// The slice field for step `index` playing slice `to` of `count`: 0 for
/// its own, the slice whose turn it takes.
fn own_or(index: usize, to: usize, count: usize) -> u8 {
    if to == index % count.max(1) { 0 } else { (to + 1) as u8 }
}

/// A step, said in words.
fn describe(step: Step, slice: usize, chance: f32) -> String {
    if !step.plays {
        return "rests".to_string();
    }
    let mut text = format!("slice {}", slice + 1);
    if step.reverse {
        text += ", backwards";
    }
    if step.repeat > 1 {
        text += &format!(", ×{}", step.repeat);
    }
    if step.pitch != 0 {
        text += &format!(", {:+} st", step.pitch);
    }
    if chance > 0.0 {
        text += &format!(", {chance:.0}% chance of another");
    }
    text
}

/// Draws one step: its slice's hue, and marks for how it plays it.
#[allow(clippy::too_many_arguments)]
fn draw_step(painter: &egui::Painter, cell: Rect, step: Step, slice: usize, count: usize, chance: f32, here: bool, z: f32) {
    let hue = slice_color(slice, count);
    let round = 2.0 * z.min(cell.width() / 4.0);
    if !step.plays {
        painter.rect_filled(cell, round, theme::background::WIDGET.gamma_multiply(0.5));
        painter.rect_stroke(cell.shrink(0.5 * z), round, Stroke::new(1.0 * z, hue.gamma_multiply(0.35)));
    } else {
        let color = if here { hue.lerp_to_gamma(Color32::WHITE, 0.4) } else { hue.gamma_multiply(0.85) };
        if here {
            painter.rect_filled(cell.expand(1.0 * z), round + 1.0 * z, hue.gamma_multiply(0.3));
        }
        painter.rect_filled(cell, round, color);
        // Repeats split it into their hits
        for cut in 1..step.repeat {
            let x = cell.left() + cell.width() * cut as f32 / step.repeat as f32;
            painter.vline(x, cell.y_range(), Stroke::new(1.2 * z, theme::background::PANEL));
        }
    }
    let ink = if step.plays { Color32::from_black_alpha(190) } else { hue.gamma_multiply(0.6) };
    let c = cell.center();
    if step.reverse && cell.width() >= 5.0 * z {
        // An arrow pointing back, low in the cell
        let (w, y) = ((cell.width() * 0.3).min(3.5 * z), cell.bottom() - 3.5 * z);
        painter.add(Shape::convex_polygon(vec![Pos2::new(c.x - w, y), Pos2::new(c.x + w, y - 2.2 * z), Pos2::new(c.x + w, y + 2.2 * z)], ink, Stroke::NONE));
    }
    // The slice it plays, while there's room, or its pitch if moved
    if cell.width() >= 9.0 * z {
        let text = if step.pitch != 0 { format!("{:+}", step.pitch) } else { (slice + 1).to_string() };
        let y = if step.reverse { c.y - 2.0 * z } else { c.y };
        painter.text(Pos2::new(c.x, y), egui::Align2::CENTER_CENTER, text, FontId::proportional(7.5 * z), ink);
    } else if step.pitch != 0 {
        let y = if step.pitch > 0 { cell.top() + 2.0 * z } else { cell.bottom() - 2.0 * z };
        painter.circle_filled(Pos2::new(c.x, y), 1.0 * z, ink);
    }
    // A die's dot, as big as the chance
    if chance > 0.0 {
        let r = (0.8 + 1.4 * chance / 100.0) * z;
        painter.circle_filled(Pos2::new(cell.right() - r - 0.8 * z, cell.top() + r + 0.8 * z), r, Color32::WHITE.gamma_multiply(0.9));
    }
    if here {
        painter.rect_stroke(cell.expand(0.5 * z), round, Stroke::new(1.2 * z, Color32::WHITE.gamma_multiply(0.85)));
    }
}

/// A step's right-click menu: everything about how it plays.
#[allow(clippy::too_many_arguments)]
fn step_menu(ui: &mut egui::Ui, node_id: NodeId, params: &Params, index: usize, step: Step, chance: f32, count: usize, steps: usize, edits: &mut Vec<SynthResponse>) {
    let what = format!("Step {}", index + 1);
    ui.label(egui::RichText::new(&what).strong());
    ui.separator();
    let set = |label: String, next: Step, edits: &mut Vec<SynthResponse>| edits.push(edit(node_id, label, vec![params.set_step(index, next)]));

    let mut plays = step.plays;
    if ui.checkbox(&mut plays, "Plays").changed() {
        set(format!("{} {what}", if plays { "Play" } else { "Rest" }), Step { plays, ..step }, edits);
    }
    let mut reverse = step.reverse;
    if ui.checkbox(&mut reverse, "Backwards").on_hover_text("Plays its slice from the end (Alt+click)").changed() {
        set(format!("{} on {what}", if reverse { "Reverse" } else { "Forwards" }), Step { plays: true, reverse, ..step }, edits);
    }
    ui.horizontal(|ui| {
        ui.label("Repeat").on_hover_text("Plays its slice again from the top, evenly across its turn (Shift+click)");
        for repeat in 1..=MAX_REPEAT {
            if ui.selectable_label(step.repeat == repeat, format!("×{repeat}")).clicked() {
                set(format!("Repeat ×{repeat} on {what}"), Step { plays: true, repeat, ..step }, edits);
                ui.close_menu();
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Pitch").on_hover_text("Semitones, tape-style: higher plays faster and leaves a gap, lower is cut short by the next step");
        for pitch in [-MAX_PITCH, -7, -5, 0, 5, 7, MAX_PITCH] {
            let label = if pitch == 0 { "0".to_string() } else { format!("{pitch:+}") };
            if ui.selectable_label(step.pitch == pitch, label).clicked() {
                set(format!("Pitch {pitch:+} on {what}"), Step { plays: true, pitch, ..step }, edits);
                ui.close_menu();
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Chance").on_hover_text("How often it swaps its slice for a random one, so the break varies itself (Ctrl+click)");
        for next in CHANCES.into_iter().chain([10.0]) {
            if ui.selectable_label((chance - next).abs() < 0.5, format!("{next:.0}%")).clicked() {
                edits.push(edit(node_id, format!("{next:.0}% chance on {what}"), vec![params.set_chance(index, next)]));
                ui.close_menu();
            }
        }
    });
    ui.separator();
    ui.label("Plays slice");
    let current = step.slice_of(index, count);
    egui::Grid::new(("chopper-slices", node_id, index)).spacing([2.0, 2.0]).show(ui, |ui| {
        for slice in 0..count {
            let text = egui::RichText::new(format!("{:>2}", slice + 1)).color(slice_color(slice, count));
            if ui.selectable_label(current == slice, text).clicked() {
                set(format!("{what} plays slice {}", slice + 1), Step { plays: true, slice: own_or(index, slice, count), ..step }, edits);
                ui.close_menu();
            }
            if (slice + 1) % 8 == 0 {
                ui.end_row();
            }
        }
    });
    ui.separator();
    if ui.add_enabled(!step.is_plain(index, count) || chance > 0.0, egui::Button::new(format!("Back to slice {}, as it was", index % count + 1))).clicked() {
        edits.push(edit(node_id, format!("Reset {what}"), vec![params.set_step(index, Step::DEFAULT), params.set_chance(index, 0.0)]));
        ui.close_menu();
    }
    ui.separator();
    // The pass it's in, and the whole row
    let first = index / count * count;
    let pass = first / count + 1;
    if first > 0 {
        let before = pass - 1;
        if ui.button(format!("Same as pass {before}")).on_hover_text("Copies every step of the pass before into this one, to vary from there").clicked() {
            let mut changes: Vec<(String, f32)> = (first..first + count).map(|k| params.set_step(k, params.step(k - count))).collect();
            changes.extend((first..first + count).map(|k| params.set_chance(k, params.chance(k - count))));
            edits.push(edit(node_id, format!("Pass {pass} as pass {before}"), changes));
            ui.close_menu();
        }
    }
    let shuffle = if steps > count { format!("Shuffle the slices in pass {pass}") } else { "Shuffle the slices".to_string() };
    if ui.button(&shuffle).on_hover_text("Every step plays a slice at random, each slice once").clicked() {
        let order = shuffled(count, ui.input(|i| i.time).to_bits());
        let changes = (first..first + count).map(|k| params.set_step(k, Step { plays: true, slice: own_or(k, order[k - first], count), ..params.step(k) })).collect();
        edits.push(edit(node_id, shuffle, changes));
        ui.close_menu();
    }
    if ui.button("Reset every step").on_hover_text("Back to the break as it was: each step its own slice, once, forwards").clicked() {
        let mut changes: Vec<(String, f32)> = (0..steps).map(|k| params.set_step(k, Step::DEFAULT)).collect();
        changes.extend((0..steps).map(|k| params.set_chance(k, 0.0)));
        edits.push(edit(node_id, "Reset every step", changes));
        ui.close_menu();
    }
}

/// `0..count` in an order drawn from `seed`.
fn shuffled(count: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).collect();
    let mut x = seed | 1;
    for i in (1..count).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        order.swap(i, (x % (i as u64 + 1)) as usize);
    }
    order
}

/// Shortens text to fit a width, with an ellipsis.
fn fit(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> String {
    let wide = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    if wide(text) <= width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let shorter: String = chars.iter().collect::<String>() + "…";
        if wide(&shorter) <= width {
            return shorter;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_slice_has_its_own_hue_and_the_ends_differ() {
        let colors: Vec<Color32> = (0..16).map(|i| slice_color(i, 16)).collect();
        for pair in colors.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        let (first, last) = (Hsva::from(colors[0]), Hsva::from(colors[15]));
        let apart = (first.h - last.h).abs();
        assert!(apart.min(1.0 - apart) > 0.1, "the first and last slices look alike");
    }

    #[test]
    fn test_a_shuffle_plays_every_slice_once() {
        for seed in [1, 42, 9_999_999] {
            let mut order = shuffled(16, seed);
            assert_ne!(order, (0..16).collect::<Vec<_>>());
            order.sort();
            assert_eq!(order, (0..16).collect::<Vec<_>>());
        }
    }

    #[test]
    fn test_a_step_names_its_own_slice_as_none() {
        assert_eq!(own_or(3, 3, 16), 0);
        assert_eq!(own_or(3, 0, 16), 1);
        assert_eq!(Step { slice: own_or(5, 9, 16), ..Step::DEFAULT }.slice_of(5, 16), 9);
        // A step in the second pass owns the slice whose turn it takes
        assert_eq!(own_or(19, 3, 16), 0);
        assert_eq!(Step { slice: own_or(19, 4, 16), ..Step::DEFAULT }.slice_of(19, 16), 4);
    }
}
