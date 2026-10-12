//! Main application struct for the Soba
//!
//! Contains the SynthApp which implements eframe::App and manages
//! the synthesizer's UI state, audio engine, and graph state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use web_time::Instant;

use eframe::egui::{self, RichText, Layout, Align};
use egui_node_graph2::{FlowGlyph, GraphEditorState, NodeResponse, NodeTemplateTrait, InputParamKind};

use crate::engine::{
    AudioEngine, AudioError, AudioProcessor, AudioSystem, DeviceInfo, EngineChannels, EngineCommand, InputMonitor, Recording, RoundTrip,
    UiHandle, MidiDeviceInfo, MidiEngine, MidiEvent, MidiReceivers, TimestampedMidiEvent,
};
use rtrb::Consumer;
use crate::graph::annotation_ui;
use crate::graph::annotations::{Annotation, AnnotationId, Frame, Note, Tint, DEFAULT_NOTE_WIDTH};
use crate::graph::{
    port_mapping, validate_connection, AllNodeTemplates, AnyParameterId, GroupId, SynthDataType, SynthGraphState,
    SynthNodeData, SynthNodeTemplate, SynthValueType,
};
use crate::modules::keyboard::{key_to_note, relative_to_midi, KeyPriority, KeyboardInput};
use crate::persistence::{
    capture_patch, examples, load_from_file, renumber_groups, save_to_file, stage_patch, Example, MidiMapping, Patch, PatchError,
    Section, EXAMPLES,
};
#[cfg(target_arch = "wasm32")]
use crate::persistence::{patch_from_json, patch_to_json};
use crate::persistence::sample_files::SampleBase;
use crate::widgets::{cpu_meter, CpuMeterConfig, KnobStyle};
use super::capture::{Capture, CaptureAction, CaptureConfig};
use super::editing::{self, Selection};
use super::engine_sync;
use super::input_device;
use super::library::{self, SavedModule};
use super::palette::{PaletteAction, QuickAdd};
use super::recording::{self, RecState, Toast, ToastAction};
use super::session::{self, Answer, Autosave, Discard, RecentFiles};
use super::theme;
use super::undo::{Applied, History};
#[cfg(not(target_arch = "wasm32"))]
use super::update::Updater;
use super::WEB;
#[cfg(target_arch = "wasm32")]
use super::web;

mod grouping;
mod loops;
mod samples;
#[cfg(not(target_arch = "wasm32"))]
mod updates;

/// Type alias for our graph editor state
type SynthGraphEditorState = GraphEditorState<SynthNodeData, SynthDataType, SynthValueType, SynthNodeTemplate, SynthGraphState>;

/// Max popup height for the toolbar device dropdowns. egui's default (200px)
/// fits only ~3 rows at the theme's padding; egui still clamps to the window.
const DEVICE_MENU_HEIGHT: f32 = 480.0;

/// How long the audio callback can go quiet before the output is shown as
/// stalled. Devices ask for audio every few milliseconds.
const AUDIO_STALL: std::time::Duration = std::time::Duration::from_millis(750);

/// Storage key for the mark drawn along cables (Cables menu)
const FLOW_GLYPH_KEY: &str = "cable_flow_glyph";

/// Storage key for how knobs are drawn (Knobs menu)
const KNOB_STYLE_KEY: &str = "knob_style";
/// Where the audio system chosen under Output is remembered.
const AUDIO_SYSTEM_KEY: &str = "audio_system";
/// Where the buffer size chosen under Output is remembered, in frames
/// (empty: the driver's own).
const BUFFER_FRAMES_KEY: &str = "buffer_frames";

/// How long a stopped recording waits for the audio thread to hand its tap
/// back before the file is finished without it (the device has gone quiet).
const RECORDING_PATIENCE: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a warning stays in the status bar, unless clicked away.
const NOTICE_SECONDS: f64 = 12.0;

/// The manual, for the Help menu.
const MANUAL_URL: &str = "https://docs.chaps.dev/soba/";

/// How long the input's status stays amber after a dropout.
const INPUT_GLITCH_HOLD: std::time::Duration = std::time::Duration::from_secs(3);

/// Target parameter for MIDI Learn mode.
///
/// When the user activates MIDI Learn on a knob, this stores the target
/// parameter information until a CC event is received.
#[derive(Debug, Clone)]
pub struct MidiLearnTarget {
    /// Engine node ID of the target parameter.
    pub node_id: u64,
    /// Parameter index within the node.
    pub param_index: usize,
    /// Parameter name for display.
    pub param_name: String,
    /// Minimum value of the parameter range.
    pub min_value: f32,
    /// Maximum value of the parameter range.
    pub max_value: f32,
}

/// Main application state for the Soba
pub struct SynthApp {
    /// Audio engine handle
    audio_engine: Result<AudioEngine, AudioError>,

    /// UI-side handle for communicating with audio engine
    ui_handle: Option<UiHandle>,

    /// Last audio error message to display
    audio_error_message: Option<String>,

    /// Whether the transport is "playing" (audio graph processing active)
    is_playing: bool,

    /// Whether theme has been applied
    theme_applied: bool,

    /// Node graph editor state
    graph_state: SynthGraphEditorState,

    /// User state for the graph editor
    user_state: SynthGraphState,

    /// Cached list of audio devices
    audio_devices: Vec<DeviceInfo>,

    /// Index of currently selected device
    selected_device_index: usize,

    /// Cached parameter values for change detection.
    /// Key is (node_id as u64, param_index), value is the last sent value.
    cached_params: HashMap<(u64, usize), f32>,

    /// Current patch file path (None if unsaved/new).
    current_patch_path: Option<PathBuf>,
    /// The example the graph was opened from, until it's saved as a file or replaced.
    current_example: Option<&'static Example>,

    /// Status message for save/load operations (auto-clears after display).
    status_message: Option<String>,

    /// Problems skipped while loading the current patch, shown in the status bar.
    load_warnings: Vec<String>,

    /// Currently pressed keyboard keys for virtual keyboard.
    /// Stores (relative_note, egui::Key) in order of press for key priority.
    pressed_keys: Vec<(i32, Option<egui::Key>)>,

    /// Timestamp when the gate was last triggered (for minimum gate duration).
    last_gate_on: Option<Instant>,

    /// Whether the gate is currently being held high (for minimum duration).
    gate_held_high: bool,

    /// The engine node a chord popover is auditioning a chord on, so it
    /// can be stopped if the popover goes without lifting it.
    auditioning: Option<u64>,

    /// Current CPU load percentage from the audio engine (0-100).
    cpu_load: f32,

    /// How wide the toolbar is with every button named, measured the last
    /// time it was drawn that way. A narrower window gets the compact row.
    toolbar_full_width: Option<f32>,

    /// The audio callback count last seen, and when it last moved: a count
    /// that stops moving means the device has stopped taking audio.
    audio_heartbeat: (u64, Instant),

    /// MIDI engine for receiving MIDI input.
    midi_engine: Option<MidiEngine>,

    /// Consumer for the UI's copy of incoming MIDI: monitor display, piano,
    /// MIDI Learn and CC mappings. Notes reach MIDI Note modules on the
    /// audio thread, through the processor's own queue.
    midi_event_consumer: Option<Consumer<TimestampedMidiEvent>>,

    /// Cached list of MIDI input devices.
    midi_devices: Vec<MidiDeviceInfo>,

    /// Index of currently selected MIDI device (None = no device).
    selected_midi_device: Option<usize>,

    /// MIDI error message to display.
    midi_error_message: Option<String>,

    /// MIDI notes currently held, in order of press, each with its velocity:
    /// for the piano display and the chord popover.
    midi_held_notes: Vec<(u8, u8)>,

    // --- MIDI CC Mapping state ---
    /// Active MIDI CC to parameter mappings.
    midi_mappings: Vec<MidiMapping>,

    /// Target for MIDI Learn mode (None = not learning).
    midi_learn_target: Option<MidiLearnTarget>,

    /// Undo and redo for edits to the patch.
    history: History,

    /// The quick-add palette, while it's open.
    quick_add: Option<QuickAdd>,

    /// Where the graph editor was drawn last frame, in screen points.
    editor_rect: egui::Rect,

    /// Where the last paste was aimed and where it landed, so pasting again
    /// without moving the mouse fans the copies out instead of stacking them.
    last_paste: Option<(egui::Pos2, egui::Pos2)>,

    // --- Session safety ---
    /// Patch files opened or saved lately, for the Recent menu.
    recent_files: RecentFiles,
    /// What's waiting on an answer to "Save changes?".
    pending_discard: Option<Discard>,
    /// A crash's autosave, until it's recovered or let go.
    recovery: Option<Autosave>,
    /// The MIDI mappings as last opened or saved. Undo history doesn't
    /// cover them, but they're saved with the patch.
    saved_midi_mappings: Vec<MidiMapping>,
    /// Whether the window may close: set once unsaved changes are dealt with.
    allow_close: bool,
    /// The window title last sent, so it's only sent when it changes.
    window_title: String,
    /// Filming the app with `--capture`: the script, clock and outputs.
    capture: Option<Capture>,

    // --- Recording ---
    /// The take being recorded, or being finished after Stop.
    recording: Option<Recording>,
    /// Where takes are written, if not the default Music/Soba.
    recordings_folder: Option<PathBuf>,
    /// The note about the last finished take.
    record_toast: Option<Toast>,

    // --- Loops ---
    /// Each Looper's loop as the patch last saved it, by engine node.
    /// Loopers not here hold what they were opened with.
    kept_loops: HashMap<crate::engine::NodeId, loops::KeptLoop>,
    /// Loop files the patch opened or wrote, which a save may remove once
    /// no Looper names them.
    loop_files: std::collections::HashSet<String>,

    // --- Updates ---
    /// The check for new versions, and installing them.
    #[cfg(not(target_arch = "wasm32"))]
    updater: Updater,
    /// Restarting into another version: the autosave is kept on the way out.
    resuming: bool,
    /// A view to put back (zoom, and the patch point at the middle), once
    /// the editor has been laid out.
    pending_view: Option<(f32, egui::Pos2)>,

    // --- Audio input ---
    /// Input devices, for the Input menu.
    input_devices: Vec<DeviceInfo>,
    /// The open input's counters and health, while one is open.
    input_monitor: Option<InputMonitor>,
    /// The input's dropouts so far (underruns plus overflows, in frames),
    /// and when that last grew.
    input_glitches: (u64, Instant),
    /// Whether this session has warned about the input hearing the speakers.
    feedback_warned: bool,
    /// A warning for the status bar, and when it was raised (UI clock).
    notice: Option<(String, f64)>,

    // --- Groups ---
    /// The groups the view went into, innermost last, each with where its
    /// node was on screen (relative to the editor), to come back out to.
    level_trail: Vec<(GroupId, egui::Vec2)>,
    /// A group just made, while it's being named: naming it names the step.
    naming_group: Option<GroupId>,
    /// Tab opened a group, and egui's focus moved on with it to a button.
    release_tab_focus: bool,
    /// The groups saved to My Modules, as of the last look.
    my_modules: Vec<SavedModule>,
    /// The Library's groups, which ship with the app.
    library_modules: Vec<SavedModule>,
    /// How big each node was last drawn, unzoomed.
    node_sizes: HashMap<egui_node_graph2::NodeId, egui::Vec2>,
    /// The level was framed before some of its nodes had been drawn.
    reframe_level: bool,

    // --- Browser ---
    /// Embedded in a page (`?patch=` in the address): just the canvas, with
    /// a Play button and a way to the full app.
    embedded: bool,
    /// Frames left to keep zooming a just-opened patch to fit the view, as
    /// its modules are drawn and measured; 0 when no fit is under way. Patches
    /// opened in the browser fit, where a phone shows only a corner of one.
    fit_pending: u8,
    /// A finger is dragging the canvas: on a touch screen that pans, where a
    /// mouse would draw a selection box.
    touch_panning: bool,
    /// Patch files the visitor picked, which the browser hands over later.
    #[cfg(target_arch = "wasm32")]
    uploads: (std::sync::mpsc::Sender<web::Upload>, std::sync::mpsc::Receiver<web::Upload>),
}

/// What a module's right-click menu asked for, handled once the graph is drawn.
/// What the canvas menu's annotation items add.
#[derive(Clone, Copy)]
enum NewAnnotation {
    Frame,
    Note,
}

/// Space left between a new frame and the modules it's drawn around, in
/// patch points. The title band goes above that.
const FRAME_PADDING: f32 = 24.0;

/// A new empty frame's size, in patch points.
const NEW_FRAME_SIZE: egui::Vec2 = egui::Vec2::new(380.0, 260.0);

enum NodeMenuAction {
    Select(egui_node_graph2::NodeId),
    Duplicate(egui_node_graph2::NodeId),
    Copy(egui_node_graph2::NodeId),
    Reset(egui_node_graph2::NodeId),
    Delete(egui_node_graph2::NodeId),
    Group(egui_node_graph2::NodeId),
    Ungroup(egui_node_graph2::NodeId),
    Enter(egui_node_graph2::NodeId),
    StartRename(egui_node_graph2::NodeId),
    Rename(egui_node_graph2::NodeId, String),
    Pin(egui_node_graph2::NodeId, String, u8),
    NameFace(egui_node_graph2::NodeId, String, String),
    /// A port's menu chose what to do with groups' jacks.
    Jack(egui_node_graph2::NodeId, crate::graph::groups::Side, String, crate::graph::groups::JackEdit),
    SaveToLibrary(egui_node_graph2::NodeId),
    OpenSample(egui_node_graph2::NodeId),
}

impl SynthApp {
    /// Create a new SynthApp instance
    ///
    /// If `enable_test_tone` is true, audio will start with a test tone immediately.
    pub fn new(enable_test_tone: bool) -> Self {
        let mut audio_engine = AudioEngine::new();

        let audio_error_message = match &audio_engine {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };

        // Get initial device list and find the default device index
        let (audio_devices, selected_device_index) = match &audio_engine {
            Ok(engine) => {
                let devices = engine.enumerate_devices();
                let default_idx = devices.iter()
                    .position(|d| d.is_default)
                    .unwrap_or(0);
                (devices, default_idx)
            }
            Err(_) => (Vec::new(), 0),
        };
        // Listed, not opened: nothing opens the microphone unasked
        let input_devices = audio_engine.as_ref().map(|e| e.enumerate_input_devices()).unwrap_or_default();

        // Initialize MIDI engine
        let (midi_engine, midi_receivers, midi_devices, midi_error_message) =
            match MidiEngine::new() {
                Ok((mut engine, receivers)) => {
                    let devices = engine.enumerate_devices();
                    (Some(engine), Some(receivers), devices, None)
                }
                Err(e) => {
                    eprintln!("MIDI initialization failed: {}", e);
                    (None, None, Vec::new(), Some(e.to_string()))
                }
            };
        let (midi_audio_consumer, midi_event_consumer) = match midi_receivers {
            Some(MidiReceivers { audio, ui }) => (Some(audio), Some(ui)),
            None => (None, None),
        };

        // Create engine channels for communication with audio thread
        let channels = EngineChannels::with_defaults();
        let (ui_handle, engine_handle) = channels.split();

        // Create and start the audio processor if engine is available
        let ui_handle = if let Ok(ref mut engine) = audio_engine {
            let sample_rate = engine.sample_rate() as f32;
            let block_size = 256; // Standard block size
            let mut processor = AudioProcessor::new(sample_rate, block_size, engine_handle);
            if let Some(midi) = midi_audio_consumer {
                processor.set_midi_input(midi);
            }

            if let Err(e) = engine.start_with_processor(processor) {
                eprintln!("Failed to start audio processor: {}", e);
            }

            Some(ui_handle)
        } else {
            // Drop the engine_handle since we can't use it
            drop(engine_handle);
            None
        };

        let app = Self {
            audio_engine,
            ui_handle,
            audio_error_message,
            is_playing: false,
            theme_applied: false,
            graph_state: GraphEditorState::new(1.0),
            user_state: SynthGraphState::default(),
            audio_devices,
            selected_device_index,
            cached_params: HashMap::new(),
            current_patch_path: None,
            current_example: None,
            status_message: None,
            load_warnings: Vec::new(),
            pressed_keys: Vec::new(),
            last_gate_on: None,
            gate_held_high: false,
            auditioning: None,
            cpu_load: 0.0,
            toolbar_full_width: None,
            audio_heartbeat: (0, Instant::now()),
            midi_engine,
            midi_event_consumer,
            midi_devices,
            selected_midi_device: None,
            midi_error_message,
            midi_held_notes: Vec::new(),
            // MIDI CC Mapping state
            midi_mappings: Vec::new(),
            midi_learn_target: None,
            history: History::default(),
            quick_add: None,
            editor_rect: egui::Rect::NOTHING,
            last_paste: None,
            recent_files: RecentFiles::default(),
            pending_discard: None,
            recovery: None,
            saved_midi_mappings: Vec::new(),
            allow_close: false,
            window_title: String::new(),
            capture: None,
            recording: None,
            recordings_folder: None,
            record_toast: None,
            kept_loops: HashMap::new(),
            loop_files: std::collections::HashSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            updater: Updater::new(),
            resuming: false,
            pending_view: None,
            input_devices,
            input_monitor: None,
            input_glitches: (0, Instant::now()),
            feedback_warned: false,
            notice: None,
            level_trail: Vec::new(),
            naming_group: None,
            release_tab_focus: false,
            my_modules: Vec::new(),
            library_modules: library::library(),
            node_sizes: HashMap::new(),
            reframe_level: false,
            embedded: false,
            fit_pending: 0,
            touch_panning: false,
            #[cfg(target_arch = "wasm32")]
            uploads: std::sync::mpsc::channel(),
        };

        // Note: enable_test_tone is ignored - test tone was removed in favor of AudioProcessor
        let _ = enable_test_tone;

        app
    }

    /// Hands the audio engine's clock to a capture: from here on audio is
    /// rendered one video frame at a time, in step with the picture.
    pub fn start_capture(&mut self, config: CaptureConfig) -> Result<(), String> {
        let engine = self.audio_engine.as_mut().map_err(|e| e.to_string())?;
        engine.go_offline(config.sample_rate as f32).map_err(|e| e.to_string())?;
        self.capture = Some(Capture::start(config)?);
        Ok(())
    }

    /// Runs the capture's cues for a new frame, then renders the audio the
    /// frame covers, so this frame's picture shows what it sounds like.
    fn step_capture(&mut self, ctx: &egui::Context) {
        let Some(capture) = self.capture.as_mut() else { return };
        if !capture.is_fresh() {
            return;
        }
        let mut actions = capture.take_actions();
        let graph = &self.graph_state.graph;
        actions.extend(capture.param_values(|module, nth, input| {
            find_input(graph, module, nth, input).map(|id| graph.get_input(id).value.actual_value())
        }));

        let mut midi = Vec::new();
        for action in actions {
            match action {
                CaptureAction::Play(on) => {
                    if on != self.is_playing {
                        self.is_playing = on;
                        self.user_state.is_playing = on;
                        self.send_command(EngineCommand::SetPlaying(on));
                    }
                }
                CaptureAction::Midi { event, offset } => {
                    // The UI's copy lights the pianos; the audio's copy is
                    // placed by sample below instead
                    if let Some(engine) = self.midi_engine.as_ref() {
                        engine.send(event);
                    }
                    midi.extend(event.to_dsp(offset));
                }
                CaptureAction::InputFile(name) => self.user_state.audio_input_name = Some(name),
                CaptureAction::Layout(path) => self.write_layout(ctx, &path),
                CaptureAction::SetParam { module, nth, input, value } => {
                    match find_input(&self.graph_state.graph, &module, nth, &input) {
                        Some(id) => self.graph_state.graph.inputs[id].value.set_actual_value(value),
                        None => eprintln!("capture: no input {} on {} #{}", input, module, nth + 1),
                    }
                }
            }
        }

        // Ship the frame's edits before rendering it
        self.sync_parameters();
        self.sync_samples();
        if let Some(handle) = self.ui_handle.as_mut() {
            handle.flush();
        }
        midi.sort_by_key(|e| e.sample_offset);
        let (Ok(engine), Some(capture)) = (self.audio_engine.as_ref(), self.capture.as_mut()) else { return };
        let (buffer, input) = capture.audio_and_input();
        engine.render_offline(buffer, 2, &mut midi, input);
        capture.commit_audio();
    }

    /// Writes where each module, frame and note was last drawn, in patch
    /// space, for a capture's `layout` cue: what frames around modules need
    /// to clear, and whether a module covers a note.
    fn write_layout(&self, ctx: &egui::Context, path: &Path) {
        let zoom = self.graph_state.pan_zoom.zoom;
        let to_patch = |screen: egui::Pos2| self.history.to_patch(self.screen_to_node(screen), zoom);
        let patch_rect = |r: egui::Rect| {
            let (min, max) = (to_patch(r.min), to_patch(r.max));
            [min.x, min.y, max.x, max.y]
        };
        let modules: Vec<serde_json::Value> = self.graph_state.node_order.iter().filter_map(|&node_id| {
            let rect = annotation_ui::module_rect(ctx, node_id)?;
            let position = self.history.to_patch(*self.graph_state.node_positions.get(node_id)?, zoom);
            Some(serde_json::json!({
                "module_id": self.graph_state.graph.nodes.get(node_id)?.user_data.module_id,
                "position": [position.x, position.y],
                "rect": patch_rect(rect),
            }))
        }).collect();
        let annotations = &self.user_state.annotations;
        let (mut frames, mut notes) = (Vec::new(), Vec::new());
        for (id, annotation) in annotations.iter() {
            let Some(&rect) = annotations.drawn.get(&id) else { continue };
            match annotation {
                Annotation::Frame(frame) => frames.push(serde_json::json!({ "title": frame.title, "rect": patch_rect(rect) })),
                Annotation::Note(note) => notes.push(serde_json::json!({ "text": note.text, "rect": patch_rect(rect) })),
            }
        }
        let layout = serde_json::json!({ "modules": modules, "frames": frames, "notes": notes });
        let json = serde_json::to_string_pretty(&layout).unwrap_or_default();
        if let Err(e) = std::fs::write(path, json) {
            eprintln!("capture: couldn't write {}: {}", path.display(), e);
        }
    }

    /// Refresh the list of available audio devices
    fn refresh_devices(&mut self) {
        if let Ok(ref engine) = self.audio_engine {
            self.audio_devices = engine.enumerate_devices();
        }
    }

    /// Refresh the list of input devices.
    fn refresh_input_devices(&mut self) {
        if let Ok(ref engine) = self.audio_engine {
            self.input_devices = engine.enumerate_input_devices();
        }
    }

    /// Opens input device `index` and connects it to the patch's Audio
    /// Input modules, or with `None` closes the input.
    fn select_input(&mut self, index: Option<usize>) {
        let Ok(engine) = self.audio_engine.as_mut() else { return };
        let opened = index.map(|index| engine.open_input(index));
        let input_name = engine.input_name().map(str::to_string);
        if index.is_none() {
            engine.close_input();
        }
        match opened {
            Some(Ok((feed, monitor))) => {
                if let Some(handle) = self.ui_handle.as_mut() {
                    handle.connect_input(feed);
                }
                self.input_glitches = (0, Instant::now());
                self.input_monitor = Some(monitor);
                self.user_state.audio_input_name = input_name;
                self.warn_about_feedback();
            }
            failed => {
                if let Some(handle) = self.ui_handle.as_mut() {
                    handle.disconnect_input();
                }
                self.input_monitor = None;
                self.user_state.audio_input_name = None;
                if let Some(Err(e)) = failed {
                    self.raise_notice(format!("Can't open the input: {}", e));
                }
            }
        }
    }

    /// The first time an input is live while the output is speakers, says
    /// once that it may feed back.
    fn warn_about_feedback(&mut self) {
        if self.feedback_warned || self.input_monitor.is_none() {
            return;
        }
        let Some(output) = self.audio_devices.get(self.selected_device_index).map(|d| d.name.clone()) else { return };
        if input_device::looks_like_headphones(&output) {
            return;
        }
        self.feedback_warned = true;
        self.raise_notice(input_device::feedback_warning(&output));
    }

    /// Puts a warning in the status bar for a while.
    fn raise_notice(&mut self, text: String) {
        // Stamped on the next frame, when the UI clock is at hand
        self.notice = Some((text, f64::NAN));
    }

    /// Refresh the list of available MIDI devices
    fn refresh_midi_devices(&mut self) {
        if let Some(ref mut engine) = self.midi_engine {
            self.midi_devices = engine.enumerate_devices();
        }
    }

    /// Connect to a MIDI device by index
    fn connect_midi_device(&mut self, index: usize) {
        if let Some(ref mut engine) = self.midi_engine {
            match engine.connect(index) {
                Ok(()) => {
                    self.selected_midi_device = Some(index);
                    self.midi_error_message = None;
                }
                Err(e) => {
                    // connect() drops the previous connection before trying
                    self.selected_midi_device = None;
                    self.midi_error_message = Some(e.to_string());
                }
            }
            // Notes held on the previous device were released (All Notes Off)
            self.clear_midi_held_notes();
        }
    }

    /// Disconnect from the current MIDI device
    fn disconnect_midi_device(&mut self) {
        if let Some(ref mut engine) = self.midi_engine {
            engine.disconnect();
            self.selected_midi_device = None;
            self.midi_error_message = None;
            self.clear_midi_held_notes();
        }
    }

    /// Clears the piano display's held notes.
    fn clear_midi_held_notes(&mut self) {
        self.midi_held_notes.clear();
        self.user_state.set_midi_active_notes(&[]);
    }

    /// Process the UI's copy of pending MIDI events.
    /// - Stores events in user state for display by MIDI Monitor modules.
    /// - Tracks held notes for the MIDI piano display.
    /// - Handles CC events for MIDI Learn and mapped parameters.
    ///
    /// Notes reach MIDI Note modules on the audio thread, sample-accurately;
    /// nothing here affects what they play.
    fn process_midi_events(&mut self) {
        let mut notes_changed = false;
        let mut cc_updates: Vec<(u64, usize, f32)> = Vec::new();
        let mut learned_mapping: Option<MidiMapping> = None;

        if let Some(ref mut consumer) = self.midi_event_consumer {
            while let Ok(timestamped) = consumer.pop() {
                let event = timestamped.event;

                // Store the event for MIDI Monitor display
                self.user_state.push_midi_event(event);

                match event {
                    MidiEvent::NoteOn { note, velocity, .. } => {
                        if !self.midi_held_notes.iter().any(|&(n, _)| n == note) {
                            self.midi_held_notes.push((note, velocity));
                            notes_changed = true;
                        }
                    }
                    MidiEvent::NoteOff { note, .. } => {
                        if let Some(pos) = self.midi_held_notes.iter().position(|&(n, _)| n == note) {
                            self.midi_held_notes.remove(pos);
                            notes_changed = true;
                        }
                    }
                    MidiEvent::ControlChange { channel, controller, value } => {
                        // Check if we're in MIDI Learn mode
                        if let Some(ref target) = self.midi_learn_target {
                            // Create new mapping from the received CC
                            learned_mapping = Some(MidiMapping::new(
                                controller,
                                0, // Omni channel for learned mappings
                                target.node_id,
                                target.param_index,
                                target.param_name.clone(),
                                target.min_value,
                                target.max_value,
                            ));
                        } else {
                            // Apply CC to all matching mappings
                            for mapping in &self.midi_mappings {
                                if mapping.matches(controller, channel) {
                                    let param_value = mapping.cc_to_value(value);
                                    cc_updates.push((mapping.node_id, mapping.param_index, param_value));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Handle MIDI Learn completion
        if let Some(mapping) = learned_mapping {
            // Remove any existing mapping for the same parameter (from user_state too)
            self.midi_mappings.retain(|m| {
                !(m.node_id == mapping.node_id && m.param_index == mapping.param_index)
            });
            self.user_state.remove_midi_mapping(mapping.node_id, mapping.param_index);

            // Also remove any existing mapping for the same CC
            self.midi_mappings.retain(|m| {
                !(m.cc_number == mapping.cc_number && (m.channel == 0 || m.channel == mapping.channel))
            });

            // Add the new mapping
            self.user_state.set_midi_mapping(
                mapping.node_id,
                mapping.param_index,
                mapping.cc_number,
                mapping.channel,
            );
            self.midi_mappings.push(mapping);

            // Exit learn mode
            self.midi_learn_target = None;
            self.user_state.midi_learn_active = false;
            self.user_state.midi_learn_target = None;
            self.status_message = Some("MIDI CC mapped successfully".to_string());
        }

        // Apply CC updates to parameters
        for (node_id, param_index, value) in cc_updates {
            self.send_command(EngineCommand::SetParameter {
                node_id,
                param_index,
                value,
            });
            // Update cached param so sync_parameters doesn't overwrite
            self.cached_params.insert((node_id, param_index), value);

            // Also update the graph UI to reflect the change
            self.update_graph_param_from_cc(node_id, param_index, value);
        }

        if notes_changed {
            self.user_state.set_midi_active_notes(&self.midi_held_notes);
        }
    }

    /// Plays a parameter from the node, as a MIDI controller would: sent at
    /// once, and not an undo step (a Looper's footswitch).
    fn play_parameter(&mut self, node_id: egui_node_graph2::NodeId, param_name: &str, value: f32) {
        let Some(engine_node_id) = self.user_state.get_engine_node_id(node_id) else { return };
        let Some(module_id) = self.graph_state.graph.nodes.get(node_id).map(|n| n.user_data.module_id) else { return };
        let Some(param_index) = crate::graph::SynthNodeTemplate::from_module_id(module_id)
            .and_then(|template| template.parameter_names().iter().position(|name| name == param_name))
        else {
            return;
        };
        self.send_command(EngineCommand::SetParameter { node_id: engine_node_id, param_index, value });
        self.cached_params.insert((engine_node_id, param_index), value);
        self.update_graph_param_from_cc(engine_node_id, param_index, value);
    }

    /// Plays a chord on a node's outputs from its face, or stops it.
    fn audition(&mut self, node_id: egui_node_graph2::NodeId, chord: Option<crate::dsp::Audition>) {
        let Some(engine_node_id) = self.user_state.get_engine_node_id(node_id) else { return };
        if let Some(other) = self.auditioning.filter(|&other| other != engine_node_id) {
            self.send_command(EngineCommand::Audition { node_id: other, chord: None });
        }
        self.send_command(EngineCommand::Audition { node_id: engine_node_id, chord });
        self.auditioning = chord.map(|_| engine_node_id);
    }

    /// Update a graph parameter value from a CC change.
    fn update_graph_param_from_cc(&mut self, engine_node_id: u64, param_index: usize, value: f32) {
        // Find the graph node ID for this engine node
        let graph_node_id = self.user_state.node_id_map.iter()
            .find(|(_, &engine_id)| engine_id == engine_node_id)
            .map(|(graph_id, _)| *graph_id);

        if let Some(graph_node_id) = graph_node_id {
            if let Some(node) = self.graph_state.graph.nodes.get_mut(graph_node_id) {
                // Find the parameter by index
                let mut current_param_index = 0;
                for (_name, input_id) in &node.inputs {
                    if let Some(input) = self.graph_state.graph.inputs.get_mut(*input_id) {
                        match input.kind {
                            InputParamKind::ConstantOnly | InputParamKind::ConnectionOrConstant => {
                                if current_param_index == param_index {
                                    input.value.set_actual_value(value);
                                    // Playing a controller isn't an edit to undo
                                    self.history.absorb_param(engine_node_id, param_index, input.value.actual_value());
                                    return;
                                }
                                current_param_index += 1;
                            }
                            InputParamKind::ConnectionOnly => {
                                // Skip connection-only inputs
                            }
                        }
                    }
                }
            }
        }
    }

    /// Start MIDI Learn mode for a parameter.
    ///
    /// Starting on another knob while learning moves learn mode to that knob.
    pub fn start_midi_learn(&mut self, target: MidiLearnTarget) {
        self.status_message = Some(format!("Move a MIDI CC to map {}... (Esc to cancel)", target.param_name));
        self.user_state.midi_learn_active = true;
        self.user_state.midi_learn_target = Some((target.node_id, target.param_index));
        self.midi_learn_target = Some(target);
    }

    /// Cancel MIDI Learn mode.
    pub fn cancel_midi_learn(&mut self) {
        self.midi_learn_target = None;
        self.user_state.midi_learn_active = false;
        self.user_state.midi_learn_target = None;
        self.status_message = Some("MIDI Learn cancelled".to_string());
    }

    /// Check if currently in MIDI Learn mode.
    pub fn is_midi_learning(&self) -> bool {
        self.midi_learn_target.is_some()
    }

    /// Get the MIDI mapping for a specific parameter, if any.
    pub fn get_mapping_for_param(&self, node_id: u64, param_index: usize) -> Option<&MidiMapping> {
        self.midi_mappings.iter()
            .find(|m| m.node_id == node_id && m.param_index == param_index)
    }

    /// Remove MIDI mapping for a specific parameter.
    pub fn clear_mapping_for_param(&mut self, node_id: u64, param_index: usize) {
        self.midi_mappings.retain(|m| {
            !(m.node_id == node_id && m.param_index == param_index)
        });
        self.status_message = Some("MIDI mapping cleared".to_string());
    }

    /// Clear all MIDI mappings.
    pub fn clear_all_midi_mappings(&mut self) {
        self.midi_mappings.clear();
        self.status_message = Some("All MIDI mappings cleared".to_string());
    }

    /// Check if there are any MIDI Note or Poly MIDI modules in the graph.
    fn has_midi_note_modules(&self) -> bool {
        self.graph_state.graph.nodes.iter()
            .any(|(_, node)| matches!(node.user_data.module_id, "input.midi_note" | "input.poly_midi"))
    }

    /// Check if there are any Poly MIDI modules in the graph.
    fn has_poly_midi_modules(&self) -> bool {
        self.graph_state.graph.nodes.iter()
            .any(|(_, node)| node.user_data.module_id == "input.poly_midi")
    }

    /// Select an audio output device by index
    fn select_device(&mut self, index: usize) {
        // A take has one rate and channel count, so it ends before the switch.
        // Stopping the stream hands its tap back, so it finishes cleanly
        if self.is_recording() {
            self.stop_recording();
            if let Some(handle) = self.ui_handle.as_mut() {
                handle.flush();
            }
        }
        if let Ok(ref mut engine) = self.audio_engine {
            // On ASIO the engine closes it: its driver is the output's
            let input = engine.input_index();
            match engine.select_device(index) {
                Ok(()) => {
                    self.selected_device_index = index;
                    self.audio_error_message = None;
                }
                Err(e) => {
                    self.audio_error_message = Some(e.to_string());
                }
            }
            // The input must run at the new output's rate: open it again,
            // which also checks the new output for feedback
            if let Some(input) = input {
                self.refresh_input_devices();
                self.select_input(Some(input));
            }
        }
    }

    /// Moves audio to another system (Windows Audio or ASIO). An open input
    /// moves too, to the new system's default input. If the system won't
    /// start, audio stays where it was and a notice says why.
    fn select_audio_system(&mut self, system: AudioSystem) {
        self.end_take_for_device_change();
        let Ok(engine) = self.audio_engine.as_mut() else { return };
        let had_input = engine.input_index().is_some();
        let result = engine.set_audio_system(system);
        self.after_output_change();
        if let Err(e) = result {
            self.raise_notice(format!("Can't use {}: {}", system.label(), e));
        }
        if had_input {
            let default = self.input_devices.iter().find(|d| d.is_default).or(self.input_devices.first());
            self.select_input(default.map(|d| d.index));
        }
    }

    /// Asks the driver for `frames` per callback (ASIO), keeping an open
    /// input open.
    fn select_buffer(&mut self, frames: u32) {
        self.end_take_for_device_change();
        let Ok(engine) = self.audio_engine.as_mut() else { return };
        let input = engine.input_index();
        let result = engine.set_buffer_size(Some(frames));
        let running = engine.buffer_frames();
        self.after_output_change();
        match result {
            Err(e) => self.raise_notice(format!("Can't change the buffer: {}", e)),
            Ok(()) => {
                if let Some(running) = running.filter(|&running| running != frames) {
                    self.raise_notice(format!("The driver doesn't run at {} frames: it chose {}", frames, running));
                }
            }
        }
        if input.is_some() {
            self.select_input(input);
        }
    }

    /// When the driver has stopped the stream to change its own settings
    /// (its buffer size, from its control panel), starts it again there.
    fn recover_from_driver_reset(&mut self) {
        let Ok(engine) = self.audio_engine.as_mut() else { return };
        if !engine.stream_reset() {
            return;
        }
        self.end_take_for_device_change();
        let Ok(engine) = self.audio_engine.as_mut() else { return };
        let input = engine.input_index();
        let result = engine.restart_after_reset();
        let running = engine.buffer_frames();
        self.after_output_change();
        match (result, running) {
            (Err(e), _) => self.audio_error_message = Some(e.to_string()),
            (Ok(()), Some(frames)) => {
                self.raise_notice(format!("The driver's settings changed: audio restarted at {} frames", frames))
            }
            (Ok(()), None) => self.raise_notice("The driver's settings changed: audio restarted".to_string()),
        }
        if input.is_some() {
            self.select_input(input);
        }
    }

    /// Ends a recording before the output changes: a take has one rate and
    /// channel count. Stopping the stream hands its tap back, so it
    /// finishes cleanly.
    fn end_take_for_device_change(&mut self) {
        if self.is_recording() {
            self.stop_recording();
            if let Some(handle) = self.ui_handle.as_mut() {
                handle.flush();
            }
        }
    }

    /// Lists the devices again after the output changed system, device or
    /// buffer, pointing the Output menu at the one running.
    fn after_output_change(&mut self) {
        let Ok(engine) = self.audio_engine.as_ref() else { return };
        self.audio_devices = engine.enumerate_devices();
        self.selected_device_index = engine.current_device_index().unwrap_or(0);
        self.input_devices = engine.enumerate_input_devices();
        if !engine.stream_failed() {
            self.audio_error_message = None;
        }
        // The engine closed the input, if it was open; select_input opens
        // it again where the caller wants it
        if engine.input_index().is_none() {
            if let Some(handle) = self.ui_handle.as_mut() {
                handle.disconnect_input();
            }
            self.input_monitor = None;
            self.user_state.audio_input_name = None;
        }
    }

    // Note: start_audio/stop_audio removed - we now use AudioProcessor which
    // starts automatically. Use the Play/Stop transport button to control audio.

    /// Toggle the test tone on/off (legacy - may conflict with AudioProcessor)
    #[allow(dead_code)]
    fn toggle_test_tone(&mut self) {
        // Test tone is disabled when using AudioProcessor
        // This function is kept for potential future debug use
    }

    /// Whether a take is being recorded (not counting one being finished).
    fn is_recording(&self) -> bool {
        self.recording.as_ref().is_some_and(|r| !r.is_stopping())
    }

    /// The folder takes are written to.
    fn recordings_folder(&self) -> PathBuf {
        self.recordings_folder.clone().unwrap_or_else(recording::default_folder)
    }

    /// Starts or stops recording.
    fn toggle_recording(&mut self) {
        // Takes are written by a thread to a folder: the browser has neither
        if WEB {
            return;
        }
        if self.is_recording() {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    /// Starts a take: a WAV named after the patch, with the patch saved
    /// beside it, recording what the device plays from the next callback.
    /// Starts the transport too, if it's stopped.
    fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        let (Ok(engine), Some(_)) = (&self.audio_engine, &self.ui_handle) else {
            self.status_message = Some("Can't record: no audio output".to_string());
            return;
        };
        // While filming, the capture renders stereo at its own rate, and the
        // take goes with its other outputs
        let (sample_rate, channels, folder) = match &self.capture {
            Some(capture) => (capture.config().sample_rate, 2, capture.config().out_dir.clone()),
            None => (engine.sample_rate(), engine.channels(), self.recordings_folder()),
        };

        if let Err(e) = std::fs::create_dir_all(&folder) {
            self.status_message = Some(format!("Can't record to {}: {}", folder.display(), e));
            return;
        }
        let path = recording::take_path(&folder, &self.patch_title(), chrono::Local::now());
        let (take, tap) = match Recording::start(&path, sample_rate, channels) {
            Ok(started) => started,
            Err(e) => {
                self.status_message = Some(format!("Can't record: {}", e));
                return;
            }
        };
        // Saved now, so even a crash mid-take leaves the patch with the audio
        self.save_take_patch(&path);
        if let Some(handle) = self.ui_handle.as_mut() {
            handle.start_recording(tap);
        }
        self.recording = Some(take);
        if !self.is_playing {
            self.set_playing(true);
        }
    }

    /// Ends the take. The file is finished once the audio thread hands the
    /// tap back, a frame or so later; then the note pops up.
    fn stop_recording(&mut self) {
        let Some(take) = self.recording.as_mut().filter(|r| !r.is_stopping()) else { return };
        take.mark_stopping();
        let path = take.path().to_path_buf();
        if let Some(handle) = self.ui_handle.as_mut() {
            handle.stop_recording();
        }
        // Saved again as the take ended, with ridden knobs where they were left
        self.save_take_patch(&path);
    }

    /// Saves the patch beside a take, as `<take>.json`.
    fn save_take_patch(&mut self, wav: &Path) {
        let path = wav.with_extension("json");
        let patch = self.patch_for_file(&self.patch_title(), &path);
        if let Err(e) = save_to_file(&patch, &path) {
            eprintln!("Couldn't save the patch beside {}: {}", wav.display(), e);
        }
    }

    /// Once a stopped take's file is finished, puts up the note about it.
    fn poll_recording(&mut self, ctx: &egui::Context) {
        let Some(take) = self.recording.as_ref() else { return };
        if !take.is_stopping() {
            return;
        }
        if !take.poll_finished(RECORDING_PATIENCE) {
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
            return;
        }
        let take = self.recording.take().expect("checked above");
        let summary = take.finish(RECORDING_PATIENCE);
        let patch = summary.path.with_extension("json");
        self.record_toast = Some(Toast {
            patch: patch.exists().then_some(patch),
            summary,
            shown_at: ctx.input(|i| i.time),
        });
    }

    /// Stops and finishes the take right now, waiting for the file to be
    /// written: for quitting.
    fn finish_recording_now(&mut self) {
        self.stop_recording();
        let Some(take) = self.recording.take() else { return };
        // Give the audio thread a moment to hand the tap back, dropping it
        // here as soon as it does
        let deadline = Instant::now() + RECORDING_PATIENCE;
        while !take.poll_finished(RECORDING_PATIENCE) && Instant::now() < deadline {
            if let Some(handle) = self.ui_handle.as_mut() {
                handle.flush();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let summary = take.finish(RECORDING_PATIENCE);
        eprintln!("Recording saved: {}", summary.path.display());
    }

    /// Plays or stops the transport. Stopping also ends a take.
    fn set_playing(&mut self, playing: bool) {
        self.is_playing = playing;
        self.user_state.is_playing = playing;
        self.send_command(EngineCommand::SetPlaying(playing));
        if !playing {
            self.stop_recording();
        }
    }

    /// The recently opened patches, as menu items.
    fn recent_menu(&self, ui: &mut egui::Ui, actions: &mut ToolbarActions) {
        if self.recent_files.is_empty() {
            ui.label(RichText::new("No recent patches").color(theme::text::DISABLED).italics());
            return;
        }
        for path in self.recent_files.iter() {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            let button = ui.add_enabled(path.exists(), egui::Button::new(name));
            let clicked = button
                .on_hover_text(path.display().to_string())
                .on_disabled_hover_text(format!("Missing: {}", path.display()))
                .clicked();
            if clicked {
                actions.open_recent = Some(path.to_path_buf());
                ui.close_menu();
            }
        }
        ui.separator();
        if ui.button(RichText::new("Clear Recent").color(theme::text::SECONDARY)).clicked() {
            actions.clear_recent = true;
            ui.close_menu();
        }
    }

    /// Draw the top toolbar: transport, file, edit and device controls.
    ///
    /// When the window is too narrow for the full row, it goes compact: the
    /// file buttons fold into one File menu, Undo and Redo keep only their
    /// arrows, the group labels drop away and the gaps tighten.
    fn draw_toolbar(&mut self, ui: &mut egui::Ui) -> ToolbarActions {
        let mut actions = ToolbarActions::default();

        // A row centres each item in the height it has reached so far, and
        // grows when a taller item arrives. Starting every row at the height
        // of a button keeps the labels, buttons and selectors on one line.
        let row_height = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
        ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(row_height);
        // Labels whole, so a wrapped row places them like any other item
        // instead of flowing their text onto the next line
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        let compact = self.toolbar_full_width.is_some_and(|width| width > ui.available_width());
        let gap = if compact { 10.0 } else { 16.0 };
        let group_break = |ui: &mut egui::Ui| {
            ui.add_space(gap);
            ui.separator();
            ui.add_space(gap);
        };
        let group_label = |ui: &mut egui::Ui, text: &str| {
            if !compact {
                ui.label(RichText::new(text).color(theme::text::SECONDARY));
                ui.add_space(8.0);
            }
        };
        let named = |icon: &str, name: &str| {
            if compact { icon.to_string() } else { format!("{icon} {name}") }
        };

        let row = |ui: &mut egui::Ui| {
            wordmark(ui);

            group_break(ui);

            // Transport controls
            group_label(ui, "Transport");

            // Play/Stop button - controls whether the audio graph is processing
            let play_text = if self.is_playing { "⏹ Stop" } else { "▶ Play" };
            let play_color = if self.is_playing {
                theme::accent::WARNING
            } else {
                theme::accent::SUCCESS
            };

            if ui.button(RichText::new(play_text).color(play_color)).clicked() {
                actions.toggle_playing = true;
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
                    RecState::Recording(_) => "Stop recording and save the take (Ctrl+R)".to_string(),
                    _ => format!(
                        "Record what you hear to a WAV in {} (Ctrl+R)\nRight-click to choose the folder",
                        recording::short_path(&folder)
                    ),
                };
                let rec = recording::rec_button(ui, &rec_state).on_hover_text(hint);
                if rec.clicked() {
                    actions.toggle_recording = true;
                }
                rec.context_menu(|ui| {
                    ui.label(RichText::new("Recordings folder").color(theme::text::SECONDARY));
                    ui.label(recording::short_path(&folder));
                    ui.separator();
                    if ui.button("📂 Open Folder").clicked() {
                        actions.open_recordings = true;
                        ui.close_menu();
                    }
                    if ui.button("Change…").clicked() {
                        actions.choose_recordings_folder = true;
                        ui.close_menu();
                    }
                    if ui.add_enabled(self.recordings_folder.is_some(), egui::Button::new("Use Music/Soba")).clicked() {
                        actions.reset_recordings_folder = true;
                        ui.close_menu();
                    }
                });
            }

            group_break(ui);

            // File operations: named buttons, or one menu when space is short
            group_label(ui, "File");

            if compact {
                ui.menu_button("📁 File", |ui| {
                    let item = |ui: &mut egui::Ui, text: &str, shortcut: &str| {
                        ui.add(egui::Button::new(text).shortcut_text(shortcut)).clicked()
                    };
                    if item(ui, "📄 New", "Ctrl+N") {
                        actions.new_patch = true;
                        ui.close_menu();
                    }
                    if item(ui, "📂 Open", "Ctrl+O") {
                        actions.load_patch = true;
                        ui.close_menu();
                    }
                    if !WEB {
                        ui.menu_button("🕘 Recent", |ui| self.recent_menu(ui, &mut actions));
                    }
                    ui.separator();
                    if item(ui, "💾 Save", "Ctrl+S") {
                        actions.save_patch = true;
                        ui.close_menu();
                    }
                    if item(ui, "💾 Save As", "Ctrl+Shift+S") {
                        actions.save_as_patch = true;
                        ui.close_menu();
                    }
                });
            } else {
                if ui.button("📄 New").on_hover_text("Start an empty patch (Ctrl+N)").clicked() {
                    actions.new_patch = true;
                }

                if ui.button("📂 Open").on_hover_text("Ctrl+O").clicked() {
                    actions.load_patch = true;
                }

                if !WEB {
                    ui.menu_button("🕘 Recent", |ui| self.recent_menu(ui, &mut actions));
                }
            }

            ui.menu_button("📚 Examples", |ui| {
                for example in EXAMPLES {
                    if ui.button(example.name).on_hover_text(example.description).clicked() {
                        actions.open_example = Some(example);
                        ui.close_menu();
                    }
                }
            });

            if !compact {
                if ui.button("💾 Save").on_hover_text("Ctrl+S").clicked() {
                    actions.save_patch = true;
                }

                if ui.button("💾 Save As").on_hover_text("Save to a new file (Ctrl+Shift+S)").clicked() {
                    actions.save_as_patch = true;
                }
            }

            group_break(ui);

            // Edit history
            group_label(ui, "Edit");

            let undo_label = self.history.undo_label();
            let undo = ui.add_enabled(undo_label.is_some(), egui::Button::new(named("↩", "Undo")));
            if undo.on_hover_text(history_hint("Undo", undo_label, "Ctrl+Z")).clicked() {
                actions.undo = true;
            }
            let redo_label = self.history.redo_label();
            let redo = ui.add_enabled(redo_label.is_some(), egui::Button::new(named("↪", "Redo")));
            if redo.on_hover_text(history_hint("Redo", redo_label, "Ctrl+Shift+Z")).clicked() {
                actions.redo = true;
            }

            group_break(ui);

            // How the signal is drawn flowing along the cables
            ui.menu_button("〰 Cables", |ui| {
                ui.label(RichText::new("Signal flow marks").color(theme::text::SECONDARY));
                for glyph in FlowGlyph::ALL {
                    ui.radio_value(&mut self.user_state.flow_glyph, glyph, glyph.name());
                }
            })
            .response
            .on_hover_text("How signal flow is drawn along cables");

            // How the modules' knobs are drawn
            ui.menu_button("◉ Knobs", |ui| {
                ui.label(RichText::new("Knob style").color(theme::text::SECONDARY));
                for style in KnobStyle::ALL {
                    ui.radio_value(&mut self.user_state.knob_style, style, style.name());
                }
            })
            .response
            .on_hover_text("How knobs are drawn");

            ui.menu_button("❓ Help", |ui| {
                if ui.button("📖 Manual").on_hover_text("docs.chaps.dev/soba, in your browser").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(MANUAL_URL));
                    ui.close_menu();
                }
                #[cfg(not(target_arch = "wasm32"))]
                self.update_menu(ui, &mut actions);
            });

            // Device selectors (engine status lives in the status bar). The
            // browser plays through its own output, and has no MIDI or input yet
            match &self.audio_engine {
                Ok(_) if WEB => {}
                Ok(_) => {
                    group_break(ui);

                    // Device selector
                    ui.label(RichText::new("Output").color(theme::text::SECONDARY));
                    ui.add_space(8.0);

                    // Get current device name for display
                    let current_device = self.audio_devices
                        .get(self.selected_device_index)
                        .map(|d| d.name.as_str())
                        .unwrap_or("No device");

                    // Truncate long device names
                    let display_name = if current_device.len() > 30 {
                        format!("{}...", &current_device[..current_device.floor_char_boundary(27)])
                    } else {
                        current_device.to_string()
                    };
                    let (system, buffer_choices, buffer_frames, sample_rate) = match &self.audio_engine {
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
                                        actions.select_audio_system = Some(choice);
                                    }
                                }
                                // Steinberg's licence asks for its logo where ASIO is chosen
                                #[cfg(feature = "asio")]
                                super::asio_badge::show(ui);
                                ui.separator();
                            }
                            for device in &self.audio_devices {
                                let label = if device.is_default {
                                    format!("{} (Default)", device.name)
                                } else {
                                    device.name.clone()
                                };

                                if ui.selectable_label(
                                    device.index == self.selected_device_index,
                                    label
                                ).clicked() {
                                    actions.select_device = Some(device.index);
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
                                            actions.select_buffer = Some(*frames);
                                        }
                                    }
                                });
                            }

                            ui.separator();
                            if ui.button("🔄 Refresh").clicked() {
                                actions.refresh_devices = true;
                            }
                        });

                    // Audio input selector: None until chosen
                    ui.add_space(12.0);
                    ui.label(RichText::new("Input").color(theme::text::SECONDARY));
                    ui.add_space(8.0);
                    let (input_index, input_name) = match &self.audio_engine {
                        Ok(engine) => (engine.input_index(), engine.input_name().map(str::to_string)),
                        Err(_) => (None, None),
                    };
                    let input_failed = self.input_monitor.as_ref().is_some_and(InputMonitor::failed);
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
                                actions.select_input = Some(None);
                            }
                            ui.separator();
                            if self.input_devices.is_empty() {
                                ui.label(RichText::new("No input devices found")
                                    .color(theme::text::DISABLED)
                                    .italics());
                            }
                            for device in &self.input_devices {
                                let label = if device.is_default {
                                    format!("{} (Default)", device.name)
                                } else {
                                    device.name.clone()
                                };
                                if ui.selectable_label(input_index == Some(device.index), label).clicked() {
                                    actions.select_input = Some(Some(device.index));
                                }
                            }
                            ui.separator();
                            if ui.button("🔄 Refresh").clicked() {
                                actions.refresh_input_devices = true;
                            }
                        })
                        .response
                        .on_hover_text("Audio Input modules hear this device: a microphone, guitar or line in");

                    group_break(ui);

                    // MIDI input selector
                    ui.label(RichText::new("MIDI In").color(theme::text::SECONDARY));
                    ui.add_space(8.0);

                    // Get current MIDI device name for display
                    let current_midi = self.selected_midi_device
                        .and_then(|idx| self.midi_devices.get(idx))
                        .map(|d| d.name.as_str())
                        .unwrap_or("None");

                    // Truncate long device names
                    let midi_display_name = if current_midi.len() > 25 {
                        format!("{}...", &current_midi[..22])
                    } else {
                        current_midi.to_string()
                    };

                    // MIDI connection indicator
                    let midi_connected = self.selected_midi_device.is_some()
                        && self.midi_engine.as_ref().map(|e| e.is_connected()).unwrap_or(false);
                    let midi_indicator = if midi_connected { "● " } else { "○ " };

                    egui::ComboBox::from_id_salt("midi_device_selector")
                        .selected_text(format!("{}{}", midi_indicator, midi_display_name))
                        .width(180.0)
                        .height(DEVICE_MENU_HEIGHT)
                        .show_ui(ui, |ui| {
                            // Option to disconnect / select none
                            if ui.selectable_label(
                                self.selected_midi_device.is_none(),
                                "None (Disconnect)"
                            ).clicked() {
                                actions.disconnect_midi = true;
                            }

                            ui.separator();

                            // List available MIDI devices
                            if self.midi_devices.is_empty() {
                                ui.label(RichText::new("No MIDI devices found")
                                    .color(theme::text::DISABLED)
                                    .italics());
                            } else {
                                for device in &self.midi_devices {
                                    let is_selected = self.selected_midi_device == Some(device.index);
                                    if ui.selectable_label(is_selected, &device.name).clicked() {
                                        actions.connect_midi_device = Some(device.index);
                                    }
                                }
                            }

                            ui.separator();
                            if ui.button("🔄 Refresh").clicked() {
                                actions.refresh_midi_devices = true;
                            }
                        });
                }
                Err(e) => {
                    group_break(ui);
                    ui.label(RichText::new(format!("⚠ Audio unavailable: {}", e))
                        .color(theme::accent::ERROR));
                }
            }
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

    /// Send a command to the audio engine.
    fn send_command(&mut self, cmd: EngineCommand) {
        if let Some(ref mut handle) = self.ui_handle {
            // Never dropped: anything that doesn't fit in the queue now is
            // held and delivered by a later flush
            handle.send_command(cmd);
        }
    }

    /// Process events from the audio engine.
    /// This handles InputValue events for knob animation, OutputValue events for LED indicators,
    /// ScopeBuffer events for oscilloscope display, and CpuLoad events for CPU metering.
    fn process_engine_events(&mut self) {
        if let Some(ref mut handle) = self.ui_handle {
            // Drain all available events
            while let Some(event) = handle.recv_event() {
                match event {
                    crate::engine::EngineEvent::InputValue { node_id, input_index, value } => {
                        // Store the input value for UI feedback
                        self.user_state.set_input_value(node_id, input_index, value);
                    }
                    crate::engine::EngineEvent::OutputValue { node_id, output_index, value, channels } => {
                        // Store the output value for LED indicators, and each
                        // channel's for drawing poly cables strand by strand
                        self.user_state.set_output_value(node_id, output_index, value);
                        self.user_state.set_output_channels(node_id, output_index, channels);
                    }
                    crate::engine::EngineEvent::ScopeBuffer { node_id, channel1, channel2, triggered } => {
                        // Store the oscilloscope waveform data for display
                        self.user_state.set_scope_data(
                            node_id,
                            channel1.into_vec(),
                            channel2.into_vec(),
                            triggered,
                        );
                    }
                    crate::engine::EngineEvent::CpuLoad(load) => {
                        // Update CPU load for display
                        self.cpu_load = load;
                    }
                    crate::engine::EngineEvent::OutputLevel(levels) => {
                        // Feed the Audio Output node's meter
                        self.user_state.output_meter.feed(levels);
                    }
                    crate::engine::EngineEvent::MeterLevels { node_id, levels } => {
                        // Feed a module's own meters (the Mixer's strips)
                        self.user_state.module_meters.entry(node_id).or_default().feed(&levels);
                    }
                    crate::engine::EngineEvent::Readout { node_id, readout } => {
                        // A Clock's tempo and beat, for its node
                        self.user_state.readouts.insert(node_id, readout);
                    }
                    // Other events are not currently handled by the app
                    // (Started, Stopped, Error)
                    _ => {}
                }
            }
        }
    }

    /// Draw the main content area with the node graph editor
    fn draw_main_area(&mut self, ctx: &egui::Context) {
        // Collect connections to remove (validated after drawing)
        let mut invalid_connections: Vec<(egui_node_graph2::OutputId, egui_node_graph2::InputId)> = Vec::new();
        // Collect commands to send (to avoid borrow issues)
        let mut commands_to_send: Vec<EngineCommand> = Vec::new();
        // Nodes whose bypass switch was clicked
        let mut bypass_toggles: Vec<egui_node_graph2::NodeId> = Vec::new();
        // What modules' right-click menus asked for
        let mut node_menu_actions: Vec<NodeMenuAction> = Vec::new();
        // Groups whose node was closed, whose insides go too
        let mut deleted_groups: Vec<GroupId> = Vec::new();
        // What this frame shows: the top of the patch, or a group's inside
        self.prepare_level();
        // Track if we clicked in the editor area
        let mut cursor_in_editor = false;
        // Store editor rect for coordinate conversion
        let mut editor_rect = egui::Rect::NOTHING;

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                // Store editor rect for coordinate conversion
                editor_rect = ui.available_rect_before_wrap();
                self.editor_rect = editor_rect;

                // Reset widget context menu flag before drawing
                self.user_state.widget_context_menu_open = false;

                // Update zoom for widget scaling, and where frames and notes go
                self.user_state.zoom = self.graph_state.pan_zoom.zoom;
                self.user_state.view_origin = self.history.view_origin();

                // The grid sits under the patch and moves with it
                let grid_origin = editor_rect.min + self.graph_state.pan_zoom.pan + self.history.view_origin();
                theme::draw_grid_background(ui.painter(), editor_rect, grid_origin, self.graph_state.pan_zoom.zoom);
                self.draw_level_backdrop(ui.painter(), editor_rect);

                // The view the copy before this one had, after an update
                #[cfg(not(target_arch = "wasm32"))]
                self.apply_pending_view(ui, editor_rect);

                // Draw the node graph editor
                let (zoom_before, pan_before) = (self.graph_state.pan_zoom.zoom, self.graph_state.pan_zoom.pan);
                // Fitted again each frame for a few: zoomed out, modules that
                // were off screen are drawn and measured, and may need more room
                if self.fit_pending > 0 {
                    self.fit_pending -= 1;
                    self.fit_level(ui);
                }
                self.follow_gestures(ui, editor_rect);
                // A capture script moves the view like a camera
                if let Some(capture) = self.capture.as_mut() {
                    let zoom = capture.take_zoom();
                    if zoom != 1.0 {
                        self.graph_state.zoom(ui, zoom);
                    }
                    if let Some(pan) = capture.take_pan(self.graph_state.pan_zoom.pan) {
                        self.graph_state.pan_zoom.pan = pan;
                    }
                }
                let graph_response = self.graph_state.draw_graph_editor(
                    ui,
                    AllNodeTemplates,
                    &mut self.user_state,
                    Vec::default(),
                );
                // Zooming moves every node; undo keeps positions that don't
                self.history.follow_zoom(zoom_before, pan_before, &self.graph_state.pan_zoom);
                self.follow_touch_pan(ctx);
                self.remember_node_sizes(ctx);
                self.user_state.view_origin = self.history.view_origin();

                // A selection box takes in the frames and notes wholly inside it
                if let Some(start) = self.graph_state.ongoing_box_selection {
                    if let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) {
                        self.user_state.annotations.select_within(egui::Rect::from_two_pos(start, pointer));
                    }
                }

                cursor_in_editor = graph_response.cursor_in_editor;

                // Disable the built-in node finder - we use our own context menu
                self.graph_state.node_finder = None;

                // Process graph responses
                for response in graph_response.node_responses {
                    match response {
                        NodeResponse::CreatedNode(node_id) => {
                            // Allocate engine node ID for the new node
                            let engine_node_id = self.user_state.allocate_engine_node_id(node_id);
                            commands_to_send.extend(engine_sync::add_module(
                                &self.graph_state.graph,
                                node_id,
                                engine_node_id,
                            ));
                        }
                        NodeResponse::DeleteNodeFull { node_id, node } => {
                            // Get engine node ID before removing from mapping
                            if let Some(engine_node_id) = self.user_state.remove_node(node_id) {
                                commands_to_send.push(EngineCommand::RemoveModule {
                                    node_id: engine_node_id,
                                });
                            }
                            // A group goes with everything in it
                            if let Some(id) = node.user_data.kind.group() {
                                deleted_groups.push(id);
                            }
                        }
                        NodeResponse::ConnectEventEnded { output, input, .. } => {
                            // Validate the connection after it was made
                            if let Some(error_msg) = self.validate_and_check_connection(output, input) {
                                // Mark for removal
                                invalid_connections.push((output, input));
                                // Show error message
                                self.user_state.set_validation_error(error_msg);
                            }
                            // The engine hears about it with the frame's other cables
                        }
                        NodeResponse::User(crate::graph::SynthResponse::ParameterChanged {
                            node_id: response_node_id,
                            param_name,
                            value,
                        }) => {
                            // Handle parameter changes from bottom_ui knobs
                            // Find the input param by name and update its value
                            if let Some(node) = self.graph_state.graph.nodes.get_mut(response_node_id) {
                                if let Some((_name, input_id)) = node.inputs.iter().find(|(name, _)| *name == param_name) {
                                    let input_id = *input_id;
                                    if let Some(input) = self.graph_state.graph.inputs.get_mut(input_id) {
                                        input.value.set_actual_value(value);
                                    }
                                }
                            }
                        }
                        NodeResponse::User(crate::graph::SynthResponse::PlayParameter { node_id: response_node_id, param_name, value }) => {
                            self.play_parameter(response_node_id, &param_name, value);
                        }
                        NodeResponse::User(crate::graph::SynthResponse::Audition { node_id: response_node_id, chord }) => {
                            self.audition(response_node_id, chord);
                        }
                        NodeResponse::User(crate::graph::SynthResponse::EditParameters {
                            node_id: response_node_id,
                            label,
                            changes,
                        }) => {
                            if let Some(node) = self.graph_state.graph.nodes.get(response_node_id) {
                                let inputs: Vec<_> = changes
                                    .iter()
                                    .filter_map(|(name, value)| {
                                        node.inputs.iter().find(|(input, _)| input == name).map(|(_, id)| (*id, *value))
                                    })
                                    .collect();
                                for (input_id, value) in inputs {
                                    if let Some(input) = self.graph_state.graph.inputs.get_mut(input_id) {
                                        input.value.set_actual_value(value);
                                    }
                                }
                            }
                            self.history.name_next(label);
                        }
                        NodeResponse::User(crate::graph::SynthResponse::SetLabels { node_id: response_node_id, label, labels }) => {
                            if let Some(node) = self.graph_state.graph.nodes.get_mut(response_node_id) {
                                for (key, text) in labels {
                                    if text.is_empty() {
                                        node.user_data.labels.remove(&key);
                                    } else {
                                        node.user_data.labels.insert(key, text);
                                    }
                                }
                            }
                            self.history.name_next(label);
                        }
                        NodeResponse::User(crate::graph::SynthResponse::MidiLearnStart {
                            engine_node_id,
                            param_index,
                            param_name,
                            min_value,
                            max_value,
                        }) => {
                            // Start MIDI Learn mode for this parameter
                            self.start_midi_learn(MidiLearnTarget {
                                node_id: engine_node_id,
                                param_index,
                                param_name,
                                min_value,
                                max_value,
                            });
                        }
                        NodeResponse::User(crate::graph::SynthResponse::MidiLearnCancel) => {
                            self.cancel_midi_learn();
                        }
                        NodeResponse::User(crate::graph::SynthResponse::MidiLearnClear {
                            engine_node_id,
                            param_index,
                        }) => {
                            // Clear MIDI mapping for this parameter
                            self.clear_mapping_for_param(engine_node_id, param_index);
                            // Update the user state
                            self.user_state.remove_midi_mapping(engine_node_id, param_index);
                        }
                        NodeResponse::User(crate::graph::SynthResponse::ToggleBypass(node_id)) => {
                            bypass_toggles.push(node_id);
                        }
                        NodeResponse::User(crate::graph::SynthResponse::NodeSelected(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Select(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::DuplicateNode(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Duplicate(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::CopyNode(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Copy(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::ResetNode(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Reset(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::DeleteNode(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Delete(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::GroupNode(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Group(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::UngroupNode(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Ungroup(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::EnterGroup(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::Enter(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::StartRename(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::StartRename(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::RenameGroup { node_id, name }) => {
                            node_menu_actions.push(NodeMenuAction::Rename(node_id, name));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::PinKnob { node_id, param_name, levels }) => {
                            node_menu_actions.push(NodeMenuAction::Pin(node_id, param_name, levels));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::NameFaceControl { node_id, param_name, name }) => {
                            node_menu_actions.push(NodeMenuAction::NameFace(node_id, param_name, name));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::Jack { node_id, side, port, edit }) => {
                            node_menu_actions.push(NodeMenuAction::Jack(node_id, side, port, edit));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::SaveGroup(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::SaveToLibrary(node_id));
                        }
                        NodeResponse::User(crate::graph::SynthResponse::OpenSample(node_id)) => {
                            node_menu_actions.push(NodeMenuAction::OpenSample(node_id));
                        }
                        _ => {
                            // Other responses not yet handled
                        }
                    }
                }
            });

        // Detect right-click in editor to open context menu
        let mut menu_just_opened = false;
        // Only show "add node" menu when clicking on empty canvas, not on nodes/widgets
        if ctx.input(|i| i.pointer.secondary_clicked()) && cursor_in_editor {
            // Only open if not already showing a menu and no widget context menu is open
            if self.user_state.context_menu_pos.is_none()
                && !self.user_state.widget_context_menu_open
                && !self.user_state.over_annotation
            {
                let click_pos = ctx.input(|i| i.pointer.interact_pos());
                // A module has its own menu, and its knobs theirs. Neither says
                // so on the frame of the click, so go by where the click was
                if let Some(click_pos) = click_pos.filter(|pos| !self.is_over_module(ctx, *pos)) {
                    self.user_state.context_menu_pos = Some(click_pos);
                    self.my_modules = library::list();
                    menu_just_opened = true;
                }
            }
        }

        // Show custom context menu for adding nodes
        if let Some(menu_pos) = self.user_state.context_menu_pos {
            let mut close_menu = false;
            let mut template_to_create: Option<SynthNodeTemplate> = None;
            let mut saved_to_add: Option<SavedModule> = None;
            let mut open_library = false;
            let mut annotation_to_create: Option<NewAnnotation> = None;

            // Hover delay before switching submenus (in seconds)
            const SUBMENU_HOVER_DELAY: f32 = 0.15;

            let categories = AllNodeTemplates::by_category();

            let menu_id = egui::Id::new("add_node_context_menu");
            let menu_response = egui::Area::new(menu_id)
                .fixed_pos(menu_pos)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::menu(ui.style()).show(ui, |ui| {
                        ui.set_min_width(120.0);

                        // The categories, then My Modules if anything's saved
                        // there, then the Library
                        let rows: Vec<(&str, egui::Color32)> = categories
                            .iter()
                            .map(|(category, _)| (category.name(), category.color()))
                            .chain((!self.my_modules.is_empty()).then_some(("My Modules", theme::module::GROUP)))
                            .chain(std::iter::once(("Library", theme::module::GROUP)))
                            .collect();
                        for (cat_index, (name, color)) in rows.into_iter().enumerate() {
                            // Create category button with arrow indicator
                            let button_text = egui::RichText::new(format!("{}  \u{25B6}", name)).color(color);

                            let response = ui.add(
                                egui::Button::new(button_text)
                                    .min_size(egui::vec2(110.0, 0.0))
                                    .frame(false)
                            );

                            // Handle hover intent with delay
                            if response.hovered() {
                                let now = Instant::now();

                                // Check if we're already tracking this category
                                if let Some((tracked_cat, hover_start)) = self.user_state.context_menu_hover_intent {
                                    if tracked_cat == cat_index {
                                        // Same category - check if delay has passed
                                        if hover_start.elapsed().as_secs_f32() >= SUBMENU_HOVER_DELAY {
                                            self.user_state.context_menu_open_category = Some(cat_index);
                                        }
                                    } else {
                                        // Different category - start new tracking
                                        self.user_state.context_menu_hover_intent = Some((cat_index, now));
                                    }
                                } else {
                                    // No tracking yet - start tracking
                                    self.user_state.context_menu_hover_intent = Some((cat_index, now));

                                    // If no submenu is open, open immediately
                                    if self.user_state.context_menu_open_category.is_none() {
                                        self.user_state.context_menu_open_category = Some(cat_index);
                                    }
                                }
                            }

                            // Handle click to immediately open/toggle
                            if response.clicked() {
                                self.user_state.context_menu_open_category = Some(cat_index);
                                self.user_state.context_menu_hover_intent = None;
                            }
                        }

                        // Frames and notes, to explain the patch, on its top level
                        let framing = !self.graph_state.selected_nodes.is_empty();
                        let annotating = self.frames_allowed();
                        if annotating {
                            ui.separator();
                        }
                        for (label, kind, hint) in [
                            ("Frame", NewAnnotation::Frame,
                                if framing { "A titled backdrop around the selected modules (Ctrl+Shift+F)" }
                                else { "A titled backdrop to group modules under (Ctrl+Shift+F frames the selection)" }),
                            ("Note", NewAnnotation::Note, "A card of text. **Bold** for emphasis"),
                        ].into_iter().filter(|_| annotating) {
                            let response = ui.add(
                                egui::Button::new(RichText::new(label).color(theme::text::SECONDARY))
                                    .min_size(egui::vec2(110.0, 0.0))
                                    .frame(false),
                            );
                            if response.hovered() {
                                self.user_state.context_menu_open_category = None;
                                self.user_state.context_menu_hover_intent = None;
                            }
                            if response.on_hover_text(hint).clicked() {
                                annotation_to_create = Some(kind);
                                close_menu = true;
                            }
                        }
                    });
                });

            // Show submenu for open category
            let mut submenu_rect: Option<egui::Rect> = None;
            if let Some(open_cat_index) = self.user_state.context_menu_open_category {
                if let Some((_category, templates)) = categories.get(open_cat_index) {
                    // Position submenu to the right of the main menu
                    let submenu_pos = menu_response.response.rect.right_top() + egui::vec2(4.0, open_cat_index as f32 * 22.0);

                    let submenu_id = egui::Id::new("add_node_submenu");
                    let submenu_response = egui::Area::new(submenu_id)
                        .fixed_pos(submenu_pos)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            egui::Frame::menu(ui.style()).show(ui, |ui| {
                                ui.set_min_width(100.0);

                                for template in templates {
                                    let label = template.node_finder_label(&mut self.user_state);
                                    if ui.button(label.as_ref()).on_hover_text(template.description()).clicked() {
                                        template_to_create = Some(*template);
                                        close_menu = true;
                                    }
                                }
                            });
                        });

                    submenu_rect = Some(submenu_response.response.rect);

                    // Keep submenu open if mouse is inside it
                    if submenu_response.response.rect.contains(ctx.input(|i| i.pointer.hover_pos().unwrap_or_default())) {
                        // Reset hover intent when mouse is in submenu
                        self.user_state.context_menu_hover_intent = None;
                    }
                } else if open_cat_index == categories.len() && !self.my_modules.is_empty() {
                    // My Modules: the groups saved there
                    let submenu_pos = menu_response.response.rect.right_top() + egui::vec2(4.0, open_cat_index as f32 * 22.0);
                    let submenu_response = egui::Area::new(egui::Id::new("add_node_submenu"))
                        .fixed_pos(submenu_pos)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            egui::Frame::menu(ui.style()).show(ui, |ui| {
                                ui.set_min_width(120.0);
                                for saved in &self.my_modules {
                                    let text = RichText::new(&saved.name).color(theme::module::GROUP);
                                    if ui.button(text).on_hover_text(&saved.summary).clicked() {
                                        saved_to_add = Some(saved.clone());
                                        close_menu = true;
                                    }
                                }
                                ui.separator();
                                if ui.button(RichText::new("Open folder").color(theme::text::SECONDARY)).clicked() {
                                    open_library = true;
                                    close_menu = true;
                                }
                            });
                        });
                    submenu_rect = Some(submenu_response.response.rect);
                    if submenu_response.response.rect.contains(ctx.input(|i| i.pointer.hover_pos().unwrap_or_default())) {
                        self.user_state.context_menu_hover_intent = None;
                    }
                } else if open_cat_index == categories.len() + usize::from(!self.my_modules.is_empty()) {
                    // The Library: ready-made groups, under their sections
                    let submenu_pos = menu_response.response.rect.right_top() + egui::vec2(4.0, open_cat_index as f32 * 22.0);
                    let submenu_response = egui::Area::new(egui::Id::new("add_node_submenu"))
                        .fixed_pos(submenu_pos)
                        .order(egui::Order::Foreground)
                        .constrain(true)
                        .show(ctx, |ui| {
                            egui::Frame::menu(ui.style()).show(ui, |ui| {
                                ui.set_min_width(140.0);
                                for (n, section) in Section::ALL.into_iter().enumerate() {
                                    if n > 0 {
                                        ui.add_space(4.0);
                                    }
                                    ui.label(RichText::new(section.name().to_uppercase()).small().color(theme::text::DISABLED));
                                    for saved in self.library_modules.iter().filter(|m| m.library_group().is_some_and(|g| g.section == section)) {
                                        let text = RichText::new(&saved.name).color(theme::module::GROUP);
                                        if ui.button(text).on_hover_text(&saved.summary).clicked() {
                                            saved_to_add = Some(saved.clone());
                                            close_menu = true;
                                        }
                                    }
                                }
                            });
                        });
                    submenu_rect = Some(submenu_response.response.rect);
                    if submenu_response.response.rect.contains(ctx.input(|i| i.pointer.hover_pos().unwrap_or_default())) {
                        self.user_state.context_menu_hover_intent = None;
                    }
                }
            }

            // Close menu on click outside
            let menu_rect = menu_response.response.rect;
            // Near the bottom of the window the menu is moved up to fit, off the
            // click that opened it, which mustn't count as a click outside
            if ctx.input(|i| i.pointer.any_click()) && !menu_just_opened {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    // Check if click was outside both main menu and submenu
                    let in_main_menu = menu_rect.contains(pos);
                    let in_submenu = submenu_rect.map_or(false, |r| r.contains(pos));
                    if !in_main_menu && !in_submenu && template_to_create.is_none() && annotation_to_create.is_none() && saved_to_add.is_none() {
                        close_menu = true;
                    }
                }
            }

            // Close menu on Escape
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                close_menu = true;
            }

            // Create node if a template was selected
            if let Some(template) = template_to_create {
                self.add_module_at(template, menu_pos);
                close_menu = true;
            }
            if let Some(saved) = saved_to_add {
                self.add_saved_module(&saved, menu_pos);
            }
            if open_library {
                self.open_library_folder();
            }
            match annotation_to_create {
                Some(NewAnnotation::Frame) if !self.graph_state.selected_nodes.is_empty() => self.frame_selection(ctx),
                Some(NewAnnotation::Frame) => self.add_frame_at(menu_pos),
                Some(NewAnnotation::Note) => self.add_note_at(menu_pos),
                None => {}
            }

            if close_menu {
                self.user_state.context_menu_pos = None;
                self.user_state.context_menu_open_category = None;
                self.user_state.context_menu_hover_intent = None;
            }
        }

        // The quick-add palette
        if let Some(palette) = &mut self.quick_add {
            match palette.show(ctx) {
                PaletteAction::None => {}
                PaletteAction::Close => self.quick_add = None,
                PaletteAction::Add(template) => {
                    let anchor = palette.anchor();
                    self.quick_add = None;
                    self.add_module_at(template, anchor);
                }
                PaletteAction::AddSaved(saved) => {
                    let anchor = palette.anchor();
                    self.quick_add = None;
                    self.add_saved_module(&saved, anchor);
                }
            }
        }

        // Send collected commands
        for cmd in commands_to_send {
            self.send_command(cmd);
        }
        for node_id in bypass_toggles {
            self.toggle_bypass(node_id);
        }
        for action in node_menu_actions {
            self.handle_node_menu(ctx, action);
        }
        for id in deleted_groups {
            self.delete_group_contents(id);
        }
        self.draw_breadcrumbs(ctx);

        // Remove invalid connections outside the UI closure
        for (output, input) in invalid_connections {
            self.graph_state.graph.remove_connection(input, output);
        }
    }

    /// Keeps each Arranger that follows another playing its leader's
    /// sections. See [`crate::graph::arranger_follow`].
    fn follow_arrangers(&mut self) {
        self.user_state.arranger_leaders = crate::graph::arranger_follow::leaders(&self.graph_state.graph);
        crate::graph::arranger_follow::sync(&mut self.graph_state.graph, &self.user_state.arranger_leaders);
    }

    /// Sends the engine whatever cables have changed since it last heard,
    /// module to module through any groups.
    fn sync_cables(&mut self) {
        for cmd in engine_sync::sync_cables(&self.graph_state.graph, &mut self.user_state) {
            self.send_command(cmd);
        }
    }

    /// Undoes the last edit to the patch.
    fn undo(&mut self) {
        let applied = self.history.undo(&mut self.graph_state, &mut self.user_state);
        self.finish_history_move(applied, "Undo", "Nothing to undo");
    }

    /// Redoes the last undone edit.
    fn redo(&mut self) {
        let applied = self.history.redo(&mut self.graph_state, &mut self.user_state);
        self.finish_history_move(applied, "Redo", "Nothing to redo");
    }

    /// Sends the engine what undo or redo changed, and says what it was.
    fn finish_history_move(&mut self, applied: Option<Applied>, verb: &str, nothing: &str) {
        let Some(applied) = applied else {
            self.status_message = Some(nothing.to_string());
            return;
        };
        // Shown where it happens: the level the edit was made on
        self.prepare_level();
        if applied.level != self.user_state.level {
            self.go_to_level(applied.level);
        }
        for cmd in applied.commands {
            // The engine now has these values, so parameter sync needn't resend them
            if let EngineCommand::SetParameter { node_id, param_index, value } = cmd {
                self.cached_params.insert((node_id, param_index), value);
            }
            self.send_command(cmd);
        }
        self.status_message = Some(format!("{}: {}", verb, applied.label));
    }

    /// Bypasses a filter or effect, or switches it back in.
    fn toggle_bypass(&mut self, node_id: egui_node_graph2::NodeId) {
        let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) else {
            return;
        };
        if !node.user_data.bypassable {
            return;
        }
        node.user_data.bypassed = !node.user_data.bypassed;
        let bypassed = node.user_data.bypassed;
        if let Some(engine_node_id) = self.user_state.get_engine_node_id(node_id) {
            self.send_command(EngineCommand::SetBypass { node_id: engine_node_id, bypassed });
        }
    }

    /// Whether a point on screen is over a module, anywhere on it.
    fn is_over_module(&self, ctx: &egui::Context, screen: egui::Pos2) -> bool {
        // The editor senses clicks on each whole node under this id
        self.graph_state.node_order.iter().any(|&node_id| {
            ctx.read_response(egui::Id::new((node_id, "window"))).is_some_and(|r| r.rect.contains(screen))
        })
    }

    /// The editor node position under a point on screen.
    fn screen_to_node(&self, screen: egui::Pos2) -> egui::Pos2 {
        screen - self.editor_rect.min.to_vec2() - self.graph_state.pan_zoom.pan
    }

    /// Where something placed "at the cursor" goes: the pointer if it's over
    /// the graph, otherwise near the middle of the view. In screen points.
    fn cursor_or_center(&self, ctx: &egui::Context) -> egui::Pos2 {
        ctx.input(|i| i.pointer.hover_pos())
            .filter(|pos| self.editor_rect.contains(*pos))
            .unwrap_or_else(|| self.editor_rect.center() - egui::vec2(150.0, 120.0))
    }

    /// Adds a module with its top-left corner at a point on screen, and selects it.
    fn add_module_at(&mut self, template: SynthNodeTemplate, screen: egui::Pos2) {
        let position = self.screen_to_node(screen);
        let (node_id, commands) = editing::add_module(&mut self.graph_state, &mut self.user_state, template, position);
        for cmd in commands {
            self.send_command(cmd);
        }
        self.graph_state.selected_nodes = vec![node_id];
    }

    /// Opens the quick-add palette at the cursor.
    fn open_quick_add(&mut self, ctx: &egui::Context) {
        let anchor = self.cursor_or_center(ctx);
        self.user_state.context_menu_pos = None;
        self.my_modules = library::list();
        let groups = self.my_modules.iter().chain(&self.library_modules).cloned().collect();
        self.quick_add = Some(QuickAdd::new(anchor, groups));
    }

    /// The patch position of a point on screen.
    fn screen_to_patch(&self, screen: egui::Pos2) -> egui::Pos2 {
        self.history.to_patch(self.screen_to_node(screen), self.graph_state.pan_zoom.zoom)
    }

    /// What's selected: modules, frames and notes.
    fn selection(&self) -> Selection {
        Selection {
            nodes: self.graph_state.selected_nodes.clone(),
            annotations: self.user_state.annotations.selection(),
        }
    }

    /// Selects exactly these, and nothing else.
    fn select(&mut self, nodes: Vec<egui_node_graph2::NodeId>, annotations: Vec<AnnotationId>) {
        self.graph_state.selected_nodes = nodes;
        self.user_state.annotations.selected = annotations.into_iter().collect();
    }

    /// What a right-click menu action applies to: the whole selection if
    /// the node is part of it, otherwise just the node.
    fn menu_targets(&self, node_id: egui_node_graph2::NodeId) -> Selection {
        if self.graph_state.selected_nodes.contains(&node_id) {
            self.selection()
        } else {
            Selection::modules(&[node_id])
        }
    }

    /// Adds a frame around the selected modules, tinted for what they are,
    /// and opens its title for naming.
    fn frame_selection(&mut self, ctx: &egui::Context) {
        if !self.frames_allowed() {
            self.status_message = Some("Frames and notes go on the top level of the patch".to_string());
            return;
        }
        let nodes = self.graph_state.selected_nodes.clone();
        let Some(screen) = nodes.iter().filter_map(|&id| annotation_ui::module_rect(ctx, id)).reduce(|a, b| a.union(b)) else {
            self.status_message = Some("Select modules to frame them (Ctrl+Shift+F)".to_string());
            return;
        };
        let around = egui::Rect::from_min_max(self.screen_to_patch(screen.min), self.screen_to_patch(screen.max));
        let rect = egui::Rect::from_min_max(
            around.min - egui::vec2(FRAME_PADDING, FRAME_PADDING + annotation_ui::FRAME_TITLE_BAND),
            around.max + egui::vec2(FRAME_PADDING, FRAME_PADDING),
        );
        let graph = &self.graph_state.graph;
        let tint = Tint::for_contents(nodes.iter().filter_map(|&id| graph.nodes.get(id)).map(|n| n.user_data.category));
        self.add_frame(rect, tint);
        let what = editing::describe_modules(&self.graph_state, &nodes);
        self.status_message = Some(format!("Framed {what}: name the frame, then press Enter"));
    }

    /// Adds an empty frame with its top-left corner at a point on screen.
    fn add_frame_at(&mut self, screen: egui::Pos2) {
        let rect = egui::Rect::from_min_size(self.screen_to_patch(screen), NEW_FRAME_SIZE);
        self.add_frame(rect, Tint::default());
        self.status_message = Some("Name the frame, then press Enter".to_string());
    }

    /// Adds a frame, selects it, and opens its title for naming. Naming it
    /// is part of the same undo step.
    fn add_frame(&mut self, rect: egui::Rect, tint: Tint) {
        let annotations = &mut self.user_state.annotations;
        let id = annotations.add(Annotation::Frame(Frame { title: String::new(), rect, tint }));
        annotations.start_editing(id, true);
        self.select(Vec::new(), vec![id]);
    }

    /// Adds a note at a point on screen, open for writing. A note left
    /// empty goes away again.
    fn add_note_at(&mut self, screen: egui::Pos2) {
        let position = self.screen_to_patch(screen);
        let annotations = &mut self.user_state.annotations;
        let id = annotations.add(Annotation::Note(Note { text: String::new(), position, width: DEFAULT_NOTE_WIDTH }));
        annotations.start_editing(id, false);
        self.select(Vec::new(), vec![id]);
    }

    fn handle_node_menu(&mut self, ctx: &egui::Context, action: NodeMenuAction) {
        match action {
            NodeMenuAction::Select(node_id) => {
                if !self.graph_state.selected_nodes.contains(&node_id) {
                    self.graph_state.selected_nodes = vec![node_id];
                }
            }
            NodeMenuAction::Duplicate(node_id) => self.duplicate_selection(&self.menu_targets(node_id)),
            NodeMenuAction::Copy(node_id) => self.copy_selection(ctx, &self.menu_targets(node_id)),
            NodeMenuAction::Reset(node_id) => self.reset_modules(&self.menu_targets(node_id).nodes),
            NodeMenuAction::Delete(node_id) => self.delete_selection(&self.menu_targets(node_id), "Delete", "Deleted"),
            NodeMenuAction::Group(node_id) => self.group_selection(ctx, &self.menu_targets(node_id).nodes),
            NodeMenuAction::Ungroup(node_id) => self.ungroup(&self.menu_targets(node_id).nodes),
            NodeMenuAction::Enter(node_id) => self.enter_group(node_id),
            NodeMenuAction::StartRename(node_id) => self.start_rename(node_id),
            NodeMenuAction::Rename(node_id, name) => self.rename_group(node_id, &name),
            NodeMenuAction::Pin(node_id, param_name, levels) => self.pin_knob(node_id, &param_name, levels),
            NodeMenuAction::NameFace(node_id, param_name, name) => self.name_face_control(node_id, &param_name, &name),
            NodeMenuAction::Jack(node_id, side, port, edit) => self.edit_jack(node_id, side, &port, edit),
            NodeMenuAction::SaveToLibrary(node_id) => self.save_to_library(node_id),
            NodeMenuAction::OpenSample(node_id) => self.open_sample_dialog(node_id),
        }
    }

    /// Deletes modules with their cables, and frames and notes. `verb` names
    /// the undo step and `done` the status message: "Delete" and "Deleted",
    /// or "Cut" and "Cut".
    fn delete_selection(&mut self, selection: &Selection, verb: &str, done: &str) {
        if selection.is_empty() {
            return;
        }
        let what = editing::describe(&self.graph_state, &self.user_state, selection);
        for cmd in editing::delete_selection(&mut self.graph_state, &mut self.user_state, selection) {
            self.send_command(cmd);
        }
        self.history.name_next(format!("{verb} {what}"));
        self.status_message = Some(format!("{done} {what}"));
    }

    /// Duplicates modules, with the cables between them, and frames and
    /// notes, and selects the copies.
    fn duplicate_selection(&mut self, selection: &Selection) {
        let what = editing::describe(&self.graph_state, &self.user_state, selection);
        let Some(pasted) = editing::duplicate(&mut self.graph_state, &mut self.user_state, selection) else {
            return;
        };
        self.finish_paste(pasted, &format!("Duplicate {what}"));
        self.status_message = Some(format!("Duplicated {what}"));
    }

    /// Puts modules' knobs back to their defaults.
    fn reset_modules(&mut self, nodes: &[egui_node_graph2::NodeId]) {
        let mut changed = false;
        for &node_id in nodes {
            changed |= editing::reset_parameters(&mut self.graph_state, node_id);
        }
        let what = editing::describe_modules(&self.graph_state, nodes);
        if changed {
            self.history.name_next(format!("Reset {what}"));
            self.status_message = Some(format!("Reset {what} to defaults"));
        } else {
            self.status_message = Some(format!("{what} already at defaults"));
        }
    }

    /// Puts modules and the cables between them, and frames and notes, on
    /// the clipboard as patch JSON. They paste back into this window or
    /// another one.
    fn copy_selection(&mut self, ctx: &egui::Context, selection: &Selection) {
        let Some(patch) = editing::copy_selection(&self.graph_state, &self.user_state, selection) else {
            return;
        };
        match serde_json::to_string_pretty(&patch) {
            Ok(json) => {
                ctx.copy_text(json);
                self.last_paste = None;
                let what = editing::describe(&self.graph_state, &self.user_state, selection);
                self.status_message = Some(format!("Copied {what}"));
            }
            Err(e) => self.status_message = Some(format!("Couldn't copy: {e}")),
        }
    }

    /// Pastes modules from clipboard text at the cursor.
    fn paste_modules(&mut self, ctx: &egui::Context, text: &str) {
        let Ok(patch) = crate::persistence::patch_from_json(text) else {
            self.status_message = Some("Nothing to paste: the clipboard doesn't hold modules".to_string());
            return;
        };
        let aim = self.screen_to_node(self.cursor_or_center(ctx));
        // Pasting again at the same spot fans out, like duplicating
        let at = match self.last_paste {
            Some((last_aim, landed)) if (last_aim - aim).length() < 1.0 => {
                landed + editing::DUPLICATE_OFFSET * self.graph_state.pan_zoom.zoom
            }
            _ => aim,
        };
        match editing::paste(&mut self.graph_state, &mut self.user_state, &patch, at) {
            Ok(pasted) if !pasted.nodes.is_empty() || !pasted.annotations.is_empty() => {
                self.last_paste = Some((aim, at));
                let pasted_selection = Selection { nodes: pasted.nodes.clone(), annotations: pasted.annotations.clone() };
                let what = editing::describe(&self.graph_state, &self.user_state, &pasted_selection);
                if !pasted.warnings.is_empty() {
                    self.load_warnings = pasted.warnings.clone();
                }
                self.finish_paste(pasted, &format!("Paste {what}"));
                self.status_message = Some(format!("Pasted {what}"));
            }
            Ok(_) => self.status_message = Some("Nothing to paste: no modules this version knows".to_string()),
            Err(e) => self.status_message = Some(format!("Couldn't paste: {e}")),
        }
    }

    /// Sends a paste's commands, selects what it added, and names the undo step.
    fn finish_paste(&mut self, pasted: editing::Pasted, label: &str) {
        for cmd in pasted.commands {
            self.send_command(cmd);
        }
        self.select(pasted.nodes, pasted.annotations);
        self.history.name_next(label);
    }

    /// Sync parameter values from the graph UI to the audio engine.
    ///
    /// This iterates through all nodes and their parameters, compares against
    /// cached values, and sends SetParameter commands for any changes.
    fn sync_parameters(&mut self) {
        let mut commands_to_send: Vec<EngineCommand> = Vec::new();

        // Iterate through all nodes
        for (node_id, node) in self.graph_state.graph.nodes.iter() {
            // Get the engine node ID for this graph node
            let Some(engine_node_id) = self.user_state.get_engine_node_id(node_id) else {
                continue;
            };

            // Keyboard and MIDI Note drive their leading params (Note, Gate, ...)
            // from live input, not from the graph UI
            let live_params = port_mapping::live_input_parameter_count(node.user_data.module_id);

            // Track which param index we're at (only count ConstantOnly params)
            let mut param_index = 0;

            // Iterate through inputs to find parameters
            for (_param_name, input_id) in &node.inputs {
                let input = self.graph_state.graph.get_input(*input_id);

                // Only process ConstantOnly or ConnectionOrConstant params
                use egui_node_graph2::InputParamKind;
                match input.kind {
                    InputParamKind::ConstantOnly | InputParamKind::ConnectionOrConstant => {
                        if param_index < live_params {
                            param_index += 1;
                            continue;
                        }

                        // The engine takes real units: Hz, seconds, dB, ...
                        let actual_value = input.value.actual_value();

                        // Create cache key
                        let cache_key = (engine_node_id, param_index);

                        // Check if value has changed (use relative tolerance for large values like frequency).
                        // Whole numbers change by whole steps, and a sequencer's step packs its
                        // settings into digits, so they're compared exactly
                        let whole = matches!(&input.value, crate::graph::SynthValueType::Number { spec, .. } if spec.stepped);
                        let needs_update = match self.cached_params.get(&cache_key) {
                            Some(&cached_value) => {
                                let diff = (actual_value - cached_value).abs();
                                let threshold = if whole {
                                    0.0
                                } else if actual_value.abs() > 10.0 {
                                    actual_value.abs() * 0.0001 // Relative tolerance for large values
                                } else {
                                    0.0001 // Absolute tolerance for small values
                                };
                                diff > threshold
                            }
                            None => true, // New parameter, needs initial sync
                        };

                        if needs_update {
                            commands_to_send.push(EngineCommand::SetParameter {
                                node_id: engine_node_id,
                                param_index,
                                value: actual_value,
                            });
                            self.cached_params.insert(cache_key, actual_value);
                        }

                        param_index += 1;
                    }
                    InputParamKind::ConnectionOnly => {
                        // This is a connection-only port, not a parameter
                        // Don't increment param_index
                    }
                }
            }
        }

        // Send collected commands
        for cmd in commands_to_send {
            self.send_command(cmd);
        }
    }

    /// Create a Patch from the current graph state.
    fn create_patch(&self, name: &str) -> Patch {
        // Zooming rescales every node position in the editor, so positions are
        // saved in patch space, which doesn't move with the view. A patch
        // opens at zoom 1 with patch (0, 0) at the editor's origin, so they
        // come back exactly where they were, along with the frames and notes
        // that are kept in the same space.
        let zoom = self.graph_state.pan_zoom.zoom;
        let position = |node_id| {
            self.graph_state.node_positions
                .get(node_id)
                .map(|&pos| {
                    let p = self.history.to_patch(pos, zoom);
                    (p.x, p.y)
                })
                .unwrap_or((0.0, 0.0))
        };

        let mut patch = capture_patch(
            name,
            &self.graph_state.graph,
            |node_id| self.user_state.get_engine_node_id(node_id),
            position,
            &self.midi_mappings,
        );
        let annotations = &self.user_state.annotations;
        let all: Vec<AnnotationId> = annotations.iter().map(|(id, _)| id).collect();
        (patch.frames, patch.notes) = annotations.to_patch(&all, egui::Vec2::ZERO);
        patch
    }

    /// Load a patch, replacing the current graph.
    ///
    /// The whole patch is built into a staging graph first, and the current
    /// graph is only replaced once that has succeeded. Anything that couldn't
    /// be restored is skipped and returned as warnings.
    ///
    /// Samplers' files saved relative to the patch are found from `base`.
    fn load_patch(&mut self, patch: &Patch, base: SampleBase) -> Result<Vec<String>, PatchError> {
        let mut staged = stage_patch(patch)?;

        // Stop playback during load
        let was_playing = self.is_playing;
        if was_playing {
            self.is_playing = false;
            self.user_state.is_playing = false;
            self.send_command(EngineCommand::SetPlaying(false));
        }

        // Clear the current graph
        self.clear_graph();

        // Reset pan/zoom to default (zoom=1.0, pan=0) before loading positions.
        // This is critical because the library's update_node_positions_after_zoom
        // mutates node_positions based on the current zoom level. Loading positions
        // at a different zoom than they were saved at would cause layout drift.
        self.graph_state.pan_zoom = egui_node_graph2::PanZoom::default();
        self.history.reset_view();

        // Swap in the staged graph. Its node IDs stay valid; its groups get
        // IDs from this session's count
        self.graph_state.graph = std::mem::take(&mut staged.graph);
        renumber_groups(&mut self.graph_state.graph, || self.user_state.allocate_group_id());
        for part in &staged.parts {
            self.graph_state.node_positions.insert(part.graph_id, egui::pos2(part.position.0, part.position.1));
            self.graph_state.node_order.push(part.graph_id);
        }

        for node in &staged.nodes {
            let pos = egui::pos2(node.position.0, node.position.1);
            self.graph_state.node_positions.insert(node.graph_id, pos);
            self.graph_state.node_order.push(node.graph_id);

            // Engine IDs keep counting across loads, so they differ from the
            // IDs in the patch file. MIDI mappings are remapped below.
            let engine_node_id = self.user_state.allocate_engine_node_id(node.graph_id);

            // Create the module in the audio engine. Parameter values follow
            // with the next parameter sync.
            for cmd in engine_sync::add_module(&self.graph_state.graph, node.graph_id, engine_node_id) {
                self.send_command(cmd);
            }
        }

        // Samplers' files, found from where the patch is
        let sample_warnings = self.resolve_samples(&staged.nodes, base);
        staged.warnings.extend(sample_warnings);

        // Frames and notes are already in patch space
        self.user_state.annotations.add_from_patch(&patch.frames, &patch.notes, egui::Vec2::ZERO);

        // The staged connections, module to module, for the engine
        self.sync_cables();

        // Load MIDI mappings, retargeted from the patch's node IDs to the new ones
        self.midi_mappings = staged.remap_midi_mappings(|graph_id| self.user_state.get_engine_node_id(graph_id));
        // Sync mappings to user state for UI display
        for mapping in &self.midi_mappings {
            self.user_state.set_midi_mapping(
                mapping.node_id,
                mapping.param_index,
                mapping.cc_number,
                mapping.channel,
            );
        }

        // Restore playback state
        if was_playing {
            self.is_playing = true;
            self.user_state.is_playing = true;
            self.send_command(EngineCommand::SetPlaying(true));
        }

        // A loaded patch starts its own history, with nothing unsaved, and
        // its Loopers hold the loops it was saved with
        self.history.reset(&self.graph_state, &self.user_state);
        self.mark_saved();
        self.opened_loops();

        Ok(staged.warnings)
    }

    /// Clear the entire graph.
    fn clear_graph(&mut self) {
        // Send clear command to audio engine
        self.send_command(EngineCommand::ClearGraph);

        // Clear graph state
        self.graph_state.graph = egui_node_graph2::Graph::default();
        self.graph_state.node_positions.clear();
        self.graph_state.node_order.clear();

        // Clear user state (also clears MIDI mapping UI state)
        self.user_state.clear();

        // Clear cached parameters
        self.cached_params.clear();

        // Clear MIDI mappings
        self.midi_mappings.clear();
        self.midi_learn_target = None;
    }

    /// Start a new patch - clears the graph and resets the current file path.
    fn new_patch(&mut self) {
        self.clear_graph();
        self.history.reset(&self.graph_state, &self.user_state);
        self.mark_saved();
        self.opened_loops();
        self.load_warnings.clear();
        self.current_patch_path = None;
        self.current_example = None;
        self.status_message = Some("New patch created".to_string());
    }

    /// Show a save file dialog and save the current patch. Returns whether
    /// it was saved, not cancelled or failed.
    fn show_save_dialog(&mut self) -> bool {
        let default_name = self.current_patch_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .or(self.current_example.map(|e| e.file_name))
            .unwrap_or("patch.json")
            .to_string();

        // The browser saves by downloading
        #[cfg(target_arch = "wasm32")]
        let saved = self.download_patch(&default_name);
        #[cfg(not(target_arch = "wasm32"))]
        let saved = self.save_patch_as(&default_name);
        saved
    }

    /// Asks where to save the patch, offering `default_name`, and saves it there.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_patch_as(&mut self, default_name: &str) -> bool {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Synth Patch", &["json"])
            .set_file_name(default_name)
            .save_file()
        {
            // Derive patch name from filename
            let name = path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled");

            // Samples from elsewhere go beside the patch, so its folder
            // holds everything, and the loops go there too
            let copy_problem = self.gather_samples(&path);
            let (patch, loop_problems) = self.patch_with_loops(name, &path);
            match save_to_file(&patch, &path) {
                Ok(()) => {
                    let problems: Vec<String> = copy_problem.into_iter().chain(loop_problems).collect();
                    if !problems.is_empty() {
                        self.load_warnings = problems;
                    }
                    self.current_patch_path = Some(path.clone());
                    self.current_example = None;
                    self.status_message = Some(format!("Saved: {}", path.display()));
                    self.saved_as(&path);
                    return true;
                }
                Err(e) => {
                    self.status_message = Some(format!("Save failed: {}", e));
                }
            }
        }
        false
    }

    /// Saves the patch as a download named `file_name`.
    #[cfg(target_arch = "wasm32")]
    fn download_patch(&mut self, file_name: &str) -> bool {
        let name = file_name.strip_suffix(".json").unwrap_or(file_name);
        let patch = self.create_patch(name);
        match patch_to_json(&patch).map_err(|e| e.to_string()).and_then(|json| web::download(file_name, &json)) {
            Ok(()) => {
                self.status_message = Some(format!("Downloaded {}", file_name));
                self.sync_history();
                self.mark_saved();
                true
            }
            Err(e) => {
                self.status_message = Some(format!("Save failed: {}", e));
                false
            }
        }
    }

    /// Show a load file dialog and load the selected patch. In the browser
    /// the file is uploaded, and opens when it arrives (see `collect_upload`).
    fn show_load_dialog(&mut self, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        web::pick_patch(ctx, self.uploads.0.clone());

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Synth Patch", &["json"])
                .pick_file()
            {
                self.open_file(&path);
            }
        }
    }

    /// In the browser, zooms a just-opened patch out to fit the view once
    /// its modules are drawn (see `fit_level`). On the desktop, a patch
    /// opens where it was laid out.
    fn fit_on_web(&mut self) {
        if WEB {
            // A few frames for its modules to be drawn and measured
            self.fit_pending = 6;
        }
    }

    /// Opens a patch file the browser has finished uploading.
    #[cfg(target_arch = "wasm32")]
    fn collect_upload(&mut self) {
        let Ok(upload) = self.uploads.1.try_recv() else { return };
        let loaded = upload
            .text
            .and_then(|json| patch_from_json(&json).map_err(|e| e.to_string()))
            .and_then(|patch| Ok((self.load_patch(&patch, SampleBase::Keys).map_err(|e| e.to_string())?, patch.name)));
        match loaded {
            Ok((warnings, name)) => {
                self.fit_on_web();
                // Remembered by name only, to offer when it's saved again
                self.current_patch_path = Some(PathBuf::from(&upload.name));
                self.current_example = None;
                self.status_message = Some(format!("Loaded: {}", name));
                self.show_load_warnings(warnings);
            }
            Err(e) => self.status_message = Some(format!("Couldn't open {}: {}", upload.name, e)),
        }
    }

    /// Load the patch file at `path`, replacing the current graph.
    fn open_file(&mut self, path: &Path) {
        let folder = path.parent().unwrap_or(Path::new("")).to_path_buf();
        match load_from_file(path).and_then(|patch| Ok((self.load_patch(&patch, SampleBase::Folder(&folder))?, patch.name))) {
            Ok((warnings, name)) => {
                self.current_patch_path = Some(path.to_path_buf());
                self.current_example = None;
                self.status_message = Some(format!("Loaded: {}", name));
                self.show_load_warnings(warnings);
                self.recent_files.push(path);
            }
            Err(e) => {
                self.status_message = Some(format!("Load failed: {}", e));
                // A file that's gone stops being offered
                if !path.exists() {
                    self.recent_files.remove(path);
                }
            }
        }
    }

    /// Open one of the bundled example patches. It has no file, so saving
    /// it asks where to put the copy.
    fn open_example(&mut self, example: &'static Example) {
        match example.patch().and_then(|patch| self.load_patch(&patch, SampleBase::Example)) {
            Ok(warnings) => {
                self.fit_on_web();
                self.current_patch_path = None;
                self.current_example = Some(example);
                self.status_message = Some(format!("Opened example: {}", example.name));
                self.show_load_warnings(warnings);
            }
            Err(e) => {
                self.status_message = Some(format!("Couldn't open example {}: {}", example.name, e));
            }
        }
    }

    /// Opens the patch the app starts with: the file named on the command
    /// line, or else the First Sound example, so the canvas is never empty.
    pub fn open_on_launch(&mut self, path: Option<&Path>) {
        match path {
            Some(path) => self.open_file(path),
            None => self.open_example(examples::first_sound()),
        }
    }

    /// Zooms and pans the view with a pinch, as on a map: two fingers on a
    /// touch screen, or a trackpad pinch (or Ctrl+scroll), about the point
    /// between the fingers or under the pointer. The node graph zooms about
    /// its middle on its own, with the wheel.
    fn follow_gestures(&mut self, ui: &egui::Ui, editor_rect: egui::Rect) {
        let (zoom, touch, pointer) = ui.input(|i| (i.zoom_delta(), i.multi_touch(), i.pointer.hover_pos()));
        let focus = touch.as_ref().map(|t| t.center_pos).or(pointer);
        let Some(focus) = focus.filter(|at| editor_rect.contains(*at)) else { return };
        // Zoom first: frames and notes follow a zoom from where the view was
        if zoom != 1.0 {
            let before = self.graph_state.pan_zoom.zoom;
            self.graph_state.zoom(ui, zoom);
            let scale = self.graph_state.pan_zoom.zoom / before;
            // Zoomed about the editor's middle; moved so the point under the
            // fingers stays put
            let middle = self.graph_state.pan_zoom.clip_rect.size() / 2.0;
            self.graph_state.pan_zoom.pan += (focus - editor_rect.min - middle) * (1.0 - scale);
        }
        if let Some(touch) = touch {
            self.graph_state.pan_zoom.pan += touch.translation_delta;
        }
    }

    /// On a touch screen, a finger dragged across empty canvas pans rather
    /// than drawing a selection box. (Two fingers pan in `follow_gestures`.)
    fn follow_touch_pan(&mut self, ctx: &egui::Context) {
        let (touching, pinching, held, delta) =
            ctx.input(|i| (i.any_touches(), i.multi_touch().is_some(), i.pointer.primary_down(), i.pointer.delta()));
        if touching && self.graph_state.ongoing_box_selection.take().is_some() {
            self.touch_panning = true;
        }
        if !held {
            self.touch_panning = false;
        } else if self.touch_panning && !pinching {
            self.graph_state.pan_zoom.pan += delta;
        }
    }

    /// Opens what the page's address asks for: `?patch=lush-pad` embeds that
    /// example, just the canvas and a Play button, for a page to frame;
    /// `?open=lush-pad` opens it in the full app. Returns whether the address
    /// named an example.
    pub fn open_from_address(&mut self, patch: Option<&str>, open: Option<&str>) -> bool {
        let find = |name: &str| EXAMPLES.iter().find(|e| e.file_name.strip_suffix(".json") == Some(name));
        let Some(example) = patch.or(open).and_then(find) else { return false };
        self.embedded = patch.is_some();
        self.open_example(example);
        true
    }

    /// An embed's controls: Play, what's playing, and the way to the full
    /// app. Returns whether Play or Stop was pressed.
    fn draw_embed_bar(&mut self, ctx: &egui::Context) -> bool {
        let status = self.audio_status(ctx);
        let mut toggle = false;
        egui::Area::new(egui::Id::new("embed_bar"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(theme::background::PANEL)
                    .stroke(egui::Stroke::new(1.0, theme::background::PANEL.gamma_multiply(1.6)))
                    .rounding(10.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 4.0),
                        blur: 16.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(90),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (text, color) = if self.is_playing {
                                ("⏹ Stop", theme::accent::WARNING)
                            } else {
                                ("▶ Play", theme::accent::SUCCESS)
                            };
                            toggle = ui.button(RichText::new(text).color(color).strong()).clicked();
                            ui.add_space(4.0);
                            let name = self.current_example.map_or("Soba", |e| e.name);
                            let label = ui.label(RichText::new(name).color(theme::text::PRIMARY).strong());
                            if let Some(example) = self.current_example {
                                label.on_hover_text(example.description);
                            }
                            match status {
                                _ if self.audio_engine.is_err() => {
                                    ui.label(RichText::new("⚠ No sound in this browser").color(theme::accent::ERROR).small());
                                }
                                AudioStatus::NoAudio => {
                                    ui.label(RichText::new("⚠ No audio").color(theme::accent::ERROR).small());
                                }
                                _ if self.user_state.samples.waiting() => {
                                    ui.label(RichText::new("loading samples…").color(theme::text::SECONDARY).small());
                                }
                                // The computer's keys play Keyboard and Poly MIDI modules
                                _ if self.has_keyboard_modules() || self.has_poly_midi_modules() => {
                                    ui.label(RichText::new("play its piano, or the Z–M keys").color(theme::text::SECONDARY).small());
                                }
                                _ => {}
                            }
                            #[cfg(target_arch = "wasm32")]
                            if let Some(app) = web::full_app_url() {
                                ui.add_space(4.0);
                                let file = self.current_example.map_or("", |e| e.file_name.trim_end_matches(".json"));
                                // A new tab: in the frame it would replace the page's embed
                                ui.add(egui::Hyperlink::from_label_and_url(
                                    RichText::new("Open in Soba ↗").color(theme::text::SECONDARY).small(),
                                    format!("{}?open={}", app, file),
                                ).open_in_new_tab(true))
                                .on_hover_text("The whole app, with this patch, in a new tab");
                            }
                        });
                    });
            });
        toggle
    }

    /// Keep the problems from a load on screen (and on stderr) until dismissed.
    fn show_load_warnings(&mut self, warnings: Vec<String>) {
        for warning in &warnings {
            eprintln!("Patch load warning: {}", warning);
        }
        self.load_warnings = warnings;
    }

    /// Quick save to the current path, or show save dialog if no path.
    /// Returns whether the patch was saved.
    fn quick_save(&mut self) -> bool {
        // In the browser the patch's path is just a name: saving downloads
        if let Some(path) = self.current_patch_path.clone().filter(|_| !WEB) {
            let name = path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled");

            let (patch, loop_problems) = self.patch_with_loops(name, &path);
            match save_to_file(&patch, &path) {
                Ok(()) => {
                    if !loop_problems.is_empty() {
                        self.load_warnings = loop_problems;
                    }
                    self.status_message = Some(format!("Saved: {}", path.display()));
                    self.saved_as(&path);
                    true
                }
                Err(e) => {
                    self.status_message = Some(format!("Save failed: {}", e));
                    false
                }
            }
        } else {
            self.show_save_dialog()
        }
    }

    /// Notes a successful save to `path`.
    fn saved_as(&mut self, path: &Path) {
        // Edits made this frame are in the file, so they're in the history first
        self.sync_history();
        self.mark_saved();
        self.recent_files.push(path);
    }

    /// Notes that the patch as it stands is what was last opened or saved.
    fn mark_saved(&mut self) {
        self.history.mark_saved();
        self.saved_midi_mappings = self.midi_mappings.clone();
    }

    /// Records any edit not yet in the undo history, so it counts as a change.
    fn sync_history(&mut self) {
        self.history.record(&self.graph_state, &self.user_state, false, Instant::now());
    }

    /// Whether the patch has changes that aren't saved anywhere, a loop
    /// recorded among them.
    fn has_unsaved_changes(&self) -> bool {
        self.history.has_unsaved_changes() || self.midi_mappings != self.saved_midi_mappings || self.loops_unsaved()
    }

    /// The patch's name: its file's, its example's, or "Untitled".
    fn patch_title(&self) -> String {
        match (&self.current_patch_path, self.current_example) {
            (Some(path), _) => path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string(),
            (None, Some(example)) => example.name.to_string(),
            (None, None) => "Untitled".to_string(),
        }
    }

    /// Does `action` now if nothing would be lost, or asks first.
    fn request(&mut self, ctx: &egui::Context, action: Discard) {
        self.sync_history();
        // Restarting would stop a take, so that waits for the take to end
        if action.restarts() && self.is_recording() {
            self.raise_notice("Soba restarts to update, so that waits until the recording ends".to_string());
            return;
        }
        // Quitting stops a take, so that asks too
        let stops_take = matches!(action, Discard::Quit) && self.is_recording();
        if self.has_unsaved_changes() || stops_take {
            self.pending_discard = Some(action);
        } else {
            self.perform(ctx, action);
        }
    }

    /// Does something that replaces or closes the patch, once its unsaved
    /// changes are saved or let go.
    fn perform(&mut self, ctx: &egui::Context, action: Discard) {
        match action {
            Discard::New => self.new_patch(),
            Discard::Open => self.show_load_dialog(ctx),
            Discard::OpenFile(path) => self.open_file(&path),
            Discard::OpenExample(example) => self.open_example(example),
            Discard::Quit => {
                self.finish_recording_now();
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            #[cfg(not(target_arch = "wasm32"))]
            Discard::Update => self.restart_for_update(ctx),
            #[cfg(not(target_arch = "wasm32"))]
            Discard::RollBack => self.roll_back(ctx),
        }
    }

    /// Shows whichever prompt is waiting, and acts on its answer.
    fn show_prompts(&mut self, ctx: &egui::Context) {
        if let Some(action) = self.pending_discard.clone() {
            let unsaved = self.has_unsaved_changes();
            let take = match (&action, &self.recording) {
                (Discard::Quit, Some(take)) if !take.is_stopping() => Some(recording::clock(take.elapsed())),
                _ => None,
            };
            if let Some(answer) = session::unsaved_changes_prompt(ctx, &self.patch_title(), &action, unsaved, take.as_deref()) {
                self.pending_discard = None;
                match answer {
                    // A cancelled save dialog cancels the whole thing
                    Answer::Save => {
                        if self.quick_save() {
                            self.perform(ctx, action);
                        }
                    }
                    Answer::Discard => self.perform(ctx, action),
                    Answer::Cancel => {}
                }
            }
        } else if let Some(autosave) = &self.recovery {
            if let Some(recover) = session::recovery_prompt(ctx, autosave) {
                let autosave = self.recovery.take().expect("shown above");
                if recover {
                    self.recover(&autosave);
                }
            }
        }
    }

    /// Whether a prompt is up, so shortcuts wait.
    fn prompt_open(&self) -> bool {
        self.pending_discard.is_some() || self.recovery.is_some()
    }

    /// Brings back the patch an autosave holds. It's still unsaved.
    fn recover(&mut self, autosave: &Autosave) {
        // An autosave keeps samples by key, as the editor does
        match autosave.patch().and_then(|patch| self.load_patch(&patch, SampleBase::Keys)) {
            Ok(warnings) => {
                self.current_patch_path = autosave.path.clone();
                self.current_example = autosave.example.as_deref()
                    .and_then(|name| EXAMPLES.iter().find(|e| e.name == name));
                self.history.mark_unsaved();
                self.status_message = Some(format!("Recovered unsaved changes to {}", autosave.name));
                self.show_load_warnings(warnings);
            }
            Err(e) => {
                self.status_message = Some(format!("Couldn't recover {}: {}", autosave.name, e));
            }
        }
    }

    /// Picks up the last session: its recent files, and any autosave a crash
    /// left, which is offered back on the first frame.
    pub fn restore_session(&mut self, storage: Option<&dyn eframe::Storage>) {
        self.recent_files = RecentFiles::load(storage);
        #[cfg(not(target_arch = "wasm32"))]
        self.updater.load(storage);
        self.recovery = Autosave::load(storage);
        if let Some(glyph) = storage.and_then(|s| s.get_string(FLOW_GLYPH_KEY)) {
            self.user_state.flow_glyph = FlowGlyph::ALL
                .into_iter()
                .find(|g| g.name() == glyph)
                .unwrap_or_default();
        }
        self.recordings_folder = storage
            .and_then(|s| s.get_string(recording::FOLDER_KEY))
            .filter(|folder| !folder.is_empty())
            .map(PathBuf::from);
        if let Some(style) = storage.and_then(|s| s.get_string(KNOB_STYLE_KEY)) {
            self.user_state.knob_style = KnobStyle::ALL
                .into_iter()
                .find(|k| k.name() == style)
                .unwrap_or_default();
        }
        // The audio system and buffer chosen last time
        let buffer = storage
            .and_then(|s| s.get_string(BUFFER_FRAMES_KEY))
            .and_then(|frames| frames.parse::<u32>().ok());
        let system = storage
            .and_then(|s| s.get_string(AUDIO_SYSTEM_KEY))
            .and_then(|key| AudioSystem::from_key(&key));
        if let Ok(engine) = self.audio_engine.as_mut() {
            let _ = engine.set_buffer_size(buffer);
        }
        if let Some(system) = system {
            self.select_audio_system(system);
        }
    }

    /// Validate a connection and return an error message if invalid.
    ///
    /// Returns None if the connection is valid, Some(error_msg) if invalid.
    fn validate_and_check_connection(
        &self,
        output: egui_node_graph2::OutputId,
        input: egui_node_graph2::InputId,
    ) -> Option<String> {
        // Get the signal types for both ports
        let output_type = self.graph_state.graph
            .any_param_type(AnyParameterId::Output(output))
            .ok()
            .map(|dt| dt.signal_type());

        let input_type = self.graph_state.graph
            .any_param_type(AnyParameterId::Input(input))
            .ok()
            .map(|dt| dt.signal_type());

        match (output_type, input_type) {
            (Some(from_type), Some(to_type)) => {
                let result = validate_connection(from_type, to_type);
                if !result.is_valid() {
                    result.error_message().map(|s| s.to_string())
                } else {
                    // An Arranger follows only another's Section, and not round in a loop
                    crate::graph::arranger_follow::refusal(&self.graph_state.graph, output, input)
                }
            }
            _ => {
                // Couldn't get types - shouldn't happen
                Some("Could not determine port types".to_string())
            }
        }
    }

    /// Check if a connection between two nodes would create a self-loop.
    #[allow(dead_code)]
    fn is_self_connection(
        &self,
        output: egui_node_graph2::OutputId,
        input: egui_node_graph2::InputId,
    ) -> bool {
        let output_node = self.graph_state.graph.get_output(output).node;
        let input_node = self.graph_state.graph.get_input(input).node;
        output_node == input_node
    }

    /// Draw the bottom status bar
    /// What the audio output is doing: the transport's state, unless the
    /// device has reported an error or stopped asking for audio.
    fn audio_status(&mut self, ctx: &egui::Context) -> AudioStatus {
        let transport = if self.is_playing { AudioStatus::Playing } else { AudioStatus::Stopped };
        // Offline (filming, or no stream to watch): the transport is the whole story
        let Ok(engine) = &self.audio_engine else { return transport };
        if !engine.is_running() {
            return transport;
        }
        #[cfg(target_arch = "wasm32")]
        if web::audio_blocked() {
            return AudioStatus::Waiting;
        }

        let now = Instant::now();
        let count = engine.callback_count();
        if count != self.audio_heartbeat.0 {
            self.audio_heartbeat = (count, now);
        }
        // Keep looking even when nothing else redraws, so a lost device shows
        ctx.request_repaint_after(AUDIO_STALL);

        if engine.stream_failed() || now - self.audio_heartbeat.1 > AUDIO_STALL {
            AudioStatus::NoAudio
        } else {
            transport
        }
    }

    fn draw_status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);

            // Priority order: status message > validation message > audio error > default status
            if let Some(ref status_msg) = self.status_message {
                // Show status message (from save/load)
                ui.label(RichText::new(status_msg)
                    .color(theme::accent::SUCCESS)
                    .small());
            } else if let Some((notice, _)) = &self.notice {
                let label = ui.add(
                    egui::Label::new(RichText::new(format!("⚠ {}", notice)).color(theme::accent::WARNING).small())
                        .sense(egui::Sense::click()),
                );
                if label.on_hover_text("Click to dismiss").clicked() {
                    self.notice = None;
                }
            } else if let Some(validation_msg) = self.user_state.validation_message() {
                // Show validation error with warning icon
                ui.label(RichText::new(format!("⚠ {}", validation_msg))
                    .color(theme::accent::WARNING)
                    .small());
            } else if let Some(ref error) = self.audio_error_message {
                // Show audio error
                ui.label(RichText::new(format!("⚠ {}", error))
                    .color(theme::accent::ERROR)
                    .small());
            } else if let Some(ref error) = self.midi_error_message {
                ui.label(RichText::new(format!("⚠ {}", error))
                    .color(theme::accent::ERROR)
                    .small());
            } else {
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

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);

                // Engine status at the far right: what the output is doing, format, load
                let status = self.audio_status(ui.ctx());
                if let Ok(engine) = &self.audio_engine {
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
                    if let Some(monitor) = &self.input_monitor {
                        let glitches = monitor.underrun_frames() + monitor.overflow_frames() + monitor.device_xruns();
                        let now = Instant::now();
                        if glitches != self.input_glitches.0 {
                            self.input_glitches = (glitches, now);
                        }
                        let recent = glitches > 0 && now - self.input_glitches.1 < INPUT_GLITCH_HOLD;
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
                    if self.is_playing {
                        cpu_meter(ui, self.cpu_load, &CpuMeterConfig::compact());
                    }
                    ui.label(RichText::new("|")
                        .color(theme::text::DISABLED)
                        .small());
                }

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
                let patch_name = match (&self.current_patch_path, self.current_example) {
                    (Some(path), _) => path.file_stem().and_then(|s| s.to_str()).map(str::to_string),
                    (None, Some(example)) => Some(format!("{} (example)", example.name)),
                    (None, None) => self.has_unsaved_changes().then(|| "Untitled".to_string()),
                };
                if let Some(name) = patch_name {
                    // Right to left: the dot sits after the name
                    if self.has_unsaved_changes() {
                        ui.label(RichText::new("●").color(theme::accent::WARNING).small())
                            .on_hover_text("Unsaved changes (Ctrl+S to save)");
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
                    ui.label(RichText::new(concat!("Soba v", env!("CARGO_PKG_VERSION"))).color(theme::text::DISABLED).small());
                }
            });
        });
    }

    /// Delete, duplicate, copy, cut, paste, Ctrl+Shift+F to frame the
    /// selection, Space or Tab for the quick-add palette, and Escape to leave
    /// MIDI Learn.
    fn handle_editing_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Event, Key, KeyboardShortcut, Modifiers};
        if self.is_midi_learning() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.cancel_midi_learn();
        }
        let mut copy = false;
        let mut cut = false;
        let mut paste = None;
        let (delete, duplicate, palette, frame) = ctx.input_mut(|i| {
            let delete = i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace);
            let duplicate = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::D));
            let frame = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::F));
            let palette = i.consume_key(Modifiers::NONE, Key::Space) || i.consume_key(Modifiers::NONE, Key::Tab);
            // The window turns Ctrl+C, Ctrl+X and Ctrl+V into these, not key presses
            i.events.retain(|event| match event {
                Event::Copy => { copy = true; false }
                Event::Cut => { cut = true; false }
                Event::Paste(text) => { paste = Some(text.clone()); false }
                // The Space that opens the palette isn't typed into it
                Event::Text(text) if palette && text == " " => false,
                _ => true,
            });
            (delete, duplicate, palette, frame)
        });

        let selected = self.selection();
        if copy || cut {
            self.copy_selection(ctx, &selected);
        }
        if cut {
            self.delete_selection(&selected, "Cut", "Cut");
        } else if delete {
            self.delete_selection(&selected, "Delete", "Deleted");
        }
        if duplicate {
            self.duplicate_selection(&selected);
        }
        if frame {
            self.frame_selection(ctx);
        }
        if let Some(text) = paste {
            self.paste_modules(ctx, &text);
        }
        if palette {
            self.open_quick_add(ctx);
        }
    }

    /// Check if there are any Keyboard modules in the graph.
    fn has_keyboard_modules(&self) -> bool {
        self.graph_state.graph.nodes.iter()
            .any(|(_, node)| node.user_data.module_id == "input.keyboard")
    }

    /// Handle keyboard events for the virtual keyboard module.
    ///
    /// Uses raw input events to capture keyboard input before the UI consumes them.
    fn handle_keyboard_events(&mut self, ctx: &egui::Context) {
        // Asked every frame, so the popover's claim lapses once it closes
        let chord_popover = crate::graph::letter_keys_taken(ctx);
        // A popover that went (its node deleted, or out of view) without
        // lifting its chord leaves it sounding: stop it
        if let (false, Some(node_id)) = (chord_popover, self.auditioning) {
            self.send_command(EngineCommand::Audition { node_id, chord: None });
            self.auditioning = None;
        }

        // Skip if modifier keys are held (those are shortcuts, not notes)
        let skip_keyboard = ctx.input(|i| {
            i.modifiers.ctrl || i.modifiers.alt || i.modifiers.command
        });
        if skip_keyboard {
            return;
        }

        // Letters typed into a text field (the palette, say) aren't notes, nor
        // are chords played into a Chord Sequencer's popover, which has the
        // keys while it's open. Releases still count, so a key held while
        // either opened doesn't stick
        let typing = ctx.wants_keyboard_input() || self.quick_add.is_some() || chord_popover;

        // Process raw keyboard events - these haven't been consumed yet
        let mut keys_changed = false;
        // With a Poly MIDI module to hear them, the keys also play as MIDI,
        // so chords can be played without a MIDI keyboard
        let play_midi = self.has_poly_midi_modules();
        let mut midi_notes = Vec::new();

        ctx.input(|i| {
            // Check raw events for key presses/releases
            for event in &i.raw.events {
                if let egui::Event::Key { key, pressed, repeat, .. } = event {
                    // Skip key repeat events
                    if *repeat {
                        continue;
                    }

                    if let Some(relative_note) = key_to_note(*key) {
                        if *pressed && typing {
                            continue;
                        }
                        if *pressed {
                            // Add key if not already in list
                            if !self.pressed_keys.iter().any(|(_, k)| *k == Some(*key)) {
                                self.pressed_keys.push((relative_note, Some(*key)));
                                keys_changed = true;
                                midi_notes.push((relative_note, true));
                            }
                        } else {
                            // Remove key from list
                            if let Some(pos) = self.pressed_keys.iter().position(|(_, k)| *k == Some(*key)) {
                                self.pressed_keys.remove(pos);
                                keys_changed = true;
                                midi_notes.push((relative_note, false));
                            }
                        }
                    }
                }
            }
        });

        // A piano key held with the mouse or a finger is one more held key,
        // with no computer key behind it (see `SynthGraphState::piano_pointer`)
        let pointer = self.pressed_keys.iter().position(|(_, k)| k.is_none());
        let pointed = self.user_state.piano_pointer;
        if pointer.map(|i| self.pressed_keys[i].0) != pointed {
            if let Some(i) = pointer {
                midi_notes.push((self.pressed_keys.remove(i).0, false));
            }
            if let Some(note) = pointed {
                self.pressed_keys.push((note, None));
                midi_notes.push((note, true));
            }
            keys_changed = true;
        }

        if let (true, Some(engine)) = (play_midi, self.midi_engine.as_ref()) {
            for (relative_note, pressed) in midi_notes {
                let note = relative_to_midi(relative_note, 0);
                engine.send(if pressed {
                    MidiEvent::NoteOn { channel: 0, note, velocity: 100 }
                } else {
                    MidiEvent::NoteOff { channel: 0, note, velocity: 0 }
                });
            }
        }

        // Update Keyboard modules if key state changed
        if keys_changed {
            self.sync_keyboard_modules();
        }
    }

    /// Minimum gate duration in milliseconds to ensure audio thread sees the trigger.
    const MIN_GATE_DURATION_MS: u64 = 30;

    /// Sync the current keyboard state to all Keyboard modules in the graph.
    ///
    /// Updates the Note and Gate parameters based on the currently pressed keys.
    /// Implements minimum gate duration to ensure reliable triggering.
    fn sync_keyboard_modules(&mut self) {
        // Held notes, in the order their keys went down
        let held: Vec<i32> = self.pressed_keys.iter().map(|(note, _)| *note).collect();

        // Determine actual gate state considering minimum duration. The gate
        // rises with the first key and stays up while any key is held, so a
        // change of note under it is legato: the pitch moves, nothing retriggers
        let gate_value = if !held.is_empty() {
            if !self.gate_held_high {
                // New note trigger
                self.last_gate_on = Some(Instant::now());
                self.gate_held_high = true;
            }
            1.0
        } else if self.gate_held_high {
            // Key released but check minimum duration
            if let Some(gate_time) = self.last_gate_on {
                let elapsed_ms = gate_time.elapsed().as_millis() as u64;
                if elapsed_ms < Self::MIN_GATE_DURATION_MS {
                    // Keep gate high until minimum duration
                    1.0
                } else {
                    // Minimum duration passed, can release
                    self.gate_held_high = false;
                    self.last_gate_on = None;
                    0.0
                }
            } else {
                self.gate_held_high = false;
                0.0
            }
        } else {
            0.0
        };

        // Collect Keyboard module engine IDs first to avoid borrow issues
        let keyboard_nodes: Vec<u64> = self.graph_state.graph.nodes.iter()
            .filter(|(_, node)| node.user_data.module_id == "input.keyboard")
            .filter_map(|(node_id, _)| self.user_state.get_engine_node_id(node_id))
            .collect();

        // Parameters are in order: Note(0), Gate(1), Octave(2), Velocity(3), Priority(4)
        let note_param_idx = 0;
        let gate_param_idx = 1;

        // Update all Keyboard modules
        for engine_node_id in keyboard_nodes {
            // Each module chooses its note from the held keys by its own
            // Priority. With nothing held the Note stays where it was, so the
            // last note rings through the release
            let priority = self.cached_params
                .get(&(engine_node_id, KeyboardInput::PARAM_PRIORITY))
                .map_or(KeyPriority::Last, |&value| KeyPriority::from_param(value));
            if let Some(note) = priority.select_note(&held) {
                let note = relative_to_midi(note, 0) as f32;
                self.send_command(EngineCommand::SetParameter {
                    node_id: engine_node_id,
                    param_index: note_param_idx,
                    value: note,
                });
                // Also update the cached params so sync_parameters doesn't overwrite
                self.cached_params.insert((engine_node_id, note_param_idx), note);
            }

            // Update Gate parameter
            self.send_command(EngineCommand::SetParameter {
                node_id: engine_node_id,
                param_index: gate_param_idx,
                value: gate_value,
            });
            self.cached_params.insert((engine_node_id, gate_param_idx), gate_value);
        }

        // Update active notes for piano display (convert relative notes to MIDI notes)
        let active_notes: Vec<u8> = self.pressed_keys.iter()
            .map(|(rel, _)| relative_to_midi(*rel, 0) as u8)
            .collect();
        self.user_state.set_keyboard_active_notes(active_notes);
    }

    /// Check if gate needs to be released after minimum duration.
    fn update_gate_timing(&mut self) {
        if self.gate_held_high && self.pressed_keys.is_empty() {
            // Key was released, check if we should release the gate now
            if let Some(gate_time) = self.last_gate_on {
                if gate_time.elapsed().as_millis() as u64 >= Self::MIN_GATE_DURATION_MS {
                    // Time to release
                    self.sync_keyboard_modules();
                }
            }
        }
    }
}

/// The `nth` module of a kind (by registry id, in graph order) and its input
/// named `input`, for capture scripts.
fn find_input(graph: &crate::graph::SynthGraph, module: &str, nth: usize, input: &str) -> Option<egui_node_graph2::InputId> {
    let node = graph.nodes.values().filter(|n| n.user_data.module_id == module).nth(nth)?;
    node.inputs.iter().find(|(name, _)| name.eq_ignore_ascii_case(input)).map(|(_, id)| *id)
}

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
    let word = ui.fonts(|fonts| fonts.layout_job(job));
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
            painter.circle(jack, 2.6, theme::background::WIDGET, egui::Stroke::new(1.2, theme::signal::AUDIO));
        }
        painter.galley(egui::pos2(rect.left() + cable.x + gap, rect.top()), word, theme::text::PRIMARY);
    }
    response.on_hover_text("Soba: a node-based modular synthesizer")
}

/// What the audio output is doing, as the status bar shows it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AudioStatus {
    Playing,
    Stopped,
    /// The device errored or stopped asking for audio.
    NoAudio,
    /// The browser holds a page's sound until it's clicked or played.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Waiting,
}

/// Actions collected from the toolbar for deferred execution
#[derive(Default)]
struct ToolbarActions {
    toggle_playing: bool,
    toggle_recording: bool,
    open_recordings: bool,
    choose_recordings_folder: bool,
    reset_recordings_folder: bool,
    select_device: Option<usize>,
    select_audio_system: Option<AudioSystem>,
    /// A buffer size, in frames.
    select_buffer: Option<u32>,
    /// Open this input device, or with `None` close the input.
    select_input: Option<Option<usize>>,
    refresh_input_devices: bool,
    refresh_devices: bool,
    save_patch: bool,
    save_as_patch: bool,
    load_patch: bool,
    open_example: Option<&'static Example>,
    open_recent: Option<PathBuf>,
    clear_recent: bool,
    new_patch: bool,
    undo: bool,
    redo: bool,
    // MIDI actions
    connect_midi_device: Option<usize>,
    disconnect_midi: bool,
    refresh_midi_devices: bool,
    // Help menu
    check_updates: bool,
    roll_back: bool,
}

impl eframe::App for SynthApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme on first frame
        if !self.theme_applied {
            theme::apply_theme(ctx);
            self.theme_applied = true;
        }

        // A capture renders this frame's audio before anything is drawn
        self.step_capture(ctx);

        if std::mem::take(&mut self.release_tab_focus) {
            if let Some(focused) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(focused));
            }
        }

        // A patch file the browser has finished uploading
        #[cfg(target_arch = "wasm32")]
        self.collect_upload();

        // Process events from the audio engine
        self.process_engine_events();
        self.recover_from_driver_reset();
        let now = ctx.input(|i| i.time);
        self.user_state.tick_signal_history(now, &self.graph_state.graph);

        // A warning in the status bar fades out after a while
        if let Some((_, raised)) = self.notice.as_mut() {
            if raised.is_nan() {
                *raised = now;
            }
            if now - *raised > NOTICE_SECONDS {
                self.notice = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(NOTICE_SECONDS - (now - *raised)));
            }
        }

        // Advance meter ballistics; keep repainting until it has settled
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.user_state.output_meter.tick(dt);
        if !self.user_state.output_meter.is_idle() {
            ctx.request_repaint();
        }
        for meters in self.user_state.module_meters.values_mut() {
            meters.tick(dt);
            if !meters.is_idle() {
                ctx.request_repaint();
            }
        }

        // Clear status message after it's been shown (user will see it on first frame)
        // We clear it on the next frame after it was set
        let had_status_message = self.status_message.is_some();

        // Request continuous repaints when playing (for LED indicators and other visualizations)
        // Also repaint continuously when there are keyboard or MIDI Note modules, so
        // their pianos follow the keys
        if self.is_playing || self.has_keyboard_modules() || self.has_midi_note_modules() {
            ctx.request_repaint();
        }

        // Handle keyboard shortcuts
        let mut keyboard_save = false;
        let mut keyboard_save_as = false;
        let mut keyboard_load = false;
        let mut keyboard_bypass = false;
        let mut keyboard_record = false;

        // Closing the window with unsaved changes asks first
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request(ctx, Discard::Quit);
        }

        // Undo and redo, unless a text field has the keys (it has its own undo).
        // Redo is checked first: Ctrl+Z alone would also match Ctrl+Shift+Z.
        let prompt_open = self.prompt_open();
        if !ctx.wants_keyboard_input() && !prompt_open {
            use egui::{Key, KeyboardShortcut, Modifiers};
            let (redo, undo) = ctx.input_mut(|i| {
                let redo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
                    || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
                (redo, i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)))
            });
            if redo {
                self.redo();
            } else if undo {
                self.undo();
            }
        }

        let mut keyboard_new = false;
        ctx.input(|i| {
            if prompt_open {
                return;
            }
            // Ctrl+N: New
            if i.modifiers.ctrl && i.key_pressed(egui::Key::N) {
                keyboard_new = true;
            }
            // Ctrl+S: Save, Ctrl+Shift+S: Save As
            if i.modifiers.ctrl && i.key_pressed(egui::Key::S) {
                if i.modifiers.shift {
                    keyboard_save_as = true;
                } else {
                    keyboard_save = true;
                }
            }
            // Ctrl+O: Open/Load
            if i.modifiers.ctrl && i.key_pressed(egui::Key::O) {
                keyboard_load = true;
            }
            // Ctrl+B: Bypass the selected nodes
            if i.modifiers.ctrl && i.key_pressed(egui::Key::B) {
                keyboard_bypass = true;
            }
            // Ctrl+R: Record, or stop recording
            if i.modifiers.ctrl && i.key_pressed(egui::Key::R) {
                keyboard_record = true;
            }
        });

        if keyboard_bypass {
            for node_id in self.graph_state.selected_nodes.clone() {
                self.toggle_bypass(node_id);
            }
        }

        // Editing shortcuts act on the graph, so they wait while a text field
        // or the palette has the keys
        if !ctx.wants_keyboard_input() && self.quick_add.is_none() && !prompt_open {
            self.handle_group_shortcuts(ctx);
            self.handle_editing_shortcuts(ctx);
        }

        // Handle musical keyboard input (QWERTY to notes)
        self.handle_keyboard_events(ctx);

        // Update gate timing (for minimum gate duration)
        self.update_gate_timing();

        let toolbar_actions = if self.embedded {
            ToolbarActions::default()
        } else {
            // Top toolbar panel
            let toolbar_actions = egui::TopBottomPanel::top("toolbar")
                .frame(egui::Frame::none()
                    .fill(theme::background::PANEL)
                    .inner_margin(egui::Margin::symmetric(8.0, 8.0)))
                .show(ctx, |ui| {
                    self.draw_toolbar(ui)
                })
                .inner;

            // Bottom status bar
            egui::TopBottomPanel::bottom("status_bar")
                .frame(egui::Frame::none()
                    .fill(theme::background::PANEL)
                    .inner_margin(egui::Margin::symmetric(0.0, 4.0)))
                .show(ctx, |ui| {
                    self.draw_status_bar(ui);
                });
            toolbar_actions
        };

        // Main content area - the node graph editor. A piano held down
        // says so as it's drawn
        self.user_state.piano_pointer = None;
        // WAV files dragged in from outside: where they'd land, and taking them
        self.handle_file_drops(ctx);
        self.draw_main_area(ctx);

        // An embed's one control, over the canvas
        let toolbar_actions = if self.embedded {
            ToolbarActions { toggle_playing: self.draw_embed_bar(ctx), ..toolbar_actions }
        } else {
            toolbar_actions
        };

        // Followers take their leaders' sections, in the same undo step
        self.follow_arrangers();

        // Sync parameter values to the audio engine
        self.sync_parameters();

        // Handle deferred actions (to avoid borrow checker issues)
        if toolbar_actions.toggle_playing {
            self.set_playing(!self.is_playing);
        }
        if toolbar_actions.toggle_recording || keyboard_record {
            self.toggle_recording();
        }
        if toolbar_actions.open_recordings {
            let folder = self.recordings_folder();
            if let Err(e) = std::fs::create_dir_all(&folder).and_then(|()| recording::open_folder(&folder)) {
                self.status_message = Some(format!("Couldn't open {}: {}", folder.display(), e));
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if toolbar_actions.choose_recordings_folder {
            if let Some(folder) = rfd::FileDialog::new().set_directory(self.recordings_folder()).pick_folder() {
                self.status_message = Some(format!("Recording to {}", folder.display()));
                self.recordings_folder = Some(folder);
            }
        }
        if toolbar_actions.reset_recordings_folder {
            self.recordings_folder = None;
        }
        if toolbar_actions.refresh_devices {
            self.refresh_devices();
        }
        if let Some(system) = toolbar_actions.select_audio_system {
            self.select_audio_system(system);
        }
        if let Some(frames) = toolbar_actions.select_buffer {
            self.select_buffer(frames);
        }
        if let Some(device_index) = toolbar_actions.select_device {
            self.select_device(device_index);
        }
        if toolbar_actions.refresh_input_devices {
            self.refresh_input_devices();
        }
        if let Some(input) = toolbar_actions.select_input {
            self.select_input(input);
        }

        // Handle save/load actions (from toolbar buttons or keyboard shortcuts)
        if toolbar_actions.save_patch || keyboard_save {
            self.quick_save();
        }
        if toolbar_actions.save_as_patch || keyboard_save_as {
            self.show_save_dialog();
        }
        if toolbar_actions.load_patch || keyboard_load {
            self.request(ctx, Discard::Open);
        }
        if let Some(example) = toolbar_actions.open_example {
            self.request(ctx, Discard::OpenExample(example));
        }
        if let Some(path) = toolbar_actions.open_recent {
            self.request(ctx, Discard::OpenFile(path));
        }
        if toolbar_actions.clear_recent {
            self.recent_files.clear();
        }
        if toolbar_actions.new_patch || keyboard_new {
            self.request(ctx, Discard::New);
        }
        if toolbar_actions.undo {
            self.undo();
        }
        if toolbar_actions.redo {
            self.redo();
        }

        // Handle MIDI actions
        if toolbar_actions.refresh_midi_devices {
            self.refresh_midi_devices();
        }
        if let Some(device_index) = toolbar_actions.connect_midi_device {
            self.connect_midi_device(device_index);
        }
        if toolbar_actions.disconnect_midi {
            self.disconnect_midi_device();
        }

        // Process pending MIDI events
        self.process_midi_events();

        // A stopped take's note, once its file is finished
        self.poll_recording(ctx);
        let mut corner = 0.0;
        if let Some(toast) = &self.record_toast {
            let (action, height) = recording::show_toast(ctx, toast);
            corner = height;
            match action {
                Some(ToastAction::ShowInFolder) => {
                    if let Err(e) = recording::reveal_in_folder(&toast.summary.path) {
                        self.status_message = Some(format!("Couldn't open the folder: {}", e));
                    }
                }
                Some(ToastAction::Dismiss) => self.record_toast = None,
                None => {}
            }
        }
        // New versions: the daily check, and its note above the take's
        #[cfg(not(target_arch = "wasm32"))]
        self.run_updates(ctx, toolbar_actions.check_updates, toolbar_actions.roll_back, corner);
        let _ = corner;

        // "Save changes?" and crash recovery, over everything else
        self.show_prompts(ctx);

        // The engine's cables follow whatever this frame did to the graph's
        self.sync_cables();
        // ...and its Samplers the files their nodes name. One the browser is
        // still fetching is looked for again shortly, and drawn the frame after
        if self.user_state.samples.waiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.sync_samples();

        // Whatever this frame changed becomes an undo step, once the mouse
        // button is up: a knob turn or a drag is one step, not one per frame
        // Typing a frame's title or a note is one step too, once it's done
        let gesture_held = ctx.input(|i| i.pointer.any_down())
            || self.user_state.annotations.is_editing()
            || self.user_state.renaming.is_some();
        self.history.record(&self.graph_state, &self.user_state, gesture_held, Instant::now());

        // How late live input is, for Loopers to take off their overdubs
        if let Some(ref handle) = self.ui_handle {
            let frames = match (&self.input_monitor, &self.audio_engine) {
                (Some(monitor), Ok(engine)) => {
                    let trip = RoundTrip::of(monitor, engine.output_latency(), engine.sample_rate());
                    (trip.total().as_secs_f64() * engine.sample_rate() as f64).round() as usize
                }
                _ => 0,
            };
            handle.set_input_latency(frames);
        }

        // Ship this frame's graph edits to the audio thread as one compiled plan
        if let Some(ref mut handle) = self.ui_handle {
            if !handle.flush() {
                // The audio thread is behind; retry soon even if nothing else
                // asks for a frame, so held-back edits aren't left waiting
                ctx.request_repaint_after(std::time::Duration::from_millis(15));
            }
        }

        // The title names the patch, with a dot while it has unsaved changes
        let title = format!(
            "{}{} · Soba",
            if self.has_unsaved_changes() { "● " } else { "" },
            self.patch_title()
        );
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }

        // Clear status message after showing it for one frame
        // This gives user time to read it but doesn't persist forever
        if had_status_message {
            // Request one more repaint to clear the message
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }

        if let Some(capture) = self.capture.as_mut() {
            capture.draw_cursor(ctx);
            capture.end_frame(ctx);
            if capture.finish() {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// While capturing, the script replaces the mouse and keyboard, and the
    /// clock moves one frame per pass.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(capture) = self.capture.as_mut() {
            capture.prepare_input(ctx, raw_input);
        }
        // The wheel over an Arranger's timeline zooms and pans it, not the canvas
        crate::graph::claim_arranger_wheel(ctx, raw_input);
    }

    /// Stores recent files, and the patch while it has unsaved changes, so
    /// a crash loses at most [`session::AUTOSAVE_INTERVAL`] of work.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // A capture's edits are the script's, and an embed's the page's: not work to keep
        if self.capture.is_some() || self.embedded {
            return;
        }
        self.recent_files.store(storage);
        #[cfg(not(target_arch = "wasm32"))]
        self.updater.store(storage);
        storage.set_string(FLOW_GLYPH_KEY, self.user_state.flow_glyph.name().to_string());
        storage.set_string(KNOB_STYLE_KEY, self.user_state.knob_style.name().to_string());
        if let Ok(engine) = &self.audio_engine {
            storage.set_string(AUDIO_SYSTEM_KEY, engine.audio_system().key().to_string());
            storage.set_string(BUFFER_FRAMES_KEY, engine.buffer_request().map(|f| f.to_string()).unwrap_or_default());
        }
        let folder = self.recordings_folder.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
        storage.set_string(recording::FOLDER_KEY, folder);
        let autosave = if let Some(recovery) = &self.recovery {
            // Not answered yet: keep it for next time
            Some(recovery.clone())
        } else if self.has_unsaved_changes() && (!self.allow_close || self.resuming) {
            let patch = self.create_patch(&self.patch_title());
            let example = self.current_example.map(|e| e.name.to_string());
            Autosave::new(&patch, self.current_patch_path.clone(), example).ok()
        } else {
            // Saved, or let go on the way out. A restart into another
            // version keeps it, in case the next copy doesn't start
            None
        };
        Autosave::store(storage, autosave.as_ref());
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        session::AUTOSAVE_INTERVAL
    }

    /// Only the app's own state is kept; egui's (open menus, scroll
    /// positions) starts fresh each launch.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}
