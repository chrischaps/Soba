//! FM Voices' voices into the hardware (#448): each node's playing voice,
//! sent as a DX7 single voice by SysEx (#440) wherever it isn't what the
//! device was last sent. That covers a patch opened, a voice bred,
//! recalled, undone or pasted, and Send, which forgets what was sent.
//!
//! A voice is 163 bytes, about 55 ms down a DIN cable, and the volca takes
//! a moment to take it up, so a node sends no faster than [`GAP`] and
//! never while its port still has SysEx to send: only the newest voice
//! goes, however many were bred in between.
//!
//! The same voice goes to the engine (#472), by value, wherever it isn't
//! what the engine has, so Soba plays it too: no gap, as it's no cable.
//!
//! The file actions live here too: a `.syx` planted in the garden, the
//! voice or the garden saved, and the garden sent as a bank into the
//! device's memories.

use std::collections::HashMap;

use eframe::egui;
use egui_node_graph2::NodeId;

use crate::engine::EngineCommand;
use crate::graph::fm_voice_face::{self, VoiceFile};
use crate::hardware::dx7::{self, Target};
use crate::hardware::garden::Garden;
use crate::link::midi::{MIDI, PORT_LABELS};
use crate::link::sysex::SysexError;
use crate::modules::fm_voice;

use super::SynthApp;

/// The shortest time between two voices from one node, in seconds.
const GAP: f64 = 0.12;

/// What each FM Voice last sent, and when.
#[derive(Default)]
pub(super) struct FmSends {
    sent: HashMap<NodeId, Sent>,
    /// What a menu asked for, taken up once the frame is drawn.
    asked: Option<(NodeId, VoiceFile)>,
}

struct Sent {
    port: usize,
    /// The message, or empty when it didn't go (to try again after GAP).
    message: Vec<u8>,
    at: f64,
    /// Nothing sends yet: tried again as frames come, without asking for one.
    quiet: bool,
}

impl FmSends {
    /// Forgets what `node_id` sent, so it sends its voice again.
    pub(super) fn again(&mut self, node_id: NodeId) {
        self.sent.remove(&node_id);
    }

    /// Takes up `action` for `node_id` once the frame is drawn.
    pub(super) fn ask(&mut self, node_id: NodeId, action: VoiceFile) {
        self.asked = Some((node_id, action));
    }
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// A file picked in the browser, once it's been read: its name and bytes, for the node.
    static PICKED: std::cell::RefCell<Option<(NodeId, String, Vec<u8>)>> = const { std::cell::RefCell::new(None) };
}

impl SynthApp {
    /// Sends every FM Voice's voice where it's changed. Call once a frame.
    pub(super) fn sync_fm_voices(&mut self, ctx: &egui::Context) {
        if let Some((node_id, action)) = self.fm_sends.asked.take() {
            self.fm_voice_file(ctx, node_id, action);
        }
        #[cfg(target_arch = "wasm32")]
        if let Some((node_id, name, bytes)) = PICKED.with_borrow_mut(Option::take) {
            self.plant_file(node_id, &name, &bytes);
        }
        for command in self.fm_voice_plays() {
            self.send_command(command);
        }
        let graph = &self.graph_state.graph;
        self.fm_sends.sent.retain(|node_id, _| graph.nodes.contains_key(*node_id));
        // In the browser, SysEx waits until it's allowed (see the Rig)
        #[cfg(target_arch = "wasm32")]
        if !matches!(crate::engine::web_midi::sysex_access(), crate::engine::web_midi::SysexAccess::Allowed) {
            return;
        }
        let now = ctx.input(|i| i.time);
        let mut waiting = false;
        for (node_id, node) in graph.nodes.iter() {
            if node.user_data.module_id != fm_voice::ID {
                continue;
            }
            let (port, channel, target) = fm_voice_face::where_to(graph, node_id);
            let message = Garden::of(&node.user_data.labels).playing().voice.single(channel, target);
            let last = self.fm_sends.sent.get(&node_id);
            if last.is_some_and(|s| s.port == port && s.message == message) {
                continue;
            }
            if last.is_some_and(|s| now - s.at < GAP) || MIDI.traffic(port).sysex_waiting > 0 {
                waiting |= !last.is_some_and(|s| s.quiet);
                continue;
            }
            let (sent, quiet) = match MIDI.send_sysex(port, &message) {
                Ok(()) => {
                    ctx.data_mut(|d| d.insert_temp(fm_voice_face::sent_id(node_id), now));
                    (message, false)
                }
                // Nothing sends yet (the plugin, or MIDI out not begun)
                Err(SysexError::NoSender) => (Vec::new(), true),
                Err(_) => (Vec::new(), false),
            };
            self.fm_sends.sent.insert(node_id, Sent { port, message: sent, at: now, quiet });
        }
        if waiting {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(GAP));
        }
    }

    /// Every FM Voice's playing voice that isn't what the engine plays.
    fn fm_voice_plays(&self) -> Vec<EngineCommand> {
        let Some(handle) = self.ui_handle.as_ref() else { return Vec::new() };
        let mut plays = Vec::new();
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            if node.user_data.module_id != fm_voice::ID {
                continue;
            }
            let Some(engine_id) = self.user_state.get_engine_node_id(node_id) else { continue };
            if !handle.graph().contains_module(engine_id) {
                continue;
            }
            let voice = Garden::of(&node.user_data.labels).playing().voice;
            if handle.graph().fm_voice(engine_id) != Some(&voice) {
                plays.push(EngineCommand::SetFmVoice { node_id: engine_id, voice });
            }
        }
        plays
    }

    /// What an FM Voice's menu asked for.
    fn fm_voice_file(&mut self, ctx: &egui::Context, node_id: NodeId, action: VoiceFile) {
        let Some(node) = self.graph_state.graph.nodes.get(node_id) else { return };
        let garden = Garden::of(&node.user_data.labels);
        let (port, channel, _) = fm_voice_face::where_to(&self.graph_state.graph, node_id);
        match action {
            VoiceFile::Open => self.open_voice_file(ctx, node_id),
            VoiceFile::SaveVoice => {
                let voice = garden.playing().voice;
                let name = file_name(&voice.name());
                self.save_syx(&format!("{name}.syx"), &voice.single(channel, Target::Dx7));
            }
            VoiceFile::SaveBank => self.save_syx("FM garden.syx", &dx7::bank(&garden.bank(), channel)),
            VoiceFile::SendBank => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let answer = rfd::MessageDialog::new()
                        .set_title("Send the garden to the device's memories?")
                        .set_description(format!(
                            "The garden's {} voices go to port {} as a bank of 32. A volca fm writes a bank over all 32 of its programs, and what they held is gone. Save the volca's own programs first if you want them back.",
                            garden.voices.len(),
                            PORT_LABELS[port]
                        ))
                        .set_buttons(rfd::MessageButtons::YesNo)
                        .show();
                    if answer != rfd::MessageDialogResult::Yes {
                        return;
                    }
                }
                self.status_message = Some(match MIDI.send_sysex(port, &dx7::bank(&garden.bank(), channel)) {
                    Ok(()) => format!("Sending the garden to port {} as a bank: about {:.1} s down a DIN cable", PORT_LABELS[port], dx7::BANK_BYTES as f32 / 3125.0),
                    Err(error) => format!("The garden wasn't sent: {error}"),
                });
            }
        }
    }

    /// Asks for a `.syx` file and plants its voices in the garden.
    fn open_voice_file(&mut self, ctx: &egui::Context, node_id: NodeId) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            let picked = rfd::FileDialog::new().set_title("Open DX7 voices").add_filter("SysEx", &["syx", "SYX"]).add_filter("All files", &["*"]).pick_file();
            let Some(path) = picked else { return };
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            match std::fs::read(&path) {
                Ok(bytes) => self.plant_file(node_id, &name, &bytes),
                Err(error) => self.status_message = Some(format!("“{name}” couldn't be read: {error}")),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Some(file) = rfd::AsyncFileDialog::new().add_filter("SysEx", &["syx", "SYX"]).pick_file().await else { return };
                let bytes = file.read().await;
                PICKED.with_borrow_mut(|picked| *picked = Some((node_id, file.file_name(), bytes)));
                ctx.request_repaint();
            });
        }
    }

    /// Plants the voices of the file `name` in node `node_id`'s garden.
    fn plant_file(&mut self, node_id: NodeId, name: &str, bytes: &[u8]) {
        let voices = match dx7::read(bytes) {
            Ok(voices) => voices,
            Err(error) => {
                self.status_message = Some(format!("“{name}” wasn't opened: {error}"));
                return;
            }
        };
        let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) else { return };
        let mut garden = Garden::of(&node.user_data.labels);
        let planted = garden.plant(&voices);
        let found = if voices.len() == 1 { "1 voice".to_string() } else { format!("{} voices", voices.len()) };
        self.status_message = Some(match planted {
            0 => format!("“{name}” holds {found}, but the garden is full of kept voices: let some go first"),
            n if n < voices.len() => format!("Planted {n} of the {found} in “{name}”: the voices kept fill the rest"),
            _ => format!("Planted {found} from “{name}”"),
        });
        if planted > 0 {
            for (key, text) in garden.labels() {
                node.user_data.labels.insert(key, text);
            }
            self.history.name_next("Open FM voices");
        }
    }

    /// Saves `bytes` as a `.syx` file, asking where, or downloads it in the browser.
    fn save_syx(&mut self, suggested: &str, bytes: &[u8]) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = rfd::FileDialog::new().set_title("Save DX7 SysEx").add_filter("SysEx", &["syx"]).set_file_name(suggested).save_file() else { return };
            self.status_message = Some(match std::fs::write(&path, bytes) {
                Ok(()) => format!("Saved “{}”", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
                Err(error) => format!("Couldn't save it: {error}"),
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            if let Err(error) = crate::app::web::download_bytes(suggested, bytes, "application/octet-stream") {
                self.status_message = Some(format!("Couldn't save it: {error}"));
            }
        }
    }
}

/// A voice's name made fit for a file's: "SOFT KEY 3", or "FM voice".
fn file_name(name: &str) -> String {
    let name: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '.' { c } else { '-' }).collect();
    match name.trim().trim_matches('.') {
        "" => "FM voice".to_string(),
        name => name.to_string(),
    }
}
