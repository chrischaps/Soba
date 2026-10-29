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

/// What a mapping hears: a CC, or a note from a pad or a key (#423).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MidiSource {
    #[default]
    Cc,
    Note,
}

impl MidiSource {
    fn is_cc(&self) -> bool {
        *self == Self::Cc
    }
}

/// The kind of control a mapping turns, which says what a CC or a note
/// does to it (#423).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MidiControl {
    /// A CC turns it across its range; a note holds it at the top.
    #[default]
    Knob,
    /// A CC sets it on at 64 or more; a note flips it.
    Toggle,
    /// A CC sweeps across the choices; a note steps to the next.
    Select,
    /// A footswitch: pressed while a note is held, or a CC is 64 or more.
    Button,
}

impl MidiControl {
    fn is_knob(&self) -> bool {
        *self == Self::Knob
    }
}

/// How a CC's values move its control (#424).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MidiEncoding {
    /// A knob or fader with ends: the value says where it is.
    #[default]
    Absolute,
    /// An endless encoder sending 64 plus the steps it turned up, or 64
    /// minus those it turned down.
    Relative64,
    /// An endless encoder sending 1 to 63 for steps up, and 127 down to 65
    /// for steps down.
    TwosComplement,
}

impl MidiEncoding {
    fn is_absolute(&self) -> bool {
        *self == Self::Absolute
    }

    /// The steps an encoder turned to send `value`, up or down, or `None`
    /// from a knob or fader with ends.
    pub fn steps(self, value: u8) -> Option<i32> {
        let value = value as i32;
        match self {
            Self::Absolute => None,
            Self::Relative64 => Some(value - 64),
            Self::TwosComplement if value >= 64 => Some(value - 128),
            Self::TwosComplement => Some(value),
        }
    }

    /// What the first few values a control sent say it is.
    ///
    /// An endless encoder stays close to its resting value (64, or the ends
    /// for two's complement). A knob or fader with ends walks: one value
    /// after another in one direction, never the same twice, never more
    /// than two apart. So values near the rest are an encoder unless they
    /// walk. An encoder turned at one speed repeats a value, a brisk one
    /// jumps about or turns back (65, 67, 70, 66), and one that speeds up
    /// (65, 66, 68, 71) takes a step of three or more. All of those lean
    /// to an encoder.
    ///
    /// Two cases can't be told apart: a fader flicked so fast that its
    /// first few values near the middle skip by three or more (read as an
    /// encoder), and an encoder turned one slow click at a time in one
    /// direction for every value heard (read as a fader). Either is put
    /// right in the knob's MIDI menu.
    pub fn guess(values: &[u8]) -> Self {
        if values.len() < 3 {
            return Self::Absolute;
        }
        let near_64 = values.iter().all(|v| (57..=71).contains(v));
        let near_ends = values.iter().all(|v| (1..=15).contains(v) || (113..=127).contains(v));
        if !near_64 && !near_ends {
            return Self::Absolute;
        }
        let steps: Vec<i16> = values.windows(2).map(|w| i16::from(w[1]) - i16::from(w[0])).collect();
        let walks = steps.iter().all(|d| (1..=2).contains(d)) || steps.iter().all(|d| (-2..=-1).contains(d));
        if walks {
            Self::Absolute
        } else if near_64 {
            Self::Relative64
        } else {
            Self::TwosComplement
        }
    }
}

/// What a mapped CC or note asks of its control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiInput {
    /// A CC's value (0-127).
    Cc(u8),
    NoteOn,
    NoteOff,
}

impl MidiInput {
    /// Is it a press: a note struck, or a CC at 64 or more?
    pub fn presses(self) -> bool {
        match self {
            Self::Cc(value) => value >= 64,
            Self::NoteOn => true,
            Self::NoteOff => false,
        }
    }
}

/// A MIDI CC or note to parameter mapping.
///
/// Maps a hardware knob, fader, pad or key to a synthesizer parameter, or
/// to a Scenes pad.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MidiMapping {
    /// CC number (0-127), or the note's when `source` is a note.
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
    /// In the plugin, the host macro (0-7) that turns the knob instead of a
    /// CC: the DAW automates it. `cc_number` and `channel` are unused then.
    #[serde(default, rename = "macro", skip_serializing_if = "Option::is_none")]
    pub macro_slot: Option<u8>,
    /// A CC (all there was before #423), or a note.
    #[serde(default, skip_serializing_if = "MidiSource::is_cc")]
    pub source: MidiSource,
    /// What kind of control it turns.
    #[serde(default, skip_serializing_if = "MidiControl::is_knob")]
    pub control: MidiControl,
    /// The Scenes pad (0-7) it recalls, in place of a parameter:
    /// `param_index` is unused then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<u8>,
    /// A knob or fader with ends, or an endless encoder (#424).
    #[serde(default, skip_serializing_if = "MidiEncoding::is_absolute")]
    pub encoding: MidiEncoding,
    /// A knob the CC turns jumps to wherever the hardware is, rather than
    /// waiting for the hardware to pick it up where it stands (#424).
    #[serde(default, skip_serializing_if = "is_false")]
    pub jump: bool,
    /// The hardware's top is the control's bottom.
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert: bool,
    /// The device it's heard from, by name (#435), or `None` for any: a
    /// mapping learned while several were open keeps the one it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
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
            macro_slot: None,
            source: MidiSource::Cc,
            control: MidiControl::Knob,
            scene: None,
            encoding: MidiEncoding::Absolute,
            jump: false,
            invert: false,
            device: None,
        }
    }

    /// The mapping, heard from a note (a pad or a key) instead of a CC.
    pub fn note(self) -> Self {
        Self { source: MidiSource::Note, ..self }
    }

    /// The mapping, turning a control of kind `control`.
    pub fn of(self, control: MidiControl) -> Self {
        Self { control, ..self }
    }

    /// The mapping, recalling Scenes pad `slot` instead of a parameter.
    pub fn recalling(self, slot: u8) -> Self {
        Self { scene: Some(slot), ..self }
    }

    /// A mapping from the plugin's host macro `slot` (0-7) instead of a CC.
    pub fn from_macro(slot: u8, node_id: u64, param_index: usize, param_name: impl Into<String>, min_value: f32, max_value: f32) -> Self {
        Self { macro_slot: Some(slot), ..Self::new(0, 0, node_id, param_index, param_name, min_value, max_value) }
    }

    /// Does it hear `number` from `source` on `channel` (0-15)?
    pub fn hears(&self, source: MidiSource, number: u8, channel: u8) -> bool {
        self.macro_slot.is_none() && self.source == source && self.cc_number == number && (self.channel == 0 || self.channel == channel + 1)
    }

    /// Does it hear what the device called `device` sends? What Soba plays
    /// itself (`None`), every mapping hears.
    pub fn hears_device(&self, device: Option<&str>) -> bool {
        match (self.device.as_deref(), device) {
            // The same device, though the system may have renumbered it (#436)
            (Some(own), Some(device)) => crate::engine::device_names::same(own, device),
            _ => true,
        }
    }

    /// What `input` sets its control to, from `current`, or `None` when it
    /// leaves it be (a toggle's note let go).
    pub fn respond(&self, input: MidiInput, current: f32) -> Option<f32> {
        let (min, max) = (self.min_value, self.max_value);
        let pressed = if input.presses() { max } else { min };
        match (self.control, input) {
            (MidiControl::Knob, MidiInput::Cc(value)) => Some(self.cc_to_value(value)),
            (MidiControl::Select, MidiInput::Cc(value)) => Some(self.cc_to_value(value).round()),
            (MidiControl::Toggle, MidiInput::NoteOn) => Some(if current > (min + max) / 2.0 { min } else { max }),
            (MidiControl::Select, MidiInput::NoteOn) => {
                let next = current.round() + 1.0;
                Some(if next > max { min } else { next })
            }
            (MidiControl::Toggle | MidiControl::Select, MidiInput::NoteOff) => None,
            // A knob's note, a toggle's CC, and a footswitch from either
            _ => Some(pressed),
        }
    }

    /// The macro's value (0-1) in the mapped parameter range.
    pub fn macro_to_value(&self, normalized: f32) -> f32 {
        self.min_value + normalized.clamp(0.0, 1.0) * (self.max_value - self.min_value)
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

    /// Where cables from module `node`'s output `port` end, through any
    /// groups' jacks between: each module's ID and input port.
    pub fn leaf_inputs(&self, node: u64, port: &str) -> Vec<(u64, String)> {
        /// One level of the patch: its group (none at the top), the level
        /// outside it, and what's on it.
        struct Level<'a> {
            group: Option<u64>,
            outside: Option<usize>,
            nodes: &'a [NodeData],
            connections: &'a [ConnectionData],
        }
        fn gather<'a>(levels: &mut Vec<Level<'a>>, group: Option<u64>, outside: Option<usize>, nodes: &'a [NodeData], connections: &'a [ConnectionData], groups: &'a [GroupData]) {
            let here = levels.len();
            levels.push(Level { group, outside, nodes, connections });
            for inner in groups {
                gather(levels, Some(inner.id), Some(here), &inner.nodes, &inner.connections, &inner.groups);
            }
        }
        let mut levels = Vec::new();
        gather(&mut levels, None, None, &self.nodes, &self.connections, &self.groups);
        let Some(start) = levels.iter().position(|level| level.nodes.iter().any(|n| n.id == node)) else { return Vec::new() };
        let mut ends = Vec::new();
        let mut trail = vec![(start, node, port.to_string())];
        while let Some((at, from, port)) = trail.pop() {
            let level = &levels[at];
            for cable in level.connections.iter().filter(|c| c.from_node == from && c.from_port == port) {
                if Some(cable.to_node) == level.group {
                    // Out through the group's jack, from the group outside it
                    trail.extend(level.outside.map(|outside| (outside, cable.to_node, cable.to_port.clone())));
                } else if let Some(inner) = levels.iter().position(|l| l.group == Some(cable.to_node) && l.outside == Some(at)) {
                    // Into a group, from its jack inside
                    trail.push((inner, cable.to_node, cable.to_port.clone()));
                } else {
                    ends.push((cable.to_node, cable.to_port.clone()));
                }
            }
        }
        ends
    }

    /// Every module in the patch, in groups or not, to change.
    pub fn all_nodes_mut(&mut self) -> Vec<&mut NodeData> {
        fn collect<'a>(nodes: &'a mut [NodeData], groups: &'a mut [GroupData], out: &mut Vec<&'a mut NodeData>) {
            out.extend(nodes);
            for group in groups {
                collect(&mut group.nodes, &mut group.groups, out);
            }
        }
        let mut out = Vec::new();
        collect(&mut self.nodes, &mut self.groups, &mut out);
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
    /// A Scenes module's scenes, by slot letter: each knob's value by
    /// module ID and parameter name. Only written when there are some.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scenes: crate::graph::scenes::SceneBank,
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
            scenes: Default::default(),
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
/// ("Audio", "Control", "Gate", "MIDI" or "Bus").
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
/// are still read; they all hold a value in real units. A tag this build
/// doesn't know (one a newer Soba added) reads as `Unknown` rather than failing
/// the whole patch, and the loader skips it with a warning (#330).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(try_from = "RawParameterValue")]
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
    /// A type this build doesn't know, kept as saved so it writes back unchanged.
    Unknown { kind: String, value: serde_json::Value },
}

/// `ParameterValue` as the JSON holds it: `{"type": ..., "value": ...}`.
#[derive(Deserialize)]
struct RawParameterValue {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    value: serde_json::Value,
}

impl TryFrom<RawParameterValue> for ParameterValue {
    type Error = serde_json::Error;

    fn try_from(raw: RawParameterValue) -> Result<Self, Self::Error> {
        let number = |value| serde_json::from_value::<f32>(value);
        Ok(match raw.kind.as_str() {
            "Number" => Self::Number(number(raw.value)?),
            "Scalar" => Self::Scalar(number(raw.value)?),
            "Frequency" => Self::Frequency(number(raw.value)?),
            "LinearHz" => Self::LinearHz(number(raw.value)?),
            "Time" => Self::Time(number(raw.value)?),
            "LinearRange" => Self::LinearRange(number(raw.value)?),
            "Toggle" => Self::Toggle(serde_json::from_value(raw.value)?),
            "Select" => Self::Select(serde_json::from_value(raw.value)?),
            _ => Self::Unknown { kind: raw.kind, value: raw.value },
        })
    }
}

/// Writes the same `{"type": ..., "value": ...}` the reader takes, with numbers
/// as f32 so they print as short as they always have.
impl Serialize for ParameterValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("ParameterValue", 2)?;
        out.serialize_field("type", self.kind())?;
        match self {
            Self::Number(v) | Self::Scalar(v) | Self::Frequency(v) | Self::LinearHz(v) | Self::Time(v) | Self::LinearRange(v) => {
                out.serialize_field("value", v)?
            }
            Self::Toggle(v) => out.serialize_field("value", v)?,
            Self::Select(v) => out.serialize_field("value", v)?,
            Self::Unknown { value, .. } => out.serialize_field("value", value)?,
        }
        out.end()
    }
}

impl ParameterValue {
    /// The `type` tag the JSON holds.
    pub fn kind(&self) -> &str {
        match self {
            Self::Number(_) => "Number",
            Self::Scalar(_) => "Scalar",
            Self::Frequency(_) => "Frequency",
            Self::LinearHz(_) => "LinearHz",
            Self::Time(_) => "Time",
            Self::LinearRange(_) => "LinearRange",
            Self::Toggle(_) => "Toggle",
            Self::Select(_) => "Select",
            Self::Unknown { kind, .. } => kind,
        }
    }

    /// Get the value as f32 for engine parameter setting. An `Unknown` value
    /// has none and reads as 0; the loader skips those before asking.
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
            Self::Unknown { .. } => 0.0,
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
                write!(
                    f,
                    "This patch was made by a newer version of Soba (format {}; this one reads up to {}). Update Soba to open it.",
                    found, expected
                )
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
            scenes: Default::default(),
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

/// The modules Note In took the place of (#385), by the IDs patches knew
/// them by: each becomes a Note In with one voice, played from where it was.
const NOTE_INPUTS: [(&str, NoteSource); 2] = [("input.keyboard", NoteSource::Keys), ("input.midi_note", NoteSource::Midi)];

/// Where a Note In made from an old module plays from, as its Source.
#[derive(Clone, Copy)]
enum NoteSource {
    Keys = 1,
    Midi = 2,
}

/// Keyboard and MIDI Note became Note In with one voice, so the patches
/// that have them open and play as they did. Keyboard's Velocity is the
/// velocity its keys play, Note In's Key Velocity; their live state (Note,
/// Gate and so on, which the player set) goes. A MIDI mapping follows its
/// knob to its new name and place.
///
/// No version marks this: a patch holding either ID is an old one, and an
/// older app opens a Note In as the Poly MIDI it knows.
fn old_note_inputs_to_note_in(patch: &mut Patch) {
    fn migrate(nodes: &mut [NodeData], groups: &mut [GroupData], migrated: &mut Vec<(u64, NoteSource)>) {
        for node in nodes.iter_mut() {
            let Some(&(_, source)) = NOTE_INPUTS.iter().find(|(id, _)| *id == node.module_id) else { continue };
            node.module_id = crate::modules::note_in::ID.to_string();
            let names = note_in_parameter_names();
            for param in node.parameters.iter_mut() {
                param.name = source.renamed(&param.name).to_string();
            }
            node.parameters.retain(|p| names.contains(&p.name));
            node.parameters.push(NamedParameter::new("Voices", ParameterValue::Number(1.0)));
            node.parameters.push(NamedParameter::new("Source", ParameterValue::Select(source as usize)));
            node.pinned = std::mem::take(&mut node.pinned)
                .into_iter()
                .map(|(name, levels)| (source.renamed(&name).to_string(), levels))
                .filter(|(name, _)| names.contains(name))
                .collect();
            node.face_labels = std::mem::take(&mut node.face_labels)
                .into_iter()
                .map(|(name, label)| (source.renamed(&name).to_string(), label))
                .filter(|(name, _)| names.contains(name))
                .collect();
            migrated.push((node.id, source));
        }
        for group in groups {
            migrate(&mut group.nodes, &mut group.groups, migrated);
        }
    }

    let mut migrated = Vec::new();
    migrate(&mut patch.nodes, &mut patch.groups, &mut migrated);
    if migrated.is_empty() {
        return;
    }
    let names = note_in_parameter_names();
    patch.midi_mappings.retain_mut(|mapping| {
        let Some(&(_, source)) = migrated.iter().find(|(id, _)| *id == mapping.node_id) else {
            return true;
        };
        mapping.param_name = source.renamed(&mapping.param_name).to_string();
        match names.iter().position(|name| *name == mapping.param_name) {
            Some(index) => {
                mapping.param_index = index;
                true
            }
            None => false,
        }
    });
}

/// Note In's parameter names, in order.
fn note_in_parameter_names() -> Vec<String> {
    SynthNodeTemplate::from_module_id(crate::modules::note_in::ID).map(|t| t.parameter_names()).unwrap_or_default()
}

impl NoteSource {
    /// What a parameter of the module this was is called on Note In:
    /// Keyboard's Velocity is the keys' velocity. (MIDI Note's was the
    /// note's, live state that goes.)
    fn renamed(self, name: &str) -> &str {
        match (self, name) {
            (NoteSource::Keys, "Velocity") => "Key Velocity",
            _ => name,
        }
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
    old_note_inputs_to_note_in(&mut patch);
    mixer_mono_out_through_a_mix(&mut patch);
    super::pads_group::pads_to_groups(&mut patch);
    Ok(patch)
}

/// The Mixer's mono Out, the sum of its channels at their levels ignoring
/// pan, went in b3ee02f (#373). Each Mixer an old patch cabled from it gets
/// a Mix beside it adding Out L and Out R, and those cables leave from the
/// Mix. Its levels undo the equal-power pan's 3 dB, so a centred channel
/// comes out as it did; one panned hard to a side is 3 dB quieter.
///
/// No version marks this: only an old patch has a cable from a Mixer's Out.
fn mixer_mono_out_through_a_mix(patch: &mut Patch) {
    fn last_id(nodes: &[NodeData], groups: &[GroupData]) -> u64 {
        let here = nodes.iter().map(|n| n.id).chain(groups.iter().map(|g| g.id)).max().unwrap_or(0);
        groups.iter().map(|g| last_id(&g.nodes, &g.groups)).fold(here, u64::max)
    }

    fn migrate(nodes: &mut Vec<NodeData>, connections: &mut Vec<ConnectionData>, groups: &mut [GroupData], next: &mut u64) {
        // Each Mixer cabled from its Out, and the Mix that takes its place
        let mut mixes: Vec<(u64, u64)> = Vec::new();
        for conn in connections.iter_mut().filter(|c| c.from_port == "Out") {
            if !nodes.iter().any(|n| n.id == conn.from_node && n.module_id == crate::modules::mixer::ID) {
                continue;
            }
            let mix = match mixes.iter().find(|(mixer, _)| *mixer == conn.from_node) {
                Some(&(_, mix)) => mix,
                None => {
                    *next += 1;
                    mixes.push((conn.from_node, *next));
                    *next
                }
            };
            conn.from_node = mix;
        }
        for &(mixer, mix) in &mixes {
            let (x, y) = nodes.iter().find(|n| n.id == mixer).map_or((0.0, 0.0), |n| n.position);
            let mut node = NodeData::new(mix, crate::modules::mix::ID, (x + 440.0, y));
            let level = || ParameterValue::Number(std::f32::consts::FRAC_1_SQRT_2);
            node.parameters = vec![NamedParameter::new("Level 1", level()), NamedParameter::new("Level 2", level())];
            nodes.push(node);
            connections.push(ConnectionData::new(mixer, "Out L", mix, "In 1"));
            connections.push(ConnectionData::new(mixer, "Out R", mix, "In 2"));
        }
        for group in groups {
            migrate(&mut group.nodes, &mut group.connections, &mut group.groups, next);
        }
    }

    let mut next = last_id(&patch.nodes, &patch.groups);
    migrate(&mut patch.nodes, &mut patch.connections, &mut patch.groups, &mut next);
}

/// A patch as the JSON its files hold.
pub fn patch_to_json(patch: &Patch) -> Result<String, PatchError> {
    Ok(serde_json::to_string_pretty(patch)?)
}

/// Save a patch to a JSON file.
///
/// The patch is written beside the file as `<name>.partial`, flushed to disk and
/// then renamed over it, so a crash, a full disk or a sync client grabbing the file
/// mid-write leaves the old patch whole rather than truncated (#310).
pub fn save_to_file(patch: &Patch, path: &std::path::Path) -> Result<(), PatchError> {
    use std::io::Write;
    let json = patch_to_json(patch)?;
    let mut partial_name = path.file_name().unwrap_or_default().to_os_string();
    partial_name.push(".partial");
    let partial = path.with_file_name(partial_name);
    let written = (|| {
        let mut file = std::fs::File::create(&partial)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&partial, path)
    })();
    if written.is_err() {
        std::fs::remove_file(&partial).ok();
    }
    Ok(written?)
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
    fn test_save_replaces_the_patch_and_leaves_no_partial_file() {
        let dir = std::env::temp_dir().join(format!("soba-atomic-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.json");
        save_to_file(&Patch::new("First"), &path).unwrap();
        save_to_file(&Patch::new("Second"), &path).unwrap();
        let loaded = load_from_file(&path).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(loaded.name, "Second");
        assert_eq!(leftovers, vec![std::ffi::OsString::from("song.json")]);
    }

    #[test]
    fn test_a_failed_save_leaves_the_old_patch_intact() {
        let dir = std::env::temp_dir().join(format!("soba-failed-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.json");
        save_to_file(&Patch::new("Original"), &path).unwrap();
        // A folder where the partial file should go makes the write fail before the rename
        std::fs::create_dir_all(dir.join("song.json.partial")).unwrap();
        let failed = save_to_file(&Patch::new("Replacement"), &path);
        let loaded = load_from_file(&path);
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(failed, Err(PatchError::IoError(_))));
        assert_eq!(loaded.unwrap().name, "Original");
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
    fn test_pad_and_scene_mappings_round_trip_and_cc_ones_are_written_as_before() {
        let mut patch = Patch::new("Pads");
        let cc = MidiMapping::new(74, 0, 1, 0, "Cutoff", 0.0, 1.0);
        let pad = MidiMapping::new(36, 10, 2, 8, "Pedal Rec", 0.0, 1.0).note().of(MidiControl::Button);
        let scene = MidiMapping::new(40, 10, 3, 0, "Scene C", 0.0, 1.0).note().of(MidiControl::Button).recalling(2);
        patch.midi_mappings = vec![cc.clone(), pad, scene];
        let json = patch_to_json(&patch).unwrap();
        assert_eq!(patch_from_json(&json).unwrap().midi_mappings, patch.midi_mappings);

        // A CC on a knob, all there was before #423, gains no fields
        let old = serde_json::to_value(&cc).unwrap();
        assert_eq!(old.as_object().unwrap().len(), 7, "{old}");
        let styled = MidiMapping { encoding: MidiEncoding::TwosComplement, jump: true, invert: true, ..cc.clone() };
        let json = serde_json::to_string(&styled).unwrap();
        assert!(json.contains("\"twos-complement\""), "{json}");
        assert_eq!(serde_json::from_str::<MidiMapping>(&json).unwrap(), styled);
        let read: MidiMapping = serde_json::from_value(old).unwrap();
        assert_eq!((read.source, read.control, read.scene), (MidiSource::Cc, MidiControl::Knob, None));
    }

    #[test]
    fn test_a_mapping_from_one_device_hears_only_it_and_round_trips() {
        let any = MidiMapping::new(74, 0, 1, 0, "Cutoff", 0.0, 1.0);
        let fantom = MidiMapping { device: Some("Fantom-06".into()), ..any.clone() };
        assert!(any.hears_device(Some("MiniLab3 MIDI")) && any.hears_device(None));
        assert!(fantom.hears_device(Some("Fantom-06")));
        assert!(!fantom.hears_device(Some("MiniLab3 MIDI")));
        assert!(fantom.hears_device(None), "what Soba plays itself, every mapping hears");

        let json = serde_json::to_string(&fantom).unwrap();
        assert!(json.contains("\"device\":\"Fantom-06\""), "{json}");
        assert_eq!(serde_json::from_str::<MidiMapping>(&json).unwrap(), fantom);
        assert!(!serde_json::to_string(&any).unwrap().contains("device"), "any device is written as before");
    }

    #[test]
    fn test_an_encoder_is_told_from_a_fader_by_its_first_few_values() {
        assert_eq!(MidiEncoding::guess(&[65, 65, 65, 63]), MidiEncoding::Relative64);
        assert_eq!(MidiEncoding::guess(&[1, 1, 2]), MidiEncoding::TwosComplement);
        assert_eq!(MidiEncoding::guess(&[127, 127, 127]), MidiEncoding::TwosComplement);
        // A fader walks through its values, wherever it starts
        assert_eq!(MidiEncoding::guess(&[63, 64, 65, 66]), MidiEncoding::Absolute);
        assert_eq!(MidiEncoding::guess(&[1, 2, 3, 4]), MidiEncoding::Absolute);
        assert_eq!(MidiEncoding::guess(&[40, 40, 40]), MidiEncoding::Absolute);
        // Too few to say
        assert_eq!(MidiEncoding::guess(&[65]), MidiEncoding::Absolute);
    }

    #[test]
    fn test_an_accelerated_encoder_is_not_taken_for_a_fader() {
        // Brisk turns jump about and turn back
        assert_eq!(MidiEncoding::guess(&[65, 67, 70, 66]), MidiEncoding::Relative64);
        assert_eq!(MidiEncoding::guess(&[1, 3, 2, 5]), MidiEncoding::TwosComplement);
        assert_eq!(MidiEncoding::guess(&[63, 61, 58, 62]), MidiEncoding::Relative64);
        assert_eq!(MidiEncoding::guess(&[127, 125, 126, 122]), MidiEncoding::TwosComplement);
        // Speeding up in one direction takes a step of three or more
        assert_eq!(MidiEncoding::guess(&[65, 66, 68, 71]), MidiEncoding::Relative64);
        assert_eq!(MidiEncoding::guess(&[127, 126, 124, 121]), MidiEncoding::TwosComplement);
        // A steady turn repeats a value
        assert_eq!(MidiEncoding::guess(&[65, 65, 65]), MidiEncoding::Relative64);
        // Crossing the ends is a jump, not a walk
        assert_eq!(MidiEncoding::guess(&[127, 1, 2]), MidiEncoding::TwosComplement);
    }

    #[test]
    fn test_a_fader_that_walks_is_still_a_fader() {
        assert_eq!(MidiEncoding::guess(&[62, 63, 64, 65]), MidiEncoding::Absolute);
        assert_eq!(MidiEncoding::guess(&[1, 2, 3, 4]), MidiEncoding::Absolute);
        assert_eq!(MidiEncoding::guess(&[66, 65, 64, 63]), MidiEncoding::Absolute);
        assert_eq!(MidiEncoding::guess(&[125, 124, 123, 121]), MidiEncoding::Absolute);
        // Moved briskly, skipping a value now and then
        assert_eq!(MidiEncoding::guess(&[60, 62, 64, 66]), MidiEncoding::Absolute);
        // Away from the rest it is a fader whatever it does
        assert_eq!(MidiEncoding::guess(&[20, 40, 41, 80]), MidiEncoding::Absolute);
        assert_eq!(MidiEncoding::guess(&[0, 0, 1]), MidiEncoding::Absolute);
    }

    #[test]
    fn test_an_encoder_counts_its_steps() {
        assert_eq!(MidiEncoding::Relative64.steps(66), Some(2));
        assert_eq!(MidiEncoding::Relative64.steps(63), Some(-1));
        assert_eq!(MidiEncoding::TwosComplement.steps(1), Some(1));
        assert_eq!(MidiEncoding::TwosComplement.steps(127), Some(-1));
        assert_eq!(MidiEncoding::TwosComplement.steps(120), Some(-8));
        assert_eq!(MidiEncoding::Absolute.steps(127), None);
    }

    #[test]
    fn test_a_mapping_responds_as_its_control_does() {
        let knob = MidiMapping::new(1, 0, 1, 0, "Cutoff", 100.0, 200.0);
        assert_eq!(knob.respond(MidiInput::Cc(127), 150.0), Some(200.0));
        assert_eq!(knob.respond(MidiInput::NoteOn, 150.0), Some(200.0));
        assert_eq!(knob.respond(MidiInput::NoteOff, 150.0), Some(100.0));

        let toggle = MidiMapping::new(1, 0, 1, 0, "Sync", 0.0, 1.0).of(MidiControl::Toggle);
        assert_eq!(toggle.respond(MidiInput::Cc(64), 0.0), Some(1.0));
        assert_eq!(toggle.respond(MidiInput::Cc(63), 1.0), Some(0.0));
        assert_eq!(toggle.respond(MidiInput::NoteOn, 1.0), Some(0.0));
        assert_eq!(toggle.respond(MidiInput::NoteOff, 1.0), None);

        let select = MidiMapping::new(1, 0, 1, 0, "Wave", 0.0, 4.0).of(MidiControl::Select);
        assert_eq!(select.respond(MidiInput::Cc(64), 0.0), Some(2.0));
        assert_eq!(select.respond(MidiInput::NoteOn, 4.0), Some(0.0));
        assert_eq!(select.respond(MidiInput::NoteOff, 4.0), None);
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
            scenes: Default::default(),
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
    fn test_unknown_parameter_type_loads_and_writes_back() {
        let json = r#"{"name": "Morph", "type": "Curve", "value": {"points": [0.0, 1.0]}}"#;
        let param: NamedParameter = serde_json::from_str(json).unwrap();
        assert_eq!(param.value.kind(), "Curve");
        assert!(matches!(param.value, ParameterValue::Unknown { .. }));
        let back: serde_json::Value = serde_json::to_value(&param).unwrap();
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(json).unwrap());
    }

    #[test]
    fn test_known_parameter_types_round_trip_as_before() {
        let params = vec![
            NamedParameter::new("A", ParameterValue::Number(0.1)),
            NamedParameter::new("B", ParameterValue::Toggle(true)),
            NamedParameter::new("C", ParameterValue::Select(3)),
        ];
        let json = serde_json::to_string(&params).unwrap();
        assert_eq!(
            json,
            r#"[{"name":"A","type":"Number","value":0.1},{"name":"B","type":"Toggle","value":true},{"name":"C","type":"Select","value":3}]"#
        );
        assert_eq!(serde_json::from_str::<Vec<NamedParameter>>(&json).unwrap(), params);
        // A known type with the wrong kind of value is still an error
        assert!(serde_json::from_str::<NamedParameter>(r#"{"name":"A","type":"Toggle","value":2.5}"#).is_err());
    }

    #[test]
    fn test_newer_version_error_says_to_update() {
        let message = PatchError::IncompatibleVersion { found: 7, expected: 6 }.to_string();
        assert!(message.contains("newer version of Soba") && message.contains("Update Soba"), "{message}");
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

    fn param<'a>(node: &'a NodeData, name: &str) -> Option<&'a ParameterValue> {
        node.parameters.iter().find(|p| p.name == name).map(|p| &p.value)
    }

    #[test]
    fn test_keyboard_becomes_a_one_voice_note_in_played_from_the_keys() {
        let json = r#"{
            "name": "Old", "version": 5,
            "nodes": [{"id": 1, "module_id": "input.keyboard", "position": [0.0, 0.0], "parameters": [
                {"name": "Note", "type": "Number", "value": 64.0},
                {"name": "Gate", "type": "Toggle", "value": true},
                {"name": "Octave", "type": "Number", "value": -1.0},
                {"name": "Velocity", "type": "Scalar", "value": 0.8},
                {"name": "Priority", "type": "Select", "value": 2},
                {"name": "Glide", "type": "Time", "value": 0.1},
                {"name": "Glide Mode", "type": "Select", "value": 0}],
              "pinned": {"Velocity": 1, "Gate": 1}, "face_labels": {"Velocity": "Touch"}}],
            "connections": [],
            "midi_mappings": [
                {"cc_number": 7, "channel": 0, "node_id": 1, "param_index": 3, "param_name": "Velocity",
                 "min_value": 0.0, "max_value": 1.0},
                {"cc_number": 8, "channel": 0, "node_id": 1, "param_index": 1, "param_name": "Gate",
                 "min_value": 0.0, "max_value": 1.0}
            ]
        }"#;
        let patch = patch_from_json(json).unwrap();
        let node = &patch.nodes[0];
        assert_eq!(node.module_id, crate::modules::note_in::ID);
        assert_eq!(param(node, "Voices"), Some(&ParameterValue::Number(1.0)));
        assert_eq!(param(node, "Source"), Some(&ParameterValue::Select(1)), "the keys");
        assert_eq!(param(node, "Key Velocity"), Some(&ParameterValue::Scalar(0.8)));
        assert_eq!(param(node, "Octave"), Some(&ParameterValue::Number(-1.0)));
        assert_eq!(param(node, "Priority"), Some(&ParameterValue::Select(2)));
        assert_eq!(param(node, "Glide"), Some(&ParameterValue::Time(0.1)));
        assert!(param(node, "Note").is_none() && param(node, "Gate").is_none(), "the player's state goes");
        assert_eq!(node.pinned.keys().collect::<Vec<_>>(), ["Key Velocity"]);
        assert_eq!(node.face_labels["Key Velocity"], "Touch");

        // The mapping follows its knob; one to the old live Gate goes
        assert_eq!(patch.midi_mappings.len(), 1);
        let mapping = &patch.midi_mappings[0];
        assert_eq!(mapping.param_name, "Key Velocity");
        assert_eq!(mapping.param_index, crate::modules::NoteIn::PARAM_KEY_VELOCITY);
    }

    #[test]
    fn test_midi_note_becomes_a_one_voice_note_in_played_from_midi() {
        // Saved with its old live state, from before MIDI moved to the audio thread
        let json = r#"{
            "name": "Old", "version": 3,
            "nodes": [{"id": 1, "module_id": "input.midi_note", "position": [0.0, 0.0], "parameters": [
                {"name": "Note", "type": "Number", "value": 64.0},
                {"name": "Velocity", "type": "Number", "value": 100.0},
                {"name": "Aftertouch", "type": "Number", "value": 0.0},
                {"name": "Channel", "type": "Select", "value": 3},
                {"name": "Octave", "type": "Number", "value": 1.0},
                {"name": "Priority", "type": "Select", "value": 1},
                {"name": "Retrigger", "type": "Toggle", "value": true},
                {"name": "Bend Range", "type": "Number", "value": 7.0}]}],
            "connections": [],
            "midi_mappings": [
                {"cc_number": 1, "channel": 0, "node_id": 1, "param_index": 4, "param_name": "Bend Range",
                 "min_value": 0.0, "max_value": 12.0}
            ]
        }"#;
        let patch = patch_from_json(json).unwrap();
        let node = &patch.nodes[0];
        assert_eq!(node.module_id, crate::modules::note_in::ID);
        assert_eq!(param(node, "Source"), Some(&ParameterValue::Select(2)), "MIDI");
        assert_eq!(param(node, "Voices"), Some(&ParameterValue::Number(1.0)));
        assert_eq!(param(node, "Channel"), Some(&ParameterValue::Select(3)));
        assert_eq!(param(node, "Retrigger"), Some(&ParameterValue::Toggle(true)));
        assert!(param(node, "Key Velocity").is_none(), "its Velocity was the note's, not the keys'");
        assert!(param(node, "Note").is_none() && param(node, "Aftertouch").is_none());
        assert_eq!(patch.midi_mappings[0].param_index, crate::modules::NoteIn::PARAM_BEND_RANGE);
    }

    #[test]
    fn test_a_keyboard_in_a_group_is_migrated_and_poly_midi_is_left_alone() {
        let json = r#"{
            "name": "Old", "version": 6,
            "nodes": [{"id": 1, "module_id": "input.poly_midi", "position": [0.0, 0.0], "parameters": [
                {"name": "Voices", "type": "Number", "value": 4.0}]}],
            "connections": [],
            "groups": [{"id": 9, "name": "Voice", "position": [0.0, 0.0], "inputs": [], "outputs": [],
                "inputs_position": [0.0, 0.0], "outputs_position": [0.0, 0.0], "connections": [],
                "nodes": [{"id": 2, "module_id": "input.keyboard", "position": [0.0, 0.0], "parameters": []}]}]
        }"#;
        let patch = patch_from_json(json).unwrap();
        assert_eq!(patch.nodes[0].parameters, [NamedParameter::new("Voices", ParameterValue::Number(4.0))]);
        let inner = &patch.groups[0].nodes[0];
        assert_eq!(inner.module_id, crate::modules::note_in::ID);
        assert_eq!(param(inner, "Source"), Some(&ParameterValue::Select(1)));
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
