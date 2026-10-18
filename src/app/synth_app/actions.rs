//! What the toolbar, its menus, the embed bar and the app's own shortcuts
//! ask for, as one list of [`AppAction`]s that [`SynthApp::dispatch`] does
//! once a frame, after the graph is drawn (#343).
//!
//! The shortcuts are one table, [`KEYS`]: each chord, when it listens and
//! what it does. Clicking Save and pressing Ctrl+S are the same action, so
//! they can't come to do different things, and a test holds the table
//! against the Keyboard Shortcuts sheet.

use std::path::PathBuf;

use eframe::egui::{self, Key, Modifiers};

use crate::engine::AudioSystem;
use crate::persistence::Example;
use super::super::lesson::LessonFile;
use super::super::recording;
use super::super::session::Discard;
use super::super::shortcuts;
use super::SynthApp;

/// Something to do once the frame is drawn.
#[derive(Clone, Debug)]
pub(super) enum AppAction {
    TogglePlaying,
    ToggleRecording,
    OpenRecordings,
    ChooseRecordingsFolder,
    ResetRecordingsFolder,
    RefreshDevices,
    SelectAudioSystem(AudioSystem),
    /// A buffer size, in frames.
    SelectBuffer(u32),
    SelectDevice(usize),
    RefreshInputDevices,
    /// Open this input device, or with `None` close the input.
    SelectInput(Option<usize>),
    New,
    Open,
    Save,
    SaveAs,
    Share,
    /// Plant the patch in the garden.
    Plant,
    OpenExample(&'static Example),
    OpenLesson(&'static LessonFile),
    OpenRecent(PathBuf),
    ClearRecent,
    Undo,
    Redo,
    AddModule,
    /// Bypass the selected modules, or switch them back on.
    Bypass,
    RefreshMidiDevices,
    ConnectMidi(usize),
    DisconnectMidi,
    // The browser has no updater
    #[cfg(not(target_arch = "wasm32"))]
    CheckUpdates,
    #[cfg(not(target_arch = "wasm32"))]
    RollBack,
}

/// When a shortcut listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Listens {
    /// Even while a text field has the keys: Ctrl+S saves from a note too.
    Always,
    /// Not while a text field has the keys (it has its own undo).
    UnlessTyping,
    /// Only while the canvas has the keys: not while typing, nor with the
    /// palette open.
    OnCanvas,
}

/// The keys a shortcut is pressed with.
#[derive(Clone, Copy, Debug)]
pub(super) enum Chord {
    /// Command (Ctrl, or ⌘ on a Mac), exactly these modifiers besides, and
    /// the key: see [`shortcuts::consume_command`].
    Command(Modifiers, Key),
    /// Shift+Space: see [`shortcuts::consume_play`].
    Play,
}

impl Chord {
    /// Takes the press, if it went down this frame.
    fn consume(self, i: &mut egui::InputState) -> bool {
        match self {
            Chord::Command(extra, key) => shortcuts::consume_command(i, extra, key),
            Chord::Play => shortcuts::consume_play(i),
        }
    }

    /// As the sheet writes it: `Ctrl + Shift + S`.
    #[cfg(test)]
    fn written(self) -> String {
        match self {
            Chord::Command(extra, key) => {
                let mut caps = vec!["Ctrl"];
                if extra.alt {
                    caps.push("Alt");
                }
                if extra.shift {
                    caps.push("Shift");
                }
                let name = key.name();
                caps.push(name);
                format!("`{}`", caps.join(" + "))
            }
            Chord::Play => "`Shift + Space`".to_string(),
        }
    }
}

/// One shortcut: its chord, when it listens, what it does, and the action
/// the Keyboard Shortcuts sheet gives it.
pub(super) struct Binding {
    pub chord: Chord,
    pub listens: Listens,
    pub action: AppAction,
    #[cfg_attr(not(test), allow(dead_code))]
    pub sheet: &'static str,
}

const fn bind(chord: Chord, listens: Listens, action: AppAction, sheet: &'static str) -> Binding {
    Binding { chord, listens, action, sheet }
}

/// The app's own shortcuts, taken in this order: where one chord holds
/// another (Ctrl+Shift+S holds Ctrl+S), the longer comes first. Editing's
/// copy, paste, groups, framing, scenes and Mutate have their own handlers,
/// which run after these.
pub(super) const KEYS: &[Binding] = &[
    bind(Chord::Command(Modifiers::NONE, Key::N), Listens::Always, AppAction::New, "New patch"),
    bind(Chord::Command(Modifiers::NONE, Key::O), Listens::Always, AppAction::Open, "Open a patch"),
    bind(Chord::Command(Modifiers::SHIFT, Key::S), Listens::Always, AppAction::SaveAs, "Save as"),
    bind(Chord::Command(Modifiers::NONE, Key::S), Listens::Always, AppAction::Save, "Save"),
    bind(Chord::Command(Modifiers::SHIFT, Key::Z), Listens::UnlessTyping, AppAction::Redo, "Redo"),
    bind(Chord::Command(Modifiers::NONE, Key::Y), Listens::UnlessTyping, AppAction::Redo, "Redo"),
    bind(Chord::Command(Modifiers::NONE, Key::Z), Listens::UnlessTyping, AppAction::Undo, "Undo"),
    bind(Chord::Command(Modifiers::NONE, Key::B), Listens::OnCanvas, AppAction::Bypass, "Bypass or switch on the selected filters and effects"),
    bind(Chord::Command(Modifiers::NONE, Key::R), Listens::OnCanvas, AppAction::ToggleRecording, "Record, or end the take"),
    // Before the palette's Space, which Shift+Space would match too
    bind(Chord::Play, Listens::OnCanvas, AppAction::TogglePlaying, "Play or stop the patch"),
];

/// The actions of the shortcuts pressed this frame, of those listening:
/// `typing` while a text field has the keys, `on_canvas` while the canvas
/// does. `hosted`, the DAW's transport plays the patch, so Shift+Space doesn't.
pub(super) fn pressed(ctx: &egui::Context, typing: bool, on_canvas: bool, hosted: bool) -> Vec<AppAction> {
    ctx.input_mut(|i| {
        KEYS.iter()
            .filter(|binding| match binding.listens {
                Listens::Always => true,
                Listens::UnlessTyping => !typing,
                Listens::OnCanvas => on_canvas,
            })
            .filter(|binding| !(hosted && matches!(binding.action, AppAction::TogglePlaying)))
            .filter(|binding| binding.chord.consume(i))
            .map(|binding| binding.action.clone())
            .collect()
    })
}

impl SynthApp {
    /// Does each of `actions`, in turn.
    pub(super) fn dispatch(&mut self, ctx: &egui::Context, actions: Vec<AppAction>) {
        for action in actions {
            self.dispatch_one(ctx, action);
        }
    }

    fn dispatch_one(&mut self, ctx: &egui::Context, action: AppAction) {
        match action {
            AppAction::TogglePlaying => self.set_playing(!self.is_playing),
            AppAction::ToggleRecording => self.toggle_recording(),
            AppAction::OpenRecordings => {
                let folder = self.recordings_folder();
                if let Err(e) = std::fs::create_dir_all(&folder).and_then(|()| recording::open_folder(&folder)) {
                    self.status_error(format!("Couldn't open {}: {}", folder.display(), e));
                }
            }
            AppAction::ChooseRecordingsFolder => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(folder) = rfd::FileDialog::new().set_directory(self.recordings_folder()).pick_folder() {
                    self.status_message = Some(format!("Recording to {}", folder.display()));
                    self.recordings_folder = Some(folder);
                }
            }
            AppAction::ResetRecordingsFolder => self.recordings_folder = None,
            AppAction::RefreshDevices => self.refresh_devices(),
            AppAction::SelectAudioSystem(system) => self.select_audio_system(system),
            AppAction::SelectBuffer(frames) => self.select_buffer(frames),
            AppAction::SelectDevice(index) => self.select_device(index),
            AppAction::RefreshInputDevices => self.refresh_input_devices(),
            AppAction::SelectInput(input) => self.select_input(input),
            AppAction::New => self.request(ctx, Discard::New),
            AppAction::Open => self.request(ctx, Discard::Open),
            AppAction::Save => {
                self.quick_save();
            }
            AppAction::SaveAs => {
                self.show_save_dialog();
            }
            AppAction::Share => self.share_patch(ctx),
            AppAction::Plant => self.plant_patch(ctx),
            AppAction::OpenExample(example) => self.request(ctx, Discard::OpenExample(example)),
            AppAction::OpenLesson(lesson) => self.request(ctx, Discard::OpenLesson(lesson)),
            AppAction::OpenRecent(path) => self.request(ctx, Discard::OpenFile(path)),
            AppAction::ClearRecent => self.document.recent.clear(),
            AppAction::Undo => self.undo(),
            AppAction::Redo => self.redo(),
            AppAction::AddModule => self.open_quick_add(ctx),
            AppAction::Bypass => {
                for node_id in self.graph_state.selected_nodes.clone() {
                    self.toggle_bypass(node_id);
                }
            }
            AppAction::RefreshMidiDevices => self.refresh_midi_devices(),
            AppAction::ConnectMidi(index) => self.connect_midi_device(index),
            AppAction::DisconnectMidi => self.disconnect_midi_device(),
            #[cfg(not(target_arch = "wasm32"))]
            AppAction::CheckUpdates => self.updater.check_now(ctx),
            #[cfg(not(target_arch = "wasm32"))]
            AppAction::RollBack => self.request(ctx, Discard::RollBack),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::shortcuts::SHEET;

    /// Every shortcut in the table is on the sheet, on the row for what it does.
    #[test]
    fn every_shortcut_is_on_the_sheet() {
        for binding in KEYS {
            let chord = binding.chord.written();
            let row = SHEET
                .iter()
                .flat_map(|group| group.rows.iter())
                .find(|(_, action)| *action == binding.sheet);
            let Some((keys, _)) = row else {
                panic!("no row on the sheet says {:?}", binding.sheet);
            };
            assert!(keys.contains(&chord), "the sheet's {:?} row is {keys}, without {chord}", binding.sheet);
        }
    }

    /// One frame with `chord` pressed, under the given listening.
    fn press(modifiers: Modifiers, key: Key, typing: bool, on_canvas: bool) -> Vec<String> {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
            ],
            ..Default::default()
        };
        let mut got = Vec::new();
        ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx();
            got = pressed(ctx, typing, on_canvas, false).iter().map(|a| format!("{a:?}")).collect();
        }).textures_delta.clear();
        got
    }

    #[test]
    fn longer_chords_win_and_typing_holds_the_rest() {
        let command = Modifiers::COMMAND;
        assert_eq!(press(command | Modifiers::SHIFT, Key::S, false, true), ["SaveAs"]);
        assert_eq!(press(command, Key::S, false, true), ["Save"]);
        assert_eq!(press(command | Modifiers::SHIFT, Key::Z, false, true), ["Redo"]);
        assert_eq!(press(command, Key::Z, false, true), ["Undo"]);
        // Saving works from a text field; undo is the field's own
        assert_eq!(press(command, Key::S, true, false), ["Save"]);
        assert!(press(command, Key::Z, true, false).is_empty());
        // Bypass waits for the canvas, and AltGr (Ctrl+Alt) types, not saves
        assert!(press(command, Key::B, false, false).is_empty());
        assert!(press(command | Modifiers::ALT, Key::S, false, true).is_empty());
    }
}
