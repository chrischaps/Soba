//! The Spectrum's face: what a sound is made of, one line per partial.
//!
//! The axis runs 20 Hz to 20 kHz on a log scale, so every octave is as wide
//! as the next and a played note's harmonics line up with the keyboard
//! along the floor: the second an octave up, the third a fifth above that,
//! the fourth two octaves, the fifth a major third over those. The levels
//! are a sine's amplitude in dB, full scale at the 0 line and the floor as
//! far down as the Range knob says.
//!
//! In 1 is drawn blue and In 2 orange behind it, each as what's sounding
//! now, filled beneath, and a finer line above holding each peak for the
//! Hold time before it sinks (see [`super::spectrum_view`]). With
//! Harmonics on, the partials of the note In 1 plays (or In 2's, with
//! nothing in In 1) are numbered over their peaks, their keys light, and a
//! dashed line falls from the first at 6 dB an octave: where a saw's
//! harmonics stand, each 1/n of the first. A square's odd ones stand on
//! it with the even ones missing; a triangle's fall away below it.
//!
//! Labels are drawn with the painter in patch units, so nothing here steps
//! the node's size as the view zooms (#255). The face is as big as the
//! hidden Size parameter says, and the grip in its corner drags it bigger.

use eframe::egui::{self, Align2, Color32, CursorIcon, FontId, Id, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::NodeId;

use super::display_frame::{self as face, curve, guide, hertz, input_patched, value_of, with_alpha};
use super::drum_display::fill_under;
use super::filter_spectrum::{HIGHEST_HZ, LOWEST_HZ};
use super::spectrum_view::{Trace, COLUMNS, SILENCE_DB};
use super::{SynthGraph, SynthGraphState};
use crate::app::theme;
use crate::modules::quantizer::NOTE_NAMES;
use crate::modules::spectrum::MAX_SIZE;

/// The face at Size 1, before zoom: as wide as six knobs.
const KNOBS_WIDE: usize = 6;
const HEIGHT: f32 = 132.0;

/// The level at the top of the plot, in dB: a little room over full scale.
const TOP_DB: f32 = 6.0;

/// The row of frequencies over the plot, the keyboard strip along its
/// floor and the row of C names under that, before zoom.
const HZ_HEIGHT: f32 = 7.5;
const KEYS_HEIGHT: f32 = 5.0;
const NOTES_HEIGHT: f32 = 8.0;

/// The axis labels' size, before zoom.
const LABEL_SIZE: f32 = 6.5;

/// The most harmonics numbered.
const MOST_HARMONICS: usize = 32;

/// The frequencies the axis may label, most wanted first: each is drawn if
/// it clears the ones before it.
const AXIS_HZ: [f32; 9] = [100.0, 1_000.0, 10_000.0, 50.0, 500.0, 5_000.0, 200.0, 2_000.0, 20.0];

/// In 1's colour and In 2's, as the Oscilloscope draws them.
const TRACE_COLOURS: [Color32; 2] = [theme::signal::AUDIO, theme::signal::CONTROL];

/// Draws the face; returns a new Size when its grip is dragged.
pub fn spectrum_display(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Option<(String, f32)> {
    let z = zoom;
    let size = value_of(graph, node_id, "Size").unwrap_or(1.0).clamp(1.0, MAX_SIZE);
    let hold = value_of(graph, node_id, "Hold").unwrap_or(1.5);
    let range = value_of(graph, node_id, "Range").unwrap_or(72.0).max(12.0);
    let numbered = value_of(graph, node_id, "Harmonics").unwrap_or(1.0) > 0.5;
    let patched = [input_patched(graph, node_id, "In 1"), input_patched(graph, node_id, "In 2")];
    let base = Vec2::new(face::knob_row_width(ui, KNOBS_WIDE, z), HEIGHT * z);

    let accent = crate::dsp::ModuleCategory::Utility.color();
    let (rect, response, painter) = face::frame_of_height(ui, base.x * size, base.y * size, accent, z)?;

    let engine_id = user_state.get_engine_node_id(node_id);
    if let Some(id) = engine_id {
        user_state.spectrum_views.ask(id, hold);
    }
    let view = engine_id.and_then(|id| user_state.spectrum_views.get(id));

    // The plot, the keyboard strip beneath it, and the row of C names
    let pad = 4.0 * z;
    let notes = Rect::from_min_max(Pos2::new(rect.left() + pad, rect.bottom() - pad - NOTES_HEIGHT * z), Pos2::new(rect.right() - pad, rect.bottom() - pad));
    let keys_height = KEYS_HEIGHT * z * size.sqrt();
    let keys = Rect::from_min_max(Pos2::new(notes.left(), notes.top() - keys_height), Pos2::new(notes.right(), notes.top()));
    let hz_row = rect.top() + pad + face::HEADER * z + 1.0 * z;
    let plot = Rect::from_min_max(Pos2::new(keys.left(), hz_row + HZ_HEIGHT * z), Pos2::new(keys.right(), keys.top() - 1.0 * z));
    let floor_db = -range;
    let axis = Axis { plot, floor_db };

    draw_grid(&painter, &axis, hz_row, z);
    let keyboard = Keyboard::new(keys);
    keyboard.paint(&painter, z);
    draw_note_names(&painter, &keyboard, notes, z);

    // Whose harmonics are numbered: In 1's, or In 2's with nothing in In 1
    let traces = view.map(|view| &view.traces);
    let pitched = traces.and_then(|traces| {
        let order = if patched[0] || !patched[1] { [0, 1] } else { [1, 0] };
        order.into_iter().find_map(|index| traces[index].pitch.fundamental().map(|(hz, shown)| (index, hz, shown)))
    });

    let mut moving = false;
    if let Some(traces) = traces {
        // In 2 behind, In 1 in front
        for index in [1, 0] {
            let trace = &traces[index];
            if trace.presence > 0.0 {
                draw_trace(&painter, &axis, trace, TRACE_COLOURS[index], z);
                moving = true;
            }
        }
        if let Some((index, hz, shown)) = pitched.filter(|_| numbered) {
            draw_harmonics(&painter, &axis, &keyboard, &traces[index], hz, shown, TRACE_COLOURS[index], z);
            moving = true;
        }
    }

    // The header: the note each input plays
    let note_of = |index: usize| -> Option<(String, Color32)> {
        let (hz, shown) = traces?[index].pitch.fundamental()?;
        let colour = with_alpha(TRACE_COLOURS[index], (255.0 * shown) as u8);
        Some((format!("{}  {}", Note::of(hz).name(), hertz(hz)), colour))
    };
    let silent = !patched[0] && !patched[1];
    let left = note_of(0).or_else(|| silent.then(|| ("patch a sound into In 1".to_string(), theme::text::QUIET)));
    face::header(&painter, rect, z, left, note_of(1));

    // Under the pointer: what it is, and what's there
    let pointer = response.hover_pos().filter(|at| plot.x_range().contains(at.x) && at.y <= keys.bottom());
    if let Some(at) = pointer {
        guide(&painter, Pos2::new(at.x, plot.top()), Pos2::new(at.x, keys.bottom()), 60, z);
    }

    let changed = grip(ui, &painter, node_id, rect, base, size, z);

    if moving || user_state.is_playing {
        crate::app::pace::animate(ui.ctx());
    }
    if let Some(at) = pointer {
        let hz = axis.hz_at(at.x);
        let note = Note::of(hz);
        let mut text = format!("{}, {} {:+.0}¢", hertz(hz), note.name(), note.cents);
        if let Some(traces) = traces {
            for (index, trace) in traces.iter().enumerate() {
                if patched[index] && trace.presence > 0.0 {
                    text.push_str(&format!("\nIn {}: {:.0} dB", index + 1, trace.held_at(hz).max(floor_db)));
                }
            }
        }
        if let Some((_, fundamental, _)) = pitched {
            let n = (hz / fundamental).round();
            if n >= 1.0 && (hz / fundamental / n - 1.0).abs() < 0.02 {
                text.push_str(&format!("\nHarmonic {n:.0} of {}", Note::of(fundamental).name()));
            }
        }
        text.push_str("\n\nEach line is a partial: a sine the sound is made of");
        if numbered && pitched.is_some() {
            text.push_str("\nThe dashed line falls as a saw's harmonics do, each 1/n of the first");
        }
        response.on_hover_text(text);
    }
    changed
}

/// Where frequencies and levels land on the plot.
struct Axis {
    plot: Rect,
    floor_db: f32,
}

impl Axis {
    fn x_at(&self, hz: f32) -> f32 {
        self.plot.left() + (hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln() * self.plot.width()
    }

    fn hz_at(&self, x: f32) -> f32 {
        LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf((x - self.plot.left()) / self.plot.width())
    }

    fn y_at(&self, db: f32) -> f32 {
        let db = db.clamp(self.floor_db, TOP_DB);
        self.plot.top() + (TOP_DB - db) / (TOP_DB - self.floor_db) * self.plot.height()
    }
}

/// The level lines every 12 dB with their numbers on the right, and the
/// frequency lines with theirs in the row over the plot from `hz_row` down,
/// as many as fit.
fn draw_grid(painter: &Painter, axis: &Axis, hz_row: f32, z: f32) {
    let plot = axis.plot;
    let font = FontId::proportional(LABEL_SIZE * z);
    let ink = with_alpha(theme::text::QUIET, 120);

    // Numbered every 12 dB, or every 24 when they'd crowd
    let per_12 = plot.height() * 12.0 / (TOP_DB - axis.floor_db);
    let every = if per_12 < LABEL_SIZE * 1.4 * z { 24 } else { 12 };
    let mut db = 0;
    while db as f32 > axis.floor_db {
        let y = axis.y_at(db as f32);
        guide(painter, Pos2::new(plot.left(), y), Pos2::new(plot.right(), y), if db == 0 { 30 } else { 14 }, z);
        if db % every == 0 {
            painter.text(Pos2::new(plot.right() - 1.0 * z, y - 0.5 * z), Align2::RIGHT_BOTTOM, db.to_string(), font.clone(), ink);
        }
        db -= 12;
    }

    // Frequencies, most wanted first, each where it clears the others
    let mut taken: Vec<(f32, f32)> = Vec::new();
    for hz in AXIS_HZ {
        let x = axis.x_at(hz);
        let text = axis_hz(hz);
        let width = text.len() as f32 * LABEL_SIZE * 0.6 * z;
        let (from, to) = (x + 1.5 * z, x + 1.5 * z + width);
        // Clear of the edge, and of each other
        if to > plot.right() || taken.iter().any(|&(a, b)| from < b + 4.0 * z && to > a - 4.0 * z) {
            continue;
        }
        taken.push((from, to));
        if hz > LOWEST_HZ {
            let decade = matches!(hz as u32, 100 | 1_000 | 10_000);
            guide(painter, Pos2::new(x, hz_row), Pos2::new(x, plot.bottom()), if decade { 20 } else { 10 }, z);
        }
        painter.text(Pos2::new(from, hz_row), Align2::LEFT_TOP, text, font.clone(), ink);
    }
}

/// A frequency as the axis writes it: "50", "500", "2k", "10k".
fn axis_hz(hz: f32) -> String {
    if hz >= 1_000.0 {
        format!("{}k", (hz / 1_000.0).round())
    } else {
        format!("{}", hz.round())
    }
}

/// The keyboard strip: one key for every semitone on the axis.
struct Keyboard {
    rect: Rect,
    /// The MIDI notes of the first and last keys.
    first: i32,
    last: i32,
}

impl Keyboard {
    fn new(rect: Rect) -> Self {
        let midi = |hz: f32| 69.0 + 12.0 * (hz / 440.0).log2();
        Self { rect, first: midi(LOWEST_HZ).ceil() as i32, last: midi(HIGHEST_HZ).floor() as i32 }
    }

    /// Where key `note` spans across the strip.
    fn key(&self, note: i32) -> Rect {
        let x = |midi: f32| {
            let hz = 440.0 * ((midi - 69.0) / 12.0).exp2();
            self.rect.left() + (hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln() * self.rect.width()
        };
        let (left, right) = (x(note as f32 - 0.5).max(self.rect.left()), x(note as f32 + 0.5).min(self.rect.right()));
        Rect::from_x_y_ranges(left..=right, self.rect.y_range())
    }

    fn paint(&self, painter: &Painter, z: f32) {
        let white = Color32::from_rgb(52, 56, 70);
        let black = Color32::from_rgb(28, 30, 38);
        for note in self.first..=self.last {
            let key = self.key(note);
            let is_black = matches!(note.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
            painter.rect_filled(key, 0.0, if is_black { black } else { white });
            // Where two white keys meet, a seam: between B and C, E and F
            if matches!(note.rem_euclid(12), 0 | 5) {
                painter.line_segment([key.left_top(), key.left_bottom()], Stroke::new(0.6 * z, face::PANEL));
            }
        }
    }

    /// Lights the key nearest `hz`.
    fn light(&self, painter: &Painter, hz: f32, colour: Color32) {
        let note = (69.0 + 12.0 * (hz / 440.0).log2()).round() as i32;
        if (self.first..=self.last).contains(&note) {
            painter.rect_filled(self.key(note), 0.0, colour);
        }
    }
}

/// C1 to C8 under the keyboard, middle C a little brighter.
fn draw_note_names(painter: &Painter, keyboard: &Keyboard, notes: Rect, z: f32) {
    let font = FontId::proportional(LABEL_SIZE * z);
    for octave in 1..=8 {
        let key = keyboard.key(12 * (octave + 1));
        let ink = if octave == 4 { with_alpha(theme::text::SECONDARY, 200) } else { with_alpha(theme::text::QUIET, 130) };
        painter.line_segment([Pos2::new(key.center().x, notes.top()), Pos2::new(key.center().x, notes.top() + 1.5 * z)], Stroke::new(0.6 * z, ink));
        painter.text(Pos2::new(key.center().x, notes.top() + 1.5 * z), Align2::CENTER_TOP, format!("C{octave}"), font.clone(), ink);
    }
}

/// One trace: what's sounding filled beneath, its held peaks over it.
fn draw_trace(painter: &Painter, axis: &Axis, trace: &Trace, colour: Color32, z: f32) {
    let plot = axis.plot;
    let points = (plot.width() / (1.0 * z.max(0.1))).clamp(64.0, 2_400.0) as usize;
    let line = |levels: &[f32; COLUMNS]| -> Vec<Pos2> {
        resample(levels, points)
            .into_iter()
            .enumerate()
            .map(|(i, db)| Pos2::new(plot.left() + (i as f32 + 0.5) / points as f32 * plot.width(), axis.y_at(db)))
            .collect()
    };
    let fade = |alpha: f32| (alpha * trace.presence).round() as u8;
    let live = line(&trace.live);
    fill_under(painter, &live, plot.bottom(), with_alpha(colour, fade(70.0)), with_alpha(colour, fade(6.0)));
    let light = lerp_colour(colour, Color32::WHITE, 0.45);
    curve(painter, line(&trace.held), with_alpha(light, fade(130.0)), 0.7 * z, false);
    curve(painter, live, with_alpha(colour, fade(230.0)), 1.1 * z, true);
}

/// `levels` at `points` evenly across: the loudest of the columns each
/// point covers, so a partial never falls between points, or read between
/// columns where points are closer than they are.
fn resample(levels: &[f32; COLUMNS], points: usize) -> Vec<f32> {
    let per = COLUMNS as f32 / points as f32;
    (0..points)
        .map(|i| {
            let (from, to) = (i as f32 * per, (i + 1) as f32 * per);
            if per >= 1.0 {
                let (first, past) = (from.floor() as usize, (to.ceil() as usize).min(COLUMNS));
                levels[first..past].iter().copied().fold(SILENCE_DB, f32::max)
            } else {
                let at = ((from + to) / 2.0 - 0.5).clamp(0.0, (COLUMNS - 1) as f32);
                let (below, t) = (at.floor() as usize, at.fract());
                let above = (below + 1).min(COLUMNS - 1);
                levels[below] + (levels[above] - levels[below]) * t
            }
        })
        .collect()
}

/// The fundamental's harmonics: a number over each peak, its key lit, and
/// the 1/n line falling from the first.
#[allow(clippy::too_many_arguments)]
fn draw_harmonics(painter: &Painter, axis: &Axis, keyboard: &Keyboard, trace: &Trace, fundamental: f32, shown: f32, colour: Color32, z: f32) {
    let plot = axis.plot;
    let ink = lerp_colour(colour, Color32::WHITE, 0.55);
    let first_db = trace.held_at(fundamental);
    let audible = |db: f32| db > axis.floor_db + 6.0;

    // How a saw's harmonics fall: 6 dB an octave, a straight line on this axis
    if audible(first_db) {
        let foot = (fundamental * 10f32.powf((first_db - axis.floor_db) / 20.0)).min(HIGHEST_HZ);
        let ends = [Pos2::new(axis.x_at(fundamental), axis.y_at(first_db)), Pos2::new(axis.x_at(foot), axis.y_at(first_db - 20.0 * (foot / fundamental).log10()))];
        painter.extend(Shape::dashed_line(&ends, Stroke::new(0.8 * z, with_alpha(Color32::WHITE, (60.0 * shown) as u8)), 3.0 * z, 2.5 * z));
    }

    let font = FontId::proportional((LABEL_SIZE + 0.5) * z);
    for n in 1..=MOST_HARMONICS {
        let hz = fundamental * n as f32;
        if hz > HIGHEST_HZ {
            break;
        }
        let x = axis.x_at(hz);
        // Stop once the numbers would run into each other
        let room = axis.x_at(hz + fundamental) - x;
        let width = n.to_string().len() as f32 * LABEL_SIZE * 0.6 * z;
        if n > 1 && room < width + 1.5 * z {
            break;
        }
        let db = trace.held_at(hz);
        let strength = ((db - axis.floor_db) / (TOP_DB - axis.floor_db)).clamp(0.0, 1.0);
        let alpha = |most: f32| (most * shown).round() as u8;
        if audible(db) {
            let top = Pos2::new(x, axis.y_at(db));
            painter.circle_filled(top, 3.0 * z, with_alpha(colour, alpha(50.0)));
            painter.circle_filled(top, 1.3 * z, with_alpha(Color32::WHITE, alpha(220.0)));
            // Over its peak, or beside it where the peak is near the top
            let (at, align) = if top.y - 2.5 * z - font.size >= plot.top() {
                (Pos2::new(x, top.y - 2.5 * z), Align2::CENTER_BOTTOM)
            } else {
                (Pos2::new(x + 3.5 * z, top.y), Align2::LEFT_CENTER)
            };
            painter.text(at, align, n.to_string(), font.clone(), with_alpha(ink, alpha(235.0)));
            keyboard.light(painter, hz, with_alpha(colour, alpha(if n == 1 { 255.0 } else { 90.0 + 140.0 * strength })));
        } else {
            // A harmonic that isn't there: its number, dim, on the floor
            let at = Pos2::new(x, plot.bottom() - 1.0 * z);
            painter.text(at, Align2::CENTER_BOTTOM, n.to_string(), font.clone(), with_alpha(theme::text::QUIET, alpha(110.0)));
        }
    }
}

/// The grip in the corner: drag it to resize the face, double-click to put
/// it back. Returns the new Size.
fn grip(ui: &egui::Ui, painter: &Painter, node_id: NodeId, rect: Rect, base: Vec2, size: f32, z: f32) -> Option<(String, f32)> {
    let grip = Rect::from_min_max(rect.max - Vec2::splat(11.0 * z), rect.max);
    let response = ui.interact(grip, Id::unique((node_id, "spectrum-grip")), Sense::click_and_drag());
    let hot = response.hovered() || response.dragged();
    let ink = if hot { theme::text::PRIMARY } else { with_alpha(theme::text::SECONDARY, 110) };
    for i in 1..=2 {
        let d = 3.5 * z * i as f32;
        painter.line_segment([rect.max - Vec2::new(d, 2.0 * z), rect.max - Vec2::new(2.0 * z, d)], Stroke::new(1.0 * z, ink));
    }
    if hot {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
    }
    let mut changed = None;
    if response.double_clicked() {
        changed = Some(1.0);
    } else if response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let reach = (pointer - rect.min) / base;
            changed = Some((reach.x.max(reach.y).clamp(1.0, MAX_SIZE) * 20.0).round() / 20.0);
        }
    }
    response.on_hover_text("Drag to make the spectrum bigger; double-click to put it back");
    changed.filter(|&new| (new - size).abs() > 1e-3).map(|new| ("Size".to_string(), new))
}

/// `a` mixed `t` of the way toward `b`.
fn lerp_colour(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// The nearest note to a frequency, and how far off it, in cents.
struct Note {
    midi: i32,
    cents: f32,
}

impl Note {
    fn of(hz: f32) -> Self {
        let midi = 69.0 + 12.0 * (hz.max(1.0) / 440.0).log2();
        Self { midi: midi.round() as i32, cents: (midi - midi.round()) * 100.0 }
    }

    /// "A2", "F#4".
    fn name(&self) -> String {
        format!("{}{}", NOTE_NAMES[self.midi.rem_euclid(12) as usize], self.midi.div_euclid(12) - 1)
    }
}

/// The column a frequency falls in, for tests.
#[cfg(test)]
fn column_hz(column: usize) -> f32 {
    super::spectrum_view::hz_at(column as f32 + 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_named_as_the_pianos_name_them() {
        assert_eq!(Note::of(440.0).name(), "A4");
        assert_eq!(Note::of(261.63).name(), "C4");
        assert_eq!(Note::of(32.70).name(), "C1");
        assert_eq!(Note::of(110.0).name(), "A2");
        let sharp = Note::of(446.0);
        assert_eq!(sharp.name(), "A4");
        assert!((sharp.cents - 23.4).abs() < 0.5, "{}", sharp.cents);
    }

    #[test]
    fn the_axis_writes_frequencies_short() {
        assert_eq!(axis_hz(50.0), "50");
        assert_eq!(axis_hz(2_000.0), "2k");
        assert_eq!(axis_hz(10_000.0), "10k");
    }

    #[test]
    fn resampling_keeps_every_peak() {
        // A single partial in one column survives being drawn at fewer points
        let mut levels = [SILENCE_DB; COLUMNS];
        levels[337] = -6.0;
        for points in [64, 150, 300, 599] {
            let drawn = resample(&levels, points);
            assert_eq!(drawn.iter().copied().fold(SILENCE_DB, f32::max), -6.0, "at {points} points");
        }
        // And more points than columns read smoothly between them
        let ramp: [f32; COLUMNS] = std::array::from_fn(|i| i as f32);
        let drawn = resample(&ramp, 1_200);
        assert!(drawn.windows(2).all(|w| w[1] >= w[0]));
        assert!((drawn[675] - 337.25).abs() < 0.01, "{}", drawn[675]);
    }

    #[test]
    fn keys_sit_under_their_frequencies() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 5.0));
        let keyboard = Keyboard::new(rect);
        let axis = Axis { plot: rect, floor_db: -72.0 };
        let a4 = keyboard.key(69);
        assert!(a4.x_range().contains(axis.x_at(440.0)));
        assert!((axis.x_at(column_hz(300)) - 300.5).abs() < 0.01, "columns lie evenly across");
        assert!((axis.hz_at(axis.x_at(1_234.0)) - 1_234.0).abs() < 0.1);
    }
}
