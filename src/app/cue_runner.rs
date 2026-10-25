//! Plays cues into the app: the pointer, keys and buttons as input events,
//! knobs turning, the camera moving. It keeps no clock of its own: the
//! capture drives it a video frame at a time, and a presentation from the
//! wall clock (#487). What only a recording can do (`still`, `layout`,
//! `input`, `end`) is handed back to whoever drives it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui::{Pos2, Vec2};

use super::capture::draw_pointer;
use super::capture_aim::{Part, Spot, Which};
use super::cues::{Cue, LookAt};
use crate::engine::MidiEvent;

/// Something the app has to do for the script this frame, beyond input.
#[derive(Debug, Clone)]
pub enum CaptureAction {
    Play(bool),
    /// A MIDI event, `offset` samples into the audio the engine renders next.
    Midi { event: MidiEvent, offset: u32 },
    /// Set a module's input to a value in real units.
    SetParam { module: String, which: Which, input: String, value: f32 },
    /// Audio Input modules are now hearing this file, of this many channels.
    InputFile(String, usize),
    /// Write where each module is drawn, in patch space, to this file.
    Layout(PathBuf),
    /// Frame the whole level, over this many seconds (0 cuts).
    Fit(f64),
    /// Centre a patch point at a zoom, over this many seconds (0 cuts).
    Aim { at: Pos2, zoom: f32, seconds: f64 },
    /// Bring a module or frame into view, framed or centred at a zoom, over
    /// this many seconds (0 cuts).
    Look { at: LookAt, zoom: Option<f32>, seconds: f64 },
    /// Dive into the group of this name, or path from the top, landing on
    /// all of it or aimed at a patch point inside at a zoom.
    Enter(String, Option<(Pos2, f32)>),
    /// Dive back out of the group on show.
    Leave,
}

/// A value easing from one point to another over a span of time.
#[derive(Debug, Clone, Copy)]
struct Glide<T> {
    from: T,
    to: T,
    start: f64,
    dur: f64,
}

impl<T: Copy + std::ops::Add<Output = T> + std::ops::Sub<Output = T> + std::ops::Mul<f32, Output = T>> Glide<T> {
    /// Where the glide is at `t`, eased in and out.
    fn at(&self, t: f64) -> T {
        self.from + (self.to - self.from) * ease(self.progress(t))
    }

    fn progress(&self, t: f64) -> f32 {
        if self.dur <= 0.0 { 1.0 } else { ((t - self.start) / self.dur).clamp(0.0, 1.0) as f32 }
    }

    fn done(&self, t: f64) -> bool {
        self.progress(t) >= 1.0
    }
}

/// A camera move: scale eases in log space, so a zoom feels even, and the
/// point at the centre of the view travels a straight line across the patch.
#[derive(Debug, Clone, Copy)]
struct CameraGlide {
    from: (f32, Vec2),
    to: (f32, Vec2),
    start: f64,
    dur: f64,
}

impl CameraGlide {
    fn progress(&self, t: f64) -> f32 {
        ((t - self.start) / self.dur).clamp(0.0, 1.0) as f32
    }

    fn at(&self, t: f64) -> (f32, Vec2) {
        let p = ease(self.progress(t));
        let scale = (self.from.0.ln() + (self.to.0.ln() - self.from.0.ln()) * p).exp();
        // The offset scales with the zoom (see `animate`), so offset / scale
        // is where the view's centre sits in the patch. Easing that, not the
        // offset itself, keeps a deep pull-back from swinging out past the
        // edge of the patch on its way to the wide shot
        let (a, b) = (self.from.1 / self.from.0, self.to.1 / self.to.0);
        (scale, (a + (b - a) * p) * scale)
    }
}

/// Smoothstep, the motion of a hand that starts and stops gently.
fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// A parameter turning from one value to another.
#[derive(Debug, Clone)]
struct Ramp {
    module: String,
    which: Which,
    input: String,
    /// Filled in from the parameter's value when the ramp starts.
    from: Option<f32>,
    to: f32,
    start: f64,
    dur: f64,
}

/// Cues on their way into the app, and what they've set moving. A
/// presentation keeps a copy as each beat begins, to play it again from.
#[derive(Clone)]
pub(crate) struct CueRunner {
    cues: Vec<(f64, Cue)>,
    next_cue: usize,
    /// The shortest a camera move or zoom takes: a capture's frame.
    step: f64,
    actions: Vec<CaptureAction>,
    /// A cue couldn't find what it aimed at.
    failed: bool,
    cursor_visible: bool,
    /// Where the pointer is; `None` until the script first moves it, so
    /// nothing is hovered by accident.
    pointer: Option<Pos2>,
    pointer_glide: Option<Glide<Vec2>>,
    /// The part the pointer is gliding to, followed as the view moves.
    pointer_aim: Option<Part>,
    button_down: bool,
    /// Modifier keys held with the last key pressed.
    modifiers: egui::Modifiers,
    ramps: Vec<Ramp>,
    pan_glide: Option<Glide<Vec2>>,
    /// Zoom still to apply: (log of the factor left, frames left).
    zoom_left: Option<(f32, f64, f64)>,
    /// The virtual camera, relative to the view when it was first moved
    /// or last fitted: the screen is scaled by `scale` about the editor's
    /// centre, then shifted by `offset` points.
    camera: (f32, Vec2),
    camera_glide: Option<CameraGlide>,
    /// Pan to add this frame, from the camera.
    pending_pan_delta: Vec2,
    pending_zoom: f32,
    /// A file dragged over the window, as from the desktop.
    hovering: Option<PathBuf>,
    /// Where this frame's presses and right-clicks landed, and their cue
    /// times, to check against what egui found under them.
    presses: Vec<(f64, &'static str, Pos2)>,
    /// Everything moving gets where it's going on the next `animate`.
    landing: bool,
    /// Every cue fired, as it fired, while a take is kept (#489).
    log: Option<Vec<(f64, Cue)>>,
}

impl CueRunner {
    /// A runner for `cues`, in time order, whose camera moves and zooms take
    /// at least `step` seconds.
    pub fn new(cues: Vec<(f64, Cue)>, step: f64) -> Self {
        Self {
            cues,
            next_cue: 0,
            step,
            actions: Vec::new(),
            failed: false,
            cursor_visible: false,
            pointer: None,
            pointer_glide: None,
            pointer_aim: None,
            button_down: false,
            modifiers: egui::Modifiers::default(),
            ramps: Vec::new(),
            pan_glide: None,
            zoom_left: None,
            camera: (1.0, Vec2::ZERO),
            camera_glide: None,
            pending_pan_delta: Vec2::ZERO,
            pending_zoom: 1.0,
            hovering: None,
            presses: Vec::new(),
            landing: false,
            log: None,
        }
    }

    /// Keeps every cue as it fires from now on, for [`take_log`](Self::take_log).
    pub fn keep_log(&mut self) {
        self.log.get_or_insert_with(Vec::new);
    }

    /// The cues fired since this was last asked, and when.
    pub fn take_log(&mut self) -> Vec<(f64, Cue)> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Plays `cues` from here, a presentation's next beat, timed from 0
    /// again. What the last beat left (the pointer, the camera) stays.
    pub fn load(&mut self, cues: Vec<(f64, Cue)>) {
        self.cues = cues;
        self.next_cue = 0;
        self.failed = false;
        self.presses.clear();
    }

    /// Whether every cue has fired and everything they set moving has
    /// arrived: no glide or ramp under way, nothing held.
    pub fn done(&self) -> bool {
        self.next_cue >= self.cues.len()
            && self.ramps.is_empty()
            && self.pointer_glide.is_none()
            && self.pan_glide.is_none()
            && self.zoom_left.is_none()
            && self.camera_glide.is_none()
            && !self.button_down
            && self.hovering.is_none()
            && self.modifiers.is_none()
    }

    /// Ends the cues on the spot at `now`, as they'd have left things:
    /// knobs and the camera where they were going (camera moves cut), the
    /// transport and MIDI as sent, and anything held let go. What would
    /// start something (a note, a press, a key, typing, a pointer move)
    /// doesn't. What's left fires now, then a `land`, so a take writes
    /// the same and a film of it ends the beat the same way.
    pub fn finish(&mut self, now: f64, raw: &mut egui::RawInput, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        let held = self.button_down;
        // Notes not yet played, whose ends aren't either
        let mut unplayed = Vec::new();
        let rest = self.cues.split_off(self.next_cue);
        let mut released = !held;
        self.cues.extend(rest.into_iter().filter_map(|(_, cue)| {
            let cue = match cue {
                Cue::Note { channel, note, on: true, .. } => {
                    unplayed.push((channel, note));
                    return None;
                }
                Cue::Note { channel, note, on: false, .. } if unplayed.contains(&(channel, note)) => {
                    unplayed.retain(|n| *n != (channel, note));
                    return None;
                }
                Cue::KeyDown(..)
                | Cue::Text(_)
                | Cue::Move { .. }
                | Cue::Button(true)
                | Cue::SecondaryButton(_)
                | Cue::Wheel { .. }
                | Cue::Hover(Some(_))
                | Cue::Drop => return None,
                Cue::Button(false) if released => return None,
                Cue::Button(false) => {
                    released = true;
                    cue
                }
                Cue::Fit(_) => Cue::Fit(0.0),
                Cue::Aim { at, zoom, .. } => Cue::Aim { at, zoom, dur: 0.0 },
                Cue::Look { at, zoom, .. } => Cue::Look { at, zoom, dur: 0.0 },
                cue => cue,
            };
            Some((now, cue))
        }));
        // Anything still held is let go
        if !released {
            self.cues.push((now, Cue::Button(false)));
        }
        if self.hovering.is_some() && !self.cues[self.next_cue..].iter().any(|(_, cue)| matches!(cue, Cue::Hover(None) | Cue::Drop)) {
            self.cues.push((now, Cue::Hover(None)));
        }
        self.cues.push((now, Cue::Land));
        self.fire(f64::INFINITY, raw, locate, &mut |_, _| None);
        self.modifiers = egui::Modifiers::default();
    }

    /// Whatever is moving gets where it's going: the knobs this pass, the
    /// pointer and camera on the next `animate`.
    fn land(&mut self) {
        for ramp in &mut self.ramps {
            ramp.dur = 0.0;
        }
        if let Some(glide) = self.pan_glide.as_mut() {
            glide.dur = 0.0;
        }
        self.landing = true;
    }

    /// Adds to the app's work, in order with what the cues ask for.
    pub fn push(&mut self, action: CaptureAction) {
        self.actions.push(action);
    }

    /// Whether a cue has failed to find what it aimed at.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Modifier keys held with the last key pressed.
    pub fn modifiers(&self) -> egui::Modifiers {
        self.modifiers
    }

    /// Whether the cues before `end` only reach the sound, so a frame that
    /// fires them needn't be drawn.
    pub fn only_heard_before(&self, end: f64) -> bool {
        self.cues[self.next_cue..].iter().take_while(|(at, _)| *at < end - 1e-9).all(|(_, cue)| {
            matches!(
                cue,
                Cue::Play(_) | Cue::Note { .. } | Cue::Midi(_) | Cue::Param { .. } | Cue::Cursor(_) | Cue::Still(_) | Cue::Input(_)
            )
        })
    }

    /// Whether nothing on screen is moving or held: no glide, pan or zoom
    /// under way, no button, file or modifier held.
    pub fn is_still(&self) -> bool {
        self.pointer_glide.is_none()
            && self.pan_glide.is_none()
            && self.zoom_left.is_none()
            && self.camera_glide.is_none()
            && self.pending_pan_delta == Vec2::ZERO
            && self.pending_zoom == 1.0
            && !self.button_down
            && self.hovering.is_none()
            && self.modifiers.is_none()
    }

    /// Tells the pass which file is dragged over the window, if one is.
    pub fn hover_into(&self, raw: &mut egui::RawInput) {
        raw.hovered_files.clear();
        if let Some(path) = &self.hovering {
            raw.hovered_files.push(egui::HoveredFile { path: Some(path.clone()), ..Default::default() });
        }
    }

    /// Fires the cues before `end`, into `raw` and the app's work.
    /// `locate` finds a module's part on screen; `recorder` takes the cues
    /// only a recording can do, and may answer with work for the app.
    pub fn fire(
        &mut self,
        end: f64,
        raw: &mut egui::RawInput,
        locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>,
        recorder: &mut dyn FnMut(f64, Cue) -> Option<CaptureAction>,
    ) {
        while let Some((at, cue)) = self.cues.get(self.next_cue).cloned() {
            if at >= end - 1e-9 {
                break;
            }
            self.next_cue += 1;
            if let Some(log) = self.log.as_mut() {
                log.push((at, cue.clone()));
            }
            match cue {
                Cue::Play(on) => self.actions.push(CaptureAction::Play(on)),
                Cue::KeyDown(key, modifiers) | Cue::KeyUp(key, modifiers) => {
                    let pressed = matches!(cue, Cue::KeyDown(..));
                    self.modifiers = if pressed { modifiers } else { egui::Modifiers::default() };
                    raw.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    });
                }
                Cue::Text(text) => raw.events.push(egui::Event::Text(text)),
                // A capture places its MIDI by sample, apart from these cues;
                // live, it's sent as it comes
                Cue::Note { .. } | Cue::Midi(_) => {
                    if let Some(event) = cue.midi_event() {
                        self.actions.push(CaptureAction::Midi { event, offset: 0 });
                    }
                }
                Cue::Param { module, which, input, value, dur } => self.ramps.push(Ramp {
                    module,
                    which,
                    input,
                    from: None,
                    to: value,
                    start: at,
                    dur,
                }),
                Cue::Cursor(on) => self.cursor_visible = on,
                Cue::Move { to: spot, dur } => {
                    // Where a glide under way was headed, for a move by
                    let heading = self.pointer_glide.map(|g| g.to.to_pos2()).or(self.pointer);
                    let to = match &spot {
                        Spot::Point(p) => Ok(*p),
                        Spot::By(by) => heading.map(|p| p + *by).ok_or_else(|| "has no pointer to move from".to_string()),
                        Spot::Part(part) => locate(part),
                    };
                    let to = match to {
                        Ok(to) => to,
                        Err(e) => {
                            eprintln!("capture: the move at {at:.2} s {e}");
                            self.failed = true;
                            continue;
                        }
                    };
                    // The first move comes in from below and to the right
                    let from = self.pointer.unwrap_or(to + Vec2::new(150.0, 200.0));
                    self.pointer = Some(from);
                    self.pointer_glide = Some(Glide { from: from.to_vec2(), to: to.to_vec2(), start: at, dur });
                    self.pointer_aim = match spot {
                        Spot::Part(part) => Some(part),
                        _ => None,
                    };
                }
                Cue::PointerOff => {
                    self.pointer = None;
                    self.pointer_glide = None;
                    self.pointer_aim = None;
                }
                Cue::Button(down) => {
                    self.button_down = down;
                    if let (true, Some(p)) = (down, self.pointer) {
                        self.presses.push((at, "press", p));
                    }
                    raw.events.push(egui::Event::PointerButton {
                        pos: self.pointer.unwrap_or_default(),
                        button: egui::PointerButton::Primary,
                        pressed: down,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                Cue::SecondaryButton(down) => {
                    if let (true, Some(p)) = (down, self.pointer) {
                        self.presses.push((at, "rclick", p));
                    }
                    raw.events.push(egui::Event::PointerButton {
                        pos: self.pointer.unwrap_or_default(),
                        button: egui::PointerButton::Secondary,
                        pressed: down,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                Cue::Wheel { delta, modifiers } => {
                    raw.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, delta), modifiers, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel });
                }
                Cue::View { pan, dur } => {
                    // `from` is filled in by the app, which knows the pan
                    self.pan_glide = Some(Glide { from: Vec2::NAN, to: pan, start: at, dur });
                }
                Cue::Camera { offset, scale, dur } => {
                    self.camera_glide = Some(CameraGlide {
                        from: self.camera,
                        to: (scale, offset),
                        start: at,
                        dur: dur.max(self.step),
                    });
                }
                Cue::Fit(dur) => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Fit(dur));
                }
                Cue::Aim { at, zoom, dur } => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Aim { at, zoom, seconds: dur });
                }
                Cue::Look { at, zoom, dur } => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Look { at, zoom, seconds: dur });
                }
                Cue::Enter(name, aim) => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Enter(name, aim));
                }
                Cue::Leave => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Leave);
                }
                Cue::Zoom { factor, dur } => {
                    self.zoom_left = Some((factor.ln(), at, dur.max(self.step)));
                }
                Cue::Hover(path) => {
                    self.hovering = path;
                    self.hover_into(raw);
                }
                Cue::Drop => {
                    if let Some(path) = self.hovering.take() {
                        raw.dropped_files.push(Arc::new(DroppedPath(path)));
                    }
                    self.hover_into(raw);
                }
                Cue::Land => self.land(),
                Cue::Still(_) | Cue::Layout(_) | Cue::Input(_) | Cue::End => {
                    if let Some(action) = recorder(at, cue) {
                        self.actions.push(action);
                    }
                }
            }
        }
    }

    /// Keeps a glide to a part on it as the view moves. Not a drag's, which
    /// might be what moves it.
    pub fn follow_aim(&mut self, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        if let (Some(part), Some(glide), false) = (&self.pointer_aim, self.pointer_glide.as_mut(), self.button_down) {
            if let Ok(to) = locate(part) {
                glide.to = to.to_vec2();
            }
        }
    }

    /// Camera moves from here on are from the view the app is about to
    /// move to (a fit, an aim, a dive), and one under way stops.
    fn reset_camera(&mut self) {
        self.camera = (1.0, Vec2::ZERO);
        self.camera_glide = None;
        self.pending_pan_delta = Vec2::ZERO;
        self.pending_zoom = 1.0;
    }

    /// Advances the pointer, pan and zoom glides from `from` to `t`, or
    /// to their ends when the cues have landed.
    pub fn animate(&mut self, from: f64, t: f64, raw: &mut egui::RawInput) {
        let t = if std::mem::take(&mut self.landing) { f64::INFINITY } else { t };
        if let Some(glide) = self.pointer_glide {
            self.pointer = Some(glide.at(t).to_pos2());
            if glide.done(t) {
                self.pointer_glide = None;
                self.pointer_aim = None;
            }
        }
        // egui needs to know where the pointer is every frame
        let pointer = match self.pointer {
            Some(p) => egui::Event::PointerMoved(p),
            None => egui::Event::PointerGone,
        };
        raw.events.insert(0, pointer);

        if let Some((log_factor, start, dur)) = self.zoom_left {
            // This frame's share of the zoom, eased
            let p0 = ease(((from - start) / dur).clamp(0.0, 1.0) as f32);
            let p1 = ease(((t - start) / dur).clamp(0.0, 1.0) as f32);
            let r = (log_factor * (p1 - p0)).exp();
            self.pending_zoom *= r;
            self.pending_pan_delta *= r;
            if p1 >= 1.0 {
                self.zoom_left = None;
            }
        }

        if let Some(glide) = self.camera_glide {
            // Zooming by r scales the screen, and so the camera's offset,
            // about the centre; panning then makes up the rest
            let (scale, offset) = glide.at(t);
            let r = scale / self.camera.0;
            self.pending_zoom *= r;
            // Pan still waiting (the app holds the camera while a fit
            // glides) is scaled by the zoom, as it would have been
            self.pending_pan_delta = self.pending_pan_delta * r + offset - self.camera.1 * r;
            self.camera = (scale, offset);
            if glide.progress(t) >= 1.0 {
                self.camera_glide = None;
            }
        }
    }

    /// The pan the graph should have at `t`, given its current pan.
    pub fn take_pan(&mut self, t: f64, current: Vec2) -> Option<Vec2> {
        let delta = std::mem::take(&mut self.pending_pan_delta);
        let Some(glide) = self.pan_glide.as_mut() else {
            return (delta != Vec2::ZERO).then(|| current + delta);
        };
        if glide.from.x.is_nan() {
            glide.from = current;
        }
        let pan = glide.at(t);
        if glide.done(t) {
            self.pan_glide = None;
        }
        Some(pan + delta)
    }

    /// The zoom factor to apply this frame (1.0 for none).
    pub fn take_zoom(&mut self) -> f32 {
        std::mem::replace(&mut self.pending_zoom, 1.0)
    }

    /// The app's work for this frame.
    pub fn take_actions(&mut self) -> Vec<CaptureAction> {
        std::mem::take(&mut self.actions)
    }

    /// Where this frame's presses and right-clicks landed, and their cue
    /// times.
    pub fn take_presses(&mut self) -> Vec<(f64, &'static str, Pos2)> {
        std::mem::take(&mut self.presses)
    }

    /// The parameter values the script sets at `t`, given a way to read a
    /// parameter's current value.
    pub fn param_values(&mut self, t: f64, mut current: impl FnMut(&str, Which, &str) -> Option<f32>) -> Vec<CaptureAction> {
        let mut out = Vec::new();
        self.ramps.retain_mut(|ramp| {
            let from = *ramp.from.get_or_insert_with(|| current(&ramp.module, ramp.which, &ramp.input).unwrap_or(ramp.to));
            let p = if ramp.dur <= 0.0 { 1.0 } else { ease(((t - ramp.start) / ramp.dur).clamp(0.0, 1.0) as f32) };
            // Wide ranges of positive values (frequencies, times) turn evenly
            // in log space, as a knob with a log taper would
            let value = if from > 0.0 && ramp.to > 0.0 && (ramp.to / from).max(from / ramp.to) > 4.0 {
                (from.ln() + (ramp.to.ln() - from.ln()) * p).exp()
            } else {
                from + (ramp.to - from) * p
            };
            out.push(CaptureAction::SetParam {
                module: ramp.module.clone(),
                which: ramp.which,
                input: ramp.input.clone(),
                value,
            });
            p < 1.0
        });
        out
    }

    /// Draws the pointer, which a screenshot wouldn't otherwise show.
    pub fn draw_cursor(&self, ctx: &egui::Context) {
        let (true, Some(p)) = (self.cursor_visible, self.pointer) else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("capture_cursor")));
        draw_pointer(&painter, p, 1.0);
        if self.button_down {
            painter.circle_stroke(p, 9.0, egui::Stroke::new(1.5_f32, egui::Color32::from_white_alpha(120)));
        }
    }
}

/// A file a script drops, as one dropped from the desktop.
#[derive(Debug)]
struct DroppedPath(PathBuf);

impl egui::DroppedFile for DroppedPath {
    fn path(&self) -> &Path {
        &self.0
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }

    // Captures film the desktop app: a page has no files to drop from
    #[cfg(target_arch = "wasm32")]
    fn bytes_async(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + '_>> {
        Box::pin(std::future::ready(Err("a page can't read a script's files".to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::cues::{parse_cues, Mode};

    #[test]
    fn glide_eases_to_its_target() {
        let g = Glide { from: Vec2::ZERO, to: Vec2::new(10.0, 0.0), start: 1.0, dur: 2.0 };
        assert_eq!(g.at(0.0), Vec2::ZERO);
        assert_eq!(g.at(2.0), Vec2::new(5.0, 0.0));
        assert_eq!(g.at(5.0), Vec2::new(10.0, 0.0));
        assert!(g.done(3.0) && !g.done(2.9));
    }

    #[test]
    fn camera_glide_carries_the_centre_straight() {
        // From a close-up on a module at the patch's right edge (centre 5366
        // at zoom 1.4) out to the whole patch (centre 2962 at 0.2), as the
        // shots write them: offset = -(centre - c) * zoom
        let c = 960.0;
        let cam = |centre: f32, zoom: f32| (zoom, Vec2::new(-(centre - c) * zoom, 0.0));
        let g = CameraGlide { from: cam(5366.0, 1.4), to: cam(2962.0, 0.2), start: 0.0, dur: 1.0 };
        let centre = |t: f64| {
            let (zoom, offset) = g.at(t);
            c - offset.x / zoom
        };
        assert!((centre(0.0) - 5366.0).abs() < 0.01 && (centre(1.0) - 2962.0).abs() < 0.01);
        // Never past either end, and always moving the same way
        let mut last = centre(0.0);
        for i in 1..=50 {
            let x = centre(i as f64 / 50.0);
            assert!(x <= last + 1e-3 && x >= 2962.0 - 0.01, "centre {x} at step {i}");
            last = x;
        }
        assert!((g.at(0.5).0 - (1.4f32 * 0.2).sqrt()).abs() < 1e-4, "zoom eases in log space");
    }

    #[test]
    fn a_runner_on_the_wall_clock_fires_each_cue_once_in_order() {
        // As a presentation drives it: whenever the app gets round to a
        // frame, everything due by then
        let cues = parse_cues("0 play\n0.5 note 60 100 0.25\n0.5 param filter.svf Cutoff 800 1\n1 look filter.svf 1.4 0.8", Mode::Beat).unwrap();
        let mut runner = CueRunner::new(cues, 1e-3);
        let mut recorder = |_: f64, cue: Cue| panic!("a beat has no {cue:?}");
        let mut fired = Vec::new();
        for now in [0.0, 0.016, 0.6, 0.61, 2.0] {
            runner.fire(now + 1e-6, &mut egui::RawInput::default(), &mut |_| Err(String::new()), &mut recorder);
            fired.extend(runner.take_actions().into_iter().map(|a| match a {
                CaptureAction::Play(_) => "play".to_string(),
                CaptureAction::Midi { event: MidiEvent::NoteOn { .. }, offset: 0 } => "on".into(),
                CaptureAction::Midi { event: MidiEvent::NoteOff { .. }, offset: 0 } => "off".into(),
                CaptureAction::Look { zoom: Some(z), seconds, .. } => format!("look {z} {seconds}"),
                other => panic!("{other:?}"),
            }));
        }
        assert_eq!(fired, ["play", "on", "off", "look 1.4 0.8"]);
        // The ramp turns from wherever the knob was, and lands
        let at = |runner: &mut CueRunner, t| match runner.param_values(t, |_, _, _| Some(200.0)).as_slice() {
            [CaptureAction::SetParam { value, .. }] => Some(*value),
            [] => None,
            other => panic!("{other:?}"),
        };
        assert_eq!(at(&mut runner, 0.5), Some(200.0));
        assert!(at(&mut runner, 1.0).is_some_and(|v| v > 200.0 && v < 800.0));
        assert_eq!(at(&mut runner, 1.5), Some(800.0));
        assert_eq!(at(&mut runner, 1.6), None, "a finished ramp lets go of the knob");
    }
}
