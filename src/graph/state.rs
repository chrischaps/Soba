//! Graph state for the synthesizer.
//!
//! Contains the user state passed to egui_node_graph2 callbacks.

use egui::{Color32, Pos2, Vec2};
use egui_node_graph2::{Backdrop, ConnectionSignalTrait, FlowGlyph, GraphEditorState, NodeId, SignalTrace};
use std::collections::{HashMap, HashSet, VecDeque};
use web_time::Instant;

use crate::dsp::Readout;
use crate::engine::{ChannelPeaks, NodeId as EngineNodeId};
use crate::engine::midi_engine::MidiEvent;
use crate::widgets::{LevelMeter, ModuleMeters};
use super::annotation_ui::{self, View};
use super::annotations::{self, Annotations};
use super::groups::{Face, GroupId, JackAction, Renaming, Side};
use super::signal_history::{OutputHistory, TraceShape};
use super::{SynthDataType, SynthGraph, SynthNodeData, SynthValueType};
use super::templates::SynthNodeTemplate;

/// Duration to show validation messages before auto-clearing.
const VALIDATION_MESSAGE_DURATION_SECS: f32 = 3.0;

/// Maximum number of MIDI events to store for display.
const MAX_MIDI_EVENTS: usize = 10;

/// A MIDI event with display timestamp for the MIDI Monitor.
#[derive(Clone, Debug)]
pub struct DisplayMidiEvent {
    /// The MIDI event.
    pub event: MidiEvent,
    /// Relative timestamp (seconds since first event).
    pub timestamp: f32,
}

/// Oscilloscope waveform data for display.
#[derive(Clone, Debug, Default)]
pub struct ScopeData {
    /// Channel 1 waveform samples.
    pub channel1: Vec<f32>,
    /// Channel 2 waveform samples.
    pub channel2: Vec<f32>,
    /// Whether this capture was triggered (vs free-running).
    pub triggered: bool,
}

/// Info about a MIDI CC mapping for display in the UI.
#[derive(Clone, Debug)]
pub struct MidiMappingInfo {
    /// CC number that is mapped.
    pub cc_number: u8,
    /// MIDI channel (0 = omni).
    pub channel: u8,
}

/// User state for the graph editor.
///
/// This is passed to all graph callbacks and can store any
/// application-specific data needed during graph editing.
pub struct SynthGraphState {
    /// Currently selected node, if any.
    pub selected_node: Option<NodeId>,

    /// Mapping from graph NodeId to audio engine NodeId.
    /// This is used to sync graph changes with the audio engine.
    pub node_id_map: HashMap<NodeId, EngineNodeId>,

    /// Counter for generating unique engine node IDs.
    next_engine_node_id: EngineNodeId,

    /// Last validation error message for display in UI.
    validation_message: Option<String>,

    /// When the validation message was set (for auto-clear).
    validation_message_time: Option<Instant>,

    /// Position where context menu was opened (screen coords).
    /// None when menu is closed.
    pub context_menu_pos: Option<Pos2>,

    /// Currently open submenu category index in the context menu.
    /// Used for hover-delay submenu behavior.
    pub context_menu_open_category: Option<usize>,

    /// Category being hovered and when hover started.
    /// Used to implement hover delay before opening a new submenu.
    pub context_menu_hover_intent: Option<(usize, Instant)>,

    /// Current input values received from the audio engine for signal feedback.
    /// Key: (engine_node_id, input_port_index), Value: sampled signal value.
    /// These values animate the knobs when their inputs are connected.
    pub input_values: HashMap<(EngineNodeId, usize), f32>,

    /// Current output values received from the audio engine for LED indicators.
    /// Key: (engine_node_id, output_port_index), Value: sampled signal value.
    /// These values light up LED indicators on nodes.
    pub output_values: HashMap<(EngineNodeId, usize), f32>,

    /// Per-channel output readings from the audio engine, for drawing
    /// polyphonic cables. Key: (engine_node_id, output_port_index). Kept
    /// while the transport is stopped, so poly cables stay drawn as bundles.
    pub output_channels: HashMap<(EngineNodeId, usize), ChannelPeaks>,

    /// The last few seconds of each monitored output, which cables draw as
    /// the signal flowing through them. Key: (engine_node_id, output_port_index).
    pub signal_history: HashMap<(EngineNodeId, usize), OutputHistory>,

    /// The mark that rides the signal along every cable.
    pub flow_glyph: FlowGlyph,

    /// How knobs are drawn.
    pub knob_style: crate::widgets::KnobStyle,

    /// Recent MIDI events for display in MIDI Monitor modules.
    pub midi_events: VecDeque<DisplayMidiEvent>,

    /// Timestamp of the first MIDI event received (for relative timestamps).
    midi_first_event_time: Option<Instant>,

    /// MIDI CC mappings for UI display.
    /// Key: (engine_node_id, param_index), Value: mapping info.
    pub midi_mappings: HashMap<(EngineNodeId, usize), MidiMappingInfo>,

    /// Whether MIDI Learn mode is active.
    pub midi_learn_active: bool,

    /// Target parameter for MIDI Learn (if active).
    /// Tuple of (engine_node_id, param_index).
    pub midi_learn_target: Option<(EngineNodeId, usize)>,

    /// Flag set when a widget context menu is shown this frame.
    /// Used to prevent the add-node menu from also appearing.
    pub widget_context_menu_open: bool,

    /// Oscilloscope waveform data received from the audio engine.
    /// Key: engine_node_id, Value: scope waveform data.
    pub scope_data: HashMap<EngineNodeId, ScopeData>,

    /// Current zoom level for scaling UI elements.
    /// Set by the graph editor before rendering.
    pub zoom: f32,

    /// Where patch (0, 0) is in editor node coordinates, at [`Self::zoom`]:
    /// a node at patch position `p` sits at `view_origin + p * zoom`. Set by
    /// the app before rendering, from its undo history's view anchor.
    pub view_origin: Vec2,

    /// The patch's frames and notes.
    pub annotations: Annotations,

    /// Whether the pointer is over a frame's or note's handle, where a
    /// right-click opens its menu rather than the add-module menu.
    pub over_annotation: bool,

    /// Whether the transport is playing.
    /// When false, cable animations are stopped.
    pub is_playing: bool,

    /// Active notes for Keyboard module display (MIDI note numbers).
    /// Updated by the keyboard input handler.
    pub keyboard_active_notes: Vec<u8>,

    /// Active notes for MIDI Note module display (MIDI note numbers).
    /// Updated by the MIDI event handler.
    pub midi_active_notes: Vec<u8>,

    /// How hard each of `midi_active_notes` was struck (1 to 127), in the
    /// same order.
    pub midi_active_velocities: Vec<u8>,

    /// The key of a Keyboard or Poly MIDI module's piano held down with the
    /// mouse or a finger, as a note from C like a computer key's (0 is C).
    /// Set as the piano is drawn, and played on the next frame.
    pub piano_pointer: Option<i32>,

    /// Output stage meter, fed by OutputLevel events from the audio engine.
    pub output_meter: LevelMeter,

    /// Modules' own meters (a Mixer's channel strips), fed by MeterLevels
    /// events. Key: engine node ID.
    pub module_meters: HashMap<EngineNodeId, ModuleMeters>,

    /// Modules' latest readouts (a Clock's tempo and beat), fed by Readout
    /// events. Key: engine node ID.
    pub readouts: HashMap<EngineNodeId, Readout>,

    /// The input device Audio Input modules are listening to, if one is open.
    pub audio_input_name: Option<String>,

    /// The files the patch's Samplers play, decoded, by sample key.
    pub samples: super::sample_shelf::SampleShelf,

    /// While a file is dragged over the window, the Sampler it would land
    /// on, or `None` for empty canvas. `Some` only during a drag.
    pub file_drop_target: Option<Option<NodeId>>,

    /// The group whose inside the editor shows, or `None` for the top of
    /// the patch.
    pub level: Option<GroupId>,

    /// Nodes on other levels than [`Self::level`], which aren't drawn. Set
    /// by the app before each frame is drawn.
    pub hidden: HashSet<NodeId>,

    /// Outputs of groups' nodes, standing for the module output each one
    /// really carries, so cables from a jack show that module's signal.
    /// Key: (graph node, output index). Set by the app before drawing.
    pub output_aliases: HashMap<(NodeId, usize), (EngineNodeId, usize)>,

    /// What each group on the shown level has on its face. Set by the app
    /// before drawing.
    pub group_faces: HashMap<GroupId, Face>,

    /// What each port on the shown level can do with groups' jacks, by node
    /// and side, then port name. Set by the app before drawing.
    pub jack_actions: HashMap<(NodeId, Side), Vec<(String, Vec<JackAction>)>>,

    /// A group being named, in a field on its face.
    pub renaming: Option<Renaming>,

    /// The cables the engine was last sent, module to module. See
    /// [`crate::app::engine_sync::sync_cables`].
    pub engine_cables: crate::app::engine_sync::LeafCables,

    /// Each Arranger that follows another, and the one it follows. Set by
    /// the app each frame: see [`super::arranger_follow`].
    pub arranger_leaders: HashMap<NodeId, NodeId>,
}

impl Default for SynthGraphState {
    fn default() -> Self {
        Self {
            selected_node: None,
            node_id_map: HashMap::new(),
            next_engine_node_id: 0,
            validation_message: None,
            validation_message_time: None,
            context_menu_pos: None,
            context_menu_open_category: None,
            context_menu_hover_intent: None,
            input_values: HashMap::new(),
            output_values: HashMap::new(),
            output_channels: HashMap::new(),
            signal_history: HashMap::new(),
            flow_glyph: FlowGlyph::default(),
            knob_style: crate::widgets::KnobStyle::default(),
            midi_events: VecDeque::new(),
            midi_first_event_time: None,
            midi_mappings: HashMap::new(),
            midi_learn_active: false,
            midi_learn_target: None,
            widget_context_menu_open: false,
            scope_data: HashMap::new(),
            zoom: 1.0,
            view_origin: Vec2::ZERO,
            annotations: Annotations::default(),
            over_annotation: false,
            is_playing: false,
            keyboard_active_notes: Vec::new(),
            midi_active_notes: Vec::new(),
            midi_active_velocities: Vec::new(),
            piano_pointer: None,
            output_meter: LevelMeter::default(),
            module_meters: HashMap::new(),
            readouts: HashMap::new(),
            audio_input_name: None,
            samples: Default::default(),
            file_drop_target: None,
            level: None,
            hidden: HashSet::new(),
            output_aliases: HashMap::new(),
            group_faces: HashMap::new(),
            jack_actions: HashMap::new(),
            renaming: None,
            engine_cables: Default::default(),
            arranger_leaders: HashMap::new(),
        }
    }
}

impl SynthGraphState {
    /// Create a new graph state.
    pub fn new() -> Self {
        Self::default()
    }

    /// What a port's right-click menu can do with groups' jacks.
    pub fn jack_actions(&self, node_id: NodeId, side: Side, port: &str) -> &[JackAction] {
        self.jack_actions
            .get(&(node_id, side))
            .and_then(|ports| ports.iter().find(|(name, _)| name == port))
            .map_or(&[], |(_, actions)| actions.as_slice())
    }

    /// Allocate a new engine node ID and map it to a graph node.
    pub fn allocate_engine_node_id(&mut self, graph_node_id: NodeId) -> EngineNodeId {
        let id = self.next_engine_node_id;
        self.next_engine_node_id += 1;
        self.node_id_map.insert(graph_node_id, id);
        id
    }

    /// Map a graph node to an engine node ID it had before, when undo
    /// brings a deleted node back. Its MIDI mappings follow it back.
    pub fn assign_engine_node_id(&mut self, graph_node_id: NodeId, engine_node_id: EngineNodeId) {
        self.next_engine_node_id = self.next_engine_node_id.max(engine_node_id + 1);
        self.node_id_map.insert(graph_node_id, engine_node_id);
    }

    /// A new group's ID, from the same count as engine node IDs, so a patch
    /// file can name modules and groups alike without them colliding.
    pub fn allocate_group_id(&mut self) -> GroupId {
        let id = self.next_engine_node_id;
        self.next_engine_node_id += 1;
        GroupId(id)
    }

    /// Notes a group ID already in use (undo bringing a group back), so
    /// it's never handed out again.
    pub fn claim_group_id(&mut self, id: GroupId) {
        self.next_engine_node_id = self.next_engine_node_id.max(id.0 + 1);
    }

    /// The module output a graph node's output carries: its own, or for a
    /// group's jack, the module output plugged in behind it.
    pub fn resolve_output(&self, graph_node_id: NodeId, output_index: usize) -> Option<(EngineNodeId, usize)> {
        if let Some(&alias) = self.output_aliases.get(&(graph_node_id, output_index)) {
            return Some(alias);
        }
        Some((self.get_engine_node_id(graph_node_id)?, output_index))
    }

    /// Get the engine node ID for a graph node.
    pub fn get_engine_node_id(&self, graph_node_id: NodeId) -> Option<EngineNodeId> {
        self.node_id_map.get(&graph_node_id).copied()
    }

    /// Remove a graph node from the mapping.
    pub fn remove_node(&mut self, graph_node_id: NodeId) -> Option<EngineNodeId> {
        self.node_id_map.remove(&graph_node_id)
    }

    /// Clear all mappings.
    pub fn clear(&mut self) {
        self.node_id_map.clear();
        self.selected_node = None;
        self.validation_message = None;
        self.validation_message_time = None;
        self.context_menu_pos = None;
        self.context_menu_open_category = None;
        self.context_menu_hover_intent = None;
        self.input_values.clear();
        self.output_values.clear();
        self.output_channels.clear();
        self.signal_history.clear();
        self.midi_events.clear();
        self.midi_first_event_time = None;
        self.midi_mappings.clear();
        self.midi_learn_active = false;
        self.midi_learn_target = None;
        self.widget_context_menu_open = false;
        self.scope_data.clear();
        self.keyboard_active_notes.clear();
        self.midi_active_notes.clear();
        self.midi_active_velocities.clear();
        self.output_meter = LevelMeter::default();
        self.module_meters.clear();
        self.readouts.clear();
        self.samples.clear();
        self.file_drop_target = None;
        self.annotations.clear();
        self.over_annotation = false;
        self.level = None;
        self.hidden.clear();
        self.output_aliases.clear();
        self.group_faces.clear();
        self.jack_actions.clear();
        self.renaming = None;
        self.engine_cables.clear();
    }

    /// Get the MIDI mapping info for a parameter, if any.
    pub fn get_midi_mapping(&self, engine_node_id: EngineNodeId, param_index: usize) -> Option<&MidiMappingInfo> {
        self.midi_mappings.get(&(engine_node_id, param_index))
    }

    /// Set or update a MIDI mapping for a parameter.
    pub fn set_midi_mapping(&mut self, engine_node_id: EngineNodeId, param_index: usize, cc_number: u8, channel: u8) {
        self.midi_mappings.insert(
            (engine_node_id, param_index),
            MidiMappingInfo { cc_number, channel },
        );
    }

    /// Remove a MIDI mapping for a parameter.
    pub fn remove_midi_mapping(&mut self, engine_node_id: EngineNodeId, param_index: usize) {
        self.midi_mappings.remove(&(engine_node_id, param_index));
    }

    /// Check if a parameter is currently the MIDI Learn target.
    pub fn is_midi_learn_target(&self, engine_node_id: EngineNodeId, param_index: usize) -> bool {
        self.midi_learn_target == Some((engine_node_id, param_index))
    }

    /// Set a validation error message to display.
    pub fn set_validation_error(&mut self, message: impl Into<String>) {
        self.validation_message = Some(message.into());
        self.validation_message_time = Some(Instant::now());
    }

    /// Clear the validation message.
    pub fn clear_validation_message(&mut self) {
        self.validation_message = None;
        self.validation_message_time = None;
    }

    /// Get the current validation message if it hasn't expired.
    pub fn validation_message(&mut self) -> Option<&str> {
        // Auto-clear after duration
        if let Some(time) = self.validation_message_time {
            if time.elapsed().as_secs_f32() > VALIDATION_MESSAGE_DURATION_SECS {
                self.clear_validation_message();
            }
        }
        self.validation_message.as_deref()
    }

    /// Update an input value from the audio engine feedback.
    pub fn set_input_value(&mut self, engine_node_id: EngineNodeId, input_index: usize, value: f32) {
        self.input_values.insert((engine_node_id, input_index), value);
    }

    /// Get the current input value for a node's input port.
    /// Returns None if no value has been received yet.
    pub fn get_input_value(&self, engine_node_id: EngineNodeId, input_index: usize) -> Option<f32> {
        self.input_values.get(&(engine_node_id, input_index)).copied()
    }

    /// Clear input values for a specific node (e.g., when node is deleted).
    pub fn clear_input_values_for_node(&mut self, engine_node_id: EngineNodeId) {
        self.input_values.retain(|(node_id, _), _| *node_id != engine_node_id);
    }

    /// Update an output value from the audio engine feedback.
    pub fn set_output_value(&mut self, engine_node_id: EngineNodeId, output_index: usize, value: f32) {
        self.output_values.insert((engine_node_id, output_index), value);
    }

    /// Get the current output value for a node's output port.
    /// Returns None if no value has been received yet.
    pub fn get_output_value(&self, engine_node_id: EngineNodeId, output_index: usize) -> Option<f32> {
        self.output_values.get(&(engine_node_id, output_index)).copied()
    }

    /// Clear output values for a specific node (e.g., when node is deleted).
    pub fn clear_output_values_for_node(&mut self, engine_node_id: EngineNodeId) {
        self.output_values.retain(|(node_id, _), _| *node_id != engine_node_id);
        self.output_channels.retain(|(node_id, _), _| *node_id != engine_node_id);
        self.signal_history.retain(|(node_id, _), _| *node_id != engine_node_id);
        self.module_meters.remove(&engine_node_id);
        self.readouts.remove(&engine_node_id);
    }

    /// Update an output's per-channel reading from the audio engine feedback.
    pub fn set_output_channels(&mut self, engine_node_id: EngineNodeId, output_index: usize, channels: ChannelPeaks) {
        self.output_channels.insert((engine_node_id, output_index), channels);
        self.signal_history.entry((engine_node_id, output_index)).or_default().observe(channels);
    }

    /// Brings every output's signal history up to `now` (seconds on the UI
    /// clock), shaped by the type of signal it carries.
    pub fn tick_signal_history(&mut self, now: f64, graph: &SynthGraph) {
        for (output_id, output) in graph.outputs.iter() {
            let Some(engine_node_id) = self.get_engine_node_id(output.node) else { continue };
            let Some(output_index) = graph.get_output_index(output_id) else { continue };
            if let Some(history) = self.signal_history.get_mut(&(engine_node_id, output_index)) {
                history.set_shape(TraceShape::of(output.typ.signal_type()));
            }
        }
        for history in self.signal_history.values_mut() {
            history.tick(now, self.is_playing);
        }
    }

    /// The last per-channel reading of a graph node's output, if any.
    pub fn output_peaks(&self, graph_node_id: NodeId, output_index: usize) -> Option<&ChannelPeaks> {
        self.output_channels.get(&self.resolve_output(graph_node_id, output_index)?)
    }

    /// Add a MIDI event for display in MIDI Monitor modules.
    ///
    /// Events are stored with a relative timestamp from the first event.
    /// Only the most recent MAX_MIDI_EVENTS are kept.
    pub fn push_midi_event(&mut self, event: MidiEvent) {
        let now = Instant::now();
        let timestamp = if let Some(first_time) = self.midi_first_event_time {
            first_time.elapsed().as_secs_f32()
        } else {
            self.midi_first_event_time = Some(now);
            0.0
        };

        self.midi_events.push_back(DisplayMidiEvent { event, timestamp });

        // Keep only the most recent events
        while self.midi_events.len() > MAX_MIDI_EVENTS {
            self.midi_events.pop_front();
        }
    }

    /// Get the recent MIDI events for display.
    pub fn midi_events(&self) -> &VecDeque<DisplayMidiEvent> {
        &self.midi_events
    }

    /// Clear all stored MIDI events.
    pub fn clear_midi_events(&mut self) {
        self.midi_events.clear();
        self.midi_first_event_time = None;
    }

    /// Update oscilloscope waveform data from the audio engine.
    pub fn set_scope_data(
        &mut self,
        engine_node_id: EngineNodeId,
        channel1: Vec<f32>,
        channel2: Vec<f32>,
        triggered: bool,
    ) {
        self.scope_data.insert(
            engine_node_id,
            ScopeData {
                channel1,
                channel2,
                triggered,
            },
        );
    }

    /// Get the current scope data for an oscilloscope node.
    pub fn get_scope_data(&self, engine_node_id: EngineNodeId) -> Option<&ScopeData> {
        self.scope_data.get(&engine_node_id)
    }

    /// Clear scope data for a specific node (e.g., when node is deleted).
    pub fn clear_scope_data_for_node(&mut self, engine_node_id: EngineNodeId) {
        self.scope_data.remove(&engine_node_id);
    }

    /// Set the active notes for the Keyboard module display.
    pub fn set_keyboard_active_notes(&mut self, notes: Vec<u8>) {
        self.keyboard_active_notes = notes;
    }

    /// Get the active notes for the Keyboard module display.
    pub fn keyboard_active_notes(&self) -> &[u8] {
        &self.keyboard_active_notes
    }

    /// Set the notes held on the MIDI keyboard, each with how hard it was
    /// struck.
    pub fn set_midi_active_notes(&mut self, held: &[(u8, u8)]) {
        self.midi_active_notes = held.iter().map(|&(note, _)| note).collect();
        self.midi_active_velocities = held.iter().map(|&(_, velocity)| velocity).collect();
    }

    /// Get the active notes for the MIDI Note module display.
    pub fn midi_active_notes(&self) -> &[u8] {
        &self.midi_active_notes
    }

    /// The notes held on the MIDI keyboard, each with how hard it was struck.
    pub fn midi_held(&self) -> Vec<(u8, u8)> {
        self.midi_active_notes.iter().copied().zip(self.midi_active_velocities.iter().copied()).collect()
    }
}

/// Implementation of ConnectionSignalTrait for signal-driven cable animations.
///
/// This enables cables to animate based on actual signal flow through the connection,
/// rather than continuously. For example:
/// - Gate signals only animate when the gate is high
/// - Audio/CV signals animate with intensity proportional to signal level
/// - All animations stop when transport is stopped
impl ConnectionSignalTrait for SynthGraphState {
    fn get_output_signal_level(&self, graph_node_id: NodeId, output_index: usize) -> Option<f32> {
        // When transport is stopped, return 0.0 to stop all animations
        if !self.is_playing {
            return Some(0.0);
        }
        // The module output this one carries, through any group jacks
        let (engine_node_id, output_index) = self.resolve_output(graph_node_id, output_index)?;
        // Get the output signal value from the audio engine feedback
        self.get_output_value(engine_node_id, output_index)
    }

    fn output_channel_count(&self, graph_node_id: NodeId, output_index: usize) -> usize {
        self.output_peaks(graph_node_id, output_index).map_or(1, ChannelPeaks::count)
    }

    fn get_output_channel_signal_level(&self, graph_node_id: NodeId, output_index: usize, channel: usize) -> Option<f32> {
        if !self.is_playing {
            return Some(0.0);
        }
        self.output_peaks(graph_node_id, output_index).map(|peaks| peaks.peak(channel))
    }

    fn output_trace(&self, graph_node_id: NodeId, output_index: usize, channel: usize) -> Option<SignalTrace<'_>> {
        self.signal_history.get(&self.resolve_output(graph_node_id, output_index)?)?.trace(channel)
    }

    fn flow_glyph(&self) -> FlowGlyph {
        self.flow_glyph
    }

    fn node_shown(&self, node_id: NodeId) -> bool {
        !self.hidden.contains(&node_id)
    }

    fn backdrop_ui(&mut self, ui: &mut egui::Ui, backdrop: Backdrop<'_>) {
        // Frames and notes are on the top level of the patch
        if self.level.is_some() {
            self.over_annotation = false;
            return;
        }
        // A scroll this frame has already zoomed the nodes, after the app set
        // the view origin, so the origin follows them here
        let mut view_origin = self.view_origin;
        if backdrop.zoom != self.zoom {
            let half_size = ui.clip_rect().size() / 2.0;
            view_origin = annotations::follow_zoom(view_origin, backdrop.zoom / self.zoom, half_size, *backdrop.pan);
        }
        let view = View { origin: backdrop.origin + view_origin, zoom: backdrop.zoom };
        self.over_annotation = annotation_ui::show(ui, &mut self.annotations, backdrop, view);
    }

    fn output_port_color(
        &self,
        graph_node_id: NodeId,
        output_index: usize,
        base_color: Color32,
    ) -> Color32 {
        // Get signal level and brighten the port color accordingly
        if let Some(signal_level) = self.get_output_signal_level(graph_node_id, output_index) {
            let brightness = signal_level.abs().clamp(0.0, 1.0);
            // Lerp from base color towards white based on signal level
            Color32::from_rgb(
                (base_color.r() as f32 + (255.0 - base_color.r() as f32) * brightness) as u8,
                (base_color.g() as f32 + (255.0 - base_color.g() as f32) * brightness) as u8,
                (base_color.b() as f32 + (255.0 - base_color.b() as f32) * brightness) as u8,
            )
        } else {
            base_color
        }
    }
}

/// Type alias for the complete graph editor state with our custom types.
/// Order: NodeData, DataType, ValueType, NodeTemplate, UserState
pub type SynthGraphEditorState = GraphEditorState<SynthNodeData, SynthDataType, SynthValueType, SynthNodeTemplate, SynthGraphState>;

/// Create a new graph editor state with default configuration.
pub fn create_editor_state() -> SynthGraphEditorState {
    GraphEditorState::new(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_state_default() {
        let state = SynthGraphState::default();
        assert!(state.selected_node.is_none());
        assert!(state.node_id_map.is_empty());
    }

    #[test]
    fn test_allocate_engine_node_id() {
        let mut state = SynthGraphState::new();

        // Create a fake graph node id for testing
        let graph_node_id: NodeId = unsafe { std::mem::transmute(1u64) };

        let engine_id = state.allocate_engine_node_id(graph_node_id);
        assert_eq!(engine_id, 0);

        let graph_node_id2: NodeId = unsafe { std::mem::transmute(2u64) };
        let engine_id2 = state.allocate_engine_node_id(graph_node_id2);
        assert_eq!(engine_id2, 1);
    }

    #[test]
    fn test_get_engine_node_id() {
        let mut state = SynthGraphState::new();
        let graph_node_id: NodeId = unsafe { std::mem::transmute(1u64) };

        // Before allocation
        assert!(state.get_engine_node_id(graph_node_id).is_none());

        // After allocation
        let engine_id = state.allocate_engine_node_id(graph_node_id);
        assert_eq!(state.get_engine_node_id(graph_node_id), Some(engine_id));
    }

    #[test]
    fn test_remove_node() {
        let mut state = SynthGraphState::new();
        let graph_node_id: NodeId = unsafe { std::mem::transmute(1u64) };

        let engine_id = state.allocate_engine_node_id(graph_node_id);
        assert_eq!(state.remove_node(graph_node_id), Some(engine_id));
        assert!(state.get_engine_node_id(graph_node_id).is_none());
    }

    #[test]
    fn test_clear() {
        let mut state = SynthGraphState::new();
        let graph_node_id: NodeId = unsafe { std::mem::transmute(1u64) };
        state.allocate_engine_node_id(graph_node_id);
        state.selected_node = Some(graph_node_id);

        state.clear();

        assert!(state.selected_node.is_none());
        assert!(state.node_id_map.is_empty());
    }
}
