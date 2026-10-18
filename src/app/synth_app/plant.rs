//! Plant in the garden (#396): the open patch published in the Patch
//! Garden, `soba.chaps.dev/garden`, for anyone to hear and grow their own
//! from.
//!
//! **🌱 Plant** beside Share asks, in a note in the corner, for a title and
//! the planter's name (optional, and remembered), and says plainly what
//! planting does: Share keeps a patch behind a link, and planting publishes
//! it. Then it posts the patch with the short link it grew from, if it did
//! (#395), so the garden can draw it as a branch of that one. A new plant
//! waits for Chris to show it, and the note says so. A busy or absent
//! service gets a quiet note that goes away by itself; words the service
//! won't keep bring the form back, saying why. A patch that plays files of
//! its own is refused as Share refuses it.
//!
//! The garden itself is a page in the browser: the desktop opens it there.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui::{self, RichText};

use crate::persistence::patch_link::{self, ShareError};
use crate::persistence::short_link::{self, Answer, Plant, GARDEN_URL, NAME_LENGTH, TITLE_LENGTH};
use super::super::notice::Notice;
use super::super::theme;
use super::SynthApp;

/// Where the planter's name is kept between launches.
const NAME_KEY: &str = "planter_name";

/// How long planting waits for the service.
const PLANTING_SECONDS: f64 = 15.0;

/// How long the note that it's planted stays up, and the one that it
/// couldn't be just now.
const PLANTED_SECONDS: f64 = 14.0;
const PASSING_SECONDS: f64 = 10.0;

/// What the service says when the patch is the very one it grew from.
const GREW_FROM_ITSELF: &str = "a patch can't grow from itself";

/// What the Plant button's tooltip says.
pub(super) const PLANT_HINT: &str = "Publish this patch in the Patch Garden, for anyone to hear and remix";

/// What the Visit the garden button's tooltip says.
pub(super) const GARDEN_HINT: &str = "soba.chaps.dev/garden: patches people have planted, to hear and open, in your browser";

/// Planting: the name to sign with, and the note in the corner.
#[derive(Default)]
pub(super) struct Planting {
    /// The planter's name, as they last gave it.
    name: String,
    note: Option<PlantNote>,
}

struct PlantNote {
    /// When it went up, in egui's clock.
    shown_at: f64,
    kind: NoteKind,
}

enum NoteKind {
    /// Asking for a title, and why the last try was turned away, if it was.
    Asking { title: String, payload: String, parent: Option<String>, why: Option<String>, focus: bool },
    /// Waiting for the service to plant `title`.
    Planting { answer: Receiver<Answer>, title: String, payload: String, parent: Option<String> },
    /// Planted: its title and short link.
    Planted { title: String, link: String },
    /// The service was busy or away, which passes: why.
    Passing(String),
}

impl Planting {
    pub(super) fn load(&mut self, storage: &dyn eframe::Storage) {
        self.name = storage.get_string(NAME_KEY).unwrap_or_default();
    }

    pub(super) fn store(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(NAME_KEY, self.name.clone());
    }
}

impl SynthApp {
    /// Asks for a title to plant the open patch under, or says why it
    /// can't go by link.
    pub(super) fn plant_patch(&mut self, ctx: &egui::Context) {
        let title = self.document.title();
        let patch = self.create_patch(&title);
        match patch_link::fragment(&patch) {
            Ok(fragment) => {
                let payload = fragment.strip_prefix("p=").unwrap_or(&fragment).to_string();
                // A new patch has no title worth offering
                let title = if self.document.path.is_none() && self.document.example.is_none() { String::new() } else { title };
                let title = title.chars().take(TITLE_LENGTH).collect();
                let parent = self.document.grown_from.clone();
                let kind = NoteKind::Asking { title, payload, parent, why: None, focus: true };
                self.planting.note = Some(PlantNote { shown_at: ctx.input(|i| i.time), kind });
            }
            Err(ShareError::OwnFiles(names)) => {
                self.planting.note = None;
                self.cant_go_by_link(ctx, names);
            }
            Err(ShareError::Json(e)) => {
                self.planting.note = None;
                self.link_refused(ctx, e);
            }
        }
    }

    /// Sends the plant, and waits for the service.
    fn send_plant(&mut self, ctx: &egui::Context, title: String, payload: String, parent: Option<String>) {
        let plant = Plant { payload: payload.clone(), title: title.clone(), name: self.planting.name.trim().to_string(), parent: parent.clone() };
        let wake = ctx.clone();
        let answer = short_link::plant(&plant, move || wake.request_repaint());
        self.await_plant(ctx, title, payload, parent, answer);
    }

    fn await_plant(&mut self, ctx: &egui::Context, title: String, payload: String, parent: Option<String>, answer: Receiver<Answer>) {
        self.status_message = Some(format!("Planting {title}…"));
        let kind = NoteKind::Planting { answer, title, payload, parent };
        self.planting.note = Some(PlantNote { shown_at: ctx.input(|i| i.time), kind });
    }

    /// Collects what the service answered, or gives up waiting.
    fn follow_planting(&mut self, ctx: &egui::Context) {
        let Some(PlantNote { shown_at, kind: NoteKind::Planting { answer, .. } }) = &self.planting.note else { return };
        let now = ctx.input(|i| i.time);
        let answer = match answer.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) if now - shown_at < PLANTING_SECONDS => {
                // The service wakes the frame when it answers; this is for the limit
                ctx.request_repaint_after(Duration::from_millis(250));
                return;
            }
            Err(_) => Err(short_link::UNANSWERED.to_string()),
        };
        let Some(PlantNote { kind: NoteKind::Planting { title, payload, parent, .. }, .. }) = self.planting.note.take() else { return };
        let kind = match answer {
            Ok(id) => {
                self.status_message = Some(format!("Planted {title} in the garden"));
                NoteKind::Planted { title, link: short_link::link(&id) }
            }
            Err(why) if short_link::passes(&why) => NoteKind::Passing(why),
            Err(why) => {
                // Opened from a link and not changed: it's that patch, not one grown from it
                let why = if why == GREW_FROM_ITSELF {
                    "This is the patch as it came to you. Change it, and plant what grows from it.".to_string()
                } else {
                    format!("The garden didn't take it: {why}.")
                };
                NoteKind::Asking { title, payload, parent, why: Some(why), focus: false }
            }
        };
        self.planting.note = Some(PlantNote { shown_at: now, kind });
    }

    /// Draws the planting note, if one is up, above notes `below` tall.
    /// Returns the height of the notes now drawn.
    pub(super) fn show_plant_note(&mut self, ctx: &egui::Context, below: f32) -> f32 {
        self.follow_planting(ctx);
        let Some(note) = &mut self.planting.note else { return below };
        let mut plant = None;
        let mut cancel = false;
        let name = &mut self.planting.name;
        let response = match &mut note.kind {
            NoteKind::Asking { title, why, focus, .. } => Notice::new("plant_note", theme::signal::GATE, "Plant in the garden")
                .above(below)
                .show(ctx, |_| {}, |ui| {
                    body(ui, "Planting publishes this patch in the Patch Garden, for anyone to hear, open and grow their own from. It appears there once it's approved.");
                    ui.add_space(6.0);
                    let field = |ui: &mut egui::Ui, text: &mut String, limit: usize, hint: &str| {
                        ui.add(egui::TextEdit::singleline(text).char_limit(limit).hint_text(hint).desired_width(f32::INFINITY))
                    };
                    let title_field = field(ui, title, TITLE_LENGTH, "Title");
                    if std::mem::take(focus) {
                        title_field.request_focus();
                    }
                    let name_field = field(ui, name, NAME_LENGTH, "Your name (optional)");
                    if let Some(why) = why {
                        ui.add_space(2.0);
                        ui.label(RichText::new(why.as_str()).color(theme::accent::WARNING));
                    }
                    ui.add_space(6.0);
                    let ready = !title.trim().is_empty();
                    let entered = (title_field.lost_focus() || name_field.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        let button = egui::Button::new(RichText::new("🌱 Plant").color(theme::signal::GATE).strong());
                        if ui.add_enabled(ready, button).clicked() || (ready && entered) {
                            plant = Some(title.trim().to_string());
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        visit_garden(ui);
                    });
                    ui.add_space(4.0);
                    small(ui, "Share keeps a patch behind a link only those you send it to can find. Planting puts it where anyone can.");
                }),
            NoteKind::Planting { title, .. } => Notice::new("plant_note", theme::signal::GATE, format!("Planting {title}…"))
                .above(below)
                .show(ctx, |ui| { ui.spinner(); }, |_| {}),
            NoteKind::Planted { title, link } => Notice::new("plant_note", theme::signal::GATE, format!("Planted {title}"))
                .lifetime(note.shown_at, PLANTED_SECONDS)
                .above(below)
                .show(ctx, |_| {}, |ui| {
                    body(ui, "It appears in the garden once it's approved, for anyone to hear and grow their own from. Its short link plays it now:");
                    ui.add_space(4.0);
                    ui.label(RichText::new(link.as_str()).monospace().color(theme::text::PRIMARY));
                    ui.add_space(4.0);
                    visit_garden(ui);
                }),
            NoteKind::Passing(why) => Notice::new("plant_note", theme::text::DISABLED, "Not planted just now")
                .lifetime(note.shown_at, PASSING_SECONDS)
                .above(below)
                .show(ctx, |_| {}, |ui| body(ui, &format!("The garden can't be reached: {why}. Nothing has changed: plant it again in a while."))),
        };
        if let Some(title) = plant {
            let Some(PlantNote { kind: NoteKind::Asking { payload, parent, .. }, .. }) = self.planting.note.take() else { unreachable!() };
            self.send_plant(ctx, title, payload, parent);
        } else if response.closed || response.expired || cancel {
            self.planting.note = None;
        }
        below + response.height
    }
}

/// The button that opens the garden, in the browser.
fn visit_garden(ui: &mut egui::Ui) {
    if ui.button("Visit the garden").on_hover_text(GARDEN_HINT).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(GARDEN_URL));
    }
}

/// A line of a note's text.
fn body(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::text::SECONDARY));
}

/// A quieter line, under the note's text.
fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::text::DISABLED).small());
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::persistence::EXAMPLES;

    fn headless() -> SynthApp {
        SynthApp::with_devices(Err(crate::engine::AudioError::NoOutputDevice), None)
    }

    fn asking(app: &SynthApp) -> Option<(&str, &Option<String>, &Option<String>)> {
        match &app.planting.note {
            Some(PlantNote { kind: NoteKind::Asking { title, parent, why, .. }, .. }) => Some((title, parent, why)),
            _ => None,
        }
    }

    /// Plants what's asked for, with `answer` as the service's.
    fn plant(app: &mut SynthApp, ctx: &egui::Context, answer: Answer) {
        let Some(PlantNote { kind: NoteKind::Asking { title, payload, parent, .. }, .. }) = app.planting.note.take() else { panic!("asking") };
        let (service, receiver) = std::sync::mpsc::channel();
        app.await_plant(ctx, title, payload, parent, receiver);
        service.send(answer).unwrap();
        app.follow_planting(ctx);
    }

    #[test]
    fn a_patch_is_planted_with_the_link_it_grew_from() {
        let ctx = egui::Context::default();
        let mut app = headless();
        app.open_example(EXAMPLES.iter().find(|e| e.name == "Lush Pad").unwrap());
        app.document.grown_from = Some("a1b2c3d4".into());
        app.plant_patch(&ctx);
        let (title, parent, why) = asking(&app).expect("asks for a title");
        assert_eq!((title, parent.as_deref(), why), ("Lush Pad", Some("a1b2c3d4"), &None));

        // Words the garden won't keep bring the form back, saying why
        plant(&mut app, &ctx, Err("please choose other words".into()));
        let (title, _, why) = asking(&app).expect("asks again");
        assert_eq!(title, "Lush Pad");
        assert!(why.as_deref().unwrap().contains("please choose other words"));

        // The very patch it came from isn't a new one
        plant(&mut app, &ctx, Err(GREW_FROM_ITSELF.into()));
        assert!(asking(&app).unwrap().2.as_deref().unwrap().contains("Change it"));

        // A busy service is a quiet note, gone by itself
        plant(&mut app, &ctx, Err("too many links at once: try again in a minute".into()));
        assert!(matches!(&app.planting.note, Some(PlantNote { kind: NoteKind::Passing(_), .. })));

        app.plant_patch(&ctx);
        plant(&mut app, &ctx, Ok("q1w2e3r4".into()));
        assert!(matches!(&app.planting.note, Some(PlantNote { kind: NoteKind::Planted { link, .. }, .. }) if link == "https://soba.chaps.dev/s/q1w2e3r4"));
    }

    #[test]
    fn a_new_patch_asks_for_a_title_and_one_with_its_own_files_is_refused() {
        let ctx = egui::Context::default();
        let mut app = headless();
        app.plant_patch(&ctx);
        assert_eq!(asking(&app).unwrap().0, "", "Untitled is no title");

        app.open_example(EXAMPLES.iter().find(|e| e.name == "Sampled Keys").unwrap());
        let sampler = app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.file.is_some()).map(|(id, _)| id).unwrap();
        app.graph_state.graph.nodes[sampler].user_data.file = Some("C:/Music/my piano.wav".into());
        app.plant_patch(&ctx);
        assert!(app.planting.note.is_none());
        assert!(app.share_note.is_some(), "Share's note says which files, and offers Save As");
    }

    #[test]
    fn the_name_is_remembered() {
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
        let planting = Planting { name: "Chris".into(), note: None };
        planting.store(&mut storage);
        let mut again = Planting::default();
        again.load(&storage);
        assert_eq!(again.name, "Chris");
    }
}
