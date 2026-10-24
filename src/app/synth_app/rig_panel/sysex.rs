//! Sending a `.syx` file to a port's device from the Rig (#440): a DX7
//! voice to a Volca FM, a bank, a controller's settings. Each port's
//! device menu offers it. The file's messages go whole and in order, as
//! fast as the port's budget lets them (see `link::midi_sender`).
//!
//! In the browser, the first file sent asks for SysEx first (see
//! [`web_midi::request_sysex`](crate::engine::web_midi::request_sysex)):
//! the browser words it as letting the page "control and reprogram your
//! MIDI devices", so the panel says why before it asks.

use super::*;

/// What the last file sent came to, for a line in the panel.
#[derive(Clone, Debug)]
pub(super) struct SysexNote {
    pub text: String,
    pub error: bool,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// What a file picked in the browser came to, once it's been read.
    static PICKED: std::cell::RefCell<Option<SysexNote>> = const { std::cell::RefCell::new(None) };
}

impl SynthApp {
    /// Asks for a `.syx` file and sends it on `port`.
    pub(super) fn send_sysex_file(&mut self, ctx: &egui::Context, port: usize) {
        let _ = ctx;
        let Some(device) = MIDI.found()[port].clone() else {
            self.rig_panel.sysex = Some(SysexNote { text: format!("Port {}'s device isn't plugged in", PORT_LABELS[port]), error: true });
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let picked = rfd::FileDialog::new().set_title(format!("Send SysEx to {device}")).add_filter("SysEx", &["syx", "SYX"]).add_filter("All files", &["*"]).pick_file();
            let Some(path) = picked else { return };
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            let note = match std::fs::read(&path) {
                Ok(file) => sent(&name, port, &device, &file),
                Err(error) => SysexNote { text: format!("“{name}” couldn't be read: {error}"), error: true },
            };
            self.rig_panel.sysex = Some(note);
        }
        #[cfg(target_arch = "wasm32")]
        {
            use crate::engine::web_midi::{self, SysexAccess};
            match web_midi::sysex_access() {
                SysexAccess::Allowed => {
                    let ctx = ctx.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let Some(file) = rfd::AsyncFileDialog::new().add_filter("SysEx", &["syx", "SYX"]).pick_file().await else { return };
                        let note = sent(&file.file_name(), port, &device, &file.read().await);
                        PICKED.with_borrow_mut(|picked| *picked = Some(note));
                        ctx.request_repaint();
                    });
                }
                SysexAccess::Asking => {}
                _ => {
                    web_midi::request_sysex(ctx);
                    self.rig_panel.sysex = Some(SysexNote {
                        text: "SysEx can rewrite a device's sounds and settings, so the browser asks first: allow it to “control and reprogram your MIDI devices”, then send the file again".to_string(),
                        error: false,
                    });
                }
            }
        }
    }

    /// Takes up what a file picked in the browser came to, and what the
    /// browser said about SysEx.
    pub(super) fn tend_sysex_note(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(note) = PICKED.with_borrow_mut(Option::take) {
                self.rig_panel.sysex = Some(note);
            }
            if let crate::engine::web_midi::SysexAccess::Refused(why) = crate::engine::web_midi::sysex_access() {
                if self.rig_panel.sysex.as_ref().is_some_and(|note| !note.error) {
                    self.rig_panel.sysex = Some(SysexNote { text: why, error: true });
                }
            }
        }
    }
}

/// Sends `file`, called `name`, on `port`, and says how it went.
fn sent(name: &str, port: usize, device: &str, file: &[u8]) -> SysexNote {
    match MIDI.send_sysex_file(port, file) {
        Ok((messages, bytes)) => {
            let count = if messages == 1 { "1 message".to_string() } else { format!("{messages} messages") };
            let budget = MIDI.budget(port);
            // A DIN port carries about three bytes a millisecond
            let pace = match budget {
                0 => String::new(),
                per_second => {
                    let seconds = bytes as f32 / (per_second as f32 * 3.0);
                    if seconds >= 0.5 { format!(", over {seconds:.1} s at the port's budget") } else { String::new() }
                }
            };
            SysexNote { text: format!("Sent “{name}” to {device} on port {}: {count}, {}{pace}", PORT_LABELS[port], size(bytes as u32)), error: false }
        }
        Err(error) => SysexNote { text: format!("“{name}” wasn't sent: {error}"), error: true },
    }
}

/// A count of bytes, short: 163 B, 4.0 KB.
pub(super) fn size(bytes: u32) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f32 / 1024.0)
    }
}

/// The note on the last file sent, and a cross to put it away: true when
/// it's put away.
pub(super) fn sysex_line(ui: &mut egui::Ui, note: &SysexNote) -> bool {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let ink = if note.error { theme::accent::ERROR } else { theme::signal::MIDI.lerp_to_gamma(theme::text::PRIMARY, 0.45) };
        let closed = cross(ui, 12.0).on_hover_text("Put this away").clicked();
        ui.add(egui::Label::new(RichText::new(&note.text).small().color(ink)).wrap());
        closed
    })
    .inner
}

/// The SysEx a port last sent, or has still to send, for its Last sent cell.
pub(super) fn sysex_last(ui: &mut egui::Ui, traffic: &Traffic, since: f64) {
    let purple = theme::signal::MIDI.lerp_to_gamma(theme::text::PRIMARY, 0.3);
    let (text, tip) = if traffic.sysex_waiting > 0 {
        (format!("SysEx · {} to go", size(traffic.sysex_waiting)), "A SysEx dump going out as fast as the port's budget lets it")
    } else {
        (format!("SysEx · {}", size(traffic.sysex_bytes)), "The last thing this port sent was a SysEx message")
    };
    let fresh = if traffic.sysex_waiting > 0 { 1.0 } else { (1.0 - since / FRESH).clamp(0.0, 1.0) as f32 };
    let ink = theme::text::DISABLED.lerp_to_gamma(purple, 0.35 + 0.65 * fresh);
    ui.add(egui::Label::new(RichText::new(text).small().monospace().color(ink)).truncate()).on_hover_text(tip);
}
