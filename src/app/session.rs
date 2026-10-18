//! Keeping work safe across a session: asking before unsaved changes are
//! thrown away, the recent files list, and the autosave that brings a patch
//! back after a crash.
//!
//! Recent files and the autosave live in eframe's app storage, which eframe
//! writes every [`AUTOSAVE_INTERVAL`] and on exit. The autosave only holds a
//! patch while it has unsaved changes, so after a clean exit it's empty, and
//! a patch found there on launch was cut off by a crash.

use std::path::{Path, PathBuf};
use std::time::Duration;
use web_time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align, Layout, RichText};
use serde::{Deserialize, Serialize};

use crate::modules::looper;
use crate::persistence::{patch_from_json, Example, Patch, PatchError};
use super::lesson::LessonFile;
use super::theme;

/// How often eframe stores recent files and the autosave.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

const RECENT_KEY: &str = "recent_files";
const AUTOSAVE_KEY: &str = "autosave";

/// Most files the Recent menu lists.
const MAX_RECENT: usize = 8;

/// Something that would replace or close the patch, so asks first when the
/// patch has unsaved changes.
#[derive(Clone)]
pub enum Discard {
    New,
    /// Pick a file to open.
    Open,
    OpenFile(PathBuf),
    OpenExample(&'static Example),
    /// Start a lesson, which opens its own patch.
    OpenLesson(&'static LessonFile),
    /// Open the patch a link holds: its fragment, `p=1.…`.
    OpenLink(String),
    Quit,
    /// Restart into a downloaded update.
    #[cfg(not(target_arch = "wasm32"))]
    Update,
    /// Restart into the version from before the last update.
    #[cfg(not(target_arch = "wasm32"))]
    RollBack,
}

impl Discard {
    /// What the prompt's discard button says.
    fn verb(&self) -> &'static str {
        match self {
            Self::Quit => "Quit Without Saving",
            _ if self.restarts() => "Restart Without Saving",
            _ => "Don't Save",
        }
    }

    /// Whether it restarts the app, which brings unsaved changes back.
    pub fn restarts(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(self, Self::Update | Self::RollBack) {
            return true;
        }
        false
    }

    /// What happens to unsaved changes.
    fn consequence(&self) -> &'static str {
        if self.restarts() {
            "Your changes come back after the restart either way. Saving puts them in the file too, in case the new version won't open."
        } else {
            "Your changes will be lost if you don't save them."
        }
    }

    /// What the prompt's button says when there's nothing to save, only a
    /// recording to stop.
    fn go_ahead(&self) -> &'static str {
        match self {
            Self::Quit => "Stop and Quit",
            _ => "Stop Recording",
        }
    }
}

/// What the unsaved-changes prompt was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

/// Patch files opened or saved lately, newest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecentFiles(Vec<PathBuf>);

impl RecentFiles {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage.and_then(|s| eframe::get_value(s, RECENT_KEY)).unwrap_or_default()
    }

    pub fn store(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, RECENT_KEY, self);
    }

    /// Puts a file at the top, moving it there if it's already listed.
    pub fn push(&mut self, path: &Path) {
        self.0.retain(|p| p != path);
        self.0.insert(0, path.to_path_buf());
        self.0.truncate(MAX_RECENT);
    }

    pub fn remove(&mut self, path: &Path) {
        self.0.retain(|p| p != path);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(PathBuf::as_path)
    }
}

/// A patch with unsaved changes, as it was at the last autosave.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Autosave {
    /// When it was taken, in seconds since the Unix epoch.
    pub saved_at: u64,
    /// The file the patch was opened from or last saved to, if any.
    pub path: Option<PathBuf>,
    /// The patch's name, for the recovery prompt.
    pub name: String,
    /// The example the patch was opened from, by name, if it's one.
    #[serde(default)]
    pub example: Option<String>,
    /// The patch, as patch-file JSON.
    patch: String,
    /// What loading `path` left out, which saving over it would lose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
}

impl Autosave {
    pub fn new(patch: &Patch, path: Option<PathBuf>, example: Option<String>) -> Result<Self, PatchError> {
        Ok(Self {
            saved_at: now(),
            path,
            name: patch.name.clone(),
            example,
            patch: serde_json::to_string(patch)?,
            dropped: Vec::new(),
        })
    }

    /// The autosave of a patch whose file has `dropped`, which loading it left out.
    pub fn with_dropped(self, dropped: Vec<String>) -> Self {
        Self { dropped, ..self }
    }

    pub fn patch(&self) -> Result<Patch, PatchError> {
        patch_from_json(&self.patch)
    }

    /// Whether the patch has a Looper, whose loop an autosave doesn't keep.
    pub fn has_looper(&self) -> bool {
        self.patch.contains(&format!("\"{}\"", looper::ID))
    }

    /// The autosave a crash left behind, if there is one.
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Option<Self> {
        // A clean exit stores `None`
        eframe::get_value::<Option<Self>>(storage?, AUTOSAVE_KEY).flatten()
    }

    /// Stores `autosave`, or clears it with `None`.
    pub fn store(storage: &mut dyn eframe::Storage, autosave: Option<&Self>) {
        eframe::set_value(storage, AUTOSAVE_KEY, &autosave);
    }

    /// How long ago it was taken, e.g. "3 minutes ago".
    pub fn age(&self) -> String {
        ago(now().saturating_sub(self.saved_at))
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn ago(seconds: u64) -> String {
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match seconds {
        0..=59 => "moments ago".to_string(),
        60..=3599 => plural(seconds / 60, "minute"),
        3600..=86_399 => plural(seconds / 3600, "hour"),
        _ => plural(seconds / 86_400, "day"),
    }
}

/// Asks whether to save `patch_name`'s changes before `action`. Returns the
/// answer on the frame it's given; Escape or a click outside is Cancel.
///
/// `recording` is the length of a take in progress that `action` will stop
/// (quitting does), so the prompt says so. With a take running and nothing
/// unsaved, it only asks whether to stop the take and go ahead: Discard.
pub fn unsaved_changes_prompt(
    ctx: &egui::Context,
    patch_name: &str,
    action: &Discard,
    unsaved: bool,
    recording: Option<&str>,
) -> Option<Answer> {
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let response = prompt(ctx, "unsaved_changes", |ui| {
        let title = if unsaved && action.restarts() {
            format!("Save changes to “{patch_name}” before restarting?")
        } else if unsaved {
            format!("Save changes to “{patch_name}”?")
        } else {
            "Stop recording?".to_string()
        };
        ui.label(RichText::new(title).size(17.0).color(theme::text::PRIMARY));
        ui.add_space(4.0);
        if unsaved {
            ui.label(RichText::new(action.consequence()).color(theme::text::SECONDARY));
        }
        if let Some(length) = recording {
            ui.label(
                RichText::new(format!("● A recording is running ({length}). It will be stopped and saved."))
                    .color(theme::accent::ERROR),
            );
        }
        ui.add_space(14.0);

        let mut answer = None;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if unsaved {
                if ui.add(primary_button("Save")).clicked() || enter {
                    answer = Some(Answer::Save);
                }
            } else if ui.add(primary_button(action.go_ahead())).clicked() || enter {
                answer = Some(Answer::Discard);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(Answer::Cancel);
            }
            if unsaved {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if ui.button(RichText::new(action.verb()).color(theme::accent::ERROR)).clicked() {
                        answer = Some(Answer::Discard);
                    }
                });
            }
        });
        answer
    });
    response.inner.or(response.should_close().then_some(Answer::Cancel))
}

/// Offers to bring back the patch a crash cut off. `Some(true)` restores it,
/// `Some(false)` lets it go.
pub fn recovery_prompt(ctx: &egui::Context, autosave: &Autosave) -> Option<bool> {
    let response = prompt(ctx, "recover_autosave", |ui| {
        ui.label(RichText::new("Recover unsaved changes?").size(17.0).color(theme::text::PRIMARY));
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Soba closed before “{}” was saved. The autosave from {} still has your changes.",
                autosave.name,
                autosave.age()
            ))
            .color(theme::text::SECONDARY),
        );
        if autosave.has_looper() {
            ui.add_space(6.0);
            ui.label(
                RichText::new("An autosave doesn’t keep loops: Loopers come back with the loops last saved with the patch, if any.")
                    .color(theme::text::SECONDARY),
            );
        }
        ui.add_space(14.0);

        let mut answer = None;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(primary_button("Recover")).clicked() {
                answer = Some(true);
            }
            if ui.button("Discard").clicked() {
                answer = Some(false);
            }
        });
        answer
    });
    // Closing it without choosing keeps the autosave for next time
    response.inner
}

/// A modal card in the panel colours, a fixed width so the text wraps.
fn prompt<R>(ctx: &egui::Context, id: &str, content: impl FnOnce(&mut egui::Ui) -> R) -> egui::ModalResponse<R> {
    egui::Modal::new(egui::Id::unique(id))
        .backdrop_color(egui::Color32::from_black_alpha(140))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .fill(theme::background::PANEL)
                .corner_radius(theme::ROUNDING)
                .inner_margin(egui::Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.set_width(380.0);
            content(ui)
        })
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(theme::text::PRIMARY).strong())
        .fill(theme::accent::PRIMARY.gamma_multiply(0.55))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_move_to_the_top_without_repeating() {
        let mut recent = RecentFiles::default();
        for name in ["a.json", "b.json", "c.json"] {
            recent.push(Path::new(name));
        }
        recent.push(Path::new("a.json"));
        let names: Vec<_> = recent.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(names, ["a.json", "c.json", "b.json"]);

        for i in 0..20 {
            recent.push(&PathBuf::from(format!("{i}.json")));
        }
        assert_eq!(recent.iter().count(), MAX_RECENT);
        assert_eq!(recent.iter().next(), Some(Path::new("19.json")));
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(5), "moments ago");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(7 * 60 + 30), "7 minutes ago");
        assert_eq!(ago(2 * 3600), "2 hours ago");
        assert_eq!(ago(86_400), "1 day ago");
    }

    #[test]
    fn autosave_round_trips_through_storage() {
        #[derive(Default)]
        struct Memory(std::collections::HashMap<String, String>);
        impl eframe::Storage for Memory {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_string(), value);
            }
            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }
            fn flush(&mut self) {}
        }

        let mut storage = Memory::default();
        assert!(Autosave::load(Some(&storage)).is_none());

        let patch = crate::persistence::examples::first_sound().patch().unwrap();
        let autosave = Autosave::new(&patch, Some(PathBuf::from("song.json")), None).unwrap();
        Autosave::store(&mut storage, Some(&autosave));
        let loaded = Autosave::load(Some(&storage)).expect("an autosave");
        assert_eq!(loaded.path, Some(PathBuf::from("song.json")));
        assert_eq!(loaded.patch().unwrap().nodes.len(), patch.nodes.len());

        Autosave::store(&mut storage, None);
        assert!(Autosave::load(Some(&storage)).is_none());

        let mut recent = RecentFiles::default();
        recent.push(Path::new("song.json"));
        recent.store(&mut storage);
        assert_eq!(RecentFiles::load(Some(&storage)), recent);
    }
}
