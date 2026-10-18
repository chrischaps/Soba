//! Share: the open patch as one link that plays in the browser, and the
//! browser's side of opening one.
//!
//! Share asks the link service for a short link (`persistence::short_link`)
//! and copies it, or copies the long one, which holds the whole patch
//! (`persistence::patch_link`), when the service doesn't answer in time.
//! A note in the corner says which, says that a short link's patch is kept
//! on soba.chaps.dev, and offers the long link instead. Or it says why the
//! patch can't go by link: it plays files of its own, and saving it as a
//! file takes them along. A patch opened from a link starts stopped, with a
//! note offering Play (browsers hold the sound until a click anyway), and
//! nothing about it is unsaved, so it's never autosaved over the visitor's
//! own work until they change it.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui::{self, RichText};

use crate::persistence::patch_link::{self, ShareError};
use crate::persistence::Patch;
use crate::persistence::sample_files::SampleBase;
use crate::persistence::short_link::{self, Answer};
use super::super::notice::Notice;
use super::super::theme;
use super::{SynthApp, WEB};

/// How long the copied note stays up.
const COPIED_SECONDS: f64 = 6.0;

/// How long the note offering the long link instead stays up.
const SHORT_COPIED_SECONDS: f64 = 12.0;

/// How long Share waits for a short link before copying the long one.
const MAKING_SECONDS: f64 = 4.0;

/// How long opening a short link waits for its patch.
const OPENING_SECONDS: f64 = 15.0;

/// Waited for, and nothing came.
const UNANSWERED: &str = "the link service didn't answer in time";

/// The note in the corner about sharing.
pub(super) struct ShareNote {
    /// When it went up, in egui's clock.
    shown_at: f64,
    kind: NoteKind,
}

enum NoteKind {
    /// Waiting for a short link to the patch named: the long one if none comes.
    Making { answer: Receiver<Answer>, name: String, long: String },
    /// A link is on the clipboard.
    Copied(Copied),
    /// It plays these files of its own.
    OwnFiles(Vec<String>),
    /// Something went wrong: what, and why.
    Failed(&'static str, String),
    /// Fetching the patch a short link names. At launch nothing else is
    /// open yet, so First Sound opens if it can't be had.
    Opening { answer: Receiver<Answer>, launch: bool },
    /// It was opened from a link, and is waiting to be played.
    Opened(String),
}

/// Which link was copied.
enum Copied {
    /// The short link, with the long one on offer.
    Short { short: String, long: String },
    /// The long link, asked for instead.
    Long(String),
    /// The long link, as the service didn't make a short one: why not.
    Fallback(String, String),
}

/// What the note asked for.
pub(super) enum ShareAction {
    SaveAs,
    Play,
    /// Open this long link's fragment (`p=1.…`), asking first about unsaved changes.
    OpenLink(String),
    /// A link at launch couldn't be opened: open what launching would.
    OpenOnLaunch,
}

impl SynthApp {
    /// Copies a link to the open patch, or says why there can't be one.
    pub(super) fn share_patch(&mut self, ctx: &egui::Context) {
        let patch = self.create_patch(&self.document.title());
        let now = ctx.input(|i| i.time);
        let kind = match patch_link::fragment(&patch) {
            Ok(fragment) => {
                let long = format!("{}#{fragment}", share_base());
                let payload = fragment.strip_prefix("p=").unwrap_or(&fragment).to_string();
                let wake = ctx.clone();
                let answer = short_link::create(payload, move || wake.request_repaint());
                NoteKind::Making { answer, name: patch.name, long }
            }
            Err(ShareError::OwnFiles(names)) => NoteKind::OwnFiles(names),
            Err(ShareError::Json(e)) => NoteKind::Failed("Couldn't make a link", e),
        };
        self.share_note = Some(ShareNote { shown_at: now, kind });
    }

    /// Opens the patch a link's fragment holds (`p=1.…`), stopped, or says
    /// why it can't, leaving the patch that's open as it was.
    pub fn open_from_link(&mut self, fragment: &str) -> Result<(), String> {
        self.open_linked(patch_link::patch_from_fragment(fragment)?)
    }

    /// Opens `patch`, from a link, stopped, or says why it can't.
    pub(super) fn open_linked(&mut self, patch: Patch) -> Result<(), String> {
        let warnings = self.load_patch(&patch, SampleBase::Keys).map_err(|e| e.to_string())?;
        // Opened from a link, it plays when asked, as an example does
        self.set_playing(false);
        self.fit_on_web();
        // In the browser it's remembered by name only, to offer when it's
        // saved. The desktop asks where, as for an example
        self.document.opened(WEB.then(|| PathBuf::from(format!("{}.json", patch.name))));
        self.status_message = Some(format!("Opened a shared patch: {}", patch.name));
        self.show_load_warnings(warnings);
        self.share_note = Some(ShareNote { shown_at: 0.0, kind: NoteKind::Opened(patch.name) });
        Ok(())
    }

    /// Fetches the patch the short link `id` names, to open once it comes.
    /// At `launch` nothing else is open, and First Sound opens if it fails.
    pub fn open_short_link(&mut self, ctx: &egui::Context, id: &str, launch: bool) {
        let wake = ctx.clone();
        let answer = short_link::resolve(id.to_string(), move || wake.request_repaint());
        self.status_message = Some("Opening a shared patch…".to_string());
        self.share_note = Some(ShareNote { shown_at: ctx.input(|i| i.time), kind: NoteKind::Opening { answer, launch } });
    }

    /// Says why a link couldn't be opened.
    pub fn link_refused(&mut self, ctx: &egui::Context, why: String) {
        let kind = NoteKind::Failed("Couldn't open the shared patch", why);
        self.share_note = Some(ShareNote { shown_at: ctx.input(|i| i.time), kind });
    }

    /// Collects what the link service answered, or gives up waiting.
    fn follow_link_service(&mut self, ctx: &egui::Context) -> Option<ShareAction> {
        let note = self.share_note.as_ref()?;
        let (answer, limit) = match &note.kind {
            NoteKind::Making { answer, .. } => (answer, MAKING_SECONDS),
            NoteKind::Opening { answer, .. } => (answer, OPENING_SECONDS),
            _ => return None,
        };
        let now = ctx.input(|i| i.time);
        let answer = match answer.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) if now - note.shown_at < limit => {
                // The service wakes the frame when it answers; this is for the limit
                ctx.request_repaint_after(Duration::from_millis(250));
                return None;
            }
            Err(_) => Err(UNANSWERED.to_string()),
        };
        let note = self.share_note.take()?;
        match note.kind {
            NoteKind::Making { name, long, .. } => {
                let copied = match answer {
                    Ok(id) => Copied::Short { short: short_link::link(&id), long },
                    Err(why) => Copied::Fallback(long, why),
                };
                ctx.copy_text(copied.link().to_string());
                self.status_message = Some(format!("Copied a link to {name}"));
                self.share_note = Some(ShareNote { shown_at: now, kind: NoteKind::Copied(copied) });
                None
            }
            NoteKind::Opening { launch, .. } => match answer {
                Ok(payload) => Some(ShareAction::OpenLink(format!("p={payload}"))),
                Err(why) => {
                    self.link_refused(ctx, why);
                    launch.then_some(ShareAction::OpenOnLaunch)
                }
            },
            _ => None,
        }
    }

    /// Draws the share note, if one is up, above notes `below` tall. Returns
    /// the height of the notes now drawn, and what the note asked for.
    pub(super) fn show_share_note(&mut self, ctx: &egui::Context, below: f32) -> (f32, Option<ShareAction>) {
        if let Some(action) = self.follow_link_service(ctx) {
            return (below, Some(action));
        }
        let Some(note) = &self.share_note else { return (below, None) };
        // Played already, by the toolbar or the space bar: it's done its job
        if matches!(note.kind, NoteKind::Opened(_)) && self.is_playing {
            self.share_note = None;
            return (below, None);
        }
        let mut action = None;
        let mut copy_long = None;
        let response = match &note.kind {
            NoteKind::Making { .. } => Notice::new("share_note", theme::accent::PRIMARY, "Making a link…")
                .above(below)
                .show(ctx, |ui| { ui.spinner(); }, |_| {}),
            NoteKind::Opening { .. } => Notice::new("share_note", theme::accent::PRIMARY, "Opening a shared patch…")
                .above(below)
                .show(ctx, |ui| { ui.spinner(); }, |_| {}),
            NoteKind::Copied(copied) => {
                let seconds = if matches!(copied, Copied::Short { .. }) { SHORT_COPIED_SECONDS } else { COPIED_SECONDS };
                Notice::new("share_note", theme::accent::SUCCESS, "Link copied")
                    .lifetime(note.shown_at, seconds)
                    .above(below)
                    .show(ctx, |_| {}, |ui| {
                        body(ui, "Paste it anywhere. Whoever opens it hears this patch in their browser, and can play with it, with nothing to install.");
                        match copied {
                            Copied::Short { short, long } => {
                                ui.add_space(4.0);
                                ui.label(RichText::new(short).monospace().color(theme::text::PRIMARY));
                                ui.add_space(4.0);
                                small(ui, "To keep the link short, the patch is kept on soba.chaps.dev, where anyone with the link can open it.");
                                ui.add_space(4.0);
                                let button = ui.button("Copy the long link instead");
                                if button.on_hover_text("It holds the whole patch, so nothing is kept anywhere, and it works offline").clicked() {
                                    copy_long = Some(long.clone());
                                }
                            }
                            Copied::Long(long) => {
                                small(ui, &format!("The long link holds the whole patch, so nothing is kept anywhere. {}", describe_length(long.len())));
                            }
                            Copied::Fallback(long, why) => {
                                small(ui, &format!("This is the long link, which holds the whole patch: {why}. {}", describe_length(long.len())));
                            }
                        }
                    })
            }
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
        if let Some(long) = copy_long {
            ctx.copy_text(long.clone());
            let shown_at = ctx.input(|i| i.time);
            self.share_note = Some(ShareNote { shown_at, kind: NoteKind::Copied(Copied::Long(long)) });
        } else if response.closed || response.expired || action.is_some() {
            self.share_note = None;
        }
        (below + response.height, action)
    }
}

impl Copied {
    /// The link on the clipboard.
    fn link(&self) -> &str {
        match self {
            Copied::Short { short, .. } => short,
            Copied::Long(long) | Copied::Fallback(long, _) => long,
        }
    }
}

/// Where a long link opens: this page, in the browser (so a local build
/// shares links to itself), and the published app from the desktop.
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

/// A quieter line, under the note's text.
fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::text::DISABLED).small());
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

#[cfg(not(target_arch = "wasm32"))]
impl SynthApp {
    /// Each example as Share sends it once it's opened: its title and its
    /// link's payload (`1.<data>`), or why it can't go by link. Opened in an
    /// app with no audio device. The garden's seed (`--bin garden_seed`,
    /// #392) plants these, so an example shared later is the same short link.
    pub fn shared_examples() -> Vec<(&'static str, Result<(String, String), ShareError>)> {
        crate::persistence::EXAMPLES
            .iter()
            .map(|example| {
                let mut app = SynthApp::with_devices(Err(crate::engine::AudioError::NoOutputDevice), None);
                app.open_example(example);
                let title = app.document.title();
                let fragment = patch_link::fragment(&app.create_patch(&title));
                (example.name, fragment.map(|f| (title, f.strip_prefix("p=").unwrap_or(&f).to_string())))
            })
            .collect()
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

    /// The link to `name`, opened in `app`.
    fn shared(app: &mut SynthApp, name: &str) -> String {
        app.open_example(example(name));
        patch_link::fragment(&app.create_patch(name)).unwrap()
    }

    #[test]
    fn a_patch_makes_the_same_link_whatever_was_open_before() {
        let headless = || SynthApp::with_devices(Err(crate::engine::AudioError::NoOutputDevice), None);
        // Breakdown's Scenes keep its knobs by module, and Canopy has groups
        let fresh = [shared(&mut headless(), "Breakdown"), shared(&mut headless(), "Canopy")];
        let mut app = headless();
        shared(&mut app, "Orrery");
        assert_eq!(shared(&mut app, "Breakdown"), fresh[0]);
        assert_eq!(shared(&mut app, "Canopy"), fresh[1]);
    }

    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn a_shared_patch_opens_the_same_and_stopped() {
        let ctx = egui::Context::default();
        let mut app = SynthApp::new();
        // Its samples ship with the examples, so go by key
        app.open_example(example("Sampled Keys"));
        let shared = app.create_patch("Sampled Keys");
        app.share_patch(&ctx);
        assert!(matches!(app.share_note, Some(ShareNote { kind: NoteKind::Making { .. }, .. })));

        let fragment = patch_link::fragment(&shared).unwrap();
        let mut friend = SynthApp::new();
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
        let mut app = SynthApp::new();
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
