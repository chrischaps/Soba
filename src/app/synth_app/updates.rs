//! The app's side of updating: the Help menu's items, the dot by the
//! version, the note, and restarting into a new version with the session
//! handed over (or picking one up, after `--resume`).

use eframe::egui::{self, RichText};

use super::super::session::Discard;
use super::super::theme;
use super::super::update::{self, release, Resume, UpdateAction};
use super::actions::AppAction;
use super::SynthApp;

impl SynthApp {
    /// The update items in the Help menu.
    pub(super) fn update_menu(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        ui.separator();
        if ui.button("Check for Updates…").clicked() {
            actions.push(AppAction::CheckUpdates);
            ui.close();
        }
        ui.checkbox(&mut self.updater.auto_check, "Check for Updates Automatically")
            .on_hover_text("Once a day, Soba asks GitHub whether there's a newer release: one request, with nothing about you or this computer in it");
        if let Some(version) = self.updater.rollback_version() {
            let label = format!("Roll Back to {version}");
            let button = ui.add_enabled(!self.is_recording(), egui::Button::new(label));
            if button
                .on_hover_text(format!("Go back to the version you had before updating, {version}, and restart"))
                .on_disabled_hover_text("Once the recording ends: rolling back restarts Soba")
                .clicked()
            {
                actions.push(AppAction::RollBack);
                ui.close();
            }
        }
    }

    /// The dot after the version in the status bar: lit when a newer
    /// version is out. Clicking either says more, or checks.
    pub(super) fn version_label(&mut self, ui: &mut egui::Ui) {
        let pending = self.updater.pending().cloned();
        let dot = ui.add(egui::Label::new(RichText::new("●").small().color(match pending {
            Some(_) => theme::accent::PRIMARY,
            None => theme::background::WIDGET_ACTIVE,
        })).sense(egui::Sense::click()));
        let version = RichText::new(concat!("Soba v", env!("CARGO_PKG_VERSION")))
            .color(if pending.is_some() { theme::text::SECONDARY } else { theme::text::DISABLED })
            .small();
        let label = ui.add(egui::Label::new(version).sense(egui::Sense::click()));
        if let Some(version) = &pending {
            // A soft halo, so it's seen without shouting. Still, not
            // breathing: a waiting update shouldn't keep the UI redrawing
            ui.painter().circle_filled(dot.rect.center(), 6.0, theme::accent::PRIMARY.gamma_multiply(0.18));
            let hint = format!("Soba {version} is out: click for what's new");
            if dot.on_hover_text(&hint).clicked() | label.on_hover_text(&hint).clicked() {
                self.updater.show_note();
            }
        } else {
            let hint = "Check for updates";
            if dot.on_hover_text(hint).clicked() | label.on_hover_text(hint).clicked() {
                self.updater.check_now(ui.ctx());
            }
        }
    }

    /// Hears from the update check and draws its note, `above` the height
    /// of a note already in the corner.
    pub(super) fn run_updates(&mut self, ctx: &egui::Context, above: f32) {
        // A film's frames are the script's, and an embed is someone's page.
        // A test server (SOBA_UPDATE_URL) lets a capture try an update
        let allowed = (self.capture.is_none() || update::test_source()) && !self.embedded;
        self.updater.tick(ctx, allowed);
        if !allowed {
            return;
        }
        match self.updater.show(ctx, self.is_recording(), above) {
            Some(UpdateAction::Restart) => self.request(ctx, Discard::Update),
            Some(UpdateAction::OpenPage(url)) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            None => {}
        }
    }

    /// Puts the downloaded version in place and restarts into it. If it
    /// can't be put in place, the note says why and nothing changes.
    pub(super) fn restart_for_update(&mut self, ctx: &egui::Context) {
        if self.updater.swap() {
            self.restart(ctx, "The new version is in place, and starts next time Soba opens");
        }
    }

    /// Swaps back to the version from before the last update, and restarts.
    pub(super) fn roll_back(&mut self, ctx: &egui::Context) {
        match self.updater.roll_back() {
            Ok(version) => self.restart(ctx, &format!("Soba {version} is back in place, and starts next time Soba opens")),
            Err(e) => self.raise_notice(format!("Couldn't roll back: {e}")),
        }
    }

    /// Starts the copy now in place with this session, and closes.
    fn restart(&mut self, ctx: &egui::Context, if_it_fails: &str) {
        let resume = match self.handover() {
            Ok(resume) => resume,
            Err(e) => {
                self.raise_notice(format!("{if_it_fails}: couldn't keep the session ({e})"));
                return;
            }
        };
        match self.updater.relaunch(&resume) {
            Ok(()) => {
                // The new copy waits for this one to close before it opens
                // the audio device (ASIO serves one program at a time)
                if let Ok(engine) = self.audio.engine.as_mut() {
                    let _ = engine.stop();
                }
                // The autosave is kept too, in case the new copy doesn't start
                self.resuming = true;
                self.document.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.raise_notice(format!("{if_it_fails}: {e}")),
        }
    }

    /// What the next copy needs to carry on: the patch, devices, view.
    fn handover(&mut self) -> Result<Resume, String> {
        self.sync_history();
        let patch = self.create_patch(&self.document.title());
        let autosave = self.document.autosave(&patch)?;
        // The view at the top of the patch; inside a group, it opens at the top
        let view = (self.level_trail.is_empty() && self.editor_rect.is_positive()).then(|| {
            let center = self.screen_to_patch(self.editor_rect.center());
            (self.graph_state.pan_zoom.zoom, [center.x, center.y])
        });
        let (output_device, input_device) = match &self.audio.engine {
            Ok(engine) => (self.audio.output().map(|d| d.name.clone()), engine.input_name().map(str::to_string)),
            Err(_) => (None, None),
        };
        let midi_device = self.midi.selected_device().map(|d| d.name.clone());
        Ok(Resume {
            patch: autosave,
            unsaved: self.has_unsaved_changes(),
            view,
            output_device,
            input_device,
            midi_device,
            playing: self.is_playing,
            from_version: release::CURRENT.to_string(),
        })
    }

    /// Picks up where the copy before left off: after an update, a roll
    /// back, or anything else started with `--resume`.
    pub fn resume(&mut self, resume: Resume) {
        // The handover has the patch; the autosave was only a fallback
        self.document.recovery = None;
        self.recover(&resume.patch);
        // The takes the last copy kept in temp are this one's now
        self.take_over_left_takes();
        if !resume.unsaved {
            self.mark_saved();
        }

        if let Some(index) = resume.output_device.and_then(|name| self.audio.outputs.iter().find(|d| d.name == name)).map(|d| d.index) {
            if index != self.audio.selected_output {
                self.select_device(index);
            }
        }
        if let Some(name) = resume.input_device {
            self.refresh_input_devices();
            if let Some(index) = self.audio.inputs.iter().find(|d| d.name == name).map(|d| d.index) {
                self.select_input(Some(index));
            }
        }
        if let Some(index) = resume.midi_device.and_then(|name| self.midi.devices.iter().find(|d| d.name == name)).map(|d| d.index) {
            self.connect_midi_device(index);
        }
        self.pending_view = resume.view.map(|(zoom, [x, y])| (zoom, egui::pos2(x, y)));
        if resume.playing {
            self.set_playing(true);
        }

        let from = release::parse_version(&resume.from_version);
        self.status_message = Some(match from {
            Some(from) if from < release::current() => format!("Updated to Soba {} from {from}", release::CURRENT),
            Some(from) if from > release::current() => format!("Rolled back to Soba {} from {from}", release::CURRENT),
            _ => format!("Picked up where Soba {} left off", resume.from_version),
        });
    }

    /// Puts the view where the last copy had it, once the editor has a size.
    pub(super) fn apply_pending_view(&mut self, ui: &egui::Ui, editor_rect: egui::Rect) {
        if !editor_rect.is_positive() {
            return;
        }
        let Some((zoom, center)) = self.pending_view.take() else { return };
        let (zoom_before, pan_before) = (self.graph_state.pan_zoom.zoom, self.graph_state.pan_zoom.pan);
        if zoom_before > 0.0 && (zoom - zoom_before).abs() > 1e-4 {
            self.graph_state.zoom(ui, zoom / zoom_before);
            self.history.follow_zoom(zoom_before, pan_before, &self.graph_state.pan_zoom);
        }
        let node = self.history.from_patch(center, self.graph_state.pan_zoom.zoom);
        self.graph_state.pan_zoom.pan = editor_rect.center() - editor_rect.min - node.to_vec2();
    }
}
