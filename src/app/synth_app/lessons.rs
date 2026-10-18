//! Lessons on screen (#305): the caption beside the part it's about, a glow
//! on that part, and a ghost pointer that shows the move once and then
//! leaves it to you. What a lesson says and waits for is in
//! [`crate::app::lesson`].
//!
//! Leaving a lesson takes all of this away and leaves the patch as it is,
//! an ordinary patch to keep playing or save.

use eframe::egui::{self, pos2, vec2, Color32, FontFamily, FontId, Pos2, Rect, Stroke, Vec2};
use egui::text::{LayoutJob, TextFormat};

use crate::app::capture::draw_pointer;
use crate::app::capture_aim;
use crate::app::lesson::{self, Facts, LessonFile, LessonRun, Show, Spot};
use crate::app::theme;
use crate::graph::annotations::emphasis_runs;
use crate::graph::groups::{self, NodeKind};
use crate::graph::annotation_ui;
use crate::persistence::patch_from_json;
use crate::persistence::sample_files::SampleBase;
use super::SynthApp;

/// A lesson's colour: lamplight, apart from every signal's and module's.
const GOLD: Color32 = Color32::from_rgb(246, 196, 92);
/// Bold words in a caption.
const EMPHASIS: Color32 = Color32::from_rgb(255, 234, 196);

const CARD_WIDTH: f32 = 330.0;
/// How far the card sits from the part it's about.
const CARD_GAP: f32 = 34.0;
/// How far from every module a right-click spot is as good as any further.
const ROOM: f32 = 160.0;
/// How long into a step the view follows a part that's out of it.
const FOLLOW_SECONDS: f64 = 2.5;
/// How long a met step shows its tick before the next.
const MET_PAUSE: f64 = 1.0;
/// How long after a step begins the ghost shows its move: time to read first.
const GHOST_DELAY: f64 = 1.2;

/// The ghost's timeline, in seconds from when it starts: it fades in beside
/// the first spot, glides there, acts, lingers and fades.
const GHOST_APPEAR: f64 = 0.35;
const GHOST_ARRIVE: f64 = 1.15;
const GHOST_PRESS: f64 = 1.3;
/// A drag's or a turn's glide, after the press.
const GHOST_CARRY: f64 = 1.0;
const GHOST_LINGER: f64 = 0.6;
const GHOST_FADE: f64 = 0.45;

/// What the card asked for this frame.
enum CardAction {
    Leave,
    ShowMe,
    Next,
    NextLesson(&'static LessonFile),
}

impl SynthApp {
    /// Starts a lesson: opens its patch (or an empty canvas) and puts its
    /// first step on screen. The patch is a new, unsaved one.
    pub(super) fn open_lesson(&mut self, file: &'static LessonFile) {
        let lesson = match file.lesson() {
            Ok(lesson) => lesson,
            Err(e) => {
                self.raise_notice(format!("Couldn't open the lesson: {e}"));
                return;
            }
        };
        match &lesson.patch {
            None => self.new_patch(),
            Some(name) => {
                let patch = lesson::patch_json(name)
                    .ok_or_else(|| format!("it has no patch {name}"))
                    .and_then(|json| patch_from_json(json).map_err(|e| e.to_string()))
                    .and_then(|patch| self.load_patch(&patch, SampleBase::Example).map_err(|e| e.to_string()));
                if let Err(e) = patch {
                    self.raise_notice(format!("Couldn't open the lesson {}: {e}", lesson.title));
                    return;
                }
                // Framed whole, at no more than life size, wherever its
                // modules sit and however big the window
                self.fit_pending = 6;
                self.document.opened(None);
            }
        }
        self.status_message = Some(format!("Lesson: {}", lesson.title));
        if self.capture.is_some() {
            eprintln!("lesson: {} begins, {} steps", lesson.title, lesson.waits());
        }
        self.lesson = Some(LessonRun::new(file, lesson));
    }

    /// Opens the lesson `?lesson=` or `--lesson` names, through the prompt
    /// for unsaved changes. Returns whether there's a lesson by that name.
    pub fn open_lesson_named(&mut self, ctx: &egui::Context, name: &str) -> bool {
        let Some(file) = lesson::find(name) else { return false };
        self.request(ctx, super::session::Discard::OpenLesson(file));
        true
    }

    /// Help → Lessons: every lesson, by its title, with what it's about.
    pub(super) fn lessons_menu(&self, ui: &mut egui::Ui) -> Option<&'static LessonFile> {
        let mut chosen = None;
        for (n, file) in lesson::LESSONS.iter().enumerate() {
            let Ok(lesson) = file.lesson() else { continue };
            let taking = self.lesson.as_ref().is_some_and(|run| run.file.file_name == file.file_name);
            let label = format!("{}. {}{}", n + 1, lesson.title, if taking { "  ·  now" } else { "" });
            if ui.button(label).on_hover_text(&lesson.about).clicked() {
                chosen = Some(file);
                ui.close();
            }
        }
        chosen
    }

    /// Each frame: checks the step's wait, moves on once it's met, and
    /// draws the glow, the ghost and the caption over the editor.
    pub(super) fn tend_lesson(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let Some(run) = self.lesson.as_mut() else { return };

        // A key a Note In plays is held as MIDI too: count it once
        let mut held: Vec<u8> = self.midi.held().iter().map(|&(note, _)| note).collect();
        for &(note, _) in &self.pressed_keys {
            let note = crate::app::qwerty_keys::relative_to_midi(note, 0);
            if !held.contains(&note) {
                held.push(note);
            }
        }
        let facts = Facts {
            graph: &self.graph_state.graph,
            state: &self.user_state,
            playing: self.is_playing,
            notes_held: held.len(),
            probing: self.probe.probing(),
        };
        let dt = run.last_time.map_or(0.0, |t| (now - t).clamp(0.0, 0.25));
        run.last_time = Some(now);
        if run.began.is_none() {
            run.began = Some(now);
            run.canvas = None;
        }
        if run.met_at.is_none() {
            if let Some(wait) = run.lesson.steps.get(run.step).and_then(|s| s.wait.as_ref()) {
                if wait.met(&facts, &mut run.progress, dt) {
                    run.met_at = Some(now);
                }
            }
        }
        if run.met_at.is_some_and(|at| now - at >= MET_PAUSE) {
            if self.capture.is_some() {
                eprintln!("lesson: step {} of {} done", run.number(), run.lesson.waits());
            }
            run.advance();
            if run.finished() && self.capture.is_some() {
                eprintln!("lesson: {} complete", run.lesson.title);
            }
            return self.tend_lesson(ctx);
        }

        let action = self.draw_lesson(ctx, now, dt);
        let Some(run) = self.lesson.as_mut() else { return };
        match action {
            Some(CardAction::Leave) => {
                self.status_message = Some(format!("Left the lesson {}: the patch is yours", run.lesson.title));
                self.lesson = None;
            }
            Some(CardAction::ShowMe) => run.ghost_from = Some(now),
            Some(CardAction::Next) => run.met_at = Some(now - MET_PAUSE),
            Some(CardAction::NextLesson(file)) => self.request(ctx, super::session::Discard::OpenLesson(file)),
            None => {}
        }
        // The glow breathes and the ghost moves; a steady frame rate is plenty
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }

    /// Where a lesson's spot is on screen now, if it's on show.
    fn lesson_spot(&self, ctx: &egui::Context, spot: &Spot, canvas: Option<Pos2>) -> Option<Rect> {
        let graph = &self.graph_state.graph;
        let state = &self.user_state;
        let zoom = self.graph_state.pan_zoom.zoom;
        match spot {
            Spot::Part(part) => capture_aim::locate_rect(ctx, graph, state, zoom, part).ok(),
            Spot::Module(kind, which) => {
                capture_aim::find_module(graph, state, kind, *which).ok().and_then(|id| annotation_ui::module_rect(ctx, id))
            }
            Spot::Group => groups::level_nodes(graph, state.level)
                .into_iter()
                .filter(|id| matches!(graph.nodes[*id].user_data.kind, NodeKind::Group(_)))
                .find_map(|id| annotation_ui::module_rect(ctx, id)),
            Spot::Canvas => canvas.map(|p| Rect::from_center_size(p, Vec2::ZERO)),
        }
    }

    /// A point on empty canvas, as far from every module on show as the
    /// view allows, for a right-click.
    fn empty_canvas(&self, ctx: &egui::Context) -> Option<Pos2> {
        let area = self.editor_rect.shrink2(vec2(110.0, 90.0));
        if !area.is_positive() {
            return None;
        }
        let modules: Vec<Rect> = groups::level_nodes(&self.graph_state.graph, self.user_state.level)
            .into_iter()
            .filter_map(|id| annotation_ui::module_rect(ctx, id))
            .collect();
        let (columns, rows) = (9, 7);
        let middle = area.center();
        (0..columns * rows)
            .map(|i| {
                let (c, r) = ((i % columns) as f32 / (columns - 1) as f32, (i / columns) as f32 / (rows - 1) as f32);
                pos2(area.left() + area.width() * c, area.top() + area.height() * r)
            })
            // Room enough for a menu to open is as good as any more: past
            // that, nearer the middle of the view is better
            .map(|p| {
                let room = modules.iter().map(|m| m.distance_to_pos(p)).fold(f32::INFINITY, f32::min).min(ROOM);
                (room - 0.25 * p.distance(middle), p)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }

    /// Draws the step: its glows, its ghost and its card. Returns what the
    /// card was asked to do.
    fn draw_lesson(&mut self, ctx: &egui::Context, now: f64, dt: f64) -> Option<CardAction> {
        let canvas = match self.lesson.as_ref()?.canvas {
            Some(p) => Some(p),
            None => {
                let p = self.empty_canvas(ctx);
                self.lesson.as_mut()?.canvas = p;
                p
            }
        };
        let run = self.lesson.as_ref()?;
        let step = run.current().cloned().unwrap_or_default();
        let points: Vec<Rect> = step.points.iter().filter_map(|s| self.lesson_spot(ctx, s, canvas)).collect();
        // The modules the parts are on, which the card would rather not cover
        let modules: Vec<Rect> = step
            .points
            .iter()
            .filter_map(|s| match s {
                Spot::Part(part) if part.frame().is_none() => Some(Spot::Module(part.module.clone(), part.which)),
                _ => None,
            })
            .filter_map(|s| self.lesson_spot(ctx, &s, canvas))
            .collect();
        let ghost: Vec<Option<Rect>> = step.show.iter().flat_map(|s| s.spots()).map(|s| self.lesson_spot(ctx, &s, canvas)).collect();

        // A part off the edge of the view is brought into it, gently, as the
        // step begins; after that the view is the learner's
        let began = run.began.unwrap_or(now);
        if let (Some(first), None, false) = (points.first(), run.met_at, ctx.input(|i| i.pointer.any_down())) {
            let comfort = self.editor_rect.shrink2(vec2(90.0, 80.0));
            let delta = comfort.clamp(first.center()) - first.center();
            if now - began < FOLLOW_SECONDS && comfort.is_positive() && delta.length() > 0.5 {
                self.graph_state.pan_zoom.pan += delta * (dt as f32 * 6.0).min(1.0);
            }
        }
        let run = self.lesson.as_ref()?;

        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("lesson_glow")));
        let breath = 0.5 + 0.5 * (now * 2.6).sin() as f32;
        if run.met_at.is_none() {
            for rect in &points {
                glow(&painter, *rect, breath);
            }
        }

        // The ghost: once as the step begins, and again when asked
        let ghost_from = run.ghost_from.unwrap_or(began + GHOST_DELAY);
        if let (Some(show), None) = (&step.show, run.met_at) {
            let spots: Option<Vec<Rect>> = ghost.iter().copied().collect();
            if let Some(spots) = spots {
                let ghost_painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("lesson_ghost")));
                draw_ghost(&ghost_painter, show, &spots, now - ghost_from);
            }
        }

        // The card beside the first part on show, clear of the move
        let mut keep_clear: Vec<Rect> = points.iter().chain(ghost.iter().flatten()).map(|r| r.expand(18.0)).collect();
        // ...and of the way between them, where a cable or a drag goes
        let ghost_spots: Vec<Rect> = ghost.iter().flatten().copied().collect();
        for pair in points.windows(2).chain(ghost_spots.windows(2)) {
            let (a, b) = (pair[0].center(), pair[1].center());
            keep_clear.extend((1..10).map(|i| Rect::from_center_size(a.lerp(b, i as f32 / 10.0), vec2(40.0, 40.0))));
        }
        let anchor = points.first().or(ghost.iter().flatten().next()).copied();
        self.lesson_card(ctx, anchor, (&keep_clear, &modules), now)
    }

    /// The caption: what to do, how far through the lesson it is, and the
    /// way out.
    fn lesson_card(&self, ctx: &egui::Context, anchor: Option<Rect>, keep_clear: (&[Rect], &[Rect]), now: f64) -> Option<CardAction> {
        let run = self.lesson.as_ref()?;
        let id = egui::Id::new("lesson_card");
        let size = ctx.memory(|m| m.area_rect(id)).map_or(vec2(CARD_WIDTH, 170.0), |r| r.size());
        let bounds = self.editor_rect.shrink(12.0);
        let at = card_position(anchor, size, bounds, keep_clear);
        // Glide rather than jump as the part it's beside moves or changes
        let x = ctx.animate_value_with_time(id.with("x"), at.x, 0.25);
        let y = ctx.animate_value_with_time(id.with("y"), at.y, 0.25);

        if let (Some(anchor), Some(card)) = (anchor, ctx.memory(|m| m.area_rect(id))) {
            if !card.expand(4.0).contains(anchor.center()) {
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("lesson_leader")));
                leader(&painter, card, anchor);
            }
        }

        let step = run.current();
        let finished = run.finished();
        let met = run.met_at.is_some();
        let fraction = step.and_then(|s| s.wait.as_ref()).and_then(|w| w.fraction(&run.progress));
        let next_lesson = lesson::after(run.file);
        let mut action = None;
        egui::Area::new(id)
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(x, y))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(theme::background::PANEL.gamma_multiply(0.98))
                    .stroke(Stroke::new(1.0_f32, GOLD.gamma_multiply(if met { 0.9 } else { 0.55 })))
                    .corner_radius(12.0)
                    .shadow(egui::epaint::Shadow { offset: [0, 6], blur: 22, spread: 0, color: Color32::from_black_alpha(110) })
                    .inner_margin(egui::Margin::symmetric(16, 13))
                    .show(ui, |ui| {
                        ui.set_width(CARD_WIDTH - 32.0);
                        // Which lesson, how far, and the way out
                        ui.horizontal(|ui| {
                            let heading = format!("LESSON  ·  {}", run.lesson.title.to_uppercase());
                            ui.label(egui::RichText::new(heading).color(GOLD).size(10.5).extra_letter_spacing(0.8));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let leave = ui.add(egui::Button::new(egui::RichText::new("×").size(15.0).color(theme::text::SECONDARY)).frame(false));
                                if leave.on_hover_text("Leave the lesson, keeping the patch").clicked() {
                                    action = Some(CardAction::Leave);
                                }
                                if !finished {
                                    let count = format!("{} / {}", run.number(), run.lesson.waits());
                                    ui.label(egui::RichText::new(count).color(theme::text::QUIET).size(11.0));
                                }
                            });
                        });
                        ui.add_space(4.0);
                        progress_dots(ui, run.number() - 1 + usize::from(met), run.lesson.waits(), finished);
                        ui.add_space(8.0);

                        if finished {
                            ui.label(egui::RichText::new("Lesson complete").font(title_font(17.0)).color(EMPHASIS));
                            ui.add_space(4.0);
                        }
                        let paragraphs = step.map(|s| s.say.as_slice()).unwrap_or_default();
                        for (i, text) in paragraphs.iter().enumerate() {
                            if i > 0 {
                                ui.add_space(6.0);
                            }
                            let job = caption_job(text, CARD_WIDTH - 32.0, i > 0);
                            ui.label(job);
                        }

                        if let Some(fraction) = fraction.filter(|_| !met) {
                            ui.add_space(8.0);
                            meter(ui, fraction);
                        }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if met {
                                ui.label(egui::RichText::new("✔  Done").color(theme::accent::SUCCESS).strong());
                                return;
                            }
                            if finished {
                                if let Some(next) = next_lesson {
                                    let title = next.lesson().map(|l| l.title).unwrap_or_default();
                                    if gold_button(ui, &format!("Next: {title}  →")).clicked() {
                                        action = Some(CardAction::NextLesson(next));
                                    }
                                }
                                if ui.button("Done").on_hover_text("Close the lesson, keeping the patch").clicked() {
                                    action = Some(CardAction::Leave);
                                }
                                return;
                            }
                            let waits_for_next = matches!(step.and_then(|s| s.wait.as_ref()), Some(lesson::Wait::Next));
                            if waits_for_next && gold_button(ui, "Next  →").clicked() {
                                action = Some(CardAction::Next);
                            }
                            if step.is_some_and(|s| s.show.is_some()) && ui.button("👁 Show me").on_hover_text("Watch the move again").clicked() {
                                action = Some(CardAction::ShowMe);
                            }
                            if !waits_for_next {
                                // Quiet word that it's watching for the move
                                let dots = ".".repeat(1 + (now * 1.6) as usize % 3);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(egui::RichText::new(format!("your turn{dots:<3}")).color(theme::text::DISABLED).size(11.0).italics());
                                });
                            }
                        });
                    });
            });
        action
    }
}

/// Where the card goes: beside the part it's about (right, left, below or
/// above, whichever is clear of the parts and the ghost's move, and if it
/// can be, of the modules they're on), or along the bottom of the editor
/// when it's about nothing on show.
fn card_position(anchor: Option<Rect>, size: Vec2, bounds: Rect, (keep_clear, rather): (&[Rect], &[Rect])) -> Pos2 {
    let Some(anchor) = anchor else {
        return pos2(bounds.center().x - size.x / 2.0, bounds.bottom() - size.y - 24.0);
    };
    let c = anchor.center();
    let candidates = [
        pos2(anchor.right() + CARD_GAP, c.y - size.y / 2.0),
        pos2(anchor.left() - CARD_GAP - size.x, c.y - size.y / 2.0),
        pos2(c.x - size.x / 2.0, anchor.bottom() + CARD_GAP),
        pos2(c.x - size.x / 2.0, anchor.top() - CARD_GAP - size.y),
    ];
    let clamp = |p: Pos2| {
        let max = (bounds.max - size).max(bounds.min);
        p.clamp(bounds.min, max)
    };
    let fits = |p: Pos2| bounds.contains_rect(Rect::from_min_size(p, size));
    let clear = |p: Pos2| !keep_clear.iter().any(|r| r.intersects(Rect::from_min_size(p, size)));
    let clearer = |p: Pos2| clear(p) && !rather.iter().any(|r| r.intersects(Rect::from_min_size(p, size)));
    // Nowhere clear: the bottom or top of the editor, or wherever covers least
    let edges = [
        pos2(bounds.center().x - size.x / 2.0, bounds.bottom() - size.y - 24.0),
        pos2(bounds.center().x - size.x / 2.0, bounds.top() + 24.0),
    ];
    let covered = |p: Pos2| -> f32 {
        let card = Rect::from_min_size(p, size);
        keep_clear.iter().map(|r| r.intersect(card)).filter(|r| r.is_positive()).map(|r| r.area()).sum()
    };
    candidates
        .iter()
        .copied()
        .find(|p| fits(*p) && clearer(*p))
        .or_else(|| candidates.iter().copied().find(|p| fits(*p) && clear(*p)))
        .or_else(|| candidates.iter().copied().map(clamp).find(|p| clear(*p)))
        .unwrap_or_else(|| {
            candidates.iter().copied().map(clamp).chain(edges.map(clamp)).min_by(|a, b| covered(*a).total_cmp(&covered(*b))).unwrap_or(edges[0])
        })
}

/// A soft, breathing ring around a part: a jack, a knob, a module, or a
/// point on the canvas.
fn glow(painter: &egui::Painter, rect: Rect, breath: f32) {
    let alpha = 0.45 + 0.55 * breath;
    if rect.width().max(rect.height()) < 48.0 {
        let radius = rect.size().max_elem() / 2.0 + 7.0 + 2.0 * breath;
        for (grow, width, a) in [(7.0, 7.0, 0.12), (3.5, 4.0_f32, 0.22), (0.0, 1.8, 0.9)] {
            painter.circle_stroke(rect.center(), radius + grow, Stroke::new(width, GOLD.gamma_multiply(a * alpha)));
        }
    } else {
        let rect = rect.expand(6.0 + 2.0 * breath);
        for (grow, width, a) in [(6.0, 7.0, 0.10), (3.0, 4.0_f32, 0.2), (0.0, 1.8, 0.85)] {
            painter.rect_stroke(rect.expand(grow), 14.0 + grow, Stroke::new(width, GOLD.gamma_multiply(a * alpha)), egui::StrokeKind::Middle);
        }
    }
}

/// A faint dotted line from the card to the part it's about.
fn leader(painter: &egui::Painter, card: Rect, anchor: Rect) {
    let to = anchor.center();
    let from = card.clamp(to);
    let length = from.distance(to);
    // Stop short of the glow around the part
    let stop = anchor.size().max_elem() / 2.0 + 14.0;
    if length <= stop + 6.0 {
        return;
    }
    let dir = (to - from) / length;
    let mut d = 0.0;
    while d < length - stop {
        painter.circle_filled(from + dir * d, 1.3, GOLD.gamma_multiply(0.55));
        d += 7.0;
    }
}

/// The ghost pointer, `t` seconds into its move: it fades in near the
/// first spot, glides onto it, does what the step asks and fades away. A
/// drag draws a cable behind it as it goes.
fn draw_ghost(painter: &egui::Painter, show: &Show, spots: &[Rect], t: f64) {
    let carry_end = GHOST_PRESS + GHOST_CARRY;
    let action_end = match show {
        Show::Move(_) => GHOST_ARRIVE,
        Show::Click(_) | Show::RightClick(_) => GHOST_PRESS + 0.2,
        Show::DoubleClick(_) => GHOST_PRESS + 0.45,
        Show::Drag(..) | Show::Turn(..) => carry_end + 0.1,
    };
    let end = action_end + GHOST_LINGER + GHOST_FADE;
    if !(0.0..end).contains(&t) {
        return;
    }
    let opacity = (t / GHOST_APPEAR).min((end - t) / GHOST_FADE).clamp(0.0, 1.0) as f32 * 0.92;
    let first = spots[0].center();
    let start = first + vec2(70.0, 90.0);
    let eased = |from: f64, to: f64| ease(((t - from) / (to - from)).clamp(0.0, 1.0) as f32);
    let target = match show {
        Show::Drag(..) => spots.get(1).map_or(first, |r| r.center()),
        Show::Turn(_, up) => first + vec2(0.0, if *up { -64.0 } else { 64.0 }),
        _ => first,
    };
    let p = if t < GHOST_ARRIVE {
        start + (first - start) * eased(GHOST_APPEAR * 0.5, GHOST_ARRIVE)
    } else {
        first + (target - first) * eased(GHOST_PRESS + 0.1, carry_end)
    };

    let pressed = |at: f64, length: f64| (at..at + length).contains(&t);
    let held = match show {
        Show::Drag(..) | Show::Turn(..) => pressed(GHOST_PRESS, carry_end + 0.1 - GHOST_PRESS),
        Show::Click(_) | Show::RightClick(_) => pressed(GHOST_PRESS, 0.16),
        Show::DoubleClick(_) => pressed(GHOST_PRESS, 0.1) || pressed(GHOST_PRESS + 0.25, 0.1),
        Show::Move(_) => false,
    };
    // A cable follows a drag from a jack
    if let (Show::Drag(..), true) = (show, t >= GHOST_PRESS) {
        let to = if t < carry_end + 0.1 { p } else { target };
        let pull = vec2(((to.x - first.x).abs() * 0.5).max(30.0), 0.0);
        let curve = |width: f32, a: f32| {
            egui::epaint::CubicBezierShape::from_points_stroke([first, first + pull, to - pull, to], false, Color32::TRANSPARENT, Stroke::new(width, GOLD.gamma_multiply(a * opacity)))
        };
        painter.add(curve(6.0, 0.15));
        painter.add(curve(2.0, 0.75));
    }
    // A ripple where it presses
    for at in [GHOST_PRESS, GHOST_PRESS + 0.25] {
        let since = t - at;
        let double = matches!(show, Show::DoubleClick(_));
        if (0.0..0.5).contains(&since) && (at == GHOST_PRESS || double) {
            let k = (since / 0.5) as f32;
            painter.circle_stroke(first, 6.0 + 16.0 * k, Stroke::new(2.0_f32, GOLD.gamma_multiply((1.0 - k) * opacity)));
        }
    }
    if held {
        painter.circle_stroke(p, 9.0, Stroke::new(1.5_f32, Color32::WHITE.gamma_multiply(0.5 * opacity)));
    }
    draw_pointer(painter, p, opacity);
    // A right-click says so, since the ghost has no buttons to show it with
    let label = match show {
        Show::RightClick(_) => Some("right-click"),
        Show::DoubleClick(_) => Some("double-click"),
        _ => None,
    };
    if let Some(label) = label.filter(|_| t >= GHOST_ARRIVE - 0.2) {
        painter.text(p + vec2(18.0, 22.0), egui::Align2::LEFT_TOP, label, FontId::proportional(12.0), GOLD.gamma_multiply(opacity));
    }
}

/// Smoothstep, a hand that starts and stops gently.
fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

fn title_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::TITLE_FAMILY.into()))
}

/// A caption's paragraph: plain words, **bold** ones brighter, and `keys`
/// set as keycaps. A paragraph after the first is quieter, an aside.
fn caption_job(text: &str, wrap: f32, aside: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let size = if aside { 13.0 } else { 14.5 };
    let ink = if aside { theme::text::SECONDARY } else { theme::text::PRIMARY };
    for (run, bold) in emphasis_runs(text) {
        for (i, piece) in run.split('`').enumerate() {
            if piece.is_empty() {
                continue;
            }
            let format = if i % 2 == 1 {
                TextFormat {
                    font_id: FontId::monospace(size - 1.5),
                    color: EMPHASIS,
                    background: theme::background::WIDGET_ACTIVE,
                    ..Default::default()
                }
            } else if bold {
                TextFormat { font_id: title_font(size * 0.97), color: EMPHASIS, ..Default::default() }
            } else {
                TextFormat { font_id: FontId::proportional(size), color: ink, ..Default::default() }
            };
            // Keycaps get a hair of room inside their background
            let piece = if i % 2 == 1 { format!("\u{2009}{piece}\u{2009}") } else { piece.to_string() };
            job.append(&piece, 0.0, format);
        }
    }
    job
}

/// A row of dots, one a step: filled for the steps done.
fn progress_dots(ui: &mut egui::Ui, done: usize, total: usize, finished: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), egui::Sense::hover());
    let gap = 9.0;
    for i in 0..total {
        let c = pos2(rect.left() + 3.0 + i as f32 * gap, rect.center().y);
        if i < done || finished {
            ui.painter().circle_filled(c, 2.6, GOLD);
        } else {
            ui.painter().circle_stroke(c, 2.4, Stroke::new(1.0_f32, GOLD.gamma_multiply(0.45)));
        }
    }
}

/// How far a timed wait has got: seconds listened, notes played.
fn meter(ui: &mut egui::Ui, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, theme::background::WIDGET);
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
    ui.painter().rect_filled(filled, 2.0, GOLD.gamma_multiply(0.85));
}

/// The card's main button, in the lesson's colour.
fn gold_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::from_rgb(40, 30, 10)).strong())
            .fill(GOLD)
            .corner_radius(6.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_sits_beside_its_part_clear_of_the_move() {
        let bounds = Rect::from_min_max(pos2(0.0, 80.0), pos2(1280.0, 700.0));
        let size = vec2(330.0, 170.0);
        let jack = Rect::from_center_size(pos2(300.0, 300.0), vec2(12.0, 12.0));
        // To its right, by default
        let at = card_position(Some(jack), size, bounds, (&[jack], &[]));
        assert_eq!(at.x, jack.right() + CARD_GAP);
        // ...unless the drag goes there: then to its left, or with no room
        // there, below
        let other = Rect::from_center_size(pos2(500.0, 300.0), vec2(12.0, 12.0)).expand(18.0);
        let at = card_position(Some(jack), size, bounds, (&[jack, other], &[]));
        assert_eq!(at.y, jack.bottom() + CARD_GAP, "{at:?}");
        let far = Rect::from_center_size(pos2(700.0, 300.0), vec2(12.0, 12.0));
        let at = card_position(Some(far), size, bounds, (&[far, other.translate(vec2(400.0, 0.0))], &[]));
        assert_eq!(at.x + size.x, far.left() - CARD_GAP, "{at:?}");
        // Near the right edge it can't go right
        let edge = Rect::from_center_size(pos2(1200.0, 300.0), vec2(12.0, 12.0));
        let at = card_position(Some(edge), size, bounds, (&[edge], &[]));
        assert!(at.x + size.x <= edge.left());
        // About nothing on show: along the bottom, in the middle
        let at = card_position(None, size, bounds, (&[], &[]));
        assert_eq!((at.x + size.x / 2.0, at.y + size.y), (640.0, 700.0 - 24.0));
        // Off the module the jack is on, where there's room
        let module = Rect::from_min_max(pos2(100.0, 250.0), pos2(306.0, 600.0));
        let input = Rect::from_center_size(pos2(800.0, 300.0), vec2(12.0, 12.0));
        let on = Rect::from_min_max(pos2(794.0, 250.0), pos2(1000.0, 600.0));
        assert_eq!(card_position(Some(jack), size, bounds, (&[jack], &[module])).x, jack.right() + CARD_GAP);
        assert_eq!(card_position(Some(input), size, bounds, (&[input], &[on])).x + size.x, input.left() - CARD_GAP);
        // Hemmed in on every side: wherever covers least
        let wide = Rect::from_min_max(pos2(200.0, 100.0), pos2(1100.0, 560.0));
        let at = card_position(Some(Rect::from_center_size(pos2(640.0, 300.0), vec2(40.0, 40.0))), size, bounds, (&[wide], &[]));
        assert_eq!(at.y, 700.0 - size.y - 24.0, "{at:?}");
        // Always inside the editor
        let corner = Rect::from_center_size(pos2(5.0, 85.0), vec2(4.0, 4.0));
        let at = card_position(Some(corner), size, bounds, (&[bounds], &[]));
        assert!(bounds.contains_rect(Rect::from_min_size(at, size)), "{at:?}");
    }

    #[test]
    fn keys_and_bold_words_are_set_apart() {
        let job = caption_job("Press **Play**, then `Z`.", 300.0, false);
        let texts: Vec<&str> = job.sections.iter().map(|s| &job.text[s.byte_range.clone()]).collect();
        assert_eq!(texts, ["Press ", "Play", ", then ", "\u{2009}Z\u{2009}", "."]);
        assert_eq!(job.sections[1].format.color, EMPHASIS);
        assert_eq!(job.sections[3].format.background, theme::background::WIDGET_ACTIVE);
    }
}
