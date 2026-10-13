//! Patch serialization for save/load functionality.
//!
//! This module defines the data structures for serializing synthesizer patches
//! to JSON files. A patch captures the complete state of the node graph including
//! all nodes, their positions, parameter values, and connections.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::graph::SynthNodeTemplate;
use crate::modules::Oscillator;

/// Current patch format version.
/// Increment this when making breaking changes to the format.
///
/// - v1: positional parameters
/// - v2: adds `midi_mappings`
/// - v3: parameters are stored by name
/// - v4: every value is in the parameter's real units (filter Drive was 0-1)
/// - v5: the oscillator's Frequency becomes Octave / Semitone / Fine, and
///   FM Depth becomes an index relative to the pitch
/// - v6: adds `groups`. Only patches that have groups are written as v6, so
///   versions before groups still open the rest
pub const PATCH_VERSION: u32 = 6;

/// The version a patch without groups is written as.
const VERSION_WITHOUT_GROUPS: u32 = 5;

/// A MIDI CC to parameter mapping.
///
/// Maps a hardware MIDI CC controller to a synthesizer parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MidiMapping {
    /// CC number (0-127).
    pub cc_number: u8,
    /// MIDI channel (0 = omni/any channel, 1-16 = specific channel).
    pub channel: u8,
    /// Target engine node ID.
    pub node_id: u64,
    /// Target parameter index within the node.
    pub param_index: usize,
    /// Name of the parameter. On load this wins over `param_index`, so a
    /// mapping survives the module's parameters being reordered.
    pub param_name: String,
    /// Minimum value of the mapped range.
    pub min_value: f32,
    /// Maximum value of the mapped range.
    pub max_value: f32,
}

impl MidiMapping {
    /// Create a new MIDI mapping with full range.
    pub fn new(
        cc_number: u8,
        channel: u8,
        node_id: u64,
        param_index: usize,
        param_name: impl Into<String>,
        min_value: f32,
        max_value: f32,
    ) -> Self {
        Self {
            cc_number,
            channel,
            node_id,
            param_index,
            param_name: param_name.into(),
            min_value,
            max_value,
        }
    }

    /// Check if this mapping matches a given CC event.
    pub fn matches(&self, cc_number: u8, channel: u8) -> bool {
        self.cc_number == cc_number && (self.channel == 0 || self.channel == channel + 1)
    }

    /// Convert a CC value (0-127) to the mapped parameter range.
    pub fn cc_to_value(&self, cc_value: u8) -> f32 {
        let normalized = cc_value as f32 / 127.0;
        self.min_value + normalized * (self.max_value - self.min_value)
    }
}

/// A complete synthesizer patch.
///
/// Contains all the information needed to recreate a graph configuration:
/// nodes with their positions and parameters, and all connections between them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Patch {
    /// Human-readable name for the patch.
    pub name: String,
    /// Patch format version for future compatibility.
    pub version: u32,
    /// All nodes in the patch.
    pub nodes: Vec<NodeData>,
    /// All connections between nodes.
    pub connections: Vec<ConnectionData>,
    /// MIDI CC mappings (optional for backwards compatibility).
    #[serde(default)]
    pub midi_mappings: Vec<MidiMapping>,
    /// Titled backdrops behind groups of modules. Only written when there
    /// are some; versions that predate them ignore the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<FrameData>,
    /// Text cards on the canvas. Only written when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteData>,
    /// Groups of modules collapsed into one node, each holding its own
    /// modules, cables and groups. Only written when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupData>,
}

impl Patch {
    /// Create a new empty patch with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: PATCH_VERSION,
            nodes: Vec::new(),
            connections: Vec::new(),
            midi_mappings: Vec::new(),
            frames: Vec::new(),
            notes: Vec::new(),
            groups: Vec::new(),
        }
    }

    /// Check if this patch version is compatible with the current format.
    pub fn is_compatible(&self) -> bool {
        self.version <= PATCH_VERSION
    }

    /// The oldest version that can read this patch, which is what it's
    /// written as: only a patch with groups needs v6.
    pub fn required_version(&self) -> u32 {
        if self.groups.is_empty() { VERSION_WITHOUT_GROUPS } else { PATCH_VERSION }
    }

    /// Every module in the patch, in groups or not.
    pub fn all_nodes(&self) -> Vec<&NodeData> {
        fn collect<'a>(nodes: &'a [NodeData], groups: &'a [GroupData], out: &mut Vec<&'a NodeData>) {
            out.extend(nodes);
            for group in groups {
                collect(&group.nodes, &group.groups, out);
            }
        }
        let mut out = Vec::new();
        collect(&self.nodes, &self.groups, &mut out);
        out
    }
}

impl Default for Patch {
    fn default() -> Self {
        Self::new("Untitled")
    }
}

/// Serialized data for a single node in the patch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeData {
    /// Unique identifier for this node within the patch.
    /// Used for referencing in connections.
    pub id: u64,
    /// Module type identifier (e.g., "osc.sine", "filter.svf").
    /// Must match a registered module ID.
    pub module_id: String,
    /// Node position in the graph editor (x, y).
    pub position: (f32, f32),
    /// Parameter values by name, in the order they appear in the node.
    /// These are the actual values (Hz for frequency, seconds for time, etc.).
    pub parameters: Vec<NamedParameter>,
    /// Whether the module is bypassed. Only written when true, so patches
    /// without bypassed modules read the same as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypassed: bool,
    /// Knobs shown on the faces of the groups around the module: parameter
    /// name to how many groups up it shows (1 is the group it's in). Only
    /// written when there are some.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pinned: BTreeMap<String, u8>,
    /// What pinned controls are called on the groups' faces, by parameter
    /// name, where they've been named; the rest wear their own short names.
    /// Only written when there are some.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub face_labels: BTreeMap<String, String>,
    /// The file the module plays (a Sampler's): relative to the patch, with
    /// forward slashes, when it's beside or below it, and absolute
    /// otherwise. Only written when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Names the module's user gave things that aren't parameters (an
    /// Arranger's sections and lanes), by key. Only written when there are
    /// some.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Slice markers placed by hand (a Sampler's, sliced by Onsets), with
    /// the Sensitivity they were placed at. Only written when there are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice_markers: Option<crate::dsp::onsets::HandMarkers>,
}

impl NodeData {
    /// Create new node data.
    pub fn new(id: u64, module_id: impl Into<String>, position: (f32, f32)) -> Self {
        Self {
            id,
            module_id: module_id.into(),
            position,
            parameters: Vec::new(),
            bypassed: false,
            pinned: BTreeMap::new(),
            face_labels: BTreeMap::new(),
            file: None,
            labels: BTreeMap::new(),
            slice_markers: None,
        }
    }
}

/// A group: modules collapsed into one node, with jacks of its own. It holds
/// its own small patch of modules, cables and groups, nested to any depth.
///
/// Its `id` is unique among the patch's module and group IDs. Outside, a
/// cable to or from it plugs into one of its jacks, by name. Inside, a cable
/// from it comes in through an input jack, and one to it goes out through an
/// output jack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroupData {
    pub id: u64,
    pub name: String,
    /// Where the group's node sits, on the level outside it.
    pub position: (f32, f32),
    pub inputs: Vec<JackData>,
    pub outputs: Vec<JackData>,
    /// Where the Inputs and Outputs nodes sit, inside.
    pub inputs_position: (f32, f32),
    pub outputs_position: (f32, f32),
    pub nodes: Vec<NodeData>,
    pub connections: Vec<ConnectionData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupData>,
}

/// One of a group's jacks: its name and the signal it carries
/// ("Audio", "Control", "Gate" or "MIDI").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JackData {
    pub name: String,
    #[serde(rename = "type")]
    pub signal: String,
}

/// A frame: a titled, tinted backdrop drawn behind a group of modules.
/// Positions and sizes are in the same units as node positions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrameData {
    pub title: String,
    /// Top-left corner.
    pub position: (f32, f32),
    /// Width and height.
    pub size: (f32, f32),
    /// The tint's name, e.g. "blue". An unknown name reads as the default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
}

/// A note: a card of text on the canvas. `**bold**` marks emphasis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteData {
    pub text: String,
    /// Top-left corner, in the same units as node positions.
    pub position: (f32, f32),
    /// Width the text wraps at. Its height follows from the text.
    pub width: f32,
}

/// A parameter value together with the name of the parameter it belongs to.
///
/// Serializes flat: `{"name": "Cutoff", "type": "Frequency", "value": 800.0}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NamedParameter {
    /// Parameter name, as shown on the node.
    pub name: String,
    /// The saved value.
    #[serde(flatten)]
    pub value: ParameterValue,
}

impl NamedParameter {
    /// Create a named parameter.
    pub fn new(name: impl Into<String>, value: ParameterValue) -> Self {
        Self { name: name.into(), value }
    }
}

/// A parameter value that preserves type information for proper restoration.
///
/// New patches save continuous values as `Number`. The older continuous tags
/// are still read; they all hold a value in real units.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum ParameterValue {
    /// Continuous value in the parameter's real units (Hz, seconds, dB, ...).
    Number(f32),
    /// Scalar value (0.0-1.0 range).
    Scalar(f32),
    /// Frequency value in Hz.
    Frequency(f32),
    /// Linear Hz value.
    LinearHz(f32),
    /// Time value in seconds.
    Time(f32),
    /// Linear range value (stored as raw value).
    LinearRange(f32),
    /// Boolean toggle.
    Toggle(bool),
    /// Selection index.
    Select(usize),
}

impl ParameterValue {
    /// Get the value as f32 for engine parameter setting.
    pub fn as_f32(&self) -> f32 {
        match self {
            Self::Number(v) => *v,
            Self::Scalar(v) => *v,
            Self::Frequency(v) => *v,
            Self::LinearHz(v) => *v,
            Self::Time(v) => *v,
            Self::LinearRange(v) => *v,
            Self::Toggle(v) => if *v { 1.0 } else { 0.0 },
            Self::Select(v) => *v as f32,
        }
    }
}

/// Serialized data for a connection between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectionData {
    /// Source node ID.
    pub from_node: u64,
    /// Output port name on source node.
    pub from_port: String,
    /// Destination node ID.
    pub to_node: u64,
    /// Input port name on destination node.
    pub to_port: String,
}

impl ConnectionData {
    /// Create new connection data.
    pub fn new(
        from_node: u64,
        from_port: impl Into<String>,
        to_node: u64,
        to_port: impl Into<String>,
    ) -> Self {
        Self {
            from_node,
            from_port: from_port.into(),
            to_node,
            to_port: to_port.into(),
        }
    }
}

/// Error type for patch operations.
#[derive(Debug)]
pub enum PatchError {
    /// File I/O error.
    IoError(std::io::Error),
    /// JSON serialization/deserialization error.
    SerializationError(serde_json::Error),
    /// Incompatible patch version.
    IncompatibleVersion { found: u32, expected: u32 },
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "File error: {}", e),
            Self::SerializationError(e) => write!(f, "Serialization error: {}", e),
            Self::IncompatibleVersion { found, expected } => {
                write!(f, "Incompatible patch version: found {}, expected <= {}", found, expected)
            }
        }
    }
}

impl std::error::Error for PatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IoError(e) => Some(e),
            Self::SerializationError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for PatchError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err)
    }
}

impl From<serde_json::Error> for PatchError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(err)
    }
}

/// A v1/v2 patch, whose parameters are stored by position.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchV2 {
    pub name: String,
    pub version: u32,
    pub nodes: Vec<NodeDataV2>,
    pub connections: Vec<ConnectionData>,
    #[serde(default)]
    pub midi_mappings: Vec<MidiMapping>,
}

/// A v1/v2 node: parameter values in the order the node's parameters appeared.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeDataV2 {
    pub id: u64,
    pub module_id: String,
    pub position: (f32, f32),
    pub parameters: Vec<ParameterValue>,
}

/// Parameter lists as they stood at v3, for modules whose list has changed
/// since. Positional values only mean something against those.
fn v3_parameter_names(module_id: &str) -> Option<Vec<String>> {
    let names: &[&str] = match module_id {
        "osc.sine" => &["Frequency", "FM Depth", "Waveform", "Pulse Width"],
        _ => return None,
    };
    Some(names.iter().map(|s| s.to_string()).collect())
}

/// Names positional parameters using each module's parameter list as of v3.
///
/// Positions only meant something against the parameter order at save time.
/// Modules that changed after v3 use their frozen v3 list; the rest use the
/// current one. Values past the end of the list, and all values of unknown
/// modules, are dropped.
pub fn migrate_v2_to_v3(old: PatchV2) -> Patch {
    let nodes = old
        .nodes
        .into_iter()
        .map(|node| {
            let names = v3_parameter_names(&node.module_id)
                .or_else(|| SynthNodeTemplate::from_module_id(&node.module_id).map(|t| t.parameter_names()))
                .unwrap_or_default();
            NodeData {
                id: node.id,
                module_id: node.module_id,
                position: node.position,
                parameters: names
                    .into_iter()
                    .zip(node.parameters)
                    .map(|(name, value)| NamedParameter { name, value })
                    .collect(),
                bypassed: false,
                pinned: BTreeMap::new(),
                face_labels: BTreeMap::new(),
                file: None,
                labels: BTreeMap::new(),
                slice_markers: None,
            }
        })
        .collect();

    Patch {
        name: old.name,
        version: 3,
        nodes,
        connections: old.connections,
        midi_mappings: old.midi_mappings,
        frames: Vec::new(),
        notes: Vec::new(),
        groups: Vec::new(),
    }
}

/// Converts v3 values that weren't in real units.
///
/// The filter's Drive knob sent 0-1 and the DSP mapped it to 1-10x; Drive is
/// now 1-10x end to end. MIDI mappings to Drive get the same conversion.
pub fn migrate_v3_to_v4(mut patch: Patch) -> Patch {
    let drive = |v: f32| 1.0 + 9.0 * v.clamp(0.0, 1.0);
    let mut filters = Vec::new();
    for node in patch.nodes.iter_mut().filter(|n| n.module_id == "filter.svf") {
        filters.push(node.id);
        for param in node.parameters.iter_mut().filter(|p| p.name == "Drive") {
            param.value = ParameterValue::Number(drive(param.value.as_f32()));
        }
    }
    for mapping in patch
        .midi_mappings
        .iter_mut()
        .filter(|m| m.param_name == "Drive" && filters.contains(&m.node_id))
    {
        mapping.min_value = drive(mapping.min_value);
        mapping.max_value = drive(mapping.max_value);
    }
    patch.version = 4;
    patch
}

/// Replaces the oscillator's Frequency knob with the tune section.
///
/// Frequency splits into the nearest Octave and Semitone with the rest in
/// Fine, so the pitch is unchanged. FM Depth was in Hz; it becomes an index
/// (Hz per Hz of pitch) at the saved frequency, which is the same depth at
/// that pitch and now follows the keyboard. A MIDI mapping to Frequency moves
/// to Octave over the same range; one to FM Depth is rescaled like the knob.
pub fn migrate_v4_to_v5(mut patch: Patch) -> Patch {
    let mut frequencies = Vec::new();
    for node in patch.nodes.iter_mut().filter(|n| n.module_id == "osc.sine") {
        let hz = node
            .parameters
            .iter()
            .find(|p| p.name == "Frequency")
            .map(|p| p.value.as_f32())
            .unwrap_or(Oscillator::C4_HZ);
        frequencies.push((node.id, hz));

        node.parameters.retain(|p| p.name != "Frequency");
        for param in node.parameters.iter_mut().filter(|p| p.name == "FM Depth") {
            param.value = ParameterValue::Number((param.value.as_f32() / hz).clamp(0.0, 5.0));
        }
        let (octave, semitone, fine) = Oscillator::tune_from_hz(hz);
        node.parameters.splice(
            0..0,
            [
                NamedParameter::new("Octave", ParameterValue::Number(octave)),
                NamedParameter::new("Semitone", ParameterValue::Number(semitone)),
                NamedParameter::new("Fine", ParameterValue::Number(fine)),
            ],
        );
    }

    for mapping in patch.midi_mappings.iter_mut() {
        let Some(&(_, hz)) = frequencies.iter().find(|(id, _)| *id == mapping.node_id) else {
            continue;
        };
        match mapping.param_name.as_str() {
            "Frequency" => {
                let octaves = |v: f32| (v.max(1e-3) / Oscillator::C4_HZ).log2().clamp(-4.0, 4.0);
                mapping.param_name = "Octave".to_string();
                mapping.min_value = octaves(mapping.min_value);
                mapping.max_value = octaves(mapping.max_value);
            }
            "FM Depth" => {
                mapping.min_value = (mapping.min_value / hz).clamp(0.0, 5.0);
                mapping.max_value = (mapping.max_value / hz).clamp(0.0, 5.0);
            }
            _ => {}
        }
    }
    patch.version = 5;
    patch
}

/// Keeps the Step Sequencer's old gate timing in patches saved before it had
/// a Gate Mode.
///
/// Gate Length used to be a share of a fixed 100 ms; it's now a share of the
/// step by default. A sequencer saved without Gate Mode is given the 100 ms
/// mode, so it plays as it always did. Every sequencer saved since has the
/// parameter, so this needs no version: older apps ignore it.
fn keep_fixed_sequencer_gates(nodes: &mut [NodeData], groups: &mut [GroupData]) {
    for node in nodes.iter_mut().filter(|n| n.module_id == "seq.step") {
        if !node.parameters.iter().any(|p| p.name == "Gate Mode") {
            let fixed = crate::modules::sequencer::GateMode::Fixed as usize;
            node.parameters.push(NamedParameter::new("Gate Mode", ParameterValue::Select(fixed)));
        }
    }
    for group in groups {
        keep_fixed_sequencer_gates(&mut group.nodes, &mut group.groups);
    }
}

/// Parses a patch from JSON, migrating older versions to the current format.
pub fn patch_from_json(json: &str) -> Result<Patch, PatchError> {
    #[derive(Deserialize)]
    struct VersionOnly {
        version: u32,
    }
    let VersionOnly { version } = serde_json::from_str(json)?;

    if version > PATCH_VERSION {
        return Err(PatchError::IncompatibleVersion {
            found: version,
            expected: PATCH_VERSION,
        });
    }
    let patch = if version < 3 {
        migrate_v2_to_v3(serde_json::from_str(json)?)
    } else {
        serde_json::from_str(json)?
    };
    let patch = if patch.version < 4 { migrate_v3_to_v4(patch) } else { patch };
    let mut patch = if patch.version < 5 { migrate_v4_to_v5(patch) } else { patch };
    keep_fixed_sequencer_gates(&mut patch.nodes, &mut patch.groups);
    Ok(patch)
}

/// A patch as the JSON its files hold.
pub fn patch_to_json(patch: &Patch) -> Result<String, PatchError> {
    Ok(serde_json::to_string_pretty(patch)?)
}

/// Save a patch to a JSON file.
pub fn save_to_file(patch: &Patch, path: &std::path::Path) -> Result<(), PatchError> {
    std::fs::write(path, patch_to_json(patch)?)?;
    Ok(())
}

/// Load a patch from a JSON file.
pub fn load_from_file(path: &std::path::Path) -> Result<Patch, PatchError> {
    let json = std::fs::read_to_string(path)?;
    patch_from_json(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frames_and_notes_survive_save_and_load() {
        let mut patch = Patch::new("Annotated");
        patch.frames.push(FrameData {
            title: "Voice".to_string(),
            position: (-40.0, 20.5),
            size: (420.0, 310.0),
            color: "teal".to_string(),
        });
        patch.notes.push(NoteData {
            text: "Turn **Resonance** up slowly.\nListen for the whistle.".to_string(),
            position: (400.0, -12.0),
            width: 260.0,
        });
        let path = std::env::temp_dir().join(format!("soba-annotated-{}.json", std::process::id()));
        save_to_file(&patch, &path).unwrap();
        let loaded = load_from_file(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(loaded.unwrap(), patch);
    }

    #[test]
    fn test_sequencers_saved_before_gate_mode_keep_100_ms_gates() {
        let gate_mode = |node: &NodeData| {
            node.parameters.iter().find(|p| p.name == "Gate Mode").map(|p| p.value.clone())
        };
        let sequencer = |id: u64, extra: &str| {
            format!(
                r#"{{"id": {id}, "module_id": "seq.step", "position": [0, 0], "parameters": [
                    {{"name": "Gate Length", "type": "Number", "value": 50.0}}{extra}]}}"#
            )
        };
        let step = r#", {"name": "Gate Mode", "type": "Select", "value": 0}"#;
        let json = format!(
            r#"{{"name": "Old", "version": 6, "nodes": [{}, {}], "connections": [],
                "groups": [{{"id": 9, "name": "G", "position": [0, 0], "inputs": [], "outputs": [],
                    "inputs_position": [0, 0], "outputs_position": [0, 0],
                    "nodes": [{}], "connections": []}}]}}"#,
            sequencer(1, ""),
            sequencer(2, step),
            sequencer(3, ""),
        );
        let patch = patch_from_json(&json).unwrap();
        let fixed = Some(ParameterValue::Select(crate::modules::sequencer::GateMode::Fixed as usize));
        assert_eq!(gate_mode(&patch.nodes[0]), fixed, "saved before Gate Mode");
        assert_eq!(gate_mode(&patch.nodes[1]), Some(ParameterValue::Select(0)), "saved with it");
        assert_eq!(gate_mode(&patch.groups[0].nodes[0]), fixed, "inside a group");
    }

    #[test]
    fn test_patches_without_frames_or_notes_read_and_write_as_before() {
        // A patch saved before frames and notes existed loads with none
        let old = r#"{"name": "Old", "version": 5, "nodes": [], "connections": []}"#;
        let patch = patch_from_json(old).unwrap();
        assert!(patch.frames.is_empty() && patch.notes.is_empty());

        // and saves without the fields, so its file doesn't change
        let json = serde_json::to_string(&patch).unwrap();
        assert!(!json.contains("frames") && !json.contains("notes"), "{json}");
    }

    #[test]
    fn test_fields_this_version_does_not_know_are_ignored() {
        // How an older version reads a patch with frames and notes: the same
        // way this one reads a field from the future
        let newer = r#"{"name": "New", "version": 5, "nodes": [], "connections": [],
            "frames": [], "sketches": [{"title": "Later"}]}"#;
        assert!(patch_from_json(newer).is_ok());
    }

    #[test]
    fn test_patch_creation() {
        let patch = Patch::new("Test Patch");
        assert_eq!(patch.name, "Test Patch");
        assert_eq!(patch.version, PATCH_VERSION);
        assert!(patch.nodes.is_empty());
        assert!(patch.connections.is_empty());
    }

    #[test]
    fn test_patch_serialization() {
        let mut patch = Patch::new("Test");
        patch.nodes.push(NodeData {
            id: 1,
            module_id: "osc.sine".to_string(),
            position: (100.0, 200.0),
            parameters: vec![
                NamedParameter::new("Frequency", ParameterValue::Frequency(440.0)),
                NamedParameter::new("Amplitude", ParameterValue::Scalar(0.5)),
            ],
            bypassed: false,
            pinned: BTreeMap::new(),
            face_labels: BTreeMap::new(),
            file: None,
            labels: BTreeMap::new(),
            slice_markers: None,
        });
        patch.connections.push(ConnectionData::new(1, "Out", 2, "In"));

        let json = serde_json::to_string(&patch).unwrap();
        let loaded: Patch = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.name, "Test");
        assert_eq!(loaded.nodes.len(), 1);
        assert_eq!(loaded.connections.len(), 1);
    }

    #[test]
    fn test_version_compatibility() {
        let patch = Patch::new("Test");
        assert!(patch.is_compatible());

        let future_patch = Patch {
            name: "Future".to_string(),
            version: PATCH_VERSION + 1,
            nodes: vec![],
            connections: vec![],
            midi_mappings: vec![],
            frames: vec![],
            notes: vec![],
            groups: vec![],
        };
        assert!(!future_patch.is_compatible());
    }

    #[test]
    fn test_parameter_value_as_f32() {
        assert!((ParameterValue::Scalar(0.5).as_f32() - 0.5).abs() < f32::EPSILON);
        assert!((ParameterValue::Frequency(440.0).as_f32() - 440.0).abs() < f32::EPSILON);
        assert!((ParameterValue::Toggle(true).as_f32() - 1.0).abs() < f32::EPSILON);
        assert!((ParameterValue::Toggle(false).as_f32()).abs() < f32::EPSILON);
        assert!((ParameterValue::Select(2).as_f32() - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_named_parameter_serializes_flat() {
        let param = NamedParameter::new("Cutoff", ParameterValue::Frequency(800.0));
        let json = serde_json::to_value(&param).unwrap();
        assert_eq!(json, serde_json::json!({"name": "Cutoff", "type": "Frequency", "value": 800.0}));
        assert_eq!(serde_json::from_value::<NamedParameter>(json).unwrap(), param);
    }

    #[test]
    fn test_v1_patch_without_mappings_migrates() {
        let json = r#"{
            "name": "Old", "version": 1,
            "nodes": [{"id": 0, "module_id": "output.audio", "position": [0.0, 0.0],
                       "parameters": [{"type": "Scalar", "value": 0.25}]}],
            "connections": []
        }"#;
        let patch = patch_from_json(json).unwrap();
        assert_eq!(patch.version, patch.required_version());
        assert_eq!(patch.nodes[0].parameters, vec![NamedParameter::new("Volume", ParameterValue::Scalar(0.25))]);
        assert!(patch.midi_mappings.is_empty());
    }

    #[test]
    fn test_migration_drops_values_it_cannot_name() {
        let json = r#"{
            "name": "Old", "version": 2,
            "nodes": [
                {"id": 0, "module_id": "output.audio", "position": [0.0, 0.0], "parameters": [
                    {"type": "Scalar", "value": 0.25}, {"type": "Toggle", "value": true},
                    {"type": "Toggle", "value": false}, {"type": "Scalar", "value": 9.0}]},
                {"id": 1, "module_id": "gone.module", "position": [0.0, 0.0], "parameters": [
                    {"type": "Scalar", "value": 0.5}]}
            ],
            "connections": []
        }"#;
        let patch = patch_from_json(json).unwrap();
        let names: Vec<_> = patch.nodes[0].parameters.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Volume", "Limiter", "Character"]);
        // Unknown modules keep their node (staging reports it) but lose their values
        assert_eq!(patch.nodes[1].module_id, "gone.module");
        assert!(patch.nodes[1].parameters.is_empty());
    }

    #[test]
    fn test_v3_filter_drive_migrates_to_real_units() {
        // v3 saved filter Drive as 0-1 (the DSP mapped it to 1-10x)
        let json = r#"{
            "name": "Old", "version": 3,
            "nodes": [
                {"id": 4, "module_id": "filter.svf", "position": [0.0, 0.0], "parameters": [
                    {"name": "Cutoff", "type": "Frequency", "value": 800.0},
                    {"name": "Drive", "type": "Scalar", "value": 0.5}]},
                {"id": 5, "module_id": "fx.distortion", "position": [0.0, 0.0], "parameters": [
                    {"name": "Drive", "type": "Scalar", "value": 0.5}]}
            ],
            "connections": [],
            "midi_mappings": [
                {"cc_number": 1, "channel": 0, "node_id": 4, "param_index": 2, "param_name": "Drive",
                 "min_value": 0.0, "max_value": 1.0},
                {"cc_number": 2, "channel": 0, "node_id": 5, "param_index": 0, "param_name": "Drive",
                 "min_value": 0.0, "max_value": 1.0}
            ]
        }"#;
        let patch = patch_from_json(json).unwrap();
        assert_eq!(patch.version, patch.required_version());
        let filter = &patch.nodes[0].parameters;
        assert_eq!(filter[0], NamedParameter::new("Cutoff", ParameterValue::Frequency(800.0)));
        assert_eq!(filter[1], NamedParameter::new("Drive", ParameterValue::Number(5.5)));
        // Distortion's Drive really is 0-1, so it's untouched
        assert_eq!(patch.nodes[1].parameters[0].value.as_f32(), 0.5);

        let ranges: Vec<_> = patch.midi_mappings.iter().map(|m| (m.min_value, m.max_value)).collect();
        assert_eq!(ranges, vec![(1.0, 10.0), (0.0, 1.0)]);
    }

    /// Pitch of an oscillator node's tune section, in Hz.
    fn tuned_hz(node: &NodeData) -> f32 {
        let get = |name: &str| node.parameters.iter().find(|p| p.name == name).unwrap().value.as_f32();
        Oscillator::C4_HZ * Oscillator::tune_octaves(get("Octave"), get("Semitone"), get("Fine")).exp2()
    }

    #[test]
    fn test_v4_oscillator_frequency_becomes_tune_section() {
        // v3 and v4 both stored Frequency by name, in Hz
        for (version, hz) in [3, 4].into_iter().flat_map(|v| [20.0f32, 55.0, 110.0, 261.62558, 440.0, 1234.5, 5000.0].map(|hz| (v, hz))) {
            let json = format!(
                r#"{{"name": "Old", "version": {version},
                    "nodes": [{{"id": 1, "module_id": "osc.sine", "position": [0.0, 0.0], "parameters": [
                        {{"name": "Frequency", "type": "Number", "value": {hz}}},
                        {{"name": "FM Depth", "type": "Number", "value": 110.0}},
                        {{"name": "Waveform", "type": "Select", "value": 1}}]}}],
                    "connections": []}}"#
            );
            let patch = patch_from_json(&json).unwrap();
            let node = &patch.nodes[0];
            let cents = 1200.0 * (tuned_hz(node) / hz).log2();
            assert!(cents.abs() < 0.01, "v{version}: {hz} Hz came back {cents:+.4} cents off");
            assert!(node.parameters.iter().all(|p| p.name != "Frequency"));
            let fm = node.parameters.iter().find(|p| p.name == "FM Depth").unwrap().value.as_f32();
            assert!((fm - (110.0 / hz).min(5.0)).abs() < 1e-5, "FM Depth {fm} at {hz} Hz");
            assert_eq!(node.parameters.iter().find(|p| p.name == "Waveform").unwrap().value, ParameterValue::Select(1));
        }
    }

    #[test]
    fn test_tune_split_reads_naturally() {
        // A4 is octave 0, +9 semitones, not octave 1, -3
        let (o, s, f) = Oscillator::tune_from_hz(440.0);
        assert_eq!((o, s), (0.0, 9.0));
        assert!(f.abs() < 0.01);
        // 110 Hz is A2: octave -2, +9
        let (o, s, f) = Oscillator::tune_from_hz(110.0);
        assert_eq!((o, s), (-2.0, 9.0));
        assert!(f.abs() < 0.01);
        // Out of reach: clamped to the top of the range
        assert_eq!(Oscillator::tune_from_hz(20000.0), (4.0, 12.0, 100.0));
    }

    #[test]
    fn test_v4_oscillator_midi_mappings_follow_the_tune_section() {
        let json = r#"{
            "name": "Old", "version": 4,
            "nodes": [{"id": 1, "module_id": "osc.sine", "position": [0.0, 0.0], "parameters": [
                {"name": "Frequency", "type": "Number", "value": 220.0},
                {"name": "FM Depth", "type": "Number", "value": 0.0}]}],
            "connections": [],
            "midi_mappings": [
                {"cc_number": 1, "channel": 0, "node_id": 1, "param_index": 0, "param_name": "Frequency",
                 "min_value": 65.40639, "max_value": 1046.5023},
                {"cc_number": 2, "channel": 0, "node_id": 1, "param_index": 1, "param_name": "FM Depth",
                 "min_value": 0.0, "max_value": 440.0}
            ]
        }"#;
        let patch = patch_from_json(json).unwrap();
        let tune = &patch.midi_mappings[0];
        assert_eq!(tune.param_name, "Octave");
        assert!((tune.min_value + 2.0).abs() < 1e-4 && (tune.max_value - 2.0).abs() < 1e-4);
        let fm = &patch.midi_mappings[1];
        assert_eq!((fm.min_value, fm.max_value), (0.0, 2.0));
    }

    #[test]
    fn test_v2_oscillator_keeps_its_pitch() {
        // v2 values are positional against the v3-era list [Frequency, FM Depth, Waveform, Pulse Width]
        let json = r#"{
            "name": "Old", "version": 2,
            "nodes": [{"id": 3, "module_id": "osc.sine", "position": [0.0, 0.0], "parameters": [
                {"type": "Frequency", "value": 110.0}, {"type": "LinearHz", "value": 0.0},
                {"type": "Select", "value": 1}, {"type": "LinearRange", "value": 0.3}]}],
            "connections": []
        }"#;
        let patch = patch_from_json(json).unwrap();
        let node = &patch.nodes[0];
        assert!((1200.0 * (tuned_hz(node) / 110.0).log2()).abs() < 0.01);
        let names: Vec<_> = node.parameters.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Octave", "Semitone", "Fine", "FM Depth", "Waveform", "Pulse Width"]);
        assert_eq!(node.parameters[5].value.as_f32(), 0.3);
    }

    #[test]
    fn test_v5_patch_is_not_migrated_again() {
        let json = r#"{
            "name": "New", "version": 5,
            "nodes": [{"id": 1, "module_id": "osc.sine", "position": [0.0, 0.0], "parameters": [
                {"name": "Octave", "type": "Number", "value": 1.0},
                {"name": "FM Depth", "type": "Number", "value": 2.0}]}],
            "connections": []
        }"#;
        let patch = patch_from_json(json).unwrap();
        assert_eq!(patch.nodes[0].parameters.len(), 2);
        assert_eq!(patch.nodes[0].parameters[1].value, ParameterValue::Number(2.0));
    }

    #[test]
    fn test_v4_patch_is_not_migrated_again() {
        let json = r#"{
            "name": "New", "version": 4,
            "nodes": [{"id": 4, "module_id": "filter.svf", "position": [0.0, 0.0], "parameters": [
                {"name": "Drive", "type": "Number", "value": 5.5}]}],
            "connections": []
        }"#;
        let patch = patch_from_json(json).unwrap();
        assert_eq!(patch.nodes[0].parameters[0].value, ParameterValue::Number(5.5));
    }

    #[test]
    fn test_future_version_is_rejected() {
        let json = format!(r#"{{"name": "New", "version": {}, "nodes": [], "connections": []}}"#, PATCH_VERSION + 1);
        assert!(matches!(
            patch_from_json(&json),
            Err(PatchError::IncompatibleVersion { found, .. }) if found == PATCH_VERSION + 1
        ));
    }
}
