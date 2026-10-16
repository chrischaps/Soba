//! Share: the open patch as one link that plays in the browser (see
//! `persistence::patch_link`), and the browser's side of opening one.
//!
//! The link is copied to the clipboard, and a note in the corner says so,
//! or says why the patch can't go by link: it plays files of its own, and
//! saving it as a file takes them along. A patch opened from a link starts
//! stopped, with a note offering Play (browsers hold the sound until a
//! click anyway), and nothing about it is unsaved, so it's never autosaved
//! over the visitor's own work until they change it.

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use crate::persistence::patch_link::{self, ShareError};
use crate::persistence::sample_files::SampleBase;
use super::super::notice::Notice;
use super::super::theme;
use super::{SynthApp, WEB};

/// How long the copied note stays up.
const COPIED_SECONDS: f64 = 6.0;

/// The note in the corner about sharing.
pub(super) struct ShareNote {
    /// When it went up, in egui's clock.
    shown_at: f64,
    kind: NoteKind,
}

enum NoteKind {
    /// The link is on the clipboard: how long it is, in characters.
    Copied(usize),
    /// It plays these files of its own.
    OwnFiles(Vec<String>),
    /// Something went wrong: what, and why.
    Failed(&'static str, String),
    /// It was opened from a link, and is waiting to be played.
    Opened(String),
}

/// What the note asked for.
pub(super) enum ShareAction {
    SaveAs,
    Play,
}

impl SynthApp {
    /// Copies a link to the open patch, or says why there can't be one.
    pub(super) fn share_patch(&mut self, ctx: &egui::Context) {
        let patch = self.create_patch(&self.patch_title());
        let kind = match patch_link::fragment(&patch) {
            Ok(fragment) => {
                let link = format!("{}#{fragment}", share_base());
                let length = link.len();
                ctx.copy_text(link);
                self.status_message = Some(format!("Copied a link to {}", patch.name));
                NoteKind::Copied(length)
            }
            Err(ShareError::OwnFiles(names)) => NoteKind::OwnFiles(names),
            Err(ShareError::Json(e)) => NoteKind::Failed("Couldn't make a link", e),
        };
        self.share_note = Some(ShareNote { shown_at: ctx.input(|i| i.time), kind });
    }

    /// Opens the patch a link's fragment holds (`p=1.…`), stopped, or says
    /// why it can't, leaving the patch that's open as it was.
    pub fn open_from_link(&mut self, fragment: &str) -> Result<(), String> {
        let patch = patch_link::patch_from_fragment(fragment)?;
        let warnings = self.load_patch(&patch, SampleBase::Keys).map_err(|e| e.to_string())?;
        // Opened from a link, it plays when asked, as an example does
        self.set_playing(false);
        self.fit_on_web();
        // In the browser it's remembered by name only, to offer when it's
        // saved. The desktop asks where, as for an example
        self.current_patch_path = WEB.then(|| PathBuf::from(format!("{}.json", patch.name)));
        self.current_example = None;
        self.status_message = Some(format!("Opened a shared patch: {}", patch.name));
        self.show_load_warnings(warnings);
        self.share_note = Some(ShareNote { shown_at: 0.0, kind: NoteKind::Opened(patch.name) });
        Ok(())
    }

    /// Says why a link couldn't be opened.
    pub fn link_refused(&mut self, ctx: &egui::Context, why: String) {
        let kind = NoteKind::Failed("Couldn't open the shared patch", why);
        self.share_note = Some(ShareNote { shown_at: ctx.input(|i| i.time), kind });
    }

    /// Draws the share note, if one is up, above notes `below` tall. Returns
    /// the height of the notes now drawn, and what the note asked for.
    pub(super) fn show_share_note(&mut self, ctx: &egui::Context, below: f32) -> (f32, Option<ShareAction>) {
        let Some(note) = &self.share_note else { return (below, None) };
        // Played already, by the toolbar or the space bar: it's done its job
        if matches!(note.kind, NoteKind::Opened(_)) && self.is_playing {
            self.share_note = None;
            return (below, None);
        }
        let mut action = None;
        let response = match &note.kind {
            NoteKind::Copied(length) => Notice::new("share_note", theme::accent::SUCCESS, "Link copied")
                .lifetime(note.shown_at, COPIED_SECONDS)
                .above(below)
                .show(ctx, |_| {}, |ui| {
                    body(ui, "Paste it anywhere. Whoever opens it hears this patch in their browser, and can play with it, with nothing to install.");
                    ui.label(RichText::new(describe_length(*length)).color(theme::text::DISABLED).small());
                }),
            NoteKind::OwnFiles(names) => Notice::new("share_note", theme::accent::WARNING, "Can't share this by link")
                .above(below)
                .show(ctx, |_| {}, |ui| {
                    let files = if names.len() == 1 { "a file of its own" } else { "files of its own" };
                    body(ui, &format!("It plays {files}: {}. A link holds the patch alone, and only the samples that come with Soba.", names.join(", ")));
                    let how = if WEB {
                        "Save downloads it with them, as a zip, to send instead."
                    } else {
                        "Save As puts them in the patch's folder, to zip and send instead."
                    };
                    body(ui, how);
                    ui.add_space(4.0);
                    if ui.button("💾 Save As").clicked() {
                        action = Some(ShareAction::SaveAs);
                    }
                }),
            NoteKind::Failed(title, e) => Notice::new("share_note", theme::accent::ERROR, *title)
                .above(below)
                .show(ctx, |_| {}, |ui| body(ui, e)),
            NoteKind::Opened(name) => Notice::new("share_note", theme::accent::PRIMARY, name.as_str())
                .above(below)
                .show(ctx, |_| {}, |ui| {
                    body(ui, "Someone shared this patch with you. It's yours to play with: nothing you change reaches them.");
                    ui.add_space(4.0);
                    if ui.button(RichText::new("▶ Play").color(theme::accent::SUCCESS).strong()).clicked() {
                        action = Some(ShareAction::Play);
                    }
                }),
        };
        if response.closed || response.expired || action.is_some() {
            self.share_note = None;
        }
        (below + response.height, action)
    }
}

/// Where a link opens: this page, in the browser (so a local build shares
/// links to itself), and the published app from the desktop.
fn share_base() -> String {
    #[cfg(target_arch = "wasm32")]
    if let Some(page) = super::web::full_app_url() {
        return page;
    }
    patch_link::PLAY_URL.to_string()
}

/// A line of a note's text.
fn body(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::text::SECONDARY));
}

/// How long a link is, and whether that's long for a chat to carry.
fn describe_length(length: usize) -> String {
    let size = if length < 10_000 { format!("{:.1} K", length as f32 / 1000.0) } else { format!("{} K", length / 1000) };
    if length > 30_000 {
        format!("{size} characters: long enough that some chats cut it short. Email or a document carries it whole.")
    } else {
        format!("{size} characters")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod round {
    use super::*;
    use crate::persistence::EXAMPLES;

    /// The patch with its modules numbered from 0, in order: the engine's
    /// IDs count up across the app.
    fn renumbered(mut patch: crate::persistence::Patch) -> crate::persistence::Patch {
        let ids: Vec<u64> = patch.nodes.iter().map(|node| node.id).collect();
        let index = |id: u64| ids.iter().position(|&i| i == id).unwrap() as u64;
        for connection in &mut patch.connections {
            connection.from_node = index(connection.from_node);
            connection.to_node = index(connection.to_node);
        }
        for node in &mut patch.nodes {
            node.id = index(node.id);
        }
        patch
    }

    fn example(name: &str) -> &'static crate::persistence::Example {
        EXAMPLES.iter().find(|e| e.name == name).expect("the example")
    }

    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn a_shared_patch_opens_the_same_and_stopped() {
        let ctx = egui::Context::default();
        let mut app = SynthApp::new(false);
        // Its samples ship with the examples, so go by key
        app.open_example(example("Sampled Keys"));
        let shared = app.create_patch("Sampled Keys");
        app.share_patch(&ctx);
        assert!(matches!(app.share_note, Some(ShareNote { kind: NoteKind::Copied(_), .. })));

        let fragment = patch_link::fragment(&shared).unwrap();
        let mut friend = SynthApp::new(false);
        friend.set_playing(true);
        friend.open_from_link(&fragment).unwrap();
        assert_eq!(renumbered(friend.create_patch("Sampled Keys")), renumbered(shared));
        assert!(!friend.is_playing);
        // Nothing to autosave over the friend's own work until they change it
        friend.sync_history();
        assert!(!friend.has_unsaved_changes());
        assert!(matches!(&friend.share_note, Some(ShareNote { kind: NoteKind::Opened(name), .. }) if name == "Sampled Keys"));
    }

    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn a_patch_with_its_own_files_says_why_it_cant_go_by_link() {
        let ctx = egui::Context::default();
        let mut app = SynthApp::new(false);
        app.open_example(example("Sampled Keys"));
        let sampler = app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.file.is_some()).map(|(id, _)| id).unwrap();
        app.graph_state.graph.nodes[sampler].user_data.file = Some("C:/Music/my piano.wav".into());
        app.share_patch(&ctx);
        assert!(matches!(&app.share_note, Some(ShareNote { kind: NoteKind::OwnFiles(names), .. }) if names == &["my piano.wav"]));

        // A damaged link leaves what's open alone
        let before = app.create_patch("x");
        assert!(app.open_from_link("p=1.AAAA").unwrap_err().contains("damaged"));
        assert_eq!(app.create_patch("x"), before);
    }
}
