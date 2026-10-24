//! The status bar along the bottom of the window. On the left, a message:
//! which one it shows, and how long each stays up (#308). Every message but
//! the patch's counts times out, so one that says what an action did can't
//! hide the warnings and refusals beneath it for good. On the right, what the
//! engine and the input are doing, and the patch's name.

use super::*;

/// What the left of the status bar shows, most pressing first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shown {
    /// The prompt to move a control or hit a pad, for as long as MIDI Learn
    /// waits for one.
    Learn,
    /// An action that failed, or a warning raised as the status.
    Failure,
    /// Why a cable was refused.
    Refusal,
    /// A warning: the input may hear the speakers, a driver reset.
    Notice,
    /// What the last action did.
    News,
    AudioError,
    MidiError,
    /// How many modules and cables the patch has, or how to add one.
    Counts,
}

impl SynthApp {
    /// Takes up news raised since it last looked as the status bar's status.
    pub(super) fn take_news(&mut self) {
        if let Some(text) = self.status_message.take() {
            self.status = Some(StatusLine::new(text, Severity::Info));
        }
    }

    pub(super) fn status_bar_shows(&mut self) -> Shown {
        let failed = self.status.as_ref().is_some_and(|line| line.severity >= Severity::Warning);
        if self.is_midi_learning() {
            Shown::Learn
        } else if failed {
            Shown::Failure
        } else if self.user_state.validation_message().is_some() {
            Shown::Refusal
        } else if self.notice.is_some() {
            Shown::Notice
        } else if self.status.is_some() {
            Shown::News
        } else if self.audio.error.is_some() {
            Shown::AudioError
        } else if self.midi.error.is_some() {
            Shown::MidiError
        } else {
            Shown::Counts
        }
    }

    /// Starts the clock of whatever the bar shows at `now`, and takes down
    /// what has been up long enough. Returns how long until something next
    /// has to come down.
    pub(super) fn tick_status_bar(&mut self, now: f64) -> Option<f64> {
        self.take_news();
        match self.status_bar_shows() {
            Shown::Failure | Shown::News => self.status.as_mut().map(|line| line.shown(now)),
            Shown::Notice => self.notice.as_mut().map(|line| line.shown(now)),
            _ => None,
        };
        let status = status_line::tick(&mut self.status, now);
        let notice = status_line::tick(&mut self.notice, now);
        status.into_iter().chain(notice).reduce(f64::min)
    }
}

/// What the audio output is doing, as the status bar shows it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AudioStatus {
    Playing,
    Stopped,
    /// The device errored or stopped asking for audio.
    NoAudio,
    /// The browser holds a page's sound until it's clicked or played.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Waiting,
}

impl SynthApp {
    /// What the audio output is doing: the transport's state, unless the
    /// device has reported an error or stopped asking for audio.
    pub(super) fn audio_status(&mut self, ctx: &egui::Context) -> AudioStatus {
        let transport = if self.is_playing { AudioStatus::Playing } else { AudioStatus::Stopped };
        // Offline (filming, or no stream to watch): the transport is the whole story
        let Ok(engine) = &self.audio.engine else { return transport };
        if !engine.is_running() {
            // A stream that couldn't be rebuilt (its device gone) is no audio
            return if engine.stream_lost() { AudioStatus::NoAudio } else { transport };
        }
        #[cfg(target_arch = "wasm32")]
        if web::audio_blocked() {
            return AudioStatus::Waiting;
        }

        let (count, failed) = (engine.callback_count(), engine.stream_failed());
        // Keep looking even when nothing else redraws, so a lost device shows
        ctx.request_repaint_after(AUDIO_STALL);

        if self.audio.stalled(count, Instant::now()) || failed {
            AudioStatus::NoAudio
        } else {
            transport
        }
    }

    /// Draw the bottom status bar
    pub(super) fn draw_status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            self.draw_status_message(ui);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                self.draw_engine_readout(ui);
                self.draw_patch_readout(ui);
            });
        });
    }

    /// The left of the bar: whatever [`Shown`] says is most pressing.
    fn draw_status_message(&mut self, ui: &mut egui::Ui) {
        // News raised since the last frame
        self.take_news();
        let warn = |ui: &mut egui::Ui, text: &str, color| {
            ui.label(RichText::new(format!("⚠ {}", text)).color(color).small());
        };
        match self.status_bar_shows() {
            Shown::Learn => {
                let prompt = self.midi.learning().map_or_else(String::new, |target| target.prompt());
                ui.label(RichText::new(prompt)
                    .color(theme::accent::PRIMARY)
                    .small());
            }
            Shown::Failure | Shown::News => {
                if self.status.as_ref().is_some_and(|line| status_line::draw(ui, line)) {
                    self.status = None;
                }
            }
            Shown::Refusal => warn(ui, self.user_state.validation_message().unwrap_or_default(), theme::accent::WARNING),
            Shown::Notice => {
                if self.notice.as_ref().is_some_and(|line| status_line::draw(ui, line)) {
                    self.notice = None;
                }
            }
            Shown::AudioError => warn(ui, self.audio.error.as_deref().unwrap_or_default(), theme::accent::ERROR),
            Shown::MidiError => warn(ui, self.midi.error.as_deref().unwrap_or_default(), theme::accent::ERROR),
            Shown::Counts => {
                // Show module and connection count, through any groups
                let node_count = self.user_state.node_id_map.len();
                let connection_count = self.user_state.engine_cables.len();

                let status = if node_count == 0 {
                    "Right-click to add nodes".to_string()
                } else if connection_count == 0 {
                    format!("{} node{}", node_count, if node_count == 1 { "" } else { "s" })
                } else {
                    format!(
                        "{} node{}, {} connection{}",
                        node_count,
                        if node_count == 1 { "" } else { "s" },
                        connection_count,
                        if connection_count == 1 { "" } else { "s" }
                    )
                };
                ui.label(RichText::new(status)
                    .color(theme::text::SECONDARY)
                    .small());
            }
        }
    }

    /// Engine status at the far right: what the output is doing, format, load
    fn draw_engine_readout(&mut self, ui: &mut egui::Ui) {
        let status = self.audio_status(ui.ctx());
        // Whether the open input dropped out lately
        let recent = self.audio.input.as_ref()
            .map(|m| m.underrun_frames() + m.overflow_frames() + m.device_xruns())
            .is_some_and(|glitches| self.audio.input_glitched(glitches, Instant::now()));
        if let Ok(engine) = &self.audio.engine {
            let (status_text, status_color, hint) = match status {
                AudioStatus::Playing => ("● Playing", theme::accent::SUCCESS, "The patch is playing"),
                AudioStatus::Stopped => ("○ Stopped", theme::text::DISABLED, "Press Play to hear the patch"),
                AudioStatus::NoAudio => (
                    "⚠ No audio",
                    theme::accent::ERROR,
                    "The output device has stopped taking audio (unplugged, or taken by another app). Choose it again under Output to reconnect.",
                ),
                AudioStatus::Waiting => (
                    "◌ Click to start sound",
                    theme::text::SECONDARY,
                    "Browsers keep a page quiet until you click or press a key in it",
                ),
            };
            ui.label(RichText::new(status_text).color(status_color).small())
                .on_hover_text(hint);
            ui.label(RichText::new(format!(
                "{}Hz • {}ch",
                engine.sample_rate(),
                engine.channels()
            )).color(theme::text::SECONDARY).small());
            self.draw_input_readout(ui, recent);
            if self.is_playing {
                let dropouts = self.poll_page_audio();
                let config = CpuMeterConfig { page: dropouts.is_some(), ..CpuMeterConfig::compact() };
                cpu_meter(ui, self.cpu_load, self.cpu_peak.peak(), &config);
                if let Some(dropouts) = dropouts {
                    page_audio::dropouts_label(ui, dropouts);
                }
            }
            ui.label(RichText::new("|")
                .color(theme::text::DISABLED)
                .small());
        }
    }

    /// The open input's round trip, and how it has held up: `recent` says
    /// it dropped out lately.
    fn draw_input_readout(&self, ui: &mut egui::Ui, recent: bool) {
        let (Some(monitor), Ok(engine)) = (&self.audio.input, &self.audio.engine) else { return };
        let trip = RoundTrip::of(monitor, engine.output_latency(), engine.sample_rate());
        let (text, color) = if monitor.failed() {
            ("⚠ Input lost".to_string(), theme::accent::ERROR)
        } else if recent {
            (input_device::round_trip_label(&trip), theme::accent::WARNING)
        } else {
            (input_device::round_trip_label(&trip), theme::text::SECONDARY)
        };
        let rate = engine.sample_rate().max(1) as f64;
        let details = if monitor.failed() {
            "The input device has stopped (unplugged, or taken by another app). Choose it again under Input.".to_string()
        } else {
            let converted = match engine.input_sample_rate() {
                Some(input_rate) if input_rate != engine.sample_rate() => format!(
                    "\nConverted from {} to {}",
                    input_device::khz(input_rate),
                    input_device::khz(engine.sample_rate())
                ),
                _ => String::new(),
            };
            let device_glitches = match monitor.device_xruns() {
                0 => String::new(),
                n => format!("\nGlitches the input device reported: {n}"),
            };
            let settling = match (monitor.settled_frames(), monitor.stretched_frames()) {
                (0, 0) => String::new(),
                (trimmed, stretched) => format!(
                    "\nSettling: {:.0} ms trimmed that wasn't needed, {:.0} ms stretched to keep up",
                    trimmed as f64 / rate * 1000.0,
                    stretched as f64 / rate * 1000.0,
                ),
            };
            format!(
                "Input: {}{}\n{}\nDropouts so far: {:.0} ms of silence, {:.0} ms skipped{}{}",
                engine.input_name().unwrap_or("?"),
                converted,
                input_device::round_trip_details(&trip),
                monitor.underrun_frames() as f64 / rate * 1000.0,
                monitor.overflow_frames() as f64 / rate * 1000.0,
                settling,
                device_glitches,
            )
        };
        ui.label(RichText::new(text).color(color).small()).on_hover_text(details);
        if recent {
            ui.ctx().request_repaint_after(INPUT_GLITCH_HOLD);
        }
    }

    /// Load warnings, the patch's name and the version, right to left.
    fn draw_patch_readout(&mut self, ui: &mut egui::Ui) {
        // Problems from the last load stay visible until dismissed
        if !self.load_warnings.is_empty() {
            let count = self.load_warnings.len();
            let label = ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "⚠ {} load warning{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    ))
                    .color(theme::accent::WARNING)
                    .small(),
                )
                .sense(egui::Sense::click()),
            );
            let details = format!("{}\n\nClick to dismiss", self.load_warnings.join("\n"));
            if label.on_hover_text(details).clicked() {
                self.load_warnings.clear();
            }
            ui.label(RichText::new("|")
                .color(theme::text::DISABLED)
                .small());
        }

        // Show current patch name if any
        if let Some(name) = self.document.status_name(self.has_unsaved_changes()) {
            // Right to left: the dot sits after the name
            if self.has_unsaved_changes() {
                ui.label(RichText::new("●").color(theme::accent::WARNING).small())
                    .on_hover_text(shortcuts::keys(ui.ctx(), "Unsaved changes (Ctrl+S to save)"));
            }
            ui.label(RichText::new(name)
                .color(theme::text::SECONDARY)
                .small());
            ui.label(RichText::new("|")
                .color(theme::text::DISABLED)
                .small());
        }
        // The version gives way on a narrow screen (a phone), rather
        // than running over the status message
        // On the desktop a dot after it lights up when there's a newer one
        if ui.available_width() >= 140.0 {
            #[cfg(not(target_arch = "wasm32"))]
            self.version_label(ui);
            #[cfg(target_arch = "wasm32")]
            ui.label(RichText::new(concat!("Soba v", env!("CARGO_PKG_VERSION"))).color(theme::text::QUIET).small());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the status bar's clock from `from` to `to`, a frame at a time.
    fn run(app: &mut SynthApp, from: f64, to: f64) {
        let mut now = from;
        while now <= to {
            app.tick_status_bar(now);
            now += 0.1;
        }
    }

    #[test]
    fn opening_an_example_doesnt_hold_the_status_bar() {
        let mut app = SynthApp::new();
        app.status_message = Some("Opened example: First Sound".to_string());
        app.tick_status_bar(0.0);
        assert_eq!(app.status_bar_shows(), Shown::News);
        run(&mut app, 0.0, 6.0);
        assert!(app.status.is_none(), "news goes after a while");
        assert_ne!(app.status_bar_shows(), Shown::News);
    }

    #[test]
    fn a_notice_shows_once_the_status_message_has_gone() {
        let mut app = SynthApp::new();
        app.status_error("Save failed: the disk is full");
        app.raise_notice("The input may be hearing the speakers".to_string());
        app.tick_status_bar(0.0);
        assert_eq!(app.status_bar_shows(), Shown::Failure);
        run(&mut app, 0.0, 11.5);
        assert_eq!(app.status_bar_shows(), Shown::Failure, "an error stays up a while");

        // The notice's time counts from when it's seen, not from when it was raised
        run(&mut app, 11.5, 13.0);
        assert_eq!(app.status_bar_shows(), Shown::Notice);
        run(&mut app, 13.0, 24.0);
        assert_eq!(app.status_bar_shows(), Shown::Notice, "it gets its whole time");
        run(&mut app, 24.0, 26.0);
        assert!(app.notice.is_none());
    }

    #[test]
    fn a_warning_comes_before_news_and_an_error_before_both() {
        let mut app = SynthApp::new();
        app.raise_notice("The driver's settings changed: audio restarted".to_string());
        app.status_message = Some("Saved: patch.json".to_string());
        app.tick_status_bar(0.0);
        assert_eq!(app.status_bar_shows(), Shown::Notice);
        app.status_error("Couldn't paste: the clipboard is empty");
        assert_eq!(app.status_bar_shows(), Shown::Failure);
        // News raised after an error replaces it
        app.status_message = Some("Pasted 2 modules".to_string());
        app.take_news();
        assert_eq!(app.status_bar_shows(), Shown::Notice);
    }
}
