//! The toolbar along the top of the window: the wordmark, transport, file
//! and edit buttons, the Cables, Knobs and Help menus, and the sound and
//! MIDI devices.

use super::*;

/// Max popup height for the toolbar device dropdowns. egui's default (200px)
/// fits only ~3 rows at the theme's padding; egui still clamps to the window.
const DEVICE_MENU_HEIGHT: f32 = 480.0;

/// The manual, for the Help menu.
const MANUAL_URL: &str = "https://docs.chaps.dev/soba/";

/// What the Share button's tooltip says.
const SHARE_HINT: &str = "Copy a link that plays this patch in a browser, with nothing to install";

/// Tooltip for the Undo and Redo buttons, e.g. "Undo Move Oscillator (Ctrl+Z)".
fn history_hint(verb: &str, label: Option<&str>, shortcut: &str) -> String {
    match label {
        Some(label) => format!("{verb} {label} ({shortcut})"),
        None => format!("Nothing to {} ({shortcut})", verb.to_lowercase()),
    }
}

/// The wordmark that starts the toolbar: one short cable between two jacks,
/// then "soba". Blender calls the wires between nodes noodles, and soba are
/// long, fine ones.
fn wordmark(ui: &mut egui::Ui) -> egui::Response {
    let mut job = egui::text::LayoutJob::default();
    job.append("soba", 0.0, egui::TextFormat {
        font_id: egui::FontId::new(20.0, egui::FontFamily::Name(theme::TITLE_FAMILY.into())),
        color: theme::text::PRIMARY,
        extra_letter_spacing: 0.6,
        ..Default::default()
    });
    let word = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let cable = egui::vec2(20.0, word.size().y);
    let gap = 7.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(cable.x + gap + word.size().x, cable.y), egui::Sense::hover());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        // Centred on the lowercase letters, which sit low in the line
        let mid = rect.left_center().y + 1.5;
        let from = egui::pos2(rect.left() + 3.0, mid - 4.5);
        let to = egui::pos2(rect.left() + cable.x - 3.0, mid + 4.5);
        let pull = egui::vec2((to.x - from.x) * 0.7, 0.0);
        let points = [from, from + pull, to - pull, to];
        let curve = |width: f32, color: egui::Color32| {
            egui::epaint::CubicBezierShape::from_points_stroke(points, false, egui::Color32::TRANSPARENT, egui::Stroke::new(width, color))
        };
        painter.add(curve(5.0, theme::signal::AUDIO.gamma_multiply(0.2)));
        painter.add(curve(1.8, theme::signal::AUDIO));
        for jack in [from, to] {
            painter.circle(jack, 2.6, theme::background::WIDGET, egui::Stroke::new(1.2_f32, theme::signal::AUDIO));
        }
        painter.galley(egui::pos2(rect.left() + cable.x + gap, rect.top()), word, theme::text::PRIMARY);
    }
    response.on_hover_text("Soba: a node-based modular synthesizer")
}

impl SynthApp {
    /// The recently opened patches, as menu items.
    fn recent_menu(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        if self.document.recent.is_empty() {
            ui.label(RichText::new("No recent patches").color(theme::text::DISABLED).italics());
            return;
        }
        for path in self.document.recent.iter() {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            let button = ui.add_enabled(path.exists(), egui::Button::new(name));
            let clicked = button
                .on_hover_text(path.display().to_string())
                .on_disabled_hover_text(format!("Missing: {}", path.display()))
                .clicked();
            if clicked {
                actions.push(AppAction::OpenRecent(path.to_path_buf()));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(RichText::new("Clear Recent").color(theme::text::SECONDARY)).clicked() {
            actions.push(AppAction::ClearRecent);
            ui.close();
        }
    }

    /// Draw the top toolbar: transport, file, edit and device controls.
    ///
    /// When the window is too narrow for the full row, it goes compact: the
    /// file buttons fold into one File menu, Undo and Redo keep only their
    /// arrows, the group labels drop away and the gaps tighten.
    pub(super) fn draw_toolbar(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();

        // A row centres each item in the height it has reached so far, and
        // grows when a taller item arrives. Starting every row at the height
        // of a button keeps the labels, buttons and selectors on one line.
        let row_height = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
        ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(row_height);
        // Labels whole, so a wrapped row places them like any other item
        // instead of flowing their text onto the next line
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        let compact = self.toolbar_full_width.is_some_and(|width| width > ui.available_width());
        let bar = Bar { ctx: ui.ctx().clone(), compact, gap: if compact { 10.0 } else { 16.0 }, row_height };

        let row = |ui: &mut egui::Ui| {
            wordmark(ui);
            bar.group_break(ui);
            self.transport_buttons(ui, &bar, &mut actions);
            bar.group_break(ui);
            self.file_buttons(ui, &bar, &mut actions);
            bar.group_break(ui);
            self.edit_buttons(ui, &bar, &mut actions);
            bar.group_break(ui);
            self.view_menus(ui, &mut actions);
            self.device_menus(ui, &bar, &mut actions);
        };

        if compact {
            // Too narrow even for the compact row: wrap rather than run off the edge
            ui.horizontal_wrapped(row);
        } else {
            let full_width = ui.horizontal(row).response.rect.width();
            if full_width > ui.available_width() {
                ui.ctx().request_repaint();
            }
            self.toolbar_full_width = Some(full_width);
        }

        actions
    }

    /// Play or Stop, and Record.
    fn transport_buttons(&mut self, ui: &mut egui::Ui, bar: &Bar, actions: &mut Vec<AppAction>) {
        bar.group_label(ui, "Transport");

        // Play/Stop button - controls whether the audio graph is processing
        let play_text = if self.is_playing { "⏹ Stop" } else { "▶ Play" };
        let play_color = if self.is_playing {
            theme::accent::WARNING
        } else {
            theme::accent::SUCCESS
        };

        // In a DAW, the DAW's transport plays it
        let play_hint = if self.is_playing { "Stop the patch (Shift+Space)" } else { "Play the patch (Shift+Space)" };
        if self.hosted.is_none() && ui.button(RichText::new(play_text).color(play_color)).on_hover_text(play_hint).clicked() {
            actions.push(AppAction::TogglePlaying);
        }

        // Record what you hear: the button breathes while a take runs.
        // The browser has no folder to write takes to
        if !WEB {
            let rec_state = match &self.recording {
                None => RecState::Idle,
                Some(take) if take.is_stopping() => RecState::Finishing,
                Some(take) => RecState::Recording(take.elapsed()),
            };
            let folder = self.recordings_folder();
            let hint = match rec_state {
                RecState::Recording(_) => bar.keys("Stop recording and save the take (Ctrl+R)"),
                _ => format!(
                    "Record what you hear to a WAV in {} ({})\nRight-click to choose the folder",
                    recording::short_path(&folder),
                    bar.keys("Ctrl+R"),
                ),
            };
            let rec = recording::rec_button(ui, &rec_state).on_hover_text(hint);
            if rec.clicked() {
                actions.push(AppAction::ToggleRecording);
            }
            crate::widgets::menu::context_menu(&rec, |ui| {
                ui.label(RichText::new("Recordings folder").color(theme::text::SECONDARY));
                ui.label(recording::short_path(&folder));
                ui.separator();
                if ui.button("📂 Open Folder").clicked() {
                    actions.push(AppAction::OpenRecordings);
                    ui.close();
                }
                if ui.button("Change…").clicked() {
                    actions.push(AppAction::ChooseRecordingsFolder);
                    ui.close();
                }
                if ui.add_enabled(self.recordings_folder.is_some(), egui::Button::new("Use Music/Soba")).clicked() {
                    actions.push(AppAction::ResetRecordingsFolder);
                    ui.close();
                }
            });
        }
    }

    /// File operations: named buttons, or one menu when space is short, and
    /// the Examples.
    fn file_buttons(&mut self, ui: &mut egui::Ui, bar: &Bar, actions: &mut Vec<AppAction>) {
        bar.group_label(ui, "File");

        if bar.compact {
            crate::widgets::menu::menu_button(ui, "📁 File", |ui| {
                let item = |ui: &mut egui::Ui, text: &str, shortcut: &'static str| {
                    ui.add(egui::Button::new(text).shortcut_text(bar.keys(shortcut))).clicked()
                };
                if item(ui, "📄 New", "Ctrl+N") {
                    actions.push(AppAction::New);
                    ui.close();
                }
                if item(ui, "📂 Open", "Ctrl+O") {
                    actions.push(AppAction::Open);
                    ui.close();
                }
                if !WEB {
                    crate::widgets::menu::menu_button(ui, "🕘 Recent", |ui| self.recent_menu(ui, actions));
                }
                ui.separator();
                if item(ui, "💾 Save", "Ctrl+S") {
                    actions.push(AppAction::Save);
                    ui.close();
                }
                if item(ui, "💾 Save As", "Ctrl+Shift+S") {
                    actions.push(AppAction::SaveAs);
                    ui.close();
                }
                if ui.button("🔗 Share").on_hover_text(SHARE_HINT).clicked() {
                    actions.push(AppAction::Share);
                    ui.close();
                }
                if ui.button("🌱 Plant in the garden").on_hover_text(plant::PLANT_HINT).clicked() {
                    actions.push(AppAction::Plant);
                    ui.close();
                }
            });
        } else {
            if ui.button("📄 New").on_hover_text(bar.keys("Start an empty patch (Ctrl+N)")).clicked() {
                actions.push(AppAction::New);
            }

            if ui.button("📂 Open").on_hover_text(bar.keys("Ctrl+O")).clicked() {
                actions.push(AppAction::Open);
            }

            if !WEB {
                crate::widgets::menu::menu_button(ui, "🕘 Recent", |ui| self.recent_menu(ui, actions));
            }
        }

        crate::widgets::menu::menu_button(ui, "📚 Examples", |ui| {
            for (n, shelf) in Shelf::ALL.into_iter().enumerate() {
                if n > 0 {
                    ui.separator();
                }
                ui.label(RichText::new(shelf.title()).small().color(theme::text::SECONDARY));
                for example in EXAMPLES.iter().filter(|e| e.shelf == shelf) {
                    if ui.button(example.name).on_hover_text(example.description).clicked() {
                        actions.push(AppAction::OpenExample(example));
                        ui.close();
                    }
                }
            }
            // More to hear: what people have planted
            ui.separator();
            if ui.button("🌱 Visit the garden").on_hover_text(plant::GARDEN_HINT).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(crate::persistence::short_link::GARDEN_URL));
                ui.close();
            }
        });

        if !bar.compact {
            if ui.button("💾 Save").on_hover_text(bar.keys("Ctrl+S")).clicked() {
                actions.push(AppAction::Save);
            }

            if ui.button("💾 Save As").on_hover_text(bar.keys("Save to a new file (Ctrl+Shift+S)")).clicked() {
                actions.push(AppAction::SaveAs);
            }

            if ui.button("🔗 Share").on_hover_text(SHARE_HINT).clicked() {
                actions.push(AppAction::Share);
            }

            if ui.button("🌱 Plant").on_hover_text(plant::PLANT_HINT).clicked() {
                actions.push(AppAction::Plant);
            }
        }
    }

    /// Edit history, and Add in the browser.
    fn edit_buttons(&mut self, ui: &mut egui::Ui, bar: &Bar, actions: &mut Vec<AppAction>) {
        bar.group_label(ui, "Edit");

        // Space or Tab opens the palette. A phone has neither key, and
        // a visitor may not know to press them
        if WEB && ui.button(bar.named("+", "Add")).on_hover_text("Add a module (Space or Tab)").clicked() {
            actions.push(AppAction::AddModule);
        }
        let undo_label = self.history.undo_label();
        let undo = ui.add_enabled(undo_label.is_some(), egui::Button::new(bar.named("↩", "Undo")));
        if undo.on_hover_text(history_hint("Undo", undo_label, &bar.keys("Ctrl+Z"))).clicked() {
            actions.push(AppAction::Undo);
        }
        let redo_label = self.history.redo_label();
        let redo = ui.add_enabled(redo_label.is_some(), egui::Button::new(bar.named("↪", "Redo")));
        if redo.on_hover_text(history_hint("Redo", redo_label, &bar.keys("Ctrl+Shift+Z"))).clicked() {
            actions.push(AppAction::Redo);
        }
    }

    /// The Cables, Knobs and Help menus.
    fn view_menus(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        // How the signal is drawn flowing along the cables
        crate::widgets::menu::menu_button(ui, "〰 Cables", |ui| {
            ui.label(RichText::new("Signal flow marks").color(theme::text::SECONDARY));
            for glyph in FlowGlyph::ALL {
                ui.radio_value(&mut self.user_state.flow_glyph, glyph, glyph.name());
            }
            ui.separator();
            ui.label(RichText::new("Under modules and labels").color(theme::text::SECONDARY));
            ui.add(
                egui::Slider::new(&mut self.user_state.cable_through, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                    .custom_parser(|text| text.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0)),
            )
            .on_hover_text("How much of a cable shows through a module, frame title or note it passes under");
            ui.separator();
            self.probe.duck_menu(ui);
        })
        .response
        .on_hover_text("How signal flow is drawn along cables");

        // How the modules' knobs are drawn
        crate::widgets::menu::menu_button(ui, "◉ Knobs", |ui| {
            ui.label(RichText::new("Knob style").color(theme::text::SECONDARY));
            for style in KnobStyle::ALL {
                ui.radio_value(&mut self.user_state.knob_style, style, style.name());
            }
        })
        .response
        .on_hover_text("How knobs are drawn");

        crate::widgets::menu::menu_button(ui, "❓ Help", |ui| {
            if ui.button("📖 Manual").on_hover_text("docs.chaps.dev/soba, in your browser").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(MANUAL_URL));
                ui.close();
            }
            if ui.add(egui::Button::new("⌨ Keyboard Shortcuts").shortcut_text("F1")).clicked() {
                self.shortcuts.open = true;
                ui.close();
            }
            crate::widgets::menu::menu_button(ui, "🎓 Lessons", |ui| {
                if let Some(lesson) = self.lessons_menu(ui) {
                    actions.push(AppAction::OpenLesson(lesson));
                }
            })
            .response
            .on_hover_text("Short lessons you do in the app, from a first sound to groups");
            #[cfg(not(target_arch = "wasm32"))]
            if self.hosted.is_none() {
                self.update_menu(ui, actions);
            }
        });
    }

    /// Device selectors (engine status lives in the status bar). The
    /// browser plays through its own output, and has no MIDI or input yet
    fn device_menus(&mut self, ui: &mut egui::Ui, bar: &Bar, actions: &mut Vec<AppAction>) {
        match &self.audio.engine {
            // The DAW has the devices
            _ if self.hosted.is_some() => {}
            // The browser picks the sound devices itself; MIDI's are Soba's
            Ok(_) if WEB => {
                #[cfg(target_arch = "wasm32")]
                {
                    bar.group_break(ui);
                    self.web_midi_in(ui, actions);
                }
            }
            Ok(_) => {
                if !bar.new_row_for(ui, "Output", 200.0) {
                    bar.group_break(ui);
                }
                ui.label(RichText::new("Output").color(theme::text::SECONDARY));
                ui.add_space(8.0);
                actions.extend(output_menu(ui, &self.audio));

                // Audio input selector: None until chosen
                if !bar.new_row_for(ui, "Input", 180.0) {
                    ui.add_space(12.0);
                }
                ui.label(RichText::new("Input").color(theme::text::SECONDARY));
                ui.add_space(8.0);
                actions.extend(input_menu(ui, &self.audio));

                if !bar.new_row_for(ui, "MIDI In", 180.0) {
                    bar.group_break(ui);
                }
                ui.label(RichText::new("MIDI In").color(theme::text::SECONDARY));
                ui.add_space(8.0);
                actions.extend(midi_in_menu(ui, &self.midi, self.map_mode_on(), self.controller_map.open, self.hands_overlay.on));
            }
            Err(e) => {
                bar.group_break(ui);
                ui.label(RichText::new(format!("⚠ Audio unavailable: {}", e))
                    .color(theme::accent::ERROR));
            }
        }
    }
}

/// How the toolbar is laid out this frame: the full row, or the compact one.
struct Bar {
    ctx: egui::Context,
    compact: bool,
    /// Space either side of the line between groups.
    gap: f32,
    /// A button's height, which every row starts at.
    row_height: f32,
}

impl Bar {
    /// A shortcut as this keyboard writes it: ⌘ on a Mac.
    fn keys(&self, text: &'static str) -> String {
        shortcuts::keys(&self.ctx, text).into_owned()
    }

    fn group_break(&self, ui: &mut egui::Ui) {
        ui.add_space(self.gap);
        ui.separator();
        ui.add_space(self.gap);
    }

    fn group_label(&self, ui: &mut egui::Ui, text: &str) {
        if !self.compact {
            ui.label(RichText::new(text).color(theme::text::SECONDARY));
            ui.add_space(8.0);
        }
    }

    fn named(&self, icon: &str, name: &str) -> String {
        if self.compact { icon.to_string() } else { format!("{icon} {name}") }
    }

    /// A selector doesn't wrap by itself: it sits wherever the row ends,
    /// running off the edge. So a wrapping row starts a new line when a
    /// selector and its label won't fit, in place of the break that would
    /// lead them. Says whether it did
    fn new_row_for(&self, ui: &mut egui::Ui, label: &str, width: f32) -> bool {
        if !self.compact {
            return false;
        }
        let gap = self.gap;
        let font = egui::TextStyle::Body.resolve(ui.style());
        let label_width = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font, egui::Color32::WHITE).size().x);
        let need = 2.0 * gap + label_width + 8.0 + width + 2.0 * ui.spacing().button_padding.x + ui.spacing().icon_width + 4.0 * ui.spacing().item_spacing.x;
        if ui.available_size_before_wrap().x < need {
            ui.end_row();
            // At a button's height from the start, so the label centres
            // on the selector beside it
            ui.allocate_exact_size(egui::vec2(0.0, self.row_height), egui::Sense::hover());
            return true;
        }
        false
    }
}

/// The Output selector: the audio system, the device and the buffer size.
fn output_menu(ui: &mut egui::Ui, audio: &AudioDevices) -> Vec<AppAction> {
    let mut actions = Vec::new();
    // Get current device name for display
    let current_device = audio.outputs
        .get(audio.selected_output)
        .map(|d| d.name.as_str())
        .unwrap_or("No device");

    // Truncate long device names
    let display_name = if current_device.len() > 30 {
        format!("{}...", &current_device[..current_device.floor_char_boundary(27)])
    } else {
        current_device.to_string()
    };
    let (system, buffer_choices, buffer_frames, sample_rate) = match &audio.engine {
        Ok(engine) => (engine.audio_system(), engine.buffer_choices(), engine.buffer_frames(), engine.sample_rate()),
        Err(_) => (AudioSystem::default(), Vec::new(), None, 0),
    };
    // ASIO shows its buffer, the number a player tunes
    let selected_text = match (system, buffer_frames) {
        (AudioSystem::Asio, Some(frames)) => format!("{} · {}", display_name, frames),
        _ => display_name,
    };
    let frames_ms = |frames: u32| frames as f64 * 1000.0 / sample_rate.max(1) as f64;

    egui::ComboBox::from_id_salt("device_selector")
        .selected_text(selected_text)
        .width(200.0)
        .height(DEVICE_MENU_HEIGHT)
        .show_ui(ui, |ui| {
            let systems = AudioSystem::available();
            if systems.len() > 1 {
                ui.label(RichText::new("Audio system").color(theme::text::SECONDARY).small());
                for choice in systems {
                    let hint = match choice {
                        AudioSystem::Asio => "Your interface's own driver: a few milliseconds from input to output, for playing live",
                        AudioSystem::System => "Shared with every other app, with more delay",
                    };
                    if ui.selectable_label(choice == system, choice.label()).on_hover_text(hint).clicked() && choice != system {
                        actions.push(AppAction::SelectAudioSystem(choice));
                    }
                }
                // Steinberg's licence asks for its logo where ASIO is chosen
                #[cfg(feature = "asio")]
                super::super::asio_badge::show(ui);
                ui.separator();
            }
            for device in &audio.outputs {
                let label = if device.is_default {
                    format!("{} (Default)", device.name)
                } else {
                    device.name.clone()
                };

                if ui.selectable_label(
                    device.index == audio.selected_output,
                    label
                ).clicked() {
                    actions.push(AppAction::SelectDevice(device.index));
                }
            }

            if !buffer_choices.is_empty() {
                ui.separator();
                ui.label(RichText::new("Buffer").color(theme::text::SECONDARY).small())
                    .on_hover_text("Smaller is quicker to answer your playing, and harder work for the computer. If you hear crackles, go up a size");
                ui.horizontal(|ui| {
                    for frames in &buffer_choices {
                        let label = ui.selectable_label(buffer_frames == Some(*frames), frames.to_string())
                            .on_hover_text(format!("{:.1} ms", frames_ms(*frames)));
                        if label.clicked() && buffer_frames != Some(*frames) {
                            actions.push(AppAction::SelectBuffer(*frames));
                        }
                    }
                });
            }

            ui.separator();
            if ui.button("🔄 Refresh").clicked() {
                actions.push(AppAction::RefreshDevices);
            }
        });
    actions
}

/// The Input selector: the device Audio Input modules hear.
fn input_menu(ui: &mut egui::Ui, audio: &AudioDevices) -> Vec<AppAction> {
    let mut actions = Vec::new();
    let (input_index, input_name) = match &audio.engine {
        Ok(engine) => (engine.input_index(), engine.input_name().map(str::to_string)),
        Err(_) => (None, None),
    };
    let input_failed = audio.input.as_ref().is_some_and(InputMonitor::failed);
    let input_text = match &input_name {
        Some(name) => {
            let name = if name.len() > 22 { format!("{}...", &name[..name.floor_char_boundary(19)]) } else { name.clone() };
            if input_failed {
                RichText::new(format!("⚠ {}", name)).color(theme::accent::ERROR)
            } else {
                RichText::new(format!("● {}", name))
            }
        }
        None => RichText::new("○ None"),
    };
    egui::ComboBox::from_id_salt("input_device_selector")
        .selected_text(input_text)
        .width(180.0)
        .height(DEVICE_MENU_HEIGHT)
        .show_ui(ui, |ui| {
            if ui.selectable_label(input_index.is_none(), "None").clicked() {
                actions.push(AppAction::SelectInput(None));
            }
            ui.separator();
            if audio.inputs.is_empty() {
                ui.label(RichText::new("No input devices found")
                    .color(theme::text::DISABLED)
                    .italics());
            }
            for device in &audio.inputs {
                let label = if device.is_default {
                    format!("{} (Default)", device.name)
                } else {
                    device.name.clone()
                };
                if ui.selectable_label(input_index == Some(device.index), label).clicked() {
                    actions.push(AppAction::SelectInput(Some(device.index)));
                }
            }
            ui.separator();
            if ui.button("🔄 Refresh").clicked() {
                actions.push(AppAction::RefreshInputDevices);
            }
        })
        .response
        .on_hover_text("Audio Input modules hear this device: a microphone, guitar or line in");
    actions
}

/// The MIDI In selector: the device that plays Note In modules and MIDI Learn.
fn midi_in_menu(ui: &mut egui::Ui, midi: &MidiSession, map_mode: bool, controller_map: bool, overlay: bool) -> Vec<AppAction> {
    let mut actions = Vec::new();
    // Get current MIDI device name for display
    let current_midi = midi.selected_device()
        .map(|d| d.name.as_str())
        .unwrap_or("None");

    // Truncate long device names
    let midi_display_name = if current_midi.len() > 25 {
        format!("{}...", &current_midi[..22])
    } else {
        current_midi.to_string()
    };

    // MIDI connection indicator
    let midi_indicator = if midi.is_connected() { "● " } else { "○ " };

    egui::ComboBox::from_id_salt("midi_device_selector")
        .selected_text(format!("{}{}", midi_indicator, midi_display_name))
        .width(180.0)
        .height(DEVICE_MENU_HEIGHT)
        .show_ui(ui, |ui| {
            // Option to disconnect / select none
            if ui.selectable_label(
                midi.selected.is_none(),
                "None (Disconnect)"
            ).clicked() {
                actions.push(AppAction::DisconnectMidi);
            }

            ui.separator();

            // List available MIDI devices
            if midi.devices.is_empty() {
                ui.label(RichText::new("No MIDI devices found")
                    .color(theme::text::DISABLED)
                    .italics());
            } else {
                for device in &midi.devices {
                    let is_selected = midi.selected == Some(device.index);
                    if ui.selectable_label(is_selected, &device.name).clicked() {
                        actions.push(AppAction::ConnectMidi(device.index));
                    }
                }
            }

            ui.separator();
            if ui.button("🔄 Refresh").clicked() {
                actions.push(AppAction::RefreshMidiDevices);
            }

            // Mapping a controller, a control at a time, and the map of it (#426)
            ui.separator();
            let keys = shortcuts::keys(ui.ctx(), "Ctrl+L");
            let item = egui::Button::selectable(map_mode, "Map Mode").shortcut_text(keys);
            let tip = "Click a control, then move a knob or fader or hit a pad on your controller to map it; then the next. Esc leaves";
            if ui.add(item).on_hover_text(tip).clicked() {
                actions.push(AppAction::ToggleMapMode);
            }
            let item = egui::Button::selectable(controller_map, "Controller Map…");
            if ui.add(item).on_hover_text("Every MIDI mapping in the patch: its range, hardware, pickup and invert").clicked() {
                actions.push(AppAction::ToggleControllerMap);
            }

            // The encoders and faders following the selection (#429)
            ui.separator();
            let taught = midi.hands.controller();
            let teach = if taught.is_some() { "Teach Your Controller Again…" } else { "Teach Your Controller…" };
            let tip = match taught {
                Some(controller) => format!("Soba knows {} on this controller. Turn each encoder, then move each fader, to teach it again", controller.summary()),
                None => "Turn each encoder, then move each fader, so Soba knows them. Once per controller".to_string(),
            };
            if ui.button(teach).on_hover_text(tip).clicked() {
                actions.push(AppAction::TeachController);
            }
            let keys = shortcuts::keys(ui.ctx(), "Ctrl+Shift+L");
            let item = egui::Button::selectable(midi.hands.on, "Hands Follow Selection").shortcut_text(keys);
            let tip = "The encoders turn the selected module's first eight knobs, and the faders the next four. The patch's own mappings on them pause meanwhile";
            if ui.add_enabled(taught.is_some(), item).on_hover_text(tip).on_disabled_hover_text("Teach Soba your controller first").clicked() {
                actions.push(AppAction::ToggleHands);
            }
            // A sketch of the controller, saying what each control does (#431)
            let keys = shortcuts::keys(ui.ctx(), "Ctrl+H");
            let item = egui::Button::selectable(overlay, "Show the Controller").shortcut_text(keys);
            let tip = "A sketch of the controller along the foot of the canvas: each encoder, fader and pad, and what it does right now";
            if ui.add(item).on_hover_text(tip).clicked() {
                actions.push(AppAction::ToggleHandsOverlay);
            }
        });
    actions
}
