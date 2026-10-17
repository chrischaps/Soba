//! The transport's Record button: where takes are written, what they're
//! called, the button itself, and the note that pops up when a take is done.
//!
//! The recording itself (the tap on the audio thread and the writer thread)
//! lives in [`crate::engine::recorder`]. Each take is a WAV named after the
//! patch and the minute it started, with the patch saved beside it as JSON,
//! so every recording remembers how it was made.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{self, RichText};

use crate::engine::RecordingSummary;
use super::notice::Notice;
use super::theme;

/// Storage key for the folder takes are written to, when it isn't the default.
pub const FOLDER_KEY: &str = "recordings_folder";

/// How long the "Recorded" note stays up, unless the mouse is over it.
const TOAST_SECONDS: f64 = 14.0;

/// One breath of the recording light, in seconds.
const PULSE_PERIOD: f64 = 1.6;

/// Where takes go unless another folder is chosen: Music/Soba.
pub fn default_folder() -> PathBuf {
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Music")))
        // Not std::env::temp_dir in the browser, where it panics
        .unwrap_or_else(|| if super::WEB { PathBuf::new() } else { std::env::temp_dir() })
        .join("Soba")
}

/// The path for a new take of `patch_name` in `folder`, started at `when`:
/// `<patch name> <yyyy-mm-dd hh-mm>.wav`, numbered if that minute is taken.
pub fn take_path(folder: &Path, patch_name: &str, when: chrono::DateTime<chrono::Local>) -> PathBuf {
    let stem = format!("{} {}", file_safe(patch_name), when.format("%Y-%m-%d %H-%M"));
    let free = |stem: &str| {
        let wav = folder.join(format!("{stem}.wav"));
        (!wav.exists() && !wav.with_extension("json").exists()).then_some(wav)
    };
    free(&stem)
        .or_else(|| (2..).find_map(|n| free(&format!("{stem} ({n})"))))
        .expect("some number is free")
}

/// A patch name with the characters files can't hold swapped for dashes.
fn file_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '-' } else { c })
        .collect();
    let safe = safe.trim().trim_end_matches('.');
    if safe.is_empty() { "Untitled".to_string() } else { safe.to_string() }
}

/// `m:ss`, or `h:mm:ss` past the hour.
pub fn clock(time: Duration) -> String {
    let secs = time.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// A folder as it reads in a menu: under the home folder, as `~\...`.
pub fn short_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// What the Record button shows.
pub enum RecState {
    Idle,
    /// Recording, this long so far.
    Recording(Duration),
    /// Stopped, and the file is being finished.
    Finishing,
}

/// The Record button. Idle it's a red dot and "Rec"; while recording it
/// breathes, a pulsing light beside the length of the take so far.
pub fn rec_button(ui: &mut egui::Ui, state: &RecState) -> egui::Response {
    let red = theme::accent::ERROR;
    match state {
        RecState::Idle => ui.button(RichText::new("● Rec").color(red)),
        RecState::Finishing => ui.add_enabled(false, egui::Button::new("● Saving…")),
        RecState::Recording(elapsed) => {
            // Room on the left for the light; the digits don't jitter
            let label = RichText::new(format!("    {}", clock(*elapsed))).monospace().color(theme::text::PRIMARY);
            let response = ui.add(egui::Button::new(label).fill(red.gamma_multiply(0.28)).stroke(egui::Stroke::new(1.0_f32, red.gamma_multiply(0.7))));

            let t = ui.input(|i| i.time);
            let breath = 0.5 + 0.5 * (t * std::f64::consts::TAU / PULSE_PERIOD).cos() as f32;
            let center = response.rect.left_center() + egui::vec2(ui.spacing().button_padding.x + 7.0, 0.0);
            let painter = ui.painter();
            painter.circle_filled(center, 9.0, red.gamma_multiply(0.18 * breath));
            painter.circle_filled(center, 6.5, red.gamma_multiply(0.3 + 0.25 * breath));
            painter.circle_filled(center, 4.5, red.gamma_multiply(0.7 + 0.3 * breath));
            ui.ctx().request_repaint();
            response
        }
    }
}

/// A finished take, as the note shows it.
pub struct Toast {
    pub summary: RecordingSummary,
    /// Where the patch was saved beside it, if it was.
    pub patch: Option<PathBuf>,
    /// When it went up, in egui's clock.
    pub shown_at: f64,
}

/// What the note was asked to do.
pub enum ToastAction {
    ShowInFolder,
    Dismiss,
}

/// The note in the corner after a take: its name and length, whether any
/// audio was lost, and a way to find it.
pub fn show_toast(ctx: &egui::Context, toast: &Toast) -> (Option<ToastAction>, f32) {
    let summary = &toast.summary;
    let name = summary.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let failed = summary.error.is_some();
    let (dot, title) = if failed {
        (theme::accent::ERROR, "Recording stopped early")
    } else {
        (theme::accent::SUCCESS, "Recorded")
    };

    let response = Notice::new("recording_toast", dot, title).lifetime(toast.shown_at, TOAST_SECONDS).show(
        ctx,
        |ui| {
            ui.label(RichText::new(clock(summary.duration())).monospace().color(theme::text::SECONDARY));
        },
        |ui| {
            ui.label(RichText::new(name).color(theme::text::SECONDARY));
            if let Some(folder) = summary.path.parent() {
                ui.label(RichText::new(format!("in {}", short_path(folder))).small().color(theme::text::QUIET));
            }
            if toast.patch.is_some() {
                ui.label(RichText::new("The patch is saved beside it.").small().color(theme::text::QUIET));
            }
            if summary.dropped_frames > 0 {
                let seconds = summary.dropped_frames as f64 / summary.sample_rate.max(1) as f64;
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "⚠ {} frames ({:.2} s) were dropped: the disk fell behind.",
                        summary.dropped_frames, seconds
                    ))
                    .color(theme::accent::WARNING),
                );
            }
            if let Some(error) = &summary.error {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("⚠ {error}")).color(theme::accent::ERROR));
            }
            ui.add_space(8.0);
            ui.button("📂 Show in folder").clicked()
        },
    );
    let action = if response.closed || response.expired {
        Some(ToastAction::Dismiss)
    } else if response.inner {
        Some(ToastAction::ShowInFolder)
    } else {
        None
    };
    (action, response.height)
}

/// Opens the file manager with `path` selected (or its folder open, where
/// selecting isn't supported).
pub fn reveal_in_folder(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    {
        // Explorer wants `/select,"C:\...\take.wav"`: quotes around the path
        // only. `arg` would quote the whole thing (take names have spaces),
        // which Explorer can't read, so it opens its default folder instead
        // It also only finds paths written with backslashes
        use std::os::windows::process::CommandExt;
        let path = path.to_string_lossy().replace('/', "\\");
        let arg = format!("/select,\"{path}\"");
        Command::new("explorer").raw_arg(arg).spawn().map(drop)
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn().map(drop)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn().map(drop)
    }
}

/// Opens a folder in the file manager.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(path).spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn takes_are_named_after_the_patch_and_minute() {
        let dir = std::env::temp_dir().join("soba-take-names");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let when = chrono::Local.with_ymd_and_hms(2026, 10, 8, 14, 3, 59).unwrap();

        let first = take_path(&dir, "First Sound", when);
        assert_eq!(first.file_name().unwrap(), "First Sound 2026-10-08 14-03.wav");

        // The same minute again gets a number
        std::fs::write(&first, b"").unwrap();
        let second = take_path(&dir, "First Sound", when);
        assert_eq!(second.file_name().unwrap(), "First Sound 2026-10-08 14-03 (2).wav");
    }

    #[test]
    fn patch_names_are_made_safe_for_files() {
        assert_eq!(file_safe("Bass: v2/final?"), "Bass- v2-final-");
        assert_eq!(file_safe("  "), "Untitled");
        assert_eq!(file_safe("Pad..."), "Pad");
    }

    #[test]
    fn clock_reads_minutes_then_hours() {
        assert_eq!(clock(Duration::from_secs(9)), "0:09");
        assert_eq!(clock(Duration::from_secs(605)), "10:05");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }
}
