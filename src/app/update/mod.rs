//! Knowing when a newer Soba is out, and installing it.
//!
//! Once a day at most, and never in the first seconds after launch, a
//! worker thread asks GitHub for the list of releases. If there's a newer
//! one, a dot by the version in the status bar lights up, and the first
//! time each version is seen a note says so, with its notes and an Install
//! button. Installing downloads the release's zip, checks it against the
//! release's `SHA256SUMS`, swaps the executable and restarts, handing the
//! patch, devices and view to the new copy (see [`resume`]).
//!
//! A failed background check is quiet: a line on stderr, nothing on screen.
//! Turning off Help → Check for Updates Automatically stops every request.

mod install;
mod net;
mod notes;
pub mod release;
pub mod resume;
mod signature;

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, RichText};
use semver::Version;
use web_time::{Instant, SystemTime, UNIX_EPOCH};

use super::notice::{self, Notice};
use super::theme;
use install::{Mode, Staged};
use net::Progress;
use release::Release;
pub use resume::Resume;

const AUTO_KEY: &str = "update_auto_check";
const LAST_CHECK_KEY: &str = "update_last_check";
const SKIPPED_KEY: &str = "update_skipped";
const ANNOUNCED_KEY: &str = "update_announced";
const KNOWN_KEY: &str = "update_known_releases";

/// The least time between automatic checks.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after launch the first automatic check waits, so it's never
/// part of starting up.
const QUIET_START: Duration = Duration::from_secs(10);

/// How long a note about a new version stays up on its own.
const OFFER_SECONDS: f64 = 30.0;

/// How long "You're up to date" stays up.
const UP_TO_DATE_SECONDS: f64 = 6.0;

/// What a worker reports.
enum Message {
    Checked { manual: bool, result: Result<Vec<Release>, String> },
    Staged(Result<Staged, String>),
}

/// Where the updater is.
enum State {
    Idle,
    Checking,
    /// A check asked for found nothing newer.
    UpToDate,
    /// A check asked for didn't work.
    CheckFailed(String),
    /// There's a newer version (in `newer`).
    Available,
    Downloading(Arc<Progress>),
    /// Downloaded, checked and unpacked: a restart puts it in place.
    Ready(Staged),
    /// The install didn't work; nothing was changed.
    Failed(String),
}

/// What the app is asked to do.
pub enum UpdateAction {
    /// Restart into the staged version (asking about unsaved changes first).
    Restart,
    /// Open a release's page in the browser.
    OpenPage(String),
}

/// The update check, its note and the What's New window.
pub struct Updater {
    /// Check once a day without being asked.
    pub auto_check: bool,
    skipped: Option<Version>,
    /// The newest version a note has been shown for.
    announced: Option<Version>,
    /// The last successful check, in seconds since the Unix epoch.
    last_check: u64,
    /// The last automatic check this launch, whatever came of it: a failed
    /// one is tried again next launch, or a day later, never straight away.
    last_attempt: Option<Instant>,
    launched: Instant,
    /// Releases newer than this copy, newest first.
    newer: Vec<Release>,
    state: State,
    inbox: (Sender<Message>, Receiver<Message>),
    /// When the note went up, in egui's clock (NaN: this frame).
    shown_at: Option<f64>,
    notes_open: bool,
    notes: Vec<(Version, Vec<notes::Block>)>,
    mode: Option<Mode>,
    rollback: Option<Version>,
    /// Staged and waiting for the go-ahead to restart.
    restart_pending: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

impl Updater {
    pub fn new() -> Self {
        // Before any swap: afterwards the OS may name this file `.old`
        let _ = install::exe_path();
        Self {
            auto_check: true,
            skipped: None,
            announced: None,
            last_check: 0,
            last_attempt: None,
            launched: Instant::now(),
            newer: Vec::new(),
            state: State::Idle,
            inbox: mpsc::channel(),
            shown_at: None,
            notes_open: false,
            notes: Vec::new(),
            mode: None,
            // Only an older version is one to roll back to
            rollback: install::rollback_version().filter(|v| *v < release::current()),
            restart_pending: false,
        }
    }

    /// Picks up the settings and what the last check found, and tidies up
    /// after an interrupted update.
    pub fn load(&mut self, storage: Option<&dyn eframe::Storage>) {
        install::clean_up();
        let Some(storage) = storage else { return };
        let get = |key: &str| storage.get_string(key).filter(|v| !v.is_empty());
        self.auto_check = get(AUTO_KEY).is_none_or(|v| v != "false");
        self.last_check = get(LAST_CHECK_KEY).and_then(|v| v.parse().ok()).unwrap_or(0);
        self.skipped = get(SKIPPED_KEY).and_then(|v| release::parse_version(&v));
        self.announced = get(ANNOUNCED_KEY).and_then(|v| release::parse_version(&v));
        let known: Vec<Release> = get(KNOWN_KEY).and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default();
        self.newer = release::newer_than(&known, &release::current());
        if !self.newer.is_empty() {
            self.state = State::Available;
        }
    }

    pub fn store(&self, storage: &mut dyn eframe::Storage) {
        let version = |v: &Option<Version>| v.as_ref().map(Version::to_string).unwrap_or_default();
        storage.set_string(AUTO_KEY, self.auto_check.to_string());
        storage.set_string(LAST_CHECK_KEY, self.last_check.to_string());
        storage.set_string(SKIPPED_KEY, version(&self.skipped));
        storage.set_string(ANNOUNCED_KEY, version(&self.announced));
        storage.set_string(KNOWN_KEY, serde_json::to_string(&self.newer).unwrap_or_default());
    }

    /// Hears from the workers, and starts the daily check when it's due.
    /// `allowed` is false while filming, when nothing should change.
    pub fn tick(&mut self, ctx: &egui::Context, allowed: bool) {
        while let Ok(message) = self.inbox.1.try_recv() {
            self.receive(message);
        }
        if !allowed || !self.auto_check || !matches!(self.state, State::Idle | State::Available) {
            return;
        }
        let since_launch = self.launched.elapsed();
        if since_launch < QUIET_START {
            ctx.request_repaint_after(QUIET_START - since_launch);
            return;
        }
        let due = now().saturating_sub(self.last_check) >= CHECK_EVERY.as_secs();
        let tried = self.last_attempt.is_some_and(|at| at.elapsed() < CHECK_EVERY);
        if due && !tried {
            self.last_attempt = Some(Instant::now());
            self.start_check(ctx, false);
        }
    }

    /// Help → Check for Updates: checks now, and says what it found.
    pub fn check_now(&mut self, ctx: &egui::Context) {
        if !matches!(self.state, State::Checking | State::Downloading(_) | State::Ready(_)) {
            self.start_check(ctx, true);
        }
        self.show_note();
    }

    fn start_check(&mut self, ctx: &egui::Context, manual: bool) {
        // Only a check that was asked for shows itself
        if manual {
            self.state = State::Checking;
        }
        let sender = self.inbox.0.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("update check".into())
            .spawn(move || {
                let result = net::fetch_releases();
                let _ = sender.send(Message::Checked { manual, result });
                ctx.request_repaint();
            })
            .map(drop)
            .unwrap_or_else(|e| eprintln!("update: couldn't start the check: {e}"));
    }

    fn receive(&mut self, message: Message) {
        match message {
            Message::Checked { manual, result } => match result {
                Ok(releases) => {
                    self.last_check = now();
                    self.newer = release::newer_than(&releases, &release::current());
                    self.notes.clear();
                    // An install under way carries on with what it has
                    if self.installing() {
                        return;
                    }
                    match self.newer.first().map(|r| r.version.clone()) {
                        None => {
                            self.state = if manual { State::UpToDate } else { State::Idle };
                            if manual {
                                self.show_note();
                            }
                        }
                        Some(latest) => {
                            self.state = State::Available;
                            let fresh = self.skipped.as_ref() != Some(&latest) && self.announced.as_ref() != Some(&latest);
                            if manual || fresh {
                                self.announced = Some(latest);
                                self.show_note();
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("update: the check didn't work: {e}");
                    if self.installing() {
                        return;
                    }
                    // A known update is still offered
                    let known = !self.newer.is_empty();
                    self.state = match (manual, known) {
                        (true, _) => State::CheckFailed(e),
                        (false, true) => State::Available,
                        (false, false) => State::Idle,
                    };
                }
            },
            Message::Staged(Ok(staged)) => {
                self.state = State::Ready(staged);
                self.restart_pending = true;
                self.show_note();
            }
            Message::Staged(Err(e)) if e == net::CANCELLED => self.state = State::Available,
            Message::Staged(Err(e)) => {
                eprintln!("update: the install didn't work: {e}");
                self.state = State::Failed(e);
                self.show_note();
            }
        }
    }

    /// Downloading an update, or holding one ready to restart into.
    fn installing(&self) -> bool {
        matches!(self.state, State::Downloading(_) | State::Ready(_))
    }

    /// The newest release, if it's newer than this copy.
    fn latest(&self) -> Option<&Release> {
        self.newer.first()
    }

    /// The version the status bar's dot is for: newer, and not skipped.
    pub fn pending(&self) -> Option<&Version> {
        self.latest().map(|r| &r.version).filter(|v| self.skipped.as_ref() != Some(*v))
    }

    /// Puts the note up (clicking the version in the status bar).
    pub fn show_note(&mut self) {
        self.shown_at = Some(f64::NAN);
    }

    fn hide_note(&mut self) {
        self.shown_at = None;
    }

    /// Whether the version kept from before the last update can be gone
    /// back to, and which it is.
    pub fn rollback_version(&self) -> Option<&Version> {
        self.rollback.as_ref()
    }

    /// How this copy updates, worked out the first time it's needed.
    fn mode(&mut self) -> &Mode {
        self.mode.get_or_insert_with(install::mode)
    }

    fn start_install(&mut self, ctx: &egui::Context) {
        let Some(latest) = self.latest().cloned() else { return };
        let progress = Arc::new(Progress::default());
        self.state = State::Downloading(progress.clone());
        let sender = self.inbox.0.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("update download".into())
            .spawn(move || {
                let _ = sender.send(Message::Staged(install::stage(&latest, &progress)));
                ctx.request_repaint();
            })
            .map(drop)
            .unwrap_or_else(|e| eprintln!("update: couldn't start the download: {e}"));
    }

    /// Takes the staged update to put in place, just before a restart.
    pub fn take_staged(&mut self) -> Option<Staged> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Ready(staged) => Some(staged),
            other => {
                self.state = other;
                None
            }
        }
    }

    /// Puts the staged update in place. On failure the running copy is
    /// untouched and the note says why.
    pub fn swap(&mut self) -> bool {
        let Some(staged) = self.take_staged() else { return false };
        match install::swap(&staged) {
            Ok(()) => true,
            Err(e) => {
                self.state = State::Failed(e);
                self.show_note();
                false
            }
        }
    }

    /// Swaps back to the version kept from before the last update, and
    /// skips the version being left, so it isn't offered straight back.
    pub fn roll_back(&mut self) -> Result<Version, String> {
        let version = install::roll_back()?;
        self.skipped = Some(release::current());
        self.announced = Some(release::current());
        Ok(version)
    }

    /// Starts the copy now in place, handing it `resume`.
    pub fn relaunch(&self, resume: &Resume) -> Result<(), String> {
        let file = resume.hand_over()?;
        install::relaunch(&[resume::OPTION.into(), file.into_os_string()])
    }

    /// Draws the note and the What's New window. `recording` holds off
    /// installing (an update restarts the app); `above` is the height of a
    /// note already in the corner, to stack over.
    pub fn show(&mut self, ctx: &egui::Context, recording: bool, above: f32) -> Option<UpdateAction> {
        let mut action = None;
        if self.restart_pending && !recording {
            self.restart_pending = false;
            action = Some(UpdateAction::Restart);
        }
        if let Some(shown_at) = self.shown_at.as_mut() {
            if shown_at.is_nan() {
                *shown_at = ctx.input(|i| i.time);
            }
        }
        if let Some(shown_at) = self.shown_at {
            action = self.show_note_card(ctx, shown_at, recording, above).or(action);
        }
        if self.notes_open {
            action = self.show_whats_new(ctx, recording).or(action);
        }
        action
    }

    fn show_note_card(&mut self, ctx: &egui::Context, shown_at: f64, recording: bool, above: f32) -> Option<UpdateAction> {
        let latest = self.latest().cloned();
        let latest_name = latest.as_ref().map(|r| r.version.to_string()).unwrap_or_default();
        let (dot, title, lifetime) = match &self.state {
            State::Idle => {
                self.hide_note();
                return None;
            }
            State::Checking => (theme::text::DISABLED, "Checking for updates…".to_string(), None),
            State::UpToDate => (theme::accent::SUCCESS, "You're up to date".to_string(), Some(UP_TO_DATE_SECONDS)),
            State::CheckFailed(_) => (theme::accent::WARNING, "Couldn't check for updates".to_string(), None),
            State::Available => (theme::accent::PRIMARY, format!("Soba {latest_name} is out"), Some(OFFER_SECONDS)),
            State::Downloading(_) => (theme::accent::PRIMARY, format!("Downloading Soba {latest_name}"), None),
            State::Ready(staged) => (theme::accent::SUCCESS, format!("Soba {} is ready", staged.version), None),
            State::Failed(_) => (theme::accent::ERROR, format!("Couldn't update to {latest_name}"), None),
        };
        let mode = self.mode().clone();

        let mut notice = Notice::new("update_notice", dot, title).above(above);
        if let Some(seconds) = lifetime {
            notice = notice.lifetime(shown_at, seconds);
        }
        let secondary = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).color(theme::text::SECONDARY));
        };
        let small = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).small().color(theme::text::QUIET));
        };
        let response = notice.show(
            ctx,
            |ui| {
                if matches!(self.state, State::Checking) {
                    ui.spinner();
                }
            },
            |ui| {
                let mut click = None;
                match &self.state {
                    State::Idle | State::Checking => {}
                    State::UpToDate => secondary(ui, &format!("Soba {} is the newest version.", release::CURRENT)),
                    State::CheckFailed(e) => secondary(ui, &sentence(e)),
                    State::Available => {
                        if let Some(latest) = &latest {
                            if !latest.subtitle().is_empty() {
                                secondary(ui, &capitalised(latest.subtitle()));
                            }
                        }
                        small(ui, &format!("You have {}.", release::CURRENT));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("What's New").clicked() {
                                click = Some(Click::WhatsNew);
                            }
                            click = install_button(ui, &mode, recording).or(click);
                            if ui.button("Later").clicked() {
                                click = Some(Click::Later);
                            }
                        });
                        ui.add_space(2.0);
                        let skip = ui.add(egui::Label::new(RichText::new("Skip this version").small().color(theme::text::QUIET)).sense(egui::Sense::click()));
                        if skip.on_hover_text("Don't offer this version again; a newer one will still be offered").clicked() {
                            click = Some(Click::Skip);
                        }
                    }
                    State::Downloading(progress) => {
                        ui.add_space(4.0);
                        notice::progress_bar(ui, progress.fraction());
                        let mb = |bytes: u64| bytes as f64 / 1_048_576.0;
                        let (done, total) = (progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
                        let amount = if total > 0 { format!("{:.1} of {:.1} MB", mb(done), mb(total)) } else { format!("{:.1} MB", mb(done)) };
                        small(ui, &amount);
                        ui.add_space(4.0);
                        if ui.button("Cancel").clicked() {
                            click = Some(Click::Cancel);
                        }
                    }
                    State::Ready(_) => {
                        secondary(ui, "Restart to finish. Your patch, devices and view come back as they are.");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let restart = ui.add_enabled(!recording, primary_button("Restart Now"));
                            if restart.on_disabled_hover_text("Once the recording ends: restarting would stop it").clicked() {
                                click = Some(Click::Restart);
                            }
                            if ui.button("Later").clicked() {
                                click = Some(Click::Later);
                            }
                        });
                    }
                    State::Failed(e) => {
                        secondary(ui, &sentence(e));
                        small(ui, "This copy wasn't changed.");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Download Instead").clicked() {
                                click = Some(Click::Download);
                            }
                            if ui.button("Try Again").clicked() {
                                click = Some(Click::Install);
                            }
                        });
                    }
                }
                click
            },
        );

        let mut action = None;
        if response.closed || response.expired {
            self.hide_note();
            if matches!(self.state, State::UpToDate | State::CheckFailed(_)) {
                self.state = if self.newer.is_empty() { State::Idle } else { State::Available };
            }
            // A cancelled restart stays ready; a failure goes back to the offer
            if matches!(self.state, State::Failed(_)) {
                self.state = State::Available;
            }
        }
        match response.inner {
            Some(Click::WhatsNew) => {
                // The window has its own Install
                self.notes_open = true;
                self.hide_note();
            }
            Some(Click::Install) => self.start_install(ctx),
            Some(Click::Download) => action = latest.map(|r| UpdateAction::OpenPage(r.page)),
            Some(Click::Later) => self.hide_note(),
            Some(Click::Skip) => {
                self.skipped = latest.map(|r| r.version);
                self.hide_note();
            }
            Some(Click::Cancel) => {
                if let State::Downloading(progress) = &self.state {
                    progress.cancel.store(true, Ordering::Relaxed);
                }
            }
            Some(Click::Restart) => action = Some(UpdateAction::Restart),
            None => {}
        }
        action
    }

    fn show_whats_new(&mut self, ctx: &egui::Context, recording: bool) -> Option<UpdateAction> {
        if self.notes.len() != self.newer.len() {
            self.notes = self.newer.iter().map(|r| (r.version.clone(), notes::parse(&r.notes))).collect();
        }
        let mode = self.mode().clone();
        let can_install = matches!(self.state, State::Available | State::Failed(_));
        let mut open = true;
        let mut clicked = None;
        let screen = ctx.content_rect();
        egui::Window::new("What's new")
            .id(egui::Id::new("update_whats_new"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            // Never wider than the app, with a margin either side
            .default_width(540.0_f32.min(screen.width() - 48.0))
            .max_width(screen.width() - 48.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .show(ctx, |ui| {
                let since = if self.newer.len() > 1 {
                    format!("{} versions since yours ({})", self.newer.len(), release::CURRENT)
                } else {
                    format!("Since yours ({})", release::CURRENT)
                };
                ui.label(RichText::new(since).small().color(theme::text::QUIET));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(screen.height() * 0.6).auto_shrink([false, true]).show(ui, |ui| {
                    for (release, (_, blocks)) in self.newer.iter().zip(&self.notes) {
                        ui.horizontal(|ui| {
                            let family = egui::FontFamily::Name(theme::TITLE_FAMILY.into());
                            ui.label(RichText::new(format!("Soba {}", release.version)).font(egui::FontId::new(20.0, family)).color(theme::text::PRIMARY));
                            ui.label(RichText::new(&release.date).small().color(theme::text::QUIET));
                        });
                        if !release.subtitle().is_empty() {
                            ui.label(RichText::new(capitalised(release.subtitle())).color(theme::accent::PRIMARY));
                        }
                        ui.add_space(6.0);
                        notes::show(ui, blocks);
                        ui.add_space(14.0);
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if can_install {
                        clicked = match &mode {
                            Mode::InPlace => {
                                let button = ui.add_enabled(!recording, primary_button("Install"));
                                button.on_disabled_hover_text("Once the recording ends").clicked().then_some(true)
                            }
                            Mode::DownloadOnly(why) => ui.add(primary_button("Download")).on_hover_text(why.as_str()).clicked().then_some(false),
                        };
                    }
                    if ui.button("Open on GitHub").clicked() {
                        clicked = clicked.or(Some(false));
                    }
                });
            });
        if !open {
            self.notes_open = false;
        }
        match clicked {
            Some(true) => {
                self.notes_open = false;
                self.start_install(ctx);
                self.show_note();
                None
            }
            Some(false) => self.latest().map(|r| UpdateAction::OpenPage(r.page.clone())),
            None => None,
        }
    }
}

/// What was clicked on the note.
#[derive(Clone, Copy)]
enum Click {
    WhatsNew,
    Install,
    Download,
    Later,
    Skip,
    Cancel,
    Restart,
}

/// Install, or Download where this copy can't replace itself.
fn install_button(ui: &mut egui::Ui, mode: &Mode, recording: bool) -> Option<Click> {
    match mode {
        Mode::InPlace => ui
            .add_enabled(!recording, primary_button("Install"))
            .on_hover_text("Download it, check it and restart into it")
            .on_disabled_hover_text("Once the recording ends: installing restarts Soba")
            .clicked()
            .then_some(Click::Install),
        Mode::DownloadOnly(why) => ui
            .add(primary_button("Download"))
            .on_hover_text(format!("{why} Opens the release page."))
            .clicked()
            .then_some(Click::Download),
    }
}

/// Whether releases come from a test server rather than GitHub.
pub fn test_source() -> bool {
    std::env::var_os(net::URL_OVERRIDE).is_some()
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(theme::text::PRIMARY).strong()).fill(theme::accent::PRIMARY.gamma_multiply(0.55))
}

/// "a low-latency download" → "A low-latency download".
fn capitalised(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut out: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    out.extend(chars);
    out
}

/// "couldn't reach GitHub" → "Couldn't reach GitHub."
fn sentence(text: &str) -> String {
    let mut out = capitalised(text);
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whats_new_fits_a_narrow_app() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let width = 480.0;
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 700.0))),
            ..Default::default()
        };
        let mut updater = Updater::new();
        updater.newer = release::parse_releases(include_str!("testdata/releases.json")).unwrap();
        updater.state = State::Available;
        // A few frames: fonts load, then the window settles on its size
        for _ in 0..4 {
            let _ = ctx.run_ui(input(), |ui| {
                let ctx = ui.ctx();
                updater.show_whats_new(ctx, false);
            });
        }
        let rect = ctx.memory(|m| m.area_rect(egui::Id::new("update_whats_new"))).expect("the window was shown");
        assert!(rect.width() <= width, "a {} px window in a {width} px app", rect.width());
        assert!(rect.left() >= 0.0 && rect.right() <= width, "{rect:?} spills off a {width} px app");
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(sentence("couldn't reach GitHub (offline?)"), "Couldn't reach GitHub (offline?).");
        assert_eq!(sentence("Done."), "Done.");
        assert_eq!(capitalised("a low-latency Windows download"), "A low-latency Windows download");
    }

    #[test]
    fn known_releases_survive_a_restart_and_skips_hold() {
        #[derive(Default)]
        struct Memory(std::collections::HashMap<String, String>);
        impl eframe::Storage for Memory {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_string(), value);
            }
            fn flush(&mut self) {}
        }

        let next = release::current().major + 1;
        let json = format!(r#"[{{"tag_name":"v{next}.0.0","name":"v{next}.0.0 — the future","body":"- **New**"}}]"#);
        let mut updater = Updater::new();
        updater.receive(Message::Checked { manual: false, result: release::parse_releases(&json).map_err(|e| e.0) });
        assert!(matches!(updater.state, State::Available));
        assert_eq!(updater.pending().map(Version::to_string), Some(format!("{next}.0.0")));
        assert!(updater.shown_at.is_some(), "a new version is announced once");

        let mut storage = Memory::default();
        updater.store(&mut storage);
        let mut again = Updater::new();
        again.load(Some(&storage));
        assert!(matches!(again.state, State::Available));
        assert_eq!(again.pending(), updater.pending());
        assert!(again.auto_check);

        // The same version found again isn't announced again
        again.receive(Message::Checked { manual: false, result: release::parse_releases(&json).map_err(|e| e.0) });
        assert!(again.shown_at.is_none());

        // Skipped, the dot goes out; asked for, it's still shown
        again.skipped = again.pending().cloned();
        assert!(again.pending().is_none());
        again.receive(Message::Checked { manual: true, result: release::parse_releases(&json).map_err(|e| e.0) });
        assert!(again.shown_at.is_some());

        // Turned off, it stays off
        again.auto_check = false;
        again.store(&mut storage);
        let mut off = Updater::new();
        off.load(Some(&storage));
        assert!(!off.auto_check);
    }

    #[test]
    fn a_check_finishing_mid_download_leaves_the_download_alone() {
        let mut updater = Updater::new();
        updater.state = State::Downloading(Arc::new(Progress::default()));
        updater.receive(Message::Checked { manual: false, result: Ok(Vec::new()) });
        assert!(matches!(updater.state, State::Downloading(_)));
        updater.receive(Message::Checked { manual: false, result: Err("offline".into()) });
        assert!(matches!(updater.state, State::Downloading(_)));
    }

    #[test]
    fn a_failed_background_check_is_quiet() {
        let mut updater = Updater::new();
        updater.receive(Message::Checked { manual: false, result: Err("couldn't reach GitHub (offline?)".into()) });
        assert!(matches!(updater.state, State::Idle));
        assert!(updater.shown_at.is_none());
        assert_eq!(updater.last_check, 0, "tried again next launch");

        // ...but not again this launch
        let ctx = egui::Context::default();
        updater.launched = Instant::now() - QUIET_START * 2;
        updater.last_attempt = Some(Instant::now());
        updater.tick(&ctx, true);
        assert!(matches!(updater.state, State::Idle));

        updater.receive(Message::Checked { manual: true, result: Err("couldn't reach GitHub (offline?)".into()) });
        assert!(matches!(updater.state, State::CheckFailed(_)));
    }

    #[test]
    fn nothing_is_asked_while_checking_is_off_or_too_soon() {
        let ctx = egui::Context::default();
        let mut updater = Updater::new();
        updater.auto_check = false;
        updater.launched = Instant::now() - QUIET_START * 2;
        updater.tick(&ctx, true);
        assert!(updater.last_attempt.is_none(), "off: no request");

        updater.auto_check = true;
        updater.launched = Instant::now();
        updater.tick(&ctx, true);
        assert!(updater.last_attempt.is_none(), "not in the first seconds");

        updater.launched = Instant::now() - QUIET_START * 2;
        updater.last_check = now() - 60;
        updater.tick(&ctx, true);
        assert!(updater.last_attempt.is_none(), "not twice a day");

        updater.last_check = 0;
        updater.tick(&ctx, false);
        assert!(updater.last_attempt.is_none(), "not while filming");

        // Due, on, allowed: one request (it fails here, quietly)
        std::env::set_var(net::URL_OVERRIDE, "http://127.0.0.1:9/releases.json");
        updater.tick(&ctx, true);
        assert!(updater.last_attempt.is_some());
        assert!(matches!(updater.state, State::Idle), "a background check doesn't show itself");
    }

}
