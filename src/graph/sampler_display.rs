//! The Sampler's display: the recording as a waveform, what plays of it,
//! and every voice playing it.
//!
//! The waveform is drawn from an overview made when the file loads, a
//! column at a time. Inside Start–End it's lit in the Source blue; outside,
//! it sinks into the background. When a loop is on, its span is washed in
//! the Control orange, which is the loop's colour on the markers too. All
//! four markers drag like knobs.
//!
//! Each voice draws a playhead where it is in the recording, as bright as
//! it's loud, so a chord is several lines moving at their own speeds and a
//! released note fades where it stands.
//!
//! Sliced, Start–End is ruled into its slices, numbered while they're wide
//! enough to read, and each slice glows while a voice plays it. A loop then
//! goes round each slice, so the loop's markers step aside. Sliced by
//! Onsets, the rules are markers where its hits begin, which drag (see
//! [`super::sampler_slices`]). Under the waveform: the file's
//! name, and a button to open another. With no file, the panel asks for
//! one, and a file dragged over the node lights it up to take it.
//!
//! ▶ beside the name plays the file from Start to End, beside the patch,
//! whether or not the transport runs; Alt-click plays it from anywhere.
//! Sliced, Alt-click plays just the slice it's on, outlined while Alt is
//! held, and letting go of a dragged marker with Alt held plays the slice
//! it now begins, so nudging a marker is a listen-and-adjust loop.
//! That cue has a playhead of its own: a plain white line capped at both
//! ends, unlike the voices', with what it has played washed faintly behind it.
//!
//! Rec, beside Open…, records what's patched into In; while it does, the
//! panel shows the take coming in (see [`super::sampler_take`]).
//!
//! The file's name opens a menu to trim the file to its sound or normalize
//! its level, and the waveform shows what each would do while it's hovered
//! (see [`super::sampler_tidy`]).

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use crate::app::theme;
use crate::dsp::onsets::DEFAULT_SENSITIVITY;
use crate::dsp::primitives::{Markers, Slices, MAX_SLICES};
use crate::dsp::MAX_CHANNELS;
use crate::modules::sampler::{LoopMode, RecState, Sampler, NO_THRESHOLD_DB};
use crate::modules::sequencer::note_to_name;
use crate::persistence::sample_files;

use super::hovered_file;
use super::sample_shelf::ShelvedSample;
use super::sampler_slices::SliceMarkers;
use super::sampler_take::{self, RecButton, RecView, REC_BUTTON};
use super::sampler_tidy;
use super::{SynthGraph, SynthGraphState, SynthResponse, SynthValueType};

/// The four markers, as (parameter, short name).
const MARKERS: [(&str, &str); 4] = [("Start", "Start"), ("End", "End"), ("Loop Start", "Loop start"), ("Loop End", "Loop end")];

/// How close (in points) the pointer must be to grab a marker.
const GRAB: f32 = 6.0;

/// Draws the display, and returns what the user did with it: markers
/// dragged, or a file asked for.
pub fn sampler_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let z = zoom;
    let mut responses = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return responses };
    let value_of = |name: &str| {
        node.inputs.iter().find(|(input, _)| input == name).and_then(|(_, id)| match graph.get_input(*id).value {
            SynthValueType::Number { value, .. } => Some(value),
            SynthValueType::Select { value, .. } => Some(value as f32),
            _ => None,
        })
    };
    let start = value_of("Start").unwrap_or(0.0).clamp(0.0, 1.0);
    let end = value_of("End").unwrap_or(1.0).clamp(0.0, 1.0);
    let (lo, hi) = if end < start { (end, start) } else { (start, end) };
    let loop_mode = LoopMode::from_param(value_of("Loop").unwrap_or(0.0));
    let loop_start = value_of("Loop Start").unwrap_or(0.0).clamp(lo, hi);
    let loop_end = value_of("Loop End").unwrap_or(1.0).clamp(lo, hi);
    let (loop_lo, loop_hi) = if loop_end < loop_start { (loop_end, loop_start) } else { (loop_start, loop_end) };
    let slices_value = value_of("Slices").unwrap_or(1.0);
    let root = value_of("Root").unwrap_or(60.0).round();
    // Sliced, each slice is its own loop: the loop's markers have nothing to say
    let looping = loop_mode != LoopMode::Off && loop_hi > loop_lo && slices_value.round() < 2.0;

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

    // Sliced by Onsets, it's cut where its hits begin, or where markers were moved to
    let sensitivity = value_of("Sensitivity").unwrap_or(DEFAULT_SENSITIVITY);
    let places = match loaded {
        Some(sample) if Slices::is_marked_param(slices_value) => sample.onsets().markers(node.user_data.slice_markers.as_ref(), sensitivity),
        _ => Vec::new(),
    };
    let slices = Slices::from_param(slices_value, &places, lo, hi);
    let slice_markers = slices.filter(Slices::is_marked).map(|_| SliceMarkers { node_id, places: &places, cutting: Markers::between(&places, lo, hi), sensitivity });

    // As wide as the knob rows below: four 44-pt columns
    let gap = ui.spacing().item_spacing.x;
    let width = 4.0 * 44.0 * z + 3.0 * gap - 8.0 * z;
    let height = 72.0 * z;

    // Separator, as the other displays have
    ui.add_space(8.0 * z);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let blue = crate::dsp::ModuleCategory::Source.color();
    ui.painter().hline(
        rect.x_range(),
        rect.top() - 4.0 * z,
        Stroke::new(1.0 * z, Color32::from_rgba_unmultiplied(blue.r(), blue.g(), blue.b(), 64)),
    );
    let painter = ui.painter_at(rect.expand(1.0 * z));
    painter.rect_filled(rect, 3.0 * z, Color32::from_rgb(20, 22, 30));

    let plot = rect.shrink2(Vec2::new(3.0 * z, 4.0 * z));
    let x_at = |place: f32| plot.left() + place.clamp(0.0, 1.0) * plot.width();
    let place_at = |x: f32| ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
    let mid = plot.center().y;
    let orange = theme::signal::CONTROL;
    let small = egui::FontId::proportional(8.0 * z);

    // How loud each slice is playing, from the voices playing it
    let engine_id = user_state.get_engine_node_id(node_id);
    let readout = engine_id.and_then(|id| user_state.readouts.get(&id)).copied();

    // Recording: what it's doing, and whether Rec can start a take now
    let rec = RecView::read(readout.as_ref(), value_of("Rec Threshold").unwrap_or(NO_THRESHOLD_DB));
    let live = rec.state == RecState::Recording;
    let (in_patched, blocked) = sampler_take::rec_block(graph, node_id, user_state, &rec);
    // While a take comes in, the panel is its: the old recording waits
    let shown = loaded.filter(|_| !live);
    let mut lit = [0.0f32; MAX_SLICES];
    if let (Some(readout), Some(slices)) = (readout, slices) {
        for c in 0..MAX_CHANNELS {
            let slice = readout.values[Sampler::READOUT_SLICES + c].round() as usize;
            let level = Sampler::unpack_playhead(readout.values[c]).map_or(0.0, |(_, level)| level.sqrt());
            if (1..=slices.count()).contains(&slice) {
                lit[slice - 1] = lit[slice - 1].max(level);
            }
        }
    }
    let slice_at = |place: f32| slices.and_then(|slices| slices.at(place as f64, lo as f64, hi as f64));
    let span_of = |slices: Slices, index: usize| {
        let (from, to) = slices.span(index, lo as f64, hi as f64);
        (from as f32, to as f32)
    };

    match shown {
        Some(sample) => {
            // What plays, against what doesn't
            let outside = Color32::from_rgba_unmultiplied(0, 0, 0, 90);
            if looping {
                let band = Rect::from_x_y_ranges(x_at(loop_lo)..=x_at(loop_hi), plot.y_range());
                painter.rect_filled(band, 0.0, orange.gamma_multiply(0.1));
            }
            // A slice glows while it plays
            if let Some(slices) = slices {
                for (index, &level) in lit.iter().enumerate().take(slices.count()).filter(|(_, &level)| level > 0.0) {
                    let (from, to) = span_of(slices, index);
                    let band = Rect::from_x_y_ranges(x_at(from)..=x_at(to), plot.y_range());
                    painter.rect_filled(band, 0.0, blue.gamma_multiply(0.08 + 0.17 * level));
                }
            }

            // The waveform, a column at a time, scaled to fill the panel
            let columns = (plot.width() / (1.0 * z)).max(16.0) as usize;
            let overview = &sample.overview;
            let tallest = overview.iter().map(|(low, high)| low.abs().max(high.abs())).fold(0.05, f32::max);
            let half = plot.height() / 2.0 * 0.92;
            let gray = Color32::from_rgb(70, 80, 100);
            for column in 0..columns {
                let from = column * overview.len() / columns;
                let to = ((column + 1) * overview.len() / columns).max(from + 1).min(overview.len());
                if from >= to {
                    continue;
                }
                let (low, high) = overview[from..to].iter().fold((0.0f32, 0.0f32), |(l, h), &(a, b)| (l.min(a), h.max(b)));
                let place = (column as f32 + 0.5) / columns as f32;
                let x = plot.left() + place * plot.width();
                let inside = place >= lo && place <= hi;
                let in_loop = looping && place >= loop_lo && place <= loop_hi;
                let glow = slice_at(place).map_or(0.0, |index| lit[index]);
                let color = match (inside, in_loop) {
                    (true, _) if glow > 0.0 => blue.lerp_to_gamma(Color32::WHITE, 0.25 * glow),
                    (true, true) => blue.gamma_multiply(0.95),
                    (true, false) => blue.gamma_multiply(0.75),
                    _ => gray.gamma_multiply(0.6),
                };
                let top = mid - (high / tallest) * half;
                let bottom = mid - (low / tallest) * half;
                painter.line_segment([Pos2::new(x, top), Pos2::new(x, bottom.max(top + 0.6 * z))], Stroke::new(1.0 * z, color));
            }
            painter.hline(plot.x_range(), mid, Stroke::new(0.5 * z, Color32::from_rgba_unmultiplied(255, 255, 255, 10)));
            // Dim what doesn't play
            if lo > 0.0 {
                painter.rect_filled(Rect::from_x_y_ranges(plot.left()..=x_at(lo), plot.y_range()), 0.0, outside);
            }
            if hi < 1.0 {
                painter.rect_filled(Rect::from_x_y_ranges(x_at(hi)..=plot.right(), plot.y_range()), 0.0, outside);
            }

            // The slices: a rule between each (or a marker, drawn with the
            // others), and its number while there's room
            if let Some(slices) = slices {
                let width = (x_at(hi) - x_at(lo)) / slices.count() as f32;
                // Rules close together fade, so 64 slices are a texture rather than a fence
                let rule = Color32::WHITE.gamma_multiply(0.08 + 0.17 * (width / (16.0 * z)).min(1.0));
                // Clear of a marker's tab
                let inset = if slices.is_marked() { 4.0 * z } else { 2.0 * z };
                for index in 0..slices.count() {
                    let (from, to) = span_of(slices, index);
                    if index > 0 && !slices.is_marked() {
                        painter.vline(x_at(from), plot.y_range(), Stroke::new(1.0 * z, rule));
                    }
                    // Markers cut slices of their own widths
                    if x_at(to) - x_at(from) >= 11.0 * z {
                        let color = if lit[index] > 0.0 { Color32::WHITE } else { theme::text::SECONDARY.gamma_multiply(0.8) };
                        painter.text(
                            Pos2::new(x_at(from) + inset, plot.top() + 1.0 * z),
                            egui::Align2::LEFT_TOP,
                            (index + 1).to_string(),
                            egui::FontId::proportional(7.0 * z),
                            color,
                        );
                    }
                }
            }

            // The voices playing, each as bright as it's loud
            if let Some(readout) = readout {
                for value in &readout.values[..MAX_CHANNELS] {
                    let value = *value;
                    let Some((place, level)) = Sampler::unpack_playhead(value) else { continue };
                    let x = x_at(place);
                    let bright = level.sqrt().clamp(0.0, 1.0);
                    painter.vline(x, plot.y_range(), Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.15 + 0.6 * bright)));
                    painter.circle_filled(Pos2::new(x, mid), 3.5 * z, blue.gamma_multiply(0.35 * bright));
                    painter.circle_filled(Pos2::new(x, mid), 1.6 * z, Color32::WHITE.gamma_multiply(0.3 + 0.7 * bright));
                }
            }

            // The cue heard from the face: what it has played, and where it is
            if let Some(cue) = cue {
                let (from, x) = (x_at(cue.from), x_at(cue.place));
                painter.rect_filled(Rect::from_x_y_ranges(from.min(x)..=from.max(x), plot.y_range()), 0.0, Color32::WHITE.gamma_multiply(0.07));
                painter.vline(x, plot.y_range(), Stroke::new(1.2 * z, Color32::WHITE));
                for (y, inward) in [(plot.top(), 1.0), (plot.bottom(), -1.0)] {
                    painter.add(Shape::convex_polygon(
                        vec![Pos2::new(x - 3.0 * z, y), Pos2::new(x + 3.0 * z, y), Pos2::new(x, y + inward * 4.0 * z)],
                        Color32::WHITE,
                        Stroke::NONE,
                    ));
                }
            }

            // A trim or a level, hovered in the file's menu
            if let Some(tidy) = sampler_tidy::previewing(ui, node_id) {
                sampler_tidy::draw_preview(&painter, plot, sample, tidy, z);
            }
            // A take just tidied as it was kept, and what that did
            if let Some((said, at)) = user_state.tidied_takes.get(&node_id) {
                if sampler_take::draw_tidied(&painter, plot, said, sampler_take::tidied_age(ui, node_id, *at), z) {
                    crate::app::pace::animate(ui.ctx());
                }
            }
        }
        None if live => {
            let scope = engine_id.and_then(|id| user_state.get_scope_data(id));
            sampler_take::draw_take(&painter, plot, &rec, scope, z);
            crate::app::pace::animate(ui.ctx());
        }
        None if hovered.is_some() => {
            if let Some(file) = hovered {
                hovered_file::draw(&painter, rect, plot, file, (lo, hi), hovered_cue, z);
            }
        }
        None => {
            // Ask for a file: a dashed outline round a hint
            let outline = plot.shrink(2.0 * z);
            let color = if drop_here { blue } else { theme::text::DISABLED.gamma_multiply(0.6) };
            dashed_rect(&painter, outline, Stroke::new(1.0 * z, color), 4.0 * z);
            let (hint, hint_color) = match (key, problem) {
                (Some(key), Some(problem)) if sample_files::is_loading(problem) => {
                    (format!("Loading {}…", sample_files::file_name(key)), theme::text::SECONDARY)
                }
                (Some(key), Some(problem)) => (format!("{}: {problem}", sample_files::file_name(key)), theme::accent::WARNING),
                _ if in_patched => ("Press Rec to record, or drop a sample".to_string(), if drop_here { blue } else { theme::text::SECONDARY }),
                _ => ("Drop a sample here".to_string(), if drop_here { blue } else { theme::text::SECONDARY }),
            };
            painter.text(outline.center() - Vec2::new(0.0, 5.0 * z), egui::Align2::CENTER_CENTER, truncate(&hint, 34), egui::FontId::proportional(9.0 * z), hint_color);
            painter.text(
                outline.center() + Vec2::new(0.0, 7.0 * z),
                egui::Align2::CENTER_CENTER,
                "or click to open one",
                small.clone(),
                theme::text::QUIET,
            );
        }
    }

    // Waiting to record: for a sound, or the next bar
    if matches!(rec.state, RecState::Armed | RecState::Waiting) {
        sampler_take::draw_waiting(&painter, rect, plot, &rec, ui.input(|i| i.time), z);
        crate::app::pace::animate(ui.ctx());
    }

    // A file dragged over: this is where it lands
    if drop_here {
        painter.rect_stroke(rect.shrink(0.5 * z), 3.0 * z, Stroke::new(1.5 * z, blue), egui::StrokeKind::Middle);
        if loaded.is_some() {
            painter.rect_filled(rect, 3.0 * z, Color32::from_rgba_unmultiplied(blue.r(), blue.g(), blue.b(), 28));
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Drop to replace", egui::FontId::proportional(10.0 * z), Color32::WHITE);
        }
    }

    // The markers: Start and End in blue at the foot, the loop's in orange at the head
    let marker_places = [lo, hi, loop_lo, loop_hi];
    let pointer = response.hover_pos().or(response.interact_pointer_pos());
    let drag_id = ui.scope_id().with(("sampler_marker", node_id));
    let dragging: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_id));
    let nearest = |x: f32| {
        (0..4)
            .filter(|&m| m < 2 || looping)
            .map(|m| (m, (x_at(marker_places[m]) - x).abs()))
            .filter(|&(_, distance)| distance <= GRAB * z)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(m, _)| m)
    };
    // A slice marker is the one the pointer is on if it's nearer than those
    let slice_dragging = slice_markers.as_ref().is_some_and(|markers| markers.dragging(ui));
    let slice_near = |x: f32| {
        let markers = slice_markers.as_ref()?;
        let (m, distance) = markers.nearest(x, x_at, GRAB * z)?;
        nearest(x).is_none_or(|region| (x_at(marker_places[region]) - x).abs() > distance).then_some(m)
    };
    let slice_hot = match (dragging, slice_dragging, shown, pointer) {
        (None, false, Some(_), Some(p)) => slice_near(p.x),
        _ => None,
    };
    let hot = dragging.or_else(|| if shown.is_some() && slice_hot.is_none() && !slice_dragging { pointer.and_then(|p| nearest(p.x)) } else { None });
    // With Alt down, a click plays from the pointer: a faint line shows where.
    // Sliced, it plays just the slice it's on, outlined
    let alt = ui.input(|i| i.modifiers.alt);
    let pointed = match (alt, shown, response.hover_pos(), dragging, slice_dragging) {
        (true, Some(_), Some(p), None, false) => slice_at(place_at(p.x)),
        _ => None,
    };
    // Letting go of a marker with Alt down plays the slice it now begins
    // (End, the one it ends; unsliced, Start to End)
    let let_go = match (dragging, &slice_markers) {
        _ if !alt || shown.is_none() => None,
        (Some(m), _) if m < 2 => Some(match slices {
            Some(slices) => span_of(slices, if m == 0 { 0 } else { slices.count() - 1 }),
            None => (lo, hi),
        }),
        (None, Some(markers)) => markers.in_hand_begins(ui).zip(slices).map(|(index, slices)| span_of(slices, index)),
        _ => None,
    };
    if let Some((from, to)) = pointed.zip(slices).map(|(index, slices)| span_of(slices, index)).or(let_go) {
        let heard = Rect::from_x_y_ranges(x_at(from)..=x_at(to), plot.y_range());
        painter.rect_stroke(heard, 1.0 * z, Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.6)), egui::StrokeKind::Middle);
    } else if let (true, Some(_), Some(p), None, false) = (alt, shown, response.hover_pos(), dragging, slice_dragging) {
        let x = x_at(place_at(p.x));
        let line = [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())];
        painter.extend(Shape::dashed_line(&line, Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.6)), 2.0 * z, 2.0 * z));
    }
    if let (Some(markers), Some(_)) = (&slice_markers, shown) {
        markers.draw(ui, &painter, rect, plot, slice_hot, z);
    }
    if shown.is_some() {
        for (m, &place) in marker_places.iter().enumerate() {
            if m >= 2 && !looping {
                continue;
            }
            let x = x_at(place);
            let lit = hot == Some(m);
            let color = if m < 2 { Color32::from_rgb(170, 210, 255) } else { orange };
            let color = if lit { color } else { color.gamma_multiply(0.75) };
            painter.vline(x, plot.y_range(), Stroke::new(if lit { 1.6 * z } else { 1.0 * z }, color));
            // A flag on the side the region lies, so a marker reads as its edge
            let inward = if m % 2 == 0 { 1.0 } else { -1.0 };
            let y = if m < 2 { plot.bottom() } else { plot.top() };
            let tip = if m < 2 { -5.0 * z } else { 5.0 * z };
            painter.add(Shape::convex_polygon(
                vec![Pos2::new(x, y), Pos2::new(x + inward * 5.0 * z, y), Pos2::new(x, y + tip)],
                color,
                Stroke::NONE,
            ));
        }
    }

    // Drag a marker; click anywhere else (or an empty panel) to open a file
    if shown.is_some() {
        if response.drag_started() {
            // Where the button went down: by the time a drag counts as one,
            // a quick hand is already a few points away
            let pressed = super::canvas_layer::in_ui(ui, ui.input(|i| i.pointer.press_origin())).or(response.interact_pointer_pos());
            match (pressed.and_then(|p| slice_near(p.x)), &slice_markers) {
                (Some(m), Some(markers)) => markers.pick_up(ui, m),
                _ => {
                    if let Some(m) = pressed.and_then(|p| nearest(p.x)) {
                        ui.ctx().data_mut(|d| d.insert_temp(drag_id, m));
                    }
                }
            }
        }
        if let (Some(markers), Some(p), true) = (&slice_markers, response.interact_pointer_pos(), response.dragged()) {
            responses.extend(markers.drag(ui, rect, p, place_at(p.x), z));
        }
        if let (Some(m), Some(p)) = (dragging, response.interact_pointer_pos()) {
            if response.dragged() {
                let (param, _) = MARKERS[m];
                // To the nearest 0.1%, as a knob would set it
                let value = (place_at(p.x) * 1000.0).round() / 1000.0;
                responses.push(SynthResponse::ParameterChanged { node_id, param_name: param.to_string(), value });
            }
        }
        if response.drag_stopped() {
            if let Some((from, to)) = let_go {
                responses.push(SynthResponse::CueSample { node_id, from, to });
            }
            ui.ctx().data_mut(|d| d.remove::<usize>(drag_id));
            if let Some(markers) = &slice_markers {
                markers.put_down(ui);
            }
        }
        // Double-click between the markers to add one
        if let (Some(markers), Some(p), true) = (&slice_markers, response.interact_pointer_pos(), response.double_clicked()) {
            if !alt && hot.is_none() && slice_hot.is_none() {
                responses.push(markers.add(place_at(p.x)));
            }
        }
        if response.clicked() && alt {
            // Sliced, just the slice; otherwise from there to End, or to the
            // end of the file past End
            let place = response.interact_pointer_pos().map_or(lo, |p| place_at(p.x));
            let (from, to) = match slices.zip(slice_at(place)) {
                Some((slices, index)) => span_of(slices, index),
                None => (place, if place < hi { hi } else { 1.0 }),
            };
            responses.push(SynthResponse::CueSample { node_id, from, to });
        }
        if alt && hot.is_none() && !slice_dragging {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        } else if hot.is_some() || slice_hot.is_some() || slice_dragging {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
    } else if response.clicked() && !live {
        responses.push(SynthResponse::OpenSample(node_id));
    }
    // A file dragged over says what it is on the panel
    if response.hovered() && dragging.is_none() && !slice_dragging && !drop_here {
        let tip = match (hot, shown, key) {
            _ if live => "Recording what's patched into In: press Rec to stop".to_string(),
            (_, Some(_), _) if alt => pointed.map_or("Click to hear it from here".to_string(), |index| format!("Click to hear slice {}", index + 1)),
            (Some(m), _, _) if m < 2 => {
                let heard = match (slices, m) {
                    (None, _) => "it, Start to End",
                    (Some(_), 0) => "the slice it begins",
                    (Some(_), _) => "the slice it ends",
                };
                format!("{}: drag to move it\nLet go with Alt held to hear {heard}", MARKERS[m].1)
            }
            (Some(m), _, _) => format!("{}: drag to move it", MARKERS[m].1),
            (None, Some(_), _) if slice_hot.is_some() => {
                let begins = slice_hot.and_then(|m| slice_markers.as_ref()?.slice_of(m));
                match begins {
                    Some(slice) => format!("Slice {slice} begins here\nDrag to move it, or off the panel to take it away\nLet go with Alt held to hear the slice"),
                    None => "A marker outside Start–End, cutting nothing\nDrag to move it, or off the panel to take it away".to_string(),
                }
            }
            (None, Some(sample), Some(key)) => match (slices, pointer.and_then(|p| slice_at(place_at(p.x)))) {
                (Some(slices), Some(index)) => {
                    let key = note_to_name((root + index as f32).clamp(0.0, 127.0) as u8);
                    let tip = format!("Slice {} of {}, played by {key}\nAlt-click to hear it", index + 1, slices.count());
                    if slices.is_marked() {
                        format!("{tip}\nDouble-click to add a marker")
                    } else {
                        tip
                    }
                }
                _ => describe(key, sample),
            },
            (None, None, Some(key)) => format!("{key}\n{}\nClick to open another sound file", problem.filter(|p| !sample_files::is_loading(p)).unwrap_or("Loading…")),
            _ => "Click to open a WAV, MP3, FLAC or Ogg file, or drag one from your files onto the node".to_string(),
        };
        response.on_hover_text(tip);
    }

    // The file's name, and a button to open another
    let row_height = 14.0 * z;
    let (row, _) = ui.allocate_exact_size(Vec2::new(width, row_height), Sense::hover());
    let name = key.map(sample_files::file_name).unwrap_or("No file");
    let name_color = if problem.is_some_and(|p| !sample_files::is_loading(p)) { theme::accent::WARNING } else if key.is_some() { theme::text::SECONDARY } else { theme::text::DISABLED };
    let button_width = 34.0 * z;
    // With a file, ▶ to hear it first, then its name
    let mut name_left = row.left() + 2.0 * z;
    if loaded.is_some() {
        let play = Rect::from_min_size(Pos2::new(row.left(), row.top() + 1.0 * z), Vec2::new(18.0 * z, row_height - 2.0 * z));
        let hear = ui.interact(play, ui.scope_id().with(("sampler_cue", node_id)), Sense::click());
        super::marks::mark(ui, node_id, "cue", play);
        let fill = match (cue.is_some(), hear.hovered()) {
            (true, _) => blue.gamma_multiply(0.45),
            (false, true) => theme::background::WIDGET_HOVERED,
            (false, false) => theme::background::WIDGET,
        };
        ui.painter().rect(play, 3.0 * z, fill, Stroke::new(1.0 * z, theme::background::GRID_MAJOR), egui::StrokeKind::Middle);
        let c = play.center();
        let icon = theme::text::PRIMARY;
        if cue.is_some() {
            ui.painter().rect_filled(Rect::from_center_size(c, Vec2::splat(5.0 * z)), 0.5 * z, icon);
        } else {
            let points = vec![Pos2::new(c.x - 2.5 * z, c.y - 3.5 * z), Pos2::new(c.x + 3.5 * z, c.y), Pos2::new(c.x - 2.5 * z, c.y + 3.5 * z)];
            ui.painter().add(Shape::convex_polygon(points, icon, Stroke::NONE));
        }
        let tip = match (cue, slices) {
            (Some(_), _) => "Stop",
            (None, Some(_)) => "Hear it, Start to End, as recorded: dry, beside the patch, playing or stopped\nAlt-click a slice to hear just it",
            (None, None) => "Hear it, Start to End, as recorded: dry, beside the patch, playing or stopped\nAlt-click the waveform to hear it from anywhere",
        };
        if hear.on_hover_text(tip).clicked() {
            responses.push(if cue.is_some() { SynthResponse::StopCue(node_id) } else { SynthResponse::CueSample { node_id, from: lo, to: hi } });
        }
        name_left = play.right() + 4.0 * z;
    }
    // Its name opens the menu to trim or normalize it, between ▶ and Rec
    match loaded.filter(|_| !live) {
        Some(sample) => {
            let length = duration(sample.source.seconds());
            let menu = Rect::from_x_y_ranges(name_left - 2.0 * z..=row.right() - button_width - 26.0 * z, row.top() + 1.0 * z..=row.bottom() - 1.0 * z);
            let in_tab = key.is_some_and(|key| key.starts_with(sample_files::DROPPED_PREFIX));
            let label = sampler_tidy::NameLabel { name, length: &length, color: name_color };
            responses.extend(sampler_tidy::name_button(ui, menu, node_id, &label, sample, in_tab, z));
        }
        None => {
            let label = match loaded {
                Some(sample) => format!("{}  {}", truncate(name, 18), duration(sample.source.seconds())),
                None => truncate(name, 28),
            };
            ui.painter().text(Pos2::new(name_left, row.center().y), egui::Align2::LEFT_CENTER, label, small.clone(), name_color);
        }
    }
    let button = Rect::from_min_size(Pos2::new(row.right() - button_width, row.top() + 1.0 * z), Vec2::new(button_width, row_height - 2.0 * z));
    let open = ui.interact(button, ui.scope_id().with(("sampler_open", node_id)), Sense::click());
    super::marks::mark(ui, node_id, "file", button);
    let fill = if open.hovered() { theme::background::WIDGET_HOVERED } else { theme::background::WIDGET };
    ui.painter().rect(button, 3.0 * z, fill, Stroke::new(1.0 * z, theme::background::GRID_MAJOR), egui::StrokeKind::Middle);
    ui.painter().text(button.center(), egui::Align2::CENTER_CENTER, "Open…", small, theme::text::PRIMARY);
    if open.on_hover_text("Open a WAV, MP3, FLAC or Ogg file for this Sampler").clicked() {
        responses.push(SynthResponse::OpenSample(node_id));
    }

    // Rec, beside it
    let rec_rect = Rect::from_min_size(Pos2::new(button.left() - 22.0 * z, row.top() + 1.0 * z), Vec2::new(18.0 * z, row_height - 2.0 * z));
    let held = value_of(REC_BUTTON).is_some_and(|v| v >= 0.5);
    let tidies = node.get_input(sampler_take::TIDY_TAKES).is_ok_and(|input| graph.get_input(input).value.actual_value() >= 0.5);
    responses.extend(sampler_take::rec_button(ui, rec_rect, node_id, user_state, &rec, RecButton { down: held, param_index: Sampler::PARAM_REC_BUTTON, tidies: Some(tidies), blocked }, z));
    responses
}

/// A tooltip on the loaded file.
fn describe(key: &str, sample: &ShelvedSample) -> String {
    let source = &sample.source;
    let channels = if source.is_stereo() { "stereo" } else { "mono" };
    let mut text = format!("{key}\n{}, {} Hz {channels}", duration(source.seconds()), source.sample_rate() as u32);
    if sample.truncated {
        text.push_str("\nCut to the 5 minutes a Sampler keeps");
    }
    text.push_str("\nDrag the markers to choose what plays, or Alt-click to hear it from there");
    text
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

/// A rectangle outlined in dashes.
fn dashed_rect(painter: &egui::Painter, rect: Rect, stroke: Stroke, dash: f32) {
    let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    for side in corners.windows(2) {
        painter.extend(Shape::dashed_line(side, stroke, dash, dash * 0.8));
    }
}
