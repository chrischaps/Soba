//! Presentation mode in the app (#488): each beat's cues played in live,
//! the presenter's notes in a window of their own (which OBS, filming the
//! main window, leaves out; in the browser, a panel in the corner), and a
//! "Beat 3/12" chip in the status bar. The script and its stepping are in
//! [`crate::app::presentation`].
//!
//! Real input stays live: the script's pointer is drawn over the patch, as
//! a lesson's ghost is, and only a beat with presses, drags, keys or typing
//! takes the mouse, for as long as it plays.
//!
//! The script's footswitches step it from MIDI, held back from everything
//! else while it's on show; and the run is kept as a take, a capture script
//! that films it again (#489). It's written as each beat begins and as the
//! presentation stops, to `--take`'s file or a new one in `takes/` beside
//! the script (#497); the notes' Save take writes it anywhere.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, vec2, Color32, FontFamily, FontId, Pos2, RichText, Stroke};
use egui::text::{LayoutJob, TextFormat};
use egui_node_graph2::{InputId, InputParamKind};

use crate::app::capture::CaptureAction;
use crate::app::presentation::{self, Command, Presentation};
use crate::app::{theme, WEB};
use crate::engine::MidiEvent;
use crate::graph::annotations::emphasis_runs;
use crate::graph::groups::{self, GroupIndex};
use crate::graph::{GroupId, SynthGraph};
use super::framing::{self, View};
use super::{find_input, SynthApp};

/// The tally light's red: a beat is playing.
const TALLY: Color32 = Color32::from_rgb(240, 84, 84);
/// Words the presenter leans on.
const EMPHASIS: Color32 = Color32::from_rgb(255, 234, 196);
/// How large the notes window sets what's said.
const SAY_SIZE: f32 = 24.0;

/// A presentation on show.
pub(super) struct Presenting {
    run: Presentation,
    /// The script's title, or its file's name.
    title: String,
    /// How things stood as each beat began, to play it again from.
    scenes: Vec<Option<Scene>>,
    /// Notes the beats have started and not yet let go, by channel.
    sounding: Vec<(u8, u8)>,
    /// The notes window is up (closing it hides it; the chip brings it back).
    notes_open: bool,
    /// Something else has the keys (a knob, a grid, a text field), so the
    /// presenter's don't reach the beats.
    keys_elsewhere: bool,
    /// The beat on show had a cue that missed, and the status bar said so.
    missed: bool,
    /// How things stood as the presentation began, to tell what changed
    /// by hand before the first press.
    opening: Scene,
    /// Where the take is written: `--take`, or a file of its own beside
    /// the script. `None` in the browser, which has only Save take.
    take_to: Option<PathBuf>,
    /// A beat began or was played again, so the take on disk is behind.
    take_grew: bool,
    /// Which of the script's footswitches are down.
    pedals_down: Vec<bool>,
    /// The script's window size, in points, until the window is asked for it.
    resize: Option<egui::Vec2>,
}

/// How things stood as a beat began: every knob, the view and the level,
/// and the transport.
#[derive(Clone)]
struct Scene {
    params: Vec<(InputId, f32)>,
    view: View,
    level: Option<GroupId>,
    playing: bool,
}

/// What the notes window asked for.
enum NotesAction {
    Command(Command),
    SaveTake,
    Stop,
}

impl SynthApp {
    /// Presents the script at `path` over the patch on show:
    /// `soba patch.json --present script.txt`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_presentation(&mut self, path: &std::path::Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
        let name = path.file_stem().map_or_else(|| path.display().to_string(), |s| s.to_string_lossy().into_owned());
        self.present_text(&name, &text).map_err(|e| format!("{}: {e}", path.display()))?;
        self.take_to(take_beside(path, chrono::Local::now().naive_local()));
        Ok(())
    }

    /// Presents a script's text, `name` its file's.
    pub(super) fn present_text(&mut self, name: &str, text: &str) -> Result<(), String> {
        if self.capture.is_some() {
            return Err("a capture is filming".into());
        }
        let script = presentation::parse(text)?;
        let title = script.title.clone().unwrap_or_else(|| name.to_string());
        let beats = script.beats.len();
        self.stop_presenting();
        // Faces note where their parts are drawn, for a move to find
        crate::graph::marks::start();
        // The footswitches are the presentation's alone while it's on show
        self.midi.set_pedals(script.pedals.iter().map(|p| (p.controller, p.channel)).collect());
        let keys = next_keys(&script);
        // The browser's canvas is the page's to size
        let resize = script.window.filter(|_| !WEB).map(|[w, h]| vec2(w, h));
        self.presenting = Some(Presenting {
            resize,
            pedals_down: vec![false; script.pedals.len()],
            run: Presentation::new(script),
            title: title.clone(),
            scenes: vec![None; beats],
            sounding: Vec::new(),
            notes_open: true,
            keys_elsewhere: false,
            missed: false,
            opening: self.scene(),
            take_to: None,
            take_grew: false,
        });
        self.status_message = Some(format!("Presenting {title}: {keys} plays the first of {beats} beats"));
        Ok(())
    }

    /// Writes the take to `path` rather than a new file beside the script:
    /// `soba patch.json --present script.txt --take take.txt`.
    pub fn take_to(&mut self, path: PathBuf) {
        if let Some(presenting) = self.presenting.as_mut() {
            presenting.take_to = Some(path);
        }
    }

    /// Help → Lessons → Present…: a script to present over the patch.
    pub(super) fn choose_presentation(&mut self, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        crate::app::web::pick_presentation(ctx, self.uploads.0.clone());

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            let Some(path) = rfd::FileDialog::new().add_filter("Presentation script", &["txt"]).pick_file() else { return };
            if let Err(e) = self.start_presentation(&path) {
                self.status_error(format!("Couldn't present {e}"));
            }
        }
    }

    /// Escape: the presentation ends, and the patch stays as the beats
    /// left it. The take is written, and the status bar says where.
    pub(super) fn stop_presenting(&mut self) {
        let written = self.write_take();
        let Some(presenting) = self.presenting.take() else { return };
        self.let_notes_go(&presenting.sounding);
        self.midi.set_pedals(Vec::new());
        if !matches!(written, Some(Err(_))) {
            let take = written.and_then(Result::ok).map(|path| format!(": the take is in {}", path.display())).unwrap_or_default();
            self.status_message = Some(format!("Stopped presenting {}{take}", presenting.title));
        }
    }

    /// The take so far as a capture script, once the first beat has begun.
    fn take_text(&self) -> Option<String> {
        let patch = self.document.path.as_ref().map(|p| p.display().to_string());
        self.presenting.as_ref()?.run.take_script(patch.as_deref())
    }

    /// Writes the take where it's kept, if there's one to write; says
    /// where, or what went wrong.
    fn write_take(&mut self) -> Option<Result<PathBuf, ()>> {
        let path = self.presenting.as_ref()?.take_to.clone()?;
        let text = self.take_text()?;
        let written = path.parent().filter(|p| !p.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all);
        Some(match written.and_then(|()| std::fs::write(&path, text)) {
            Ok(()) => Ok(path),
            Err(e) => {
                self.status_error(format!("Couldn't write the take to {}: {e}", path.display()));
                Err(())
            }
        })
    }

    /// The notes' Save take: the take so far, to a file of the presenter's
    /// choosing (in the browser, a download).
    fn save_take(&mut self) {
        let Some(text) = self.take_text() else { return };
        let title = self.presenting.as_ref().map_or_else(String::new, |p| p.title.clone());
        let name = format!("{} take.txt", title.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], " ").trim());
        #[cfg(target_arch = "wasm32")]
        if let Err(e) = crate::app::web::download(&name, &text) {
            self.status_error(format!("Couldn't save the take: {e}"));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = rfd::FileDialog::new().add_filter("Capture script", &["txt"]).set_file_name(&name).save_file() else { return };
            match std::fs::write(&path, text) {
                Ok(()) => self.status_message = Some(format!("Take saved: soba {} --capture {}", self.document.path.as_ref().map_or_else(|| "patch.json".into(), |p| p.display().to_string()), path.display())),
                Err(e) => self.status_error(format!("Couldn't save the take to {}: {e}", path.display())),
            }
        }
    }

    /// A presenter's footswitches, which nothing else heard: channel, CC
    /// and value. Each steps the beats as it goes down.
    pub(super) fn press_pedals(&mut self, heard: &[(u8, u8, u8)]) {
        let Some(presenting) = self.presenting.as_mut() else { return };
        for &(channel, controller, value) in heard {
            let pedals = &presenting.run.script.pedals;
            let Some(i) = pedals.iter().position(|p| p.hears(channel, controller)) else { continue };
            let (command, down) = (pedals[i].command, value >= 64);
            // Once a press: a pedal says when it's let go too
            if down && !presenting.pedals_down[i] {
                presenting.run.command(command);
            }
            presenting.pedals_down[i] = down;
        }
    }

    /// A key or the notes window asks the presentation for something.
    pub(super) fn present_command(&mut self, ctx: &egui::Context, command: Command) {
        if let Some(presenting) = self.presenting.as_mut() {
            presenting.run.command(command);
            ctx.request_repaint();
        }
    }

    /// Whether the presenter's keys reach the beats: only on the canvas
    /// with nothing else holding the keys, so a knob or a step's piano
    /// keeps its arrows and Page keys.
    pub(super) fn presenting_keys(&self, ctx: &egui::Context, on_canvas: bool) -> bool {
        self.presenting.is_some() && on_canvas && ctx.memory(|m| m.focused()).is_none() && !egui::Popup::is_any_open(ctx)
    }

    /// Before the pass: whatever was asked, then the beat's cues due by now.
    pub(super) fn prepare_presentation(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(presenting) = self.presenting.as_mut() else { return };
        let now = raw.time.unwrap_or_else(|| ctx.input(|i| i.time));
        let (graph, state, zoom) = (&self.graph_state.graph, &self.user_state, self.graph_state.pan_zoom.zoom);
        presenting.run.prepare_input(now, raw, &mut |part| crate::app::capture_aim::locate(ctx, graph, state, zoom, part));
    }

    /// Before anything is drawn: puts things back for a beat played again,
    /// notes how they stand as one begins, then does the beat's work.
    pub(super) fn step_presentation(&mut self, ctx: &egui::Context) {
        let Some(mut presenting) = self.presenting.take() else { return };
        if let Some(size) = presenting.resize.take() {
            // Out of maximized first, or the window keeps the screen's size
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        if let Some(k) = presenting.run.take_restore() {
            self.let_notes_go(&std::mem::take(&mut presenting.sounding));
            if let Some(scene) = presenting.scenes[k].clone() {
                self.restore_scene(&scene);
            }
            presenting.missed = false;
            presenting.take_grew = true;
        }
        if let Some(k) = presenting.run.take_began() {
            presenting.scenes[k] = Some(self.scene());
            presenting.missed = false;
            presenting.take_grew = true;
        }
        // The take opens on things as they stood at the first press
        if presenting.run.take_wants_opening() {
            let opening = self.take_opening(&presenting.opening);
            presenting.run.open_take(opening);
            // ...in a window the size its film will be
            if let Some(size) = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size()).filter(|_| !WEB) {
                presenting.run.take_window([size.x, size.y]);
                if let Some([w, h]) = presenting.run.window() {
                    if (size.x - w).abs() > 1.5 || (size.y - h).abs() > 1.5 {
                        self.status_error(format!(
                            "The window is {:.0}x{:.0} points, not the {w:.0}x{h:.0} the script asks for: the film won't place things as you see them",
                            size.x, size.y
                        ));
                    }
                }
            }
        }

        let mut actions = presenting.run.take_actions();
        let (graph, state) = (&self.graph_state.graph, &self.user_state);
        actions.extend(presenting.run.param_values(|module, which, input| {
            find_input(graph, state, module, which, input).ok().map(|id| graph.get_input(id).value.actual_value())
        }));
        let beat = presenting.run.beat().map_or(0, |k| k + 1);
        for action in actions {
            match self.apply_cue(ctx, action) {
                Ok(Some(CaptureAction::Play(on))) => {
                    if on != self.is_playing {
                        self.set_playing(on);
                    }
                }
                Ok(Some(CaptureAction::Midi { event, .. })) => {
                    match event {
                        MidiEvent::NoteOn { channel, note, .. } => presenting.sounding.push((channel, note)),
                        MidiEvent::NoteOff { channel, note, .. } => presenting.sounding.retain(|n| *n != (channel, note)),
                        _ => {}
                    }
                    self.midi.send_ui(event);
                }
                Ok(_) => {}
                Err(e) => self.raise_notice(format!("Beat {beat}: {e}")),
            }
        }
        if presenting.run.failed() && !presenting.missed {
            presenting.missed = true;
            self.raise_notice(format!("Beat {beat}: a move couldn't find what it aimed at"));
        }
        self.presenting = Some(presenting);
    }

    /// Does a script's cue to the view or a knob, as a capture and a
    /// presentation both do. Hands back what's the caller's to do.
    pub(super) fn apply_cue(&mut self, ctx: &egui::Context, action: CaptureAction) -> Result<Option<CaptureAction>, String> {
        match action {
            CaptureAction::Fit(seconds) => self.start_glide_over(ctx, framing::Target::All, seconds),
            CaptureAction::Aim { at, zoom, seconds } => {
                self.start_glide_over(ctx, framing::Target::Point { at, zoom }, seconds);
            }
            CaptureAction::Look { at, zoom, seconds } => {
                let target = self.look_target(&at, zoom).map_err(|e| format!("can't look at {at}: {e}"))?;
                self.start_glide_over(ctx, target, seconds);
            }
            CaptureAction::Enter(name, aim) => {
                self.dive_into_named(ctx, &name, aim).map_err(|e| format!("can't enter {name}: {e}"))?;
            }
            CaptureAction::Leave => {
                if !self.dive_out(ctx) {
                    return Err("no group to leave".into());
                }
            }
            CaptureAction::SetParam { module, which, input, value } => {
                let id = find_input(&self.graph_state.graph, &self.user_state, &module, which, &input).map_err(|e| format!("param {e}"))?;
                self.graph_state.graph.inputs[id].value.set_actual_value(value);
            }
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    /// The zoom a script's camera asks for this pass: a capture's or a
    /// presentation's (1.0 for none).
    pub(super) fn cue_zoom(&mut self) -> f32 {
        match (self.capture.as_mut(), self.presenting.as_mut()) {
            (Some(capture), _) => capture.take_zoom(),
            (None, Some(presenting)) => presenting.run.take_zoom(),
            (None, None) => 1.0,
        }
    }

    /// The pan a script's camera asks for this pass, given the pan now.
    pub(super) fn cue_pan(&mut self, current: egui::Vec2) -> Option<egui::Vec2> {
        match (self.capture.as_mut(), self.presenting.as_mut()) {
            (Some(capture), _) => capture.take_pan(current),
            (None, Some(presenting)) => presenting.run.take_pan(current),
            (None, None) => None,
        }
    }

    /// How things stand now, for a beat to begin again from.
    fn scene(&self) -> Scene {
        let params = self.graph_state.graph.inputs.iter().map(|(id, input)| (id, input.value.actual_value())).collect();
        Scene { params, view: self.view(), level: self.user_state.level, playing: self.is_playing }
    }

    /// Puts things back as they stood: knobs, level, view, transport.
    fn restore_scene(&mut self, scene: &Scene) {
        self.land_dive();
        self.glide = None;
        for &(id, value) in &scene.params {
            if let Some(input) = self.graph_state.graph.inputs.get_mut(id) {
                if input.value.actual_value() != value {
                    input.value.set_actual_value(value);
                }
            }
        }
        if self.user_state.level != scene.level {
            self.go_to_level(scene.level);
        }
        self.land_level_on(scene.view);
        if self.is_playing != scene.playing {
            self.set_playing(scene.playing);
        }
    }

    /// Cues that set things as they stand now: the view, and the level
    /// it's on, the transport, and any knob turned since `opening`.
    fn take_opening(&self, opening: &Scene) -> Vec<String> {
        let mut cues = Vec::new();
        let (at, zoom) = self.aim_of(self.view());
        match self.user_state.level.and_then(|level| self.group_path(level)) {
            Some(path) => cues.push(format!("enter {path} at {} {} {zoom}", at.x, at.y)),
            None => cues.push(format!("aim {} {} {zoom} 0", at.x, at.y)),
        }
        if self.is_playing {
            cues.push("play".into());
        }
        let before: HashMap<InputId, f32> = opening.params.iter().copied().collect();
        let graph = &self.graph_state.graph;
        for (id, input) in graph.inputs.iter() {
            let value = input.value.actual_value();
            if before.get(&id).is_some_and(|was| *was != value) {
                if let Some(knob) = knob_name(graph, id) {
                    cues.push(format!("param {knob} {value} 0"));
                }
            }
        }
        cues
    }

    /// How an `enter` cue names a group, from the top: `Bells/FM Bell`,
    /// `Strings/Pump#2`. `None` if the name wouldn't find it again.
    fn group_path(&self, id: GroupId) -> Option<String> {
        let graph = &self.graph_state.graph;
        let index = GroupIndex::of(graph);
        let position = |n| self.graph_state.node_positions.get(n).copied();
        let mut parts = Vec::new();
        let mut at = Some(id);
        while let Some(group) = at {
            let parent = index.parent_of(graph, group);
            let name = groups::name(graph, index.node(group)?);
            // Several of its name side by side: which, counted as a page reads
            let mut twins: Vec<(GroupId, Pos2)> = index
                .ids()
                .filter(|&g| index.parent_of(graph, g) == parent)
                .filter_map(|g| index.node(g).filter(|&n| groups::name(graph, n) == name).map(|n| (g, position(n).unwrap_or_default())))
                .collect();
            twins.sort_by(|a, b| a.1.y.total_cmp(&b.1.y).then(a.1.x.total_cmp(&b.1.x)));
            parts.push(match twins.len() {
                1 => name.to_string(),
                _ => format!("{name}#{}", twins.iter().position(|t| t.0 == group)? + 1),
            });
            at = parent;
        }
        parts.reverse();
        let path = parts.join("/");
        (groups::find_group(graph, position, None, &path).ok() == Some(id)).then_some(path)
    }

    /// Lets go of notes the beats started.
    fn let_notes_go(&mut self, sounding: &[(u8, u8)]) {
        for &(channel, note) in sounding {
            self.midi.send_ui(MidiEvent::NoteOff { channel, note, velocity: 0 });
        }
    }

    /// After the patch is drawn: the script's pointer, and the notes.
    pub(super) fn tend_presentation(&mut self, ctx: &egui::Context) {
        // The take is on disk as each beat begins, so a crash keeps it too,
        // and closing the window keeps the rest
        let grew = self.presenting.as_mut().is_some_and(|p| std::mem::take(&mut p.take_grew));
        if grew || ctx.input(|i| i.viewport().close_requested()) {
            self.write_take();
        }
        let Some(presenting) = self.presenting.as_mut() else { return };
        presenting.run.draw_cursor(ctx);
        let typing = crate::widgets::typing(ctx) || self.quick_add.is_some();
        presenting.keys_elsewhere = typing || ctx.memory(|m| m.focused()).is_some() || egui::Popup::is_any_open(ctx);
        if presenting.run.busy() {
            ctx.request_repaint();
        }
        if !presenting.notes_open {
            return;
        }
        let now = ctx.input(|i| i.time);
        let asked = if WEB {
            self.notes_corner(ctx, now)
        } else {
            let presenting = self.presenting.as_ref().expect("presenting");
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{} · notes", presenting.title))
                .with_inner_size([560.0, 460.0])
                .with_min_inner_size([360.0, 260.0]);
            let (asked, closed) = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("presenter_notes"), builder, |ui, _| {
                let closed = ui.input(|i| i.viewport().close_requested());
                let mut asked = notes_keys(ui);
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::background::MAIN).inner_margin(egui::Margin::symmetric(22, 18)))
                    .show(ui, |ui| asked.extend(draw_notes(ui, presenting, now)));
                (asked, closed)
            });
            if closed {
                if let Some(presenting) = self.presenting.as_mut() {
                    presenting.notes_open = false;
                }
            }
            asked
        };
        for action in asked {
            match action {
                NotesAction::Command(command) => self.present_command(ctx, command),
                NotesAction::SaveTake => self.save_take(),
                NotesAction::Stop => self.stop_presenting(),
            }
        }
    }

    /// The notes in the browser, which has one window: a panel in the
    /// editor's top right corner.
    fn notes_corner(&self, ctx: &egui::Context, now: f64) -> Vec<NotesAction> {
        let Some(presenting) = self.presenting.as_ref() else { return Vec::new() };
        let corner = self.editor_rect.right_top() + vec2(-14.0, 14.0);
        egui::Area::new(egui::Id::unique("presenter_notes"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(corner)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(theme::background::MAIN.gamma_multiply(0.96))
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.set_max_height(self.editor_rect.height() * 0.6);
                        draw_notes(ui, presenting, now)
                    })
                    .inner
            })
            .inner
    }

    /// The status bar's "Beat 3/12" chip, lit while a beat plays. A click
    /// brings the notes back.
    pub(super) fn presentation_chip(&mut self, ui: &mut egui::Ui) {
        let Some(presenting) = self.presenting.as_mut() else { return };
        let beats = presenting.run.script.beats.len();
        let on = presenting.run.beat().map_or(0, |k| k + 1);
        let (dot, color) = if presenting.run.playing() { ("●", TALLY) } else { ("○", theme::text::SECONDARY) };
        let text = RichText::new(format!("{dot} Beat {on}/{beats}")).color(color).small();
        let keys = next_keys(&presenting.run.script);
        let chip = ui
            .add(egui::Button::new(text).fill(theme::background::WIDGET).corner_radius(8.0).small())
            .on_hover_text(format!("Presenting {}: {keys} plays the next beat, Escape stops. Click for the notes", presenting.title));
        if chip.clicked() {
            presenting.notes_open = true;
        }
        ui.add_space(6.0);
    }
}

/// How a script names an input's knob: `filter.svf#2 Cutoff`. `None` for
/// what isn't a knob, or a name a script can't write (with a number in it,
/// which would read as the value).
fn knob_name(graph: &SynthGraph, id: InputId) -> Option<String> {
    let input = graph.inputs.get(id)?;
    if !matches!(input.kind, InputParamKind::ConstantOnly | InputParamKind::ConnectionOrConstant) {
        return None;
    }
    let node = graph.nodes.get(input.node)?;
    let kind = node.user_data.module_id;
    let nth = graph.nodes.iter().filter(|(_, n)| n.user_data.module_id == kind).position(|(n, _)| n == input.node)?;
    let (name, _) = node.inputs.iter().find(|(_, i)| *i == id)?;
    if name.split_whitespace().any(|w| w.parse::<f64>().is_ok()) {
        return None;
    }
    Some(format!("{kind}#{} {name}", nth + 1))
}

/// What plays the next beat: Page Down, and the script's footswitch.
fn next_keys(script: &presentation::Script) -> String {
    match script.pedals.iter().find(|p| p.command == Command::Next) {
        Some(pedal) => format!("Page Down or {}", pedal.name()),
        None => "Page Down".into(),
    }
}

/// Where a run of the script at `script` is kept without `--take`: a file
/// of its own in `takes/` beside it, named for the script and when it began,
/// so no run overwrites another.
#[cfg(not(target_arch = "wasm32"))]
fn take_beside(script: &std::path::Path, began: chrono::NaiveDateTime) -> PathBuf {
    let stem = script.file_stem().map_or_else(|| "presentation".into(), |s| s.to_string_lossy());
    let name = format!("{stem}-{}.txt", began.format("%Y-%m-%d-%H%M%S"));
    script.parent().unwrap_or(std::path::Path::new("")).join("takes").join(name)
}

/// Seconds as m:ss.
fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// The presenter's keys while the notes window has them, as on the canvas.
fn notes_keys(ui: &egui::Ui) -> Vec<NotesAction> {
    use egui::{Key, Modifiers};
    ui.input_mut(|i| {
        let mut asked = Vec::new();
        let shift = i.modifiers.matches_exact(Modifiers::SHIFT);
        let bare = i.modifiers.matches_exact(Modifiers::NONE);
        let mut pressed = |modifiers, key| i.consume_key(modifiers, key);
        if shift && (pressed(Modifiers::SHIFT, Key::PageDown) | pressed(Modifiers::SHIFT, Key::ArrowRight)) {
            asked.push(NotesAction::Command(Command::Finish));
        }
        if bare && (pressed(Modifiers::NONE, Key::PageDown) | pressed(Modifiers::NONE, Key::ArrowRight)) {
            asked.push(NotesAction::Command(Command::Next));
        }
        if bare && (pressed(Modifiers::NONE, Key::PageUp) | pressed(Modifiers::NONE, Key::ArrowLeft)) {
            asked.push(NotesAction::Command(Command::Back));
        }
        if bare && pressed(Modifiers::NONE, Key::Escape) {
            asked.push(NotesAction::Stop);
        }
        asked
    })
}

/// The notes: which beat, what to say over it, what comes next, and the
/// buttons for those without a clicker.
fn draw_notes(ui: &mut egui::Ui, presenting: &Presenting, now: f64) -> Vec<NotesAction> {
    let run = &presenting.run;
    let beats = &run.script.beats;
    let on_show = run.beat();
    // Before the first press, the first beat is what's coming
    let shown = on_show.unwrap_or(0);
    let beat = &beats[shown];
    let mut asked = Vec::new();
    let wide = ui.available_width();
    let say_size = if WEB { 17.0 } else { SAY_SIZE };

    ui.horizontal(|ui| {
        ui.label(RichText::new(&presenting.title).color(theme::text::QUIET).small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = match on_show {
                None => ("READY", theme::text::SECONDARY),
                Some(_) if run.playing() => ("● LIVE", TALLY),
                Some(_) => ("✓ DONE", theme::accent::SUCCESS),
            };
            ui.label(RichText::new(text).color(color).small().strong());
        });
    });
    ui.add_space(6.0);
    let heading = match on_show {
        Some(k) => format!("BEAT {} OF {}", k + 1, beats.len()),
        None => format!("UP FIRST · {} BEATS", beats.len()),
    };
    ui.label(RichText::new(heading).color(theme::text::ACCENT).size(11.5).extra_letter_spacing(1.5));
    ui.label(RichText::new(&beat.name).font(title_font(if WEB { 19.0 } else { 26.0 })).color(theme::text::PRIMARY));
    beat_dots(ui, on_show, beats.len());
    ui.add_space(12.0);

    egui::ScrollArea::vertical().max_height(ui.available_height() - 120.0).auto_shrink([false, true]).show(ui, |ui| {
        let ink = if on_show.is_some() { theme::text::PRIMARY } else { theme::text::SECONDARY };
        if beat.say.is_empty() {
            ui.label(RichText::new("Nothing to say: let it play.").italics().color(theme::text::QUIET).size(say_size * 0.7));
        }
        for paragraph in &beat.say {
            ui.label(say_job(paragraph, wide, say_size, ink));
            ui.add_space(say_size * 0.45);
        }
    });

    if let Some((left, whole)) = run.auto_left(now) {
        ui.add_space(4.0);
        let (rect, _) = ui.allocate_exact_size(vec2(wide, 3.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 1.5, theme::background::WIDGET);
        let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * (left / whole) as f32, rect.height()));
        ui.painter().rect_filled(filled, 1.5, theme::text::ACCENT.gamma_multiply(0.8));
        ui.label(RichText::new(format!("On to the next beat in {:.0} s", left.ceil())).color(theme::text::QUIET).small());
    }

    ui.add_space(8.0);
    let line = match on_show {
        None => format!("{} plays it", next_keys(&run.script)),
        Some(k) if k + 1 < beats.len() => {
            let next = &beats[k + 1];
            let first = next.say.first().map(|s| format!(" · {}", s.replace("**", ""))).unwrap_or_default();
            format!("Next: {}{first}", next.name)
        }
        Some(_) => "That's the last beat. Escape stops presenting.".to_string(),
    };
    ui.add(egui::Label::new(RichText::new(line).color(theme::text::SECONDARY)).truncate());
    if presenting.keys_elsewhere {
        ui.label(RichText::new("⚠ A knob or field has the keys: click the canvas to give them back").color(theme::accent::WARNING).small());
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let can_back = on_show.is_some();
        if ui.add_enabled(can_back, egui::Button::new("⟲ Again")).on_hover_text("Page Up: play this beat again from its start, or the one before if it's only just begun").clicked() {
            asked.push(NotesAction::Command(Command::Back));
        }
        if ui.add_enabled(run.playing(), egui::Button::new("Finish ⏭")).on_hover_text("Shift + Page Down: end this beat now, as it would have").clicked() {
            asked.push(NotesAction::Command(Command::Finish));
        }
        let can_next = on_show.is_none_or(|k| k + 1 < beats.len());
        let next = egui::Button::new(RichText::new("Next ▶").color(Color32::from_rgb(16, 24, 38)).strong()).fill(theme::text::ACCENT);
        if ui.add_enabled(can_next, next).on_hover_text("Page Down or a clicker: play the next beat").clicked() {
            asked.push(NotesAction::Command(Command::Next));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Stop").on_hover_text("Escape: stop presenting, leaving the patch as it is").clicked() {
                asked.push(NotesAction::Stop);
            }
        });
    });

    // The take: how long it's run, and what isn't in it
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        match run.take_length(now) {
            Some(length) => {
                let take = ui.label(RichText::new(format!("● Take {}", clock(length))).color(TALLY.gamma_multiply(0.85)).small());
                if let Some(path) = &presenting.take_to {
                    take.on_hover_text(format!("Kept in {} as each beat begins", path.display()));
                }
                ui.label(RichText::new("· hand moves aren't in it").color(theme::text::QUIET).small());
            }
            None => {
                ui.label(RichText::new("The take starts with the first beat").color(theme::text::QUIET).small());
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save = egui::Button::new(RichText::new("Save take…").small()).small();
            let hover = "The run so far as a capture script: `soba patch.json --capture take.txt` films it again, \
                in time with what you said. Knobs turned and the view moved by hand aren't in it";
            if ui.add_enabled(on_show.is_some(), save).on_hover_text(hover).clicked() {
                asked.push(NotesAction::SaveTake);
            }
        });
    });
    asked
}

/// A dot a beat: filled for those played, ringed for the one on show.
fn beat_dots(ui: &mut egui::Ui, on_show: Option<usize>, total: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), egui::Sense::hover());
    let gap = (rect.width() / total.max(1) as f32).min(12.0);
    for i in 0..total {
        let c = egui::pos2(rect.left() + 4.0 + i as f32 * gap, rect.center().y);
        match on_show {
            Some(k) if i == k => {
                ui.painter().circle_filled(c, 3.2, theme::text::ACCENT);
                ui.painter().circle_stroke(c, 5.2, Stroke::new(1.0_f32, theme::text::ACCENT.gamma_multiply(0.5)));
            }
            Some(k) if i < k => {
                ui.painter().circle_filled(c, 2.6, theme::text::ACCENT.gamma_multiply(0.55));
            }
            _ => {
                ui.painter().circle_stroke(c, 2.4, Stroke::new(1.0_f32, theme::text::QUIET.gamma_multiply(0.6)));
            }
        }
    }
}

/// A paragraph of what's said, its **bold** words brighter.
fn say_job(text: &str, wrap: f32, size: f32, ink: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    for (run, bold) in emphasis_runs(text) {
        let (font_id, color) = if bold { (title_font(size), EMPHASIS) } else { (FontId::proportional(size), ink) };
        job.append(run, 0.0, TextFormat { font_id, color, line_height: Some(size * 1.35), ..Default::default() });
    }
    job
}

fn title_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::TITLE_FAMILY.into()))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_take_without_take_goes_beside_its_script() {
        let began = chrono::NaiveDate::from_ymd_opt(2026, 10, 28).unwrap().and_hms_opt(9, 5, 3).unwrap();
        let path = take_beside(Path::new("presentations/ep1-how-a-synth-works.txt"), began);
        assert_eq!(path, Path::new("presentations/takes/ep1-how-a-synth-works-2026-10-28-090503.txt"));
        // A script named on its own is in the folder Soba started in
        assert_eq!(take_beside(Path::new("ep4.txt"), began), Path::new("takes/ep4-2026-10-28-090503.txt"));
    }
}
